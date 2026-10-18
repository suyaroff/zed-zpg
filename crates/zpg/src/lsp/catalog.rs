//! The `.pgdb` catalog: relations of every connection.

use std::path::{Path, PathBuf};

use lsp_server::Connection;
use lsp_types::CodeActionOrCommand;
use serde_json::Value;
use zpg_core::catalog::{self, Section};

use super::{
    CMD_ACTIVITY, CMD_BACKUP, CMD_CONFIG, CMD_DDL, CMD_DROP, CMD_OPEN, CMD_REFRESH, Kind, Res, Server, command,
};
use crate::db;

impl Server {
    pub(super) fn refresh_catalog(&mut self, conn: &Connection, key: &str) -> Res<()> {
        self.schemas.clear();
        let names: Vec<String> = self.settings.connections.keys().cloned().collect();
        let sections: Vec<Section> = names
            .into_iter()
            .map(|name| {
                let relations = self.db.client(&self.settings, &name).and_then(db::catalog);
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
        }
        out.push(command("zpg: Refresh catalog".into(), CMD_REFRESH, vec![arg()]));
        if self.caps.show_document {
            out.push(command("zpg: Open settings file".into(), CMD_CONFIG, vec![arg()]));
        }
        out
    }
}
