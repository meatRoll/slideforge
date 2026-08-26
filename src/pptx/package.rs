//! The OPC package model: every part a `.pptx` contains.
//!
//! A minimal editable PPTX produced from PPTD needs roughly:
//!
//! ```text
//! [Content_Types].xml            — content-type registry
//! _rels/.rels                    — package-level relationships
//! docProps/core.xml              — title, dates, ...
//! ppt/presentation.xml           — slide list + slide size + transitions
//! ppt/_rels/presentation.xml.rels— presentation → slides/masters/theme
//! ppt/presProps.xml, ppt/viewProps.xml, ppt/tableStyles.xml
//! ppt/theme/theme1.xml           — colors/fonts/effects (from PPTD theme)
//! ppt/slideMasters/slideMaster1.xml, ppt/slideLayouts/slideLayout1.xml
//! ppt/slides/slideN.xml          — one per page (sld > cSld > spTree)
//! ppt/slides/_rels/slideN.xml.rels
//! ppt/media/...                  — images/fonts
//! ```
//!
//! The writer is not implemented yet; this module fixes the vocabulary
//! (part names, content types) so the renderer can be built against it.

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

/// Canonical part paths inside the OPC package (indexes are 1-based).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    ContentTypes,
    RootRelationships,
    CoreProperties,
    Presentation,
    PresentationRelationships,
    Theme { index: usize },
    SlideMaster { index: usize },
    SlideLayout { index: usize },
    Slide { index: usize },
}

impl Part {
    /// The part name (path) as written into the ZIP archive.
    pub fn path(&self) -> String {
        match self {
            Part::ContentTypes => "[Content_Types].xml".to_owned(),
            Part::RootRelationships => "_rels/.rels".to_owned(),
            Part::CoreProperties => "docProps/core.xml".to_owned(),
            Part::Presentation => "ppt/presentation.xml".to_owned(),
            Part::PresentationRelationships => "ppt/_rels/presentation.xml.rels".to_owned(),
            Part::Theme { index } => format!("ppt/theme/theme{index}.xml"),
            Part::SlideMaster { index } => format!("ppt/slideMasters/slideMaster{index}.xml"),
            Part::SlideLayout { index } => format!("ppt/slideLayouts/slideLayout{index}.xml"),
            Part::Slide { index } => format!("ppt/slides/slide{index}.xml"),
        }
    }

    /// The content type this part declares in `[Content_Types].xml`.
    pub fn content_type(&self) -> ContentType {
        match self {
            Part::ContentTypes => unreachable!("the content-types part is not typed in itself"),
            Part::RootRelationships => unreachable!("_rels/.rels carries no content type"),
            Part::CoreProperties => ContentType::CoreProperties,
            Part::Presentation | Part::PresentationRelationships => ContentType::Presentation,
            Part::Slide { .. } => ContentType::Slide,
            Part::SlideMaster { .. } => ContentType::SlideMaster,
            Part::SlideLayout { .. } => ContentType::SlideLayout,
            Part::Theme { .. } => ContentType::Theme,
        }
    }
}
