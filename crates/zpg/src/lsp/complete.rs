use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionTextEdit, InsertTextFormat, Position, Range, TextEdit,
};
use zpg_core::cell;
use zpg_core::header::display_ident;
use zpg_core::{Column, Snapshot};

fn item(label: &str, kind: CompletionItemKind, detail: Option<&str>, range: Range) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        kind: Some(kind),
        detail: detail.map(Into::into),
        filter_text: Some(label.to_string()),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(range, label.to_string()))),
        ..Default::default()
    }
}

/// Values that fit column `i`; `range` is what a choice replaces.
pub fn values(snap: &Snapshot, i: usize, range: Range) -> Vec<CompletionItem> {
    let Some(c): Option<&Column> = snap.columns.get(i) else {
        return vec![];
    };
    let mut out = vec![];
    if snap.pk.contains(&i) && (c.has_default || c.generated) {
        out.push(item("+", CompletionItemKind::KEYWORD, Some("new row, key from its default"), range));
    }
    if c.has_default || c.generated {
        out.push(item("DEFAULT", CompletionItemKind::KEYWORD, Some("column default"), range));
    }
    if !c.not_null && !snap.pk.contains(&i) {
        out.push(item("NULL", CompletionItemKind::KEYWORD, None, range));
    }
    if c.type_name == "boolean" {
        for v in ["true", "false"] {
            out.push(item(v, CompletionItemKind::VALUE, None, range));
        }
    }
    for v in &c.enum_values {
        out.push(item(&cell::escape(v), CompletionItemKind::ENUM_MEMBER, Some(&c.type_name), range));
    }
    out
}

/// Inside `where="…"` / `order="…"` / `columns="…"`: column names; elsewhere: header fields.
pub fn header(line: &str, pos: Position, snap: &Snapshot) -> Vec<CompletionItem> {
    let chars: Vec<char> = line.chars().collect();
    let mut col16 = 0;
    let mut idx = 0;
    while idx < chars.len() && col16 < pos.character {
        col16 += chars[idx].len_utf16() as u32;
        idx += 1;
    }
    // Which `key=` the cursor is in, and whether inside its quotes.
    let mut key = String::new();
    let mut word = String::new();
    let mut in_value = false;
    let mut quoted = false;
    let mut i = 0;
    while i < idx {
        let c = chars[i];
        if quoted {
            match c {
                '\\' => i += 1,
                '"' => {
                    quoted = false;
                    in_value = false;
                }
                _ => {}
            }
        } else if c == '=' && !in_value {
            key = std::mem::take(&mut word);
            in_value = true;
            quoted = chars.get(i + 1) == Some(&'"');
            if quoted {
                i += 1;
            }
        } else if c.is_whitespace() {
            word.clear();
            in_value = false;
        } else if !in_value {
            word.push(c);
        }
        i += 1;
    }
    let start = chars[..idx]
        .iter()
        .rposition(|c| !(c.is_alphanumeric() || *c == '_'))
        .map_or(0, |p| p + 1);
    let start16: u32 = chars[..start].iter().map(|c| c.len_utf16() as u32).sum();
    let range = Range::new(Position::new(pos.line, start16), pos);

    if quoted && matches!(key.as_str(), "where" | "order" | "columns") {
        let mut out: Vec<CompletionItem> = snap
            .columns
            .iter()
            .map(|c| item(&display_ident(&c.name), CompletionItemKind::FIELD, Some(&c.type_name), range))
            .collect();
        let extra: &[&str] = match key.as_str() {
            "order" => &["asc", "desc", "nulls first", "nulls last"],
            "where" => &["and", "or", "not", "is null", "is not null", "in", "like", "ilike", "between"],
            _ => &[],
        };
        out.extend(extra.iter().map(|k| item(k, CompletionItemKind::KEYWORD, None, range)));
        return out;
    }
    if in_value {
        return match key.as_str() {
            "apply" => ["save", "manual"].iter().map(|v| item(v, CompletionItemKind::VALUE, None, range)).collect(),
            _ => vec![],
        };
    }
    [
        ("where", "where=\"$0\"", "filter rows (SQL condition)"),
        ("order", "order=\"$0\"", "sort rows (SQL ORDER BY list)"),
        ("columns", "columns=\"$0\"", "columns to show, comma-separated"),
        ("page", "page=$0", "page number"),
        ("limit", "limit=$0", "rows per page"),
        ("width", "width=$0", "max cell width, 0 = no limit"),
        ("apply", "apply=$0", "save | manual"),
    ]
    .iter()
    .map(|(label, snippet, detail)| CompletionItem {
        label: label.to_string(),
        kind: Some(CompletionItemKind::PROPERTY),
        detail: Some(detail.to_string()),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(range, snippet.to_string()))),
        ..Default::default()
    })
    .collect()
}
