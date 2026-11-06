//! Reading CSV for *Import*: RFC 4180 with what spreadsheets add (a BOM,
//! `;` or tab between fields). As in `export::csv`, an empty field is NULL
//! and `""` an empty string.

use crate::cell;
use crate::{Column, Snapshot};

/// A line of the file; `None` is an empty field without quotes.
pub type Record = Vec<Option<String>>;

/// The separator used most on the first line outside quotes.
fn delimiter(text: &str) -> char {
    let (mut quoted, mut counts) = (false, [0usize; 3]);
    const CANDIDATES: [char; 3] = [',', ';', '\t'];
    for c in text.chars() {
        match c {
            '"' => quoted = !quoted,
            '\n' if !quoted => break,
            c if !quoted => {
                if let Some(k) = CANDIDATES.iter().position(|d| *d == c) {
                    counts[k] += 1;
                }
            }
            _ => {}
        }
    }
    let best = (0..CANDIDATES.len()).rev().max_by_key(|&k| counts[k]).unwrap_or(0);
    CANDIDATES[best]
}

/// The records of a file, the header first. Blank lines are skipped.
pub fn parse(text: &str) -> Result<Vec<Record>, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let delim = delimiter(text);
    let mut records: Vec<Record> = vec![];
    let mut record: Record = vec![];
    let mut field = String::new();
    let (mut was_quoted, mut in_quotes) = (false, false);
    let (mut line, mut record_line) = (1usize, 1usize);
    let end_field = |record: &mut Record, field: &mut String, was_quoted: &mut bool| {
        let f = std::mem::take(field);
        record.push(if f.is_empty() && !*was_quoted { None } else { Some(f) });
        *was_quoted = false;
    };
    let end_record = |records: &mut Vec<Record>, record: &mut Record, at: usize| -> Result<(), String> {
        let r = std::mem::take(record);
        if matches!(r.as_slice(), [None]) {
            return Ok(());
        }
        if let Some(head) = records.first()
            && head.len() != r.len()
        {
            return Err(format!("line {at}: {} fields, the header has {}", r.len(), head.len()));
        }
        records.push(r);
        Ok(())
    };

    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => in_quotes = false,
                c => {
                    line += usize::from(c == '\n');
                    field.push(c);
                }
            }
            continue;
        }
        match c {
            '"' if field.is_empty() && !was_quoted => {
                in_quotes = true;
                was_quoted = true;
            }
            c if c == delim => end_field(&mut record, &mut field, &mut was_quoted),
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' | '\r' => {
                end_field(&mut record, &mut field, &mut was_quoted);
                end_record(&mut records, &mut record, record_line)?;
                line += 1;
                record_line = line;
            }
            c => field.push(c),
        }
    }
    if in_quotes {
        return Err(format!("line {record_line}: a quote is not closed"));
    }
    if !field.is_empty() || was_quoted || !record.is_empty() {
        end_field(&mut record, &mut field, &mut was_quoted);
        end_record(&mut records, &mut record, record_line)?;
    }
    Ok(records)
}

/// The column named `name`: the exact name, else the only one that differs by case.
fn find_column(columns: &[Column], name: &str) -> Option<usize> {
    if let Some(i) = columns.iter().position(|c| c.name == name) {
        return Some(i);
    }
    let mut similar = (0..columns.len()).filter(|&i| columns[i].name.eq_ignore_ascii_case(name));
    similar.next().filter(|_| similar.next().is_none())
}

/// Table lines for the records of a file, as new rows of the page `snap`.
/// Columns the file doesn't have, empty key cells and generated columns get
/// what a new row has by default.
pub fn page_rows(snap: &Snapshot, records: &[Record]) -> Result<Vec<String>, String> {
    let Some((head, rows)) = records.split_first() else {
        return Err("the file is empty".into());
    };
    let shown = snap.shown();
    let mut field_of_cell: Vec<Option<usize>> = vec![None; shown.len()];
    for (f, name) in head.iter().enumerate() {
        let name = name.as_deref().unwrap_or("");
        let Some(col) = find_column(&snap.columns, name) else {
            return Err(format!("`{name}` is not a column of {}", snap.table));
        };
        let Some(cell) = shown.iter().position(|&c| c == col) else {
            return Err(format!("`{name}` is not on the page: add it to columns="));
        };
        if field_of_cell[cell].replace(f).is_some() {
            return Err(format!("`{name}` is in the file twice"));
        }
    }
    let template = snap.insert_template();
    Ok(rows
        .iter()
        .map(|r| {
            let cells: Vec<String> = shown
                .iter()
                .enumerate()
                .map(|(k, &col)| match field_of_cell[k].map(|f| &r[f]) {
                    Some(_) if snap.columns[col].generated => template[k].clone(),
                    Some(Some(v)) => cell::escape(v),
                    Some(None) if !snap.pk.contains(&col) => "NULL".into(),
                    _ => template[k].clone(),
                })
                .collect();
            format!("| {} |", cells.join(" | "))
        })
        .collect())
}
