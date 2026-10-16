//! Cell text <-> value. Values are Postgres text representations; `NULL`,
//! `DEFAULT` and `+` are keywords, so the same literal strings are written
//! with a leading backslash.

use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Null,
    Default,
    /// `+` in a key column: a row to insert.
    Plus,
    Text(String),
}

impl Value {
    pub fn from_db(v: Option<&str>) -> Self {
        v.map_or(Value::Null, |s| Value::Text(s.to_string()))
    }
}

pub const ELLIPSIS: char = '…';

pub fn escape(s: &str) -> String {
    if matches!(s, "NULL" | "DEFAULT" | "+") {
        return format!("\\{s}");
    }
    let lead = s.len() - s.trim_start_matches(' ').len();
    let trail = if lead == s.len() { 0 } else { s.len() - s.trim_end_matches(' ').len() };
    let mut out = String::with_capacity(s.len());
    for (i, c) in s.char_indices() {
        match c {
            ' ' if i < lead || i >= s.len() - trail => out.push_str("\\s"),
            '\\' => out.push_str("\\\\"),
            '|' => out.push_str("\\|"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

pub fn render(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::Default => "DEFAULT".into(),
        Value::Plus => "+".into(),
        Value::Text(s) => escape(s),
    }
}

/// Parses trimmed cell text.
pub fn parse(s: &str) -> Result<Value, String> {
    match s {
        "NULL" => return Ok(Value::Null),
        "DEFAULT" => return Ok(Value::Default),
        "+" => return Ok(Value::Plus),
        _ => {}
    }
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('\\') => out.push('\\'),
            Some('|') => out.push('|'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('s') => out.push(' '),
            Some(c @ ('N' | 'D' | '+')) => out.push(c),
            Some('u') => {
                let rest: String = it.by_ref().take_while(|&c| c != '}').collect();
                let code = rest
                    .strip_prefix('{')
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .and_then(char::from_u32)
                    .ok_or_else(|| format!("bad escape `\\u{rest}}}`, expected `\\u{{hex}}`"))?;
                out.push(code);
            }
            Some(c) => return Err(format!("unknown escape `\\{c}`")),
            None => return Err("trailing `\\`".into()),
        }
    }
    Ok(Value::Text(out))
}

pub fn display_width(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// Cell text for a value, cut to `width` display columns (0 = no limit).
/// Returns the text and whether it was cut.
pub fn render_cut(v: &Value, width: u32) -> (String, bool) {
    let full = render(v);
    let width = width as usize;
    if width == 0 || display_width(&full) <= width {
        return (full, false);
    }
    let mut out = String::new();
    let mut w = 0;
    for c in full.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw + 1 > width {
            break;
        }
        out.push(c);
        w += cw;
    }
    // Never end on a dangling backslash: it would escape the ellipsis.
    let mut slashes = out.chars().rev().take_while(|&c| c == '\\').count();
    while slashes % 2 == 1 {
        out.pop();
        slashes -= 1;
    }
    out.push(ELLIPSIS);
    (out, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for s in ["", "a b", " lead", "trail ", "  ", "a|b\\c\nd\te\r", "NULL", "DEFAULT", "+", "\u{1b}x", "кот"] {
            let v = Value::Text(s.into());
            assert_eq!(parse(&render(&v)).unwrap(), v, "{s:?}");
        }
        assert_eq!(parse("NULL").unwrap(), Value::Null);
        assert_eq!(parse("DEFAULT").unwrap(), Value::Default);
        assert_eq!(parse("+").unwrap(), Value::Plus);
    }

    #[test]
    fn cut() {
        assert_eq!(render_cut(&Value::Text("abcdef".into()), 4), ("abc…".into(), true));
        assert_eq!(render_cut(&Value::Text("abcd".into()), 4), ("abcd".into(), false));
        assert_eq!(render_cut(&Value::Text("ab\\cd".into()), 4), ("ab…".into(), true));
        assert_eq!(render_cut(&Value::Text("日本語".into()), 5), ("日本…".into(), true));
    }
}
