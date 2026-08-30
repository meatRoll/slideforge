//! AST → OOXML rendering orchestrator: builds every part of the OPC package
//! and zips it into a `.pptx`.
//!
//! # Implemented / not yet implemented
//!
//! Implemented:
//! - OPC shell: `[Content_Types].xml`, root/presentation/slide `.rels`,
//!   `docProps/core.xml`, `ppt/presentation.xml`;
//! - synthesized minimal slide master + blank layout (structural
//!   compliance only, see `docs/pptx-layout-synthesis.md`), plus one real
//!   `slideLayoutN.xml` per declared `Presentation.layouts` key (P3);
//! - `theme1.xml` from `Presentation.theme` (clrScheme slot mapping is a
//!   draft; see `src/pptx/theme.rs`);
//! - slides for `text`, `shape`, `line`, `icon` (Font Awesome glyphs as
//!   custom geometry, `pptd:icon` round-trip extension) and `image`
//!   elements, page/layout backgrounds (solid/gradient and image
//!   `a:blipFill`), and a default fade transition;
//! - rich text (`<p>`/`<span style>`/`<strong>`/`<em>`): per-paragraph
//!   `text-align`/`line-height`/`margin-top`, per-run color/font-size/...
//!   style overrides;
//! - shape drop shadows (`a:effectLst > a:outerShdw` / `a:innerShdw`,
//!   incl. the `scale` and `inner` SlideForge extensions);
//! - media parts (`png`/`jpg`/`jpeg`) with `contain`/`cover` `a:srcRect`
//!   cropping computed from sniffed image dimensions.
//!
//! Not yet: `table`, `chart` elements (build fails with an explicit
//! [`Error::Unsupported`]); rich text tags beyond `p`/`span`/`strong`/`em`
//! (`<u>`/`<s>`/`<sup>`/`<sub>`/`<a>`/`<ul>`/`<ol>`/`<li>`); image fills on
//! shapes (`fill_xml` rejects `Fill::Image` — only backgrounds support
//! `a:blipFill`); `notes` (notesSlide) and `animations` (`p:timing`) are
//! not emitted yet.
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

use std::collections::HashMap;

use crate::pptd::validate::validate_project;
use crate::pptd::{
    Element, FontFamily, LayoutDef, Page, Presentation, Project, TextStyleConfig, Theme,
};
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

        // SlideForge layout extension (P3): if the deck declares layouts,
        // emit one real slideLayout per key (background + decorative
        // elements) and point each slide at its layout. Otherwise fall back
        // to a single synthesized blank layout (canonical PPTD decks).
        let layouts_owned: Vec<(String, LayoutDef)> = match &presentation.layouts {
            Some(m) if !m.is_empty() => {
                let mut v: Vec<(String, LayoutDef)> =
                    m.iter().map(|(k, d)| (k.clone(), d.clone())).collect();
                v.sort_by(|a, b| a.0.cmp(&b.0));
                v
            }
            _ => Vec::new(),
        };
        let has_real_layouts = !layouts_owned.is_empty();
        let layout_index: HashMap<String, usize> = layouts_owned
            .iter()
            .enumerate()
            .map(|(i, (k, _))| (k.clone(), i + 1))
            .collect();

        // slide master (skeleton; sldLayoutIdLst lists every layout).
        let layout_count = if has_real_layouts {
            layouts_owned.len()
        } else {
            1
        };
        entries.push(PackageEntry::typed(
            "ppt/slideMasters/slideMaster1.xml",
            ContentType::SlideMaster,
            slide_master_xml(layout_count, presentation.theme.as_ref()),
        ));
        let mut master_rels: Vec<Rel> = Vec::new();
        if has_real_layouts {
            for i in 1..=layouts_owned.len() {
                master_rels.push(Rel::new(
                    &format!("rId{i}"),
                    rel_kind::SLIDE_LAYOUT,
                    format!("../slideLayouts/slideLayout{i}.xml"),
                ));
            }
            master_rels.push(Rel::new(
                &format!("rId{}", layouts_owned.len() + 1),
                rel_kind::THEME,
                "../theme/theme1.xml",
            ));
        } else {
            master_rels.push(Rel::new(
                "rId1",
                rel_kind::SLIDE_LAYOUT,
                "../slideLayouts/slideLayout1.xml",
            ));
            master_rels.push(Rel::new("rId2", rel_kind::THEME, "../theme/theme1.xml"));
        }
        entries.push(PackageEntry::opaque(
            "ppt/slideMasters/_rels/slideMaster1.xml.rels",
            rels_xml(&master_rels),
        ));

        if has_real_layouts {
            // Real layouts: one slideLayout part per key, each carrying its
            // background + decorative elements (non-selectable on slides).
            for (i, (_key, def)) in layouts_owned.iter().enumerate() {
                let ln = i + 1;
                let lmedia = collect_media_from(def.background.as_ref(), &def.elements);
                let mut ctx = render::RenderCtx::new(presentation.theme.as_ref());
                ctx.media = lmedia.clone();
                for src in &lmedia {
                    let part_index = media.index_of(src)?;
                    ctx.image_sizes
                        .push((src.clone(), media.part(part_index).size));
                }
                entries.push(PackageEntry::typed(
                    format!("ppt/slideLayouts/slideLayout{ln}.xml"),
                    ContentType::SlideLayout,
                    layout_xml(def, &mut ctx, presentation.theme.as_ref())?,
                ));
                let mut rels = vec![Rel::new(
                    "rId1",
                    rel_kind::SLIDE_MASTER,
                    "../slideMasters/slideMaster1.xml",
                )];
                for (pos, src) in lmedia.iter().enumerate() {
                    let part_index = media.index_of(src)?;
                    let path = media.part(part_index).package_path.clone();
                    rels.push(Rel::new(
                        &format!("rId{}", pos + 2),
                        rel_kind::IMAGE,
                        rel_target(&path),
                    ));
                }
                entries.push(PackageEntry::opaque(
                    format!("ppt/slideLayouts/_rels/slideLayout{ln}.xml.rels"),
                    rels_xml(&rels),
                ));
            }
        } else {
            // Blank fallback layout (canonical PPTD decks without layouts).
            entries.push(PackageEntry::typed(
                "ppt/slideLayouts/slideLayout1.xml",
                ContentType::SlideLayout,
                slide_layout_xml(),
            ));
            entries.push(PackageEntry::opaque(
                "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
                rels_xml(&[Rel::new(
                    "rId1",
                    rel_kind::SLIDE_MASTER,
                    "../slideMasters/slideMaster1.xml",
                )]),
            ));
        }

        // slides
        for (i, page) in pages.iter().enumerate() {
            let index = i + 1;
            // P3: slide media = own background image + own element images
            // (layout decorations live on the slideLayout part now).
            let used = collect_media_from(page.background.as_ref(), &page.elements);
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

            // rId1 → this slide's layout (real layout part, or blank fallback).
            let layout_target = match (page.layout.as_deref(), has_real_layouts) {
                (Some(k), true) => {
                    let ln = layout_index.get(k).copied().unwrap_or(1);
                    format!("../slideLayouts/slideLayout{ln}.xml")
                }
                _ => "../slideLayouts/slideLayout1.xml".to_string(),
            };
            let mut rels = vec![Rel::new("rId1", rel_kind::SLIDE_LAYOUT, &layout_target)];
            for (pos, src) in used.iter().enumerate() {
                let part_index = media.index_of(src)?;
                let part = media.part(part_index);
                // Audio/video clips need the video/audio rel type (plus the
                // p14:media embed on the element); images stay IMAGE.
                let kind = match part.extension.as_str() {
                    "mp4" | "m4v" | "mov" => rel_kind::VIDEO,
                    "mp3" | "m4a" | "wav" | "wma" => rel_kind::AUDIO,
                    _ => rel_kind::IMAGE,
                };
                rels.push(Rel::new(
                    &format!("rId{}", pos + 2),
                    kind,
                    rel_target(&part.package_path),
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
        // P3: the slide carries only its own background + content. Layout
        // decorations live on the referenced slideLayout part (non-selectable
        // on the slide); a missing `background` inherits the layout's `<p:bg>`.
        if let Some(background) = &page.background {
            x.start("p:bg", &[]);
            x.start("p:bgPr", &[]);
            render::bg_fill_xml(&mut x, ctx, theme, background)?;
            x.leaf("a:effectLst", &[]);
            x.end("p:bgPr");
            x.end("p:bg");
        }
        x.start("p:spTree", &[]);
        group_prolog(&mut x);
        render::render_sp_tree(&mut x, ctx, &page.elements, page.groups.as_ref(), index - 1)?;
        x.end("p:spTree");
        x.end("p:cSld");
        x.start("p:clrMapOvr", &[]);
        x.leaf("a:masterClrMapping", &[]);
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
    x.inline_text(
        "dcterms:modified",
        &[("xsi:type", "dcterms:W3CDTF")],
        "1970-01-01T00:00:00Z",
    );
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

fn slide_master_xml(layout_count: usize, theme: Option<&Theme>) -> String {
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
    // rId1..rId{layout_count} → layouts (master rels list them, theme after).
    for i in 1..=layout_count {
        let rid = format!("rId{i}");
        let id = (2147483648 + i).to_string();
        x.leaf("p:sldLayoutId", &[("id", &id), ("r:id", &rid)]);
    }
    x.end("p:sldLayoutIdLst");
    x.start("p:txStyles", &[]);
    emit_tx_style(
        &mut x,
        "p:titleStyle",
        theme.and_then(|t| t.text_styles.get("title")),
        "4400",
    );
    emit_tx_style(
        &mut x,
        "p:bodyStyle",
        theme.and_then(|t| t.text_styles.get("body")),
        "1800",
    );
    emit_tx_style(
        &mut x,
        "p:otherStyle",
        theme.and_then(|t| t.text_styles.get("other")),
        "1800",
    );
    x.end("p:txStyles");
    x.end("p:sldMaster");
    x.into_string()
}

/// Emit one `p:txStyles` paragraph style from the deck's `textStyles`
/// (`title`/`body`/`other` keys captured by `slideforge convert` from the
/// source master's `txStyles`). Placeholder shapes inside slides inherit
/// these defaults per the OOXML chain — the default run size and line
/// spacing must match the source deck or placeholder text (e.g. a subtitle)
/// re-measures its line pitch differently in every consumer.
fn emit_tx_style(x: &mut Xml, name: &str, style: Option<&TextStyleConfig>, fallback_sz: &str) {
    x.start(name, &[]);
    x.start("a:lvl1pPr", &[]);
    if let Some(lh) = style.and_then(|s| s.line_height) {
        let v = ((lh * 100000.0).round() as u64).to_string();
        x.start("a:lnSpc", &[]);
        x.leaf("a:spcPct", &[("val", &v)]);
        x.end("a:lnSpc");
    }
    // Space-before: the master's `lvl1pPr spcBef` (points) is inherited by
    // placeholder paragraphs; dropping it re-measures placeholder line
    // pitch in every consumer.
    if let Some(mt) = style.and_then(|s| s.margin_top).filter(|v| *v > 0.0) {
        let pts = ((mt * 100.0).round() as u64).to_string();
        x.start("a:spcBef", &[]);
        x.leaf("a:spcPts", &[("val", &pts)]);
        x.end("a:spcBef");
    }
    let sz_hundredths = style
        .and_then(|s| s.font_size)
        .map(|pt| ((pt * 100.0).round() as u64).to_string())
        .unwrap_or_else(|| fallback_sz.to_string());
    x.start("a:defRPr", &[("sz", &sz_hundredths)]);
    match style.and_then(|s| s.color.as_ref()) {
        Some(c) => {
            x.start("a:solidFill", &[]);
            x.leaf("a:srgbClr", &[("val", c.0.trim_start_matches('#'))]);
            x.end("a:solidFill");
        }
        None => {
            x.start("a:solidFill", &[]);
            x.leaf("a:schemeClr", &[("val", "tx1")]);
            x.end("a:solidFill");
        }
    }
    if let Some(FontFamily::Single(f)) = style.and_then(|s| s.font_family.as_ref()) {
        x.leaf("a:latin", &[("typeface", f)]);
        x.leaf("a:ea", &[("typeface", f)]);
        x.leaf("a:cs", &[("typeface", f)]);
    }
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
    x.leaf("a:masterClrMapping", &[]);
    x.end("p:clrMapOvr");
    x.end("p:sldLayout");
    x.into_string()
}

/// Media sources referenced by a fill (its image, if any) plus a set of
/// elements (their `Image`s), in order, deduplicated. Used to register a
/// slide's or layout's own media for relationship ids.
fn collect_media_from(
    background: Option<&crate::pptd::shared::Fill>,
    elements: &[Element],
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let push = |src: &str, out: &mut Vec<String>| {
        if !out.iter().any(|s| s == src) {
            out.push(src.to_string());
        }
    };
    if let Some(crate::pptd::shared::Fill::Image { src, .. }) = background {
        push(src, &mut out);
    }
    for element in elements {
        match element {
            Element::Image(image) => push(&image.src, &mut out),
            Element::Video(m) | Element::Audio(m) => {
                push(&m.src, &mut out);
                if let Some(poster) = &m.poster {
                    push(poster, &mut out);
                }
            }
            _ => {}
        }
    }
    out
}

/// A media part's target path relative to a slide/layout part dir.
fn rel_target(package_path: &str) -> String {
    format!(
        "../{}",
        package_path.strip_prefix("ppt/").unwrap_or(package_path)
    )
}

/// Render a [`LayoutDef`] as a real `slideLayout{N}.xml`: its background
/// (`<p:bg>`) + decorative elements in the spTree. Placeholders are not
/// emitted here (P3-core treats slide placeholders as plain shapes; a
/// future refinement can emit `<p:ph>` defs).
fn layout_xml(
    def: &LayoutDef,
    ctx: &mut render::RenderCtx<'_>,
    theme: Option<&Theme>,
) -> Result<String> {
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
    x.start("p:cSld", &[("name", "Slide Layout")]);
    if let Some(background) = &def.background {
        x.start("p:bg", &[]);
        x.start("p:bgPr", &[]);
        render::bg_fill_xml(&mut x, ctx, theme, background)?;
        x.leaf("a:effectLst", &[]);
        x.end("p:bgPr");
        x.end("p:bg");
    }
    x.start("p:spTree", &[]);
    group_prolog(&mut x);
    ctx.in_layout = true;
    render::render_sp_tree(&mut x, ctx, &def.elements, def.groups.as_ref(), 0)?;
    ctx.in_layout = false;
    // One `<p:sp>` per declared placeholder: `<p:ph type>` + xfrm +
    // lstStyle/defRPr so slide placeholders inherit geometry + run-style
    // (the layout→slide placeholder chain; layout extension §8). Sorted by
    // type for deterministic output (HashMap iteration is unordered).
    let mut phs: Vec<_> = def.placeholders.iter().collect();
    phs.sort_by_key(|(k, _)| *k);
    for (type_name, ph_def) in phs {
        render::render_layout_placeholder(&mut x, ctx, theme, type_name, ph_def)?;
    }
    x.end("p:spTree");
    x.end("p:cSld");
    x.start("p:clrMapOvr", &[]);
    x.leaf("a:masterClrMapping", &[]);
    x.end("p:clrMapOvr");
    x.end("p:sldLayout");
    Ok(x.into_string())
}
