//! The `.pgdb` layout: `[connection]` sections listing relations.

use crate::cell::display_width;
use crate::header::QualName;

pub const HEADER: &str = "# zpg v1 catalog";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relation {
    pub name: QualName,
    /// table, view, matview, foreign, partitioned
    pub kind: String,
    /// Planner estimate; `None` when unknown.
    pub rows: Option<i64>,
}

pub struct Section {
    pub conn: String,
    pub relations: Result<Vec<Relation>, String>,
}

pub fn render(sections: &[Section]) -> String {
    let mut out = format!(
        "{HEADER}\n# Go to definition (F12) on a table opens it; code actions refresh.\n"
    );
    if sections.is_empty() {
        out += "\n# No connections. Add them to ~/.config/zpg/config.json (code action: Open settings file):\n\
                # \"connections\": { \"local\": \"postgresql:///mydb?host=/var/run/postgresql\" }\n";
    }
    for s in sections {
        out += &format!("\n[{}]\n", s.conn);
        match &s.relations {
            Err(e) => {
                for l in e.lines() {
                    out += &format!("! {l}\n");
                }
            }
            Ok(rels) if rels.is_empty() => out += "# no tables\n",
            Ok(rels) => {
                let names: Vec<String> = rels.iter().map(|r| r.name.to_string()).collect();
                let w = names.iter().map(|n| display_width(n)).max().unwrap_or(0);
                for (r, n) in rels.iter().zip(&names) {
                    let mut line = format!("{n}{}  {:<11}", " ".repeat(w - display_width(n)), r.kind);
                    if let Some(rows) = r.rows {
                        line += &format!(" ~{rows} rows");
                    }
                    out += line.trim_end();
                    out.push('\n');
                }
            }
        }
    }
    out
}

/// The connection and relation named on `line`, if any.
pub fn entry_at(text: &str, line: u32) -> Option<(String, QualName)> {
    let mut conn = None;
    for (i, l) in text.split('\n').enumerate() {
        let t = l.trim();
        if let Some(name) = t.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            conn = Some(name.to_string());
        }
        if i as u32 == line {
            if t.starts_with(['#', '!', '[']) || t.is_empty() {
                return None;
            }
            let (q, _) = QualName::parse_prefix(t)?;
            return Some((conn?, q));
        }
    }
    None
}

/// The connection whose section `line` is in (its `[name]` line included).
pub fn conn_at(text: &str, line: u32) -> Option<String> {
    text.split('\n')
        .take(line as usize + 1)
        .filter_map(|l| l.trim().strip_prefix('[')?.strip_suffix(']').map(String::from))
        .last()
}

/// The kind written after the relation name on `line`: table, view, matview…
pub fn kind_at(text: &str, line: u32) -> Option<String> {
    let l = text.split('\n').nth(line as usize)?.trim();
    let (_, n) = QualName::parse_prefix(l)?;
    l[n..].split_whitespace().next().map(String::from)
}

/// `DROP …` for a relation of `kind`.
pub fn drop_statement(kind: &str, name: &QualName) -> String {
    let what = match kind {
        "view" => "VIEW",
        "matview" => "MATERIALIZED VIEW",
        "foreign" => "FOREIGN TABLE",
        _ => "TABLE",
    };
    format!("DROP {what} {}", name.sql())
}

pub fn is_catalog(text: &str) -> bool {
    text.lines().next().is_some_and(|l| l.trim_end() == HEADER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_and_lookup() {
        let text = render(&[
            Section {
                conn: "local".into(),
                relations: Ok(vec![
                    Relation {
                        name: QualName::new("public", "users"),
                        kind: "table".into(),
                        rows: Some(10),
                    },
                    Relation {
                        name: QualName::new("public", "My View"),
                        kind: "view".into(),
                        rows: None,
                    },
                ]),
            },
            Section {
                conn: "prod".into(),
                relations: Err("connection refused".into()),
            },
        ]);
        let lines: Vec<&str> = text.lines().collect();
        let users = lines.iter().position(|l| l.starts_with("public.users")).unwrap() as u32;
        assert_eq!(entry_at(&text, users), Some(("local".into(), QualName::new("public", "users"))));
        assert_eq!(
            entry_at(&text, users + 1),
            Some(("local".into(), QualName::new("public", "My View")))
        );
        assert!(text.contains("! connection refused"));
        assert_eq!(entry_at(&text, 0), None);
        assert_eq!(kind_at(&text, users + 1).as_deref(), Some("view"));
        assert_eq!(
            drop_statement("view", &QualName::new("public", "My View")),
            r#"DROP VIEW "public"."My View""#
        );
    }

    #[test]
    fn connection_of_a_line() {
        let text = "# zpg v1 catalog\n\n[local]\npublic.a  table\n\n[prod]\n! error\n";
        assert_eq!(conn_at(text, 1), None);
        assert_eq!(conn_at(text, 2).as_deref(), Some("local"));
        assert_eq!(conn_at(text, 4).as_deref(), Some("local"));
        assert_eq!(conn_at(text, 6).as_deref(), Some("prod"));
    }
}
