//! What hover shows about a value beyond its cell text, by column type.

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde_json::Value;

/// Hover limit for a full value; the row view shows any size.
const MAX_HOVER_VALUE: usize = 20_000;

/// Markdown for `v` of `type_name` (a `format_type()` name like `integer[]`
/// or a wire name like `_int4`), starting with a blank line, or empty.
/// `cut`: the cell doesn't show the whole value.
pub fn value_md(v: &str, type_name: &str, cut: bool) -> String {
    let t = type_name.to_ascii_lowercase();
    let special = if t.ends_with("[]") || (t.starts_with('_') && v.starts_with(['{', '['])) {
        array_md(v)
            .filter(|list| cut || list.lines().count() > 1)
            .map(|list| block("", &list, ""))
    } else if t == "bytea" {
        bytea_md(v)
    } else if t == "xml" {
        (cut || v.len() > 60).then(|| block("xml", &pretty_xml(v), ""))
    } else if t.starts_with("timestamp") || t == "date" {
        time_md(v, &t).map(|md| format!("\n\n{md}"))
    } else if matches!(t.as_str(), "integer" | "bigint" | "int4" | "int8" | "numeric") {
        epoch_md(v).map(|md| format!("\n\n{md}"))
    } else {
        None
    };
    if let Some(md) = special {
        return md;
    }
    if !cut {
        return String::new();
    }
    let looks_json = matches!(t.as_str(), "json" | "jsonb") || v.trim_start().starts_with(['{', '[']);
    match serde_json::from_str::<Value>(v) {
        Ok(j) if looks_json => block("json", &serde_json::to_string_pretty(&j).unwrap_or_else(|_| v.to_string()), ""),
        _ => block("", v, ""),
    }
}

/// A fenced block, cut to `MAX_HOVER_VALUE`, followed by `after`.
fn block(lang: &str, body: &str, after: &str) -> String {
    let mut body = body.to_string();
    let mut note = String::new();
    if body.len() > MAX_HOVER_VALUE {
        let mut cut = MAX_HOVER_VALUE;
        while !body.is_char_boundary(cut) {
            cut -= 1;
        }
        body.truncate(cut);
        note = "\n\n… cut here; F12 on the key opens the whole row".into();
    }
    // A fence longer than any backtick run inside the value.
    let longest = body.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!("\n\n{fence}{lang}\n{body}\n{fence}{note}{after}")
}

/// `{a,"b c",{1,2}}` -> `[1] a`, `[2] b c`, `[3][1] 1`… one per line.
fn array_md(v: &str) -> Option<String> {
    // `[0:2]={…}` carries explicit bounds; the elements are what matter.
    let v = v.find("={").map_or(v, |i| &v[i + 1..]);
    let chars: Vec<char> = v.chars().collect();
    let mut out = vec![];
    let mut path: Vec<usize> = vec![];
    let mut i = 0;
    let mut expect_elem = false;
    while i < chars.len() {
        match chars[i] {
            '{' => {
                if let Some(last) = path.last_mut() {
                    *last += 1;
                }
                path.push(0);
                expect_elem = true;
                i += 1;
            }
            '}' => {
                path.pop();
                expect_elem = false;
                i += 1;
            }
            ',' => {
                expect_elem = true;
                i += 1;
            }
            c if c.is_whitespace() => i += 1,
            _ if !expect_elem => return None,
            _ => {
                let mut text = String::new();
                if chars[i] == '"' {
                    i += 1;
                    while i < chars.len() && chars[i] != '"' {
                        if chars[i] == '\\' {
                            i += 1;
                        }
                        if let Some(&c) = chars.get(i) {
                            text.push(c);
                        }
                        i += 1;
                    }
                    i += 1;
                } else {
                    while i < chars.len() && !matches!(chars[i], ',' | '}') {
                        text.push(chars[i]);
                        i += 1;
                    }
                    text = text.trim().to_string();
                }
                let last = path.last_mut()?;
                *last += 1;
                let idx: String = path.iter().map(|n| format!("[{n}]")).collect();
                out.push(format!("{idx} {text}"));
                expect_elem = false;
            }
        }
    }
    (!out.is_empty()).then(|| out.join("\n"))
}

/// Size, then text when it's UTF-8, else a hex dump of the start.
fn bytea_md(v: &str) -> Option<String> {
    let hex = v.strip_prefix("\\x")?;
    let bytes: Vec<u8> = (0..hex.len() / 2)
        .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16))
        .collect::<Result<_, _>>()
        .ok()?;
    let size = format!("\n\n{} bytes", bytes.len());
    if let Ok(s) = std::str::from_utf8(&bytes)
        && !s.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Some(format!("{size} of UTF-8 text{}", block("", s, "")));
    }
    let mut dump = String::new();
    for (row, chunk) in bytes.chunks(16).take(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        let ascii: String = chunk
            .iter()
            .map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' })
            .collect();
        dump += &format!("{:08x}  {:<47}  {ascii}\n", row * 16, hex.join(" "));
    }
    let more = if bytes.len() > 256 { "\n\nfirst 256 bytes" } else { "" };
    Some(format!("{size}{}", block("", dump.trim_end(), more)))
}

/// Tags on their own lines, indented by nesting; `<a>text</a>` and `<a></a>`
/// stay on one line.
fn pretty_xml(v: &str) -> String {
    #[derive(Clone, Copy, PartialEq)]
    enum Tok {
        Open,
        Close,
        Text,
        Other,
    }
    let mut out = String::new();
    let mut depth: usize = 0;
    let mut prev = [Tok::Other, Tok::Other];
    let mut rest = v.trim();
    while !rest.is_empty() {
        let end = if rest.starts_with('<') {
            rest.find('>').map_or(rest.len(), |e| e + 1)
        } else {
            rest.find('<').unwrap_or(rest.len())
        };
        let (piece, tail) = rest.split_at(end);
        rest = tail.trim_start();
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        let tok = if !piece.starts_with('<') {
            Tok::Text
        } else if piece.starts_with("</") {
            Tok::Close
        } else if piece.ends_with("/>") || piece.starts_with("<?") || piece.starts_with("<!") {
            Tok::Other
        } else {
            Tok::Open
        };
        if tok == Tok::Close {
            depth = depth.saturating_sub(1);
        }
        let inline = match tok {
            Tok::Text => prev[1] == Tok::Open,
            Tok::Close => prev[1] == Tok::Open || (prev[1] == Tok::Text && prev[0] == Tok::Open),
            _ => false,
        };
        if !inline && !out.is_empty() {
            out.push('\n');
            out.push_str(&"  ".repeat(depth));
        }
        out.push_str(piece);
        if tok == Tok::Open {
            depth += 1;
        }
        prev = [prev[1], tok];
    }
    out
}

fn ago(secs: i64) -> String {
    let (n, unit) = match secs.unsigned_abs() {
        s if s < 60 => return "just now".into(),
        s if s < 3600 => (s / 60, "minute"),
        s if s < 86_400 => (s / 3600, "hour"),
        s if s < 86_400 * 31 => (s / 86_400, "day"),
        s if s < 86_400 * 365 => (s / (86_400 * 30), "month"),
        s => (s / (86_400 * 365), "year"),
    };
    let unit = if n == 1 { unit.to_string() } else { format!("{unit}s") };
    if secs >= 0 { format!("{n} {unit} ago") } else { format!("in {n} {unit}") }
}

const LOCAL_FMT: &str = "%Y-%m-%d %H:%M:%S";

fn time_md(v: &str, t: &str) -> Option<String> {
    let now = Utc::now();
    if t == "date" {
        let d = NaiveDate::parse_from_str(v.trim(), "%Y-%m-%d").ok()?;
        let days = (Local::now().date_naive() - d).num_days();
        let rel = match days {
            0 => "today".to_string(),
            1 => "yesterday".to_string(),
            -1 => "tomorrow".to_string(),
            _ => ago(days * 86_400),
        };
        return Some(format!("{} · {rel}", d.format("%A")));
    }
    if t.contains("with time zone") || t == "timestamptz" {
        let dt = parse_tz(v.trim())?;
        let local = dt.with_timezone(&Local);
        return Some(format!(
            "{} · {} · UTC {} · local {}",
            local.format("%A"),
            ago((now - dt.with_timezone(&Utc)).num_seconds()),
            dt.with_timezone(&Utc).format(LOCAL_FMT),
            local.format(LOCAL_FMT),
        ));
    }
    let naive = NaiveDateTime::parse_from_str(v.trim(), "%Y-%m-%d %H:%M:%S%.f").ok()?;
    let local = Local.from_local_datetime(&naive).single()?;
    Some(format!(
        "{} · {} if it's in your time zone, {} if UTC",
        naive.format("%A"),
        ago((now - local.with_timezone(&Utc)).num_seconds()),
        ago((now - naive.and_utc()).num_seconds()),
    ))
}

/// `2026-09-18 14:01:48+05`, `…:48.123+05:30`.
fn parse_tz(v: &str) -> Option<DateTime<chrono::FixedOffset>> {
    let sign = v.rfind(['+', '-']).filter(|&p| p > 10)?;
    let (base, off) = v.split_at(sign);
    let off = if off.len() == 3 { format!("{off}:00") } else { off.to_string() };
    DateTime::parse_from_str(&format!("{base}{off}"), "%Y-%m-%d %H:%M:%S%.f%:z").ok()
}

/// Integers that read as Unix time in seconds (10 digits) or milliseconds
/// (13 digits) between 2001 and 2100.
fn epoch_md(v: &str) -> Option<String> {
    let n: i64 = v.trim().parse().ok()?;
    let secs = match v.trim().len() {
        10 if (1_000_000_000..4_102_444_800).contains(&n) => n,
        13 if (1_000_000_000_000..4_102_444_800_000).contains(&n) => n / 1000,
        _ => return None,
    };
    let dt = DateTime::<Utc>::from_timestamp(secs, 0)?;
    Some(format!(
        "as Unix time: UTC {} · local {} · {}",
        dt.format(LOCAL_FMT),
        dt.with_timezone(&Local).format(LOCAL_FMT),
        ago((Utc::now() - dt).num_seconds()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrays() {
        assert_eq!(array_md("{1,2,3}").unwrap(), "[1] 1\n[2] 2\n[3] 3");
        assert_eq!(array_md(r#"{"a b",NULL,"x\"y"}"#).unwrap(), "[1] a b\n[2] NULL\n[3] x\"y");
        assert_eq!(array_md("{{1,2},{3}}").unwrap(), "[1][1] 1\n[1][2] 2\n[2][1] 3");
        assert_eq!(array_md("[0:1]={7,8}").unwrap(), "[1] 7\n[2] 8");
        assert!(value_md("{1}", "integer[]", false).is_empty());
    }

    #[test]
    fn bytea() {
        assert!(bytea_md("\\x68656c6c6f").unwrap().contains("5 bytes of UTF-8 text"));
        let dump = bytea_md("\\x00ff41").unwrap();
        assert!(dump.contains("00000000  00 ff 41") && dump.contains("..A"), "{dump}");
    }

    #[test]
    fn xml() {
        assert_eq!(
            pretty_xml("<a><b x=\"1\">text</b><c/></a>"),
            "<a>\n  <b x=\"1\">text</b>\n  <c/>\n</a>"
        );
    }

    #[test]
    fn times() {
        assert!(parse_tz("2026-09-18 14:01:48+05").is_some());
        assert!(parse_tz("2026-09-18 14:01:48.123456-03:30").is_some());
        assert!(time_md("2026-09-18 14:01:48", "timestamp(0) without time zone").unwrap().contains("if UTC"));
        assert!(time_md("2026-09-18", "date").unwrap().starts_with("Friday"));
        assert!(epoch_md("1791983764").unwrap().starts_with("as Unix time: UTC 2026-10-14"));
        assert!(epoch_md("12345").is_none());
        assert!(ago(-7200).starts_with("in 2 hours"));
        assert_eq!(ago(86_400 * 3), "3 days ago");
    }
}
