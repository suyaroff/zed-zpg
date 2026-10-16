//! Running statements from `.sql` files and the `.pgres` result views.

use std::path::{Path, PathBuf};
use std::time::Instant;

use lsp_server::Connection;
use lsp_types::{
    CodeActionOrCommand, CodeLens, Command, Hover, HoverContents, MarkupContent, MarkupKind, MessageType, Position,
    Range,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zpg_core::header::display_ident;
use zpg_core::{Apply, Column, Issue, QualName, Query, SnapRow, Snapshot, catalog, sqlsplit, table};

use super::{
    CMD_EDIT_RESULT, CMD_RERUN, CMD_RUN, Doc, Kind, Res, Server, cell_at, command, encode_name, fnv, show, snapshot,
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
    fn conns_for(&self, text: &str, offset: usize) -> Vec<String> {
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
        let (max_rows, timeout) = (self.settings.max_result_rows, self.settings.statement_timeout_ms);
        let client = match self.db.client(&self.settings, conn_name) {
            Ok(c) => c,
            Err(e) => return show(conn, MessageType::ERROR, format!("zpg: {e}")),
        };
        let started = Instant::now();
        let outcomes = match db::run(client, &stmts, read_only, max_rows, timeout) {
            Ok(o) => o,
            Err(e) => {
                let r = &ranges[e.stmt];
                // Past the last character means "at end of input": point at the end.
                let at = match e.position {
                    Some(p) => text[r.clone()]
                        .char_indices()
                        .nth(p.saturating_sub(1))
                        .map_or(r.end.saturating_sub(1).max(r.start), |(b, _)| r.start + b),
                    None => r.start,
                };
                let word_end = text[at..r.end]
                    .find(|c: char| c.is_whitespace() || c == ',' || c == ')' || c == '(')
                    .map_or(r.end, |n| at + n.max(1));
                let (s, t) = (offset_to_position(&text, at), offset_to_position(&text, word_end.max(at + 1).min(r.end)));
                let end_col = if t.line == s.line { t.character } else { s.character + 1 };
                self.failed.insert(
                    key.to_string(),
                    Issue::error(s.line, s.character, end_col.max(s.character + 1), e.message.clone()),
                );
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
        if tags.iter().any(|t| t.starts_with("CREATE") || t.starts_with("DROP") || t.starts_with("ALTER")) {
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

    fn result_state(&mut self, path: &Path) -> Option<&ResultState> {
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
        ) {
            Ok(mut o) => o.remove(0),
            Err(e) => return show(conn, MessageType::ERROR, format!("zpg: {} (rolled back)", e.message)),
        };
        let new = ResultState {
            snap: result_snapshot(&st.conn, &out.columns, &out.rows, self.settings.max_cell_width),
            columns: out.columns,
            more: out.more,
            millis: started.elapsed().as_millis(),
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

/// Arguments of the run command: bytes `start..end` of the buffer, and the
/// hash of the text they held when the command was made, so that a stale
/// command can find its statement again (see `relocate`).
pub(super) struct RunTarget {
    start: usize,
    end: usize,
    conn: String,
    hash: Option<u64>,
    selection: bool,
}

impl RunTarget {
    pub(super) fn from_args(args: &[Value]) -> Option<Self> {
        Some(Self {
            start: args.get(1)?.as_u64()? as usize,
            end: args.get(2)?.as_u64()? as usize,
            conn: args.get(3)?.as_str()?.to_string(),
            hash: args.get(4).and_then(Value::as_str).and_then(|h| u64::from_str_radix(h, 16).ok()),
            selection: args.get(5).and_then(Value::as_bool).unwrap_or(false),
        })
    }
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
            })
            .collect(),
        pk: vec![],
        foreign_keys: vec![],
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
