//! The `.pgdb` catalog: relations of every connection.

use std::path::{Path, PathBuf};

use lsp_server::Connection;
use lsp_types::{CodeActionOrCommand, Location, Position, Range, SymbolInformation, SymbolKind};
use serde_json::Value;
use zpg_core::catalog::{self, Section};
use zpg_core::header::encode_name;

use super::{
    CMD_ACTIVITY, CMD_BACKUP, CMD_CONFIG, CMD_DATABASES, CMD_DDL, CMD_DROP, CMD_OPEN, CMD_REFRESH, CMD_SCHEMAS, Kind,
    Res, Server, command, path_to_uri,
};
use crate::db;

const MAX_SYMBOLS: usize = 2000;

impl Server {
    pub(super) fn refresh_catalog(&mut self, conn: &Connection, key: &str) -> Res<()> {
        self.schemas.clear();
        let names: Vec<String> = self.settings.active().cloned().collect();
        let sections: Vec<Section> = names
            .into_iter()
            .map(|name| {
                let relations = self.db.read(&self.settings, &name, db::catalog);
                Section { conn: name, relations }
            })
            .collect();
        self.replace_doc(conn, key, catalog::render(&sections))
    }

    /// Re-reads every open catalog.
    pub(super) fn refresh_catalogs(&mut self, conn: &Connection) -> Res<()> {
        let keys: Vec<String> =
            self.docs.iter().filter(|(_, d)| d.kind() == Kind::Catalog).map(|(k, _)| k.clone()).collect();
        for key in keys {
            self.refresh_catalog(conn, &key)?;
        }
        Ok(())
    }

    /// F12 on a catalog line opens its table.
    pub(super) fn catalog_definition(&mut self, path: &Path, text: &str, line: u32) -> Result<Option<PathBuf>, String> {
        let Some((conn, table)) = catalog::entry_at(text, line) else {
            return Ok(None);
        };
        let root = path.parent().ok_or("catalog has no directory")?.to_path_buf();
        self.open_table(&root, &conn, &table).map(Some)
    }

    /// Relations of the catalog whose name has the letters of `query` in order.
    /// Called on every key press, so it only looks at the catalog file, not the
    /// database. A table with a page leads there, others to their catalog line.
    pub(super) fn workspace_symbols(&self, query: &str) -> Vec<SymbolInformation> {
        let open = self.docs.values().find(|d| d.kind() == Kind::Catalog);
        let (path, text) = match open {
            Some(d) => (d.path(), d.text.clone()),
            None => {
                // `zpg_root` looks in the ancestors of a file; any name in the root does.
                let Some(root) = self.root.as_ref().map(|r| self.zpg_root(&r.join("_"))) else {
                    return vec![];
                };
                let path = root.join("catalog.pgdb");
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                (Some(path), text)
            }
        };
        let Some(path) = path else {
            return vec![];
        };
        let Some(root) = path.parent() else {
            return vec![];
        };
        let query = query.to_lowercase();
        catalog::entries(&text)
            .into_iter()
            .filter(|(_, _, table)| {
                let name = table.to_string().to_lowercase();
                let mut letters = name.chars();
                query.chars().all(|q| letters.any(|c| c == q))
            })
            .take(MAX_SYMBOLS)
            .filter_map(|(line, conn, table)| {
                let page = root.join(encode_name(&conn)).join(format!("{}.pgt", table.file_stem()));
                let (target, line) = if page.exists() { (page, 0) } else { (path.clone(), line) };
                let at = Position::new(line, 0);
                #[allow(deprecated)]
                Some(SymbolInformation {
                    name: table.to_string(),
                    kind: SymbolKind::STRUCT,
                    tags: None,
                    deprecated: None,
                    location: Location { uri: path_to_uri(&target).ok()?, range: Range::new(at, at) },
                    container_name: Some(conn),
                })
            })
            .collect()
    }

    pub(super) fn catalog_actions(&self, key: &str, text: &str, line: u32) -> Vec<CodeActionOrCommand> {
        let arg = || Value::String(key.to_string());
        let mut out = vec![];
        if self.caps.show_document
            && let Some((_, table)) = catalog::entry_at(text, line)
        {
            out.push(command(format!("zpg: Open {table}"), CMD_OPEN, vec![arg(), Value::from(line)]));
            out.push(command(format!("zpg: Show DDL of {table}"), CMD_DDL, vec![arg(), Value::from(line)]));
            out.push(command(format!("zpg: Back up {table} (pg_dump)"), CMD_BACKUP, vec![arg(), Value::from(line)]));
            out.push(command(
                format!("zpg: Drop {table}… (opens the SQL to review and run)"),
                CMD_DROP,
                vec![arg(), Value::from(line)],
            ));
        }
        if self.caps.show_document
            && let Some(c) = catalog::conn_at(text, line)
        {
            out.push(command(format!("zpg: Server activity of {c}"), CMD_ACTIVITY, vec![arg(), Value::from(line)]));
            out.push(command(format!("zpg: Databases of {c}"), CMD_DATABASES, vec![arg(), Value::from(line)]));
            out.push(command(format!("zpg: Schemas of {c}"), CMD_SCHEMAS, vec![arg(), Value::from(line)]));
        }
        out.push(command("zpg: Refresh catalog".into(), CMD_REFRESH, vec![arg()]));
        if self.caps.show_document {
            out.push(command("zpg: Open settings file".into(), CMD_CONFIG, vec![arg()]));
        }
        out
    }
}
