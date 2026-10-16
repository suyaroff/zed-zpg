//! `lsp.zpg.settings` from Zed's settings.json.

use std::collections::BTreeMap;

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
        }
    }
}

impl Settings {
    /// Accepts the settings object itself or one wrapped as `{"zpg": {...}}`.
    pub fn from_value(v: Option<serde_json::Value>) -> Result<Self, String> {
        let v = match v {
            None | Some(serde_json::Value::Null) => return Ok(Self::default()),
            Some(serde_json::Value::Object(mut m)) if m.len() == 1 && m.contains_key("zpg") => m.remove("zpg").unwrap(),
            Some(v) => v,
        };
        let mut s: Self = serde_json::from_value(v).map_err(|e| format!("invalid lsp.zpg.settings: {e}"))?;
        s.page_size = s.page_size.clamp(1, 10_000);
        s.max_result_rows = s.max_result_rows.clamp(1, 100_000);
        Ok(s)
    }
}

/// Replaces `${VAR}` with the environment variable, so passwords can stay out of settings.json.
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
        let s = Settings::from_value(Some(serde_json::json!({
            "connections": {
                "local": "postgresql:///app",
                "prod": { "url": "postgresql://u:${PW}@db/app", "read_only": true }
            },
            "page_size": 50
        })))
        .unwrap();
        assert_eq!(s.connections["local"].url(), "postgresql:///app");
        assert!(s.connections["prod"].read_only());
        assert_eq!(s.page_size, 50);
        assert_eq!(s.max_cell_width, 40);
    }

    #[test]
    fn env() {
        unsafe { std::env::set_var("ZPG_TEST_PW", "s3cr3t") };
        assert_eq!(expand_env("a:${ZPG_TEST_PW}@h").unwrap(), "a:s3cr3t@h");
        assert!(expand_env("${ZPG_NOPE_XYZ}").is_err());
    }
}
