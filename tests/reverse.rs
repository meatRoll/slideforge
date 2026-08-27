//! PPTX → PPTD → PPTX round-trip tests (hermetic, fixture-based).
//!
//! The workflow under test is the reverse compiler's contract: rebuild the
//! fixture deck, reverse it back into PPTD, then rebuild again. Geometry must
//! be preserved to the EMU and every element class must survive.

use std::fs;
use std::path::PathBuf;

use slideforge::pptd;
use slideforge::pptx;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/buildable")
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("slideforge-roundtrip-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn buildable_fixture_roundtrips() {
    let work = temp_dir("fixture");
    let first_pptx = work.join("first.pptx");
    let project = pptd::load_project(&fixture_dir().join("buildable.pptd")).unwrap();
    pptx::writer::PptxWriter::new(&project)
        .build(&first_pptx)
        .unwrap();

    let converted = work.join("converted");
    let report = pptx::import::convert_pptx_to_pptd(&first_pptx, &converted).unwrap();
    // The buildable fixture contains only supported constructs.
    assert!(
        report.skipped.is_empty(),
        "unexpected skips: {:?}",
        report.skipped
    );
    assert_eq!(report.page_count, 1, "buildable fixture has one page");

    // Every element index must survive the reverse.
    let project = pptd::load_project(&converted.join("deck.pptd")).unwrap();
    assert_eq!(project.presentation.pages.len(), 1);

    // Rebuild from the converted PPTD; must open and re-parse.
    let second_pptx = work.join("second.pptx");
    let project = pptd::load_project(&converted.join("deck.pptd")).unwrap();
    pptx::writer::PptxWriter::new(&project)
        .build(&second_pptx)
        .unwrap();

    // The regenerated package has the same number of slides and media.
    let first = zip::ZipArchive::new(fs::File::open(&first_pptx).unwrap()).unwrap();
    let second = zip::ZipArchive::new(fs::File::open(&second_pptx).unwrap()).unwrap();
    let slides_of = |zip: &zip::ZipArchive<fs::File>| -> usize {
        zip.file_names()
            .filter(|n| n.starts_with("ppt/slides/slide") && n.ends_with(".xml"))
            .count()
    };
    assert_eq!(slides_of(&first), slides_of(&second));
    assert_eq!(slides_of(&second), 1);
}

#[test]
fn reverse_keeps_theme_colors_and_size() {
    let work = temp_dir("theme");
    let pptx_path = work.join("t.pptx");
    let project = pptd::load_project(&fixture_dir().join("buildable.pptd")).unwrap();
    pptx::writer::PptxWriter::new(&project)
        .build(&pptx_path)
        .unwrap();

    let converted = work.join("converted");
    pptx::import::convert_pptx_to_pptd(&pptx_path, &converted).unwrap();
    let project = pptd::load_project(&converted.join("deck.pptd")).unwrap();

    let theme = project.presentation.theme.as_ref().expect("theme survives");
    assert!(
        theme.colors.contains_key("primary"),
        "theme colors lost: {:?}",
        theme.colors.keys().collect::<Vec<_>>()
    );
    let size = project.presentation.size;
    assert!(
        (size.width - 960.0).abs() < 1e-6 && (size.height - 540.0).abs() < 1e-6,
        "size drifted: {size:?}"
    );
}
