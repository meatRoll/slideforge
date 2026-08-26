//! AST → OOXML rendering (PPTD → presentationml / drawingml XML).
//!
//! This is the heart of the future `slideforge build` command. The public API
//! is implemented as a scaffold so the calling code is stable; the XML
//! generation itself is the next milestone.
//!
//! # Planned rendering pipeline
//!
//! 1. **Package shell.** Assemble an OPC package: `[Content_Types].xml`,
//!    `_rels/.rels`, `docProps/core.xml` (from `Presentation.title`).
//! 2. **Theme.** Map `Presentation.theme` onto `ppt/theme/theme1.xml`
//!    (clrScheme from `theme.colors`, `ThemeColorScheme`, font scheme; the
//!    `$key` references resolve here).
//! 3. **Slide shell.** One `ppt/slideMasters/slideMaster1.xml` and two
//!    `slideLayouts` (blank + title) per deck; every page becomes
//!    `ppt/slides/slideN.xml` with:
//!    - `p:sld > p:cSld > p:spTree`: one `p:sp` with `a:xfrm` (from
//!      `bounds` / `rotation` / `flip`) and `a:prstGeom` per element;
//!    - text → `a:txBody > a:p/a:r/a:rPr`, parsing the rich-text tags
//!      (`<p>`, `<span style=...>`, `<strong>`, ...) into runs;
//!    - table → `a:tbl > a:tr/a:tc` with `a:tcPr` and merge via
//!      `gridSpan` / `rowSpan`;
//!    - chart → embedded chart XML parts + drawingml `c:chart` reference
//!      (the largest remaining chunk);
//!    - image / icon → media parts + `a:blip` relationships;
//!    - `Page.background` → full-bleed background shape or `p:bg`.
//! 4. **Transitions / notes.** `Page.animations` → `p:timing` on each slide;
//!    `Page.notes` → `ppt/notesSlides/notesSlideN.xml`.
//! 5. **Packaging.** Zip every part with the `zip` crate and output the
//!    `.pptx`, keeping part order and relationship ids stable.
//!
//! # Coordinate conversion
//!
//! PPTD uses px = pt (1:1), so `bounds` map directly to EMU via
//! `x_emu = x_px * 12700` (`a:off`) and `w_emu = w_px * 12700` (`a:ext`).

use std::path::Path;

use crate::pptd::Project;
use crate::{Error, Result};

/// Renders a validated [`Project`] to a `.pptx` OPC package.
///
/// The renderer itself is not implemented yet; the type exists so the CLI
/// and tests can lock in the public API and the part layout in [`super::package`].
#[derive(Debug)]
pub struct PptxWriter<'a> {
    project: &'a Project,
}

impl<'a> PptxWriter<'a> {
    pub fn new(project: &'a Project) -> Self {
        Self { project }
    }

    /// The project this writer renders.
    pub fn project(&self) -> &'a Project {
        self.project
    }

    /// Render `project` into an OPC/ZIP package at `output`.
    ///
    /// Planned dependencies: `quick-xml` (XML writer) + `zip` (OPC package).
    pub fn build(&self, output: &Path) -> Result<()> {
        let _ = (output, self.project);
        Err(Error::NotImplemented(
            "OOXML/OPC writer (planned: quick-xml + zip)",
        ))
    }
}
