//! Semantic tokens: column names, and the cells saving would write.

use lsp_server::Connection;
use lsp_types::SemanticToken;
use lsp_types::request::SemanticTokensRefresh;
use zpg_core::{diff, record, table};

use super::{Kind, Res, Server, snapshot};

/// Semantic token types, styled by `semantic_token_rules.json` of each language.
const TOKEN_MODIFIED: u32 = 0;
const TOKEN_INSERTED: u32 = 1;
const TOKEN_COLUMN: u32 = 2;
pub(super) const TOKEN_TYPES: [&str; 3] = ["zpgModified", "zpgInserted", "zpgColumn"];

impl Server {
    /// Column names, and cells that saving would write.
    pub(super) fn semantic_tokens(&mut self, key: &str) -> Vec<SemanticToken> {
        let Some(doc) = self.docs.get(key) else {
            return vec![];
        };
        let kind = doc.kind();
        let text = doc.text.clone();
        let Some(path) = doc.path() else {
            return vec![];
        };
        if kind == Kind::Result {
            let parsed = table::parse(&text);
            let spans = parsed
                .columns
                .iter()
                .flat_map(|cols| cols.cells.iter().map(|c| (cols.line, c.start, c.end, TOKEN_COLUMN)))
                .collect();
            return encode_tokens(spans);
        }
        let Some(snap) = snapshot(&mut self.snaps, &path) else {
            return vec![];
        };
        // (line, start, end, type)
        let mut spans: Vec<(u32, u32, u32, u32)> = vec![];
        match kind {
            Kind::Table => {
                let parsed = table::parse(&text);
                if let Some(cols) = &parsed.columns {
                    spans.extend(cols.cells.iter().map(|c| (cols.line, c.start, c.end, TOKEN_COLUMN)));
                }
                if !parsed.has_errors() {
                    let cells_of = |line: u32| parsed.rows.iter().find(|r| r.line == line).map(|r| &r.cells);
                    let shown = snap.shown();
                    for op in diff::diff(&parsed, snap).ops {
                        match op {
                            diff::Op::Update { line, sets, .. } => {
                                if let Some(cells) = cells_of(line) {
                                    for (col, _) in &sets {
                                        if let Some(c) = shown.iter().position(|s| s == col).and_then(|k| cells.get(k))
                                        {
                                            spans.push((line, c.start, c.end, TOKEN_MODIFIED));
                                        }
                                    }
                                }
                            }
                            diff::Op::Insert { line, .. } => {
                                if let Some(cells) = cells_of(line) {
                                    spans.extend(cells.iter().map(|c| (line, c.start, c.end, TOKEN_INSERTED)));
                                }
                            }
                            diff::Op::Delete { .. } => {}
                        }
                    }
                }
            }
            Kind::Row => {
                let parsed = record::parse(&text, snap);
                for op in record::diff(&parsed, snap).ops {
                    let diff::Op::Update { sets, .. } = op else { continue };
                    for f in parsed.fields.iter().filter(|f| sets.iter().any(|(i, _)| *i == f.col)) {
                        spans.push((f.line, f.start, f.end, TOKEN_MODIFIED));
                        for (k, l) in f.lines.iter().enumerate().skip(1) {
                            spans.push((f.line + k as u32, 0, zpg_core::utf16_len(l), TOKEN_MODIFIED));
                        }
                    }
                }
            }
            _ => {}
        }
        encode_tokens(spans)
    }

    pub(super) fn refresh_tokens(&mut self, conn: &Connection) -> Res<()> {
        if self.caps.tokens_refresh {
            self.send_request::<SemanticTokensRefresh>(conn, ())?;
        }
        Ok(())
    }
}

/// LSP's relative encoding of (line, start, end, type) spans.
fn encode_tokens(mut spans: Vec<(u32, u32, u32, u32)>) -> Vec<SemanticToken> {
    spans.retain(|s| s.2 > s.1);
    spans.sort();
    let mut out = Vec::with_capacity(spans.len());
    let (mut prev_line, mut prev_start) = (0, 0);
    for (line, start, end, typ) in spans {
        let delta_line = line - prev_line;
        out.push(SemanticToken {
            delta_line,
            delta_start: if delta_line == 0 { start - prev_start } else { start },
            length: end - start,
            token_type: typ,
            token_modifiers_bitset: 0,
        });
        (prev_line, prev_start) = (line, start);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_relative() {
        let t = encode_tokens(vec![(2, 4, 6, 1), (0, 1, 3, 0), (2, 0, 2, 2), (3, 5, 5, 0)]);
        let got: Vec<_> = t.iter().map(|t| (t.delta_line, t.delta_start, t.length, t.token_type)).collect();
        assert_eq!(got, vec![(0, 1, 2, 0), (2, 0, 2, 2), (0, 4, 2, 1)]);
    }
}
