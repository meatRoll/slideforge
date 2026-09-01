//! Sync-point bookkeeping (`.sync.hash` sidecar).
//!
//! One sidecar in the PPTD work directory records, for every file this
//! project has legitimately written, the SHA-256 the file had at that
//! moment. Format: a `<hash>` line followed by its `<path>` line, repeated:
//!
//! ```text
//! <sha256-hex>
//! <canonical path>
//! ```
//!
//! `convert` records the source it converted; `build` records the output it
//! wrote. Guards then compare live: when a later `build` wants to overwrite
//! an existing file, that file's *current* hash is computed on the spot and
//! checked against the recorded value. A mismatch means an external edit the
//! PPTD cannot vouch for, and the build refuses. Doing this inside the CLI —
//! instead of leaving it to shell snippets in the skill — means the agent
//! physically cannot forget to check.
//!
//! Older builds kept the convert source in `.src.hash` (optionally without a
//! path line) and, briefly, the last build output in `.build.hash`. Those
//! sidecars use the same pair format; they are absorbed into the map on
//! load and removed on the next save.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::Result;

/// Sidecar file names, all inside the work directory.
const SYNC_SIDECAR: &str = ".sync.hash";
const LEGACY_SIDECARS: [&str; 2] = [".src.hash", ".build.hash"];

/// Compute the lowercase hex SHA-256 of a file.
pub fn sha256_of(path: &Path) -> Result<String> {
    let data = fs::read(path).map_err(|source| crate::Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(hex(&Sha256::digest(&data)))
}

fn sidecar_path(work_dir: &Path, name: &str) -> PathBuf {
    work_dir.join(name)
}

/// Canonical form of `path`, falling back to the literal path when it does
/// not resolve (e.g. the file was moved or deleted after being recorded).
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// True for a 64-character lowercase hex string.
fn is_hash(text: &str) -> bool {
    text.len() == 64 && text.chars().all(|c| c.is_ascii_hexdigit())
}

/// Sync records for a work directory: canonical file path → hash at the
/// last legitimate write of that file by this project.
#[derive(Debug, Default, Clone)]
pub struct Records {
    entries: BTreeMap<PathBuf, String>,
    /// Hash from a legacy path-less `.src.hash`. It names no file, so it can
    /// only be re-bound to the file being converted (see `legacy_hash`).
    legacy_hash: Option<String>,
    /// Whether any legacy sidecar was found on load (removed on `save`).
    legacy_present: bool,
}

impl Records {
    /// Load the work directory's records, absorbing legacy sidecars.
    /// Legacy entries only fill keys the current format does not already
    /// know; the current format is authoritative.
    pub fn load(work_dir: &Path) -> Records {
        let mut records = Records::default();
        if let Some(entries) = parse_pairs(&sidecar_path(work_dir, SYNC_SIDECAR)) {
            records.entries = entries;
        }
        for name in LEGACY_SIDECARS {
            let Some(sp) = parse_single(&sidecar_path(work_dir, name)) else {
                continue;
            };
            records.legacy_present = true;
            match sp.path {
                Some(path) => {
                    records.entries.entry(canonical(&path)).or_insert(sp.hash);
                }
                None => {
                    if records.legacy_hash.is_none() {
                        records.legacy_hash = Some(sp.hash);
                    }
                }
            }
        }
        records
    }

    /// The recorded hash for `path`, if this project ever wrote it.
    pub fn get(&self, path: &Path) -> Option<&String> {
        self.entries.get(&canonical(path))
    }

    /// Record `hash` as the last-known state of `path`.
    pub fn set(&mut self, path: &Path, hash: &str) {
        self.entries.insert(canonical(path), hash.to_string());
    }

    /// Hash from a legacy path-less sidecar, if any.
    pub fn legacy_hash(&self) -> Option<&String> {
        self.legacy_hash.as_ref()
    }

    /// Whether legacy sidecars were found (they are removed on `save`).
    pub fn has_legacy(&self) -> bool {
        self.legacy_present
    }

    /// Write the records and remove absorbed legacy sidecars.
    pub fn save(&self, work_dir: &Path) -> Result<()> {
        let mut text = String::new();
        for (path, hash) in &self.entries {
            text.push_str(hash);
            text.push('\n');
            text.push_str(&path.to_string_lossy());
            text.push('\n');
        }
        fs::write(sidecar_path(work_dir, SYNC_SIDECAR), text).map_err(|source| {
            crate::Error::Io {
                path: sidecar_path(work_dir, SYNC_SIDECAR),
                source,
            }
        })?;
        for name in LEGACY_SIDECARS {
            let _ = fs::remove_file(sidecar_path(work_dir, name));
        }
        Ok(())
    }
}

/// Parse the multi-entry pair format. Stops at the first malformed pair,
/// keeping the entries parsed so far.
fn parse_pairs(path: &Path) -> Option<BTreeMap<PathBuf, String>> {
    let text = fs::read_to_string(path).ok()?;
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let mut entries = BTreeMap::new();
    while let Some(hash) = lines.next() {
        if !is_hash(hash) {
            break;
        }
        let Some(path) = lines.next() else {
            break;
        };
        entries.insert(PathBuf::from(path), hash.to_string());
    }
    Some(entries)
}

/// Parse a legacy single-entry sidecar (`hash` line, optional `path` line).
fn parse_single(path: &Path) -> Option<SyncPoint> {
    let text = fs::read_to_string(path).ok()?;
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let hash = lines.next()?;
    if !is_hash(hash) {
        return None;
    }
    let path = lines.next().map(PathBuf::from);
    Some(SyncPoint {
        hash: hash.to_string(),
        path,
    })
}

/// A legacy sidecar record: hash plus, when known, the file it refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncPoint {
    pub hash: String,
    pub path: Option<PathBuf>,
}

/// Verdict for "may this `build` overwrite an existing output file?"
///
/// The overwrite guard requires positive evidence that the bytes about to
/// be clobbered are accounted for: the file's current hash (computed live)
/// must match the value recorded when this project last legitimately wrote
/// it. Anything else refuses, per the skill protocol: **a build must never
/// silently destroy a file it cannot vouch for.**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputGuard {
    /// The file's current hash matches its record — safe to overwrite, and
    /// this build refreshes the record on completion.
    InSync,
    /// The file changed after its last recorded write: external edits not
    /// present in the PPTD would be destroyed.
    Stale,
    /// No record covers this file (never written by this project, or the
    /// sidecar is missing). Its provenance is unknown, so overwriting is
    /// refused until `convert` establishes a baseline.
    Uncovered,
}

/// Classify an output file against the work directory's records. Callers
/// should skip the guard entirely when the output does not exist.
pub fn classify_output(output: &Path, work_dir: &Path) -> Result<OutputGuard> {
    match Records::load(work_dir).get(output) {
        Some(recorded) => {
            if *recorded == sha256_of(output)? {
                Ok(OutputGuard::InSync)
            } else {
                Ok(OutputGuard::Stale)
            }
        }
        None => Ok(OutputGuard::Uncovered),
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
    fn records_roundtrip_and_guard() {
        let dir = std::env::temp_dir().join(format!("sf-hash-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.pptx");
        fs::write(&a, b"hello").unwrap();

        // Nothing recorded yet → uncovered.
        let records = Records::load(&dir);
        assert!(records.get(&a).is_none());
        assert_eq!(classify_output(&a, &dir).unwrap(), OutputGuard::Uncovered);

        // Record a, save, reload → InSync.
        let h = sha256_of(&a).unwrap();
        assert_eq!(h.len(), 64);
        let mut records = Records::load(&dir);
        records.set(&a, &h);
        records.save(&dir).unwrap();
        assert_eq!(classify_output(&a, &dir).unwrap(), OutputGuard::InSync);

        // Content changed → Stale (hash computed live vs recorded).
        fs::write(&a, b"world").unwrap();
        assert_eq!(classify_output(&a, &dir).unwrap(), OutputGuard::Stale);

        // Same content but a different file → still uncovered: records are
        // per file, not per content.
        let b = dir.join("b.pptx");
        fs::write(&b, b"world").unwrap();
        assert_eq!(classify_output(&b, &dir).unwrap(), OutputGuard::Uncovered);

        // Multiple outputs coexist in one map.
        let mut records = Records::load(&dir);
        records.set(&b, &sha256_of(&b).unwrap());
        records.save(&dir).unwrap();
        assert_eq!(classify_output(&a, &dir).unwrap(), OutputGuard::Stale);
        assert_eq!(classify_output(&b, &dir).unwrap(), OutputGuard::InSync);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_sidecars_are_absorbed_and_removed() {
        let dir = std::env::temp_dir().join(format!("sf-hash-legacy-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.pptx");
        fs::write(&a, b"hello").unwrap();
        let h = sha256_of(&a).unwrap();

        // Legacy `.src.hash` with a path line feeds the map on load.
        fs::write(dir.join(".src.hash"), format!("{h}\n{}\n", a.display())).unwrap();
        fs::write(dir.join(".build.hash"), format!("{h}\n{}\n", a.display())).unwrap();
        let records = Records::load(&dir);
        assert!(records.has_legacy());
        assert_eq!(records.get(&a), Some(&h));
        assert_eq!(classify_output(&a, &dir).unwrap(), OutputGuard::InSync);
        records.save(&dir).unwrap();
        assert!(!dir.join(".src.hash").exists());
        assert!(!dir.join(".build.hash").exists());
        assert!(dir.join(".sync.hash").exists());

        // Legacy path-less `.src.hash` exposes its hash for re-binding.
        let dir2 = std::env::temp_dir().join(format!("sf-hash-legacy2-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir2);
        fs::create_dir_all(&dir2).unwrap();
        fs::write(dir2.join(".src.hash"), format!("{h}\n")).unwrap();
        let records = Records::load(&dir2);
        assert_eq!(records.legacy_hash(), Some(&h));
        assert!(records.get(&a).is_none());

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&dir2);
    }
}
