pub mod catalog;
pub mod cell;
pub mod diff;
pub mod export;
pub mod header;
pub mod history;
pub mod record;
pub mod snapshot;
pub mod sqlctx;
pub mod sqlsplit;
pub mod sql;
pub mod table;

pub use header::{Apply, QualName, Query, TableHeader, Total};
pub use snapshot::{BackRef, Column, ForeignKey, Snapshot, SnapRow};

pub const MAGIC: &str = "# zpg v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// A problem located on one line; `start`/`end` are UTF-16 columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub line: u32,
    pub start: u32,
    pub end: u32,
    pub severity: Severity,
    pub message: String,
}

impl Issue {
    pub fn error(line: u32, start: u32, end: u32, message: impl Into<String>) -> Self {
        Self {
            line,
            start,
            end,
            severity: Severity::Error,
            message: message.into(),
        }
    }

    pub fn line_error(line: u32, text: &str, message: impl Into<String>) -> Self {
        Self::error(line, 0, utf16_len(text), message)
    }
}

pub fn utf16_len(s: &str) -> u32 {
    s.chars().map(|c| c.len_utf16() as u32).sum()
}
