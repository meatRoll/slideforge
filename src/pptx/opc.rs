//! OPC (Open Packaging Conventions) helpers: namespaces, relationship types,
//! and the builders for `[Content_Types].xml` and `.rels` files.

use super::package::PackageEntry;
use super::xml::Xml;

/// XML namespaces used by the generated parts.
pub mod ns {
    pub const PRESENTATIONML: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
    pub const DRAWINGML: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
    pub const RELATIONSHIPS: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    pub const CONTENT_TYPES: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
    pub const PACKAGE_REL: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
    pub const CORE_PROPS: &str =
        "http://schemas.openxmlformats.org/package/2006/metadata/core-properties";
    pub const DC: &str = "http://purl.org/dc/elements/1.1/";
    pub const DCTERMS: &str = "http://purl.org/dc/terms/";
    pub const DCMITYPE: &str = "http://purl.org/dc/dcmitype/";
    pub const XSI: &str = "http://www.w3.org/2001/XMLSchema-instance";
}

/// Relationship type URIs used by the writer.
pub mod rel_kind {
    pub const OFFICE_DOCUMENT: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
    pub const CORE_PROPERTIES: &str =
        "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties";
    pub const SLIDE: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide";
    pub const SLIDE_LAYOUT: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout";
    pub const SLIDE_MASTER: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster";
    pub const THEME: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme";
}

/// One relationship inside a `.rels` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rel {
    pub id: String,
    pub kind: &'static str,
    pub target: String,
}

impl Rel {
    pub fn new(id: &str, kind: &'static str, target: impl Into<String>) -> Self {
        Self {
            id: id.to_owned(),
            kind,
            target: target.into(),
        }
    }
}

/// Render a `.rels` file.
pub fn rels_xml(rels: &[Rel]) -> String {
    let mut xml = Xml::new();
    xml.start("Relationships", &[("xmlns", ns::PACKAGE_REL)]);
    for rel in rels {
        xml.leaf(
            "Relationship",
            &[("Id", &rel.id), ("Type", rel.kind), ("Target", &rel.target)],
        );
    }
    xml.end("Relationships");
    xml.into_string()
}

/// Render `[Content_Types].xml` from the package entries. Adopts the OPC
/// rule: a `Default` for `xml` / `rels` extensions, and an `Override` for
/// every typed part.
pub fn content_types_xml(entries: &[PackageEntry]) -> String {
    let mut xml = Xml::new();
    xml.start("Types", &[("xmlns", ns::CONTENT_TYPES)]);
    xml.leaf(
        "Default",
        &[
            ("Extension", "rels"),
            (
                "ContentType",
                "application/vnd.openxmlformats-package.relationships+xml",
            ),
        ],
    );
    xml.leaf(
        "Default",
        &[("Extension", "xml"), ("ContentType", "application/xml")],
    );
    for entry in entries {
        if let Some(content_type) = &entry.content_type {
            xml.leaf(
                "Override",
                &[
                    ("PartName", &format!("/{}", entry.path)),
                    ("ContentType", content_type.mime()),
                ],
            );
        }
    }
    xml.end("Types");
    xml.into_string()
}
