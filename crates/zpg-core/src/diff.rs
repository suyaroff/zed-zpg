//! Buffer rows vs. snapshot rows, matched by primary key.
//!
//! A row whose key isn't in the snapshot (or has `+`/DEFAULT in a key cell) is
//! inserted; a snapshot row missing from the buffer is deleted. So editing a key
//! value means delete + insert.

use std::collections::HashMap;

use crate::cell::{self, Value};
use crate::table::{Line, Parsed};
use crate::{Issue, Snapshot};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Update { line: u32, row: usize, sets: Vec<(usize, Value)> },
    /// One value per column; `Plus` is already turned into `Default`.
    Insert { line: u32, values: Vec<Value> },
    Delete { row: usize },
}

impl Op {
    pub fn line(&self) -> Option<u32> {
        match self {
            Op::Update { line, .. } | Op::Insert { line, .. } => Some(*line),
            Op::Delete { .. } => None,
        }
    }
}

/// Where a buffer row comes from, in buffer order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Snapshot(usize),
    /// Index into `Diff::ops`.
    Op(usize),
}

#[derive(Debug, Default)]
pub struct Diff {
    pub ops: Vec<Op>,
    pub order: Vec<Source>,
    pub issues: Vec<Issue>,
}

impl Diff {
    pub fn counts(&self) -> (usize, usize, usize) {
        let mut c = (0, 0, 0);
        for op in &self.ops {
            match op {
                Op::Update { .. } => c.0 += 1,
                Op::Insert { .. } => c.1 += 1,
                Op::Delete { .. } => c.2 += 1,
            }
        }
        c
    }

    pub fn summary(&self) -> String {
        let (u, i, d) = self.counts();
        let mut parts = vec![];
        for (n, what) in [(u, "update"), (i, "insert"), (d, "delete")] {
            if n > 0 {
                parts.push(format!("{n} {what}{}", if n == 1 { "" } else { "s" }));
            }
        }
        parts.join(", ")
    }
}

fn cell_issue(line: u32, c: &crate::table::CellRef, msg: impl Into<String>) -> Issue {
    Issue::error(line, c.start, c.end.max(c.start + 1), msg)
}

pub fn diff(parsed: &Parsed, snap: &Snapshot) -> Diff {
    let mut d = Diff::default();
    let ncols = snap.columns.len();

    if let Some(cols) = &parsed.columns {
        let names: Vec<Value> = cols.cells.iter().map(|c| cell::parse(&c.text).unwrap_or(Value::Null)).collect();
        let expected: Vec<Value> = snap.columns.iter().map(|c| Value::Text(c.name.clone())).collect();
        if names != expected {
            d.issues.push(Issue::error(
                cols.line,
                0,
                cols.cells.last().map_or(1, |c| c.end + 2),
                format!("column row must stay `{}`", snap.column_names().join(" | ")),
            ));
            return d;
        }
    }

    let mut rows: Vec<&Line> = vec![];
    for row in &parsed.rows {
        if row.cells.len() != ncols {
            let end = row.cells.last().map_or(1, |c| c.end + 2);
            d.issues.push(Issue::error(
                row.line,
                0,
                end,
                format!("row has {} cells, table has {ncols} columns", row.cells.len()),
            ));
        } else {
            rows.push(row);
        }
    }
    if !d.issues.is_empty() {
        return d;
    }

    if let Some(why) = &snap.read_only {
        let same = rows.len() == snap.rows.len()
            && rows.iter().zip(&snap.rows).all(|(r, s)| {
                r.cells.iter().zip(&s.values).enumerate().all(|(i, (c, v))| c.text == snap.cell(i, v).0)
            });
        if !same {
            d.issues.push(Issue::error(0, 0, 8, format!("read-only: {why}; edits won't be saved")));
        }
        d.order = (0..snap.rows.len()).map(Source::Snapshot).collect();
        return d;
    }

    let mut by_key: HashMap<Vec<String>, usize> = HashMap::new();
    for (i, r) in snap.rows.iter().enumerate() {
        by_key.insert(snap.pk.iter().map(|&c| snap.cell(c, &r.values[c]).0).collect(), i);
    }
    let mut seen = vec![false; snap.rows.len()];

    for row in rows {
        let key: Vec<String> = snap.pk.iter().map(|&c| row.cells[c].text.clone()).collect();
        let new = key.iter().any(|k| k == "+" || k == "DEFAULT");
        let found = if new { None } else { by_key.get(&key).copied() };

        let mut values = Vec::with_capacity(ncols);
        let mut bad = false;
        for (i, c) in row.cells.iter().enumerate() {
            match cell::parse(&c.text) {
                Ok(Value::Plus) if !snap.pk.contains(&i) => {
                    d.issues.push(cell_issue(row.line, c, "`+` is only allowed in key columns"));
                    bad = true;
                }
                Ok(Value::Null) if snap.pk.contains(&i) => {
                    d.issues.push(cell_issue(row.line, c, "key column can't be NULL"));
                    bad = true;
                }
                Ok(v) => values.push(v),
                Err(e) => {
                    d.issues.push(cell_issue(row.line, c, e));
                    bad = true;
                }
            }
        }
        if bad {
            continue;
        }

        match found {
            Some(i) if seen[i] => {
                let c = &row.cells[snap.pk[0]];
                d.issues.push(cell_issue(row.line, c, "duplicate key: this row appears twice"));
            }
            Some(i) => {
                seen[i] = true;
                let snap_row = &snap.rows[i];
                let mut sets = vec![];
                for (j, c) in row.cells.iter().enumerate() {
                    if snap.pk.contains(&j) {
                        continue;
                    }
                    let (text, was_cut) = snap.cell(j, &snap_row.values[j]);
                    if c.text == text {
                        continue;
                    }
                    if was_cut {
                        d.issues.push(cell_issue(
                            row.line,
                            c,
                            format!("value is cut to width={}; raise `width` in the header to edit it", snap.width),
                        ));
                        continue;
                    }
                    if values[j] == Value::from_db(snap_row.values[j].as_deref()) {
                        continue;
                    }
                    if snap.columns[j].generated && values[j] != Value::Default {
                        d.issues.push(cell_issue(row.line, c, "generated column: only DEFAULT can be written"));
                        continue;
                    }
                    sets.push((j, values[j].clone()));
                }
                if sets.is_empty() {
                    d.order.push(Source::Snapshot(i));
                } else {
                    d.order.push(Source::Op(d.ops.len()));
                    d.ops.push(Op::Update {
                        line: row.line,
                        row: i,
                        sets,
                    });
                }
            }
            None => {
                let values: Vec<Value> = values
                    .into_iter()
                    .map(|v| if v == Value::Plus { Value::Default } else { v })
                    .collect();
                for (j, c) in row.cells.iter().enumerate() {
                    if snap.columns[j].generated && values[j] != Value::Default {
                        d.issues.push(cell_issue(row.line, c, "generated column: only DEFAULT can be written"));
                    }
                }
                d.order.push(Source::Op(d.ops.len()));
                d.ops.push(Op::Insert { line: row.line, values });
            }
        }
    }

    for (i, s) in seen.iter().enumerate() {
        if !s {
            d.ops.push(Op::Delete { row: i });
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::{Apply, QualName, Query};
    use crate::{Column, SnapRow, table};

    fn col(name: &str, has_default: bool) -> Column {
        Column {
            name: name.into(),
            type_name: "text".into(),
            not_null: false,
            has_default,
            generated: false,
            enum_values: vec![],
        }
    }

    fn snap() -> Snapshot {
        let row = |id: &str, name: Option<&str>| SnapRow {
            xmin: Some("1".into()),
            values: vec![Some(id.into()), name.map(Into::into)],
        };
        Snapshot {
            conn: "c".into(),
            table: QualName::new("public", "t"),
            columns: vec![col("id", true), col("name", false)],
            pk: vec![0],
            foreign_keys: vec![],
            read_only: None,
            query: Query {
                page: 1,
                limit: 100,
                filter: String::new(),
                order: String::new(),
            },
            width: 10,
            apply: Apply::Save,
            rows: vec![row("1", Some("a")), row("2", None), row("3", Some("very long value"))],
        }
    }

    #[test]
    fn unchanged() {
        let s = snap();
        let d = diff(&table::parse(&s.render()), &s);
        assert!(d.ops.is_empty() && d.issues.is_empty(), "{d:?}");
        assert_eq!(d.order, vec![Source::Snapshot(0), Source::Snapshot(1), Source::Snapshot(2)]);
    }

    #[test]
    fn update_insert_delete() {
        let s = snap();
        let text = s
            .render()
            .replace("| 1  | a          |", "| 1 | b |")
            .replace("| 2  | NULL       |\n", "")
            + "| +  | new |\n| 9 | NULL |\n";
        let d = diff(&table::parse(&text), &s);
        assert!(d.issues.is_empty(), "{:?}", d.issues);
        assert_eq!(
            d.ops,
            vec![
                Op::Update {
                    line: 3,
                    row: 0,
                    sets: vec![(1, Value::Text("b".into()))]
                },
                Op::Insert {
                    line: 5,
                    values: vec![Value::Default, Value::Text("new".into())]
                },
                Op::Insert {
                    line: 6,
                    values: vec![Value::Text("9".into()), Value::Null]
                },
                Op::Delete { row: 1 },
            ]
        );
        assert_eq!(d.summary(), "1 update, 2 inserts, 1 delete");
    }

    #[test]
    fn cut_cells_are_read_only() {
        let s = snap();
        let text = s.render().replace("very long…", "changed");
        let d = diff(&table::parse(&text), &s);
        assert_eq!(d.issues.len(), 1, "{:?}", d.issues);
        assert!(d.issues[0].message.contains("width"));
    }

    #[test]
    fn long_keys_are_not_cut() {
        let mut s = snap();
        s.rows[0].values[0] = Some("cache-0c8a518a715046b604d39cc8be47e767".into());
        s.rows[1].values[0] = Some("cache-0c8a518a715046b604d39cc8be47e768".into());
        let text = s.render();
        assert!(text.contains("e767") && text.contains("e768"), "{text}");
        let d = diff(&table::parse(&text), &s);
        assert!(d.ops.is_empty() && d.issues.is_empty(), "{d:?}");
    }

    #[test]
    fn duplicate_key() {
        let s = snap();
        let text = s.render() + "| 1 | x |\n";
        let d = diff(&table::parse(&text), &s);
        assert!(d.issues[0].message.contains("duplicate"), "{:?}", d.issues);
    }
}
