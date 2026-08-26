//! The OOXML / OPC output pipeline: PPTD AST → `.pptx` package.
//!
//! A `.pptx` is a ZIP archive following the OPC (Open Packaging
//! Conventions) rules. Module layout:
//!
//! * [`package`] — content types and package entries;
//! * [`opc`] — namespaces, relationship types, `[Content_Types].xml` / `.rels`;
//! * [`xml`] — a small indenting XML writer;
//! * [`theme`] — `Theme` → `theme1.xml` mapping and color resolution;
//! * [`render`] — element → drawingml rendering inside slides;
//! * [`writer`] — package assembly and ZIP output.

pub mod fa;
pub mod import;
pub mod media;
pub mod opc;
pub mod package;
pub mod render;
pub mod svg_path;
pub mod theme;
pub mod writer;
pub mod xml;
