//! Splitting a `.sql` buffer into statements, and the `-- @conn name`
//! directives that pick the connection for the statements below them.

use std::ops::Range;

/// Byte ranges of the statements in `text`, without the `;` and without
/// leading/trailing whitespace and comments. Semicolons inside strings,
/// quoted identifiers, dollar-quoted bodies and comments don't split. A
/// line starting with `\` is a psql meta-command (`\restrict` in pg_dump
/// output, `\set`…): it ends the statement before it and is left out.
pub fn split(text: &str) -> Vec<Range<usize>> {
    let b = text.as_bytes();
    let mut out = vec![];
    let mut start = 0;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\'' | b'"' => {
                let q = b[i];
                // E'..' strings use backslash escapes.
                let backslash = q == b'\'' && i > 0 && matches!(b[i - 1], b'E' | b'e') && !is_ident(b, i - 1);
                i += 1;
                while i < b.len() {
                    if backslash && b[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if b[i] == q {
                        if b.get(i + 1) == Some(&q) {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                i += 1;
            }
            b'-' if b.get(i + 1) == Some(&b'-') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let mut depth = 0;
                while i < b.len() {
                    if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                        depth += 1;
                        i += 2;
                    } else if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
            }
            b'$' if !is_ident(b, i) => match dollar_tag(&text[i..]) {
                Some(tag) => {
                    let body = i + tag.len();
                    i = text[body..].find(tag).map_or(b.len(), |p| body + p + tag.len());
                }
                None => i += 1,
            },
            b'\\' if text[text[..i].rfind('\n').map_or(0, |p| p + 1)..i].trim().is_empty() => {
                push(text, start..i, &mut out);
                i = text[i..].find('\n').map_or(b.len(), |p| i + p + 1);
                start = i;
            }
            b';' => {
                push(text, start..i, &mut out);
                i += 1;
                start = i;
                // pg_dump data after `COPY … FROM stdin;` runs up to a `\.` line.
                if out.last().is_some_and(|r| is_copy_from_stdin(&text[r.clone()])) {
                    i = text[i..].find("\n\\.").map_or(b.len(), |p| {
                        let after = i + p + 3;
                        text[after..].find('\n').map_or(b.len(), |q| after + q + 1)
                    });
                    start = i;
                }
            }
            _ => i += 1,
        }
    }
    push(text, start..b.len(), &mut out);
    out
}

/// The statement the cursor at `offset` is in, including text typed after
/// its last token; an empty range between statements.
pub fn around(text: &str, offset: usize) -> Range<usize> {
    match split(text).into_iter().rev().find(|r| r.start <= offset) {
        Some(r) if offset <= r.end => r,
        Some(r) if !text[r.end..offset].contains(';') => r.start..offset,
        _ => offset..offset,
    }
}

fn is_copy_from_stdin(stmt: &str) -> bool {
    let s = stmt.to_ascii_lowercase();
    s.starts_with("copy") && s.split_whitespace().collect::<Vec<_>>().windows(2).any(|w| w == ["from", "stdin"])
}

/// Whether the byte before `i` continues an identifier (so `$` or `E` at `i` is part of it).
pub(crate) fn is_ident(b: &[u8], i: usize) -> bool {
    i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_' || b[i - 1] >= 0x80)
}

/// `$tag$` or `$$` at the start of `s`.
pub(crate) fn dollar_tag(s: &str) -> Option<&str> {
    let rest = &s[1..];
    let end = rest.find('$')?;
    let tag = &rest[..end];
    let valid = tag.chars().enumerate().all(|(k, c)| c == '_' || c.is_alphabetic() || (k > 0 && c.is_ascii_digit()));
    valid.then(|| &s[..end + 2])
}

/// Adds the statement in `r` with surrounding blanks and comments trimmed, if
/// anything is left.
fn push(text: &str, r: Range<usize>, out: &mut Vec<Range<usize>>) {
    let s = &text[r.clone()];
    let mut a = 0;
    loop {
        let t = &s[a..];
        let trimmed = t.trim_start();
        a += t.len() - trimmed.len();
        if trimmed.starts_with("--") {
            a += trimmed.find('\n').unwrap_or(trimmed.len());
        } else if trimmed.starts_with("/*") {
            match trimmed.find("*/") {
                Some(p) => a += p + 2,
                None => return,
            }
        } else {
            break;
        }
    }
    let body = s[a..].trim_end();
    if !body.is_empty() {
        out.push(r.start + a..r.start + a + body.len());
    }
}

/// The statement on one line, for a header field: line breaks become spaces
/// and `--` comments become `/* */`. `None` when a string literal or quoted
/// identifier itself spans lines.
pub fn one_line(sql: &str) -> Option<String> {
    let b = sql.as_bytes();
    let mut out = String::with_capacity(sql.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            q @ (b'\'' | b'"') => {
                let start = i;
                i += 1;
                while i < b.len() && !(b[i] == q && b.get(i + 1) != Some(&q)) {
                    i += if b[i] == q { 2 } else { 1 };
                }
                i += 1;
                let lit = &sql[start..i.min(b.len())];
                if lit.contains('\n') {
                    return None;
                }
                out.push_str(lit);
            }
            b'$' => match dollar_tag(&sql[i..]).filter(|_| !is_ident(b, i)) {
                Some(tag) => {
                    let body = i + tag.len();
                    let end = sql[body..].find(tag).map_or(b.len(), |p| body + p + tag.len());
                    let lit = &sql[i..end];
                    if lit.contains('\n') {
                        return None;
                    }
                    out.push_str(lit);
                    i = end;
                }
                None => {
                    out.push('$');
                    i += 1;
                }
            },
            b'-' if b.get(i + 1) == Some(&b'-') => {
                let end = sql[i..].find('\n').map_or(b.len(), |p| i + p);
                out.push_str(&format!("/* {} */", sql[i + 2..end].trim().replace("*/", "* /")));
                i = end;
            }
            b'\n' | b'\r' | b'\t' | b' ' => {
                if !out.ends_with(' ') {
                    out.push(' ');
                }
                i += 1;
            }
            _ => {
                let ch = sql[i..].chars().next().unwrap_or(' ');
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    Some(out.trim().to_string())
}

/// Connection named by the last `-- @conn name` line before byte `offset`.
pub fn conn_before(text: &str, offset: usize) -> Option<String> {
    text[..offset.min(text.len())]
        .lines()
        .rev()
        .filter_map(|l| l.trim().strip_prefix("--")?.trim().strip_prefix("@conn"))
        .find_map(|rest| rest.split_whitespace().next().map(String::from))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(text: &str) -> Vec<&str> {
        split(text).into_iter().map(|r| &text[r]).collect()
    }

    #[test]
    fn splits_on_top_level_semicolons() {
        let text = "-- @conn local\nselect 1;\n\n/* c; */ select 'a;b', \"x;y\" from t;\nselect $$a;b$$, $f$ ; $f$;  \n-- trailing; comment\n";
        assert_eq!(
            parts(text),
            vec!["select 1", "select 'a;b', \"x;y\" from t", "select $$a;b$$, $f$ ; $f$"]
        );
        assert_eq!(parts("select E'it\\'s;' ; select 'it''s;'"), vec!["select E'it\\'s;'", "select 'it''s;'"]);
        assert_eq!(parts("do $body$ begin perform 1; end $body$"), vec!["do $body$ begin perform 1; end $body$"]);
        assert_eq!(parts("select a$1 from t; select 2"), vec!["select a$1 from t", "select 2"]);
        assert!(parts("  -- only a comment\n").is_empty());
        assert_eq!(
            parts("\\restrict abc\nset x = 1;\n  \\set ON_ERROR_STOP on\nselect 'a\n\\b';\n\\unrestrict abc\n"),
            vec!["set x = 1", "select 'a\n\\b'"]
        );
        assert_eq!(
            parts("COPY t (a) FROM stdin;\n1\tit's; odd\n\\.\n\nselect 1;"),
            vec!["COPY t (a) FROM stdin", "select 1"]
        );
    }

    #[test]
    fn statement_around_cursor() {
        let text = "select 1;\nselect * from t \n;  ";
        let at = |s: &str| text.find(s).unwrap();
        assert_eq!(&text[around(text, at("1"))], "select 1");
        assert_eq!(&text[around(text, at("from") + 7)], "select * from t ");
        assert_eq!(around(text, text.len()), text.len()..text.len());
    }

    #[test]
    fn one_line_keeps_meaning() {
        assert_eq!(
            one_line("select a -- the a\nfrom t\n  where b = 'x;y'").as_deref(),
            Some("select a /* the a */ from t where b = 'x;y'")
        );
        assert_eq!(one_line("select 'a\nb'"), None);
        assert_eq!(one_line("select $$x$$").as_deref(), Some("select $$x$$"));
    }

    #[test]
    fn conn_directives() {
        let text = "-- @conn a\nselect 1;\n--@conn b\nselect 2;";
        assert_eq!(conn_before(text, text.find("select 1").unwrap()), Some("a".into()));
        assert_eq!(conn_before(text, text.find("select 2").unwrap()), Some("b".into()));
        assert_eq!(conn_before("select 1", 0), None);
    }
}
