//! *Back up* a table with `pg_dump` into `.zpg/<conn>/backups/`.

use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;

use lsp_server::Connection;
use lsp_types::MessageType;
use zpg_core::{QualName, catalog};

use super::{Kind, Res, Server, encode_name, show, snapshot};

/// Bigger backups are written but not opened in the editor.
const MAX_OPEN: u64 = 20 << 20;

/// The file extension for `pg_dump`'s output format among `args`; the
/// directory format can't go to one file.
fn dump_ext(args: &[String]) -> Result<&'static str, String> {
    let mut ext = "sql";
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        let format = match a.as_str() {
            "-F" | "--format" => it.next().map(String::as_str),
            a => a.strip_prefix("--format=").or_else(|| a.strip_prefix("-F")),
        };
        match format.map(|f| f.chars().next().unwrap_or('p')) {
            Some('c') => ext = "dump",
            Some('t') => ext = "tar",
            Some('d') => return Err("pg_dump's directory format can't be written to one file".into()),
            Some(_) => ext = "sql",
            None => {}
        }
    }
    Ok(ext)
}

impl Server {
    /// The relation on catalog `line` or of table page `key`, with its
    /// connection and that connection's `.zpg/<conn>` directory.
    pub(super) fn relation_at(&mut self, key: &str, line: Option<u32>) -> Option<(String, QualName, PathBuf)> {
        let doc = self.docs.get(key)?;
        let path = doc.path()?;
        match (doc.kind(), line) {
            (Kind::Catalog, Some(line)) => {
                let (conn, table) = catalog::entry_at(&doc.text, line)?;
                let dir = path.parent()?.join(encode_name(&conn));
                Some((conn, table, dir))
            }
            (Kind::Table, _) => {
                let snap = snapshot(&mut self.snaps, &path)?;
                Some((snap.conn.clone(), snap.table.clone(), path.parent()?.to_path_buf()))
            }
            _ => None,
        }
    }

    /// Runs `pg_dump --table` for the relation of catalog `line` or table page
    /// `key`, writing `.zpg/<conn>/backups/<table>-<time>.sql`, and opens it.
    pub(super) fn backup(&mut self, conn: &Connection, key: &str, line: Option<u32>) -> Res<()> {
        let Some((conn_name, table, dir)) = self.relation_at(key, line) else {
            return Ok(());
        };
        let args = self.settings.pg_dump_args.clone();
        let ext = match dump_ext(&args) {
            Ok(e) => e,
            Err(e) => return show(conn, MessageType::ERROR, format!("zpg: {e}")),
        };
        // Connecting first brings up an SSH tunnel and tells where the database is.
        if let Err(e) = self.db.client(&self.settings, &conn_name) {
            return show(conn, MessageType::ERROR, format!("zpg: {e}"));
        }
        let env = self.db.pg_env(&conn_name).unwrap_or_default();

        let dir = dir.join("backups");
        std::fs::create_dir_all(&dir)?;
        let now = chrono::Local::now();
        let target = dir.join(format!("{}-{}.{ext}", table.file_stem(), now.format("%Y%m%d-%H%M%S")));
        let mut file = File::create(&target)?;
        if ext == "sql" {
            write!(
                file,
                "-- zpg: pg_dump of {table} on {conn_name}, {}.\n\
                 -- Restore with psql (it has COPY data that ▶ can't run): psql -d <database> -f <this file>\n\n",
                now.format("%Y-%m-%d %H:%M:%S")
            )?;
        }

        let program = self.settings.pg_dump.clone();
        let spawned = Command::new(&program)
            .arg("--table")
            .arg(table.sql())
            .args(&args)
            .envs(env)
            .stdin(Stdio::null())
            .stdout(file.try_clone()?)
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => {
                let _ = std::fs::remove_file(&target);
                let hint = if e.kind() == std::io::ErrorKind::NotFound {
                    "; install the PostgreSQL client tools or set \"pg_dump\" in the zpg settings"
                } else {
                    ""
                };
                return show(conn, MessageType::ERROR, format!("zpg: can't run {program}: {e}{hint}"));
            }
        };
        // Read stderr meanwhile, so a chatty pg_dump can't block on a full pipe.
        let errors = child.stderr.take().map(|mut s| {
            thread::spawn(move || {
                let mut out = String::new();
                let _ = s.read_to_string(&mut out);
                out
            })
        });
        let status = self.wait_child(conn, &format!("pg_dump of {table}"), &mut child)?;
        drop(file);
        // After a kill, whatever the program started may still hold stderr: don't wait for it.
        let Some(status) = status else {
            let _ = std::fs::remove_file(&target);
            return show(conn, MessageType::INFO, "zpg: backup cancelled".into());
        };
        let errors = errors.and_then(|t| t.join().ok()).unwrap_or_default();
        if !status.success() {
            let _ = std::fs::remove_file(&target);
            let first: Vec<&str> = errors.lines().filter(|l| !l.trim().is_empty()).take(3).collect();
            return show(conn, MessageType::ERROR, format!("zpg: pg_dump failed: {}", first.join(" ")));
        }
        let bytes = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
        let root = dir.parent().and_then(|d| d.parent()).and_then(|d| d.parent());
        let shown = root.and_then(|r| target.strip_prefix(r).ok()).unwrap_or(&target);
        show(
            conn,
            MessageType::INFO,
            format!("zpg: backed up {table} to {} ({} kB)", shown.display(), bytes.div_ceil(1024)),
        )?;
        if ext == "sql" && bytes <= MAX_OPEN {
            self.show_document(conn, &target)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ext(args: &[&str]) -> Result<&'static str, String> {
        dump_ext(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn output_format() {
        assert_eq!(ext(&[]), Ok("sql"));
        assert_eq!(ext(&["--column-inserts"]), Ok("sql"));
        assert_eq!(ext(&["-Fc"]), Ok("dump"));
        assert_eq!(ext(&["--format=custom"]), Ok("dump"));
        assert_eq!(ext(&["-F", "t"]), Ok("tar"));
        assert_eq!(ext(&["--format", "plain"]), Ok("sql"));
        assert!(ext(&["-Fd"]).is_err());
    }
}
