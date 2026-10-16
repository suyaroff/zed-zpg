use std::fmt;

use serde::{Deserialize, Serialize};

use crate::MAGIC;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct QualName {
    pub schema: String,
    pub name: String,
}

pub fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// Quotes only when Postgres would otherwise fold case or reject the name.
pub fn display_ident(s: &str) -> String {
    let mut chars = s.chars();
    let plain = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '$');
    if plain { s.to_string() } else { quote_ident(s) }
}

impl QualName {
    pub fn new(schema: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            schema: schema.into(),
            name: name.into(),
        }
    }

    pub fn sql(&self) -> String {
        format!("{}.{}", quote_ident(&self.schema), quote_ident(&self.name))
    }

    /// Parses `schema.name`, where either part may be a double-quoted identifier.
    /// Returns the name and the number of bytes consumed.
    pub fn parse_prefix(s: &str) -> Option<(Self, usize)> {
        let (schema, a) = parse_ident(s)?;
        let rest = s[a..].strip_prefix('.')?;
        let (name, b) = parse_ident(rest)?;
        Some((Self::new(schema, name), a + 1 + b))
    }

    pub fn file_stem(&self) -> String {
        let enc = |s: &str| {
            s.chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == '_' || c == '-' {
                        c.to_string()
                    } else {
                        c.to_string().bytes().map(|b| format!("%{b:02X}")).collect()
                    }
                })
                .collect::<String>()
        };
        format!("{}.{}", enc(&self.schema), enc(&self.name))
    }
}

impl fmt::Display for QualName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", display_ident(&self.schema), display_ident(&self.name))
    }
}

impl std::str::FromStr for QualName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match Self::parse_prefix(s) {
            Some((q, n)) if n == s.len() => Ok(q),
            _ => Err(format!("expected schema.table, got `{s}`")),
        }
    }
}

/// `a, "B c", d` -> names; unquoted names are folded to lower case.
pub fn parse_ident_list(s: &str) -> Result<Vec<String>, String> {
    s.split_terminator(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| match parse_ident(p) {
            Some((name, n)) if n == p.len() => Ok(name),
            _ => Err(format!("`{p}` is not a column name")),
        })
        .collect()
}

fn parse_ident(s: &str) -> Option<(String, usize)> {
    if let Some(rest) = s.strip_prefix('"') {
        let mut out = String::new();
        let mut it = rest.char_indices().peekable();
        while let Some((i, c)) = it.next() {
            if c == '"' {
                if it.peek().is_some_and(|&(_, n)| n == '"') {
                    it.next();
                    out.push('"');
                } else {
                    return Some((out, i + 2));
                }
            } else {
                out.push(c);
            }
        }
        None
    } else {
        let end = s
            .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
            .unwrap_or(s.len());
        (end > 0).then(|| (s[..end].to_lowercase(), end))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Apply {
    Save,
    /// Default: only the Apply command writes, so a stray save can't delete rows.
    #[default]
    Manual,
}

/// Which rows of the table a buffer shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Query {
    /// 1-based.
    pub page: u32,
    pub limit: u32,
    pub filter: String,
    pub order: String,
}

impl Query {
    pub fn offset(&self) -> u64 {
        u64::from(self.page.saturating_sub(1)) * u64::from(self.limit)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableHeader {
    pub conn: String,
    pub table: QualName,
    /// Informational; the snapshot holds the real key.
    pub pk: String,
    pub query: Query,
    /// Max display width of a cell; longer values are cut and become read-only. 0 = no limit.
    pub width: u32,
    pub apply: Apply,
    /// Columns to show; empty = all. Key columns are always shown.
    pub columns: Vec<String>,
}

pub fn quote_value(v: &str) -> String {
    if !v.is_empty() && !v.contains([' ', '"', '\\', '\t']) {
        return v.to_string();
    }
    format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Header fields whose unquoted value may have spaces: it runs up to the
/// next ` field=`, so `order=id desc` needs no quotes.
const SPACED: &[&str] = &["where", "order", "columns"];
const TABLE_FIELDS: &[&str] = &["conn", "table", "pk", "page", "limit", "width", "apply", "where", "order", "columns"];

/// Splits `key=value key="quoted value"` pairs; unquoted values of
/// `SPACED` keys extend to the next ` known=`.
pub fn parse_pairs(s: &str, known: &[&str]) -> Result<Vec<(String, String)>, String> {
    let mut out = vec![];
    let mut it = s.chars().peekable();
    loop {
        while it.peek().is_some_and(|c| c.is_whitespace()) {
            it.next();
        }
        if it.peek().is_none() {
            return Ok(out);
        }
        let mut key = String::new();
        while let Some(&c) = it.peek() {
            if c == '=' || c.is_whitespace() {
                break;
            }
            key.push(c);
            it.next();
        }
        if it.next() != Some('=') {
            return Err(format!("expected `{key}=value`"));
        }
        let mut val = String::new();
        if it.peek() == Some(&'"') {
            it.next();
            loop {
                match it.next() {
                    Some('"') => break,
                    Some('\\') => match it.next() {
                        Some(c) => val.push(c),
                        None => return Err(format!("unterminated value of `{key}`")),
                    },
                    Some(c) => val.push(c),
                    None => return Err(format!("unterminated value of `{key}`")),
                }
            }
        } else if SPACED.contains(&key.as_str()) {
            let rest: String = it.clone().collect();
            let end = next_field(&rest, known).unwrap_or(rest.len());
            val = rest[..end].trim_end().to_string();
            for _ in rest[..end].chars() {
                it.next();
            }
        } else {
            while let Some(&c) = it.peek() {
                if c.is_whitespace() {
                    break;
                }
                val.push(c);
                it.next();
            }
        }
        out.push((key, val));
    }
}

/// Byte offset of the whitespace before the next ` known=` in `s`.
fn next_field(s: &str, known: &[&str]) -> Option<usize> {
    s.char_indices().find_map(|(i, c)| {
        let after = s[i..].trim_start();
        let starts = c.is_whitespace() && known.iter().any(|k| after.strip_prefix(k).is_some_and(|r| r.starts_with('=')));
        starts.then_some(i)
    })
}

impl TableHeader {
    pub fn render(&self) -> String {
        let mut s = format!(
            "{MAGIC} conn={} table={} pk={} page={} limit={} width={} apply={}",
            quote_value(&self.conn),
            quote_value(&self.table.to_string()),
            quote_value(&self.pk),
            self.query.page,
            self.query.limit,
            self.width,
            match self.apply {
                Apply::Save => "save",
                Apply::Manual => "manual",
            },
        );
        if !self.query.filter.is_empty() {
            s += &format!(" where={}", quote_value(&self.query.filter));
        }
        if !self.query.order.is_empty() {
            s += &format!(" order={}", quote_value(&self.query.order));
        }
        if !self.columns.is_empty() {
            let names: Vec<String> = self.columns.iter().map(|c| display_ident(c)).collect();
            s += &format!(" columns={}", quote_value(&names.join(", ")));
        }
        s
    }

    pub fn parse(line: &str) -> Result<Self, String> {
        let rest = line
            .strip_prefix(MAGIC)
            .ok_or_else(|| format!("first line must start with `{MAGIC}`"))?;
        let mut conn = None;
        let mut table = None;
        let mut h = TableHeader {
            conn: String::new(),
            table: QualName::new("", ""),
            pk: String::new(),
            query: Query {
                page: 1,
                limit: 100,
                filter: String::new(),
                order: String::new(),
            },
            width: 40,
            apply: Apply::Manual,
            columns: vec![],
        };
        let num = |k: &str, v: &str| v.parse::<u32>().map_err(|_| format!("`{k}` must be a number, got `{v}`"));
        for (k, v) in parse_pairs(rest, TABLE_FIELDS)? {
            match k.as_str() {
                "conn" => conn = Some(v),
                "table" => table = Some(v.parse::<QualName>()?),
                "pk" => h.pk = v,
                "page" => h.query.page = num(&k, &v)?.max(1),
                "limit" => h.query.limit = num(&k, &v)?.clamp(1, 10_000),
                "width" => h.width = num(&k, &v)?,
                "where" => h.query.filter = v,
                "order" => h.query.order = v,
                "columns" => h.columns = parse_ident_list(&v)?,
                "apply" => {
                    h.apply = match v.as_str() {
                        "save" => Apply::Save,
                        "manual" => Apply::Manual,
                        _ => return Err(format!("`apply` must be save or manual, got `{v}`")),
                    }
                }
                _ => {
                    return Err(format!(
                        "unknown header field `{k}`; fields: where, order, columns, page, limit, width, apply"
                    ));
                }
            }
        }
        h.conn = conn.ok_or("header has no `conn`")?;
        h.table = table.ok_or("header has no `table`")?;
        Ok(h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trip() {
        let h = TableHeader {
            conn: "local".into(),
            table: QualName::new("public", "My Table"),
            pk: "id".into(),
            query: Query {
                page: 3,
                limit: 50,
                filter: "name = 'a \"b\"'".into(),
                order: "id desc".into(),
            },
            width: 0,
            apply: Apply::Manual,
            columns: vec!["id".into(), "Full Name".into()],
        };
        let line = h.render();
        assert!(line.ends_with(r#" columns="id, \"Full Name\"""#), "{line}");
        assert_eq!(TableHeader::parse(&line).unwrap(), h);
    }

    #[test]
    fn unquoted_spaced_values() {
        let h = TableHeader::parse("# zpg v1 conn=c table=public.t order=id desc where=a = 'x y' and b > 2 page=2").unwrap();
        assert_eq!(h.query.order, "id desc");
        assert_eq!(h.query.filter, "a = 'x y' and b > 2");
        assert_eq!(h.query.page, 2);
        let h = TableHeader::parse("# zpg v1 conn=c table=public.t apply=manual order=name").unwrap();
        assert_eq!((h.query.order.as_str(), h.apply), ("name", Apply::Manual));
        assert!(h.render().ends_with("order=name"));
        assert!(TableHeader::parse("# zpg v1 conn=c table=public.t order=id desc").unwrap().render().contains(r#"order="id desc""#));
    }

    #[test]
    fn unknown_field_lists_valid_ones() {
        let e = TableHeader::parse("# zpg v1 conn=c table=public.t sort=id").unwrap_err();
        assert!(e.contains("`sort`") && e.contains("order"), "{e}");
    }

    #[test]
    fn qualname_parse() {
        assert_eq!("Public.Users".parse::<QualName>().unwrap(), QualName::new("public", "users"));
        assert_eq!(
            r#"s."a""b.c""#.parse::<QualName>().unwrap(),
            QualName::new("s", "a\"b.c")
        );
        assert_eq!(QualName::new("s", "a\"b.c").to_string(), r#"s."a""b.c""#);
    }
}
