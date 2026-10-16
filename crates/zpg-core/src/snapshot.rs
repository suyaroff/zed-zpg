use serde::{Deserialize, Serialize};

use crate::cell::{self, Value, display_width};
use crate::header::{Apply, QualName, Query, TableHeader};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    /// `format_type()` output, usable in `CAST(... AS <type>)`.
    pub type_name: String,
    pub not_null: bool,
    pub has_default: bool,
    /// Stored generated column: only DEFAULT can be written.
    pub generated: bool,
    /// Labels of an enum type, for completion.
    #[serde(default)]
    pub enum_values: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForeignKey {
    /// Indices into `Snapshot::columns`.
    pub columns: Vec<usize>,
    pub table: QualName,
    /// Referenced column names, matching `columns` one to one.
    pub ref_columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapRow {
    /// Row version for optimistic locking; `None` for views.
    pub xmin: Option<String>,
    pub values: Vec<Option<String>>,
}

/// What the buffer showed after the last load or write, kept outside the buffer
/// because on save the file on disk already holds the edited text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub conn: String,
    pub table: QualName,
    pub columns: Vec<Column>,
    /// Indices into `columns`.
    pub pk: Vec<usize>,
    #[serde(default)]
    pub foreign_keys: Vec<ForeignKey>,
    /// Why the rows can't be edited, if they can't.
    pub read_only: Option<String>,
    pub query: Query,
    pub width: u32,
    pub apply: Apply,
    /// Column names from the header's `columns=`; empty = all.
    #[serde(default)]
    pub visible: Vec<String>,
    pub rows: Vec<SnapRow>,
}

/// Zed soft-wraps lines longer than 512 columns whatever the settings say, so
/// table lines are kept below that.
pub const MAX_LINE: usize = 500;

/// Narrowest a column gets when squeezing a wide table.
const MIN_CUT: u32 = 8;

impl Snapshot {
    pub fn header(&self) -> TableHeader {
        TableHeader {
            conn: self.conn.clone(),
            table: self.table.clone(),
            pk: self
                .pk
                .iter()
                .map(|&i| self.columns[i].name.as_str())
                .collect::<Vec<_>>()
                .join(","),
            query: self.query.clone(),
            width: self.width,
            apply: self.apply,
            columns: self.visible.clone(),
        }
    }

    /// Indices of the columns the buffer shows, in table order: those named in
    /// `visible` (all when empty) plus the key.
    pub fn shown(&self) -> Vec<usize> {
        (0..self.columns.len())
            .filter(|&i| self.visible.is_empty() || self.pk.contains(&i) || self.visible.contains(&self.columns[i].name))
            .collect()
    }

    /// Names in `visible` that aren't columns of the table.
    pub fn unknown_visible(&self, names: &[String]) -> Vec<String> {
        names
            .iter()
            .filter(|n| !self.columns.iter().any(|c| &c.name == *n))
            .cloned()
            .collect()
    }

    pub fn column_names(&self) -> Vec<String> {
        self.shown().into_iter().map(|i| self.columns[i].name.clone()).collect()
    }

    /// Cut width of every column (0 = not cut). Key columns are never cut:
    /// they identify the row. When a line would get longer than `MAX_LINE`,
    /// the widest other columns are cut further to a common width.
    pub fn widths(&self) -> Vec<u32> {
        let shown = self.shown();
        let base: Vec<u32> = (0..self.columns.len())
            .map(|i| if self.pk.contains(&i) { 0 } else { self.width })
            .collect();
        let name_w: Vec<usize> = self.columns.iter().map(|c| display_width(&cell::escape(&c.name))).collect();
        let full_w: Vec<usize> = (0..self.columns.len())
            .map(|i| {
                self.rows
                    .iter()
                    .map(|r| display_width(&cell::render_cut(&Value::from_db(r.values[i].as_deref()), base[i]).0))
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let line_len = |cap: Option<usize>| {
            1 + shown
                .iter()
                .map(|&i| {
                    let w = match cap {
                        Some(c) if !self.pk.contains(&i) => full_w[i].min(c),
                        _ => full_w[i],
                    };
                    w.max(name_w[i]) + 3
                })
                .sum::<usize>()
        };
        if line_len(None) <= MAX_LINE {
            return base;
        }
        let widest = shown.iter().map(|&i| full_w[i]).max().unwrap_or(0);
        let mut cap = MIN_CUT as usize;
        let (mut lo, mut hi) = (MIN_CUT as usize, widest);
        while lo <= hi {
            let mid = (lo + hi) / 2;
            if line_len(Some(mid)) <= MAX_LINE {
                cap = mid;
                lo = mid + 1;
            } else {
                hi = mid - 1;
            }
        }
        (0..self.columns.len())
            .map(|i| {
                if self.pk.contains(&i) || full_w[i] <= cap.max(name_w[i]) {
                    base[i]
                } else {
                    cap.max(name_w[i]) as u32
                }
            })
            .collect()
    }

    /// Text of column `i` holding `v` with `widths` from [`Self::widths`], and
    /// whether it was cut.
    pub fn cell(&self, i: usize, v: &Option<String>, widths: &[u32]) -> (String, bool) {
        cell::render_cut(&Value::from_db(v.as_deref()), widths.get(i).copied().unwrap_or(self.width))
    }

    pub fn cells(&self) -> Vec<Vec<String>> {
        let w = self.widths();
        let shown = self.shown();
        self.rows
            .iter()
            .map(|r| shown.iter().map(|&i| self.cell(i, &r.values[i], &w).0).collect())
            .collect()
    }

    pub fn render(&self) -> String {
        crate::table::render(&self.header().render(), &self.column_names(), &self.cells())
    }

    /// A row to paste for inserting: `+` in key columns that have defaults,
    /// DEFAULT where the table has one, NULL where allowed.
    pub fn insert_template(&self) -> Vec<String> {
        self.shown()
            .into_iter()
            .map(|i| {
                let c = &self.columns[i];
                if self.pk.contains(&i) && (c.has_default || c.generated) {
                    "+"
                } else if c.has_default || c.generated {
                    "DEFAULT"
                } else if !c.not_null {
                    "NULL"
                } else {
                    ""
                }
                .to_string()
            })
            .collect()
    }
}
