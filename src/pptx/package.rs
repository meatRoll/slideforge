//! The OPC package model: content types and package entries.
//!
//! A `.pptx` is a ZIP archive following the OPC (Open Packaging
//! Conventions) rules, made of *parts*. Typed parts (slides, theme, ...)
//! are listed in `[Content_Types].xml`; the `.rels` relationship files are
//! exempt. See `docs/pptx-layout-synthesis.md` for the synthesized part set.

/// A content type handled by the writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    Presentation,
    Slide,
    SlideMaster,
    SlideLayout,
    Theme,
    CoreProperties,
    ImagePng,
    ImageJpeg,
}

impl ContentType {
    /// The `Content-Type` header used in `[Content_Types].xml`.
    pub fn mime(&self) -> &'static str {
        match self {
            ContentType::Presentation => {
                "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"
            }
            ContentType::Slide => {
                "application/vnd.openxmlformats-officedocument.presentationml.slide+xml"
            }
            ContentType::SlideMaster => {
                "application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"
            }
            ContentType::SlideLayout => {
                "application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"
            }
            ContentType::Theme => "application/vnd.openxmlformats-officedocument.theme+xml",
            ContentType::CoreProperties => {
                "application/vnd.openxmlformats-package.core-properties+xml"
            }
            ContentType::ImagePng => "image/png",
            ContentType::ImageJpeg => "image/jpeg",
        }
    }
}

/// One file inside the OPC package.
///
/// `content_type == None` for parts that are exempt from `[Content_Types].xml`
/// (the `.rels` relationship files and `[Content_Types].xml` itself).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageEntry {
    /// Part name (path) as written into the ZIP archive.
    pub path: String,
    pub content_type: Option<ContentType>,
    pub data: Vec<u8>,
}

impl PackageEntry {
    /// A part that is declared in `[Content_Types].xml` (slides, theme, ...).
    /// `data` accepts either `String` (XML) or raw bytes.
    pub fn typed(
        path: impl Into<String>,
        content_type: ContentType,
        data: impl Into<Vec<u8>>,
    ) -> Self {
        Self {
            path: path.into(),
            content_type: Some(content_type),
            data: data.into(),
        }
    }

    /// A part that is exempt from the content-types registry (a `.rels` file
    /// or `[Content_Types].xml` itself).
    pub fn opaque(path: impl Into<String>, data: impl Into<Vec<u8>>) -> Self {
        Self {
            path: path.into(),
            content_type: None,
            data: data.into(),
        }
    }
}
