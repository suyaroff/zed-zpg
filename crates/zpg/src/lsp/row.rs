//! `.pgr` row views: one row, one field per line.

use std::path::{Path, PathBuf};

use lsp_server::Connection;
use lsp_types::{CodeActionOrCommand, CompletionItem, MessageType, Position, Range};
use serde_json::Value;
use zpg_core::cell::Value as CellValue;
use zpg_core::{Apply, Severity, Snapshot, record};

use super::page::{backref_actions, cell_texts, column_md};
use super::uri::{row_file, rows_dir, table_of_row};
use super::{CMD_APPLY, CMD_REFRESH, Res, Server, command, complete, show, snapshot, values};
use crate::db;

impl Server {
    /// True when the row view has edits not yet in the database, saved or not
    /// (with `apply=manual` a saved file can still hold them).
    fn row_has_local_edits(&mut self, path: &Path) -> bool {
        let text = match self.doc_key(path) {
            Some(k) if !self.docs[&k].is_clean() => return true,
            Some(k) => self.docs[&k].text.clone(),
            None => match std::fs::read_to_string(path) {
                Ok(t) => t,
                Err(_) => return false,
            },
        };
        let Some(snap) = snapshot(&mut self.snaps, path) else {
            return false;
        };
        let parsed = record::parse(&text, snap);
        !parsed.issues.is_empty() || !record::diff(&parsed, snap).ops.is_empty()
    }

    /// Writes the row's `.pgr` (unless it holds edits not yet in the database) and returns its path.
    pub(super) fn open_row(&mut self, table_path: &Path, snap: &Snapshot, row: usize) -> Result<PathBuf, String> {
        let key = snap.pk.iter().map(|&k| snap.rows[row].values[k].clone()).collect::<Vec<_>>();
        let path = rows_dir(table_path).join(row_file(&key));
        if self.row_has_local_edits(&path) {
            return Ok(path);
        }
        let fetched = self
            .db
            .read(&self.settings, &snap.conn, |c| db::fetch_row(c, snap, &key, true))?
            .ok_or("the row no longer exists; refresh the table")?;
        let row_snap = Snapshot { rows: vec![fetched], width: 0, ..snap.clone() };
        let label = snap
            .pk
            .iter()
            .zip(&key)
            .map(|(&k, v)| format!("{}={}", snap.columns[k].name, v.as_deref().unwrap_or("NULL")))
            .collect::<Vec<_>>()
            .join(",");
        self.pending_writes.push((path.clone(), record::render(&row_snap, &label)));
        self.store(&path, row_snap)?;
        Ok(path)
    }

    pub(super) fn refresh_row(&mut self, conn: &Connection, key: &str, path: &Path) -> Res<()> {
        let Some(snap) = snapshot(&mut self.snaps, path).cloned() else {
            return show(conn, MessageType::ERROR, "zpg: no saved state; open the row from its table".into());
        };
        let Some(row) = snap.rows.first() else {
            return Ok(());
        };
        let key_values: Vec<Option<String>> = snap.pk.iter().map(|&k| row.values[k].clone()).collect();
        let label = record::RowHeader::parse(self.docs[key].text.lines().next().unwrap_or(""))
            .map(|h| h.key)
            .unwrap_or_default();
        let fetched = match self.db.read(&self.settings, &snap.conn, |c| db::fetch_row(c, &snap, &key_values, true)) {
            Ok(Some(r)) => r,
            Ok(None) => return show(conn, MessageType::WARNING, "zpg: the row no longer exists".into()),
            Err(e) => return show(conn, MessageType::ERROR, format!("zpg: {e}")),
        };
        let new = Snapshot { rows: vec![fetched], ..snap };
        let text = record::render(&new, &label);
        self.store(path, new)?;
        self.failed.remove(key);
        self.replace_doc(conn, key, text)?;
        self.publish(conn, key)
    }

    /// Like `save_table`; a row view follows the `apply` mode of its table.
    pub(super) fn save_row(&mut self, conn: &Connection, key: &str, force: bool) -> Res<()> {
        let Some(doc) = self.docs.get(key) else {
            return Ok(());
        };
        let Some(path) = doc.path() else {
            return Ok(());
        };
        let text = doc.text.clone();
        let Some(snap) = snapshot(&mut self.snaps, &path).cloned() else {
            return self.publish(conn, key);
        };
        let parsed = record::parse(&text, &snap);
        let d = record::diff(&parsed, &snap);
        if !parsed.issues.is_empty() || d.issues.iter().any(|i| i.severity == Severity::Error) {
            self.publish(conn, key)?;
            return show(conn, MessageType::WARNING, "zpg: not written to the database, fix the errors first".into());
        }
        if d.ops.is_empty() || (snap.apply == Apply::Manual && !force) {
            return self.publish(conn, key);
        }
        if self.run_diff(conn, key, &snap, &d)?.is_none() {
            return Ok(());
        }
        show(conn, MessageType::INFO, "zpg: saved 1 update".into())?;
        self.refresh_row(conn, key, &path)?;
        self.refresh_tokens(conn)?;
        self.sync_table_row(conn, &path, &snap)
    }

    /// After a row view was saved: update that row in its table page, unless
    /// the page has edits of its own.
    fn sync_table_row(&mut self, conn: &Connection, row_path: &Path, row_snap: &Snapshot) -> Res<()> {
        let Some(table_path) = table_of_row(row_path) else {
            return Ok(());
        };
        if self.has_local_edits(&table_path) {
            return Ok(());
        }
        let Some(mut table) = snapshot(&mut self.snaps, &table_path).cloned() else {
            return Ok(());
        };
        let Some(row) = row_snap.rows.first() else {
            return Ok(());
        };
        let key: Vec<Option<String>> = row_snap.pk.iter().map(|&k| row.values[k].clone()).collect();
        let Some(i) = table.rows.iter().position(|r| table.pk.iter().zip(&key).all(|(&k, v)| &r.values[k] == v)) else {
            return Ok(());
        };
        let fetched = self.db.read(&self.settings, &table.conn, |c| db::fetch_row(c, &table, &key, false));
        if let Ok(Some(r)) = fetched {
            table.rows[i] = r;
            let text = table.render();
            self.store(&table_path, table)?;
            self.write_file(conn, &table_path, text)?;
        }
        Ok(())
    }

    /// F12 on a foreign key field opens the row it points at.
    pub(super) fn row_definition(&mut self, path: &Path, text: &str, line: u32) -> Result<Option<PathBuf>, String> {
        let Some(snap) = snapshot(&mut self.snaps, path).cloned() else {
            return Ok(None);
        };
        let parsed = record::parse(text, &snap);
        let Some(field) = parsed.field_at(line) else {
            return Ok(None);
        };
        let Some(fk) = snap.foreign_keys.iter().find(|f| f.columns.contains(&field.col)) else {
            return Ok(None);
        };
        let Some(values) = cell_texts(&fk.columns, |c| parsed.value_of(c)) else {
            return Ok(None);
        };
        let Some(table_path) = table_of_row(path) else {
            return Ok(None);
        };
        self.follow_fk(&table_path, &snap, fk, &values).map(Some)
    }

    /// The field under `pos`: its column, value and the row a foreign key points at.
    pub(super) fn row_hover(&mut self, path: &Path, text: &str, pos: Position) -> Option<String> {
        let snap = snapshot(&mut self.snaps, path)?.clone();
        let parsed = record::parse(text, &snap);
        let field = parsed.field_at(pos.line)?;
        let mut md = column_md(&snap, field.col);
        if let Ok(CellValue::Text(v)) = &field.value {
            md += &values::value_md(v, &snap.columns[field.col].type_name, false);
        }
        if let Some(fk) = snap.foreign_keys.iter().find(|f| f.columns.contains(&field.col))
            && let Some(values) = cell_texts(&fk.columns, |c| parsed.value_of(c))
        {
            md += &self.referenced_md(&snap, fk, &values);
        }
        Some(md)
    }

    pub(super) fn row_completion(&mut self, path: &Path, text: &str, pos: Position) -> Vec<CompletionItem> {
        let Some(snap) = snapshot(&mut self.snaps, path) else {
            return vec![];
        };
        let parsed = record::parse(text, snap);
        let Some(f) = parsed.field_at(pos.line).filter(|f| f.line == pos.line) else {
            return vec![];
        };
        let range = Range::new(Position::new(pos.line, f.start), Position::new(pos.line, f.end.max(pos.character)));
        complete::values(snap, f.col, range)
    }

    pub(super) fn row_actions(&mut self, key: &str, path: &Path, text: &str) -> Vec<CodeActionOrCommand> {
        let arg = || Value::String(key.to_string());
        let mut out = vec![];
        if let Some(snap) = snapshot(&mut self.snaps, path) {
            let parsed = record::parse(text, snap);
            let d = record::diff(&parsed, snap);
            if parsed.issues.is_empty() && !d.ops.is_empty() && d.issues.is_empty() {
                out.push(command("zpg: Apply changes (1 update)".into(), CMD_APPLY, vec![arg()]));
            }
        }
        out.push(command("zpg: Refresh row (discard changes)".into(), CMD_REFRESH, vec![arg()]));
        if self.caps.show_document
            && let Some(snap) = snapshot(&mut self.snaps, path)
        {
            out.extend(backref_actions(snap, key, 0));
        }
        out
    }
}
