//! What the cursor is on in a SQL statement, for completion, hover and
//! go-to-definition in `.sql` files: a light tokenizer and a scan for the
//! tables a statement names, not a SQL parser.

use crate::catalog::Relation;
use crate::header::display_ident;
use crate::snapshot::Column;
use crate::sqlsplit::{dollar_tag, is_ident};

/// Where an unqualified table name is looked up first.
pub const DEFAULT_SCHEMA: &str = "public";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    /// Unquoted names are folded to lower case.
    Ident { name: String, quoted: bool },
    Dot,
    Comma,
    Open,
    Close,
    /// A string literal; `closed` is false when it runs to the end of the text.
    Str { closed: bool },
    Comment { closed: bool },
    Other,
}

#[derive(Debug, Clone)]
struct Token {
    tok: Tok,
    start: usize,
    end: usize,
}

impl Token {
    fn ident(&self) -> Option<&str> {
        match &self.tok {
            Tok::Ident { name, .. } => Some(name),
            _ => None,
        }
    }

    /// An unquoted identifier equal to one of `words`.
    fn is_word(&self, words: &[&str]) -> bool {
        matches!(&self.tok, Tok::Ident { name, quoted: false } if words.contains(&name.as_str()))
    }
}

fn tokens(sql: &str) -> Vec<Token> {
    let b = sql.as_bytes();
    let ident_byte = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c >= 0x80;
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        let start = i;
        let tok = match b[i] {
            c if c.is_ascii_whitespace() => {
                i += 1;
                continue;
            }
            b'-' if b.get(i + 1) == Some(&b'-') => {
                i = sql[i..].find('\n').map_or(b.len(), |p| i + p);
                Tok::Comment { closed: false }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => match sql[i + 2..].find("*/") {
                Some(p) => {
                    i += 2 + p + 2;
                    Tok::Comment { closed: true }
                }
                None => {
                    i = b.len();
                    Tok::Comment { closed: false }
                }
            },
            b'\'' => {
                let backslash = i > 0 && matches!(b[i - 1], b'E' | b'e') && !is_ident(b, i - 1);
                i += 1;
                let mut closed = false;
                while i < b.len() {
                    let escaped = backslash && b[i] == b'\\';
                    if escaped || (b[i] == b'\'' && b.get(i + 1) == Some(&b'\'')) {
                        i += 2;
                    } else if b[i] == b'\'' {
                        i += 1;
                        closed = true;
                        break;
                    } else {
                        i += 1;
                    }
                }
                i = i.min(b.len());
                Tok::Str { closed }
            }
            b'"' => {
                let mut name = String::new();
                i += 1;
                loop {
                    match sql[i..].find('"') {
                        Some(p) if b.get(i + p + 1) == Some(&b'"') => {
                            name += &sql[i..i + p + 1];
                            i += p + 2;
                        }
                        Some(p) => {
                            name += &sql[i..i + p];
                            i += p + 1;
                            break;
                        }
                        None => {
                            name += &sql[i..];
                            i = b.len();
                            break;
                        }
                    }
                }
                Tok::Ident { name, quoted: true }
            }
            b'$' if !is_ident(b, i) && dollar_tag(&sql[i..]).is_some() => {
                let tag = dollar_tag(&sql[i..]).unwrap_or("$$");
                let body = i + tag.len();
                match sql[body..].find(tag) {
                    Some(p) => {
                        i = body + p + tag.len();
                        Tok::Str { closed: true }
                    }
                    None => {
                        i = b.len();
                        Tok::Str { closed: false }
                    }
                }
            }
            b'.' => {
                i += 1;
                Tok::Dot
            }
            b',' => {
                i += 1;
                Tok::Comma
            }
            b'(' => {
                i += 1;
                Tok::Open
            }
            b')' => {
                i += 1;
                Tok::Close
            }
            c if c.is_ascii_digit() => {
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.' || b[i] == b'_') {
                    i += 1;
                }
                Tok::Other
            }
            c if ident_byte(c) => {
                while i < b.len() && ident_byte(b[i]) {
                    i += 1;
                }
                Tok::Ident {
                    name: sql[start..i].to_lowercase(),
                    quoted: false,
                }
            }
            _ => {
                i += 1;
                Tok::Other
            }
        };
        out.push(Token { tok, start, end: i });
    }
    out
}

/// Tokens without comments.
fn code(sql: &str) -> Vec<Token> {
    tokens(sql).into_iter().filter(|t| !matches!(t.tok, Tok::Comment { .. })).collect()
}

/// Words after which a name can't be an alias.
const NOT_ALIAS: &[&str] = &[
    "where", "join", "inner", "left", "right", "full", "cross", "outer", "natural", "on", "using", "group", "order",
    "having", "limit", "offset", "fetch", "for", "window", "union", "except", "intersect", "returning", "set",
    "values", "select", "lateral", "when", "then", "and", "or", "default", "do", "tablesample",
];

/// A table named in a statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRef {
    pub schema: Option<String>,
    pub name: String,
    pub alias: Option<String>,
}

/// `name` or `schema.name` starting at token `i`; the parts and the index after them.
fn dotted(toks: &[Token], i: usize) -> Option<(Vec<String>, usize)> {
    let mut parts = vec![toks.get(i)?.ident()?.to_string()];
    let mut j = i + 1;
    while toks.get(j).is_some_and(|t| t.tok == Tok::Dot) {
        let Some(name) = toks.get(j + 1).and_then(Token::ident) else {
            break;
        };
        parts.push(name.to_string());
        j += 2;
    }
    Some((parts, j))
}

/// Tables after FROM / JOIN / UPDATE / INTO / TABLE / TRUNCATE (and the rest of a
/// comma-separated FROM list), with their aliases.
pub fn tables_in(sql: &str) -> Vec<TableRef> {
    let toks = code(sql);
    let mut out = vec![];
    let mut i = 0;
    while i < toks.len() {
        if !toks[i].is_word(&["from", "join", "update", "into", "table", "truncate"]) {
            i += 1;
            continue;
        }
        let from = toks[i].is_word(&["from"]);
        let into = toks[i].is_word(&["into"]);
        i += 1;
        loop {
            while toks.get(i).is_some_and(|t| t.is_word(&["only", "lateral", "if", "not", "exists", "table"])) {
                i += 1;
            }
            let Some((parts, next)) = dotted(&toks, i).filter(|(p, _)| p.len() <= 2) else {
                break;
            };
            if toks.get(i).is_some_and(|t| t.is_word(NOT_ALIAS)) {
                break;
            }
            i = next;
            // `from f(...)` is a function, `into t (a, b)` a column list.
            let call = toks.get(i).is_some_and(|t| t.tok == Tok::Open) && !into;
            if toks.get(i).is_some_and(|t| t.is_word(&["as"])) {
                i += 1;
            }
            let alias = match toks.get(i) {
                Some(t) if t.ident().is_some() && !t.is_word(NOT_ALIAS) => {
                    i += 1;
                    t.ident().map(String::from)
                }
                _ => None,
            };
            if !call {
                let mut parts = parts;
                let name = parts.pop().unwrap_or_default();
                out.push(TableRef {
                    schema: parts.pop(),
                    name,
                    alias,
                });
            }
            if from && toks.get(i).is_some_and(|t| t.tok == Tok::Comma) {
                i += 1;
                continue;
            }
            break;
        }
    }
    out
}

/// Index of the `)` closing the `(` at `open`.
fn close_of(toks: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0;
    for (k, t) in toks.iter().enumerate().skip(open) {
        match t.tok {
            Tok::Open => depth += 1,
            Tok::Close => {
                depth -= 1;
                if depth == 0 {
                    return Some(k);
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether `sql` has the unquoted word `word` outside strings and comments.
pub fn has_word(sql: &str, word: &str) -> bool {
    code(sql).iter().any(|t| t.is_word(&[word]))
}

/// `EXPLAIN ANALYZE <stmt>` runs `<stmt>`: its danger, if any.
fn explain_danger(stmt: &str, toks: &[Token]) -> Option<String> {
    let mut analyze = false;
    let mut i = 1;
    if toks.get(i).is_some_and(|t| t.tok == Tok::Open) {
        let close = close_of(toks, i)?;
        for k in i + 1..close {
            if toks[k].is_word(&["analyze", "analyse"]) {
                analyze = !toks.get(k + 1).is_some_and(|t| t.is_word(&["false", "off"]));
            }
        }
        i = close + 1;
    }
    while toks.get(i).is_some_and(|t| t.is_word(&["analyze", "analyse", "verbose"])) {
        analyze |= toks[i].is_word(&["analyze", "analyse"]);
        i += 1;
    }
    if analyze { danger(&stmt[toks.get(i)?.start..]) } else { None }
}

/// The danger of a CTE body of `WITH … AS (…)`, else of the main statement.
fn with_danger(stmt: &str, toks: &[Token]) -> Option<String> {
    let mut i = 1;
    if toks.get(i).is_some_and(|t| t.is_word(&["recursive"])) {
        i += 1;
    }
    loop {
        toks.get(i)?.ident()?;
        i += 1;
        if toks.get(i).is_some_and(|t| t.tok == Tok::Open) {
            i = close_of(toks, i)? + 1;
        }
        if !toks.get(i)?.is_word(&["as"]) {
            return None;
        }
        i += 1;
        while toks.get(i).is_some_and(|t| t.is_word(&["not", "materialized"])) {
            i += 1;
        }
        if toks.get(i)?.tok != Tok::Open {
            return None;
        }
        let close = close_of(toks, i)?;
        if let Some(d) = danger(&stmt[toks[i].end..toks[close].start]) {
            return Some(d);
        }
        i = close + 1;
        if toks.get(i).is_some_and(|t| t.tok == Tok::Comma) {
            i += 1;
            continue;
        }
        return danger(&stmt[toks.get(i)?.start..]);
    }
}

/// Why a statement deserves a second look before it runs: it changes every
/// row, or drops data for good. `None` for everything else.
pub fn danger(stmt: &str) -> Option<String> {
    let toks = code(stmt);
    match toks.first() {
        Some(t) if t.is_word(&["explain"]) => return explain_danger(stmt, &toks),
        Some(t) if t.is_word(&["with"]) => return with_danger(stmt, &toks),
        _ => {}
    }
    let words: Vec<&str> = toks.iter().filter_map(|t| t.ident().filter(|_| !matches!(t.tok, Tok::Ident { quoted: true, .. }))).collect();
    let target = || {
        tables_in(stmt)
            .first()
            .map(|r| r.schema.as_ref().map_or(display_ident(&r.name), |s| format!("{}.{}", display_ident(s), display_ident(&r.name))))
            .unwrap_or_else(|| "the table".into())
    };
    // `WHERE` of the statement itself, not of a subquery.
    let top_level_where = || {
        let mut depth = 0i32;
        toks.iter().any(|t| {
            match t.tok {
                Tok::Open => depth += 1,
                Tok::Close => depth -= 1,
                _ => {}
            }
            depth == 0 && t.is_word(&["where"])
        })
    };
    match words.first().copied()? {
        "update" if !top_level_where() => Some(format!("UPDATE without WHERE changes every row of {}", target())),
        "delete" if !top_level_where() => Some(format!("DELETE without WHERE deletes every row of {}", target())),
        "truncate" => Some(format!("TRUNCATE deletes every row of {}", target())),
        "drop" => {
            let what = words.get(1).map_or(String::new(), |w| format!(" {}", w.to_uppercase()));
            Some(format!("DROP{what} can't be undone"))
        }
        "alter" => {
            let k = words.iter().position(|w| *w == "drop")?;
            let keep = ["constraint", "default", "not", "identity", "expression", "owned", "policy", "trigger"];
            (!words.get(k + 1).is_some_and(|w| keep.contains(w)))
                .then(|| "ALTER … DROP COLUMN deletes the column with its data".to_string())
        }
        _ => None,
    }
}

/// What is being typed at a cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    /// Names before the last dot: `u.na|` → `["u"]`.
    pub qualifier: Vec<String>,
    /// Byte offset where the word being typed starts (the cursor when none).
    pub start: usize,
    /// The unquoted keyword right before the name, e.g. `from`.
    pub after: Option<String>,
}

/// The cursor at byte `offset` of `sql`; `None` inside a string or comment.
pub fn cursor(sql: &str, offset: usize) -> Option<Cursor> {
    let all = tokens(sql);
    if all.iter().any(|t| {
        t.start < offset
            && match t.tok {
                Tok::Str { closed } | Tok::Comment { closed } => offset < t.end || (offset == t.end && !closed),
                _ => false,
            }
    }) {
        return None;
    }
    let toks: Vec<Token> = all.into_iter().filter(|t| !matches!(t.tok, Tok::Comment { .. })).collect();
    // The word being typed is toks[j], or a new one goes at j.
    let (mut j, start) = match toks.iter().position(|t| t.start < offset && offset <= t.end && t.ident().is_some()) {
        Some(w) => (w, toks[w].start),
        None => (toks.iter().take_while(|t| t.end <= offset).count(), offset),
    };
    let mut qualifier = vec![];
    let mut next = start;
    while j >= 2
        && toks[j - 1].tok == Tok::Dot
        && toks[j - 1].end == next
        && let Some(name) = toks[j - 2].ident()
        && toks[j - 2].end == toks[j - 1].start
    {
        qualifier.insert(0, name.to_string());
        next = toks[j - 2].start;
        j -= 2;
    }
    let after = j
        .checked_sub(1)
        .and_then(|p| toks.get(p))
        .filter(|t| matches!(t.tok, Tok::Ident { quoted: false, .. }))
        .and_then(|t| t.ident().map(String::from));
    Some(Cursor { qualifier, start, after })
}

/// The dotted name under byte `offset`: its parts and which one the offset is on.
pub fn name_at(sql: &str, offset: usize) -> Option<(Vec<String>, usize)> {
    let toks = code(sql);
    let k = toks
        .iter()
        .position(|t| t.start <= offset && offset < t.end && t.ident().is_some())
        .or_else(|| toks.iter().position(|t| t.end == offset && t.ident().is_some()))?;
    let mut first = k;
    while first >= 2
        && toks[first - 1].tok == Tok::Dot
        && toks[first - 2].ident().is_some()
        && toks[first - 2].end == toks[first - 1].start
        && toks[first - 1].end == toks[first].start
    {
        first -= 2;
    }
    let mut parts = vec![];
    let mut j = first;
    loop {
        parts.push(toks[j].ident()?.to_string());
        let adjacent = toks.get(j + 1).is_some_and(|d| d.tok == Tok::Dot && d.start == toks[j].end)
            && toks.get(j + 2).is_some_and(|n| n.ident().is_some() && n.start == toks[j + 1].end);
        if !adjacent {
            break;
        }
        j += 2;
    }
    Some((parts, (k - first) / 2))
}

/// A relation of a connection, with its columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaTable {
    pub rel: Relation,
    pub columns: Vec<Column>,
}

impl SchemaTable {
    /// The name as written in SQL: without the default schema.
    pub fn short_name(&self) -> String {
        if self.rel.name.schema == DEFAULT_SCHEMA {
            display_ident(&self.rel.name.name)
        } else {
            self.rel.name.to_string()
        }
    }
}

/// The relations of a connection, for `.sql` files.
#[derive(Debug, Clone, Default)]
pub struct Schema {
    pub tables: Vec<SchemaTable>,
}

/// Something a completion can insert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Candidate<'a> {
    Keyword(&'static str),
    Schema(&'a str),
    /// A table, and whether to write it schema-qualified.
    Table(&'a SchemaTable, bool),
    Column(&'a SchemaTable, &'a Column),
}

/// What a name in a statement refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target<'a> {
    Table(&'a SchemaTable),
    Column(&'a SchemaTable, &'a Column),
}

pub const KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "AND", "OR", "NOT", "NULL", "IS", "IN", "LIKE", "ILIKE", "BETWEEN", "EXISTS", "JOIN",
    "LEFT JOIN", "INNER JOIN", "ON", "USING", "GROUP BY", "ORDER BY", "HAVING", "LIMIT", "OFFSET", "DISTINCT", "AS",
    "CASE", "WHEN", "THEN", "ELSE", "END", "UNION ALL", "ASC", "DESC", "WITH", "INSERT INTO", "VALUES", "UPDATE",
    "SET", "DELETE FROM", "RETURNING", "DEFAULT", "TRUE", "FALSE", "COUNT", "EXPLAIN", "ANALYZE",
];

/// After these only a table name fits.
const TABLE_CONTEXT: &[&str] = &["from", "join", "update", "into", "table"];

impl Schema {
    fn schemas(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self.tables.iter().map(|t| t.rel.name.schema.as_str()).collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// `name` looks in the default schema first, then in any schema.
    pub fn table(&self, schema: Option<&str>, name: &str) -> Option<&SchemaTable> {
        let named = |s: &str| self.tables.iter().find(|t| t.rel.name.schema == s && t.rel.name.name == name);
        match schema {
            Some(s) => named(s),
            None => named(DEFAULT_SCHEMA).or_else(|| self.tables.iter().find(|t| t.rel.name.name == name)),
        }
    }

    /// The table `q` stands for in a statement naming `refs`: an alias, a table name, or a known table.
    fn qualifier_table(&self, refs: &[TableRef], q: &str) -> Option<&SchemaTable> {
        let r = refs
            .iter()
            .find(|r| r.alias.as_deref() == Some(q))
            .or_else(|| refs.iter().find(|r| r.name == q));
        match r {
            Some(r) => self.table(r.schema.as_deref(), &r.name),
            None => self.table(None, q),
        }
    }

    fn statement_tables(&self, refs: &[TableRef]) -> Vec<&SchemaTable> {
        let mut out: Vec<&SchemaTable> = vec![];
        for r in refs {
            if let Some(t) = self.table(r.schema.as_deref(), &r.name)
                && !out.iter().any(|o| std::ptr::eq(*o, t))
            {
                out.push(t);
            }
        }
        out
    }

    /// Completions at byte `offset` of statement `sql`, and where the replaced word starts.
    pub fn complete(&self, sql: &str, offset: usize) -> Option<(usize, Vec<Candidate<'_>>)> {
        let c = cursor(sql, offset)?;
        let refs = tables_in(sql);
        let mut out = vec![];
        match c.qualifier.as_slice() {
            [] => {
                let table_only = c.after.as_deref().is_some_and(|w| TABLE_CONTEXT.contains(&w));
                if !table_only {
                    for t in self.statement_tables(&refs) {
                        out.extend(t.columns.iter().map(|col| Candidate::Column(t, col)));
                    }
                }
                out.extend(self.tables.iter().map(|t| Candidate::Table(t, t.rel.name.schema != DEFAULT_SCHEMA)));
                out.extend(self.schemas().into_iter().filter(|s| *s != DEFAULT_SCHEMA).map(Candidate::Schema));
                if !table_only {
                    out.extend(KEYWORDS.iter().map(|k| Candidate::Keyword(k)));
                }
            }
            [q] => {
                let is_alias = refs.iter().any(|r| r.alias.as_deref() == Some(q.as_str()));
                if !is_alias && self.schemas().contains(&q.as_str()) {
                    out.extend(self.tables.iter().filter(|t| &t.rel.name.schema == q).map(|t| Candidate::Table(t, false)));
                }
                if let Some(t) = self.qualifier_table(&refs, q) {
                    out.extend(t.columns.iter().map(|col| Candidate::Column(t, col)));
                }
            }
            [s, n] => {
                if let Some(t) = self.table(Some(s), n) {
                    out.extend(t.columns.iter().map(|col| Candidate::Column(t, col)));
                }
            }
            _ => {}
        }
        Some((c.start, out))
    }

    /// What the name under byte `offset` of statement `sql` refers to.
    pub fn resolve(&self, sql: &str, offset: usize) -> Option<Target<'_>> {
        let (parts, at) = name_at(sql, offset)?;
        let refs = tables_in(sql);
        // An alias or an unqualified table name of the statement.
        let in_refs = |q: &str| refs.iter().any(|r| r.alias.as_deref() == Some(q) || (r.name == q && r.schema.is_none()));
        match (parts.as_slice(), at) {
            ([n], 0) if in_refs(n) => self.qualifier_table(&refs, n).map(Target::Table),
            ([n], 0) => self
                .statement_tables(&refs)
                .into_iter()
                .find_map(|t| column_of(t, n))
                .or_else(|| self.table(None, n).map(Target::Table)),
            ([q, n], 1) if in_refs(q) => self.qualifier_table(&refs, q).and_then(|t| column_of(t, n)),
            ([q, n], 1) => self
                .table(Some(q), n)
                .map(Target::Table)
                .or_else(|| self.table(None, q).and_then(|t| column_of(t, n))),
            ([q, _], 0) if in_refs(q) || !self.schemas().contains(&q.as_str()) => {
                self.qualifier_table(&refs, q).map(Target::Table)
            }
            ([s, t, n], 2) => self.table(Some(s), t).and_then(|t| column_of(t, n)),
            ([s, t, _], 1) => self.table(Some(s), t).map(Target::Table),
            _ => None,
        }
    }
}

fn column_of<'s>(t: &'s SchemaTable, name: &str) -> Option<Target<'s>> {
    t.columns.iter().find(|c| c.name == name).map(|c| Target::Column(t, c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::QualName;

    fn rel(schema: &str, name: &str) -> Relation {
        Relation {
            name: QualName::new(schema, name),
            kind: "table".into(),
            rows: None,
        }
    }

    fn col(name: &str) -> Column {
        Column {
            name: name.into(),
            type_name: "text".into(),
            ..Default::default()
        }
    }

    fn schema() -> Schema {
        Schema {
            tables: vec![
                SchemaTable {
                    rel: rel("public", "users"),
                    columns: vec![col("id"), col("name")],
                },
                SchemaTable {
                    rel: rel("public", "orders"),
                    columns: vec![col("id"), col("user_id")],
                },
                SchemaTable {
                    rel: rel("audit", "log"),
                    columns: vec![col("at")],
                },
            ],
        }
    }

    fn labels(c: &[Candidate]) -> Vec<String> {
        c.iter()
            .map(|c| match c {
                Candidate::Keyword(k) => format!("kw:{k}"),
                Candidate::Schema(s) => format!("schema:{s}"),
                Candidate::Table(t, _) => format!("table:{}", t.rel.name),
                Candidate::Column(t, c) => format!("{}.{}", t.rel.name.name, c.name),
            })
            .collect()
    }

    /// Completions where `|` is in `sql`.
    fn complete(sql: &str) -> Vec<String> {
        let at = sql.find('|').unwrap();
        let text = sql.replace('|', "");
        schema().complete(&text, at).map(|(_, c)| labels(&c)).unwrap_or_default()
    }

    #[test]
    fn finds_tables_and_aliases() {
        let refs = tables_in(
            "select * from users u, orders join audit.\"Log\" as l on true where x in (select 1 from generate_series(1, 2) g)",
        );
        let names: Vec<_> = refs.iter().map(|r| (r.schema.as_deref(), r.name.as_str(), r.alias.as_deref())).collect();
        assert_eq!(names, vec![(None, "users", Some("u")), (None, "orders", None), (Some("audit"), "Log", Some("l"))]);
        let refs = tables_in("insert into Users (id) values (1); update orders set x = 1");
        assert_eq!(refs.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), vec!["users", "orders"]);
        let refs = tables_in("select * from users where id = 1");
        assert_eq!(refs[0].alias, None);
    }

    #[test]
    fn dangerous_statements() {
        assert_eq!(danger("update users set a = 1").as_deref(), Some("UPDATE without WHERE changes every row of users"));
        assert_eq!(danger("update users set a = 1 where id = 2"), None);
        assert!(danger("update t set a = (select b from c where c.id = t.id)").is_some());
        assert!(danger("DELETE FROM audit.log").unwrap().ends_with("of audit.log"));
        assert_eq!(danger("delete from t using u where t.id = u.id"), None);
        assert_eq!(danger("-- note\ntruncate t").as_deref(), Some("TRUNCATE deletes every row of t"));
        assert_eq!(danger("truncate table only t").as_deref(), Some("TRUNCATE deletes every row of t"));
        assert_eq!(danger("drop table t").as_deref(), Some("DROP TABLE can't be undone"));
        assert!(danger("alter table t drop column c").is_some());
        assert!(danger("alter table t drop c").is_some());
        assert_eq!(danger("alter table t drop constraint c"), None);
        assert_eq!(danger("select * from t"), None);
        assert_eq!(danger("insert into t values (1)"), None);
    }

    #[test]
    fn dangerous_inside_explain_and_with() {
        let all = Some("DELETE without WHERE deletes every row of t".to_string());
        assert_eq!(danger("explain analyze delete from t"), all);
        assert_eq!(danger("EXPLAIN ANALYZE VERBOSE delete from t"), all);
        assert_eq!(danger("explain (analyze, buffers) delete from t"), all);
        assert_eq!(danger("explain (analyze false) delete from t"), None);
        assert_eq!(danger("explain delete from t"), None);
        assert_eq!(danger("explain analyze select * from t"), None);
        assert_eq!(danger("with x as (select 1) delete from t"), all);
        assert_eq!(danger("with recursive x (n) as materialized (select 1), y as (select 2) delete from t"), all);
        assert_eq!(danger("with d as (delete from t returning *) select * from d"), all);
        assert_eq!(danger("with x as (select 1) delete from t where id in (select * from x)"), None);
        assert_eq!(danger("with x as (select 1) select * from x"), None);
        assert!(has_word("a = 1 OR(b = 2)", "or"));
        assert!(!has_word("a = 'x or y' and \"or\" = 1", "or"));
    }

    #[test]
    fn cursor_context() {
        let c = |sql: &str| {
            let at = sql.find('|').unwrap();
            cursor(&sql.replace('|', ""), at)
        };
        assert_eq!(c("select u.na| from users u").unwrap().qualifier, vec!["u"]);
        assert_eq!(c("select u.| from users u").unwrap().start, 9);
        assert_eq!(c("select a.b.| from x").unwrap().qualifier, vec!["a", "b"]);
        assert_eq!(c("select * from us|").unwrap().after.as_deref(), Some("from"));
        assert_eq!(c("select * from |").unwrap().after.as_deref(), Some("from"));
        assert_eq!(c("select u |").unwrap().qualifier, Vec::<String>::new());
        assert!(c("select 'u.|'").is_none());
        assert!(c("select 1 -- u.|").is_none());
        assert!(c("select 'abc' |").is_some());
    }

    #[test]
    fn completes_by_context() {
        let all = complete("select | from users u");
        assert!(all.contains(&"users.name".to_string()));
        assert!(!all.contains(&"orders.user_id".to_string()));
        assert!(all.contains(&"table:public.orders".to_string()));
        assert!(all.contains(&"schema:audit".to_string()));
        assert!(all.contains(&"kw:SELECT".to_string()));

        assert_eq!(complete("select u.| from users u"), vec!["users.id", "users.name"]);
        assert_eq!(complete("select users.| from users"), vec!["users.id", "users.name"]);
        assert_eq!(complete("select * from audit.|"), vec!["table:audit.log"]);
        assert_eq!(complete("select audit.log.| from audit.log"), vec!["log.at"]);
        let from = complete("select * from |");
        assert!(from.iter().all(|l| l.starts_with("table:") || l.starts_with("schema:")));
        assert!(complete("select x.| from users").is_empty());
    }

    #[test]
    fn resolves_names() {
        let s = schema();
        let at = |sql: &str, word: &str| s.resolve(sql, sql.find(word).unwrap() + 1);
        let sql = "select u.name, id from users u join audit.log l on true";
        assert!(matches!(at(sql, "name"), Some(Target::Column(t, c)) if t.rel.name.name == "users" && c.name == "name"));
        assert!(matches!(at(sql, "users"), Some(Target::Table(t)) if t.rel.name.name == "users"));
        assert!(matches!(at(sql, "log"), Some(Target::Table(t)) if t.rel.name.schema == "audit"));
        assert!(matches!(at(sql, "id"), Some(Target::Column(t, _)) if t.rel.name.name == "users"));
        assert_eq!(at(sql, "audit"), None);
        assert!(matches!(at("select * from orders", "orders"), Some(Target::Table(_))));
    }
}
