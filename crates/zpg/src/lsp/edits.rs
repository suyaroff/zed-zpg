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
            edits.push(TextEdit::new(
                Range::new(Position::new(o0 as u32, 0), Position::new(o1 as u32, 0)),
                text,
            ));
        } else {
            // Touches the last line, which has no trailing newline to anchor on.
            let start = if o0 > 0 {
                Position::new(o0 as u32 - 1, utf16_len(a[o0 - 1]))
            } else {
                Position::new(0, 0)
            };
            let mut text = b[n0..n1].join("\n");
            if o0 > 0 {
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
