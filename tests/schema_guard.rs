//! Structural OOXML schema guards for the generated package.
//!
//! Every rule below corresponds to a regression that once produced a
//! "repair needed" file (see the commit that fixed the WPS anomaly):
//!
//! * `a:xfrm` — `rot` / `flipH` / `flipV` are **attributes** of the
//!   transform, never child elements;
//! * `a:ln` — `prstDash` is a **child element** that must come *after* the
//!   fill, never an attribute;
//! * text boxes carry an explicit `a:noFill`;
//! * `a:bgFillStyleLst` ships three background fills (PowerPoint habit);
//! * `p:sld` children follow the strict order `cSld → clrMapOvr → transition`;
//! * no legacy element names (`a:cTo`) survive.
//!
//! The scans parse every XML part with `xmltree`, so malformed XML also
//! fails here.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use slideforge::pptd;
use slideforge::pptx::writer::PptxWriter;
use xmltree::{Element, XMLNode};

const BUILDABLE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/buildable/buildable.pptd"
);

fn build_deck(tag: &str) -> PathBuf {
    let project = pptd::load_project(Path::new(BUILDABLE)).unwrap();
    let path = std::env::temp_dir().join(format!(
        "slideforge-schema-{tag}-{}.pptx",
        std::process::id()
    ));
    let _ = fs::remove_file(&path);
    PptxWriter::new(&project).build(&path).unwrap();
    path
}

/// Direct element children of `element` (skipping comments / CDATA).
fn children(element: &Element) -> Vec<&Element> {
    element
        .children
        .iter()
        .filter_map(|node| match node {
            XMLNode::Element(child) => Some(child),
            _ => None,
        })
        .collect()
}

/// Every XML part of the package: `(part name, parsed root element)`.
fn xml_parts(path: &Path) -> Vec<(String, Element)> {
    let file = fs::File::open(path).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();
    let mut parts = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).unwrap();
        if entry.name().ends_with(".xml") || entry.name().ends_with(".rels") {
            let mut data = Vec::new();
            entry.read_to_end(&mut data).unwrap();
            let root = Element::parse(data.as_slice())
                .unwrap_or_else(|err| panic!("{} is not well-formed XML: {err}", entry.name()));
            parts.push((entry.name().to_owned(), root));
        }
    }
    parts
}

/// Depth-first walk over every element.
fn walk<'a>(element: &'a Element, out: &mut Vec<&'a Element>) {
    out.push(element);
    for child in children(element) {
        walk(child, out);
    }
}

#[test]
fn xfrm_transform_flags_are_attributes() {
    let path = build_deck("xfrm");
    for (part, root) in xml_parts(&path) {
        let mut all = Vec::new();
        walk(&root, &mut all);
        for element in all {
            if element.name != "a:xfrm" {
                continue;
            }
            for forbidden in ["a:rot", "a:flipH", "a:flipV"] {
                assert!(
                    children(element).iter().all(|c| c.name != forbidden),
                    "{part}: <{forbidden}> must be an attribute of <a:xfrm>, not a child"
                );
            }
        }
    }
}

#[test]
fn line_dash_is_a_child_element_after_the_fill() {
    let path = build_deck("ln");
    for (part, root) in xml_parts(&path) {
        let mut all = Vec::new();
        walk(&root, &mut all);
        for element in all {
            if element.name != "a:ln" {
                continue;
            }
            assert!(
                !element.attributes.contains_key("prstDash"),
                "{part}: a:prstDash must not be an attribute of <a:ln>"
            );
            let children = children(element);
            if let Some(dash) = children.iter().position(|c| c.name == "a:prstDash") {
                let fill = children.iter().position(|c| {
                    matches!(
                        c.name.as_str(),
                        "a:solidFill" | "a:noFill" | "a:gradFill" | "a:blipFill" | "a:pattFill"
                    )
                });
                if let Some(fill) = fill {
                    assert!(
                        dash > fill,
                        "{part}: <a:prstDash> must come after the fill inside <a:ln>"
                    );
                }
            }
        }
    }
}

#[test]
fn text_boxes_explicitly_disable_fill() {
    let path = build_deck("text");
    for (part, root) in xml_parts(&path) {
        let mut all = Vec::new();
        walk(&root, &mut all);
        for element in all {
            if element.name != "p:sp" {
                continue;
            }
            let sp_children = children(element);
            let is_text_box = sp_children
                .iter()
                .find(|c| c.name == "p:nvSpPr")
                .map(|nv| {
                    let nv_children = children(nv);
                    nv_children
                        .iter()
                        .find(|c| c.name == "p:cNvSpPr")
                        .and_then(|c| c.attributes.get("txBox"))
                        .is_some_and(|v| v == "1")
                })
                .unwrap_or(false);
            if !is_text_box {
                continue;
            }
            let sp_pr = sp_children
                .iter()
                .find(|c| c.name == "p:spPr")
                .unwrap_or_else(|| panic!("{part}: text box p:sp without p:spPr"));
            assert!(
                children(sp_pr).iter().any(|c| c.name == "a:noFill"),
                "{part}: text box must carry <a:noFill> (automatic fill would paint the box)"
            );
        }
    }
}

#[test]
fn background_style_list_has_three_entries() {
    let path = build_deck("bgfill");
    for (part, root) in xml_parts(&path) {
        if part != "ppt/theme/theme1.xml" {
            continue;
        }
        let mut all = Vec::new();
        walk(&root, &mut all);
        for element in all {
            if element.name == "a:bgFillStyleLst" {
                assert!(
                    children(element).len() >= 3,
                    "{part}: a:bgFillStyleLst should ship three background fills"
                );
            }
        }
    }
}

#[test]
fn slide_children_follow_the_schema_order() {
    let path = build_deck("sld");
    for (part, root) in xml_parts(&path) {
        if root.name != "p:sld" {
            continue;
        }
        let order = ["p:cSld", "p:clrMapOvr", "p:transition"];
        let positions: Vec<usize> = order
            .iter()
            .map(|name| {
                children(&root)
                    .iter()
                    .position(|c| c.name == *name)
                    .unwrap_or_else(|| panic!("{part}: <p:sld> is missing <{name}>"))
            })
            .collect();
        let mut sorted = positions.clone();
        sorted.sort_unstable();
        assert_eq!(
            positions, sorted,
            "{part}: <p:sld> children must be ordered cSld → clrMapOvr → transition"
        );
    }
}

#[test]
fn no_legacy_element_names_survive() {
    let path = build_deck("legacy");
    for (part, root) in xml_parts(&path) {
        let mut all = Vec::new();
        walk(&root, &mut all);
        for element in all {
            assert_ne!(
                element.name, "a:cTo",
                "{part}: legacy <a:cTo> element (should be <a:cubicBezTo>)"
            );
        }
    }
}
