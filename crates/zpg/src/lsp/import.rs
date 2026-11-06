//! *Import*: the rows of a CSV file become new rows of a table page, to be
//! checked and applied like any other edit. LSP has no file picker and zpg
//! isn't attached to `.csv` files, so the files come from one folder.

use std::path::{Path, PathBuf};

use lsp_server::Connection;
use lsp_types::{CodeActionOrCommand, MessageType};
use serde_json::Value;
use zpg_core::header::encode_name;
use zpg_core::table;
use zpg_core::{Snapshot, csv};

use super::{CMD_IMPORT, Res, Server, command, show, snapshot};

const MAX_FILE: u64 = 10 << 20;
/// Apply writes rows one by one and can't be cancelled.
const MAX_ROWS: usize = 2000;
const MAX_LISTED: usize = 20;

fn imports_dir(root: &Path, conn: &str) -> PathBuf {
    root.join(encode_name(conn)).join("imports")
}

/// An action per file of the imports folder; one that tells where it is when it's empty.
pub(super) fn actions(key: &str, root: &Path, snap: &Snapshot) -> Vec<CodeActionOrCommand> {
    let mut names: Vec<String> = std::fs::read_dir(imports_dir(root, &snap.conn))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.to_ascii_lowercase().ends_with(".csv"))
        .collect();
    names.sort();
    let arg = || Value::String(key.to_string());
    if names.is_empty() {
        return vec![command("zpg: Import CSV… (shows where to put the file)".into(), CMD_IMPORT, vec![arg()])];
    }
    names.truncate(MAX_LISTED);
    names
        .into_iter()
        .map(|n| command(format!("zpg: Import rows from {n}"), CMD_IMPORT, vec![arg(), Value::String(n)]))
        .collect()
}

/// Table lines for the rows of `file`.
fn read_rows(file: &Path, snap: &Snapshot) -> Result<Vec<String>, String> {
    let size = std::fs::metadata(file).map_err(|e| e.to_string())?.len();
    if size > MAX_FILE {
        return Err(format!("the file is over {} MB; use psql \\copy for it", MAX_FILE >> 20));
    }
    let bytes = std::fs::read(file).map_err(|e| e.to_string())?;
    let text = String::from_utf8(bytes).map_err(|_| "the file is not UTF-8; save it as UTF-8 first".to_string())?;
    let records = csv::parse(&text)?;
    let rows = records.len().saturating_sub(1);
    if rows > MAX_ROWS {
        return Err(format!("the file has {rows} rows, at most {MAX_ROWS} can be edited here; use psql \\copy for it"));
    }
    csv::page_rows(snap, &records)
}

/// `text` with `rows` after its last row, aligned.
fn with_rows(text: &str, rows: &[String]) -> String {
    let parsed = table::parse(text);
    let after = parsed.rows.last().map(|r| r.line).or(parsed.columns.as_ref().map(|c| c.line + 1)).unwrap_or(0);
    let mut lines: Vec<&str> = text.split('\n').collect();
    let at = (after as usize + 1).min(lines.len());
    lines.splice(at..at, rows.iter().map(String::as_str));
    let joined = lines.join("\n");
    table::realign(&joined).unwrap_or(joined)
}

impl Server {
    /// Adds the rows of `file` from the imports folder to the page; without
    /// a file, creates the folder and says where it is. Nothing is written to the database.
    pub(super) fn import_csv(&mut self, conn: &Connection, key: &str, file: Option<&str>) -> Res<()> {
        let Some(doc) = self.docs.get(key) else {
            return Ok(());
        };
        let Some(path) = doc.path() else {
            return Ok(());
        };
        let text = doc.text.clone();
        let root = self.zpg_root(&path);
        let Some(snap) = snapshot(&mut self.snaps, &path).cloned() else {
            return show(conn, MessageType::ERROR, "zpg: no saved state for this file; refresh it first".into());
        };
        if let Some(why) = &snap.read_only {
            return show(conn, MessageType::WARNING, format!("zpg: read-only: {why}"));
        }
        let dir = imports_dir(&root, &snap.conn);
        let Some(name) = file.and_then(|f| Path::new(f).file_name()) else {
            std::fs::create_dir_all(&dir)?;
            let shown = dir.strip_prefix(root.parent().unwrap_or(&root)).unwrap_or(&dir);
            return show(
                conn,
                MessageType::INFO,
                format!("zpg: put .csv files into {}, then ctrl-. here", shown.display()),
            );
        };
        let shown_name = name.to_string_lossy();
        let rows = match read_rows(&dir.join(name), &snap) {
            Ok(r) => r,
            Err(e) => return show(conn, MessageType::ERROR, format!("zpg: {shown_name}: {e}")),
        };
        if rows.is_empty() {
            return show(conn, MessageType::INFO, format!("zpg: {shown_name} has no rows"));
        }
        self.replace_doc(conn, key, with_rows(&text, &rows))?;
        show(
            conn,
            MessageType::INFO,
            format!("zpg: added {} rows from {shown_name}; nothing is written yet, check them and Apply", rows.len()),
        )?;
        self.refresh_tokens(conn)?;
        self.publish(conn, key)
    }
}
