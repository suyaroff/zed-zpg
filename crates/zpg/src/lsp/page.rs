//! `.pgt` table pages: loading, paging, saving, and what the cursor is on.

use std::path::{Path, PathBuf};

use lsp_server::Connection;
use lsp_types::{CodeActionKind, CodeActionOrCommand, CompletionItem, MessageType, Position, Range, TextEdit, Uri};
use serde_json::Value;
use zpg_core::cell::{self, Value as CellValue};
use zpg_core::diff::{self, Diff, Source};
use zpg_core::header::{display_ident, encode_name, quote_literal};
use zpg_core::table::{self, Parsed};
use zpg_core::{Apply, ForeignKey, Issue, QualName, Query, Severity, Snapshot, Total, record, sql, sqlctx};

use super::uri::table_of_row;
use super::{
    CMD_APPLY, CMD_BACKREF, CMD_BACKUP, CMD_DDL, CMD_FILTER, CMD_OPEN_ROW, CMD_PAGE, CMD_PREVIEW, CMD_REFRESH,
    CMD_SORT, Kind, Res, Server, command, complete, edit_action, export, line_range, show, snapshot, values,
};
use crate::db::{self, Defaults};
use crate::state;

/// How `refresh` changes the page it reloads.
pub(super) enum Reload {
    Same,
    Page(u32),
    /// A new `order`, from page 1.
    Order(String),
    /// A new `where`, from page 1.
    Filter(String),
}

/// Index of the cell under UTF-16 column `ch`.
pub(super) fn cell_at(line: &str, ch: u32) -> Option<usize> {
    table::split(line).iter().rposition(|c| c.start <= ch + 1)
}

/// Column shown in the cell under UTF-16 column `ch`.
fn column_at(snap: &Snapshot, line: &str, ch: u32) -> Option<usize> {
    snap.shown().get(cell_at(line, ch)?).copied()
}

/// The snapshot row a buffer line stands for, matched by its key cells.
fn row_of_line(snap: &Snapshot, line: &str) -> Option<usize> {
    let cells = table::split(line);
    let shown = snap.shown();
    if cells.len() != shown.len() || snap.pk.is_empty() {
        return None;
    }
    let widths = snap.widths();
    snap.rows.iter().position(|r| {
        snap.pk.iter().all(|&k| {
            shown
                .iter()
                .position(|&c| c == k)
                .is_some_and(|cell| snap.cell(k, &r.values[k], &widths).0 == cells[cell].text)
        })
    })
}

/// Value of column `col` on a buffer line: the cell when shown, else the
/// loaded value of that row.
fn line_value(snap: &Snapshot, line: &str, col: usize) -> Option<CellValue> {
    let cells = table::split(line);
    let shown = snap.shown();
    if cells.len() != shown.len() {
        return None;
    }
    match shown.iter().position(|&c| c == col) {
        Some(k) => cell::parse(&cells[k].text).ok(),
        None => row_of_line(snap, line).map(|r| CellValue::from_db(snap.rows[r].values[col].as_deref())),
    }
}

/// The text values of columns `cols`; `None` when any is NULL or missing.
pub(super) fn cell_texts(cols: &[usize], value_of: impl Fn(usize) -> Option<CellValue>) -> Option<Vec<String>> {
    cols.iter()
        .map(|&c| match value_of(c)? {
            CellValue::Text(s) => Some(s),
            _ => None,
        })
        .collect()
}

pub(super) fn column_md(snap: &Snapshot, i: usize) -> String {
    let col = &snap.columns[i];
    let mut md = format!("**{}** `{}`", col.name, col.type_name);
    if snap.pk.contains(&i) {
        md += " · primary key";
    }
    if col.not_null {
        md += " · not null";
    }
    if col.generated {
        md += " · generated";
    } else if col.has_default {
        md += " · has default";
    }
    for fk in snap.foreign_keys.iter().filter(|f| f.columns.contains(&i)) {
        md += &format!(" · → {}({})", fk.table, fk.ref_columns.join(", "));
    }
    if let Some(c) = &col.comment {
        md += &format!("\n\n{c}");
    }
    let refs: Vec<String> = snap
        .referenced_by
        .iter()
        .filter(|b| b.ref_columns.contains(&i))
        .map(|b| format!("{}({})", b.table, b.columns.join(", ")))
        .collect();
    if !refs.is_empty() {
        md += &format!("\n\n← referenced by {}", refs.join(", "));
    }
    md
}

/// Longest value offered as a filter.
const MAX_FILTER_VALUE: usize = 200;

/// *Filter* actions for the cell at `ch` of a data `line`: rows with the
/// same value or a different one (or NULL / not NULL), added to `where`.
fn filter_actions(snap: &Snapshot, key: &str, line: &str, ch: u32) -> Vec<CodeActionOrCommand> {
    let Some(i) = column_at(snap, line, ch) else {
        return vec![];
    };
    // The loaded value, uncut; without a key, the cell if it isn't cut.
    let value = match row_of_line(snap, line) {
        Some(r) => Some(snap.rows[r].values[i].clone()),
        None => match line_value(snap, line, i) {
            Some(CellValue::Text(s)) if !snap.cell(i, &Some(s.clone()), &snap.widths()).1 => Some(Some(s)),
            Some(CellValue::Null) => Some(None),
            _ => None,
        },
    };
    let Some(value) = value else {
        return vec![];
    };
    let col = &snap.columns[i];
    let name = display_ident(&col.name);
    // Types without `=`: compare their text.
    let lhs = match col.type_name.as_str() {
        "json" | "xml" | "point" | "line" | "lseg" | "box" | "path" | "polygon" | "circle" => format!("{name}::text"),
        _ => name.clone(),
    };
    let conds: Vec<(String, String)> = match &value {
        None => vec![
            (format!("{name} IS NULL"), format!("{name} IS NULL")),
            (format!("{name} IS NOT NULL"), format!("{name} IS NOT NULL")),
        ],
        Some(v) if v.chars().count() <= MAX_FILTER_VALUE && !v.contains('\n') => {
            let shown: String = v.chars().take(30).collect();
            let shown = if shown.len() < v.len() { format!("{shown}…") } else { shown };
            vec![
                (format!("{lhs} = {}", quote_literal(v)), format!("{name} = '{shown}'")),
                (format!("{lhs} <> {}", quote_literal(v)), format!("{name} <> '{shown}'")),
            ]
        }
        Some(_) => vec![],
    };
    let old = snap.query.filter.trim();
    conds
        .into_iter()
        .map(|(cond, title)| {
            let filter = match old {
                "" => cond,
                o if sqlctx::has_word(o, "or") => format!("({o}) AND {cond}"),
                o => format!("{o} AND {cond}"),
            };
            let verb = if old.is_empty() { "Filter" } else { "Also filter" };
            command(
                format!("zpg: {verb}: {title}"),
                CMD_FILTER,
                vec![Value::String(key.to_string()), Value::String(filter)],
            )
        })
        .collect()
}

/// *Referencing rows* actions for the row on `line` (any line of a row view).
pub(super) fn backref_actions(snap: &Snapshot, key: &str, line: u32) -> Vec<CodeActionOrCommand> {
    snap.referenced_by
        .iter()
        .enumerate()
        .map(|(k, b)| {
            command(
                format!("zpg: Rows of {} referencing this ({})", b.table, b.columns.join(", ")),
                CMD_BACKREF,
                vec![Value::String(key.to_string()), Value::from(line), Value::from(k)],
            )
        })
        .collect()
}

/// Inserts a template row below the cursor's row (or after the last row).
fn insert_row(uri: &Uri, parsed: &Parsed, snap: &Snapshot, cursor_line: u32) -> CodeActionOrCommand {
    let after = parsed
        .rows
        .iter()
        .rev()
        .find(|r| r.line <= cursor_line)
        .or(parsed.rows.last())
        .map(|r| r.line)
        .or(parsed.columns.as_ref().map(|c| c.line + 1))
        .unwrap_or(0);
    let row = format!("| {} |\n", snap.insert_template().join(" | "));
    edit_action(
        "zpg: Insert row".into(),
        CodeActionKind::REFACTOR,
        uri,
        vec![TextEdit::new(Range::new(Position::new(after + 1, 0), Position::new(after + 1, 0)), row)],
    )
}

impl Server {
    pub(super) fn defaults(
        &self,
        conn: &str,
        query: Query,
        width: u32,
        apply: Apply,
        visible: Vec<String>,
    ) -> Defaults {
        Defaults {
            query,
            width,
            apply,
            visible,
            read_only_conn: self.settings.connections.get(conn).is_some_and(|c| c.read_only()),
            total: None,
        }
    }

    /// Loads the table's shape and a page into a snapshot stored for `path`.
    pub(super) fn load_table(
        &mut self,
        path: &Path,
        conn: &str,
        table: &QualName,
        d: &Defaults,
    ) -> Result<Snapshot, String> {
        let client = self.db.client(&self.settings, conn)?;
        let snap = db::load(client, conn, table, d)?;
        self.store(path, snap.clone())?;
        Ok(snap)
    }

    /// Path of the table's `.pgt` under the `.zpg` dir `root`, loading it unless it's already there.
    pub(super) fn open_table(&mut self, root: &Path, conn: &str, table: &QualName) -> Result<PathBuf, String> {
        let path = root.join(encode_name(conn)).join(format!("{}.pgt", table.file_stem()));
        if path.exists() && snapshot(&mut self.snaps, &path).is_some() {
            return Ok(path);
        }
        let query = Query::first(self.settings.page_size);
        let d = self.defaults(conn, query, self.settings.max_cell_width, self.settings.apply.into(), vec![]);
        let snap = self.load_table(&path, conn, table, &d)?;
        state::atomic_write(&path, snap.render().as_bytes())?;
        Ok(path)
    }

    /// True when the file is open with edits that aren't in the database yet.
    pub(super) fn has_local_edits(&mut self, path: &Path) -> bool {
        let Some(key) = self.doc_key(path) else {
            return false;
        };
        let doc = &self.docs[&key];
        if !doc.is_clean() {
            return true;
        }
        let parsed = table::parse(&doc.text);
        match snapshot(&mut self.snaps, path) {
            Some(snap) => parsed.has_errors() || !diff::diff(&parsed, snap).ops.is_empty(),
            None => false,
        }
    }

    /// Opens the referenced table filtered to the referenced row.
    pub(super) fn follow_fk(
        &mut self,
        from: &Path,
        snap: &Snapshot,
        fk: &ForeignKey,
        values: &[String],
    ) -> Result<PathBuf, String> {
        self.open_filtered(from, &snap.conn, &fk.table, &fk.ref_columns, values)
    }

    /// Opens the rows of `referenced_by[k]` that point at the row on `line`
    /// (of a table page; any line of a row view).
    pub(super) fn referencing(&mut self, key: &str, line: u32, k: usize) -> Result<Option<PathBuf>, String> {
        let Some(doc) = self.docs.get(key) else {
            return Ok(None);
        };
        let Some(path) = doc.path() else {
            return Ok(None);
        };
        let (kind, text) = (doc.kind(), doc.text.clone());
        let Some(snap) = snapshot(&mut self.snaps, &path).cloned() else {
            return Ok(None);
        };
        let Some(b) = snap.referenced_by.get(k) else {
            return Ok(None);
        };
        let (from, values) = match kind {
            Kind::Table => {
                let l = text.split('\n').nth(line as usize).unwrap_or("");
                (path.clone(), cell_texts(&b.ref_columns, |c| line_value(&snap, l, c)))
            }
            Kind::Row => {
                let parsed = record::parse(&text, &snap);
                let Some(from) = table_of_row(&path) else {
                    return Ok(None);
                };
                (from, cell_texts(&b.ref_columns, |c| parsed.value_of(c)))
            }
            _ => return Ok(None),
        };
        let Some(values) = values else {
            return Err("the key of this row is empty".into());
        };
        self.open_filtered(&from, &snap.conn, &b.table, &b.columns, &values).map(Some)
    }

    /// Opens `table` of `conn` filtered to `columns` = `values`; `from` is a page under `.zpg/<conn>/`.
    fn open_filtered(
        &mut self,
        from: &Path,
        conn: &str,
        table: &QualName,
        columns: &[String],
        values: &[String],
    ) -> Result<PathBuf, String> {
        let root = from.parent().and_then(Path::parent).ok_or("table file is not inside .zpg/<conn>/")?.to_path_buf();
        let path = self.open_table(&root, conn, table)?;
        if self.has_local_edits(&path) {
            return Ok(path);
        }
        let Some(target) = snapshot(&mut self.snaps, &path).cloned() else {
            return Ok(path);
        };
        let filter = columns
            .iter()
            .zip(values)
            .map(|(c, v)| format!("{} = {}", display_ident(c), quote_literal(v)))
            .collect::<Vec<_>>()
            .join(" AND ");
        let query = Query { page: 1, filter, ..target.query.clone() };
        let d = self.defaults(&target.conn, query, target.width, target.apply, target.visible.clone());
        let loaded = self.load_table(&path, &target.conn, &target.table, &d)?;
        self.pending_writes.push((path.clone(), loaded.render()));
        Ok(path)
    }

    /// Reloads a catalog, table page (another page or order per `how`) or row.
    pub(super) fn refresh(&mut self, conn: &Connection, key: &str, how: Reload) -> Res<()> {
        let Some(doc) = self.docs.get(key) else {
            return Ok(());
        };
        let kind = doc.kind();
        let Some(path) = doc.path() else {
            return Ok(());
        };
        let text = doc.text.clone();
        match kind {
            Kind::Catalog => return self.refresh_catalog(conn, key),
            Kind::Row => return self.refresh_row(conn, key, &path),
            Kind::Result => return self.rerun(conn, key),
            Kind::Table => {}
            Kind::Sql | Kind::Other => return Ok(()),
        }
        let parsed = table::parse(&text);
        let old = snapshot(&mut self.snaps, &path).cloned();
        if !matches!(how, Reload::Same)
            && let Some(old) = &old
            && !parsed.has_errors()
            && !diff::diff(&parsed, old).ops.is_empty()
        {
            return show(conn, MessageType::WARNING, "zpg: apply or discard changes first".into());
        }
        // The header wins, so a refresh also works when the saved state is gone.
        let (conn_name, table, mut query, width, apply, visible) = match (&parsed.header, &old) {
            (Some(h), _) => (h.conn.clone(), h.table.clone(), h.query.clone(), h.width, h.apply, h.columns.clone()),
            (None, Some(o)) => (o.conn.clone(), o.table.clone(), o.query.clone(), o.width, o.apply, o.visible.clone()),
            (None, None) => return show(conn, MessageType::ERROR, "zpg: fix the header line first".into()),
        };
        let page = match how {
            Reload::Same => None,
            Reload::Page(p) => Some(p.max(1)),
            Reload::Order(o) => {
                query.order = o;
                Some(1)
            }
            Reload::Filter(f) => {
                query.filter = f;
                Some(1)
            }
        };
        if let Some(p) = page {
            query.page = p;
        }
        let mut d = self.defaults(&conn_name, query, width, apply, visible);
        // Paging and sorting don't change how many rows match.
        if page.is_some()
            && let Some(o) = &old
            && o.table == table
            && o.query.filter == d.query.filter
        {
            d.total = o.total;
        }
        let snap = match self.load_table(&path, &conn_name, &table, &d) {
            Ok(s) => s,
            Err(e) => return show(conn, MessageType::ERROR, format!("zpg: {e}")),
        };
        if page.is_some_and(|p| p > 1) && snap.rows.is_empty() {
            if let Some(o) = old {
                self.store(&path, o)?;
            }
            return show(conn, MessageType::INFO, "zpg: no more rows".into());
        }
        self.failed.remove(key);
        self.replace_doc(conn, key, snap.render())?;
        self.publish(conn, key)
    }

    /// Records a failed write and reports it.
    fn fail(&mut self, conn: &Connection, key: &str, line: u32, message: &str) -> Res<()> {
        let r = line_range(&self.docs[key].text, line);
        self.failed
            .insert(key.to_string(), Issue::error(line, 0, r.end.character.max(1), format!("not saved: {message}")));
        show(conn, MessageType::ERROR, format!("zpg: nothing written, transaction rolled back: {message}"))?;
        self.publish(conn, key)
    }

    /// Runs `d` against the database; `None` after reporting a failure.
    pub(super) fn run_diff(
        &mut self,
        conn: &Connection,
        key: &str,
        snap: &Snapshot,
        d: &Diff,
    ) -> Res<Option<Vec<Option<zpg_core::SnapRow>>>> {
        let client = match self.db.client(&self.settings, &snap.conn) {
            Ok(c) => c,
            Err(e) => {
                show(conn, MessageType::ERROR, format!("zpg: {e}"))?;
                return Ok(None);
            }
        };
        match db::apply(client, snap, d) {
            Ok(rows) => Ok(Some(rows)),
            Err(e) => {
                let line = d.ops[e.op].line().unwrap_or(0);
                self.fail(conn, key, line, &e.message)?;
                Ok(None)
            }
        }
    }

    /// Writes the buffer's changes to the database. In `apply=manual` mode only
    /// when `force` (the Apply command).
    pub(super) fn save_table(&mut self, conn: &Connection, key: &str, force: bool) -> Res<()> {
        let Some(doc) = self.docs.get(key) else {
            return Ok(());
        };
        let Some(path) = doc.path() else {
            return Ok(());
        };
        let parsed = table::parse(&doc.text);
        let Some(snap) = snapshot(&mut self.snaps, &path).cloned() else {
            return self.publish(conn, key);
        };
        let not_saved = |conn: &Connection| {
            show(conn, MessageType::WARNING, "zpg: not written to the database, fix the errors first".into())
        };
        let Some(header) = parsed.header.clone().filter(|_| !parsed.has_errors()) else {
            self.publish(conn, key)?;
            return not_saved(conn);
        };
        if header.conn != snap.conn || header.table != snap.table || !snap.unknown_visible(&header.columns).is_empty() {
            self.publish(conn, key)?;
            return not_saved(conn);
        }
        let d = diff::diff(&parsed, &snap);
        if d.issues.iter().any(|i| i.severity == Severity::Error) {
            self.publish(conn, key)?;
            return not_saved(conn);
        }

        let mut new = snap.clone();
        new.apply = header.apply;
        let mut summary = None;
        if !d.ops.is_empty() {
            if header.apply == Apply::Manual && !force {
                if new.apply != snap.apply {
                    self.store(&path, new)?;
                }
                return self.publish(conn, key);
            }
            let Some(rows) = self.run_diff(conn, key, &snap, &d)? else {
                return Ok(());
            };
            new.rows = d
                .order
                .iter()
                .filter_map(|s| match *s {
                    Source::Snapshot(i) => Some(snap.rows[i].clone()),
                    Source::Op(k) => rows[k].clone(),
                })
                .collect();
            summary = Some(d.summary());
            if let Some(t) = new.total {
                let (_, inserted, deleted) = d.counts();
                let n = (t.rows() + inserted as u64).saturating_sub(deleted as u64);
                // Inserted rows may not match the filter.
                new.total = Some(match t {
                    Total::Exact(_) if snap.query.filter.trim().is_empty() => Total::Exact(n),
                    _ => Total::Estimate(n),
                });
            }
        }

        // `width` and `columns` only change how the loaded rows are drawn.
        new.width = header.width;
        new.visible = header.columns.clone();
        if header.query != snap.query {
            let fetched = self.db.client(&self.settings, &snap.conn).and_then(|c| db::fetch(c, &new, &header.query));
            match fetched {
                Ok(rows) => {
                    if header.query.filter != snap.query.filter
                        && let Ok(c) = self.db.client(&self.settings, &snap.conn)
                    {
                        new.total = db::count(c, &new, &header.query);
                    }
                    new.query = header.query.clone();
                    new.rows = rows;
                }
                Err(e) => show(conn, MessageType::ERROR, format!("zpg: {e}"))?,
            }
        }

        let text = new.render();
        self.store(&path, new)?;
        self.failed.remove(key);
        self.replace_doc(conn, key, text)?;
        if let Some(s) = summary {
            show(conn, MessageType::INFO, format!("zpg: saved {s}"))?;
        }
        self.refresh_tokens(conn)?;
        self.publish(conn, key)
    }

    pub(super) fn preview(&mut self, conn: &Connection, key: &str) -> Res<()> {
        let Some(doc) = self.docs.get(key) else {
            return Ok(());
        };
        let Some(path) = doc.path() else {
            return Ok(());
        };
        let parsed = table::parse(&doc.text);
        let Some(snap) = snapshot(&mut self.snaps, &path) else {
            return Ok(());
        };
        let d = diff::diff(&parsed, snap);
        let mut out = String::from("-- What saving would run, in one transaction.\nBEGIN;\n");
        for op in &d.ops {
            out += &sql::inline(&sql::for_op(snap, op));
            out += ";\n";
        }
        out += "COMMIT;\n";
        let target = path.with_extension("preview.sql");
        state::atomic_write(&target, out.as_bytes())?;
        self.show_document(conn, &target)
    }

    /// The row `fk` points at, as markdown.
    pub(super) fn referenced_md(&mut self, snap: &Snapshot, fk: &ForeignKey, values: &[String]) -> String {
        let fetched = self.db.client(&self.settings, &snap.conn).and_then(|c| db::referenced(c, snap, fk, values));
        match fetched {
            Ok(Some(fields)) => {
                let mut md = format!("\n\n**→ {}** (F12 opens it)\n\n", fk.table);
                for (name, v) in fields.iter().take(20) {
                    let (text, _) = cell::render_cut(&CellValue::from_db(v.as_deref()), 80);
                    md += &format!("- `{name}`: {}\n", text.replace('`', "'"));
                }
                if fields.len() > 20 {
                    md += &format!("- … {} more\n", fields.len() - 20);
                }
                md
            }
            Ok(None) => format!("\n\n**→ {}**: no such row", fk.table),
            Err(e) => format!("\n\n**→ {}**: {e}", fk.table),
        }
    }

    /// F12 on a table page: the row a foreign key cell points at, else the row view of the line.
    pub(super) fn table_definition(
        &mut self,
        path: &Path,
        text: &str,
        pos: Position,
    ) -> Result<Option<PathBuf>, String> {
        let Some(snap) = snapshot(&mut self.snaps, path).cloned() else {
            return Ok(None);
        };
        if pos.line == 0 {
            return Ok(None);
        }
        let line = text.split('\n').nth(pos.line as usize).unwrap_or("");
        let Some(i) = column_at(&snap, line, pos.character) else {
            return Ok(None);
        };
        if let Some(fk) = snap.foreign_keys.iter().find(|f| f.columns.contains(&i)) {
            let values = cell_texts(&fk.columns, |c| line_value(&snap, line, c));
            return match values {
                Some(v) => self.follow_fk(path, &snap, fk, &v).map(Some),
                None => Ok(None),
            };
        }
        match row_of_line(&snap, line) {
            Some(r) => self.open_row(path, &snap, r).map(Some),
            None => Ok(None),
        }
    }

    /// The cell under `pos`: its column, value and the row a foreign key points at.
    pub(super) fn table_hover(&mut self, path: &Path, text: &str, pos: Position) -> Option<String> {
        let snap = snapshot(&mut self.snaps, path)?.clone();
        let line = text.split('\n').nth(pos.line as usize)?;
        if pos.line == 0 {
            return None;
        }
        let i = column_at(&snap, line, pos.character)?;
        let mut md = column_md(&snap, i);
        if pos.line > 1
            && let Some(r) = row_of_line(&snap, line)
            && let Some(v) = &snap.rows[r].values[i]
        {
            let cut = snap.cell(i, &snap.rows[r].values[i], &snap.widths()).1;
            md += &values::value_md(v, &snap.columns[i].type_name, cut);
        }
        if let Some(fk) = snap.foreign_keys.iter().find(|f| f.columns.contains(&i))
            && let Some(values) = cell_texts(&fk.columns, |c| line_value(&snap, line, c))
        {
            md += &self.referenced_md(&snap, fk, &values);
        }
        Some(md)
    }

    /// Header fields on line 0, values that fit the column in a cell.
    pub(super) fn table_completion(&mut self, path: &Path, text: &str, pos: Position) -> Vec<CompletionItem> {
        let Some(snap) = snapshot(&mut self.snaps, path) else {
            return vec![];
        };
        let line = text.split('\n').nth(pos.line as usize).unwrap_or("");
        if pos.line == 0 {
            return complete::header(line, pos, snap);
        }
        let cells = table::split(line);
        let Some(i) = cell_at(line, pos.character) else {
            return vec![];
        };
        let Some(&col) = snap.shown().get(i) else {
            return vec![];
        };
        let c = &cells[i];
        let range = if pos.character >= c.start && pos.character <= c.end.max(c.start) {
            Range::new(Position::new(pos.line, c.start), Position::new(pos.line, c.end.max(pos.character)))
        } else {
            Range::new(pos, pos)
        };
        complete::values(snap, col, range)
    }

    pub(super) fn table_actions(
        &mut self,
        uri: &Uri,
        path: &Path,
        text: &str,
        range: Range,
    ) -> Vec<CodeActionOrCommand> {
        let key = uri.as_str();
        let arg = || Value::String(key.to_string());
        let mut out = vec![];

        let parsed = table::parse(text);
        let Some(snap) = snapshot(&mut self.snaps, path) else {
            return out;
        };
        let d = (!parsed.has_errors()).then(|| diff::diff(&parsed, snap));
        let pending = d.as_ref().is_some_and(|d| !d.ops.is_empty());
        let clean = d.as_ref().is_some_and(|d| !d.issues.iter().any(|i| i.severity == Severity::Error));

        let on_row = self.caps.show_document
            && range.start.line > 0
            && text.split('\n').nth(range.start.line as usize).is_some_and(|line| row_of_line(snap, line).is_some());
        if on_row {
            out.push(command("zpg: Open row".into(), CMD_OPEN_ROW, vec![arg(), Value::from(range.start.line)]));
        }
        if let Some(cols) = &parsed.columns
            && cols.line == range.start.line
            && let Some(line) = text.split('\n').nth(cols.line as usize)
            && let Some(i) = column_at(snap, line, range.start.character)
        {
            let name = zpg_core::header::display_ident(&snap.columns[i].name);
            let current = parsed.header.as_ref().map_or(snap.query.order.clone(), |h| h.query.order.clone());
            for (arrow, order) in [("↑", name.clone()), ("↓", format!("{name} desc"))] {
                if current.trim() != order {
                    out.push(command(
                        format!("zpg: Sort by {name} {arrow}"),
                        CMD_SORT,
                        vec![arg(), Value::from(order)],
                    ));
                }
            }
        }
        if snap.read_only.is_none() {
            out.push(insert_row(uri, &parsed, snap, range.start.line));
        }
        if pending && clean {
            let summary = d.as_ref().map(|d| d.summary()).unwrap_or_default();
            out.push(command(format!("zpg: Apply changes ({summary})"), CMD_APPLY, vec![arg()]));
            if self.caps.show_document {
                out.push(command("zpg: Preview SQL".into(), CMD_PREVIEW, vec![arg()]));
            }
        }
        if !pending
            && parsed.rows.iter().any(|r| r.line == range.start.line)
            && let Some(line) = text.split('\n').nth(range.start.line as usize)
        {
            out.extend(filter_actions(snap, key, line, range.start.character));
        }
        if !pending && !snap.query.filter.trim().is_empty() {
            out.push(command("zpg: Clear filter (where=)".into(), CMD_FILTER, vec![arg(), Value::from("")]));
        }
        if !pending {
            let page = parsed.header.as_ref().map_or(snap.query.page, |h| h.query.page);
            let pages = snap.total.map(|t| t.pages(snap.query.limit));
            let of = match (pages, snap.total) {
                (Some(n), Some(Total::Estimate(_))) => format!("/~{n}"),
                (Some(n), _) => format!("/{n}"),
                _ => String::new(),
            };
            let mut go = |title: &str, p: u64| {
                out.push(command(format!("zpg: {title} ({p}{of})"), CMD_PAGE, vec![arg(), Value::from(p)]));
            };
            let more = snap.rows.len() as u32 >= snap.query.limit;
            if more {
                go("Next page", u64::from(page) + 1);
            }
            if page > 1 {
                go("Previous page", u64::from(page) - 1);
            }
            if page > 2 {
                go("First page", 1);
            }
            if let Some(n) = pages.filter(|&n| more && n > u64::from(page) + 1) {
                go("Last page", n);
            }
        }
        if self.caps.show_document {
            out.push(command("zpg: Show DDL".into(), CMD_DDL, vec![arg()]));
            out.push(command(format!("zpg: Back up {} (pg_dump)", snap.table), CMD_BACKUP, vec![arg()]));
            out.extend(export::actions(key, Kind::Table));
        }
        out.push(command(
            if pending { "zpg: Refresh (discard changes)".into() } else { "zpg: Refresh".into() },
            CMD_REFRESH,
            vec![arg()],
        ));
        if let Some(h) = &parsed.header {
            let mut toggled = h.clone();
            toggled.apply = match h.apply {
                Apply::Save => Apply::Manual,
                Apply::Manual => Apply::Save,
            };
            let title = match toggled.apply {
                Apply::Manual => "zpg: Apply changes manually (apply=manual)",
                Apply::Save => "zpg: Apply changes on save (apply=save)",
            };
            out.push(edit_action(
                title.into(),
                CodeActionKind::REFACTOR,
                uri,
                vec![TextEdit::new(line_range(text, 0), toggled.render())],
            ));
        }
        // Last: a much-referenced table has dozens.
        if on_row {
            out.extend(backref_actions(snap, key, range.start.line));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zpg_core::{Column, SnapRow};

    fn snap(filter: &str) -> Snapshot {
        let col = |n: &str| Column { name: n.into(), type_name: "text".into(), ..Default::default() };
        Snapshot {
            conn: "c".into(),
            table: QualName::new("public", "t"),
            columns: vec![col("id"), col("city")],
            pk: vec![0],
            foreign_keys: vec![],
            referenced_by: vec![],
            read_only: None,
            query: Query { filter: filter.into(), ..Query::first(100) },
            width: 40,
            apply: Apply::Manual,
            visible: vec![],
            total: None,
            rows: vec![SnapRow { xmin: Some("1".into()), values: vec![Some("1".into()), Some("it's".into())] }],
        }
    }

    /// (title, new `where`) of each filter action on the `city` cell.
    fn filters(filter: &str) -> Vec<(String, String)> {
        let s = snap(filter);
        let line = "| 1  | it's |";
        filter_actions(&s, "k", line, line.find("it's").unwrap() as u32)
            .into_iter()
            .map(|a| match a {
                CodeActionOrCommand::CodeAction(a) => {
                    let args = a.command.and_then(|c| c.arguments).unwrap_or_default();
                    (a.title, args[1].as_str().unwrap_or_default().to_string())
                }
                CodeActionOrCommand::Command(c) => (c.title, String::new()),
            })
            .collect()
    }

    #[test]
    fn filter_by_cell() {
        let f = filters("");
        assert_eq!(f[0], ("zpg: Filter: city = 'it's'".into(), "city = 'it''s'".into()));
        assert_eq!(f[1].1, "city <> 'it''s'");
        assert_eq!(
            filters("id > 0")[0],
            ("zpg: Also filter: city = 'it's'".into(), "id > 0 AND city = 'it''s'".into())
        );
        assert_eq!(filters("id = 1 or(id = 2)")[0].1, "(id = 1 or(id = 2)) AND city = 'it''s'");
        assert_eq!(filters("note = 'x or y'")[0].1, "note = 'x or y' AND city = 'it''s'");
    }
}
