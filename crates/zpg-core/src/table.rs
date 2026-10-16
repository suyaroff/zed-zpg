//! The `.pgt` text layout: a header line, then a `|`-separated table.

use crate::cell::{display_width, escape};
use crate::header::TableHeader;
use crate::{Issue, utf16_len};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellRef {
    /// Trimmed, still escaped.
    pub text: String,
    /// UTF-16 columns of `text`; an empty cell gets an empty range at its position.
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub line: u32,
    pub cells: Vec<CellRef>,
}

#[derive(Debug, Default)]
pub struct Parsed {
    pub header: Option<TableHeader>,
    pub columns: Option<Line>,
    pub rows: Vec<Line>,
    pub issues: Vec<Issue>,
}

impl Parsed {
    pub fn has_errors(&self) -> bool {
        self.issues.iter().any(|i| i.severity == crate::Severity::Error)
    }
}

enum Kind {
    Skip,
    Separator,
    Table(Vec<CellRef>),
    Junk,
}

fn is_separator(s: &str) -> bool {
    s.contains('-') && s.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

fn classify(text: &str) -> Kind {
    let t = text.trim();
    if t.is_empty() || t.starts_with('#') {
        Kind::Skip
    } else if !t.starts_with('|') {
        Kind::Junk
    } else if is_separator(t) {
        Kind::Separator
    } else {
        Kind::Table(split(text))
    }
}

/// Splits a table line on unescaped `|`.
pub fn split(text: &str) -> Vec<CellRef> {
    let mut segments: Vec<(usize, usize)> = vec![];
    let Some(first) = text.find('|') else {
        return vec![];
    };
    let mut start = first + 1;
    let mut chars = text[start..].char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            '|' => {
                segments.push((start, start + i));
                start = start + i + 1;
                chars = text[start..].char_indices();
            }
            _ => {}
        }
    }
    if !text[start..].trim().is_empty() {
        segments.push((start, text.len()));
    }
    segments
        .into_iter()
        .map(|(a, b)| {
            let seg = &text[a..b];
            let lead = seg.len() - seg.trim_start_matches([' ', '\t']).len();
            let body = seg.trim_matches([' ', '\t']);
            let col = utf16_len(&text[..a + lead.min(seg.len())]);
            CellRef {
                text: body.to_string(),
                start: col,
                end: col + utf16_len(body),
            }
        })
        .collect()
}

pub fn parse(text: &str) -> Parsed {
    let mut p = Parsed::default();
    let mut lines = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
    match lines.next() {
        Some(first) => match TableHeader::parse(first) {
            Ok(h) => p.header = Some(h),
            Err(e) => p.issues.push(Issue::line_error(0, first, e)),
        },
        None => return p,
    }
    for (n, line) in lines.enumerate() {
        let n = n as u32 + 1;
        match classify(line) {
            Kind::Skip | Kind::Separator => {}
            Kind::Junk => p
                .issues
                .push(Issue::line_error(n, line, "expected a table row starting with `|`")),
            Kind::Table(cells) => {
                let l = Line { line: n, cells };
                if p.columns.is_none() {
                    p.columns = Some(l);
                } else {
                    p.rows.push(l);
                }
            }
        }
    }
    p
}

fn render_line(cells: &[String], widths: &[usize]) -> String {
    let mut s = String::from("|");
    for (i, c) in cells.iter().enumerate() {
        let w = widths.get(i).copied().unwrap_or(0);
        s.push(' ');
        s.push_str(c);
        s.push_str(&" ".repeat(w.saturating_sub(display_width(c))));
        s.push_str(" |");
    }
    s
}

fn widths<'a>(lines: impl Iterator<Item = &'a Vec<String>>) -> Vec<usize> {
    let mut w: Vec<usize> = vec![];
    for cells in lines {
        for (i, c) in cells.iter().enumerate() {
            if w.len() <= i {
                w.push(0);
            }
            w[i] = w[i].max(display_width(c));
        }
    }
    w
}

fn separator(widths: &[usize]) -> String {
    let mut s = String::from("|");
    for w in widths {
        s.push_str(&"-".repeat(w + 2));
        s.push('|');
    }
    s
}

/// Renders a page; `cells` are already escaped.
pub fn render(header_line: &str, columns: &[String], cells: &[Vec<String>]) -> String {
    let names: Vec<String> = columns.iter().map(|c| escape(c)).collect();
    let w = widths(std::iter::once(&names).chain(cells.iter()));
    let mut out = format!("{header_line}\n{}\n{}\n", render_line(&names, &w), separator(&w));
    for row in cells {
        out.push_str(&render_line(row, &w));
        out.push('\n');
    }
    out
}

/// Re-pads table lines without touching cell contents. `None` if nothing changes.
pub fn realign(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.split('\n').collect();
    let kinds: Vec<Kind> = lines
        .iter()
        .enumerate()
        .map(|(i, l)| if i == 0 { Kind::Skip } else { classify(l) })
        .collect();
    let rows: Vec<Vec<String>> = kinds
        .iter()
        .filter_map(|k| match k {
            Kind::Table(c) => Some(c.iter().map(|c| c.text.clone()).collect()),
            _ => None,
        })
        .collect();
    let w = widths(rows.iter());
    let mut rows = rows.into_iter();
    let out: Vec<String> = lines
        .iter()
        .zip(&kinds)
        .map(|(l, k)| match k {
            Kind::Table(_) => render_line(&rows.next().unwrap_or_default(), &w),
            Kind::Separator => separator(&w),
            _ => l.to_string(),
        })
        .collect();
    let out = out.join("\n");
    (out != text).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_escaped_pipes() {
        let c = split("| a\\|b |  c |");
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].text, "a\\|b");
        assert_eq!((c[0].start, c[0].end), (2, 6));
        assert_eq!(c[1].text, "c");
        assert_eq!(c[1].start, 10);
        assert_eq!(split("| a | b").len(), 2);
        assert_eq!(split("| a |  |").len(), 2);
        assert_eq!(split("| a |  |")[1].text, "");
    }

    #[test]
    fn render_parse() {
        let h = "# zpg v1 conn=c table=public.t";
        let text = render(
            h,
            &["id".into(), "name".into()],
            &[vec!["1".into(), "кот".into()], vec!["22".into(), "NULL".into()]],
        );
        assert_eq!(
            text,
            "# zpg v1 conn=c table=public.t\n| id | name |\n|----|------|\n| 1  | кот  |\n| 22 | NULL |\n"
        );
        let p = parse(&text);
        assert!(p.issues.is_empty(), "{:?}", p.issues);
        assert_eq!(p.columns.unwrap().cells.len(), 2);
        assert_eq!(p.rows.len(), 2);
        assert_eq!(p.rows[1].line, 4);
        assert_eq!(realign(&text), None);
        let messy = text.replace("| 1  | кот  |", "|1|кот длинный|");
        let fixed = realign(&messy).unwrap();
        assert!(fixed.contains("| 1  | кот длинный |"), "{fixed}");
        assert!(fixed.contains("|----|-------------|"), "{fixed}");
    }
}
