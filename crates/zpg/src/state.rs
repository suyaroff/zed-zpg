//! What was loaded for a file lives in `.state/` next to it.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

pub fn state_path(pgt: &Path) -> PathBuf {
    let name = pgt.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    pgt.with_file_name(".state").join(format!("{name}.json"))
}

pub fn load<T: DeserializeOwned>(file: &Path) -> Option<T> {
    let text = std::fs::read_to_string(state_path(file)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save<T: Serialize>(file: &Path, state: &T) -> Result<(), String> {
    let path = state_path(file);
    let json = serde_json::to_string(state).map_err(|e| e.to_string())?;
    atomic_write(&path, json.as_bytes())
}

pub fn atomic_write(path: &Path, data: &[u8]) -> Result<(), String> {
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(err)?;
    }
    let tmp = path.with_extension("zpg-tmp");
    std::fs::write(&tmp, data).map_err(err)?;
    std::fs::rename(&tmp, path).map_err(err)
}

/// Creates the `.zpg` directory with a `.gitignore` that keeps it out of git.
pub fn ensure_root(root: &Path) -> Result<(), String> {
    let ignore = root.join(".gitignore");
    if ignore.exists() {
        return Ok(());
    }
    atomic_write(&ignore, b"*\n")
}
