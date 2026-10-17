//! `.zpg/history.sql`: statements run from `.sql` files, newest last. Each
//! entry carries `-- @conn`, so ▶ runs it again on the same connection.

pub const HEADER: &str = "-- zpg: statements run from .sql files, newest last; ▶ runs one again.\n\n";

/// Above this many bytes the oldest entries are dropped, down to about half.
pub const MAX_LEN: usize = 1 << 20;

/// One entry; `summary` is e.g. `SELECT 12 · 34 ms`.
pub fn entry(time: &str, conn: &str, summary: &str, stmts: &[&str]) -> String {
    let mut s = format!("-- {time} · {summary}\n-- @conn {conn}\n");
    for st in stmts {
        s += st.trim_end().trim_end_matches(';');
        s += ";\n";
    }
    s.push('\n');
    s
}

/// `text` with `entry` added, keeping it under `max` bytes by dropping whole old entries.
pub fn append(text: &str, entry: &str, max: usize) -> String {
    let body = text.strip_prefix(HEADER).unwrap_or(text);
    let mut all = format!("{body}{entry}");
    if all.len() > max {
        let mut from = all.len().saturating_sub(max / 2);
        while !all.is_char_boundary(from) {
            from += 1;
        }
        let cut = all[from..].find("\n\n-- ").map_or(all.len() - entry.len(), |p| from + p + 2);
        all = all[cut.min(all.len() - entry.len())..].to_string();
    }
    format!("{HEADER}{all}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_and_trimming() {
        let e = entry("2026-10-17 12:00:00", "local", "SELECT 1 · 3 ms", &["select 1", "select 2;"]);
        assert_eq!(e, "-- 2026-10-17 12:00:00 · SELECT 1 · 3 ms\n-- @conn local\nselect 1;\nselect 2;\n\n");
        let one = append("", &e, MAX_LEN);
        assert_eq!(one, format!("{HEADER}{e}"));
        let two = append(&one, &e, MAX_LEN);
        assert_eq!(two.matches("@conn").count(), 2);

        let mut text = String::new();
        for i in 0..100 {
            text = append(&text, &entry(&i.to_string(), "c", "s", &["select 'ж'"]), 1000);
        }
        assert!(text.len() <= 1000 + HEADER.len(), "{}", text.len());
        assert!(text.starts_with(HEADER));
        assert!(text[HEADER.len()..].starts_with("-- "));
        assert!(text.ends_with("-- 99 · s\n-- @conn c\nselect 'ж';\n\n"));
    }
}
