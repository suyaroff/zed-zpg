//! Running statements from `.sql` files and the `.pgres` result views.

use std::path::{Path, PathBuf};
use std::time::Instant;

use lsp_server::Connection;
use lsp_types::request::ShowMessageRequest;
use lsp_types::{
    CodeActionOrCommand, CodeLens, Command, Hover, HoverContents, MarkupContent, MarkupKind, MessageActionItem,
    MessageType, Position, Range, ShowMessageRequestParams,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zpg_core::header::display_ident;
use zpg_core::{
    Apply, Column, Issue, QualName, Query, Severity, SnapRow, Snapshot, catalog, history, sqlctx, sqlsplit, table,
};

use super::{
    CMD_EDIT_RESULT, CMD_EXPLAIN, CMD_RERUN, CMD_RUN, Doc, Kind, Res, Server, cell_at, command, encode_name, fnv, show, snapshot,
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
    let mut h = format!(
        "# zpg v1 result conn={} rows={rows}{} time={}ms",
        st.conn,
        if st.more { "+" } else { "" },
        st.millis
    );
    let first = st.sql.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let first: String = first.chars().take(200).collect();
    h += &format!("\n# {first}");
    if let Some(e) = &st.edit {
        h += &format!("\n# Rows of {}: code action “Edit rows in table” opens them for editing.", e.table);
    }
    if st.more {
        h += &format!("\n# Only the first {rows} rows are shown (lsp.zpg.settings.max_result_rows).");
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
    /// it, the only configured one, or (ambiguous) all of them.
    pub(super) fn conns_for(&self, text: &str, offset: usize) -> Vec<String> {
        if let Some(c) = sqlsplit::conn_before(text, offset) {
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
        for r in sqlsplit::split(&doc.text) {
            let conns = self.conns_for(&doc.text, r.start);
            let pos = offset_to_position(&doc.text, r.start);
            for c in &conns {
                let title = if conns.len() == 1 { format!("▶ Run on {c}") } else { format!("▶ {c}") };
                out.push(CodeLens {
                    range: Range::new(pos, pos),
                    command: Some(Command::new(title, CMD_RUN.into(), Some(run_args(&doc.text, key, r.start, r.end, c)))),
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
        let (a, b) = (position_to_offset(text, range.start), position_to_offset(text, range.end));
        if a < b && !text[a..b].trim().is_empty() {
            for c in self.conns_for(text, a) {
                let mut args = run_args(text, key, a, b, &c);
                args.push(Value::Bool(true));
                out.push(command(format!("zpg: Run selection on {c}"), CMD_RUN, args));
            }
        }
        let stmts = sqlsplit::split(text);
        let at = stmts
            .iter()
            .find(|r| r.start <= a && a <= r.end + 1)
            .or_else(|| stmts.iter().rev().find(|r| r.start <= a));
        if let Some(r) = at {
            for c in self.conns_for(text, r.start) {
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
        let RunTarget {
            start,
            end,
            conn: conn_name,
            hash,
            selection,
            confirmed,
        } = target;
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
        let ranges: Vec<_> = sqlsplit::split(&text[start..end])
            .into_iter()
            .map(|r| start + r.start..start + r.end)
            .collect();
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
            return self.confirm(conn, key, again, format!("zpg: {}. Run on {conn_name}?", dangers.join("; ")));
        }
        let (max_rows, timeout) = (self.settings.max_result_rows, self.settings.statement_timeout_ms);
        let client = match self.db.client(&self.settings, conn_name) {
            Ok(c) => c,
            Err(e) => return show(conn, MessageType::ERROR, format!("zpg: {e}")),
        };
        let started = Instant::now();
        let outcomes = match db::run(client, &stmts, read_only, max_rows, timeout, false) {
            Ok(o) => o,
            Err(e) => {
                let issue = error_issue(&text, ranges[e.stmt].clone(), e.position, e.message.clone());
                self.failed.insert(key.to_string(), issue);
                self.publish(conn, key)?;
                let first = e.message.lines().next().unwrap_or("");
                return show(conn, MessageType::ERROR, format!("zpg: {first} (rolled back)"));
            }
        };
        let millis = started.elapsed().as_millis();
        self.failed.remove(key);
        self.publish(conn, key)?;
        let tags: Vec<&str> = outcomes.iter().map(|o| o.tag.as_str()).collect();
        show(conn, MessageType::INFO, format!("zpg: {} ({millis} ms)", tags.join("; ")))?;
        self.record_history(conn, &path, conn_name, &stmts, &format!("{} · {millis} ms", tags.join("; ")))?;
        if tags.iter().any(|t| t.starts_with("CREATE") || t.starts_with("DROP") || t.starts_with("ALTER")) {
            self.schemas.clear();
            let catalogs: Vec<String> = self
                .docs
                .iter()
                .filter(|(_, d)| d.kind() == Kind::Catalog)
                .map(|(k, _)| k.clone())
                .collect();
            for k in catalogs {
                self.refresh_catalog(conn, &k)?;
            }
        }

        let Some((k, out)) = outcomes.iter().enumerate().rev().find(|(_, o)| !o.columns.is_empty()) else {
            return Ok(());
        };
        let edit = self
            .db
            .client(&self.settings, conn_name)
            .ok()
            .and_then(|c| db::edit_source(c, &out.columns));
        let st = ResultState {
            conn: conn_name.to_string(),
            sql: stmts[k].to_string(),
            columns: out.columns.clone(),
            edit,
            snap: result_snapshot(conn_name, &out.columns, &out.rows, self.settings.max_cell_width),
            more: out.more,
            millis,
        };
        let root = self.zpg_root(&path);
        state::ensure_root(&root)?;
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "query".into());
        let target = root
            .join(encode_name(conn_name))
            .join("results")
            .join(format!("{}.pgres", encode_name(&stem)));
        let rendered = render_result(&st);
        state::save(&target, &st)?;
        self.results.insert(target.clone(), st);
        self.write_file(conn, &target, rendered)?;
        self.show_document(conn, &target)
    }

    /// Asks before running `target`; the answer comes back as a response (see `answer`).
    fn confirm(&mut self, conn: &Connection, key: &str, target: RunTarget, message: String) -> Res<()> {
        let id = self.send_request::<ShowMessageRequest>(
            conn,
            ShowMessageRequestParams {
                typ: MessageType::WARNING,
                message,
                actions: Some(vec![
                    MessageActionItem {
                        title: RUN.into(),
                        properties: Default::default(),
                    },
                    MessageActionItem {
                        title: "Cancel".into(),
                        properties: Default::default(),
                    },
                ]),
            },
        )?;
        self.confirms.insert(id, (key.to_string(), target));
        Ok(())
    }

    /// The client's answer to a `confirm` question.
    pub(super) fn answer(&mut self, conn: &Connection, id: i32, result: Option<Value>) -> Res<()> {
        let Some((key, target)) = self.confirms.remove(&id) else {
            return Ok(());
        };
        let chosen = result.as_ref().and_then(|r| r.get("title")).and_then(Value::as_str);
        if chosen == Some(RUN) { self.run_sql(conn, &key, target) } else { Ok(()) }
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
        for r in ranges {
            let stmt = &text[r.clone()];
            if let [c] = self.conns_for(&text, r.start).as_slice()
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
    fn record_history(&mut self, conn: &Connection, from: &Path, conn_name: &str, stmts: &[&str], summary: &str) -> Res<()> {
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
        let client = match self.db.client(&self.settings, conn_name) {
            Ok(c) => c,
            Err(e) => return show(conn, MessageType::ERROR, format!("zpg: {e}")),
        };
        let started = Instant::now();
        let plan = match db::run(client, &[&sql], read_only, MAX_PLAN_LINES, timeout, analyze) {
            Ok(mut o) => o.remove(0).rows,
            Err(e) => return show(conn, MessageType::ERROR, format!("zpg: {} (rolled back)", e.message)),
        };
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
        let root = self.zpg_root(&path);
        state::ensure_root(&root)?;
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "query".into());
        let target = root
            .join(encode_name(conn_name))
            .join("results")
            .join(format!("{}.plan.txt", encode_name(&stem)));
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
        let client = match self.db.client(&self.settings, &st.conn) {
            Ok(c) => c,
            Err(e) => return show(conn, MessageType::ERROR, format!("zpg: {e}")),
        };
        let read_only = self.settings.connections.get(&st.conn).is_some_and(|c| c.read_only());
        let started = Instant::now();
        let out = match db::run(
            client,
            &[&st.sql],
            read_only,
            self.settings.max_result_rows,
            self.settings.statement_timeout_ms,
            false,
        ) {
            Ok(mut o) => o.remove(0),
            Err(e) => return show(conn, MessageType::ERROR, format!("zpg: {} (rolled back)", e.message)),
        };
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
        let query = Query {
            page: 1,
            filter,
            order: String::new(),
            limit: page.query.limit,
        };
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
        let target = root
            .join(encode_name(&conn_name))
            .join(format!("drop-{}.sql", table.file_stem()));
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
        let Some(doc) = self.docs.get(key) else {
            return Ok(());
        };
        let Some(path) = doc.path() else {
            return Ok(());
        };
        let (conn_name, table, target) = match (doc.kind(), line) {
            (Kind::Catalog, Some(line)) => {
                let Some((c, t)) = catalog::entry_at(&doc.text, line) else {
                    return Ok(());
                };
                let Some(root) = path.parent() else {
                    return Ok(());
                };
                let target = root.join(encode_name(&c)).join(format!("{}.ddl.sql", t.file_stem()));
                (c, t, target)
            }
            (Kind::Table, _) => {
                let Some(snap) = snapshot(&mut self.snaps, &path) else {
                    return Ok(());
                };
                (snap.conn.clone(), snap.table.clone(), path.with_extension("ddl.sql"))
            }
            _ => return Ok(()),
        };
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
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: md,
            }),
            range: None,
        })
    }
}

const MAX_LENS_FILE: usize = 5 << 20;
const MAX_PLAN_LINES: usize = 100_000;
/// Files scanned for dangerous statements on every change.
const MAX_DANGER_FILE: usize = 1 << 20;
const RUN: &str = "Run";
/// Files with more statements aren't checked: one round trip each.
const MAX_CHECKED: usize = 300;

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
/// started (it was edited itself); `None` rather than guess.
/// A selection must still hold exactly the same text.
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
    if valid(start, end) && fnv(&text[start..end]) == hash {
        return Some((start, end));
    }
    if selection {
        return None;
    }
    let stmts = sqlsplit::split(text);
    if let Some(r) = stmts
        .iter()
        .filter(|r| fnv(&text[r.start..r.end]) == hash)
        .min_by_key(|r| r.start.abs_diff(start))
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
            .map(|c| Column {
                name: c.name.clone(),
                type_name: c.type_name.clone(),
                not_null: false,
                has_default: false,
                generated: false,
                enum_values: vec![],
                comment: None,
            })
            .collect(),
        pk: vec![],
        foreign_keys: vec![],
        referenced_by: vec![],
        total: None,
        read_only: Some("query result; use “Edit rows in table” to change rows".into()),
        query: Query {
            page: 1,
            limit: rows.len() as u32,
            filter: String::new(),
            order: String::new(),
        },
        width,
        apply: Apply::Manual,
        visible: vec![],
        rows: rows
            .iter()
            .map(|values| SnapRow {
                xmin: None,
                values: values.clone(),
            })
            .collect(),
    }
}
