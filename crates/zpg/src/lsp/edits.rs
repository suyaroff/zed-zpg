use std::time::{Duration, Instant};

use lsp_types::{Position, Range, TextEdit};
use similar::{Algorithm, DiffOp, capture_diff_slices_deadline};
use zpg_core::utf16_len;

/// Edits turning `old` into `new` that touch as little as possible, so the cursor and
/// scroll position survive re-rendering the buffer.
pub fn minimal_edits(old: &str, new: &str) -> Vec<TextEdit> {
    if old == new {
        return vec![];
    }
    let a: Vec<&str> = old.split('\n').collect();
    let b: Vec<&str> = new.split('\n').collect();
    let deadline = Instant::now() + Duration::from_millis(300);
    let mut edits = vec![];
    for op in capture_diff_slices_deadline(Algorithm::Myers, &a, &b, Some(deadline)) {
        let (o0, o1, n0, n1) = match op {
            DiffOp::Equal { .. } => continue,
            DiffOp::Delete { old_index, old_len, new_index } => (old_index, old_index + old_len, new_index, new_index),
            DiffOp::Insert { old_index, new_index, new_len } => (old_index, old_index, new_index, new_index + new_len),
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                (old_index, old_index + old_len, new_index, new_index + new_len)
            }
        };
        if o1 - o0 == n1 - n0 {
            for (i, j) in (o0..o1).zip(n0..n1) {
                edits.extend(line_edits(i as u32, a[i], b[j]));
            }
        } else if o1 < a.len() {
            let text: String = b[n0..n1].iter().flat_map(|l| [*l, "\n"]).collect();
            edits.push(TextEdit::new(Range::new(Position::new(o0 as u32, 0), Position::new(o1 as u32, 0)), text));
        } else {
            // Touches the last line, which has no trailing newline to anchor on.
            let start = if o0 > 0 { Position::new(o0 as u32 - 1, utf16_len(a[o0 - 1])) } else { Position::new(0, 0) };
            let mut text = b[n0..n1].join("\n");
            if o0 > 0 && n1 > n0 {
                text.insert(0, '\n');
            }
            let end = Position::new(a.len() as u32 - 1, utf16_len(a[a.len() - 1]));
            edits.push(TextEdit::new(Range::new(start, end), text));
        }
    }
    edits
}

/// Character-level edits within one line.
fn line_edits(line: u32, old: &str, new: &str) -> Vec<TextEdit> {
    let a: Vec<char> = old.chars().collect();
    let b: Vec<char> = new.chars().collect();
    let col = |chars: &[char]| chars.iter().map(|c| c.len_utf16() as u32).sum::<u32>();
    let edit = |o0: usize, o1: usize, n0: usize, n1: usize| {
        TextEdit::new(
            Range::new(Position::new(line, col(&a[..o0])), Position::new(line, col(&a[..o1]))),
            b[n0..n1].iter().collect(),
        )
    };
    capture_diff_slices_deadline(Algorithm::Myers, &a, &b, None)
        .into_iter()
        .filter_map(|op| match op {
            DiffOp::Equal { .. } => None,
            DiffOp::Delete { old_index, old_len, new_index } => {
                Some(edit(old_index, old_index + old_len, new_index, new_index))
            }
            DiffOp::Insert { old_index, new_index, new_len } => {
                Some(edit(old_index, old_index, new_index, new_index + new_len))
            }
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                Some(edit(old_index, old_index + old_len, new_index, new_index + new_len))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `text` with `edits` (in order, not overlapping) applied.
    fn apply(text: &str, edits: &[TextEdit]) -> String {
        let offset = |p: Position| {
            let line_start: usize = text.split('\n').take(p.line as usize).map(|l| l.len() + 1).sum();
            let line = text[line_start.min(text.len())..].split('\n').next().unwrap_or("");
            let mut col = 0;
            for (b, c) in line.char_indices() {
                if col >= p.character {
                    return line_start + b;
                }
                col += c.len_utf16() as u32;
            }
            line_start + line.len()
        };
        let mut out = text.to_string();
        for e in edits.iter().rev() {
            out.replace_range(offset(e.range.start)..offset(e.range.end), &e.new_text);
        }
        out
    }

    #[test]
    fn edits_turn_old_into_new() {
        let cases = [
            ("a\nb\nc\n", "a\nb\nc\n"),
            ("a\nb\nc\n", "a\nB\nc\n"),
            ("| 1 | кот |\n| 2 | x |\n", "| 1 | кот 🐈 |\n| 2 | x |\n"),
            ("a\nb\nc\n", "a\nx\ny\nb\nc\n"),
            ("a\nb\nc\nd\n", "a\nd\n"),
            ("a\nb", "a\nb\nc"),
            ("a\nb\nc", "a"),
            ("a\nb\n", "a\nb"),
            ("", "x\ny\n"),
            ("x\ny\n", ""),
        ];
        for (old, new) in cases {
            let edits = minimal_edits(old, new);
            assert_eq!(apply(old, &edits), new, "{old:?} -> {new:?}: {edits:?}");
        }
        assert!(minimal_edits("a\n", "a\n").is_empty());
    }

    #[test]
    fn every_short_pair() {
        let mut texts = vec![String::new()];
        for _ in 0..4 {
            let longer: Vec<String> = texts.iter().flat_map(|t| ["a", "é", "\n"].map(|c| format!("{t}{c}"))).collect();
            texts.extend(longer);
        }
        texts.sort();
        texts.dedup();
        for old in &texts {
            for new in &texts {
                assert_eq!(&apply(old, &minimal_edits(old, new)), new, "{old:?} -> {new:?}");
            }
        }
    }

    #[test]
    fn changed_cell_is_a_small_edit() {
        let edits = minimal_edits("h\n| 1 | abc |\n| 2 | def |\n", "h\n| 1 | abd |\n| 2 | def |\n");
        assert_eq!(edits.len(), 1, "{edits:?}");
        assert_eq!(edits[0].range.start, Position::new(1, 8));
        assert_eq!(edits[0].new_text, "d");
    }
}
