//! Settings from the user's `~/.config/zpg/config.json`, shared by all projects.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;
use zpg_core::Apply;

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ConnSpec {
    Url(String),
    Full {
        url: String,
        #[serde(default)]
        read_only: bool,
        /// `user@host` or a `~/.ssh/config` alias to tunnel through; the url
        /// then names the database as the SSH server sees it.
        #[serde(default)]
        ssh: Option<String>,
        /// Extra `ssh` arguments, e.g. `["-p", "2222"]`.
        #[serde(default)]
        ssh_options: Vec<String>,
    },
}

impl ConnSpec {
    pub fn url(&self) -> &str {
        match self {
            ConnSpec::Url(u) | ConnSpec::Full { url: u, .. } => u,
        }
    }

    pub fn read_only(&self) -> bool {
        matches!(self, ConnSpec::Full { read_only: true, .. })
    }

    pub fn ssh(&self) -> Option<(&str, &[String])> {
        match self {
            ConnSpec::Full { ssh: Some(target), ssh_options, .. } => Some((target, ssh_options)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub connections: BTreeMap<String, ConnSpec>,
    pub page_size: u32,
    pub max_cell_width: u32,
    pub apply: ApplySetting,
    /// Rows shown from a query run from a `.sql` file.
    pub max_result_rows: usize,
    /// Per run of `.sql` statements; 0 = no limit.
    pub statement_timeout_ms: u64,
    /// Have Postgres parse the statements of `.sql` files on open and save, to underline errors.
    pub check_sql: bool,
    /// The `pg_dump` program for backups.
    pub pg_dump: String,
    /// Added to every `pg_dump` command line, e.g. `["--column-inserts"]`.
    pub pg_dump_args: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApplySetting {
    Save,
    #[default]
    Manual,
}

impl From<ApplySetting> for Apply {
    fn from(a: ApplySetting) -> Self {
        match a {
            ApplySetting::Save => Apply::Save,
            ApplySetting::Manual => Apply::Manual,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            connections: BTreeMap::new(),
            page_size: 100,
            max_cell_width: 40,
            apply: ApplySetting::Manual,
            max_result_rows: 1000,
            statement_timeout_ms: 60_000,
            check_sql: true,
            pg_dump: "pg_dump".into(),
            pg_dump_args: vec![],
        }
    }
}

impl Settings {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut s: Self = serde_json::from_str(&strip_jsonc(text)).map_err(|e| e.to_string())?;
        s.page_size = s.page_size.clamp(1, 10_000);
        s.max_result_rows = s.max_result_rows.clamp(1, 100_000);
        Ok(s)
    }
}

pub const TEMPLATE: &str = r#"// zpg settings, shared by all projects. Keep this file private (chmod 600).
// Saved changes apply right away. Comments and trailing commas are allowed.
{
  "connections": {
    // "local": "postgresql:///mydb?host=/var/run/postgresql",
    // "prod": { "url": "postgresql://app:secret@db.example.com/app", "read_only": true },
    // "cloud": "postgresql://app:${CLOUD_PG_PASSWORD}@db.example.com/app?sslmode=verify-full",
    // "remote": { "url": "postgresql:///app?host=/var/run/postgresql", "ssh": "deploy@prod", "ssh_options": ["-p", "2222"] },
  },
  // "page_size": 100,
  // "max_cell_width": 40,
  // "apply": "manual",
  // "max_result_rows": 1000,
  // "statement_timeout_ms": 60000,
  // Underline errors in .sql files on open and save (Postgres parses them, nothing runs):
  // "check_sql": true,
  // Table backups (catalog or table page: ctrl-. → Back up): the program and extra arguments,
  // e.g. "pg_dump": "/usr/lib/postgresql/16/bin/pg_dump", "pg_dump_args": ["--column-inserts"]
  // "pg_dump": "pg_dump",
  // "pg_dump_args": [],
}
"#;

/// `$ZPG_CONFIG`, else `zpg/config.json` in the user's config directory.
pub fn config_path() -> Option<PathBuf> {
    let var = |name| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(p) = var("ZPG_CONFIG") {
        return Some(p);
    }
    let dir = if cfg!(windows) {
        var("APPDATA")?
    } else {
        var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|h| h.join(".config")))?
    };
    Some(dir.join("zpg").join("config.json"))
}

/// The settings file's contents; defaults when there is none.
pub fn load() -> Result<Settings, String> {
    let Some(path) = config_path() else {
        return Ok(Settings::default());
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => Settings::parse(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Creates the settings file from `TEMPLATE` unless it exists; returns its path.
pub fn ensure_config_file() -> Result<PathBuf, String> {
    let path = config_path().ok_or("no home directory to keep zpg settings in")?;
    if path.exists() {
        return Ok(path);
    }
    crate::state::atomic_write(&path, TEMPLATE.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(path)
}

/// JSON with `//` and `/* */` comments and trailing commas, as Zed's settings, made plain JSON.
fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_str = false;
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            match c {
                '\\' => out.extend(chars.next()),
                '"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_str = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                // Keep the newline so serde's line numbers stay right.
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push(c);
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut prev = ' ';
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push(c);
                    }
                    if prev == '*' && c == '/' {
                        break;
                    }
                    prev = c;
                }
            }
            ('}' | ']', _) => {
                let kept = out.trim_end().len();
                if out[..kept].ends_with(',') {
                    out.replace_range(kept - 1..kept, " ");
                }
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// Replaces `${VAR}` with the environment variable.
pub fn expand_env(s: &str) -> Result<String, String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let end = after.find('}').ok_or("unterminated `${` in connection url")?;
        let name = &after[..end];
        let val = std::env::var(name).map_err(|_| format!("environment variable {name} is not set"))?;
        out.push_str(&val);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_settings() {
        let s = Settings::parse(
            r#"{
              // a comment
              "connections": {
                "local": "postgresql:///app", /* inline */
                "prod": { "url": "postgresql://u:${PW}@db/app?x=//y", "read_only": true },
                "remote": { "url": "postgresql:///app?host=/var/run/postgresql", "ssh": "deploy@srv", "ssh_options": ["-p", "2222",] },
              },
              "page_size": 50,
            }"#,
        )
        .unwrap();
        assert_eq!(s.connections["local"].url(), "postgresql:///app");
        assert_eq!(s.connections["prod"].url(), "postgresql://u:${PW}@db/app?x=//y");
        assert!(s.connections["prod"].read_only());
        assert!(s.connections["prod"].ssh().is_none());
        let (target, opts) = s.connections["remote"].ssh().unwrap();
        assert_eq!((target, opts.len()), ("deploy@srv", 2));
        assert_eq!(s.page_size, 50);
        assert_eq!(s.max_cell_width, 40);
    }

    #[test]
    fn template() {
        let s = Settings::parse(TEMPLATE).unwrap();
        assert!(s.connections.is_empty());
        assert_eq!(s.page_size, 100);
        // Uncommenting any example line keeps it valid.
        let all = TEMPLATE.replace("    // \"", "    \"").replace("  // \"", "  \"");
        let s = Settings::parse(&all).unwrap();
        assert_eq!(s.connections.len(), 4);
        assert_eq!(s.statement_timeout_ms, 60_000);
    }

    #[test]
    fn strings_keep_comment_like_text() {
        assert_eq!(strip_jsonc(r#"{"a": "x\" // y", "b": "/* z */,}"}"#), r#"{"a": "x\" // y", "b": "/* z */,}"}"#);
        assert!(Settings::parse("{ \"page_size\": }").is_err());
    }

    #[test]
    fn env() {
        unsafe { std::env::set_var("ZPG_TEST_PW", "s3cr3t") };
        assert_eq!(expand_env("a:${ZPG_TEST_PW}@h").unwrap(), "a:s3cr3t@h");
        assert!(expand_env("${ZPG_NOPE_XYZ}").is_err());
    }
}
