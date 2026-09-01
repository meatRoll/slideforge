//! End-to-end tests for the build overwrite guard.
//!
//! Protocol under test: `build` may overwrite an existing output file only
//! when its current hash (computed live) matches the value recorded in
//! `.sync.hash` when this project last legitimately wrote it. Everything
//! else must refuse with exit code 2 and leave the file untouched.

use std::fs;
use std::path::{Path, PathBuf};

use clap::Parser as _;
use slideforge::cli::Cli;
use slideforge::hash;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/buildable");

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sf-guard-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn copy_fixture(dest: &Path) {
    fn rec(src: &Path, dest: &Path) {
        fs::create_dir_all(dest).unwrap();
        for entry in fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            let target = dest.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                rec(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    rec(Path::new(FIXTURE), dest);
}

fn run(args: &[&str]) -> i32 {
    Cli::parse_from(args).run()
}

/// Rewrite the archive with one extra part added: still a valid OPC
/// package that `convert` can absorb, but with different bytes (an
/// "external edit").
fn mutate_zip(path: &Path) {
    use std::io::{Read as _, Write as _};

    let file = fs::File::open(path).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buf);
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).unwrap();
            let mut content = Vec::new();
            entry.read_to_end(&mut content).unwrap();
            writer
                .start_file(
                    entry.name().to_string(),
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
            writer.write_all(&content).unwrap();
        }
        writer
            .start_file(
                "docProps/external-edit.marker",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        writer.write_all(b"external edit").unwrap();
        writer.finish().unwrap();
    }
    fs::write(path, buf.into_inner()).unwrap();
}

/// The `.sync.hash` entry for `path`, if present.
fn recorded_hash(work: &Path, path: &Path) -> Option<String> {
    let text = fs::read_to_string(work.join(".sync.hash")).ok()?;
    let key = path.canonicalize().unwrap();
    let mut lines = text.lines();
    while let (Some(hash), Some(line_path)) = (lines.next(), lines.next()) {
        if Path::new(line_path) == key {
            return Some(hash.to_string());
        }
    }
    None
}

fn build(work: &Path, entry: &str, out: &Path) -> i32 {
    run(&[
        "slideforge",
        "build",
        &work.join(entry).to_string_lossy(),
        "--output",
        &out.to_string_lossy(),
    ])
}

fn convert(input: &Path, out_dir: &Path) -> i32 {
    run(&[
        "slideforge",
        "convert",
        &input.to_string_lossy(),
        &out_dir.to_string_lossy(),
    ])
}

/// Fresh build writes the file and records its hash; rebuilding over the
/// same output stays authorized (the A/C-mode iterate loop).
#[test]
fn fresh_build_then_iterate_is_allowed() {
    let work = temp_dir("iterate");
    copy_fixture(&work);
    let out = work.join("out.pptx");

    assert_eq!(build(&work, "buildable.pptd", &out), 0);
    assert!(out.is_file());
    assert_eq!(
        recorded_hash(&work, &out).as_deref(),
        Some(hash::sha256_of(&out).unwrap().as_str())
    );

    // Second build over the same output: covered by the record.
    assert_eq!(build(&work, "buildable.pptd", &out), 0);
}

/// Output exists but nothing covers it → refuse, file untouched.
#[test]
fn refuses_uncovered_existing_output() {
    let work = temp_dir("uncovered");
    copy_fixture(&work);
    let out = work.join("out.pptx");
    fs::write(&out, b"precious unknown bytes").unwrap();

    assert_eq!(build(&work, "buildable.pptd", &out), 2);
    assert_eq!(fs::read(&out).unwrap(), b"precious unknown bytes");
}

/// External edit after a build → refuse.
#[test]
fn refuses_output_changed_after_build() {
    let work = temp_dir("build-stale");
    copy_fixture(&work);
    let out = work.join("out.pptx");

    assert_eq!(build(&work, "buildable.pptd", &out), 0);
    let before = fs::read(&out).unwrap();
    let recorded_before = recorded_hash(&work, &out).unwrap();
    mutate_zip(&out);

    assert_eq!(build(&work, "buildable.pptd", &out), 2);
    assert_ne!(
        fs::read(&out).unwrap(),
        before,
        "external edit must survive"
    );
    assert_eq!(
        recorded_hash(&work, &out).as_deref(),
        Some(recorded_before.as_str()),
        "refused build must not touch the record either"
    );
}

/// External edit after convert → refuse; convert absorbs and re-arms.
#[test]
fn convert_source_flow_stale_and_rearm() {
    let work = temp_dir("source");
    copy_fixture(&work);
    let out = work.join("out.pptx");
    let pptd = work.join("pptd");

    assert_eq!(build(&work, "buildable.pptd", &out), 0);
    assert_eq!(convert(&out, &pptd), 0);
    assert_eq!(
        recorded_hash(&pptd, &out).as_deref(),
        Some(hash::sha256_of(&out).unwrap().as_str())
    );

    // External edit after the convert sync point → refuse.
    mutate_zip(&out);
    assert_eq!(build(&pptd, "deck.pptd", &out), 2);

    // convert absorbs the external edit and refreshes the record…
    assert_eq!(convert(&out, &pptd), 0);
    // …then an overwriting build succeeds and records itself again.
    assert_eq!(build(&pptd, "deck.pptd", &out), 0);
    assert_eq!(
        recorded_hash(&pptd, &out).as_deref(),
        Some(hash::sha256_of(&out).unwrap().as_str())
    );
}

/// Legacy sidecars (old `.src.hash`, hash-only or with path) are migrated
/// into `.sync.hash`, after which the guard is fully armed again.
#[test]
fn legacy_sidecars_migrate_and_guard_rearms() {
    let work = temp_dir("legacy");
    copy_fixture(&work);
    let out = work.join("out.pptx");
    let pptd = work.join("pptd");

    assert_eq!(build(&work, "buildable.pptd", &out), 0);
    assert_eq!(convert(&out, &pptd), 0);

    // Degrade to the old world: hash-only `.src.hash`, no `.sync.hash`.
    let hash_line = recorded_hash(&pptd, &out).unwrap();
    fs::remove_file(pptd.join(".sync.hash")).unwrap();
    fs::write(pptd.join(".src.hash"), &hash_line).unwrap();

    // A skipped convert migrates the legacy record in place.
    assert_eq!(convert(&out, &pptd), 0);
    assert!(!pptd.join(".src.hash").exists(), "legacy sidecar removed");
    assert_eq!(
        recorded_hash(&pptd, &out).as_deref(),
        Some(hash_line.as_str()),
        "record re-bound to the same file and hash"
    );

    // Now the guard is fully armed: external edit → refuse.
    mutate_zip(&out);
    assert_eq!(build(&pptd, "deck.pptd", &out), 2);
}

/// One work_dir can serve several distinct outputs: each file's record is
/// independent (the old single-slot design clobbered them).
#[test]
fn multiple_outputs_are_tracked_independently() {
    let work = temp_dir("multi");
    copy_fixture(&work);
    let a = work.join("a.pptx");
    let b = work.join("b.pptx");

    assert_eq!(build(&work, "buildable.pptd", &a), 0);
    assert_eq!(build(&work, "buildable.pptd", &b), 0);

    // Iterating back over the FIRST output still works.
    assert_eq!(build(&work, "buildable.pptd", &a), 0);

    // And tampering with the SECOND one is caught.
    mutate_zip(&b);
    assert_eq!(build(&work, "buildable.pptd", &b), 2);
}
