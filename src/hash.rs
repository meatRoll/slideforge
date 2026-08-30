//! Source-file hash bookkeeping (`.src.hash` sidecar).
//!
//! The skill protocol compares the SHA-256 of a `.pptx` against the hash
//! recorded at the last sync point (convert or overwriting build) to decide
//! whether a re-`convert` is needed. Doing this *inside* the CLI — instead of
//! leaving it to shell snippets in the skill — means the agent physically
//! cannot forget to check: `convert` reports "unchanged, skipped" on its own.

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::Result;

/// Compute the lowercase hex SHA-256 of a file.
pub fn sha256_of(path: &Path) -> Result<String> {
    let data = fs::read(path).map_err(|source| crate::Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(hex(&Sha256::digest(&data)))
}

/// Path of the `.src.hash` sidecar next to `work_dir`'s main entry.
fn sidecar_path(work_dir: &Path) -> PathBuf {
    work_dir.join(".src.hash")
}

/// A recorded sync point: the hash plus (optionally) which file it refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncPoint {
    /// Lowercase hex SHA-256 of the source file at the sync point.
    pub hash: String,
    /// Path of the file the hash was taken from, when known.
    pub path: Option<PathBuf>,
}

/// Read the recorded sync-point hash, if any. Missing or unreadable → `None`
/// (treated as "never converted here", i.e. always convert).
pub fn read_stored(work_dir: &Path) -> Option<SyncPoint> {
    let text = fs::read_to_string(sidecar_path(work_dir)).ok()?;
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let hash = lines.next()?;
    if hash.is_empty() || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    // Line 2 (optional) records which file the hash belongs to; legacy
    // sidecars written by older builds carry the hash only.
    let path = lines.next().map(PathBuf::from);
    Some(SyncPoint {
        hash: hash.to_string(),
        path,
    })
}

/// Record `hash` as the current sync point for `work_dir`. When `source` is
/// known, it is recorded too so `build` can later detect that it is
/// overwriting the same file (see [`overwrites_sync_source`]).
pub fn write_stored(work_dir: &Path, hash: &str, source: Option<&Path>) -> Result<()> {
    let mut text = format!("{hash}\n");
    if let Some(p) = source {
        text.push_str(&p.to_string_lossy());
        text.push('\n');
    }
    fs::write(sidecar_path(work_dir), text).map_err(|source| crate::Error::Io {
        path: sidecar_path(work_dir),
        source,
    })
}

/// Hash `input` and compare against the stored sync-point hash.
///
/// When the sidecar records which file the hash belongs to, a *different*
/// file never matches — even with identical content — because the sync
/// point is per source file, not per content.
pub fn matches_stored(input: &Path, work_dir: &Path) -> Result<bool> {
    let Some(sp) = read_stored(work_dir) else {
        return Ok(false);
    };
    if sp.hash != sha256_of(input)? {
        return Ok(false);
    }
    if let Some(recorded) = &sp.path {
        if !same_file(input, recorded) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// True when `output` overwrites the file recorded as the sync-point
/// source: building it in place means it *is* the new sync point, so the
/// CLI auto-records the fresh hash without needing an explicit `--sync`.
pub fn overwrites_sync_source(output: &Path, work_dir: &Path) -> bool {
    read_stored(work_dir)
        .and_then(|sp| sp.path)
        .map(|recorded| same_file(output, &recorded))
        .unwrap_or(false)
}

/// Compare paths robustly: canonicalize when both resolve, else fall back
/// to a literal comparison.
fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_mismatch() {
        let dir = std::env::temp_dir().join(format!("sf-hash-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.pptx");
        fs::write(&file, b"hello").unwrap();

        // No stored hash yet → never matches.
        assert!(!matches_stored(&file, &dir).unwrap());

        let h = sha256_of(&file).unwrap();
        assert_eq!(h.len(), 64);
        write_stored(&dir, &h, Some(&file)).unwrap();
        assert!(matches_stored(&file, &dir).unwrap());
        assert!(overwrites_sync_source(&file, &dir));

        // Content changed → mismatch.
        fs::write(&file, b"world").unwrap();
        assert!(!matches_stored(&file, &dir).unwrap());

        // Same content but a different file → still a mismatch (sync point
        // is per source file, not per content).
        let other = dir.join("b.pptx");
        fs::write(&other, b"world").unwrap();
        assert!(!matches_stored(&other, &dir).unwrap());
        assert!(!overwrites_sync_source(&other, &dir));

        // Legacy sidecar (hash only, no path) still works.
        fs::write(
            dir.join(".src.hash"),
            format!("{}\n", sha256_of(&other).unwrap()),
        )
        .unwrap();
        assert!(matches_stored(&other, &dir).unwrap());
        assert!(!overwrites_sync_source(&other, &dir));

        let _ = fs::remove_dir_all(&dir);
    }
}
