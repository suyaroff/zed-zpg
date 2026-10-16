use serde::{Deserialize, Serialize};

use crate::cell::{self, Value};
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
    pub rows: Vec<SnapRow>,
}

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
        }
    }

    pub fn column_names(&self) -> Vec<String> {
        self.columns.iter().map(|c| c.name.clone()).collect()
    }

    /// Text of column `i` holding `v`, and whether it was cut to the snapshot
    /// width. Key columns are never cut: they identify the row.
    pub fn cell(&self, i: usize, v: &Option<String>) -> (String, bool) {
        let width = if self.pk.contains(&i) { 0 } else { self.width };
        cell::render_cut(&Value::from_db(v.as_deref()), width)
    }

    pub fn cells(&self) -> Vec<Vec<String>> {
        self.rows
            .iter()
            .map(|r| r.values.iter().enumerate().map(|(i, v)| self.cell(i, v).0).collect())
            .collect()
    }

    pub fn render(&self) -> String {
        crate::table::render(&self.header().render(), &self.column_names(), &self.cells())
    }

    /// A row to paste for inserting: `+` in key columns that have defaults,
    /// DEFAULT where the table has one, NULL where allowed.
    pub fn insert_template(&self) -> Vec<String> {
        self.columns
            .iter()
            .enumerate()
            .map(|(i, c)| {
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
