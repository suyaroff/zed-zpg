//! The `.pgr` layout: one row, one `column: value` field per line.
//!
//! Multi-line text is written as a block, indented by two spaces:
//!
//! ```text
//! note: |
//!   first line
//!   second line
//! ```

use crate::cell::{self, Value};
use crate::diff::{Diff, Op, Source};
use crate::header::{QualName, parse_pairs, quote_value};
use crate::{Issue, Snapshot, utf16_len};

pub const ROW_MAGIC: &str = "# zpg v1 row";

const INDENT: &str = "  ";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowHeader {
    pub conn: String,
    pub table: QualName,
    /// Informational.
    pub key: String,
}

impl RowHeader {
    pub fn render(&self) -> String {
        format!(
            "{ROW_MAGIC} conn={} table={} key={}",
            quote_value(&self.conn),
            quote_value(&self.table.to_string()),
            quote_value(&self.key)
        )
    }

    pub fn parse(line: &str) -> Result<Self, String> {
        let rest = line
            .strip_prefix(ROW_MAGIC)
            .ok_or_else(|| format!("first line must start with `{ROW_MAGIC}`"))?;
        let (mut conn, mut table, mut key) = (None, None, String::new());
        for (k, v) in parse_pairs(rest, &["conn", "table", "key"])? {
            match k.as_str() {
                "conn" => conn = Some(v),
                "table" => table = Some(v.parse::<QualName>()?),
                "key" => key = v,
                _ => return Err(format!("unknown header field `{k}`")),
            }
        }
        Ok(Self {
            conn: conn.ok_or("header has no `conn`")?,
            table: table.ok_or("header has no `table`")?,
            key,
        })
    }
}

/// A block survives a round trip through Zed only if no line has trailing
/// whitespace (stripped on save) and there's no trailing newline or CR.
fn as_block(s: &str) -> bool {
    s.contains('\n')
        && !s.ends_with('\n')
        && !s.contains('\r')
        && s.split('\n').all(|l| l == l.trim_end() && !l.chars().any(|c| c.is_control() && c != '\t'))
}

/// Lines of one field.
pub fn render_field(name: &str, v: &Option<String>) -> Vec<String> {
    match v {
        Some(s) if as_block(s) => {
            let mut out = vec![format!("{name}: |")];
            out.extend(s.split('\n').map(|l| if l.is_empty() { String::new() } else { format!("{INDENT}{l}") }));
            out
        }
        Some(s) if s.is_empty() => vec![format!("{name}:")],
        _ => vec![format!("{name}: {}", cell::render(&Value::from_db(v.as_deref())))],
    }
}

pub fn render(snap: &Snapshot, key: &str) -> String {
    let header = RowHeader {
        conn: snap.conn.clone(),
        table: snap.table.clone(),
        key: key.to_string(),
    };
    let mut out = header.render();
    out.push('\n');
    if let Some(row) = snap.rows.first() {
        for (c, v) in snap.columns.iter().zip(&row.values) {
            for l in render_field(&c.name, v) {
                out += &l;
                out.push('\n');
            }
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub col: usize,
    /// First line of the field.
    pub line: u32,
    /// UTF-16 range of the value on the first line (empty for a block marker).
    pub start: u32,
    pub end: u32,
    /// All lines of the field, trailing whitespace removed.
    pub lines: Vec<String>,
    pub value: Result<Value, String>,
}

#[derive(Debug, Default)]
pub struct ParsedRow {
    pub header: Option<RowHeader>,
    pub fields: Vec<Field>,
    pub issues: Vec<Issue>,
}

impl ParsedRow {
    /// The parsed value of column `col`, if its field parsed.
    pub fn value_of(&self, col: usize) -> Option<Value> {
        self.fields.iter().find(|f| f.col == col).and_then(|f| f.value.clone().ok())
    }

    pub fn field_at(&self, line: u32) -> Option<&Field> {
        self.fields
            .iter()
            .rev()
            .find(|f| f.line <= line && line < f.line + f.lines.len() as u32)
    }
}

pub fn parse(text: &str, snap: &Snapshot) -> ParsedRow {
    let mut p = ParsedRow::default();
    let lines: Vec<&str> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    match lines.first().map(|l| RowHeader::parse(l)) {
        Some(Ok(h)) => p.header = Some(h),
        Some(Err(e)) => p.issues.push(Issue::line_error(0, lines[0], e)),
        None => return p,
    }
    let mut i = 1;
    let mut next_col = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() || line.starts_with('#') {
            i += 1;
            continue;
        }
        let Some(c) = snap.columns.get(next_col) else {
            p.issues.push(Issue::line_error(i as u32, line, "unexpected line after the last field"));
            i += 1;
            continue;
        };
        let Some(rest) = line.strip_prefix(&c.name).and_then(|r| r.strip_prefix(':')) else {
            p.issues.push(Issue::line_error(i as u32, line, format!("expected field `{}:`", c.name)));
            return p;
        };
        let start = utf16_len(&line[..line.len() - rest.len()]) + u32::from(rest.starts_with(' '));
        let raw = rest.strip_prefix(' ').unwrap_or(rest);
        let first = i;
        let mut field_lines = vec![line.trim_end().to_string()];
        let value = if raw.trim_end() == "|" {
            let mut body = vec![];
            let mut j = i + 1;
            loop {
                // Blank lines belong to the block only if more indented lines follow.
                let mut k = j;
                while k < lines.len() && lines[k].trim().is_empty() {
                    k += 1;
                }
                if k >= lines.len() || !lines[k].starts_with(INDENT) {
                    break;
                }
                for l in &lines[j..k] {
                    body.push(String::new());
                    field_lines.push(l.trim_end().to_string());
                }
                body.push(lines[k][INDENT.len()..].trim_end().to_string());
                field_lines.push(lines[k].trim_end().to_string());
                j = k + 1;
            }
            i = j;
            if body.is_empty() {
                Err("empty block; write `name:` for an empty string".to_string())
            } else {
                Ok(Value::Text(body.join("\n")))
            }
        } else {
            i += 1;
            match cell::parse(raw.trim()) {
                Ok(Value::Plus) => Err("`+` has no meaning here".into()),
                v => v,
            }
        };
        if let Err(e) = &value {
            p.issues.push(Issue::error(first as u32, start, utf16_len(line).max(start + 1), e.clone()));
        }
        p.fields.push(Field {
            col: next_col,
            line: first as u32,
            start,
            end: utf16_len(line),
            lines: field_lines,
            value,
        });
        next_col += 1;
    }
    if next_col < snap.columns.len() && p.issues.is_empty() {
        let last = lines.len().saturating_sub(1) as u32;
        p.issues.push(Issue::error(
            last,
            0,
            1,
            format!("missing field `{}:`", snap.columns[next_col].name),
        ));
    }
    p
}

/// The row's changes as at most one UPDATE.
pub fn diff(parsed: &ParsedRow, snap: &Snapshot) -> Diff {
    let mut d = Diff::default();
    let Some(row) = snap.rows.first() else {
        return d;
    };
    if let Some(h) = &parsed.header
        && (h.conn != snap.conn || h.table != snap.table)
    {
        d.issues.push(Issue::header(format!("`conn` and `table` must stay {} / {}", snap.conn, snap.table)));
    }
    let mut sets = vec![];
    let mut first_line = None;
    for f in &parsed.fields {
        let old = &row.values[f.col];
        if render_field(&snap.columns[f.col].name, old) == f.lines {
            continue;
        }
        let Ok(v) = &f.value else { continue };
        if *v == Value::from_db(old.as_deref()) {
            continue;
        }
        let issue = |m: &str| Issue::error(f.line, f.start, f.end.max(f.start + 1), m);
        if let Some(why) = &snap.read_only {
            d.issues.push(issue(&format!("read-only: {why}")));
        } else if snap.pk.contains(&f.col) {
            d.issues.push(issue("key fields can't be changed here"));
        } else if snap.columns[f.col].generated && *v != Value::Default {
            d.issues.push(issue("generated column: only DEFAULT can be written"));
        } else {
            first_line.get_or_insert(f.line);
            sets.push((f.col, v.clone()));
        }
    }
    if !sets.is_empty() {
        d.ops.push(Op::Update {
            line: first_line.unwrap_or(0),
            row: 0,
            sets,
        });
        d.order.push(Source::Op(0));
    } else {
        d.order.push(Source::Snapshot(0));
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::{Apply, Query};
    use crate::{Column, SnapRow};

    fn snap(values: Vec<Option<&str>>) -> Snapshot {
        let col = |n: &str| Column {
            name: n.into(),
            type_name: "text".into(),
            ..Default::default()
        };
        Snapshot {
            conn: "c".into(),
            table: QualName::new("public", "t"),
            columns: vec![col("id"), col("note"), col("meta"), col("empty")],
            pk: vec![0],
            foreign_keys: vec![],
            referenced_by: vec![],
            total: None,
            read_only: None,
            query: Query::first(1),
            width: 0,
            apply: Apply::Save,
            visible: vec![],
            rows: vec![SnapRow {
                xmin: Some("1".into()),
                values: values.into_iter().map(|v| v.map(Into::into)).collect(),
            }],
        }
    }

    #[test]
    fn round_trip_unchanged() {
        let s = snap(vec![Some("1"), Some("a\n\n  b\nc"), Some("x |\n"), Some("")]);
        let text = render(&s, "id=1");
        assert_eq!(
            text,
            "# zpg v1 row conn=c table=public.t key=id=1\nid: 1\nnote: |\n  a\n\n    b\n  c\nmeta: x \\|\\n\nempty:\n"
        );
        let p = parse(&text, &s);
        assert!(p.issues.is_empty(), "{:?}", p.issues);
        assert_eq!(p.fields[1].value, Ok(Value::Text("a\n\n  b\nc".into())));
        assert_eq!(p.fields[2].value, Ok(Value::Text("x |\n".into())));
        assert_eq!(p.fields[3].value, Ok(Value::Text(String::new())));
        let d = diff(&p, &s);
        assert!(d.ops.is_empty() && d.issues.is_empty(), "{d:?}");
    }

    #[test]
    fn edits() {
        let s = snap(vec![Some("1"), Some("a\nb"), None, Some("")]);
        let text = render(&s, "id=1").replace("  b\n", "  b2\n  b3\n").replace("meta: NULL", "meta: {\"k\": 1}");
        let p = parse(&text, &s);
        let d = diff(&p, &s);
        assert!(d.issues.is_empty(), "{:?}", d.issues);
        assert_eq!(
            d.ops,
            vec![Op::Update {
                line: 2,
                row: 0,
                sets: vec![(1, Value::Text("a\nb2\nb3".into())), (2, Value::Text("{\"k\": 1}".into()))]
            }]
        );
        let bad = render(&s, "id=1").replace("id: 1", "id: 2");
        assert!(diff(&parse(&bad, &s), &s).issues[0].message.contains("key"));
        let missing = render(&s, "id=1").replace("empty:\n", "");
        assert!(parse(&missing, &s).issues[0].message.contains("missing field"));
    }
}
