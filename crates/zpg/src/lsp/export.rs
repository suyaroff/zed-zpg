//! Exporting a table page or a query result as CSV, JSON or INSERT statements.

use std::fs::File;

use lsp_server::Connection;
use lsp_types::{CodeActionOrCommand, MessageType};
use serde_json::Value;
use zpg_core::export;

use super::{CMD_EXPORT, Kind, Res, Server, command, encode_name, running, show, snapshot};
use crate::db;
use crate::state;

/// Bigger exports are written but not opened in the editor.
const MAX_OPEN: u64 = 20 << 20;

/// *Export* actions for a table page or a result.
pub(super) fn actions(key: &str, kind: Kind) -> Vec<CodeActionOrCommand> {
    let formats: &[(&str, &str)] = match kind {
        Kind::Table => &[
            ("zpg: Export page as CSV", "csv"),
            ("zpg: Export page as JSON", "json"),
            ("zpg: Export page as INSERT statements", "sql"),
            ("zpg: Export all rows matching where= as CSV", "csv-all"),
        ],
        Kind::Result => &[("zpg: Export result as CSV", "csv"), ("zpg: Export result as JSON", "json")],
        _ => &[],
    };
    formats
        .iter()
        .map(|(title, f)| command(title.to_string(), CMD_EXPORT, vec![Value::String(key.to_string()), Value::from(*f)]))
        .collect()
}

impl Server {
    /// Writes `.zpg/<conn>/exports/<name>.<ext>` and opens it. Pages export
    /// what was loaded (not unsaved edits) in the shown columns.
    pub(super) fn export(&mut self, conn: &Connection, key: &str, format: &str) -> Res<()> {
        let Some(doc) = self.docs.get(key) else {
            return Ok(());
        };
        let Some(path) = doc.path() else {
            return Ok(());
        };
        let kind = doc.kind();
        let snap = match kind {
            Kind::Table => snapshot(&mut self.snaps, &path).cloned(),
            Kind::Result => self.result_state(&path).map(|st| st.snap.clone()),
            _ => None,
        };
        let Some(snap) = snap else {
            return show(conn, MessageType::ERROR, "zpg: no saved state for this file; refresh it first".into());
        };
        let stem = match kind {
            Kind::Table => snap.table.file_stem(),
            _ => path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "result".into()),
        };
        let shown = snap.shown();
        let names: Vec<String> = shown.iter().map(|&i| snap.columns[i].name.clone()).collect();
        let types: Vec<String> = shown.iter().map(|&i| snap.columns[i].type_name.clone()).collect();
        let rows: Vec<Vec<Option<String>>> =
            snap.rows.iter().map(|r| shown.iter().map(|&i| r.values[i].clone()).collect()).collect();

        let root = self.zpg_root(&path);
        state::ensure_root(&root)?;
        let dir = root.join(encode_name(&snap.conn)).join("exports");
        std::fs::create_dir_all(&dir)?;
        let ext = if format == "csv-all" { "csv" } else { format };
        let target = dir.join(format!("{stem}.{ext}"));
        let (what, bytes) = match format {
            "csv-all" => {
                let timeout = self.settings.statement_timeout_ms;
                let mut file = File::create(&target)?;
                let written = self
                    .cancellable(conn, &snap.conn, &format!("Exporting {}", snap.table), |c| {
                        db::copy_csv(c, &snap, &snap.query, timeout, &mut file)
                    })?
                    .and_then(|r| r);
                drop(file);
                match written {
                    Ok(n) => ("all matching rows".to_string(), n),
                    Err(e) => {
                        let _ = std::fs::remove_file(&target);
                        return if running::cancelled(&e) {
                            show(conn, MessageType::INFO, "zpg: export cancelled".into())
                        } else {
                            show(conn, MessageType::ERROR, format!("zpg: {e}"))
                        };
                    }
                }
            }
            "csv" | "json" | "sql" => {
                let text = match format {
                    "csv" => export::csv(&names, &rows),
                    "json" => export::json(&names, &types, &rows),
                    _ => export::inserts(&snap.table, &names, &rows),
                };
                let n = text.len() as u64;
                self.write_file(conn, &target, text)?;
                (format!("{} row{}", rows.len(), if rows.len() == 1 { "" } else { "s" }), n)
            }
            _ => return Ok(()),
        };
        let shown_path = target.strip_prefix(root.parent().unwrap_or(&root)).unwrap_or(&target);
        show(conn, MessageType::INFO, format!("zpg: exported {what} to {}", shown_path.display()))?;
        if bytes <= MAX_OPEN {
            self.show_document(conn, &target)?;
        }
        Ok(())
    }
}
