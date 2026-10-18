//! *Server activity*: who is connected and what runs, as a `.pgres`, and
//! cancelling or terminating a backend from any result with a `pid` column.

use std::path::Path;
use std::time::Instant;

use lsp_server::Connection;
use lsp_types::{CodeActionOrCommand, MessageType};
use serde_json::Value;
use zpg_core::{catalog, table};

use super::query::Pending;
use super::{CMD_SIGNAL, Res, Server, command, encode_name, show};
use crate::db;

/// Client sessions except zpg's own one, busiest first. `blocked_by` lists
/// the pids holding the locks a session waits for.
const ACTIVITY_SQL: &str = r"SELECT pid, usename AS user, datname AS db, application_name AS app,
       coalesce(client_addr::text, 'local') AS client, state,
       date_trunc('second', now() - xact_start) AS xact_age,
       date_trunc('second', now() - query_start) AS query_age,
       wait_event_type || ': ' || wait_event AS waiting,
       nullif(pg_blocking_pids(pid), '{}') AS blocked_by,
       left(regexp_replace(query, '\s+', ' ', 'g'), 300) AS query
FROM pg_stat_activity
WHERE backend_type = 'client backend' AND pid <> pg_backend_pid()
ORDER BY state = 'idle', xact_start NULLS LAST";

impl Server {
    /// Opens `.zpg/<conn>/activity.pgres` for the connection of catalog `line`.
    pub(super) fn activity(&mut self, conn: &Connection, key: &str, line: u32) -> Res<()> {
        let Some(doc) = self.docs.get(key) else {
            return Ok(());
        };
        let Some(conn_name) = catalog::conn_at(&doc.text, line) else {
            return Ok(());
        };
        let Some(root) = doc.path().and_then(|p| p.parent().map(Path::to_path_buf)) else {
            return Ok(());
        };
        let (max_rows, timeout) = (self.settings.max_result_rows, self.settings.statement_timeout_ms);
        let started = Instant::now();
        let ran = self.cancellable(conn, &conn_name, &format!("Reading activity of {conn_name}"), |c| {
            db::run(c, &[ACTIVITY_SQL], true, max_rows, timeout, false)
        })?;
        let Some(mut outcomes) = self.outcomes(conn, ran)? else {
            return Ok(());
        };
        let out = outcomes.remove(0);
        let millis = started.elapsed().as_millis();
        let target = root.join(encode_name(&conn_name)).join("activity.pgres");
        self.show_result(conn, &conn_name, ACTIVITY_SQL, &out, millis, &target)
    }

    /// *Cancel* / *Terminate* for the row on `line` of a result with a `pid` column.
    pub(super) fn pid_actions(&mut self, key: &str, path: &Path, text: &str, line: u32) -> Vec<CodeActionOrCommand> {
        let Some(st) = self.result_state(path) else {
            return vec![];
        };
        let Some(col) = st.columns.iter().position(|c| c.name == "pid") else {
            return vec![];
        };
        let Some(row) = table::parse(text).rows.iter().position(|r| r.line == line) else {
            return vec![];
        };
        let Some(pid) =
            st.snap.rows.get(row).and_then(|r| r.values[col].as_deref()).and_then(|v| v.parse::<i32>().ok())
        else {
            return vec![];
        };
        let arg = |what: &str| vec![Value::String(key.to_string()), Value::from(pid), Value::from(what)];
        vec![
            command(format!("zpg: Cancel the query of pid {pid}"), CMD_SIGNAL, arg("cancel")),
            command(format!("zpg: Terminate backend pid {pid}…"), CMD_SIGNAL, arg("terminate")),
        ]
    }

    /// `pg_cancel_backend` / `pg_terminate_backend` (asked first) on the
    /// connection of result `key`, then reruns the result.
    pub(super) fn signal(
        &mut self,
        conn: &Connection,
        key: &str,
        pid: i32,
        terminate: bool,
        confirmed: bool,
    ) -> Res<()> {
        let Some(path) = self.docs.get(key).and_then(|d| d.path()) else {
            return Ok(());
        };
        let Some(conn_name) = self.result_state(&path).map(|st| st.conn.clone()) else {
            return Ok(());
        };
        if terminate && !confirmed && self.caps.message_actions {
            let message = format!(
                "zpg: Terminate backend {pid} on {conn_name}? Its session ends and an open transaction rolls back."
            );
            return self.confirm(conn, Pending::Terminate(key.to_string(), pid), message);
        }
        let sql = if terminate { "SELECT pg_terminate_backend($1)" } else { "SELECT pg_cancel_backend($1)" };
        let sent = self
            .db
            .client(&self.settings, &conn_name)
            .and_then(|c| c.query_one(sql, &[&pid]).map_err(|e| db::fmt_err(&e)))
            .map(|r| r.get::<_, bool>(0));
        match sent {
            Ok(true) if terminate => show(conn, MessageType::INFO, format!("zpg: terminated backend {pid}"))?,
            Ok(true) => show(conn, MessageType::INFO, format!("zpg: asked backend {pid} to cancel its query"))?,
            Ok(false) => show(conn, MessageType::WARNING, format!("zpg: no backend {pid} (it may have ended)"))?,
            Err(e) => return show(conn, MessageType::ERROR, format!("zpg: {e}")),
        }
        self.rerun(conn, key)
    }
}
