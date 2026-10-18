//! File paths: `file://` URIs and the names of generated files.

use std::path::{Path, PathBuf};

use lsp_types::Uri;
use zpg_core::header::encode_name;

use super::Res;

pub(super) fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    let s = uri.as_str().strip_prefix("file://")?;
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(b) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    let path = String::from_utf8(out).ok()?;
    // file:///C:/dir -> C:/dir
    if cfg!(windows) && path.as_bytes().get(2) == Some(&b':') {
        return Some(PathBuf::from(&path[1..]));
    }
    Some(PathBuf::from(path))
}

pub(super) fn path_to_uri(path: &Path) -> Res<Uri> {
    let mut s = path.to_string_lossy().into_owned();
    if cfg!(windows) {
        s = format!("/{}", s.replace('\\', "/"));
    }
    let mut out = String::from("file://");
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/:".contains(&b) {
            out.push(b as char);
        } else {
            out += &format!("%{b:02X}");
        }
    }
    Ok(out.parse()?)
}

/// File name of a row's `.pgr`: its key, shortened with a hash when long.
pub(super) fn row_file(key: &[Option<String>]) -> String {
    let text = key.iter().map(|v| v.as_deref().unwrap_or("NULL")).collect::<Vec<_>>().join(",");
    let enc = encode_name(&text);
    if enc.chars().count() <= 120 {
        return format!("{enc}.pgr");
    }
    format!("{}-{:016x}.pgr", enc.chars().take(80).collect::<String>(), fnv(&text))
}

/// FNV-1a: stable across builds and runs, unlike std's hasher.
pub(super) fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100000001b3))
}

/// `.zpg/<conn>/schema.table.pgt` -> `.zpg/<conn>/schema.table.rows/`
pub(super) fn rows_dir(table_path: &Path) -> PathBuf {
    table_path.with_extension("rows")
}

/// `.zpg/<conn>/schema.table.rows/<key>.pgr` -> `.zpg/<conn>/schema.table.pgt`
pub(super) fn table_of_row(row_path: &Path) -> Option<PathBuf> {
    Some(row_path.parent()?.with_extension("pgt"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn uri_round_trip() {
        let path = Path::new("/tmp/a b/кот%#.pgt");
        let uri = path_to_uri(path).unwrap();
        assert_eq!(uri.as_str(), "file:///tmp/a%20b/%D0%BA%D0%BE%D1%82%25%23.pgt");
        assert_eq!(uri_to_path(&uri).as_deref(), Some(path));
        assert_eq!(uri_to_path(&"untitled:x".parse().unwrap()), None);
    }

    #[test]
    fn row_files() {
        assert_eq!(row_file(&[Some("1".into()), None]), "1%2CNULL.pgr");
        let long = row_file(&[Some("x".repeat(200))]);
        assert!(long.starts_with(&"x".repeat(80)) && long.ends_with(".pgr") && long.len() == 80 + 1 + 16 + 4, "{long}");
        assert_ne!(long, row_file(&[Some("x".repeat(201))]));
    }
}
