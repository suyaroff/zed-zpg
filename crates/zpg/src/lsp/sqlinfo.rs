//! Completion, hover and go-to-definition of table and column names in `.sql` files.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use lsp_types::{CompletionItem, CompletionItemKind, Hover, Position, Range};
use zpg_core::header::display_ident;
use zpg_core::sqlctx::{Candidate, Schema, SchemaTable, Target};
use zpg_core::{Column, catalog, sqlsplit, utf16_len};

use super::complete::item;
use super::query::position_to_offset;
use super::{Kind, Server, markdown_hover};
use crate::db;
use crate::state;

/// How long tables and columns read for a connection are trusted; DDL run
/// from zpg and *Refresh catalog* drop them sooner.
const SCHEMA_TTL: Duration = Duration::from_secs(300);
/// After a failed read, don't reconnect on every keystroke.
const FAILED_TTL: Duration = Duration::from_secs(30);
/// Columns listed in a table's hover.
const HOVER_COLUMNS: usize = 60;

/// The statement around the cursor in a `.sql` buffer.
struct SqlAt {
    stmt: String,
    /// Byte offset of the cursor in `stmt`.
    offset: usize,
    conn: String,
    path: PathBuf,
}

fn rows_note(t: &SchemaTable) -> String {
    t.rel.rows.map(|n| format!(" · ~{n} rows")).unwrap_or_default()
}

fn column_flags(c: &Column) -> String {
    let mut s = String::new();
    if c.not_null {
        s += " · not null";
    }
    if c.generated {
        s += " · generated";
    } else if c.has_default {
        s += " · has default";
    }
    s
}

/// `note` ends the title line; `comment` goes under it.
fn table_md(t: &SchemaTable, note: &str, comment: Option<&str>) -> String {
    let mut md = format!("**{}** · {}{}", t.rel.name, t.rel.kind, rows_note(t));
    if !note.is_empty() {
        md += &format!(" · {note}");
    }
    md.push('\n');
    if let Some(c) = comment {
        md += &format!("\n{c}\n");
    }
    for c in t.columns.iter().take(HOVER_COLUMNS) {
        md += &format!("\n- `{}` {}{}", display_ident(&c.name), c.type_name, column_flags(c));
        if let Some(comment) = &c.comment {
            md += &format!(" — {}", comment.lines().next().unwrap_or(""));
        }
    }
    if t.columns.len() > HOVER_COLUMNS {
        md += &format!("\n- … {} more", t.columns.len() - HOVER_COLUMNS);
    }
    md
}

impl Server {
    fn sql_at(&self, key: &str, pos: Position) -> Option<SqlAt> {
        let doc = self.docs.get(key).filter(|d| d.kind() == Kind::Sql)?;
        let at = position_to_offset(&doc.text, pos);
        let r = sqlsplit::around(&doc.text, at);
        let conn = self.conns_for(&sqlsplit::conn_directives(&doc.text), r.start).into_iter().next()?;
        Some(SqlAt {
            stmt: doc.text[r.clone()].to_string(),
            offset: at - r.start,
            conn,
            path: doc.path()?,
        })
    }

    /// Tables and columns of `conn`, read on first use.
    fn schema(&mut self, conn: &str) -> Option<&Schema> {
        let fresh = self.schemas.get(conn).is_some_and(|(at, s)| {
            at.elapsed() < if s.is_some() { SCHEMA_TTL } else { FAILED_TTL }
        });
        if !fresh {
            let schema = self.db.client(&self.settings, conn).and_then(db::schema).ok();
            self.schemas.insert(conn.to_string(), (Instant::now(), schema));
        }
        self.schemas.get(conn)?.1.as_ref()
    }

    pub(super) fn sql_completion(&mut self, key: &str, pos: Position) -> Vec<CompletionItem> {
        let Some(at) = self.sql_at(key, pos) else {
            return vec![];
        };
        let Some(schema) = self.schema(&at.conn) else {
            return vec![];
        };
        let Some((start, candidates)) = schema.complete(&at.stmt, at.offset) else {
            return vec![];
        };
        let typed = utf16_len(&at.stmt[start..at.offset]);
        let range = Range::new(Position::new(pos.line, pos.character.saturating_sub(typed)), pos);
        candidates
            .into_iter()
            .map(|c| {
                let (rank, mut it) = match c {
                    Candidate::Column(t, col) => {
                        let detail = format!("{} · {}", col.type_name, t.short_name());
                        (0, item(&display_ident(&col.name), CompletionItemKind::FIELD, Some(&detail), range))
                    }
                    Candidate::Table(t, qualified) => {
                        let label = if qualified { t.rel.name.to_string() } else { display_ident(&t.rel.name.name) };
                        let kind = match t.rel.kind.as_str() {
                            "table" | "partitioned" | "foreign" => CompletionItemKind::STRUCT,
                            _ => CompletionItemKind::INTERFACE,
                        };
                        let detail = format!("{}{}", t.rel.kind, rows_note(t));
                        (1, item(&label, kind, Some(&detail), range))
                    }
                    Candidate::Schema(s) => (2, item(&display_ident(s), CompletionItemKind::MODULE, Some("schema"), range)),
                    Candidate::Keyword(k) => (3, item(k, CompletionItemKind::KEYWORD, None, range)),
                };
                it.sort_text = Some(format!("{rank}{}", it.label));
                it
            })
            .collect()
    }

    pub(super) fn sql_hover(&mut self, key: &str, pos: Position) -> Option<Hover> {
        let at = self.sql_at(key, pos)?;
        let conn = at.conn.clone();
        let md = match self.schema(&at.conn)?.resolve(&at.stmt, at.offset)? {
            Target::Table(t) => table_md(t, &conn, None),
            Target::Column(t, c) => {
                let mut md = format!(
                    "**{}.{}** `{}`{}",
                    t.short_name(),
                    display_ident(&c.name),
                    c.type_name,
                    column_flags(c)
                );
                if let Some(comment) = &c.comment {
                    md += &format!("\n\n{comment}");
                }
                md
            }
        };
        Some(markdown_hover(md))
    }

    /// A catalog line: the relation's columns, size and indexes.
    pub(super) fn catalog_hover(&mut self, text: &str, pos: Position) -> Option<Hover> {
        let (conn, table) = catalog::entry_at(text, pos.line)?;
        let details = self.db.client(&self.settings, &conn).and_then(|c| db::details(c, &table)).ok();
        let t = self.schema(&conn)?.table(Some(&table.schema), &table.name)?;
        let note = details.as_ref().map_or(String::new(), |d| d.size.clone());
        let mut md = table_md(t, &note, details.as_ref().and_then(|d| d.comment.as_deref()));
        if let Some(d) = details.filter(|d| !d.indexes.is_empty()) {
            md += "\n\n**Indexes**\n";
            for i in d.indexes {
                md += &format!("\n- {i}");
            }
        }
        Some(markdown_hover(md))
    }

    /// F12 on a table (or a column) opens the table's page.
    pub(super) fn sql_definition(&mut self, key: &str, pos: Position) -> Result<Option<PathBuf>, String> {
        let Some(at) = self.sql_at(key, pos) else {
            return Ok(None);
        };
        let table = match self.schema(&at.conn).and_then(|s| s.resolve(&at.stmt, at.offset)) {
            Some(Target::Table(t) | Target::Column(t, _)) => t.rel.name.clone(),
            None => return Ok(None),
        };
        let root = self.zpg_root(&at.path);
        state::ensure_root(&root)?;
        self.open_table(&root, &at.conn, &table).map(Some)
    }
}
