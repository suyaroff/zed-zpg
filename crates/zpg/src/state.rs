//! Snapshots live in `.state/` next to the `.pgt` file they belong to.

use std::path::{Path, PathBuf};

use zpg_core::Snapshot;

pub fn state_path(pgt: &Path) -> PathBuf {
    let name = pgt.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    pgt.with_file_name(".state").join(format!("{name}.json"))
}

pub fn load(pgt: &Path) -> Option<Snapshot> {
    let text = std::fs::read_to_string(state_path(pgt)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save(pgt: &Path, snap: &Snapshot) -> Result<(), String> {
    let path = state_path(pgt);
    let json = serde_json::to_string(snap).map_err(|e| e.to_string())?;
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
