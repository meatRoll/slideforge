//! AST → OOXML rendering orchestrator: builds every part of the OPC package
//! and zips it into a `.pptx`.
//!
//! # Implemented / not yet implemented
//!
//! Implemented:
//! - OPC shell: `[Content_Types].xml`, root/presentation/slide `.rels`,
//!   `docProps/core.xml`, `ppt/presentation.xml`;
//! - synthesized minimal slide master + blank layout (structural compliance
//!   only, see `docs/pptx-layout-synthesis.md`);
//! - `theme1.xml` from `Presentation.theme` (clrScheme slot mapping is a
//!   draft; see `src/pptx/theme.rs`);
//! - slides for `text`, `shape`, `line`, `icon` (Font Awesome glyphs as
//!   custom geometry, `pptd:icon` round-trip extension) and `image`
//!   elements, page backgrounds, and a default fade transition;
//! - media parts (`png`/`jpg`/`jpeg`) with `contain`/`cover` `a:srcRect`
//!   cropping computed from sniffed image dimensions.
//!
//! Not yet: `table`, `chart` elements (build fails with an explicit
//! [`Error::Unsupported`]); rich text tags (`<p>`/`<span>`/...) are
//! plain-text only; `Fill::Image` backgrounds/fills, shape shadows, `notes`
//! and `animations` are not emitted yet.
//!
//! # Coordinate conversion
//!
//! PPTD uses px = pt (1:1), so `bounds` map directly to EMU via
//! `x_emu = x_px * 12700` (`a:off`) and `w_emu = w_px * 12700` (`a:ext`).
//!
//! # Slide master / layout synthesis
//!
//! PPTD is a flat model (no masters / layouts); OOXML requires every slide
//! to hang off a slideLayout → slideMaster chain. The writer synthesizes a
//! minimal compliant skeleton and resolves all PPTD style inheritance into
//! explicit slide attributes — the synthesized parts carry no runtime style.
//! See `docs/pptx-layout-synthesis.md` for the full design.

use std::fs;
use std::io::Write;
use std::path::Path;

use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use crate::pptd::validate::validate_project;
use crate::pptd::{Element, Page, Presentation, Project};
use crate::pptx::media::MediaRegistry;
use crate::pptx::opc::{Rel, content_types_xml, ns, rel_kind, rels_xml};
use crate::pptx::package::{ContentType, PackageEntry};
use crate::pptx::render;
use crate::pptx::theme::theme_xml;
use crate::pptx::xml::Xml;
use crate::{Error, Result};

/// Renders a validated [`Project`] to a `.pptx` OPC package.
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
    /// The project is re-validated here (defense in depth): the CLI already
    /// checks, but calling the writer directly from tests/embeddings must
    /// not produce a package from an invalid deck.
    pub fn build(&self, output: &Path) -> Result<()> {
        if let Some(issue) = validate_project(self.project).first() {
            return Err(Error::Validation(issue.to_string()));
        }

        let (mut entries, media_extensions) = self.collect_entries()?;
        let content_types = content_types_xml(&entries, &media_extensions);
        entries.insert(
            0,
            PackageEntry::opaque("[Content_Types].xml", content_types.into_bytes()),
        );

        let file = fs::File::create(output).map_err(|source| Error::Io {
            path: output.to_path_buf(),
            source,
        })?;
        let mut zip = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for entry in &entries {
            zip.start_file(&entry.path, options).map_err(zip_err)?;
            zip.write_all(&entry.data)
                .map_err(|e| Error::Zip(e.to_string()))?;
        }
        zip.finish().map_err(zip_err)?;
        Ok(())
    }

    /// Assemble every part of the package (excluding `[Content_Types].xml`,
    /// which needs the full list and is prepended by the caller) plus the
    /// media extensions that need `Default` content-type entries.
    fn collect_entries(&self) -> Result<(Vec<PackageEntry>, Vec<String>)> {
        let presentation = &self.project.presentation;
        let pages = &self.project.pages;
        let page_count = pages.len();

        let mut entries: Vec<PackageEntry> = Vec::with_capacity(12 + page_count * 2);
        let mut media = MediaRegistry::new(&self.project.root_dir);

        // docProps
        entries.push(core_properties_xml(presentation));

        // theme
        entries.push(PackageEntry::typed(
            "ppt/theme/theme1.xml",
            ContentType::Theme,
            theme_xml(presentation.theme.as_ref()),
        ));

        // slide master + layouts (structural skeleton)
        entries.push(PackageEntry::typed(
            "ppt/slideMasters/slideMaster1.xml",
            ContentType::SlideMaster,
            slide_master_xml(),
        ));
        entries.push(PackageEntry::typed(
            "ppt/slideLayouts/slideLayout1.xml",
            ContentType::SlideLayout,
            slide_layout_xml(),
        ));
        entries.push(PackageEntry::opaque(
            "ppt/slideMasters/_rels/slideMaster1.xml.rels",
            rels_xml(&[
                Rel::new("rId1", rel_kind::THEME, "theme/theme1.xml"),
                Rel::new(
                    "rId2",
                    rel_kind::SLIDE_LAYOUT,
                    "../slideLayouts/slideLayout1.xml",
                ),
            ]),
        ));
        entries.push(PackageEntry::opaque(
            "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
            rels_xml(&[Rel::new(
                "rId1",
                rel_kind::SLIDE_MASTER,
                "../slideMasters/slideMaster1.xml",
            )]),
        ));

        // slides
        for (i, page) in pages.iter().enumerate() {
            let index = i + 1;

            // Register this slide's media up front so rendering can resolve
            // relationship ids and image dimensions.
            let used = collect_media(page);
            let mut ctx = render::RenderCtx::new(presentation.theme.as_ref());
            ctx.media = used.clone();
            for src in &used {
                let part_index = media.index_of(src)?;
                ctx.image_sizes
                    .push((src.clone(), media.part(part_index).size));
            }

            entries.push(PackageEntry::typed(
                format!("ppt/slides/slide{index}.xml"),
                ContentType::Slide,
                self.slide_xml(index, page, &mut ctx)?,
            ));

            // Slide relationships: rId1 = layout, then media in usage order.
            let mut rels = vec![Rel::new(
                "rId1",
                rel_kind::SLIDE_LAYOUT,
                "../../slideLayouts/slideLayout1.xml",
            )];
            for (pos, src) in used.iter().enumerate() {
                let part_index = media.index_of(src)?;
                let part = media.part(part_index);
                rels.push(Rel::new(
                    &format!("rId{}", pos + 2),
                    rel_kind::IMAGE,
                    format!(
                        "../{}",
                        part.package_path
                            .strip_prefix("ppt/")
                            .unwrap_or(&part.package_path)
                    ),
                ));
            }
            entries.push(PackageEntry::opaque(
                format!("ppt/slides/_rels/slide{index}.xml.rels"),
                rels_xml(&rels),
            ));
        }

        // media parts (deduplicated by the registry) + their Default entries
        let media_extensions = media.extensions();
        for part in media.into_parts() {
            entries.push(PackageEntry::opaque(part.package_path, part.data));
        }

        // presentation
        entries.push(PackageEntry::typed(
            "ppt/presentation.xml",
            ContentType::Presentation,
            presentation_xml(presentation),
        ));
        let mut presentation_rels = vec![
            Rel::new(
                "rId1",
                rel_kind::SLIDE_MASTER,
                "slideMasters/slideMaster1.xml",
            ),
            Rel::new("rId2", rel_kind::THEME, "theme/theme1.xml"),
        ];
        for i in 1..=page_count {
            presentation_rels.push(Rel::new(
                &format!("rId{}", i + 2),
                rel_kind::SLIDE,
                format!("slides/slide{i}.xml"),
            ));
        }
        entries.push(PackageEntry::opaque(
            "ppt/_rels/presentation.xml.rels",
            rels_xml(&presentation_rels),
        ));

        // package root relationships
        entries.push(PackageEntry::opaque(
            "_rels/.rels",
            rels_xml(&[
                Rel::new("rId1", rel_kind::OFFICE_DOCUMENT, "ppt/presentation.xml"),
                Rel::new("rId2", rel_kind::CORE_PROPERTIES, "docProps/core.xml"),
            ]),
        ));

        Ok((entries, media_extensions))
    }

    /// One slide per page; content is fully self-contained (background +
    /// elements), never borrowed from the synthesized master/layout.
    fn slide_xml(
        &self,
        index: usize,
        page: &Page,
        ctx: &mut render::RenderCtx<'_>,
    ) -> Result<String> {
        let theme = self.project.presentation.theme.as_ref();
        let mut x = Xml::new();
        x.start(
            "p:sld",
            &[
                ("xmlns:p", ns::PRESENTATIONML),
                ("xmlns:a", ns::DRAWINGML),
                ("xmlns:r", ns::RELATIONSHIPS),
                ("xmlns:pptd", ns::PPTD),
            ],
        );
        x.start("p:cSld", &[]);
        if let Some(background) = &page.background {
            x.start("p:bg", &[]);
            x.start("p:bgPr", &[]);
            render::fill_xml(&mut x, theme, background, None)?;
            x.leaf("a:effectLst", &[]);
            x.end("p:bgPr");
            x.end("p:bg");
        }
        x.start("p:spTree", &[]);
        group_prolog(&mut x);
        for element in &page.elements {
            render::render_element(&mut x, ctx, element, index - 1)?;
        }
        x.end("p:spTree");
        x.end("p:cSld");
        x.start("p:clrMapOvr", &[]);
        x.leaf("a:overrideClrMapping", &[]);
        x.end("p:clrMapOvr");
        x.start("p:transition", &[("spd", "med")]);
        x.leaf("p:fade", &[]);
        x.end("p:transition");
        x.end("p:sld");
        Ok(x.into_string())
    }
}

/// Map a ZIP error into [`Error::Zip`].
fn zip_err(error: zip::result::ZipError) -> Error {
    Error::Zip(error.to_string())
}

/// The `nvGrpSpPr`/`grpSpPr` pair required at the start of every `p:spTree`.
fn group_prolog(x: &mut Xml) {
    x.start("p:nvGrpSpPr", &[]);
    x.leaf("p:cNvPr", &[("id", "1"), ("name", "")]);
    x.leaf("p:cNvGrpSpPr", &[]);
    x.leaf("p:nvPr", &[]);
    x.end("p:nvGrpSpPr");
    x.start("p:grpSpPr", &[]);
    x.start("a:xfrm", &[]);
    x.leaf("a:off", &[("x", "0"), ("y", "0")]);
    x.leaf("a:ext", &[("cx", "0"), ("cy", "0")]);
    x.leaf("a:chOff", &[("x", "0"), ("y", "0")]);
    x.leaf("a:chExt", &[("cx", "0"), ("cy", "0")]);
    x.end("a:xfrm");
    x.end("p:grpSpPr");
}

fn core_properties_xml(presentation: &Presentation) -> PackageEntry {
    let mut x = Xml::new();
    x.start(
        "cp:coreProperties",
        &[
            ("xmlns:cp", ns::CORE_PROPS),
            ("xmlns:dc", ns::DC),
            ("xmlns:dcterms", ns::DCTERMS),
            ("xmlns:dcmitype", ns::DCMITYPE),
            ("xmlns:xsi", ns::XSI),
        ],
    );
    if let Some(title) = &presentation.title {
        x.text_elem("dc:title", title);
    }
    // Fixed timestamp: deterministic builds (no clock in the artifact).
    x.start("dcterms:modified", &[("xsi:type", "dcterms:W3CDTF")]);
    x.text("1970-01-01T00:00:00Z");
    x.end("dcterms:modified");
    x.end("cp:coreProperties");
    PackageEntry::typed(
        "docProps/core.xml",
        ContentType::CoreProperties,
        x.into_string().into_bytes(),
    )
}

fn presentation_xml(presentation: &Presentation) -> Vec<u8> {
    let cx = render::emu(presentation.size.width).to_string();
    let cy = render::emu(presentation.size.height).to_string();
    let mut x = Xml::new();
    x.start(
        "p:presentation",
        &[
            ("xmlns:p", ns::PRESENTATIONML),
            ("xmlns:a", ns::DRAWINGML),
            ("xmlns:r", ns::RELATIONSHIPS),
        ],
    );
    x.start("p:sldMasterIdLst", &[]);
    x.leaf("p:sldMasterId", &[("id", "2147483648"), ("r:id", "rId1")]);
    x.end("p:sldMasterIdLst");
    x.start("p:sldIdLst", &[]);
    for (i, _) in presentation.pages.iter().enumerate() {
        let slide_id = (256 + i).to_string();
        let r_id = format!("rId{}", i + 3);
        x.leaf("p:sldId", &[("id", &slide_id), ("r:id", &r_id)]);
    }
    x.end("p:sldIdLst");
    x.leaf("p:sldSz", &[("cx", &cx), ("cy", &cy)]);
    x.leaf("p:notesSz", &[("cx", "6858000"), ("cy", "9144000")]);
    x.end("p:presentation");
    x.into_string().into_bytes()
}

fn slide_master_xml() -> String {
    let mut x = Xml::new();
    x.start(
        "p:sldMaster",
        &[
            ("xmlns:p", ns::PRESENTATIONML),
            ("xmlns:a", ns::DRAWINGML),
            ("xmlns:r", ns::RELATIONSHIPS),
        ],
    );
    x.start("p:cSld", &[]);
    x.start("p:bg", &[]);
    x.start("p:bgPr", &[]);
    x.start("a:solidFill", &[]);
    x.leaf("a:srgbClr", &[("val", "FFFFFF")]);
    x.end("a:solidFill");
    x.leaf("a:effectLst", &[]);
    x.end("p:bgPr");
    x.end("p:bg");
    x.start("p:spTree", &[]);
    group_prolog(&mut x);
    x.end("p:spTree");
    x.end("p:cSld");
    x.leaf(
        "p:clrMap",
        &[
            ("bg1", "lt1"),
            ("tx1", "dk1"),
            ("bg2", "lt2"),
            ("tx2", "dk2"),
            ("accent1", "accent1"),
            ("accent2", "accent2"),
            ("accent3", "accent3"),
            ("accent4", "accent4"),
            ("accent5", "accent5"),
            ("accent6", "accent6"),
            ("hlink", "hlink"),
            ("folHlink", "folHlink"),
        ],
    );
    x.start("p:sldLayoutIdLst", &[]);
    x.leaf("p:sldLayoutId", &[("id", "2147483649"), ("r:id", "rId2")]);
    x.end("p:sldLayoutIdLst");
    x.start("p:txStyles", &[]);
    emit_tx_style(&mut x, "p:titleStyle", "4400");
    emit_tx_style(&mut x, "p:bodyStyle", "1800");
    emit_tx_style(&mut x, "p:otherStyle", "1800");
    x.end("p:txStyles");
    x.end("p:sldMaster");
    x.into_string()
}

fn emit_tx_style(x: &mut Xml, name: &str, sz: &str) {
    x.start(name, &[]);
    x.start("a:lvl1pPr", &[]);
    x.start("a:defRPr", &[("sz", sz)]);
    x.start("a:solidFill", &[]);
    x.leaf("a:schemeClr", &[("val", "tx1")]);
    x.end("a:solidFill");
    x.end("a:defRPr");
    x.end("a:lvl1pPr");
    x.end(name);
}

fn slide_layout_xml() -> String {
    let mut x = Xml::new();
    x.start(
        "p:sldLayout",
        &[
            ("xmlns:p", ns::PRESENTATIONML),
            ("xmlns:a", ns::DRAWINGML),
            ("xmlns:r", ns::RELATIONSHIPS),
            ("type", "blank"),
            ("preserve", "1"),
        ],
    );
    x.start("p:cSld", &[("name", "Blank")]);
    x.start("p:spTree", &[]);
    group_prolog(&mut x);
    x.end("p:spTree");
    x.end("p:cSld");
    x.start("p:clrMapOvr", &[]);
    x.leaf("a:overrideClrMapping", &[]);
    x.end("p:clrMapOvr");
    x.end("p:sldLayout");
    x.into_string()
}

/// Media sources referenced by a page (in element order, deduplicated).
fn collect_media(page: &Page) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for element in &page.elements {
        if let Element::Image(image) = element {
            if !out.contains(&image.src) {
                out.push(image.src.clone());
            }
        }
    }
    out
}
