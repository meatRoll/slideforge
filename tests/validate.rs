//! Integration tests: semantic validation of loaded PPTD projects.

use std::fs;
use std::path::Path;

use slideforge::pptd;
use slideforge::pptd::validate::validate_project;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/demo/demo.pptd");

/// Write a minimal deck into a unique temp directory and return the entry path.
fn tmp_deck(tag: &str, deck_yaml: &str, pages: &[(&str, &str)]) -> String {
    let dir = std::env::temp_dir().join(format!("slideforge-test-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("pages")).unwrap();
    fs::write(dir.join("deck.pptd"), deck_yaml).unwrap();
    for (name, body) in pages {
        fs::write(dir.join("pages").join(name), body).unwrap();
    }
    dir.join("deck.pptd").to_string_lossy().into_owned()
}

const MINIMAL_DECK: &str = "version: v2\nsize: [960, 540]\npages:\n  - pages/p.page\n";

#[test]
fn valid_fixture_has_no_issues() {
    let project = pptd::load_project(Path::new(FIXTURE)).unwrap();
    assert!(
        validate_project(&project).is_empty(),
        "demo fixture should be clean"
    );
}

#[test]
fn duplicate_element_id_is_reported() {
    let page = r#"
elements:
  - elementId: a
    elementType: text
    bounds: [0, 0, 100, 50]
    content: {text: first}
  - elementId: a
    elementType: text
    bounds: [0, 60, 100, 50]
    content: {text: second}
"#;
    let entry = tmp_deck("dup-id", MINIMAL_DECK, &[("p.page", page)]);
    let project = pptd::load_project(Path::new(&entry)).unwrap();
    let issues = validate_project(&project);
    assert!(
        issues
            .iter()
            .any(|d| d.message.contains("duplicate elementId")),
        "expected a duplicate-elementId finding, got {issues:?}"
    );
}

#[test]
fn non_positive_bounds_are_reported() {
    let page = r#"
elements:
  - elementId: bad
    elementType: text
    bounds: [0, 0, 0, 50]
    content: {text: nope}
"#;
    let entry = tmp_deck("zero-bounds", MINIMAL_DECK, &[("p.page", page)]);
    let project = pptd::load_project(Path::new(&entry)).unwrap();
    let issues = validate_project(&project);
    assert!(
        issues
            .iter()
            .any(|d| d.message.contains("positive width and height")),
        "expected a bounds finding, got {issues:?}"
    );
}

#[test]
fn missing_theme_reference_is_reported() {
    let page = r#"
elements:
  - elementId: t
    elementType: text
    bounds: [0, 0, 100, 50]
    content:
      style: "$nope"
      text: hi
"#;
    let entry = tmp_deck("missing-style", MINIMAL_DECK, &[("p.page", page)]);
    let project = pptd::load_project(Path::new(&entry)).unwrap();
    let issues = validate_project(&project);
    assert!(
        issues
            .iter()
            .any(|d| d.message.contains("theme.textStyles")),
        "expected a style-reference finding, got {issues:?}"
    );
}

#[test]
fn chart_row_length_mismatch_is_reported() {
    let page = r#"
elements:
  - elementId: c
    elementType: chart
    bounds: [0, 0, 400, 300]
    data:
      cols: [a, b]
      rows:
        - [1, 2]
        - [1, 2, 3]
    series:
      - type: bar
        encode: {x: a, y: b}
"#;
    let entry = tmp_deck("chart-rows", MINIMAL_DECK, &[("p.page", page)]);
    let project = pptd::load_project(Path::new(&entry)).unwrap();
    let issues = validate_project(&project);
    assert!(
        issues
            .iter()
            .any(|d| d.message.contains("row 1 has 3 cells, expected 2")),
        "expected a row-length finding, got {issues:?}"
    );
}

#[test]
fn unknown_chart_series_type_is_reported() {
    let page = r#"
elements:
  - elementId: c
    elementType: chart
    bounds: [0, 0, 400, 300]
    data:
      cols: [a, b]
      rows:
        - [1, 2]
    series:
      - type: radar
        encode: {category: a, y: b}
"#;
    let entry = tmp_deck("unknown-series", MINIMAL_DECK, &[("p.page", page)]);
    let result = pptd::load_project(Path::new(&entry));
    let message = result
        .expect_err("unknown series type must fail to parse")
        .to_string();
    assert!(
        message.contains("unknown variant `radar`"),
        "expected the serde unknown-variant finding, got: {message}"
    );
}

#[test]
fn table_ratio_sum_mismatch_is_reported() {
    let page = r#"
elements:
  - elementId: t
    elementType: table
    bounds: [0, 0, 400, 300]
    columnWidths: [0.5, 0.6]
    rowHeights: [1.0]
    rows:
      - - text: a
        - text: b
"#;
    let entry = tmp_deck("table-ratios", MINIMAL_DECK, &[("p.page", page)]);
    let project = pptd::load_project(Path::new(&entry)).unwrap();
    let issues = validate_project(&project);
    assert!(
        issues
            .iter()
            .any(|d| d.message.contains("columnWidths must sum to 1")),
        "expected a ratio-sum finding, got {issues:?}"
    );
}

#[test]
fn animation_must_reference_existing_element() {
    let page = r#"
elements:
  - elementId: t
    elementType: text
    bounds: [0, 0, 100, 50]
    content: {text: hi}
animations:
  - elementId: ghost
    effect: fade-in
"#;
    let entry = tmp_deck("animation-target", MINIMAL_DECK, &[("p.page", page)]);
    let project = pptd::load_project(Path::new(&entry)).unwrap();
    let issues = validate_project(&project);
    assert!(
        issues
            .iter()
            .any(|d| d.message.contains("references unknown elementId")),
        "expected an animation-target finding, got {issues:?}"
    );
}
