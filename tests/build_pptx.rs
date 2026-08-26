//! End-to-end tests: `PptxWriter` produces a valid OPC/ZIP package.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use slideforge::pptd;
use slideforge::pptx::writer::PptxWriter;

const BUILDABLE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/buildable/buildable.pptd"
);
const DEMO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/demo/demo.pptd");

fn out_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "slideforge-build-{tag}-{}.pptx",
        std::process::id()
    ))
}

const REQUIRED_PARTS: [&str; 11] = [
    "[Content_Types].xml",
    "_rels/.rels",
    "docProps/core.xml",
    "ppt/presentation.xml",
    "ppt/_rels/presentation.xml.rels",
    "ppt/theme/theme1.xml",
    "ppt/slideMasters/slideMaster1.xml",
    "ppt/slideLayouts/slideLayout1.xml",
    "ppt/slides/slide1.xml",
    "ppt/slides/_rels/slide1.xml.rels",
    "ppt/media/image1.png",
];

fn read_zip_part(path: &Path, name: &str) -> String {
    let file = fs::File::open(path).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();
    let mut data = String::new();
    zip.by_name(name)
        .unwrap()
        .read_to_string(&mut data)
        .unwrap();
    data
}

#[test]
fn buildable_deck_produces_a_complete_package() {
    let project = pptd::load_project(Path::new(BUILDABLE)).unwrap();
    let path = out_path("ok");
    let _ = fs::remove_file(&path);

    PptxWriter::new(&project)
        .build(&path)
        .expect("buildable deck must build");

    let file = fs::File::open(&path).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();
    let names: Vec<String> = (0..zip.len())
        .map(|i| zip.by_index(i).unwrap().name().to_owned())
        .collect();
    for expected in REQUIRED_PARTS {
        assert!(
            names.iter().any(|n| n == expected),
            "missing part {expected}; got {names:?}"
        );
    }

    // Slide content sanity: text, shape, line, icon, image and the fade.
    let slide = read_zip_part(&path, "ppt/slides/slide1.xml");
    assert!(slide.contains("Hello SlideForge"), "slide text missing");
    assert!(slide.contains("roundRect"), "shape missing");
    assert!(slide.contains("cxnSp"), "line missing");
    assert!(slide.contains("cubicBezTo"), "icon geometry missing");
    assert!(slide.contains("<pptd:icon"), "icon pptd extension missing");
    assert!(slide.contains("<p:pic>"), "image missing");
    assert!(
        slide.contains("r:embed=\"rId2\""),
        "image blip embed missing"
    );
    assert!(
        slide.contains("<a:srcRect"),
        "contain fit should pad via srcRect"
    );
    assert!(slide.contains("<p:transition"), "transition missing");
    assert!(slide.contains("<p:fade/>"), "fade transition missing");

    // Theme carries the resolved color slots.
    let theme = read_zip_part(&path, "ppt/theme/theme1.xml");
    assert!(theme.contains("2563EB"), "theme primary color missing");

    // Media relationship: rId1 = layout, rId2 = the png image.
    let slide_rels = read_zip_part(&path, "ppt/slides/_rels/slide1.xml.rels");
    assert!(
        slide_rels.contains("relationships/image"),
        "media rel missing"
    );
}

#[test]
fn content_types_cover_every_typed_part() {
    let project = pptd::load_project(Path::new(BUILDABLE)).unwrap();
    let path = out_path("ct");
    let _ = fs::remove_file(&path);
    PptxWriter::new(&project).build(&path).unwrap();

    let content_types = read_zip_part(&path, "[Content_Types].xml");
    assert!(
        content_types.contains("Extension=\"png\"") && content_types.contains("image/png"),
        "png Default content type missing"
    );
    for (part, mime) in [
        (
            "/ppt/presentation.xml",
            "presentationml.presentation.main+xml",
        ),
        ("/ppt/slides/slide1.xml", "presentationml.slide+xml"),
        ("/ppt/theme/theme1.xml", "officedocument.theme+xml"),
        (
            "/ppt/slideMasters/slideMaster1.xml",
            "presentationml.slideMaster+xml",
        ),
        (
            "/ppt/slideLayouts/slideLayout1.xml",
            "presentationml.slideLayout+xml",
        ),
        ("/docProps/core.xml", "core-properties+xml"),
    ] {
        assert!(
            content_types.contains(part) && content_types.contains(mime),
            "content type for {part} missing or wrong"
        );
    }
}

#[test]
fn builds_fail_with_a_clear_error_on_unsupported_elements() {
    // The demo deck's first slide uses rich text (`<p><span ...>`), which
    // the writer flags first; the error must name element + page.
    let project = pptd::load_project(Path::new(DEMO)).unwrap();
    let path = out_path("rich-text");
    let _ = fs::remove_file(&path);

    let message = PptxWriter::new(&project)
        .build(&path)
        .expect_err("rich text must fail the build")
        .to_string();
    assert!(message.contains("rich text"), "got: {message}");
    assert!(message.contains("`subtitle`"), "should name the element");
    assert!(message.contains("page 1"), "should name the page");
}

#[test]
fn unsupported_element_kinds_are_reported() {
    // A deck with a chart (plain text elsewhere) exercises the element-kind
    // guard rather than the rich-text guard.
    let deck = "version: v2\nsize: [960, 540]\npages:\n  - pages/p.page\n";
    let page = r#"
elements:
  - elementId: bench
    elementType: text
    bounds: [0, 0, 100, 50]
    content: {text: ok}
  - elementId: c
    elementType: chart
    bounds: [0, 60, 400, 300]
    data:
      cols: [a, b]
      rows:
        - [1, 2]
    series:
      - type: bar
        encode: {x: a, y: b}
"#;
    let dir = std::env::temp_dir().join(format!("slideforge-build-chart-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("pages")).unwrap();
    fs::write(dir.join("deck.pptd"), deck).unwrap();
    fs::write(dir.join("pages/p.page"), page).unwrap();

    let project = pptd::load_project(&dir.join("deck.pptd")).unwrap();
    let path = out_path("chart");
    let _ = fs::remove_file(&path);

    let message = PptxWriter::new(&project)
        .build(&path)
        .expect_err("chart must fail the build")
        .to_string();
    assert!(message.contains("not supported"), "got: {message}");
    assert!(message.contains("`c`"), "should name the chart element");
    assert!(message.contains("page 1"), "should name the page");
}
