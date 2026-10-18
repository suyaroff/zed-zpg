//! Running statements from `.sql` files and the `.pgres` result views.

use std::path::{Path, PathBuf};
use std::time::Instant;

use lsp_server::Connection;
use lsp_types::request::ShowMessageRequest;
use lsp_types::{
    CodeActionOrCommand, CodeLens, Command, Hover, MessageActionItem, MessageType, Position, Range,
    ShowMessageRequestParams,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zpg_core::header::display_ident;
use zpg_core::{
    Apply, Column, Issue, QualName, Query, Severity, SnapRow, Snapshot, catalog, history, sqlctx, sqlsplit, table,
};

use super::page::cell_at;
use super::running;
use super::{
    CMD_EDIT_RESULT, CMD_EXPLAIN, CMD_RERUN, CMD_RUN, Doc, Kind, Res, Server, command, encode_name, fnv,
    markdown_hover, show, snapshot,
};
use crate::db::{self, EditSource, ResultCol};
use crate::state;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultState {
    pub conn: String,
    /// The statement whose rows are shown.
    pub sql: String,
    pub columns: Vec<ResultCol>,
    pub edit: Option<EditSource>,
    /// The rows, kept as a read-only snapshot so tables code can draw them.
    pub snap: Snapshot,
    pub more: bool,
    pub millis: u128,
}

pub fn offset_to_position(text: &str, offset: usize) -> Position {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() as u32;
    let col = before.rsplit('\n').next().unwrap_or("");
    Position::new(line, zpg_core::utf16_len(col))
}

pub fn position_to_offset(text: &str, pos: Position) -> usize {
    let mut offset = 0;
    for (i, line) in text.split('\n').enumerate() {
        if i as u32 == pos.line {
            let mut col = 0;
            for (b, ch) in line.char_indices() {
                if col >= pos.character {
                    return offset + b;
                }
                col += ch.len_utf16() as u32;
            }
            return offset + line.len();
        }
        offset += line.len() + 1;
    }
    text.len()
}

fn result_header(st: &ResultState) -> String {
    let rows = st.snap.rows.len();
    let mut h =
        format!("# zpg v1 result conn={} rows={rows}{} time={}ms", st.conn, if st.more { "+" } else { "" }, st.millis);
    let first = st.sql.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let first: String = first.chars().take(200).collect();
    h += &format!("\n# {first}");
    if let Some(e) = &st.edit {
        h += &format!("\n# Rows of {}: code action “Edit rows in table” opens them for editing.", e.table);
    }
    if st.more {
        h += &format!("\n# Only the first {rows} rows are shown (`max_result_rows` in the zpg settings).");
    }
    h
}

pub fn render_result(st: &ResultState) -> String {
    table::render(&result_header(st), &st.snap.column_names(), &st.snap.cells())
}

impl Server {
    /// The `.zpg` directory for a file: next to the closest project marker.
    pub(super) fn zpg_root(&self, file: &Path) -> PathBuf {
        let ancestors = || file.ancestors().skip(1);
        if let Some(a) = ancestors().find(|a| a.join(".zpg").is_dir()) {
            return a.join(".zpg");
        }
        if let Some(a) = ancestors().find(|a| a.join(".zed").is_dir()) {
            return a.join(".zpg");
        }
        match &self.root {
            Some(r) => r.join(".zpg"),
            None => file.parent().unwrap_or(file).join(".zpg"),
        }
    }

    /// Connections a statement at `offset` may run on: the `-- @conn` above
    /// it (of `directives`, see `sqlsplit::conn_directives`), the only
    /// configured one, or (ambiguous) all of them.
    pub(super) fn conns_for(&self, directives: &[(usize, String)], offset: usize) -> Vec<String> {
        if let Some(c) = sqlsplit::conn_at(directives, offset) {
            return vec![c];
        }
        self.settings.connections.keys().cloned().collect()
    }

    pub(super) fn code_lens(&self, key: &str) -> Vec<CodeLens> {
        let Some(doc) = self.docs.get(key).filter(|d| d.kind() == Kind::Sql) else {
            return vec![];
        };
        // A full dump has thousands of statements; code actions still run them.
        if doc.text.len() > MAX_LENS_FILE {
            return vec![];
        }
        let mut out = vec![];
        let directives = sqlsplit::conn_directives(&doc.text);
        for r in sqlsplit::split(&doc.text) {
            let conns = self.conns_for(&directives, r.start);
            let pos = offset_to_position(&doc.text, r.start);
            for c in &conns {
                let title = if conns.len() == 1 { format!("▶ Run on {c}") } else { format!("▶ {c}") };
                out.push(CodeLens {
                    range: Range::new(pos, pos),
                    command: Some(Command::new(
                        title,
                        CMD_RUN.into(),
                        Some(run_args(&doc.text, key, r.start, r.end, c)),
                    )),
                    data: None,
                });
            }
            if let [c] = conns.as_slice()
                && explainable(&doc.text[r.clone()])
            {
                out.push(CodeLens {
                    range: Range::new(pos, pos),
                    command: Some(Command::new(
                        "Explain".into(),
                        CMD_EXPLAIN.into(),
                        Some(explain_args(&doc.text, key, r.start, r.end, c, false)),
                    )),
                    data: None,
                });
            }
        }
        out
    }

    pub(super) fn sql_actions(&self, doc: &Doc, key: &str, range: Range) -> Vec<CodeActionOrCommand> {
        let text = &doc.text;
        let mut out = vec![];
        let directives = sqlsplit::conn_directives(text);
        let (a, b) = (position_to_offset(text, range.start), position_to_offset(text, range.end));
        if a < b && !text[a..b].trim().is_empty() {
            for c in self.conns_for(&directives, a) {
                let mut args = run_args(text, key, a, b, &c);
                args.push(Value::Bool(true));
                out.push(command(format!("zpg: Run selection on {c}"), CMD_RUN, args));
            }
        }
        let stmts = sqlsplit::split(text);
        let at =
            stmts.iter().find(|r| r.start <= a && a <= r.end + 1).or_else(|| stmts.iter().rev().find(|r| r.start <= a));
        if let Some(r) = at {
            for c in self.conns_for(&directives, r.start) {
                out.push(command(
                    format!("zpg: Run statement on {c}"),
                    CMD_RUN,
                    run_args(text, key, r.start, r.end, &c),
                ));
                if explainable(&text[r.clone()]) {
                    out.push(command(
                        format!("zpg: Explain statement on {c}"),
                        CMD_EXPLAIN,
                        explain_args(text, key, r.start, r.end, &c, false),
                    ));
                    out.push(command(
                        format!("zpg: Explain analyze statement on {c} (runs it, then rolls back)"),
                        CMD_EXPLAIN,
                        explain_args(text, key, r.start, r.end, &c, true),
                    ));
                }
            }
        }
        out
    }

    /// Runs the statements of `target` in the `.sql` buffer `key`.
    pub(super) fn run_sql(&mut self, conn: &Connection, key: &str, target: RunTarget) -> Res<()> {
        let RunTarget { start, end, conn: conn_name, hash, selection, confirmed } = target;
        let conn_name = conn_name.as_str();
        let Some(doc) = self.docs.get(key) else {
            return Ok(());
        };
        let text = doc.text.clone();
        let Some(path) = doc.path() else {
            return Ok(());
        };
        let Some((start, end)) = relocate(&text, start, end, hash, selection) else {
            return show(
                conn,
                MessageType::WARNING,
                "zpg: the file changed since this ▶ was drawn; nothing was run, click it again".into(),
            );
        };
        let ranges: Vec<_> =
            sqlsplit::split(&text[start..end]).into_iter().map(|r| start + r.start..start + r.end).collect();
        if ranges.is_empty() {
            return Ok(());
        }
        let stmts: Vec<&str> = ranges.iter().map(|r| &text[r.clone()]).collect();
        let read_only = self.settings.connections.get(conn_name).is_some_and(|c| c.read_only());
        let dangers: Vec<String> = stmts.iter().filter_map(|s| sqlctx::danger(s)).collect();
        if !dangers.is_empty() && !confirmed && !read_only && self.caps.message_actions {
            let again = RunTarget {
                start,
                end,
                conn: conn_name.to_string(),
                hash: Some(fnv(&text[start..end])),
                selection,
                confirmed: true,
            };
            let message = format!("zpg: {}. Run on {conn_name}?", dangers.join("; "));
            return self.confirm(conn, Pending::Run(key.to_string(), again), message);
        }
        let (max_rows, timeout) = (self.settings.max_result_rows, self.settings.statement_timeout_ms);
        let started = Instant::now();
        let ran = self.cancellable(conn, conn_name, &format!("Running on {conn_name}"), |c| {
            db::run(c, &stmts, read_only, max_rows, timeout, false)
        })?;
        // A failing statement is underlined, so the message needs only its first line.
        if let Ok(Err(e)) = &ran
            && !running::cancelled(&e.message)
        {
            let issue = error_issue(&text, ranges[e.stmt].clone(), e.position, e.message.clone());
            self.failed.insert(key.to_string(), issue);
            self.publish(conn, key)?;
            let first = e.message.lines().next().unwrap_or("");
            return show(conn, MessageType::ERROR, format!("zpg: {first} (rolled back)"));
        }
        let Some(outcomes) = self.outcomes(conn, ran)? else {
            return Ok(());
        };
        let millis = started.elapsed().as_millis();
        self.failed.remove(key);
        self.publish(conn, key)?;
        let tags: Vec<&str> = outcomes.iter().map(|o| o.tag.as_str()).collect();
        show(conn, MessageType::INFO, format!("zpg: {} ({millis} ms)", tags.join("; ")))?;
        self.record_history(conn, &path, conn_name, &stmts, &format!("{} · {millis} ms", tags.join("; ")))?;
        if tags.iter().any(|t| t.starts_with("CREATE") || t.starts_with("DROP") || t.starts_with("ALTER")) {
            self.schemas.clear();
            self.refresh_catalogs(conn)?;
        }

        let Some((k, out)) = outcomes.iter().enumerate().rev().find(|(_, o)| !o.columns.is_empty()) else {
            return Ok(());
        };
        let target = self.result_path(&path, conn_name, "pgres")?;
        self.show_result(conn, conn_name, stmts[k], out, millis, &target)
    }

    /// `.zpg/<conn>/results/<name of the .sql file>.<ext>` for what ran from `from`.
    fn result_path(&self, from: &Path, conn_name: &str, ext: &str) -> Res<PathBuf> {
        let root = self.zpg_root(from);
        state::ensure_root(&root)?;
        let stem = from.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "query".into());
        Ok(root.join(encode_name(conn_name)).join("results").join(format!("{}.{ext}", encode_name(&stem))))
    }

    /// The outcomes of a [`Self::cancellable`] `db::run`, or `None` after
    /// telling the user why there are none.
    pub(super) fn outcomes(
        &self,
        conn: &Connection,
        ran: Result<Result<Vec<db::Outcome>, db::RunError>, String>,
    ) -> Res<Option<Vec<db::Outcome>>> {
        let (typ, message) = match ran {
            Ok(Ok(o)) => return Ok(Some(o)),
            Ok(Err(e)) if running::cancelled(&e.message) => (MessageType::INFO, "zpg: cancelled (rolled back)".into()),
            Ok(Err(e)) => (MessageType::ERROR, format!("zpg: {} (rolled back)", e.message)),
            Err(e) => (MessageType::ERROR, format!("zpg: {e}")),
        };
        show(conn, typ, message)?;
        Ok(None)
    }

    /// Writes the rows of `sql` as the `.pgres` at `target` and opens it.
    pub(super) fn show_result(
        &mut self,
        conn: &Connection,
        conn_name: &str,
        sql: &str,
        out: &db::Outcome,
        millis: u128,
        target: &Path,
    ) -> Res<()> {
        let edit = self.db.client(&self.settings, conn_name).ok().and_then(|c| db::edit_source(c, &out.columns));
        let st = ResultState {
            conn: conn_name.to_string(),
            sql: sql.to_string(),
            columns: out.columns.clone(),
            edit,
            snap: result_snapshot(conn_name, &out.columns, &out.rows, self.settings.max_cell_width),
            more: out.more,
            millis,
        };
        let rendered = render_result(&st);
        state::save(target, &st)?;
        self.results.insert(target.to_path_buf(), st);
        self.write_file(conn, target, rendered)?;
        self.show_document(conn, target)
    }

    /// Asks before doing `pending`; the answer comes back as a response (see `answer`).
    pub(super) fn confirm(&mut self, conn: &Connection, pending: Pending, message: String) -> Res<()> {
        let id = self.send_request::<ShowMessageRequest>(
            conn,
            ShowMessageRequestParams {
                typ: MessageType::WARNING,
                message,
                actions: Some(vec![
                    MessageActionItem { title: RUN.into(), properties: Default::default() },
                    MessageActionItem { title: "Cancel".into(), properties: Default::default() },
                ]),
            },
        )?;
        self.confirms.insert(id, pending);
        Ok(())
    }

    /// The client's answer to a `confirm` question.
    pub(super) fn answer(&mut self, conn: &Connection, id: i32, result: Option<Value>) -> Res<()> {
        let Some(pending) = self.confirms.remove(&id) else {
            return Ok(());
        };
        if result.as_ref().and_then(|r| r.get("title")).and_then(Value::as_str) != Some(RUN) {
            return Ok(());
        }
        match pending {
            Pending::Run(key, target) => self.run_sql(conn, &key, target),
            Pending::Terminate(key, pid) => self.signal(conn, &key, pid, true, true),
        }
    }

    /// Underlines what Postgres refuses to parse in the `.sql` buffer `key`.
    /// Statements after DDL in the same file may name what it creates, so
    /// "does not exist" errors are dropped there.
    pub(super) fn check_sql(&mut self, key: &str) {
        self.checked.remove(key);
        let Some(doc) = self.docs.get(key).filter(|d| d.kind() == Kind::Sql) else {
            return;
        };
        if !self.settings.check_sql || doc.text.len() > MAX_DANGER_FILE {
            return;
        }
        let text = doc.text.clone();
        let ranges = sqlsplit::split(&text);
        if ranges.len() > MAX_CHECKED {
            return;
        }
        let mut issues = vec![];
        let mut unreachable: Vec<String> = vec![];
        let mut after_ddl = false;
        let directives = sqlsplit::conn_directives(&text);
        for r in ranges {
            let stmt = &text[r.clone()];
            if let [c] = self.conns_for(&directives, r.start).as_slice()
                && !unreachable.contains(c)
            {
                match self.db.client(&self.settings, c) {
                    Ok(client) => {
                        if let Err((code, e)) = db::check(client, stmt) {
                            let missing = matches!(code.as_str(), "42P01" | "42703" | "42883" | "42704" | "3F000");
                            // `$1` without a type is fine when the statement runs from code.
                            if !(after_ddl && missing) && code != "42P18" {
                                issues.push(error_issue(&text, r.clone(), e.position, e.message));
                            }
                        }
                    }
                    Err(_) => unreachable.push(c.clone()),
                }
            }
            after_ddl |= matches!(first_word(stmt).as_str(), "create" | "alter" | "drop" | "comment" | "do");
        }
        self.checked.insert(key.to_string(), issues);
    }

    /// Warnings for statements that change every row or drop data.
    pub(super) fn danger_issues(text: &str) -> Vec<Issue> {
        if text.len() > MAX_DANGER_FILE {
            return vec![];
        }
        sqlsplit::split(text)
            .into_iter()
            .filter_map(|r| {
                let message = sqlctx::danger(&text[r.clone()])?;
                let s = offset_to_position(text, r.start);
                let line_end = text[r.start..r.end].find('\n').map_or(r.end, |n| r.start + n);
                let e = offset_to_position(text, line_end);
                Some(Issue {
                    line: s.line,
                    start: s.character,
                    end: e.character.max(s.character + 1),
                    severity: Severity::Warning,
                    message,
                })
            })
            .collect()
    }

    /// Adds statements that ran fine to `.zpg/history.sql`.
    fn record_history(
        &mut self,
        conn: &Connection,
        from: &Path,
        conn_name: &str,
        stmts: &[&str],
        summary: &str,
    ) -> Res<()> {
        let root = self.zpg_root(from);
        state::ensure_root(&root)?;
        let target = root.join("history.sql");
        let old = match self.doc_key(&target) {
            Some(k) => self.docs[&k].text.clone(),
            None => std::fs::read_to_string(&target).unwrap_or_default(),
        };
        let time = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let text = history::append(&old, &history::entry(&time, conn_name, summary, stmts), history::MAX_LEN);
        self.write_file(conn, &target, text)
    }

    /// Runs `EXPLAIN` (with `analyze`: `EXPLAIN (ANALYZE, BUFFERS)` in a
    /// transaction that is rolled back) and opens the plan as text.
    pub(super) fn explain(&mut self, conn: &Connection, key: &str, target: RunTarget, analyze: bool) -> Res<()> {
        let Some(doc) = self.docs.get(key) else {
            return Ok(());
        };
        let text = doc.text.clone();
        let Some(path) = doc.path() else {
            return Ok(());
        };
        let Some((start, end)) = relocate(&text, target.start, target.end, target.hash, target.selection) else {
            return show(
                conn,
                MessageType::WARNING,
                "zpg: the file changed since this was drawn; nothing was run, try again".into(),
            );
        };
        let stmt = text[start..end].trim();
        let sql = if analyze { format!("EXPLAIN (ANALYZE, BUFFERS) {stmt}") } else { format!("EXPLAIN {stmt}") };
        let conn_name = target.conn.as_str();
        let read_only = self.settings.connections.get(conn_name).is_some_and(|c| c.read_only());
        let timeout = self.settings.statement_timeout_ms;
        let started = Instant::now();
        let ran = self.cancellable(conn, conn_name, &format!("Explaining on {conn_name}"), |c| {
            db::run(c, &[&sql], read_only, MAX_PLAN_LINES, timeout, analyze)
        })?;
        let Some(mut outcomes) = self.outcomes(conn, ran)? else {
            return Ok(());
        };
        let plan = outcomes.remove(0).rows;
        let millis = started.elapsed().as_millis();
        let first = stmt.lines().next().unwrap_or("");
        let mut out = format!(
            "-- EXPLAIN{} on {conn_name} · {millis} ms{}\n-- {}\n\n",
            if analyze { " ANALYZE" } else { "" },
            if analyze { " · rolled back" } else { "" },
            first.chars().take(200).collect::<String>(),
        );
        for row in plan {
            out += row.first().and_then(|v| v.as_deref()).unwrap_or("");
            out.push('\n');
        }
        let target = self.result_path(&path, conn_name, "plan.txt")?;
        self.write_file(conn, &target, out)?;
        self.show_document(conn, &target)
    }

    pub(super) fn result_state(&mut self, path: &Path) -> Option<&ResultState> {
        if !self.results.contains_key(path) {
            let st: ResultState = state::load(path)?;
            self.results.insert(path.to_path_buf(), st);
        }
        self.results.get(path)
    }

    pub(super) fn rerun(&mut self, conn: &Connection, key: &str) -> Res<()> {
        let Some(path) = self.docs.get(key).and_then(Doc::path) else {
            return Ok(());
        };
        let Some(st) = self.result_state(&path).cloned() else {
            return show(conn, MessageType::ERROR, "zpg: no saved state for this result".into());
        };
        let read_only = self.settings.connections.get(&st.conn).is_some_and(|c| c.read_only());
        let (max_rows, timeout) = (self.settings.max_result_rows, self.settings.statement_timeout_ms);
        let started = Instant::now();
        let ran = self.cancellable(conn, &st.conn, &format!("Running on {}", st.conn), |c| {
            db::run(c, &[&st.sql], read_only, max_rows, timeout, false)
        })?;
        let Some(mut outcomes) = self.outcomes(conn, ran)? else {
            return Ok(());
        };
        let out = outcomes.remove(0);
        let millis = started.elapsed().as_millis();
        self.record_history(conn, &path, &st.conn, &[&st.sql], &format!("{} · {millis} ms", out.tag))?;
        let new = ResultState {
            snap: result_snapshot(&st.conn, &out.columns, &out.rows, self.settings.max_cell_width),
            columns: out.columns,
            more: out.more,
            millis,
            ..st
        };
        let rendered = render_result(&new);
        state::save(&path, &new)?;
        self.results.insert(path.clone(), new);
        show(conn, MessageType::INFO, format!("zpg: {}", out.tag))?;
        self.replace_doc(conn, key, rendered)?;
        self.publish(conn, key)
    }

    /// Opens the result's rows as a page of their table, filtered by the query.
    pub(super) fn edit_result(&mut self, conn: &Connection, key: &str) -> Res<()> {
        let Some(path) = self.docs.get(key).and_then(Doc::path) else {
            return Ok(());
        };
        let Some(st) = self.result_state(&path).cloned() else {
            return Ok(());
        };
        let Some(edit) = &st.edit else {
            return show(
                conn,
                MessageType::WARNING,
                "zpg: only results from one table that include its primary key can be edited".into(),
            );
        };
        let root = self.zpg_root(&path);
        let target = match self.open_table(&root, &st.conn, &edit.table) {
            Ok(p) => p,
            Err(e) => return show(conn, MessageType::ERROR, format!("zpg: {e}")),
        };
        if self.has_local_edits(&target) {
            show(
                conn,
                MessageType::WARNING,
                format!("zpg: {} has changes not written yet; opened as it is", edit.table),
            )?;
            return self.show_document(conn, &target);
        }
        let Some(page) = snapshot(&mut self.snaps, &target).cloned() else {
            return Ok(());
        };
        let Some(sql) = sqlsplit::one_line(&st.sql) else {
            return show(
                conn,
                MessageType::WARNING,
                "zpg: the query has a string spanning lines and can't go into the table header".into(),
            );
        };
        let table_keys: Vec<String> = edit.pk.iter().map(|(t, _)| display_ident(t)).collect();
        let result_keys: Vec<String> = edit.pk.iter().map(|(_, r)| display_ident(r)).collect();
        let sub = format!("SELECT {} FROM ({sql}) AS zpg_q", result_keys.join(", "));
        let filter = if table_keys.len() == 1 {
            format!("{} IN ({sub})", table_keys[0])
        } else {
            format!("({}) IN ({sub})", table_keys.join(", "))
        };
        let query = Query { filter, ..Query::first(page.query.limit) };
        let d = self.defaults(&st.conn, query, page.width, page.apply, edit.columns.clone());
        let loaded = match self.load_table(&target, &st.conn, &edit.table, &d) {
            Ok(s) => s,
            Err(e) => return show(conn, MessageType::ERROR, format!("zpg: {e}")),
        };
        self.write_file(conn, &target, loaded.render())?;
        self.show_document(conn, &target)
    }

    /// Writes a `DROP` for the catalog entry on `line` into a `.sql` file and
    /// opens it: dropping takes a look and a click on ▶, never one action.
    pub(super) fn prepare_drop(&mut self, conn: &Connection, key: &str, line: u32) -> Res<()> {
        let Some(doc) = self.docs.get(key) else {
            return Ok(());
        };
        let Some((conn_name, table)) = catalog::entry_at(&doc.text, line) else {
            return Ok(());
        };
        let kind = catalog::kind_at(&doc.text, line).unwrap_or_default();
        let Some(root) = doc.path().and_then(|p| p.parent().map(Path::to_path_buf)) else {
            return Ok(());
        };
        let target = root.join(encode_name(&conn_name)).join(format!("drop-{}.sql", table.file_stem()));
        let text = format!(
            "-- @conn {conn_name}\n\
             -- Review, then run with ▶ (or ctrl-. → Run statement). This can't be undone.\n\
             -- Add CASCADE to also drop views and foreign keys that depend on it.\n\
             {};\n",
            catalog::drop_statement(&kind, &table)
        );
        self.write_file(conn, &target, text)?;
        self.show_document(conn, &target)
    }

    /// Opens the relation's DDL as a `.sql` next to its page; `line` is the
    /// catalog line when called from the catalog.
    pub(super) fn show_ddl(&mut self, conn: &Connection, key: &str, line: Option<u32>) -> Res<()> {
        let Some((conn_name, table, dir)) = self.relation_at(key, line) else {
            return Ok(());
        };
        let target = dir.join(format!("{}.ddl.sql", table.file_stem()));
        let ddl = match self.db.client(&self.settings, &conn_name).and_then(|c| db::ddl(c, &table)) {
            Ok(d) => d,
            Err(e) => return show(conn, MessageType::ERROR, format!("zpg: {e}")),
        };
        let text = format!(
            "-- @conn {conn_name}\n\
             -- {table}, rebuilt by zpg from the system catalogs (owners, grants and storage options are left out).\n\n\
             {ddl}"
        );
        self.write_file(conn, &target, text)?;
        self.show_document(conn, &target)
    }

    pub(super) fn result_actions(&mut self, key: &str, path: &Path) -> Vec<CodeActionOrCommand> {
        let arg = || vec![Value::String(key.to_string())];
        let mut out = vec![command("zpg: Run again".into(), CMD_RERUN, arg())];
        if self.result_state(path).is_some_and(|s| s.edit.is_some()) {
            out.insert(0, command("zpg: Edit rows in table".into(), CMD_EDIT_RESULT, arg()));
        }
        out
    }

    pub(super) fn result_issues(&mut self, text: &str, path: &Path) -> Vec<Issue> {
        let Some(st) = self.result_state(path) else {
            return vec![];
        };
        // Line 0 is a result header, not a table header.
        let parsed = table::parse(text);
        let mut issues: Vec<Issue> = parsed.issues.iter().filter(|i| i.line > 0).cloned().collect();
        issues.extend(zpg_core::diff::diff(&parsed, &st.snap).issues);
        issues
    }

    pub(super) fn result_hover(&mut self, text: &str, path: &Path, pos: Position) -> Option<Hover> {
        let st = self.result_state(path)?;
        let line = text.split('\n').nth(pos.line as usize)?;
        let i = cell_at(line, pos.character)?;
        let col = st.columns.get(i)?;
        let mut md = format!("**{}** `{}`", col.name, col.type_name);
        let parsed = table::parse(text);
        if let Some(row) = parsed.rows.iter().position(|r| r.line == pos.line)
            && let Some(r) = st.snap.rows.get(row)
            && let Some(v) = &r.values[i]
        {
            let cut = st.snap.cell(i, &r.values[i], &st.snap.widths()).1;
            md += &super::values::value_md(v, &col.type_name, cut);
        }
        Some(markdown_hover(md))
    }
}

const MAX_LENS_FILE: usize = 5 << 20;
const MAX_PLAN_LINES: usize = 100_000;
/// Files scanned for dangerous statements on every change.
const MAX_DANGER_FILE: usize = 1 << 20;
const RUN: &str = "Run";
/// Files with more statements aren't checked: one round trip each.
const MAX_CHECKED: usize = 300;

/// What waits for the user's *Run*.
pub(super) enum Pending {
    /// Running statements of a `.sql` buffer.
    Run(String, RunTarget),
    /// Terminating a backend listed in a `.pgres`.
    Terminate(String, i32),
}

/// Arguments of the run command: bytes `start..end` of the buffer, and the
/// hash of the text they held when the command was made, so that a stale
/// command can find its statement again (see `relocate`).
#[derive(Clone)]
pub(super) struct RunTarget {
    start: usize,
    end: usize,
    conn: String,
    hash: Option<u64>,
    selection: bool,
    /// The user already agreed to run a dangerous statement.
    confirmed: bool,
}

impl RunTarget {
    pub(super) fn from_args(args: &[Value]) -> Option<Self> {
        Some(Self {
            start: args.get(1)?.as_u64()? as usize,
            end: args.get(2)?.as_u64()? as usize,
            conn: args.get(3)?.as_str()?.to_string(),
            hash: args.get(4).and_then(Value::as_str).and_then(|h| u64::from_str_radix(h, 16).ok()),
            selection: args.get(5).and_then(Value::as_bool).unwrap_or(false),
            confirmed: false,
        })
    }
}

fn first_word(stmt: &str) -> String {
    stmt.split(|c: char| !c.is_alphanumeric()).find(|w| !w.is_empty()).unwrap_or("").to_lowercase()
}

/// Statements EXPLAIN accepts.
fn explainable(stmt: &str) -> bool {
    matches!(
        first_word(stmt).as_str(),
        "select" | "insert" | "update" | "delete" | "with" | "values" | "table" | "merge" | "execute"
    )
}

/// An error Postgres reported at 1-based character `position` of the
/// statement at bytes `r` of `text`, underlining the word there.
fn error_issue(text: &str, r: std::ops::Range<usize>, position: Option<usize>, message: String) -> Issue {
    // Past the last character means "at end of input": point at the end.
    let at = match position {
        Some(p) => text[r.clone()]
            .char_indices()
            .nth(p.saturating_sub(1))
            .map_or(r.end.saturating_sub(1).max(r.start), |(b, _)| r.start + b),
        None => r.start,
    };
    let word_end = text[at..r.end]
        .find(|c: char| c.is_whitespace() || c == ',' || c == ')' || c == '(')
        .map_or(r.end, |n| at + n.max(1));
    let (s, t) = (offset_to_position(text, at), offset_to_position(text, word_end.max(at + 1).min(r.end)));
    let end_col = if t.line == s.line { t.character } else { s.character + 1 };
    Issue::error(s.line, s.character, end_col.max(s.character + 1), message)
}

fn explain_args(text: &str, key: &str, start: usize, end: usize, conn: &str, analyze: bool) -> Vec<Value> {
    let mut args = run_args(text, key, start, end, conn);
    args.extend([Value::Bool(false), Value::Bool(analyze)]);
    args
}

fn run_args(text: &str, key: &str, start: usize, end: usize, conn: &str) -> Vec<Value> {
    vec![
        Value::String(key.to_string()),
        Value::from(start),
        Value::from(end),
        Value::String(conn.to_string()),
        Value::String(format!("{:016x}", fnv(&text[start..end]))),
    ]
}

/// Where the statement a command was made for is now. Offsets in a code
/// lens go stale as soon as the buffer is edited, so: the same text if it's
/// still there (the nearest copy), else the statement that starts where it
/// started (it was edited itself); `None` rather than guess. The same text
/// must be a whole statement: what was `… id = 1` may now be the start of
/// `… id = 10`. A selection must still hold exactly the same text.
pub(super) fn relocate(
    text: &str,
    start: usize,
    end: usize,
    hash: Option<u64>,
    selection: bool,
) -> Option<(usize, usize)> {
    let valid = |s: usize, e: usize| s <= e && e <= text.len() && text.is_char_boundary(s) && text.is_char_boundary(e);
    let Some(hash) = hash else {
        return valid(start, end).then_some((start, end));
    };
    if selection {
        return (valid(start, end) && fnv(&text[start..end]) == hash).then_some((start, end));
    }
    let stmts = sqlsplit::split(text);
    if let Some(r) = stmts.iter().filter(|r| fnv(&text[r.start..r.end]) == hash).min_by_key(|r| r.start.abs_diff(start))
    {
        return Some((r.start, r.end));
    }
    stmts.iter().find(|r| r.start == start).map(|r| (r.start, r.end))
}

/// Result rows as a read-only snapshot, so they're drawn like a table page.
fn result_snapshot(conn: &str, cols: &[ResultCol], rows: &[Vec<Option<String>>], width: u32) -> Snapshot {
    Snapshot {
        conn: conn.to_string(),
        table: QualName::new("", ""),
        columns: cols
            .iter()
            .map(|c| Column { name: c.name.clone(), type_name: c.type_name.clone(), ..Default::default() })
            .collect(),
        pk: vec![],
        foreign_keys: vec![],
        referenced_by: vec![],
        total: None,
        read_only: Some("query result; use “Edit rows in table” to change rows".into()),
        query: Query::first(rows.len() as u32),
        width,
        apply: Apply::Manual,
        visible: vec![],
        rows: rows.iter().map(|values| SnapRow { xmin: None, values: values.clone() }).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_and_positions() {
        let text = "ab\n🐈x\n";
        for (offset, pos) in [(0, (0, 0)), (2, (0, 2)), (3, (1, 0)), (7, (1, 2)), (8, (1, 3)), (9, (2, 0))] {
            assert_eq!(offset_to_position(text, offset), Position::new(pos.0, pos.1), "{offset}");
            assert_eq!(position_to_offset(text, Position::new(pos.0, pos.1)), offset, "{pos:?}");
        }
        assert_eq!(position_to_offset(text, Position::new(0, 99)), 2);
        assert_eq!(position_to_offset(text, Position::new(9, 0)), text.len());
    }

    #[test]
    fn stale_commands_find_their_statement() {
        let text = "select 1;\nselect 2;\n";
        let hash = |s: &str| Some(fnv(s));
        let (s2, e2) = (10, 18);
        assert_eq!(relocate(text, s2, e2, hash("select 2"), false), Some((s2, e2)));
        // Text added above: the same statement, moved.
        let moved = format!("-- note\n{text}");
        assert_eq!(relocate(&moved, s2, e2, hash("select 2"), false), Some((s2 + 8, e2 + 8)));
        // The statement itself was edited: the one starting where it started.
        let edited = "select 1;\nselect 22;\n";
        assert_eq!(relocate(edited, s2, e2, hash("select 2"), false), Some((s2, e2 + 1)));
        assert_eq!(relocate(edited, s2, e2 + 1, hash("select 2;"), true), None);
        // A selection keeps its exact text, whatever the statements are.
        assert_eq!(relocate(text, 0, 13, hash("select 1;\nsel"), true), Some((0, 13)));
        assert_eq!(relocate(text, 3, 99, None, false), None);
    }

    #[test]
    fn errors_underline_the_word_at_their_position() {
        let text = "select 1;\nselect nope from t;";
        let r = 10..text.len() - 1;
        let at = |p| {
            let i = error_issue(text, r.clone(), p, "e".into());
            (i.line, i.start, i.end)
        };
        assert_eq!(at(Some(8)), (1, 7, 11));
        assert_eq!(at(None), (1, 0, 6));
        assert_eq!(at(Some(999)), (1, 17, 18));
    }

    #[test]
    fn explainable_statements() {
        assert!(explainable("  (select 1)"));
        assert!(explainable("WITH x AS (select 1) select * from x"));
        assert!(!explainable("create table t ()"));
    }
}
