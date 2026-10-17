//! Loaded rows as CSV, JSON or INSERT statements. Values come as Postgres
//! text output; `None` is NULL.

use crate::header::{QualName, display_ident};

type Row = Vec<Option<String>>;

/// Rows per INSERT statement.
const INSERT_BATCH: usize = 500;

fn csv_field(v: &str) -> String {
    if v.is_empty() || v.contains([',', '"', '\n', '\r']) || v.trim() != v {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v.to_string()
    }
}

/// RFC 4180 with a header line; NULL is an empty field, an empty string `""`.
pub fn csv(names: &[String], rows: &[Row]) -> String {
    let mut out = names.iter().map(|n| csv_field(n)).collect::<Vec<_>>().join(",");
    out.push('\n');
    for r in rows {
        let fields: Vec<String> = r.iter().map(|v| v.as_deref().map_or(String::new(), csv_field)).collect();
        out += &fields.join(",");
        out.push('\n');
    }
    out
}

fn json_string(s: &str) -> String {
    let mut out = String::from('"');
    for c in s.chars() {
        match c {
            '"' => out += "\\\"",
            '\\' => out += "\\\\",
            '\n' => out += "\\n",
            '\r' => out += "\\r",
            '\t' => out += "\\t",
            c if (c as u32) < 0x20 => out += &format!("\\u{:04x}", c as u32),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A value of type `ty` (as `format_type` or the wire type name gives it) as JSON.
fn json_value(v: &str, ty: &str) -> String {
    let number = matches!(
        ty,
        "smallint" | "integer" | "bigint" | "real" | "double precision" | "int2" | "int4" | "int8" | "float4" | "float8" | "oid"
    ) || ty.starts_with("numeric");
    match ty {
        "boolean" | "bool" if v == "true" || v == "false" => v.to_string(),
        "json" | "jsonb" => v.to_string(),
        _ if number && v.parse::<f64>().is_ok_and(f64::is_finite) => v.to_string(),
        _ => json_string(v),
    }
}

/// An array of objects, one per line; numbers, booleans and json stay typed.
pub fn json(names: &[String], types: &[String], rows: &[Row]) -> String {
    let mut out = String::from("[");
    for (k, r) in rows.iter().enumerate() {
        let fields: Vec<String> = names
            .iter()
            .zip(types)
            .zip(r)
            .map(|((n, t), v)| {
                let v = v.as_deref().map_or("null".to_string(), |v| json_value(v, t));
                format!("{}: {v}", json_string(n))
            })
            .collect();
        out += if k == 0 { "\n  {" } else { ",\n  {" };
        out += &fields.join(", ");
        out.push('}');
    }
    out += if rows.is_empty() { "]\n" } else { "\n]\n" };
    out
}

fn literal(v: &Option<String>) -> String {
    match v {
        None => "NULL".into(),
        Some(s) => format!("'{}'", s.replace('\'', "''")),
    }
}

/// `INSERT INTO table (…) VALUES …;` in batches.
pub fn inserts(table: &QualName, names: &[String], rows: &[Row]) -> String {
    let cols: Vec<String> = names.iter().map(|n| display_ident(n)).collect();
    let mut out = String::new();
    for chunk in rows.chunks(INSERT_BATCH) {
        out += &format!("INSERT INTO {table} ({}) VALUES\n", cols.join(", "));
        let values: Vec<String> = chunk
            .iter()
            .map(|r| format!("  ({})", r.iter().map(literal).collect::<Vec<_>>().join(", ")))
            .collect();
        out += &values.join(",\n");
        out += ";\n";
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<Row> {
        vec![
            vec![Some("1".into()), Some("a,\"b\"".into()), None, Some("{\"k\": [1]}".into())],
            vec![Some("2".into()), Some(String::new()), Some("true".into()), None],
        ]
    }

    fn names() -> Vec<String> {
        ["id", "name", "ok", "doc"].map(String::from).to_vec()
    }

    #[test]
    fn csv_quotes_only_when_needed() {
        assert_eq!(csv(&names(), &rows()), "id,name,ok,doc\n1,\"a,\"\"b\"\"\",,\"{\"\"k\"\": [1]}\"\n2,\"\",true,\n");
        assert_eq!(csv_field(" x"), "\" x\"");
        assert_eq!(csv_field("line\nbreak"), "\"line\nbreak\"");
    }

    #[test]
    fn json_keeps_types() {
        let types = ["bigint", "text", "boolean", "jsonb"].map(String::from).to_vec();
        assert_eq!(
            json(&names(), &types, &rows()),
            "[\n  {\"id\": 1, \"name\": \"a,\\\"b\\\"\", \"ok\": null, \"doc\": {\"k\": [1]}},\n  \
             {\"id\": 2, \"name\": \"\", \"ok\": true, \"doc\": null}\n]\n"
        );
        assert_eq!(json(&names(), &types, &[]), "[]\n");
        assert_eq!(json_value("NaN", "numeric(10,2)"), "\"NaN\"");
        assert_eq!(json_value("1.5", "numeric(10,2)"), "1.5");
        assert_eq!(json_value("x\ty", "text"), "\"x\\ty\"");
    }

    #[test]
    fn insert_statements() {
        let sql = inserts(&QualName::new("public", "T"), &names(), &rows()[..1]);
        assert_eq!(
            sql,
            "INSERT INTO public.\"T\" (id, name, ok, doc) VALUES\n  ('1', 'a,\"b\"', NULL, '{\"k\": [1]}');\n"
        );
        let many: Vec<Row> = (0..501).map(|i| vec![Some(i.to_string())]).collect();
        assert_eq!(inserts(&QualName::new("public", "t"), &["id".into()], &many).matches("INSERT").count(), 2);
    }
}
