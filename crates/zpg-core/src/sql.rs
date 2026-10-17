//! Statements for reading a page and writing changes. Every value travels as
//! text and is cast by Postgres, so any column type works without client-side
//! conversions.

use crate::cell::Value;
use crate::diff::Op;
use crate::header::{Query, quote_ident};
use crate::{Column, Snapshot};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stmt {
    pub sql: String,
    pub params: Vec<Option<String>>,
}

/// Result columns shared by SELECT and RETURNING: xmin, then every column, as text.
fn returning(columns: &[Column], has_xmin: bool) -> String {
    let mut s = String::from(if has_xmin { "xmin::text" } else { "NULL::text" });
    for c in columns {
        s += &format!(", {}::text", quote_ident(&c.name));
    }
    s
}

struct Builder {
    params: Vec<Option<String>>,
}

impl Builder {
    fn cast(&mut self, v: &Value, ty: &str) -> String {
        match v {
            Value::Default | Value::Plus => "DEFAULT".into(),
            Value::Null => "NULL".into(),
            Value::Text(s) => {
                self.params.push(Some(s.clone()));
                format!("CAST(${}::text AS {ty})", self.params.len())
            }
        }
    }

    /// `"pk" = ... AND xmin = ...` for a snapshot row.
    fn row_match(&mut self, snap: &Snapshot, row: usize) -> String {
        let r = &snap.rows[row];
        let mut parts: Vec<String> = snap
            .pk
            .iter()
            .map(|&i| {
                let c = &snap.columns[i];
                let v = Value::from_db(r.values[i].as_deref());
                format!("{} = {}", quote_ident(&c.name), self.cast(&v, &c.type_name))
            })
            .collect();
        if let Some(x) = &r.xmin {
            parts.push(format!("xmin = {}", self.cast(&Value::Text(x.clone()), "xid")));
        }
        parts.join(" AND ")
    }
}

/// ` WHERE (<filter>)`, or nothing without a filter.
fn filter_clause(q: &Query) -> String {
    if q.filter.trim().is_empty() {
        String::new()
    } else {
        format!(" WHERE ({})", q.filter)
    }
}

pub fn select_page(snap: &Snapshot, has_xmin: bool, q: &Query) -> Stmt {
    let mut sql = format!("SELECT {} FROM {}", returning(&snap.columns, has_xmin), snap.table.sql());
    sql += &filter_clause(q);
    if !q.order.trim().is_empty() {
        sql += &format!(" ORDER BY {}", q.order);
    } else if !snap.pk.is_empty() {
        let keys: Vec<String> = snap.pk.iter().map(|&i| quote_ident(&snap.columns[i].name)).collect();
        sql += &format!(" ORDER BY {}", keys.join(", "));
    }
    sql += &format!(" LIMIT {} OFFSET {}", q.limit, q.offset());
    Stmt { sql, params: vec![] }
}

/// Counts the rows `q.filter` matches.
pub fn count(snap: &Snapshot, q: &Query) -> String {
    format!("SELECT count(*) FROM {}{}", snap.table.sql(), filter_clause(q))
}

/// The planner's guess of the same, as a JSON plan with `Plan Rows`.
pub fn estimate(snap: &Snapshot, q: &Query) -> String {
    format!("EXPLAIN (FORMAT JSON) SELECT 1 FROM {}{}", snap.table.sql(), filter_clause(q))
}

/// One row by key. With `pretty`, jsonb comes indented over several lines,
/// which the row view shows as a block.
pub fn select_row(snap: &Snapshot, key: &[Option<String>], pretty: bool) -> Stmt {
    let mut b = Builder { params: vec![] };
    let mut cols = String::from("xmin::text");
    for c in &snap.columns {
        let name = quote_ident(&c.name);
        if pretty && c.type_name == "jsonb" {
            cols += &format!(", jsonb_pretty({name})");
        } else {
            cols += &format!(", {name}::text");
        }
    }
    let cond: Vec<String> = snap
        .pk
        .iter()
        .zip(key)
        .map(|(&i, v)| {
            let c = &snap.columns[i];
            format!("{} = {}", quote_ident(&c.name), b.cast(&Value::from_db(v.as_deref()), &c.type_name))
        })
        .collect();
    Stmt {
        sql: format!("SELECT {cols} FROM {} WHERE {}", snap.table.sql(), cond.join(" AND ")),
        params: b.params,
    }
}

pub fn for_op(snap: &Snapshot, op: &Op) -> Stmt {
    let mut b = Builder { params: vec![] };
    let table = snap.table.sql();
    let ret = returning(&snap.columns, true);
    let sql = match op {
        Op::Update { row, sets, .. } => {
            let sets: Vec<String> = sets
                .iter()
                .map(|(i, v)| {
                    let c = &snap.columns[*i];
                    format!("{} = {}", quote_ident(&c.name), b.cast(v, &c.type_name))
                })
                .collect();
            let cond = b.row_match(snap, *row);
            format!("UPDATE {table} SET {} WHERE {cond} RETURNING {ret}", sets.join(", "))
        }
        Op::Insert { values, .. } => {
            let (cols, vals): (Vec<String>, Vec<String>) = snap
                .columns
                .iter()
                .zip(values)
                .filter(|(_, v)| **v != Value::Default)
                .map(|(c, v)| (quote_ident(&c.name), b.cast(v, &c.type_name)))
                .unzip();
            if cols.is_empty() {
                format!("INSERT INTO {table} DEFAULT VALUES RETURNING {ret}")
            } else {
                format!(
                    "INSERT INTO {table} ({}) VALUES ({}) RETURNING {ret}",
                    cols.join(", "),
                    vals.join(", ")
                )
            }
        }
        Op::Delete { row } => {
            let cond = b.row_match(snap, *row);
            format!("DELETE FROM {table} WHERE {cond}")
        }
    };
    Stmt { sql, params: b.params }
}

/// Human-readable SQL with parameters inlined as literals, for previews.
pub fn inline(stmt: &Stmt) -> String {
    let mut sql = stmt.sql.clone();
    for (i, p) in stmt.params.iter().enumerate().rev() {
        let lit = match p {
            Some(s) => format!("'{}'", s.replace('\'', "''")),
            None => "NULL".into(),
        };
        sql = sql.replace(&format!("${}::text", i + 1), &lit);
    }
    sql
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::{Apply, QualName};
    use crate::{Column, SnapRow};

    fn snap() -> Snapshot {
        let col = |n: &str, t: &str| Column {
            name: n.into(),
            type_name: t.into(),
            not_null: false,
            has_default: false,
            generated: false,
            enum_values: vec![],
            comment: None,
        };
        Snapshot {
            conn: "c".into(),
            table: QualName::new("public", "T"),
            columns: vec![col("id", "integer"), col("na\"me", "text")],
            pk: vec![0],
            foreign_keys: vec![],
            referenced_by: vec![],
            total: None,
            read_only: None,
            query: Query {
                page: 2,
                limit: 10,
                filter: String::new(),
                order: String::new(),
            },
            width: 40,
            apply: Apply::Save,
            visible: vec![],
            rows: vec![SnapRow {
                xmin: Some("77".into()),
                values: vec![Some("5".into()), Some("x".into())],
            }],
        }
    }

    #[test]
    fn statements() {
        let s = snap();
        assert_eq!(
            select_page(&s, true, &s.query).sql,
            r#"SELECT xmin::text, "id"::text, "na""me"::text FROM "public"."T" ORDER BY "id" LIMIT 10 OFFSET 10"#
        );
        let up = for_op(
            &s,
            &Op::Update {
                line: 3,
                row: 0,
                sets: vec![(1, Value::Text("it's".into()))],
            },
        );
        assert_eq!(
            up.sql,
            r#"UPDATE "public"."T" SET "na""me" = CAST($1::text AS text) WHERE "id" = CAST($2::text AS integer) AND xmin = CAST($3::text AS xid) RETURNING xmin::text, "id"::text, "na""me"::text"#
        );
        assert_eq!(up.params, vec![Some("it's".into()), Some("5".into()), Some("77".into())]);
        assert!(inline(&up).contains("CAST('it''s' AS text)"));
        let ins = for_op(
            &s,
            &Op::Insert {
                line: 4,
                values: vec![Value::Default, Value::Null],
            },
        );
        assert_eq!(
            ins.sql,
            r#"INSERT INTO "public"."T" ("na""me") VALUES (NULL) RETURNING xmin::text, "id"::text, "na""me"::text"#
        );
        let del = for_op(&s, &Op::Delete { row: 0 });
        assert_eq!(
            del.sql,
            r#"DELETE FROM "public"."T" WHERE "id" = CAST($1::text AS integer) AND xmin = CAST($2::text AS xid)"#
        );
    }
}
