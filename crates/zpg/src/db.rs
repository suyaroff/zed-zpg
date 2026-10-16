use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use postgres::config::{Host, SslMode};
use postgres::types::ToSql;
use postgres::{Client, Config, NoTls};
use zpg_core::catalog::Relation;
use zpg_core::diff::{Diff, Op};
use zpg_core::sql::{self, Stmt};
use zpg_core::header::quote_ident;
use zpg_core::{Apply, Column, ForeignKey, QualName, Query, SnapRow, Snapshot};

use crate::config::{Settings, expand_env};

pub fn fmt_err(e: &postgres::Error) -> String {
    match e.as_db_error() {
        Some(db) => {
            let mut s = db.message().to_string();
            if let Some(d) = db.detail() {
                s += &format!("\n{d}");
            }
            if let Some(h) = db.hint() {
                s += &format!("\nhint: {h}");
            }
            s
        }
        None => e.to_string(),
    }
}

/// `sslmode=prefer/require` encrypt without checking the certificate, like libpq.
mod tls {
    use rustls::DigitallySignedStruct;
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};

    #[derive(Debug)]
    pub struct NoVerify(pub CryptoProvider);

    impl ServerCertVerifier for NoVerify {
        fn verify_server_cert(
            &self,
            _: &CertificateDer<'_>,
            _: &[CertificateDer<'_>],
            _: &ServerName<'_>,
            _: &[u8],
            _: UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
        }

        fn verify_tls13_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
        }

        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            self.0.signature_verification_algorithms.supported_schemes()
        }
    }
}

fn connect(url: &str, read_only: bool) -> Result<Client, String> {
    let url = expand_env(url)?;
    let mut cfg: Config = url.parse().map_err(|e| format!("bad connection url: {e}"))?;
    if cfg.get_connect_timeout().is_none() {
        cfg.connect_timeout(Duration::from_secs(5));
    }
    if cfg.get_application_name().is_none() {
        cfg.application_name("zpg");
    }
    if cfg.get_user().is_none()
        && let Ok(u) = std::env::var("USER").or_else(|_| std::env::var("USERNAME"))
    {
        cfg.user(&u);
    }
    if read_only {
        cfg.options("-c default_transaction_read_only=on");
    }
    let local = cfg.get_hosts().iter().all(|h| !matches!(h, Host::Tcp(_)));
    let res = if local || cfg.get_ssl_mode() == SslMode::Disable {
        cfg.connect(NoTls)
    } else {
        let provider = rustls::crypto::ring::default_provider();
        let tls = rustls::ClientConfig::builder_with_provider(provider.clone().into())
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(tls::NoVerify(provider)))
            .with_no_client_auth();
        cfg.connect(tokio_postgres_rustls::MakeRustlsConnect::new(tls))
    };
    res.map_err(|e| fmt_err(&e))
}

#[derive(Default)]
pub struct Db {
    clients: HashMap<String, Client>,
}

impl Db {
    pub fn clear(&mut self) {
        self.clients.clear();
    }

    pub fn client(&mut self, settings: &Settings, conn: &str) -> Result<&mut Client, String> {
        let spec = settings
            .connections
            .get(conn)
            .ok_or_else(|| format!("no connection `{conn}` in lsp.zpg.settings.connections"))?;
        if self.clients.get(conn).is_none_or(|c| c.is_closed()) {
            let c = connect(spec.url(), spec.read_only())?;
            self.clients.insert(conn.to_string(), c);
        }
        Ok(self.clients.get_mut(conn).unwrap())
    }
}

fn params(stmt: &Stmt) -> Vec<&(dyn ToSql + Sync)> {
    stmt.params.iter().map(|p| p as &(dyn ToSql + Sync)).collect()
}

fn to_row(r: &postgres::Row) -> SnapRow {
    SnapRow {
        xmin: r.get(0),
        values: (1..r.len()).map(|i| r.get(i)).collect(),
    }
}

pub fn catalog(c: &mut Client) -> Result<Vec<Relation>, String> {
    let rows = c
        .query(
            "SELECT n.nspname::text, c.relname::text,
                    CASE c.relkind WHEN 'r' THEN 'table' WHEN 'p' THEN 'partitioned' WHEN 'v' THEN 'view'
                                   WHEN 'm' THEN 'matview' ELSE 'foreign' END,
                    CASE WHEN c.relkind IN ('r', 'm') AND c.reltuples >= 0 THEN c.reltuples::bigint END
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
             WHERE c.relkind IN ('r', 'p', 'v', 'm', 'f')
               AND n.nspname NOT IN ('pg_catalog', 'information_schema')
               AND n.nspname !~ '^pg_(toast|temp)'
               AND NOT c.relispartition
               AND has_table_privilege(c.oid, 'SELECT')
             ORDER BY 1, 2",
            &[],
        )
        .map_err(|e| fmt_err(&e))?;
    Ok(rows
        .iter()
        .map(|r| Relation {
            name: QualName::new(r.get::<_, String>(0), r.get::<_, String>(1)),
            kind: r.get(2),
            rows: r.get(3),
        })
        .collect())
}

pub struct Defaults {
    pub query: Query,
    pub width: u32,
    pub apply: Apply,
    pub visible: Vec<String>,
    pub read_only_conn: bool,
}

/// Reads the table's shape and the page `d.query`.
pub fn load(c: &mut Client, conn: &str, table: &QualName, d: &Defaults) -> Result<Snapshot, String> {
    let name = table.sql();
    let kind: Option<String> = c
        .query_opt("SELECT relkind::text FROM pg_class WHERE oid = to_regclass($1)", &[&name])
        .map_err(|e| fmt_err(&e))?
        .map(|r| r.get(0));
    let kind = kind.ok_or_else(|| format!("relation {table} does not exist"))?;
    let columns: Vec<Column> = c
        .query(
            "SELECT attname::text, format_type(atttypid, atttypmod), attnotnull,
                    atthasdef OR attidentity <> '', attgenerated <> '',
                    (SELECT array_agg(e.enumlabel::text ORDER BY e.enumsortorder)
                     FROM pg_enum e WHERE e.enumtypid = atttypid)
             FROM pg_attribute
             WHERE attrelid = to_regclass($1) AND attnum > 0 AND NOT attisdropped
             ORDER BY attnum",
            &[&name],
        )
        .map_err(|e| fmt_err(&e))?
        .iter()
        .map(|r| Column {
            name: r.get(0),
            type_name: r.get(1),
            not_null: r.get(2),
            has_default: r.get(3),
            generated: r.get(4),
            enum_values: r.get::<_, Option<Vec<String>>>(5).unwrap_or_default(),
        })
        .collect();
    let pk_names: Vec<String> = c
        .query(
            "SELECT a.attname::text
             FROM pg_index i
             CROSS JOIN LATERAL unnest(i.indkey::int2[]) WITH ORDINALITY AS k(attnum, ord)
             JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = k.attnum
             WHERE i.indrelid = to_regclass($1) AND i.indisprimary
             ORDER BY k.ord",
            &[&name],
        )
        .map_err(|e| fmt_err(&e))?
        .iter()
        .map(|r| r.get(0))
        .collect();
    let index = |n: &String| columns.iter().position(|c| &c.name == n);
    let pk: Vec<usize> = pk_names.iter().filter_map(index).collect();
    let foreign_keys: Vec<ForeignKey> = c
        .query(
            "SELECT (SELECT array_agg(a.attname::text ORDER BY k.ord)
                     FROM unnest(c.conkey) WITH ORDINALITY AS k(attnum, ord)
                     JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = k.attnum),
                    rn.nspname::text, rc.relname::text,
                    (SELECT array_agg(a.attname::text ORDER BY k.ord)
                     FROM unnest(c.confkey) WITH ORDINALITY AS k(attnum, ord)
                     JOIN pg_attribute a ON a.attrelid = c.confrelid AND a.attnum = k.attnum)
             FROM pg_constraint c
             JOIN pg_class rc ON rc.oid = c.confrelid
             JOIN pg_namespace rn ON rn.oid = rc.relnamespace
             WHERE c.conrelid = to_regclass($1) AND c.contype = 'f'
             ORDER BY c.conname",
            &[&name],
        )
        .map_err(|e| fmt_err(&e))?
        .iter()
        .filter_map(|r| {
            let local: Vec<String> = r.get(0);
            Some(ForeignKey {
                columns: local.iter().map(index).collect::<Option<_>>()?,
                table: QualName::new(r.get::<_, String>(1), r.get::<_, String>(2)),
                ref_columns: r.get(3),
            })
        })
        .collect();

    let read_only = if !matches!(kind.as_str(), "r" | "p") {
        Some("not a table".to_string())
    } else if pk.is_empty() {
        Some("table has no primary key".to_string())
    } else if d.read_only_conn {
        Some("connection is read_only".to_string())
    } else {
        None
    };
    let mut snap = Snapshot {
        conn: conn.to_string(),
        table: table.clone(),
        columns,
        pk,
        foreign_keys,
        read_only,
        query: d.query.clone(),
        width: d.width,
        apply: d.apply,
        visible: d.visible.clone(),
        rows: vec![],
    };
    snap.rows = fetch(c, &snap, &snap.query)?;
    Ok(snap)
}

pub fn fetch(c: &mut Client, snap: &Snapshot, q: &Query) -> Result<Vec<SnapRow>, String> {
    let has_xmin = snap.read_only.is_none();
    let stmt = sql::select_page(snap, has_xmin, q);
    // Read-only, so a `where` like `1=1; DROP ...` can't do harm (and the extended protocol refuses it anyway).
    let mut tx = c.build_transaction().read_only(true).start().map_err(|e| fmt_err(&e))?;
    let rows = tx.query(&stmt.sql, &params(&stmt)).map_err(|e| fmt_err(&e))?;
    let rows = rows.iter().map(to_row).collect();
    tx.commit().map_err(|e| fmt_err(&e))?;
    Ok(rows)
}

/// One row by key values (as text); see `sql::select_row`.
pub fn fetch_row(c: &mut Client, snap: &Snapshot, key: &[Option<String>], pretty: bool) -> Result<Option<SnapRow>, String> {
    let stmt = sql::select_row(snap, key, pretty);
    let mut tx = c.build_transaction().read_only(true).start().map_err(|e| fmt_err(&e))?;
    let row = tx.query_opt(&stmt.sql, &params(&stmt)).map_err(|e| fmt_err(&e))?;
    tx.commit().map_err(|e| fmt_err(&e))?;
    Ok(row.as_ref().map(to_row))
}

pub struct ApplyError {
    /// Index into `Diff::ops`.
    pub op: usize,
    pub message: String,
}

const CONFLICT: &str = "row was changed or deleted since it was loaded; refresh to see the current data";

/// Runs all changes in one transaction. On success returns the new row for
/// every update/insert (`None` for deletes), in `diff.ops` order.
pub fn apply(c: &mut Client, snap: &Snapshot, diff: &Diff) -> Result<Vec<Option<SnapRow>>, ApplyError> {
    let fail = |op, message| ApplyError { op, message };
    let mut tx = c.transaction().map_err(|e| fail(0, fmt_err(&e)))?;
    let mut out = Vec::with_capacity(diff.ops.len());
    for (k, op) in diff.ops.iter().enumerate() {
        let stmt = sql::for_op(snap, op);
        let p = params(&stmt);
        match op {
            Op::Delete { .. } => {
                let n = tx.execute(&stmt.sql, &p).map_err(|e| fail(k, fmt_err(&e)))?;
                if n != 1 {
                    return Err(fail(k, CONFLICT.into()));
                }
                out.push(None);
            }
            Op::Update { .. } | Op::Insert { .. } => {
                let rows = tx.query(&stmt.sql, &p).map_err(|e| fail(k, fmt_err(&e)))?;
                match rows.as_slice() {
                    [r] => out.push(Some(to_row(r))),
                    _ => return Err(fail(k, CONFLICT.into())),
                }
            }
        }
    }
    tx.commit().map_err(|e| fail(diff.ops.len().saturating_sub(1), fmt_err(&e)))?;
    Ok(out)
}

/// Column name and value.
pub type Field = (String, Option<String>);

/// The row `fk` points at for `values` (one per FK column, as text), as
/// (column, value) pairs; `None` when there is no such row.
pub fn referenced(
    c: &mut Client,
    snap: &Snapshot,
    fk: &ForeignKey,
    values: &[String],
) -> Result<Option<Vec<Field>>, String> {
    let mut tx = c.build_transaction().read_only(true).start().map_err(|e| fmt_err(&e))?;
    let target = fk.table.sql();
    let names: Vec<String> = tx
        .query(
            "SELECT attname::text FROM pg_attribute
             WHERE attrelid = to_regclass($1) AND attnum > 0 AND NOT attisdropped ORDER BY attnum",
            &[&target],
        )
        .map_err(|e| fmt_err(&e))?
        .iter()
        .map(|r| r.get(0))
        .collect();
    let select: Vec<String> = names.iter().map(|n| format!("{}::text", quote_ident(n))).collect();
    let cond: Vec<String> = fk
        .ref_columns
        .iter()
        .zip(&fk.columns)
        .enumerate()
        .map(|(i, (rc, &lc))| {
            format!("{} = CAST(${}::text AS {})", quote_ident(rc), i + 1, snap.columns[lc].type_name)
        })
        .collect();
    let sql = format!("SELECT {} FROM {target} WHERE {} LIMIT 1", select.join(", "), cond.join(" AND "));
    let params: Vec<&(dyn ToSql + Sync)> = values.iter().map(|v| v as &(dyn ToSql + Sync)).collect();
    let row = tx.query_opt(&sql, &params).map_err(|e| fmt_err(&e))?;
    tx.commit().map_err(|e| fmt_err(&e))?;
    Ok(row.map(|r| names.into_iter().enumerate().map(|(i, n)| (n, r.get(i))).collect()))
}

/// A column of a query result, with where it comes from (when it's a plain table column).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResultCol {
    pub name: String,
    pub type_name: String,
    pub table_oid: u32,
    pub column_id: i16,
}

#[derive(Debug, Clone)]
pub struct Outcome {
    /// e.g. `UPDATE 3`, `SELECT 12`.
    pub tag: String,
    pub columns: Vec<ResultCol>,
    pub rows: Vec<Vec<Option<String>>>,
    /// More rows existed than were fetched.
    pub more: bool,
}

pub struct RunError {
    /// Index of the failing statement.
    pub stmt: usize,
    pub message: String,
    /// 1-based character position inside that statement.
    pub position: Option<usize>,
}

fn run_err(stmt: usize, e: &postgres::Error) -> RunError {
    let position = e.as_db_error().and_then(|d| match d.position() {
        Some(postgres::error::ErrorPosition::Original(p)) => Some(*p as usize),
        _ => None,
    });
    RunError {
        stmt,
        message: fmt_err(e),
        position,
    }
}

fn first_word(sql: &str) -> String {
    sql.split_whitespace().next().unwrap_or("").to_uppercase()
}

/// `UPDATE 3` for statements that count rows, `CREATE TABLE` for the rest.
fn tag(sql: &str, rows: Option<u64>) -> String {
    let verb = first_word(sql);
    match rows {
        Some(n) if matches!(verb.as_str(), "INSERT" | "UPDATE" | "DELETE" | "MERGE" | "SELECT" | "COPY" | "WITH") => {
            format!("{verb} {n}")
        }
        _ => sql.split_whitespace().take(2).collect::<Vec<_>>().join(" ").to_uppercase(),
    }
}

/// Rows of a simple-protocol response, as text, plus the completion count.
fn simple_rows(msgs: Vec<postgres::SimpleQueryMessage>) -> (Vec<Vec<Option<String>>>, Option<u64>) {
    let mut rows = vec![];
    let mut count = None;
    for m in msgs {
        match m {
            postgres::SimpleQueryMessage::Row(r) => {
                rows.push((0..r.len()).map(|i| r.get(i).map(String::from)).collect());
            }
            postgres::SimpleQueryMessage::CommandComplete(n) => count = Some(n),
            _ => {}
        }
    }
    (rows, count)
}

/// Runs `stmts` in one transaction and returns what each produced. Row sets
/// are read through a cursor, so at most `max_rows` are fetched.
pub fn run(
    c: &mut Client,
    stmts: &[&str],
    read_only: bool,
    max_rows: usize,
    timeout_ms: u64,
) -> Result<Vec<Outcome>, RunError> {
    match run_in_tx(c, stmts, read_only, max_rows, timeout_ms) {
        // VACUUM, CREATE INDEX CONCURRENTLY and the like refuse transactions.
        Err(e) if stmts.len() == 1 && !read_only && e.message.contains("cannot run inside a transaction block") => {
            let msgs = c.simple_query(stmts[0]).map_err(|e| run_err(0, &e))?;
            let (_, n) = simple_rows(msgs);
            Ok(vec![Outcome {
                tag: tag(stmts[0], n),
                columns: vec![],
                rows: vec![],
                more: false,
            }])
        }
        r => r,
    }
}

fn run_in_tx(
    c: &mut Client,
    stmts: &[&str],
    read_only: bool,
    max_rows: usize,
    timeout_ms: u64,
) -> Result<Vec<Outcome>, RunError> {
    let mut tx = c.build_transaction().read_only(read_only).start().map_err(|e| run_err(0, &e))?;
    if timeout_ms > 0 {
        tx.batch_execute(&format!("SET LOCAL statement_timeout = {timeout_ms}"))
            .map_err(|e| run_err(0, &e))?;
    }
    let mut out = vec![];
    for (k, sql) in stmts.iter().enumerate() {
        let prepared = tx.prepare(sql).map_err(|e| run_err(k, &e))?;
        if prepared.columns().is_empty() {
            let (_, n) = simple_rows(tx.simple_query(sql).map_err(|e| run_err(k, &e))?);
            out.push(Outcome {
                tag: tag(sql, n),
                columns: vec![],
                rows: vec![],
                more: false,
            });
            continue;
        }
        let columns: Vec<ResultCol> = prepared
            .columns()
            .iter()
            .map(|col| ResultCol {
                name: col.name().to_string(),
                type_name: col.type_().name().to_string(),
                table_oid: col.table_oid().unwrap_or(0),
                column_id: col.column_id().unwrap_or(0),
            })
            .collect();
        // A cursor bounds what's fetched; DML with RETURNING can't be a
        // cursor, so that falls back to reading everything.
        tx.batch_execute("SAVEPOINT zpg_run").map_err(|e| run_err(k, &e))?;
        let mut rows = match tx.batch_execute(&format!("DECLARE zpg_cursor NO SCROLL CURSOR FOR {sql}")) {
            Ok(()) => {
                let msgs = tx
                    .simple_query(&format!("FETCH FORWARD {} FROM zpg_cursor", max_rows + 1))
                    .map_err(|e| run_err(k, &e))?;
                tx.batch_execute("CLOSE zpg_cursor; RELEASE SAVEPOINT zpg_run")
                    .map_err(|e| run_err(k, &e))?;
                simple_rows(msgs).0
            }
            Err(_) => {
                tx.batch_execute("ROLLBACK TO SAVEPOINT zpg_run").map_err(|e| run_err(k, &e))?;
                simple_rows(tx.simple_query(sql).map_err(|e| run_err(k, &e))?).0
            }
        };
        let more = rows.len() > max_rows;
        rows.truncate(max_rows);
        out.push(Outcome {
            tag: format!("{} {}{}", first_word(sql), rows.len(), if more { "+" } else { "" }),
            columns,
            rows,
            more,
        });
    }
    tx.commit().map_err(|e| run_err(stmts.len().saturating_sub(1), &e))?;
    Ok(out)
}

/// Where an editable result comes from: one table, with its whole key selected.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EditSource {
    pub table: QualName,
    /// (table column, result column) for each key column.
    pub pk: Vec<(String, String)>,
    /// Table columns that the result shows.
    pub columns: Vec<String>,
}

pub fn edit_source(c: &mut Client, cols: &[ResultCol]) -> Option<EditSource> {
    let oid = cols.first()?.table_oid;
    if oid == 0 || cols.iter().any(|c| c.table_oid != oid || c.column_id <= 0) {
        return None;
    }
    let mut tx = c.build_transaction().read_only(true).start().ok()?;
    let rel = tx
        .query_opt(
            "SELECT n.nspname::text, c.relname::text FROM pg_class c
             JOIN pg_namespace n ON n.oid = c.relnamespace
             WHERE c.oid = $1 AND c.relkind IN ('r', 'p')",
            &[&oid],
        )
        .ok()??;
    let names: HashMap<i16, String> = tx
        .query(
            "SELECT attnum, attname::text FROM pg_attribute WHERE attrelid = $1 AND attnum > 0",
            &[&oid],
        )
        .ok()?
        .iter()
        .map(|r| (r.get(0), r.get(1)))
        .collect();
    let pk: Vec<i16> = tx
        .query(
            "SELECT unnest(indkey::int2[]) FROM pg_index WHERE indrelid = $1 AND indisprimary",
            &[&oid],
        )
        .ok()?
        .iter()
        .map(|r| r.get(0))
        .collect();
    let _ = tx.commit();
    if pk.is_empty() {
        return None;
    }
    let pk = pk
        .iter()
        .map(|a| {
            let result_col = cols.iter().find(|c| c.column_id == *a)?;
            Some((names.get(a)?.clone(), result_col.name.clone()))
        })
        .collect::<Option<Vec<_>>>()?;
    let mut columns: Vec<String> = vec![];
    for c in cols {
        let n = names.get(&c.column_id)?;
        if !columns.contains(n) {
            columns.push(n.clone());
        }
    }
    Some(EditSource {
        table: QualName::new(rel.get::<_, String>(0), rel.get::<_, String>(1)),
        pk,
        columns,
    })
}
