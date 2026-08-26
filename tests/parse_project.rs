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
    // The custom Element serde bridge must keep the PPTD YAML shape intact.
    let project = pptd::load_project(Path::new(FIXTURE)).unwrap();
    let page = &project.pages[0];

    for element in &page.elements {
        let yaml = serde_yaml::to_string(element).expect("element must serialize");
        let back: pptd::Element = serde_yaml::from_str(&yaml).expect("element must deserialize");
        assert_eq!(element, &back, "round-trip mismatch for element");
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
                }),
            ]),
        ),
    ];

    for (yaml, expected) in cases {
        let parsed: BorderSpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(&parsed, expected, "BorderSpec parse of `{yaml}`");
    }
}
