//! Integration tests: loading the demo PPTD project from disk.

use std::path::Path;

use slideforge::pptd;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/demo/demo.pptd");

#[test]
fn loads_demo_project() {
    let project = pptd::load_project(Path::new(FIXTURE)).expect("fixture must parse");
    assert_eq!(project.presentation.version, "v2");
    assert_eq!(
        project.presentation.title.as_deref(),
        Some("SlideForge Demo")
    );
    assert_eq!(project.presentation.size.width, 960.0);
    assert_eq!(project.presentation.size.height, 540.0);
    assert_eq!(project.pages.len(), 2);
    assert_eq!(project.page_paths.len(), 2);
}

#[test]
fn parses_element_kinds() {
    let project = pptd::load_project(Path::new(FIXTURE)).unwrap();

    let cover = &project.pages[0];
    assert_eq!(cover.page_type.as_deref(), Some("cover"));
    assert_eq!(
        cover.elements.len(),
        4,
        "cover elements: 2 texts + shape + line"
    );
    assert!(matches!(cover.elements[0], pptd::Element::Text(_)));
    assert!(matches!(cover.elements[2], pptd::Element::Shape(_)));
    assert!(matches!(cover.elements[3], pptd::Element::Line(_)));
    assert!(cover.background.is_some());

    let content = &project.pages[1];
    assert_eq!(content.page_type.as_deref(), Some("content"));
    assert!(matches!(content.elements[0], pptd::Element::Text(_)));
    assert!(matches!(content.elements[1], pptd::Element::Table(_)));
    assert!(matches!(content.elements[2], pptd::Element::Chart(_)));
}

#[test]
fn parses_fill_variants() {
    use slideforge::pptd::Fill;

    let project = pptd::load_project(Path::new(FIXTURE)).unwrap();

    // Cover background is a solid fill with a `$primary` theme reference.
    let background = project.pages[0].background.as_ref().unwrap();
    let Fill::Solid { color } = background else {
        panic!("expected a solid fill, got {background:?}");
    };
    assert!(color.is_theme_ref());
    assert_eq!(color.theme_key(), Some("primary"));

    // Shape fill with a theme reference.
    let badge = match &project.pages[0].elements[2] {
        pptd::Element::Shape(shape) => shape,
        other => panic!("expected a shape, got {other:?}"),
    };
    assert_eq!(badge.shape_name, "roundRect");
    assert!(badge.fill.is_some());
}

#[test]
fn round_trips_through_yaml() {
    // The derived internally-tagged serde impls must keep the PPTD YAML
    // shape intact, for every element kind (incl. the boxed chart).
    let project = pptd::load_project(Path::new(FIXTURE)).unwrap();

    for (page_idx, page) in project.pages.iter().enumerate() {
        for element in &page.elements {
            let yaml = serde_yaml::to_string(element).expect("element must serialize");
            let back: pptd::Element =
                serde_yaml::from_str(&yaml).expect("element must deserialize");
            assert_eq!(
                element,
                &back,
                "round-trip mismatch on page {} for element",
                page_idx + 1
            );
        }
    }
}

#[test]
fn parses_border_spec_shapes() {
    use slideforge::pptd::{Border, BorderSpec, LineStyle};

    let cases: &[(&str, BorderSpec)] = &[
        ("null", BorderSpec::Clear),
        (
            "{style: dash}",
            BorderSpec::Uniform(Border {
                style: Some(LineStyle::Dash),
                width: None,
                color: None,
                gradient: None,
            }),
        ),
        (
            "[null, {style: solid}]",
            BorderSpec::VerticalHorizontal([
                None,
                Some(Border {
                    style: Some(LineStyle::Solid),
                    width: None,
                    color: None,
                    gradient: None,
                }),
            ]),
        ),
    ];

    for (yaml, expected) in cases {
        let parsed: BorderSpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(&parsed, expected, "BorderSpec parse of `{yaml}`");
    }
}

// --- SlideForge layout extension (P1: data model) ---------------------------

#[test]
fn parses_layout_extension_fields() {
    use slideforge::pptd::{Fill, Presentation};
    let deck_yaml = "\
version: v2
size: [960, 540]
pages: []
layouts:
  cover:
    background: {type: solid, color: \"#0070C0\"}
    elements:
      - {elementId: bar, elementType: shape, bounds: [0, 520, 960, 20],
         shapeName: rect, fill: {type: solid, color: \"#0070C0\"}}
    placeholders:
      title:
        bounds: [60, 40, 600, 60]
        style: \"$title\"
        fontSize: 40
        bold: true
";
    let pres: Presentation = serde_yaml::from_str(deck_yaml).expect("deck must parse");
    let layouts = pres.layouts.expect("layouts present");
    let cover = layouts.get("cover").expect("cover layout");
    match cover.background.as_ref().unwrap() {
        Fill::Solid { color } => assert_eq!(color.0, "#0070C0"),
        other => panic!("expected solid bg, got {other:?}"),
    }
    assert_eq!(cover.elements.len(), 1, "deco element");
    let title_ph = cover.placeholders.get("title").expect("title placeholder");
    assert_eq!(title_ph.bounds.x, 60.0);
    assert_eq!(title_ph.bounds.width, 600.0);
    assert_eq!(title_ph.style.as_deref(), Some("$title"));
    assert_eq!(title_ph.font_size, Some(40.0));
    assert_eq!(title_ph.bold, Some(true));
    assert!(title_ph.italic.is_none(), "italic unset");
}

#[test]
fn page_layout_and_text_placeholder_parse() {
    use slideforge::pptd::{Element, Page};
    let page_yaml = "\
layout: cover
background: {type: solid, color: \"#FFFFFF\"}
elements:
  - elementId: t1
    elementType: text
    bounds: [60, 40, 600, 60]
    placeholder: title
    content: {text: Hello}
";
    let page: Page = serde_yaml::from_str(page_yaml).expect("page must parse");
    assert_eq!(page.layout.as_deref(), Some("cover"));
    match &page.elements[0] {
        Element::Text(t) => assert_eq!(t.placeholder.as_deref(), Some("title")),
        other => panic!("expected text, got {other:?}"),
    }
}

#[test]
fn flags_dangling_layout_reference() {
    use slideforge::pptd::{Page, Presentation, Project, validate_project};
    use std::path::PathBuf;
    let presentation: Presentation = serde_yaml::from_str(
        "version: v2\nsize: [960, 540]\npages: [pages/1.page]\nlayouts:\n  cover: {}\n",
    )
    .unwrap();
    let page: Page = serde_yaml::from_str("layout: nope\nelements: []\n").unwrap();
    let project = Project {
        root_dir: PathBuf::from("."),
        presentation,
        page_paths: vec![PathBuf::from("pages/1.page")],
        pages: vec![page],
    };
    let diags = validate_project(&project);
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("not defined in presentation.layouts")),
        "expected dangling-layout diagnostic, got {diags:?}"
    );
}

#[test]
fn accepts_valid_layout_reference() {
    use slideforge::pptd::{Page, Presentation, Project, validate_project};
    use std::path::PathBuf;
    let presentation: Presentation = serde_yaml::from_str(
        "version: v2\nsize: [960, 540]\npages: [pages/1.page]\nlayouts:\n  cover: {}\n",
    )
    .unwrap();
    let page: Page = serde_yaml::from_str("layout: cover\nelements: []\n").unwrap();
    let project = Project {
        root_dir: PathBuf::from("."),
        presentation,
        page_paths: vec![PathBuf::from("pages/1.page")],
        pages: vec![page],
    };
    let diags = validate_project(&project);
    assert!(
        diags.is_empty(),
        "valid layout ref must not warn, got {diags:?}"
    );
}
