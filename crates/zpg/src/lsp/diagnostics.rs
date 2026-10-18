//! Diagnostics of every kind of buffer.

use lsp_server::Connection;
use lsp_types::notification::PublishDiagnostics;
use lsp_types::{Diagnostic, DiagnosticSeverity, Position, PublishDiagnosticsParams, Range};
use zpg_core::diff::{self, Diff};
use zpg_core::{Apply, Issue, Severity, cell, record, table};

use super::{Kind, Res, Server, notify, snapshot};

impl Server {
    pub(super) fn publish(&mut self, conn: &Connection, key: &str) -> Res<()> {
        let Some(doc) = self.docs.get(key) else {
            return Ok(());
        };
        let kind = doc.kind();
        let uri = doc.uri.clone();
        let version = doc.version;
        let text = doc.text.clone();
        let mut issues = vec![];
        let path = doc.path();
        let no_state = |what: &str| Issue::header(format!("no saved state for this file; open it from {what}"));
        match (kind, path) {
            (Kind::Table, Some(path)) => {
                let parsed = table::parse(&text);
                issues.extend(parsed.issues.iter().cloned());
                match snapshot(&mut self.snaps, &path) {
                    None => issues.push(no_state("the catalog (or run Refresh)")),
                    Some(snap) if !parsed.has_errors() => {
                        if let Some(h) = &parsed.header
                            && (h.conn != snap.conn || h.table != snap.table)
                        {
                            issues.push(Issue::header(format!(
                                "`conn` and `table` must stay {} / {}",
                                snap.conn, snap.table
                            )));
                        }
                        if let Some(h) = &parsed.header {
                            let unknown = snap.unknown_visible(&h.columns);
                            if !unknown.is_empty() {
                                issues.push(Issue::header(format!("`columns`: no such column {}", unknown.join(", "))));
                            }
                        }
                        let d = diff::diff(&parsed, snap);
                        let manual = parsed.header.as_ref().is_some_and(|h| h.apply == Apply::Manual);
                        issues.extend(pending(&d, manual));
                        // Zed wraps lines over 512 columns no matter what, and cutting values didn't help.
                        let longest = text.lines().skip(1).map(cell::display_width).max().unwrap_or(0);
                        if longest > 512
                            && let Some(cols) = &parsed.columns
                        {
                            issues.push(Issue {
                                line: cols.line,
                                start: 0,
                                end: 1,
                                severity: Severity::Info,
                                message: format!(
                                    "rows are {longest} characters and Zed wraps lines over 512; \
                                     pick columns with `columns=\"…\"` in the header"
                                ),
                            });
                        }
                    }
                    Some(_) => {}
                }
            }
            (Kind::Row, Some(path)) => match snapshot(&mut self.snaps, &path) {
                None => issues.push(no_state("its table (F12 on the key)")),
                Some(snap) => {
                    let parsed = record::parse(&text, snap);
                    issues.extend(parsed.issues.iter().cloned());
                    if parsed.issues.is_empty() {
                        issues.extend(pending(&record::diff(&parsed, snap), snap.apply == Apply::Manual));
                    }
                }
            },
            (Kind::Result, Some(path)) => issues.extend(self.result_issues(&text, &path)),
            (Kind::Sql, _) => {
                issues.extend(Server::danger_issues(&text));
                issues.extend(self.checked.get(key).into_iter().flatten().cloned());
            }
            _ => {}
        }
        if let Some(f) = self.failed.get(key) {
            issues.push(f.clone());
        }
        let diags = issues
            .into_iter()
            .map(|i| Diagnostic {
                range: Range::new(Position::new(i.line, i.start), Position::new(i.line, i.end)),
                severity: Some(match i.severity {
                    Severity::Error => DiagnosticSeverity::ERROR,
                    Severity::Warning => DiagnosticSeverity::WARNING,
                    Severity::Info => DiagnosticSeverity::INFORMATION,
                }),
                source: Some("zpg".into()),
                message: i.message,
                ..Default::default()
            })
            .collect();
        notify::<PublishDiagnostics>(conn, PublishDiagnosticsParams::new(uri, diags, Some(version)))
    }
}

/// The diff's own issues plus a header note listing what saving would write.
fn pending(d: &Diff, manual: bool) -> Vec<Issue> {
    let mut out = d.issues.clone();
    if !d.ops.is_empty() && !d.issues.iter().any(|i| i.severity == Severity::Error) {
        let how = if manual { "run the code action “Apply changes”" } else { "save to write them" };
        out.push(Issue { severity: Severity::Info, ..Issue::header(format!("pending: {}; {how}", d.summary())) });
    }
    out
}
