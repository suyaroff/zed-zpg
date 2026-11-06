//! `<schema.table>.changes.sql`: what Apply wrote to a table and the
//! statements that revert it, newest first. What was applied stays in
//! comments, so only the reverting statements can be run.

pub const HEADER: &str = "-- zpg: changes written by Apply to this table, newest first.\n\n";

/// Above this many bytes the oldest entries are dropped.
pub const MAX_LEN: usize = 1 << 20;

/// One entry; statements are on one line each, so a blank line only ever
/// separates entries.
pub fn entry(time: &str, conn: &str, table: &str, summary: &str, applied: &[String], undo: &[String]) -> String {
    let mut s = format!("-- {time} · {table} · {summary}\n-- Applied:\n");
    for a in applied {
        s += &format!("--   {};\n", a.replace('\n', " "));
    }
    s += "-- Undo: select the statements below and run them; UPDATE 0 or DELETE 0 means the row changed since.\n";
    s += &format!("-- @conn {conn}\n");
    for u in undo {
        s += u;
        s += ";\n";
    }
    s
}

/// `text` with `entry` on top, under `max` bytes by dropping whole old
/// entries. The new entry stays even when it alone is larger: a part of an
/// undo is worse than none.
pub fn prepend(text: &str, entry: &str, max: usize) -> String {
    let body = text.strip_prefix(HEADER).unwrap_or(text);
    let mut out = format!("{HEADER}{entry}");
    for old in body.split("\n\n").map(str::trim_end).filter(|e| !e.is_empty()) {
        if out.len() + old.len() + 2 > max {
            break;
        }
        out.push('\n');
        out += old;
        out.push('\n');
    }
    out
}
