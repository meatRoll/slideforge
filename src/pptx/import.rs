//! PPTX → PPTD reverse compiler (`slideforge convert`).
//!
//! Reads an existing OPC `.pptx` package via `zip` + `xmltree` and produces a
//! PPTD project directory that — rebuilt with the forward pipeline — renders
//! the same slides for the supported element corpus:
//!
//! * text boxes (`p:sp` + `p:txBody`), shapes (`p:sp` without text) with
//!   preset geometry or custom path geometry (guide formulas are evaluated at
//!   import time and baked into the SVG path);
//! * straight connectors (`p:cxnSp`) → `elementType: line`;
//! * pictures (`p:pic`) with `media/…` assets copied next to the project;
//! * `pptd:icon` extension shapes → `elementType: icon` (round-trips our own
//!   files and Kimi exports);
//! * solid / gradient fills, borders, theme colors, page backgrounds.
//!
//! Constructs the downstream pipeline cannot represent are **not silently
//! dropped**: they are reported in [`ConvertReport::skipped`] with the slide
//! number, element name and reason (charts, tables, image fills, unusual
//! geometry or transforms…).
//!
//! Fidelity contract: element geometry is converted with the exact inverse of
//! the writer's px↔EMU mapping (`12700` EMU per px), so rebuilding a converted
//! project reproduces the original XML coordinates to the EMU.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use xmltree::Element as XmlEl;

use crate::pptd::ast::{Page, Presentation};
use crate::pptd::elements::{
    Element, ElementCommon, Icon, Image, Line, Shape, Text, TextAutofit, TextContent,
    TextDirection,
};
use crate::pptd::layout::{LayoutDef, PlaceholderDef};
use crate::pptd::shared::{
    Alignment, Border, Bounds, Color, Fill, FontFamily, GradientFill, GradientType,
    HorizontalAlign, ImageCrop, ImageFit, ImageFitMode, LineStyle, Shadow, VerticalAlign,
};
use crate::pptd::theme::Theme;
use crate::{Error, Result};

/// EMU per design px; the exact inverse of [`super::render::emu`].
const EMU_PER_PX: f64 = 12700.0;

/// Extension URI of the `pptd:icon` round-trip marker.
const PPTD_ICON_URI: &str = "{F5677B7D-0D2A-4D9B-9A5C-6D8D7D0E5A5D}";

/// Semantic color key → clrScheme slot (must mirror `pptx::theme`).
const SEMANTIC_KEYS: [(&str, &str); 12] = [
    ("text", "dk1"),
    ("text2", "dk2"),
    ("background", "lt1"),
    ("background2", "lt2"),
    ("primary", "accent1"),
    ("secondary", "accent2"),
    ("accent", "accent3"),
    ("success", "accent4"),
    ("warning", "accent5"),
    ("danger", "accent6"),
    ("link", "hlink"),
    ("folHlink", "folHlink"),
];

/// One unsupported construct that was reported instead of silently dropped.
#[derive(Debug, Clone, PartialEq)]
pub struct Skipped {
    /// One-based slide number.
    pub page: usize,
    /// Element name from `cNvPr`.
    pub name: String,
    /// Why it could not be represented in PPTD.
    pub reason: String,
}

/// Result of a conversion.
#[derive(Debug, Clone, PartialEq)]
pub struct ConvertReport {
    /// Where the PPTD project was written.
    pub output_dir: PathBuf,
    /// Number of slides converted.
    pub page_count: usize,
    /// Total elements written across all pages.
    pub element_count: usize,
    /// Media files copied into `media/`.
    pub media_count: usize,
    /// Unsupported constructs (honest failures, never silent).
    pub skipped: Vec<Skipped>,
}

/// clrScheme slot → hex color (already uppercase, no `#`).
type SlotColors = BTreeMap<String, String>;

/// Convert a `.pptx` package into a PPTD project directory.
pub fn convert_pptx_to_pptd(input: &Path, out_dir: &Path) -> Result<ConvertReport> {
    let file = fs::File::open(input).map_err(|e| {
        Error::Invalid(format!("cannot open {}: {e}", input.display()))
    })?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| Error::Invalid(format!("not a valid OPC zip ({}): {e}", input.display())))?;

    let presentation_part = root_presentation_part(&mut zip)?;
    let pres = parse_part(&mut zip, &presentation_part)?;

    let size = parse_size(&pres)?;
    let master_target = first_rels_target(&mut zip, &presentation_part, "slideMaster");
    let theme_part = master_target
        .as_deref()
        .and_then(|master| first_rels_target(&mut zip, master, "theme"));
    let slots = theme_part
        .as_deref()
        .map(|part| read_theme_slots(&mut zip, part))
        .unwrap_or_default();
    let fonts = theme_part
        .as_deref()
        .map(|part| theme_fonts(&mut zip, part))
        .unwrap_or_default();
    let defaults = master_target
        .as_deref()
        .map(|master| master_defaults(&mut zip, master, &slots))
        .unwrap_or_default();
    let master_bg = master_target
        .as_deref()
        .map(|master| read_master_bg(&mut zip, master, &slots))
        .unwrap_or_default();

    // Slides in `sldIdLst` presentation order (not archive order).
    let slide_parts = presentation_slides(&pres, &mut zip, &presentation_part)?;

    fs::create_dir_all(out_dir.join("pages")).map_err(|e| {
        Error::Invalid(format!("create pages dir: {e}"))
    })?;

    // Media are collected during element extraction and copied once.
    let mut media: BTreeMap<String, String> = BTreeMap::new(); // part path -> basename
    let mut pages = Vec::with_capacity(slide_parts.len());
    let mut skipped = Vec::new();
    let mut element_count = 0usize;
    // SlideForge layout extension: one `LayoutDef` per distinct source
    // layout, shared by every slide that references it. Decorative shapes,
    // background and placeholder templates live here instead of being
    // flattened onto each slide.
    let mut layouts: HashMap<String, LayoutDef> = HashMap::new();
    let mut layout_cache: HashMap<String, LayoutBuild> = HashMap::new();

    for (idx, part) in slide_parts.iter().enumerate() {
        let page_no = idx + 1;
        let slide = parse_part(&mut zip, part)?;
        let (rels, layout_part) = parse_slide_rels(&mut zip, part);
        let master_part = layout_part
            .as_deref()
            .and_then(|l| first_rels_target(&mut zip, l, "slideMaster"));

        // Build (or reuse) the layout for this slide's source layout part:
        // decorative shapes + placeholders + per-layout defaults, captured
        // once. The slide only keeps its own content + a `layout` reference.
        let (layout_field, lprotos, ldefaults, lfonts, lfbg) = match layout_part.as_deref() {
            Some(lp) => {
                if !layout_cache.contains_key(lp) {
                    let built = build_layout(
                        &mut zip,
                        lp,
                        master_part.as_deref(),
                        &slots,
                        &mut media,
                        &mut skipped,
                    )?;
                    layouts.insert(built.key.clone(), built.def.clone());
                    layout_cache.insert(lp.to_string(), built);
                }
                let lb = layout_cache.get(lp).expect("just inserted");
                (
                    Some(lb.key.clone()),
                    lb.protos.clone(),
                    lb.defaults.clone(),
                    lb.fonts.clone(),
                    lb.fallback_bg.clone(),
                )
            }
            // Slide with no layout (rare/invalid): fall back to the
            // presentation-level master defaults/fonts/bg, flat (no layout).
            None => (
                None,
                BTreeMap::new(),
                defaults.clone(),
                fonts.clone(),
                master_bg.clone(),
            ),
        };
        let mut ctx = PageCtx {
            page_no,
            slots: &slots,
            defaults: ldefaults,
            layout_placeholders: lprotos,
            fonts: lfonts,
            used_ids: BTreeSet::new(),
            media: &mut media,
            skipped: &mut skipped,
            fallback_bg: lfbg,
            layout_key: layout_field.clone(),
        };
        let page = extract_slide(&slide, part, &mut zip, &mut ctx, &rels)?;
        element_count += page.elements.len();

        let file_name = format!("{page_no}.page");
        let page_path = out_dir.join("pages").join(&file_name);
        let yaml = serde_yaml::to_string(&page)
            .map_err(|e| Error::Invalid(format!("serialize page {page_no}: {e}")))?;
        fs::write(&page_path, yaml)
            .map_err(|e| Error::Invalid(format!("write {}: {e}", page_path.display())))?;
        pages.push(format!("pages/{file_name}"));
    }

    // Copy referenced media parts once.
    let media_dir = out_dir.join("media");
    let mut media_count = 0usize;
    for (part_path, basename) in &media {
        let mut data = Vec::new();
        zip.by_name(part_path)
            .map_err(|e| Error::Invalid(format!("read media part {part_path}: {e}")))?
            .read_to_end(&mut data)
            .map_err(|e| Error::Invalid(format!("read media part {part_path}: {e}")))?;
        fs::create_dir_all(&media_dir).map_err(|e| {
            Error::Invalid(format!("create media dir: {e}"))
        })?;
        fs::write(media_dir.join(basename), data).map_err(|e| {
            Error::Invalid(format!("write media/{basename}: {e}"))
        })?;
        media_count += 1;
    }

    let title = read_title(&mut zip)?;
    let theme = if slots.is_empty() {
        None
    } else {
        let mut colors = std::collections::HashMap::new();
        for (key, slot) in SEMANTIC_KEYS {
            if let Some(hex) = slots.get(slot) {
                colors.insert(key.to_string(), Color(format!("#{hex}")));
            }
        }
        Some(Theme {
            colors,
            text_styles: std::collections::HashMap::new(),
            table_styles: std::collections::HashMap::new(),
        })
    };

    let presentation = Presentation {
        version: "v2".to_string(),
        title,
        custom_fonts: Vec::new(),
        size,
        theme,
        layouts: if layouts.is_empty() { None } else { Some(layouts) },
        pages,
    };
    let deck_yaml = serde_yaml::to_string(&presentation).map_err(|e| Error::Invalid(format!("serialize deck: {e}")))?;
    fs::write(out_dir.join("deck.pptd"), deck_yaml).map_err(|e| {
        Error::Invalid(format!("write deck.pptd: {e}"))
    })?;

    // DROP TABLE-like safety: report the skipped constructs.
    Ok(ConvertReport {
        output_dir: out_dir.to_path_buf(),
        page_count: slide_parts.len(),
        element_count,
        media_count,
        skipped,
    })
}

/// Context threaded through a single slide's element extraction.
struct PageCtx<'a> {
    page_no: usize,
    slots: &'a SlotColors,
    /// Master `otherStyle` default run properties for un-styled runs.
    defaults: MasterDefaults,
    /// Placeholders captured from the layout/master spTree (keyed by
    /// `(type, idx)`) so slide placeholders that omit `<a:xfrm>` inherit
    /// geometry + `lstStyle` defaults per the OOXML placeholder chain.
    layout_placeholders: BTreeMap<PhKey, PlaceholderProto>,
    /// Theme major/minor fonts to resolve `+mj-lt` / `+mn-lt` aliases.
    fonts: ThemeFonts,
    used_ids: BTreeSet<String>,
    media: &'a mut BTreeMap<String, String>,
    skipped: &'a mut Vec<Skipped>,
    fallback_bg: Option<Fill>,
    /// SlideForge extension key this slide's page will reference
    /// (`Presentation.layouts[key]`); set from the resolved layout.
    layout_key: Option<String>,
}

impl<'a> PageCtx<'a> {
    fn skip(&mut self, name: &str, reason: impl Into<String>) {
        self.skipped.push(Skipped {
            page: self.page_no,
            name: name.to_string(),
            reason: reason.into(),
        });
    }

    /// Unique element id derived from the drawing name.
    fn unique_id(&mut self, base: &str, fallback: &str) -> String {
        let base = if base.trim().is_empty() { fallback } else { base.trim() };
        if self.used_ids.insert(base.to_string()) {
            return base.to_string();
        }
        let mut n = 2;
        loop {
            let candidate = format!("{base} {n}");
            if self.used_ids.insert(candidate.clone()) {
                return candidate;
            }
            n += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Package navigation
// ---------------------------------------------------------------------------

/// Find the `ppt/presentation.xml` part through the package root rels.
fn root_presentation_part(zip: &mut zip::ZipArchive<fs::File>) -> Result<String> {
    let root = parse_part(zip, "_rels/.rels")?;
    for rel in children(&root, "Relationship") {
        let ty = attr(rel, "Type").unwrap_or_default();
        let target = attr(rel, "Target").unwrap_or_default().to_string();
        if ty.ends_with("/officeDocument") {
            return Ok(target);
        }
    }
    Err(Error::Invalid(
        "package has no officeDocument relationship in _rels/.rels".into(),
    ))
}

/// Resolve a relationship target against the part's directory, yielding an
/// archive-internal path like `ppt/slides/slide1.xml`.
fn resolve_target(part: &str, target: &str) -> String {
    let dir = match part.rsplit_once('/') {
        Some((dir, _)) => dir,
        None => "",
    };
    let joined = format!("{dir}/{target}");
    let mut parts: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/// Relationship part path for a main part: `ppt/slides/slide1.xml` →
/// `ppt/slides/_rels/slide1.xml.rels`.
fn rels_part(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((dir, name)) => format!("{dir}/_rels/{name}.rels"),
        None => format!("_rels/{part}.rels"),
    }
}

/// First relationship of the given type from the part's rels.
fn first_rels_target(
    zip: &mut zip::ZipArchive<fs::File>,
    part: &str,
    type_suffix: &str,
) -> Option<String> {
    let rels_part = rels_part(part);
    let root = parse_part(zip, &rels_part).ok()?;
    for rel in children(&root, "Relationship") {
        let ty = attr(rel, "Type").unwrap_or_default();
        let target = attr(rel, "Target").unwrap_or_default();
        if ty.ends_with(type_suffix) {
            return Some(resolve_target(part, target));
        }
    }
    None
}

/// Slide parts in the order of `p:sldIdLst`.
fn presentation_slides(
    pres: &XmlEl,
    zip: &mut zip::ZipArchive<fs::File>,
    presentation_part: &str,
) -> Result<Vec<String>> {
    let sld_id_lst = first(pres, "sldIdLst").ok_or_else(|| {
        Error::Invalid("presentation.xml has no p:sldIdLst".into())
    })?;
    let rels_part = rels_part(presentation_part);
    let rels = parse_part(zip, &rels_part)?;
    let mut by_id = BTreeMap::new();
    for rel in children(&rels, "Relationship") {
        let id = attr(rel, "Id").unwrap_or_default().to_string();
        let target = attr(rel, "Target").unwrap_or_default();
        by_id.insert(id, resolve_target(presentation_part, target));
    }
    let mut out = Vec::new();
    for sld_id in children(sld_id_lst, "sldId") {
        // xmltree stores attribute local names: `r:id` → `id`.
        let rid = attr(sld_id, "id").ok_or_else(|| {
            Error::Invalid("p:sldId missing r:id".into())
        })?;
        let target = by_id.get(rid).ok_or_else(|| {
            Error::Invalid(format!("slide rId {rid} not in presentation rels"))
        })?;
        out.push(target.clone());
    }
    Ok(out)
}

/// `p:sldSz` → design size in px (exact inverse of the writer).
fn parse_size(pres: &XmlEl) -> Result<crate::pptd::shared::Size> {
    let sz = first(pres, "sldSz").ok_or_else(|| {
        Error::Invalid("presentation.xml has no p:sldSz".into())
    })?;
    let cx: i64 = attr(sz, "cx")
        .ok_or_else(|| Error::Invalid("sldSz missing cx".into()))?
        .parse()
        .map_err(|_| Error::Invalid("sldSz cx not an integer".into()))?;
    let cy: i64 = attr(sz, "cy")
        .ok_or_else(|| Error::Invalid("sldSz missing cy".into()))?
        .parse()
        .map_err(|_| Error::Invalid("sldSz cy not an integer".into()))?;
    Ok(crate::pptd::shared::Size {
        width: cx as f64 / EMU_PER_PX,
        height: cy as f64 / EMU_PER_PX,
    })
}

/// Theme major/minor font names, for resolving `+mj-lt` / `+mn-lt` aliases.
#[derive(Debug, Clone, Default)]
struct ThemeFonts {
    major: Option<String>,
    minor: Option<String>,
}

/// Resolve a theme font alias to the concrete typeface, keeping explicit
/// names untouched. `+mn-lt`/`+mn-ea`/`+mn-cs` and the `+mj-*` set map to
/// the theme's minor/major Latin face.
fn resolve_typeface(tf: &str, fonts: &ThemeFonts) -> Option<String> {
    match tf {
        "+mn-lt" | "+mn-ea" | "+mn-cs" => fonts.minor.clone(),
        "+mj-lt" | "+mj-ea" | "+mj-cs" => fonts.major.clone(),
        s => (!s.is_empty()).then(|| s.to_string()),
    }
}

fn theme_fonts(zip: &mut zip::ZipArchive<fs::File>, theme_part: &str) -> ThemeFonts {
    let Ok(theme) = parse_part(zip, theme_part) else {
        return ThemeFonts::default();
    };
    let slot = |name: &str| {
        first_descendant(&theme, name)
            .and_then(|f| first(f, "latin"))
            .and_then(|l| attr(l, "typeface"))
            .filter(|s| !s.is_empty() && !s.starts_with('+'))
            .map(str::to_string)
    };
    ThemeFonts {
        major: slot("majorFont"),
        minor: slot("minorFont"),
    }
}

/// Default run properties for plain (non-placeholder) text: the master's
/// `otherStyle → lvl1pPr → defRPr`. Runs lacking an explicit attribute
/// inherit these, and so does the paragraph mark — the rebuild must carry
/// them explicitly to reproduce the source (a missing `sz` in the source
/// run is *not* the renderer's 18pt default but the master's 28.35pt).
#[derive(Debug, Clone, Default)]
struct MasterDefaults {
    sz: Option<f64>,
    color: Option<Color>,
    /// Raw typeface (may still be a `+mn-lt` alias; resolved at use).
    latin_typeface: Option<String>,
    bold: Option<bool>,
    italic: Option<bool>,
}

fn master_defaults(
    zip: &mut zip::ZipArchive<fs::File>,
    master_part: &str,
    slots: &SlotColors,
) -> MasterDefaults {
    let empty = MasterDefaults::default();
    let Ok(master) = parse_part(zip, master_part) else {
        return empty;
    };
    let Some(other) = first_descendant(&master, "otherStyle") else {
        return empty;
    };
    let Some(lvl1) = first(other, "lvl1pPr") else {
        return empty;
    };
    let Some(rpr) = first(lvl1, "defRPr") else {
        return empty;
    };
    def_rpr_defaults(rpr, slots)
}

/// `defRPr` → `MasterDefaults` (shared by the master `otherStyle` path and
/// the layout placeholder `lstStyle` path).
fn def_rpr_defaults(rpr: &XmlEl, slots: &SlotColors) -> MasterDefaults {
    MasterDefaults {
        sz: attr(rpr, "sz").and_then(|s| s.parse::<f64>().ok()).map(|s| s / 100.0),
        color: color_from_fill(rpr, slots),
        latin_typeface: first(rpr, "latin")
            .and_then(|l| attr(l, "typeface"))
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        bold: attr(rpr, "b").and_then(|s| s.parse::<u8>().ok()).map(|v| v != 0),
        italic: attr(rpr, "i").and_then(|s| s.parse::<u8>().ok()).map(|v| v != 0),
    }
}

/// Placeholder match key: OOXML `type` (absent → "body") plus optional `idx`.
type PhKey = (String, Option<String>);

/// Inherited geometry + text defaults of a layout/master placeholder, used
/// to rebuild slide placeholders that omit `<a:xfrm>` / `<a:prstGeom>` /
/// `<a:lstStyle>` per the OOXML placeholder inheritance chain.
#[derive(Clone)]
struct PlaceholderProto {
    /// Absolute (slide-space) transform captured from the layout shape.
    xfrm: Xform,
    /// Preset geometry name when the layout placeholder carries one.
    prst: Option<String>,
    /// Run-style fallback from the placeholder's `lstStyle → lvl1pPr →
    /// defRPr` — for a title placeholder this is the only source of the
    /// 24pt bold blue face, since the master `otherStyle` is generic.
    defaults: MasterDefaults,
}

/// `(type, idx)` match key for a `<p:ph>` element. Absent `type` normalises
/// to `"body"` so a body placeholder matches its layout peer.
fn ph_key(ph: &XmlEl) -> PhKey {
    (
        attr(ph, "type").unwrap_or("body").to_string(),
        attr(ph, "idx").map(str::to_string),
    )
}

/// Look up an inherited placeholder by `(type, idx)`, falling back to a
/// type-only match when the indices do not line up.
fn lookup_placeholder<'a>(
    map: &'a BTreeMap<PhKey, PlaceholderProto>,
    key: &PhKey,
) -> Option<&'a PlaceholderProto> {
    map.get(key).or_else(|| map.get(&(key.0.clone(), None)))
}

/// Capture a layout/master placeholder's geometry + `lstStyle` defaults so
/// slide placeholders that rely on OOXML inheritance can be rebuilt.
fn placeholder_proto(el: &XmlEl, slots: &SlotColors) -> Option<PlaceholderProto> {
    let sp_pr = first(el, "spPr")?;
    let xfrm_el = first(sp_pr, "xfrm")?;
    let xfrm = Xform::parse(xfrm_el);
    let prst = first(sp_pr, "prstGeom")
        .and_then(|g| attr(g, "prst"))
        .map(str::to_string);
    let defaults = first(el, "txBody")
        .and_then(|tb| first(tb, "lstStyle"))
        .and_then(|lst| first(lst, "lvl1pPr"))
        .and_then(|lvl1| first(lvl1, "defRPr"))
        .map(|rpr| def_rpr_defaults(rpr, slots))
        .unwrap_or_default();
    Some(PlaceholderProto { xfrm, prst, defaults })
}

/// Owned bundle of a layout's build artifacts, cached per distinct layout
/// part so every slide sharing the layout inherits the same decorations,
/// placeholders and defaults (built once).
struct LayoutBuild {
    key: String,
    def: LayoutDef,
    /// Placeholder protos for slide-time inheritance (richer than the
    /// serialised [`PlaceholderDef`]).
    protos: BTreeMap<PhKey, PlaceholderProto>,
    defaults: MasterDefaults,
    fonts: ThemeFonts,
    fallback_bg: Option<Fill>,
}

/// Derive a stable PPTD layout key from the layout part path, e.g.
/// `ppt/slideLayouts/slideLayout13.xml` → `layout_13`.
fn layout_key(layout_part: &str) -> String {
    let stem = layout_part.rsplit('/').next().unwrap_or(layout_part);
    let stem = stem.trim_end_matches(".xml");
    let digits: String = stem
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    if digits.is_empty() {
        format!("layout_{}", stem.replace('.', "_"))
    } else {
        format!("layout_{digits}")
    }
}

/// Convert a captured layout placeholder proto into the serialisable
/// [`PlaceholderDef`] (geometry + run-style defaults).
fn placeholder_def(proto: &PlaceholderProto, fonts: &ThemeFonts) -> PlaceholderDef {
    PlaceholderDef {
        bounds: to_bounds(&proto.xfrm),
        style: None,
        color: proto.defaults.color.clone(),
        font_size: proto.defaults.sz,
        font_family: proto
            .defaults
            .latin_typeface
            .as_deref()
            .and_then(|tf| resolve_typeface(tf, fonts))
            .map(FontFamily::Single),
        bold: proto.defaults.bold,
        italic: proto.defaults.italic,
        align: None,
    }
}

/// Parse a slide's `.rels` → (relationship map, layout part path).
fn parse_slide_rels(
    zip: &mut zip::ZipArchive<fs::File>,
    part: &str,
) -> (BTreeMap<String, String>, Option<String>) {
    let mut rels = BTreeMap::new();
    let mut layout_part = None;
    if let Ok(rels_root) = parse_part(zip, &rels_part(part)) {
        for rel in children(&rels_root, "Relationship") {
            let id = attr(rel, "Id").unwrap_or_default().to_string();
            let target = attr(rel, "Target").unwrap_or_default();
            let resolved = resolve_target(part, target);
            if attr(rel, "Type").is_some_and(|t| t.ends_with("/slideLayout"))
                && layout_part.is_none()
            {
                layout_part = Some(resolved.clone());
            }
            rels.insert(id, resolved);
        }
    }
    (rels, layout_part)
}

/// Build a [`LayoutDef`] (background + decorative elements + placeholders)
/// plus the internal proto map and per-layout defaults, by walking the
/// layout's master spTree and the layout spTree once. Decorative shapes go
/// into `def.elements`; placeholders into both `def.placeholders` and the
/// returned `protos` (for slide-time inheritance).
fn build_layout(
    zip: &mut zip::ZipArchive<fs::File>,
    layout_part: &str,
    master_part: Option<&str>,
    slots: &SlotColors,
    media: &mut BTreeMap<String, String>,
    skipped: &mut Vec<Skipped>,
) -> Result<LayoutBuild> {
    let key = layout_key(layout_part);

    // Per-layout defaults/fonts/bg from the layout's master (same chain the
    // slide would resolve).
    let (defaults, fonts, fallback_bg) = match master_part {
        Some(master) => (
            master_defaults(zip, master, slots),
            first_rels_target(zip, master, "theme")
                .map(|t| theme_fonts(zip, &t))
                .unwrap_or_default(),
            read_master_bg(zip, master, slots),
        ),
        None => (MasterDefaults::default(), ThemeFonts::default(), None),
    };

    let mut elements = Vec::new();
    let protos: BTreeMap<PhKey, PlaceholderProto>;
    {
        let mut lctx = PageCtx {
            page_no: 0,
            slots,
            defaults: defaults.clone(),
            layout_placeholders: BTreeMap::new(),
            fonts: fonts.clone(),
            used_ids: BTreeSet::new(),
            media,
            skipped,
            fallback_bg: fallback_bg.clone(),
            layout_key: None,
        };
        // Master decorative shapes + placeholders (master inherits to
        // every slide via this layout).
        if let Some(master) = master_part {
            if let Ok(master_el) = parse_part(zip, master) {
                if let Some(tree) = first(&master_el, "cSld").and_then(|c| first(c, "spTree")) {
                    let master_rels = layout_rels(zip, master);
                    for child in tree.children.iter().filter_map(|n| n.as_element()) {
                        let mut generated = Vec::new();
                        walk_sp_tree_child(child, &master_rels, &mut lctx, None, true, &mut generated)?;
                        elements.extend(generated.into_iter().flatten());
                    }
                }
            }
        }
        // Layout decorative shapes + placeholders.
        if let Ok(layout_el) = parse_part(zip, layout_part) {
            if let Some(tree) = first(&layout_el, "cSld").and_then(|c| first(c, "spTree")) {
                let layout_rels = layout_rels(zip, layout_part);
                for child in tree.children.iter().filter_map(|n| n.as_element()) {
                    let mut generated = Vec::new();
                    walk_sp_tree_child(child, &layout_rels, &mut lctx, None, true, &mut generated)?;
                    elements.extend(generated.into_iter().flatten());
                }
            }
        }
        protos = std::mem::take(&mut lctx.layout_placeholders);
    }

    // Layout background: the layout's own <p:bg>, else the master's.
    let background = parse_part(zip, layout_part)
        .ok()
        .and_then(|l| bg_fill(&l, slots))
        .or_else(|| fallback_bg.clone());

    let placeholders = protos
        .iter()
        .map(|(k, p)| (k.0.clone(), placeholder_def(p, &fonts)))
        .collect();

    Ok(LayoutBuild {
        key,
        def: LayoutDef {
            background,
            elements,
            placeholders,
        },
        protos,
        defaults,
        fonts,
        fallback_bg,
    })
}

/// Bullet + paragraph-margin info parsed from a `<a:lvl1pPr>`.
#[derive(Default, Clone)]
struct BulletInfo {
    char: Option<String>,
    font: Option<String>,
    margin: Option<f64>,
    indent: Option<f64>,
}

/// `<p:style><a:fontRef idx="minor|major"> <colour/> </a:fontRef>` → the
/// shape's default text colour / font, merged onto the master `otherStyle`.
/// A coloured "label" card (e.g. a blue rect with the word 背景) carries no
/// explicit run colour; the white text comes from the style's fontRef.
fn font_ref_defaults(base: MasterDefaults, sp_el: &XmlEl, slots: &SlotColors) -> MasterDefaults {
    let Some(style) = first(sp_el, "style") else { return base; };
    let Some(fr) = first(style, "fontRef") else { return base; };
    let mut d = base;
    if let Some(c) = color_from_fill(fr, slots) {
        d.color = Some(c);
    }
    match attr(fr, "idx") {
        Some("minor") => d.latin_typeface = Some("+mn-lt".to_string()),
        Some("major") => d.latin_typeface = Some("+mj-lt".to_string()),
        _ => {}
    }
    d
}

/// Merge a `<a:defRPr>` onto `base` (overrides only present attributes),
/// so an `lstStyle` that specifies just `sz`/`i` keeps the base colour/font.
fn merge_def_rpr(mut base: MasterDefaults, rpr: &XmlEl, slots: &SlotColors) -> MasterDefaults {
    if let Some(sz) = attr(rpr, "sz").and_then(|s| s.parse::<f64>().ok()) {
        base.sz = Some(sz / 100.0);
    }
    if let Some(c) = color_from_fill(rpr, slots) {
        base.color = Some(c);
    }
    if let Some(tf) = first(rpr, "latin")
        .and_then(|l| attr(l, "typeface"))
        .filter(|s| !s.is_empty())
    {
        base.latin_typeface = Some(tf.to_string());
    }
    if let Some(b) = attr(rpr, "b").and_then(|s| s.parse::<u8>().ok()) {
        base.bold = Some(b != 0);
    }
    if let Some(i) = attr(rpr, "i").and_then(|s| s.parse::<u8>().ok()) {
        base.italic = Some(i != 0);
    }
    base
}

/// `txBody > lstStyle > lvl1pPr` → merged run-style defaults (`defRPr`) plus
/// the bullet glyph / hanging indent. Returns `base` unchanged when the box
/// has no `lstStyle` (plain text boxes / placeholders inherit elsewhere).
fn lst_style_info(
    base: MasterDefaults,
    tx_body: &XmlEl,
    slots: &SlotColors,
) -> (MasterDefaults, BulletInfo) {
    let mut d = base;
    let mut bullet = BulletInfo::default();
    let Some(lst) = first(tx_body, "lstStyle") else { return (d, bullet); };
    let Some(lvl1) = first(lst, "lvl1pPr") else { return (d, bullet); };
    if let Some(v) = attr(lvl1, "marL").and_then(|s| s.parse::<f64>().ok()) {
        bullet.margin = Some(px(v));
    }
    if let Some(v) = attr(lvl1, "indent").and_then(|s| s.parse::<f64>().ok()) {
        bullet.indent = Some(px(v));
    }
    if let Some(bf) = first(lvl1, "buFont").and_then(|e| attr(e, "typeface")) {
        bullet.font = Some(bf.to_string());
    }
    if let Some(bc) = first(lvl1, "buChar").and_then(|e| attr(e, "char")) {
        bullet.char = Some(bc.to_string());
    }
    if let Some(rpr) = first(lvl1, "defRPr") {
        d = merge_def_rpr(d, rpr, slots);
    }
    (d, bullet)
}

/// 12 clrScheme slots → hex strings, plus the standard `a:clrMap` aliases
/// (`bg1→lt1`, `tx1→dk1`, `bg2→lt2`, `tx2→dk2`) so schemeClr refs like
/// `bg1` resolve without reading the master's clrMap.
fn read_theme_slots(zip: &mut zip::ZipArchive<fs::File>, theme_part: &str) -> SlotColors {
    let Ok(theme) = parse_part(zip, theme_part) else {
        return SlotColors::new();
    };
    // clrScheme lives under a:themeElements in real themes.
    let Some(scheme) = first_descendant(&theme, "clrScheme") else {
        return SlotColors::new();
    };
    let mut slots = SlotColors::new();
    for child in scheme.children.iter().filter_map(|n| n.as_element()) {
        let rgb = color_rgb(child, &SlotColors::new());
        if let Some(rgb) = rgb {
            slots.insert(child.name.clone(), rgb);
        }
    }
    // Scheme aliases (the mapping a slide uses via bg1/tx1/bg2/tx2).
    if let (Some(lt1), Some(dk1), Some(lt2), Some(dk2)) = (
        slots.get("lt1").cloned(),
        slots.get("dk1").cloned(),
        slots.get("lt2").cloned(),
        slots.get("dk2").cloned(),
    ) {
        slots.insert("bg1".into(), lt1);
        slots.insert("tx1".into(), dk1);
        slots.insert("bg2".into(), lt2);
        slots.insert("tx2".into(), dk2);
    }
    slots
}

/// Master slide background (only solid fills are representable).
fn read_master_bg(
    zip: &mut zip::ZipArchive<fs::File>,
    master_part: &str,
    slots: &SlotColors,
) -> Option<Fill> {
    let master = parse_part(zip, master_part).ok()?;
    bg_fill(&master, slots)
}

/// `docProps/core.xml` dc:title.
fn read_title(zip: &mut zip::ZipArchive<fs::File>) -> Result<Option<String>> {
    let Ok(core) = parse_part(zip, "docProps/core.xml") else {
        return Ok(None);
    };
    Ok(first(&core, "title").and_then(|el| el.get_text()).map(|c| c.trim().to_string()).filter(|s| !s.is_empty()))
}

// ---------------------------------------------------------------------------
// Slide extraction
// ---------------------------------------------------------------------------

fn extract_slide(
    slide: &XmlEl,
    part: &str,
    _zip: &mut zip::ZipArchive<fs::File>,
    ctx: &mut PageCtx<'_>,
    rels: &BTreeMap<String, String>,
) -> Result<Page> {
    let c_sld = first(slide, "cSld").ok_or_else(|| {
        Error::Invalid(format!("slide part {part} has no p:cSld"))
    })?;
    let sp_tree = first(c_sld, "spTree").ok_or_else(|| {
        Error::Invalid(format!("slide part {part} has no p:spTree"))
    })?;

    // SlideForge layout extension: the slide carries only its own content.
    // Master/layout decorations, background and placeholder templates now
    // live in `Presentation.layouts[ctx.layout_key]`; the build step merges
    // them back when baking (P3 will emit real slideLayout parts). A slide
    // without a layout keeps the resolved fallback (flat, canonical PPTD).
    let background = if ctx.layout_key.is_some() {
        bg_fill(slide, ctx.slots)
    } else {
        bg_fill(slide, ctx.slots).or_else(|| ctx.fallback_bg.clone())
    };

    let mut elements = Vec::new();
    for child in sp_tree.children.iter().filter_map(|n| n.as_element()) {
        let mut generated = Vec::new();
        walk_sp_tree_child(child, rels, ctx, None, false, &mut generated)?;
        elements.extend(generated.into_iter().flatten());
    }

    Ok(Page {
        page_type: None,
        layout: ctx.layout_key.clone(),
        background,
        notes: None,
        elements,
        animations: None,
    })
}

/// Slide-layout rels (targets resolved against the layout part).
fn layout_rels(zip: &mut zip::ZipArchive<fs::File>, layout_part: &str) -> BTreeMap<String, String> {
    let mut rels = BTreeMap::new();
    if let Ok(rels_root) = parse_part(zip, &rels_part(layout_part)) {
        for rel in children(&rels_root, "Relationship") {
            let id = attr(rel, "Id").unwrap_or_default().to_string();
            let target = attr(rel, "Target").unwrap_or_default();
            rels.insert(id, resolve_target(layout_part, target));
        }
    }
    rels
}

/// One level of the spTree: dispatch on the element kind and flatten groups.
fn walk_sp_tree_child(
    el: &XmlEl,
    rels: &BTreeMap<String, String>,
    ctx: &mut PageCtx<'_>,
    group: Option<&Xform>,
    in_layout: bool,
    out: &mut Vec<Option<Element>>,
) -> Result<()> {
    match el.name.as_str() {
        "sp" => out.push(map_sp(el, rels, ctx, group, in_layout)?),
        "pic" => out.push(map_pic(el, rels, ctx, group)),
        "cxnSp" => out.push(map_line(el, ctx, group)),
        "grpSp" => {
            flatten_group(el, rels, ctx, group, in_layout, out)?;
        }
        "graphicFrame" => {
            let name = cnv_name(el).unwrap_or_else(|| "graphicFrame".into());
            let uri = graphic_uri(el).unwrap_or_default();
            let kind = if uri.contains("/chart") {
                "chart"
            } else if uri.contains("/table") {
                "table"
            } else {
                "graphicFrame"
            };
            ctx.skip(&name, format!("{kind} elements are not representable in PPTD"));
            out.push(None);
        }
        "oleObj" => {
            ctx.skip(cnv_name(el).unwrap_or_else(|| "oleObj".into()).as_str(), "embedded objects are not supported");
            out.push(None);
        }
        // nvGrpSpPr / grpSpPr are descriptors, not drawables.
        other => {
            out.push(None);
            let _ = other;
        }
    }
    Ok(())
}

fn flatten_group(
    el: &XmlEl,
    rels: &BTreeMap<String, String>,
    ctx: &mut PageCtx<'_>,
    outer: Option<&Xform>,
    in_layout: bool,
    out: &mut Vec<Option<Element>>,
) -> Result<()> {
    let Some(grel) = first(el, "grpSpPr").and_then(|p| first(p, "xfrm")) else {
        return Ok(());
    };
    let g = Xform::parse(grel).apply(outer);
    if let Some(rot) = g.rot {
        if rot.abs() > 1e-9 {
            ctx.skip(cnv_name(el).unwrap_or_else(|| "grpSp".into()).as_str(), "rotated groups are not supported");
            return Ok(());
        }
    }
    for child in el.children.iter().filter_map(|n| n.as_element()) {
        match child.name.as_str() {
            "nvGrpSpPr" | "grpSpPr" => continue,
            _ => {}
        }
        walk_sp_tree_child(child, rels, ctx, Some(&g), in_layout, out)?;
    }
    Ok(())
}

/// The shape's `a:xfrm` (optionally composed into an enclosing group box).
#[derive(Debug, Clone, Copy)]
struct Xform {
    off: (f64, f64),
    ext: (f64, f64),
    ch_off: (f64, f64),
    ch_ext: (f64, f64),
    rot: Option<f64>,
    flip: Option<(bool, bool)>,
}

impl Xform {
    fn parse(xfrm: &XmlEl) -> Self {
        // `a:off` uses x/y, `a:ext` uses cx/cy.
        let (off, ext) = match (xy(xfrm, "off", "x", "y"), xy(xfrm, "ext", "cx", "cy")) {
            (Some(off), Some(ext)) => (off, ext),
            _ => ((0.0, 0.0), (0.0, 0.0)),
        };
        let ch_off = xy(xfrm, "chOff", "x", "y").unwrap_or((0.0, 0.0));
        let ch_ext = xy(xfrm, "chExt", "cx", "cy").unwrap_or(ext);
        Xform {
            off,
            ext,
            ch_off,
            ch_ext,
            rot: attr(xfrm, "rot").and_then(|s| s.parse().ok()).map(|v: i64| v as f64 / 60000.0),
            flip: Some((
                attr(xfrm, "flipH").is_some_and(|v| v == "1"),
                attr(xfrm, "flipV").is_some_and(|v| v == "1"),
            )),
        }
    }

    /// Compose this shape's xfrm with an enclosing flattened group xfrm.
    fn apply(&self, group: Option<&Xform>) -> Xform {
        match group {
            None => *self,
            Some(g) => {
                let (sx, sy) = if g.ch_ext.0 > 0.0 && g.ch_ext.1 > 0.0 {
                    (g.ext.0 / g.ch_ext.0, g.ext.1 / g.ch_ext.1)
                } else {
                    (1.0, 1.0)
                };
                Xform {
                    off: (
                        g.off.0 + (self.off.0 - g.ch_off.0) * sx,
                        g.off.1 + (self.off.1 - g.ch_off.1) * sy,
                    ),
                    ext: (self.ext.0 * sx, self.ext.1 * sy),
                    ch_off: self.ch_off,
                    ch_ext: self.ch_ext,
                    rot: self.rot,
                    flip: self.flip,
                }
            }
        }
    }
}

fn xy(el: &XmlEl, name: &str, x_attr: &str, y_attr: &str) -> Option<(f64, f64)> {
    let n = first(el, name)?;
    Some((
        attr(n, x_attr).and_then(|s| s.parse().ok()).unwrap_or(0.0),
        attr(n, y_attr).and_then(|s| s.parse().ok()).unwrap_or(0.0),
    ))
}

fn px(emu: f64) -> f64 {
    emu / EMU_PER_PX
}

fn to_bounds(x: &Xform) -> Bounds {
    Bounds {
        x: px(x.off.0),
        y: px(x.off.1),
        width: px(x.ext.0),
        height: px(x.ext.1),
    }
}

fn common(x: &Xform, id: String) -> ElementCommon {
    ElementCommon {
        element_id: id,
        bounds: to_bounds(x),
        rotation: x.rot,
        opacity: None,
        flip: x.flip.filter(|(h, v)| *h || *v),
    }
}

/// The first `p:cNvPr` name of an spTree child.
fn cnv_name(el: &XmlEl) -> Option<String> {
    let nv = first(el, "nv*Pr").or_else(|| {
        el.children
            .iter()
            .filter_map(|n| n.as_element())
            .find(|c| c.name.starts_with("nv") && c.name.ends_with("Pr"))
    })?;
    first(nv, "cNvPr").and_then(|p| attr(p, "name")).map(str::to_string)
}

/// `graphicFrame` → graphicData URI.
fn graphic_uri(el: &XmlEl) -> Option<&str> {
    let graphic = first(el, "graphic")?;
    let data = first(graphic, "graphicData")?;
    attr(data, "uri")
}

// ---------------------------------------------------------------------------
// Text boxes
// ---------------------------------------------------------------------------

fn map_sp(
    el: &XmlEl,
    _rels: &BTreeMap<String, String>,
    ctx: &mut PageCtx<'_>,
    group: Option<&Xform>,
    in_layout: bool,
) -> Result<Option<Element>> {
    let name = cnv_name(el).unwrap_or_else(|| "sp".to_string());
    let nv_pr = first(el, "nvSpPr").and_then(|nv| first(nv, "nvPr"));
    let ph = nv_pr.and_then(|p| first(p, "ph"));
    // Layout/master placeholders only carry sample text that never renders
    // on real slides, so they are not emitted — but first capture their
    // geometry + `lstStyle` defaults so slide placeholders that omit
    // `<a:xfrm>` (the OOXML placeholder inheritance chain) can inherit it.
    // Layout entries overwrite master entries (layout wins, per the chain).
    if in_layout {
        if let Some(ph_el) = ph {
            if let Some(proto) = placeholder_proto(el, ctx.slots) {
                ctx.layout_placeholders.insert(ph_key(ph_el), proto);
            }
            return Ok(None);
        }
    }
    let ext = nv_pr
        .and_then(|p| first(p, "extLst"))
        .and_then(|lst| children_in(lst, "ext").into_iter().find(|e| attr(e, "uri") == Some(PPTD_ICON_URI)))
        .and_then(|ext| first(ext, "icon"));
    let sp_pr = first(el, "spPr");
    let xfrm_el = sp_pr.and_then(|p| first(p, "xfrm"));
    // A slide placeholder with no `<a:xfrm>` inherits the layout/master
    // placeholder's transform (plus prstGeom + `lstStyle` defaults) instead
    // of being skipped — PowerPoint/WPS omit `<a:xfrm>` whenever the shape
    // is meant to line up with the layout placeholder.
    let mut inherited_defaults: Option<MasterDefaults> = None;
    // A placeholder inherits the layout placeholder's run-style defaults
    // (colour/font/size) even when the slide carries its own `<a:xfrm>`:
    // geometry may be overridden per-slide while text styling still falls
    // back to the layout's `lstStyle` defRPr.
    let inherited_proto: Option<PlaceholderProto> = ph.and_then(|ph_el| {
        lookup_placeholder(&ctx.layout_placeholders, &ph_key(ph_el)).cloned()
    });
    if let Some(proto) = &inherited_proto {
        inherited_defaults = Some(proto.defaults.clone());
    }
    let (x, inherited_prst) = match (xfrm_el, inherited_proto) {
        (Some(xel), _) => (Xform::parse(xel).apply(group), None),
        (None, Some(proto)) => {
            // The proto xfrm is already in slide space; placeholders are
            // never grouped, so the group transform does not apply.
            (proto.xfrm, proto.prst)
        }
        (None, None) => {
            ctx.skip(
                &name,
                if ph.is_some() {
                    "placeholder has no a:xfrm and no matching layout placeholder"
                } else {
                    "shape has no a:xfrm"
                },
            );
            return Ok(None);
        }
    };

    // `pptd:icon` extension → icon element.
    if let Some(icon_el) = ext {
        if let Some(icon_name) = icon_payload(icon_el) {
            let id = ctx.unique_id(&name, "icon");
            let fill = sp_pr.and_then(|p| solid_fill(p, ctx.slots));
            return Ok(Some(Element::Icon(Icon {
                common: common(&x, id),
                icon_name,
                fill,
                border: border_from_el(el, ctx.slots),
                shadow: shadow_from(sp_pr, ctx.slots),
            })));
        }
    }

    // Determine geometry: preset name or custom path (guide evaluation).
    let geom = sp_pr.and_then(|p| first(p, "custGeom"));
    // A slide placeholder that omits `<a:prstGeom>` inherits the layout's
    // (typically `rect`) — without it the shape would hit the
    // "neither prstGeom nor custGeom" skip.
    let prst = sp_pr
        .and_then(|p| first(p, "prstGeom"))
        .and_then(|g| attr(g, "prst"))
        .map(str::to_string)
        .or(inherited_prst);

    // Text boxes: a `p:txBody` with non-empty text.
    if let Some(tx_body) = first(el, "txBody") {
        // Run-style fallback chain (low → high priority): master
        // `otherStyle` → shape `<p:style>` fontRef (label colour/font) →
        // placeholder-inherited layout `lstStyle` → the box's own
        // `lstStyle` defRPr (size/italic/…). Each layer merges so an
        // unspecified attribute keeps falling back; runs/paragraphs then
        // override on top (handled inside `text_content`).
        let mut base = ctx.defaults.clone();
        base = font_ref_defaults(base, el, ctx.slots);
        if let Some(d) = &inherited_defaults {
            base = d.clone();
        }
        let (base, bullet) = lst_style_info(base, tx_body, ctx.slots);
        let saved_defaults = ctx.defaults.clone();
        ctx.defaults = base;
        let mut content = text_content(tx_body, ctx.slots, ctx, &name);
        ctx.defaults = saved_defaults;
        if let Some(ref mut c) = content {
            c.bullet_char = bullet.char;
            c.bullet_font = bullet.font;
            c.list_margin = bullet.margin;
            c.list_indent = bullet.indent;
        }
        if let Some(content) = content {
            let id = ctx.unique_id(&name, "text");
            // A text box may itself be a coloured card (solid/gradient fill)
            // with an outline; capture both so the rebuild reproduces the
            // box behind the text instead of a transparent text box.
            let fill = shape_fill(el, ctx.slots);
            let border = border_from_el(el, ctx.slots);
            // SlideForge layout extension: mark a slide placeholder with its
            // `<p:ph type>` so P3 can emit `<p:ph type=...>` and the layout
            // template in `layouts[key].placeholders[type]` is the inheritance
            // source. Non-placeholder text boxes carry `None`.
            let placeholder = ph.as_ref().map(|p| {
                attr(p, "type").unwrap_or("body").to_string()
            });
            return Ok(Some(Element::Text(Text {
                common: common(&x, id),
                content,
                fill,
                border,
                placeholder,
            })));
        }
        // Empty text bodies: invisible padding boxes — only keep them when
        // they carry a visible fill/border of their own.
    }

    let fill = shape_fill(el, ctx.slots);
    if sp_pr.is_some_and(|p| first(p, "blipFill").is_some()) {
        ctx.skip(&name, "image fills on shapes are not representable in PPTD");
    }

    let mut element = None;
    if let Some(geom_el) = geom {
        match custom_shape(geom_el, &name, ctx) {
            Ok(Some((view_box, path, adjustments))) => {
                let id = ctx.unique_id(&name, "custom shape");
                element = Some(Element::Shape(Shape {
                    common: common(&x, id),
                    shape_name: "custom".to_string(),
                    adjustments,
                    view_box: Some(view_box),
                    path: Some(path),
                    fill,
                    border: border_from_el(el, ctx.slots),
                    shadow: shadow_from(sp_pr, ctx.slots),
                }));
            }
            Ok(None) => {}
            Err(reason) => ctx.skip(&name, reason),
        }
    } else if let Some(prst) = prst {
        if prst == "rect" {
            // A rect without text acts as a plain fill/border rectangle.
        }
        let id = ctx.unique_id(&name, "shape");
        let adjustments = av_list_values(sp_pr.and_then(|p| first(p, "prstGeom")));
        element = Some(Element::Shape(Shape {
            common: common(&x, id),
            shape_name: prst.to_string(),
            adjustments,
            view_box: None,
            path: None,
            fill,
            border: border_from_el(el, ctx.slots),
            shadow: shadow_from(sp_pr, ctx.slots),
        }));
    } else {
        ctx.skip(&name, "shape with neither prstGeom nor custGeom");
    }

    // Drop invisible leftovers (no fill, no border, no text). Such a shape
    // contributes nothing to the paint order, so deleting it is lossless.
    if let Some(Element::Shape(shape)) = &element {
        if shape.fill.is_none() && shape.border.is_none() {
            let _ = shape;
            return Ok(None);
        }
    }
    Ok(element)
}

/// Fill from the shape properties: solid / gradient; `noFill`/absent → None.
/// Blip (image) fills cannot be represented → reported.
fn fill_and_border(sp_pr: &XmlEl, slots: &SlotColors) -> Option<Fill> {
    if first(sp_pr, "noFill").is_some() {
        return None;
    }
    if let Some(solid) = first(sp_pr, "solidFill") {
        return color_from_fill(solid, slots).map(|color| Fill::Solid { color });
    }
    if let Some(grad) = first(sp_pr, "gradFill") {
        return gradient_from(grad, slots);
    }
    None
}

/// `<p:style><a:fillRef idx="N"> <colour/> </a:fillRef>` → the shape's fill
/// when `spPr` carries no explicit fill. `idx="0"` means noFill; `idx≥1`
/// uses the fillRef's child colour (resolved through the theme slots). WPS
/// draws these "label card" rectangles (矩形 6/11/13 on slide 3) with no
/// `spPr` fill — the accent1 fill comes from here.
fn fill_ref_fill(sp_el: &XmlEl, slots: &SlotColors) -> Option<Fill> {
    let style = first(sp_el, "style")?;
    let fr = first(style, "fillRef")?;
    if attr(fr, "idx")? == "0" {
        return None;
    }
    color_from_fill(fr, slots).map(|color| Fill::Solid { color })
}

/// Shape fill precedence: explicit `spPr` fill (solid/gradient) →
/// `<a:noFill/>` (no fill, do NOT fall back) → `<p:style><a:fillRef>`.
fn shape_fill(sp_el: &XmlEl, slots: &SlotColors) -> Option<Fill> {
    if let Some(sp_pr) = first(sp_el, "spPr") {
        if first(sp_pr, "noFill").is_some() {
            return None;
        }
        if let Some(solid) = first(sp_pr, "solidFill") {
            return color_from_fill(solid, slots).map(|color| Fill::Solid { color });
        }
        if let Some(grad) = first(sp_pr, "gradFill") {
            return gradient_from(grad, slots);
        }
    }
    fill_ref_fill(sp_el, slots)
}

fn solid_fill(sp_pr: &XmlEl, slots: &SlotColors) -> Option<Fill> {
    if first(sp_pr, "noFill").is_some() {
        return None;
    }
    first(sp_pr, "solidFill")
        .and_then(|f| color_from_fill(f, slots))
        .map(|color| Fill::Solid { color })
}

/// `<p:spPr><a:effectLst><a:outerShdw …>` → [`Shadow`]. `innerShdw` and
/// other effects are ignored. The colour (incl. `<a:alpha>`) is resolved
/// via [`color_from_fill`]; `dist`/`dir` map to the `[x, y]` offset (px).
/// `blurRad` → `blur` (px); the OOXML scale attrs `sx`/`sy` are not
/// representable in the PPTD `Shadow` model and are dropped.
fn shadow_from(sp_pr: Option<&XmlEl>, slots: &SlotColors) -> Option<Shadow> {
    let sp_pr = sp_pr?;
    let effects = first(sp_pr, "effectLst")?;
    let shdw = first(effects, "outerShdw")?;
    let blur = attr(shdw, "blurRad")
        .and_then(|s| s.parse::<f64>().ok())
        .map(|v| px(v))
        .unwrap_or(0.0);
    let dist = attr(shdw, "dist")
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);
    let dir_deg = attr(shdw, "dir")
        .and_then(|s| s.parse::<f64>().ok())
        .map(|v| v / 60000.0)
        .unwrap_or(0.0);
    let r = dir_deg.to_radians();
    let ox = dist * r.cos();
    let oy = dist * r.sin();
    let offset = if ox.abs() < 1e-6 && oy.abs() < 1e-6 {
        None
    } else {
        Some((px(ox), px(oy)))
    };
    let color = color_from_fill(shdw, slots)?;
    let scale = attr(shdw, "sx")
        .and_then(|s| s.parse::<f64>().ok())
        .map(|v| v / 100000.0)
        .filter(|&v| (v - 1.0).abs() > 1e-6);
    Some(Shadow { blur, color, offset, scale })
}

fn border_from_el(sp_el: &XmlEl, slots: &SlotColors) -> Option<Border> {
    // Explicit `<a:ln>` in spPr takes precedence; `<a:noFill>` inside it
    // means "no line" and must NOT fall back to the style lnRef.
    if let Some(sp_pr) = first(sp_el, "spPr") {
        if let Some(ln) = first(sp_pr, "ln") {
            if first(ln, "noFill").is_some() {
                return None;
            }
            let width = attr(ln, "w").and_then(|s| s.parse::<i64>().ok()).map(|v| px(v as f64));
            let style = first(ln, "prstDash").and_then(|d| attr(d, "val")).map(|v| match v {
                "dash" | "sysDash" | "dashDot" | "lgDash" | "lgDashDot" | "lgDashDotDot" | "sysDashDot" => LineStyle::Dash,
                "dot" | "sysDot" => LineStyle::Dot,
                _ => LineStyle::Solid,
            });
            // A gradient outline is captured verbatim; a solid colour
            // directly. `<a:ln>` without any fill child draws no visible
            // outline in Office (it inherits the shape's fill) — emit
            // nothing rather than a 1pt black ring.
            let gradient = first(ln, "gradFill")
                .and_then(|g| gradient_from(g, slots))
                .and_then(|f| match f {
                    Fill::Gradient { gradient_type, stops, angle } => {
                        Some(GradientFill { gradient_type, stops, angle })
                    }
                    _ => None,
                });
            let color = first(ln, "solidFill").and_then(|s| color_from_fill(s, slots));
            if gradient.is_none() && color.is_none() {
                return None;
            }
            return Some(Border {
                style,
                width,
                color,
                gradient,
            });
        }
    }
    // No explicit spPr `<a:ln>` → fall back to `<p:style><a:lnRef>`.
    ln_ref_border(sp_el, slots)
}

/// `<p:style><a:lnRef idx="N"> <colour/> </a:lnRef>` → the shape's outline
/// when spPr carries no explicit `<a:ln>`. `idx="0"` → no line; `idx≥1`
/// references a theme line style — width scales with idx in the stock
/// Office theme (0.5/1/1.5 px for 1/2/3) and the colour is the lnRef's
/// child, resolved through the theme slots (lumMod etc. applied).
fn ln_ref_border(sp_el: &XmlEl, slots: &SlotColors) -> Option<Border> {
    let style = first(sp_el, "style")?;
    let lr = first(style, "lnRef")?;
    let idx = attr(lr, "idx")?;
    if idx == "0" {
        return None;
    }
    let width = match idx {
        "1" => 0.5,
        "2" => 1.0,
 "3" => 1.5,
        _ => 1.0,
    };
    color_from_fill(lr, slots).map(|c| Border {
        style: Some(LineStyle::Solid),
        width: Some(width),
        color: Some(c),
        gradient: None,
    })
}

/// `prstGeom > avLst` guide values (preset units, e.g. 16667 = 1/6 radius).
fn av_list_values(prst: Option<&XmlEl>) -> Option<Vec<f64>> {
    let mut values = Vec::new();
    for gd in prst?.children.iter().filter_map(|n| n.as_element()) {
        if gd.name != "avLst" {
            continue;
        }
        for g in gd.children.iter().filter_map(|n| n.as_element()) {
            if g.name == "gd" {
                if let Some(fmla) = attr(g, "fmla") {
                    if let Some(rest) = fmla.strip_prefix("val ") {
                        if let Ok(v) = rest.parse::<f64>() {
                            values.push(v);
                        }
                    }
                }
            }
        }
    }
    if values.is_empty() {
        None
    } else {
        Some(values)
    }
}

// ---------------------------------------------------------------------------
// Custom geometry: guide evaluation + path → SVG string
// ---------------------------------------------------------------------------

/// Simple geometry-guide evaluator (ECMA-376 `CT_GeomGuide`).
struct GeomGuides {
    values: BTreeMap<String, f64>,
}

impl GeomGuides {
    fn new(rect: (f64, f64, f64, f64), av: &[(String, f64)]) -> Self {
        let (l, t, r, b) = rect;
        let w = r - l;
        let h = b - t;
        let mut values = BTreeMap::new();
        values.insert("l".into(), l);
        values.insert("t".into(), t);
        values.insert("r".into(), r);
        values.insert("b".into(), b);
        values.insert("w".into(), w);
        values.insert("h".into(), h);
        values.insert("hc".into(), (l + r) / 2.0);
        values.insert("vc".into(), (t + b) / 2.0);
        values.insert("wd2".into(), w / 2.0);
        values.insert("hd2".into(), h / 2.0);
        for (name, v) in av {
            values.insert(name.clone(), *v);
        }
        Self { values }
    }

    fn get(&self, name: &str) -> Option<f64> {
        self.values.get(name).copied()
    }

    /// Evaluate one `fmla` string; `None` when an operand is not yet known.
    fn evaluate(&mut self, name: &str, fmla: &str) -> std::result::Result<(), String> {
        let tokens: Vec<&str> = fmla.split_whitespace().collect();
        if tokens.is_empty() {
            return Err(format!("empty formula for guide `{name}`"));
        }
        let op = tokens[0];
        let resolve = |tok: &str, g: &GeomGuides| -> Option<f64> {
            tok.parse::<f64>().ok().or_else(|| g.get(tok))
        };
        let args: Vec<Option<f64>> = tokens[1..].iter().map(|t| resolve(t, self)).collect();
        let value = match op {
            "val" => args.first().copied().flatten(),
            "*/" => {
                let [Some(a), Some(b), Some(c)] = args.as_slice() else { return Err(format!("bad */ fmla `{fmla}`")); };
                Some(a * b / c)
            }
            "+/" => {
                let [Some(a), Some(b), Some(c)] = args.as_slice() else { return Err(format!("bad +/ fmla `{fmla}`")); };
                Some((a + b) / c)
            }
            "+-" => {
                let [Some(a), Some(b), Some(c)] = args.as_slice() else { return Err(format!("bad +- fmla `{fmla}`")); };
                Some(a + b - c)
            }
            "--" => {
                let [Some(a), Some(b), Some(c)] = args.as_slice() else { return Err(format!("bad -- fmla `{fmla}`")); };
                Some(a - b - c)
            }
            "min" => {
                let [Some(a), Some(b)] = args.as_slice() else { return Err(format!("bad min fmla `{fmla}`")); };
                Some(a.min(*b))
            }
            "max" => {
                let [Some(a), Some(b)] = args.as_slice() else { return Err(format!("bad max fmla `{fmla}`")); };
                Some(a.max(*b))
            }
            "abs" => args.first().copied().flatten().map(f64::abs),
            "sqrt" => args.first().copied().flatten().map(f64::sqrt),
            "sin" | "cos" | "tan" => {
                // `sin ang hd2 y` → sin(ang) * hd2 + y, ang in 60000ths of a
                // degree.
                let [Some(a), Some(b), Some(c)] = args.as_slice() else { return Err(format!("bad {op} fmla `{fmla}`")); };
                let rad = a.to_radians();
                let f = match op {
                    "sin" => rad.sin(),
                    "cos" => rad.cos(),
                    _ => rad.tan(),
                };
                Some(f * b + c)
            }
            "cat2" => {
                // atan2(y, x) in 60000ths of a degree.
                let [Some(y), Some(x)] = args.as_slice() else { return Err(format!("bad cat2 fmla `{fmla}`")); };
                Some(y.atan2(*x).to_degrees() * 60000.0)
            }
            _ => return Err(format!("unsupported guide operator `{op}` in `{fmla}`")),
        };
        match value {
            Some(v) => {
                self.values.insert(name.to_string(), v);
                Ok(())
            }
            None => Err(format!("guide `{name}` has unresolved operands: `{fmla}`")),
        }
    }
}

/// Outcome of evaluating a `custGeom`: view box, SVG path string, and any
/// preset adjustments.
type CustomGeom = ((f64, f64), String, Option<Vec<f64>>);

/// Extract `<a:path>` list from a custGeom → PPTD `(view_box, path)`.
/// Returns `Ok(None)` for silently-skippable empty geometry.
fn custom_shape(
    geom: &XmlEl,
    _name: &str,
    ctx: &mut PageCtx<'_>,
) -> std::result::Result<Option<CustomGeom>, String> {
    let rect_el = first(geom, "rect");
    let path_lst = first(geom, "pathLst").ok_or("custGeom without pathLst")?;

    // Adjustment values from avLst.
    let av: Vec<(String, f64)> = vec![];
    let rect = match rect_el {
        Some(r) => (
            attr(r, "l").and_then(|s| s.parse().ok()).unwrap_or(0.0),
            attr(r, "t").and_then(|s| s.parse().ok()).unwrap_or(0.0),
            attr(r, "r").and_then(|s| s.parse().ok()).unwrap_or(0.0),
            attr(r, "b").and_then(|s| s.parse().ok()).unwrap_or(0.0),
        ),
        None => {
            // Fall back to the first path's coordinate space.
            let p = first(path_lst, "path").ok_or("custGeom without a:path")?;
            let w = p.attributes.get("w").and_then(|s| s.parse::<f64>().ok()).unwrap_or(100.0);
            let h = p.attributes.get("h").and_then(|s| s.parse::<f64>().ok()).unwrap_or(100.0);
            (0.0, 0.0, w, h)
        }
    };

    let mut guides = GeomGuides::new(rect, &av);

    // avLst adjustments feed `adj` guides (their fmla is `val N`).
    if let Some(av_lst) = first(geom, "avLst") {
        for gd in av_lst.children.iter().filter_map(|n| n.as_element()) {
            if gd.name == "gd" {
                if let Some(fmla) = attr(gd, "fmla") {
                    let gname = attr(gd, "name").unwrap_or("adj").to_string();
                    if let Some(rest) = fmla.strip_prefix("val ") {
                        if let Ok(v) = rest.parse::<f64>() {
                            guides.values.insert(gname.clone(), v);
                        }
                    }
                }
            }
        }
    }
    for gd in first(geom, "gdLst").into_iter().flat_map(|lst| lst.children.iter().filter_map(|n| n.as_element())) {
        if gd.name == "gd" {
            let gname = attr(gd, "name").unwrap_or("").to_string();
            let fmla = attr(gd, "fmla").unwrap_or("");
            // Retry up to N times: formulas may reference earlier guides.
            let mut ok = false;
            for _ in 0..8 {
                match guides.evaluate(&gname, fmla) {
                    Ok(()) => {
                        ok = true;
                        break;
                    }
                    Err(_) => continue,
                }
            }
            if !ok {
                return Err(format!("could not evaluate guide `{gname}` (`{fmla}`)"));
            }
        }
    }

    // Build the SVG path string from every `<a:path>`.
    let mut d_parts: Vec<String> = Vec::new();
    let mut view_box: Option<(f64, f64)> = None;
    for p in children_in(path_lst, "path") {
        let w = p.attributes.get("w").and_then(|s| s.parse::<f64>().ok()).unwrap_or(rect.2 - rect.0);
        let h = p.attributes.get("h").and_then(|s| s.parse::<f64>().ok()).unwrap_or(rect.3 - rect.1);
        view_box = Some((w, h));
        let mut d = String::new();
        for seg in p.children.iter().filter_map(|n| n.as_element()) {
            // A handful of WPS ornaments ship `moveTo`/`cubicBezTo` without
            // points; Office tolerates them, so we skip those segments too.
            let pt_opt = |s: &XmlEl| pt(s, &guides).ok();
            let seg_pt = |s: &XmlEl| -> Option<(f64, f64)> {
                // moveTo/lnTo carry <a:pt> as a *child*; the segment itself
                // has no x/y attributes.
                let child = first(s, "pt")?;
                pt(child, &guides).ok()
            };
            match seg.name.as_str() {
                "moveTo" => {
                    if let Some((x, y)) = seg_pt(seg) {
                        d.push_str(&format!("M {} {}", rnd(x), rnd(y)));
                    }
                }
                "lnTo" => {
                    if let Some((x, y)) = seg_pt(seg) {
                        d.push_str(&format!("L {} {}", rnd(x), rnd(y)));
                    }
                }
                "cubicBezTo" => {
                    let pts = seg
                        .children
                        .iter()
                        .filter_map(|n| n.as_element())
                        .filter_map(pt_opt)
                        .collect::<Vec<_>>();
                    if pts.len() == 3 {
                        d.push_str(&format!(
                            "C {} {} {} {} {} {}",
                            rnd(pts[0].0), rnd(pts[0].1), rnd(pts[1].0), rnd(pts[1].1), rnd(pts[2].0), rnd(pts[2].1)
                        ));
                    }
                }
                "quadBezTo" => {
                    let pts = seg
                        .children
                        .iter()
                        .filter_map(|n| n.as_element())
                        .filter_map(pt_opt)
                        .collect::<Vec<_>>();
                    if pts.len() == 2 {
                        d.push_str(&format!("Q {} {} {} {}", rnd(pts[0].0), rnd(pts[0].1), rnd(pts[1].0), rnd(pts[1].1)));
                    }
                }
                "close" => d.push('Z'),
                other => {
                    let _ = other;
                }
            }
        }
        d_parts.push(d);
    }
    let Some(view_box) = view_box else {
        return Ok(None);
    };
    if d_parts.is_empty() {
        return Ok(None);
    }
    let path = d_parts.join(" ");

    // Only `val` adjustments that are referenced by guides matter for
    // rebuilding; since values are baked into the path, none are needed.
    let adjustments = None;
    let _ = ctx;
    Ok(Some((view_box, path, adjustments)))
}

fn rnd(v: f64) -> i64 {
    v.round() as i64
}

fn pt(el: &XmlEl, guides: &GeomGuides) -> std::result::Result<(f64, f64), String> {
    let x_raw = attr(el, "x").map(str::to_string);
    let y_raw = attr(el, "y").map(str::to_string);
    let gx = x_raw.as_deref().and_then(|s| s.parse::<f64>().ok()).or_else(|| guides.get(x_raw.as_deref()?));
    let gy = y_raw.as_deref().and_then(|s| s.parse::<f64>().ok()).or_else(|| guides.get(y_raw.as_deref()?));
    match (gx, gy) {
        (Some(x), Some(y)) => Ok((x, y)),
        _ => Err(format!("path point references an unknown guide (x={x_raw:?} y={y_raw:?})")),
    }
}

// ---------------------------------------------------------------------------
// Pictures / connectors / icons
// ---------------------------------------------------------------------------

fn map_pic(
    el: &XmlEl,
    rels: &BTreeMap<String, String>,
    ctx: &mut PageCtx<'_>,
    group: Option<&Xform>,
) -> Option<Element> {
    let name = cnv_name(el).unwrap_or_else(|| "pic".to_string());
    let blip_fill = first(el, "blipFill");
    let embed = blip_fill
        .and_then(|b| first(b, "blip"))
        .and_then(|b| attr(b, "embed")); // `r:embed` → local name `embed`
    let sp_pr = first(el, "spPr");
    let xfrm = sp_pr.and_then(|p| first(p, "xfrm"))?;
    let x = Xform::parse(xfrm).apply(group);

    let (src, _part) = match embed.and_then(|rid| rels.get(rid)) {
        Some(target) => {
            let ext = target.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
            if !matches!(ext.as_str(), "png" | "jpg" | "jpeg") {
                ctx.skip(&name, format!("media extension .{ext} is not supported by the builder"));
                return None;
            }
            let basename = target.rsplit('/').next().unwrap_or(target).to_string();
            // `rels` already holds resolved archive paths (`ppt/media/imageN.png`).
            let part = target.clone();
            ctx.media.entry(part.clone()).or_insert_with(|| basename.clone());
            (format!("media/{basename}"), part)
        }
        None => {
            ctx.skip(&name, "picture without r:embed");
            return None;
        }
    };
    let _ = _part;

    // srcRect → fit mode approximation (exact for full-bleed stretch).
    let (fit, crop) = src_rect_fit(blip_fill, ctx);
    let id = ctx.unique_id(&name, "image");
    Some(Element::Image(Image {
        common: common(&x, id),
        src,
        crop_shape: None,
        fit,
        crop,
        border: None,
        shadow: None,
    }))
}

/// Map `a:srcRect` ppm values to PPTD fit/crop.
fn src_rect_fit(blip_fill: Option<&XmlEl>, _ctx: &mut PageCtx<'_>) -> (Option<ImageFit>, Option<crate::pptd::shared::ImageCrop>) {
    let Some(rect) = blip_fill.and_then(|b| first(b, "srcRect")) else {
        // Office stretches pictures by default; PPTD default would crop.
        return (Some(ImageFit { mode: ImageFitMode::Fill }), None);
    };
    let v = |k: &str| attr(rect, k).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
    let (l, t, r, b) = (v("l"), v("t"), v("r"), v("b"));
    let f = |n: f64| n / 100000.0;
    if l != 0.0 || t != 0.0 || r != 0.0 || b != 0.0 {
        if (t < 0.0 || b < 0.0) && !(l > 0.0 || r > 0.0) {
            // Negative crops pad the slack axis → contain.
            (Some(ImageFit { mode: ImageFitMode::Contain }), None)
        } else {
            // Positive crops clip the source image; stretchen into the box
            // (Fill) with the explicit fractions reproduces the srcRect
            // exactly, whereas `cover` would re-derive its own crops.
            let crop = Some(ImageCrop {
                left: Some(f(l)),
                top: Some(f(t)),
                right: Some(f(r)),
                bottom: Some(f(b)),
            });
            (Some(ImageFit { mode: ImageFitMode::Fill }), crop)
        }
    } else {
        (Some(ImageFit { mode: ImageFitMode::Fill }), None)
    }
}

/// Straight connector → `elementType: line` (degenerate axis allowed).
fn map_line(el: &XmlEl, ctx: &mut PageCtx<'_>, group: Option<&Xform>) -> Option<Element> {
    let name = cnv_name(el).unwrap_or_else(|| "line".to_string());
    let sp_pr = first(el, "spPr");
    let xfrm = sp_pr.and_then(|p| first(p, "xfrm"))?;
    let x = Xform::parse(xfrm).apply(group);

    // The `line` preset runs corner-to-corner; flips select the corners.
    let (mut x0, mut y0) = (0.0f64, 0.0f64);
    let (mut x1, mut y1) = (x.ext.0, x.ext.1);
    // flipH mirrors horizontally; flipV vertically.
    if let Some((fh, fv)) = x.flip {
        if fh {
            std::mem::swap(&mut x0, &mut x1);
        }
        if fv {
            std::mem::swap(&mut y0, &mut y1);
        }
    }
    // Line geometry? `prstGeom` may be "line"/"straightConnector1"/… — keep
    // the bounds as the viewBox so the path space matches the box exactly.
    let view_box = (x.ext.0, x.ext.1);
    let points = format!("{},{} {},{}", rnd(x0), rnd(y0), rnd(x1), rnd(y1));

    let border = border_from_el(el, ctx.slots);
    let id = ctx.unique_id(&name, "line");
    Some(Element::Line(Line {
        common: common(&x, id),
        view_box,
        points,
        curve: None,
        arrow: None,
        border,
        shadow: None,
    }))
}

/// Decode the base64url `pptd:icon` payload → icon name.
fn icon_payload(icon_el: &XmlEl) -> Option<String> {
    let _ = icon_el.attributes.get("encoding");
    let text = icon_el.get_text()?;
    let bytes = base64url_decode(text.trim())?;
    let json = String::from_utf8(bytes).ok()?;
    let key = "\"iconName\":\"";
    let start = json.find(key)? + key.len();
    let end = json[start..].find('"')? + start;
    Some(json[start..end].to_string())
}

fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut bits: u32 = 0;
    let mut nbits = 0u32;
    for ch in s.bytes() {
        let v = TABLE.iter().position(|&c| c == ch)?;
        bits = (bits << 6) | v as u32;
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            out.push((bits >> nbits) as u8);
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Colors / fills / backgrounds
// ---------------------------------------------------------------------------

/// `#RRGGBB` / `#RRGGBBAA` from any color child (srgbClr/schemeClr/sysClr),
/// searched recursively (`solidFill > srgbClr`, `ln > solidFill > …`). The
/// standard modifier children (`tint`/`shade`/`lumMod`/`lumOff`/`satMod`)
/// are folded into the final RGB, e.g. `bg1 + lumMod 50000` → light gray.
fn color_from_fill(el: &XmlEl, slots: &SlotColors) -> Option<Color> {
    if let Some(rgb) = first_descendant(el, "srgbClr") {
        let val = attr(rgb, "val")?;
        return Some(col_with_modifiers(val, alpha_of(rgb), rgb));
    }
    if let Some(scheme) = first_descendant(el, "schemeClr") {
        let slot = attr(scheme, "val")?;
        let rgb = slots.get(slot)?;
        return Some(col_with_modifiers(rgb, alpha_of(scheme), scheme));
    }
    if let Some(sys) = first_descendant(el, "sysClr") {
        let hex = attr(sys, "lastClr");
        if let Some(hex) = hex {
            return Some(col_with_modifiers(hex, alpha_of(sys), sys));
        }
        // sysClr without lastClr: known names.
        let hex = match attr(sys, "val")? {
            "windowText" | "blackText" => "000000",
            "window" => "FFFFFF",
            _ => "000000",
        };
        return Some(col(hex, None));
    }
    if let Some(prst) = first_descendant(el, "prstClr") {
        let hex = preset_color_hex(attr(prst, "val")?);
        return Some(col_with_modifiers(hex, alpha_of(prst), prst));
    }
    None
}

/// OOXML `<a:prstClr val="..."/>` preset colour name → hex (no `#`).
/// Covers the named colours most commonly used in PowerPoint/WPS decks;
/// unknown names fall back to black so the run is never silently dropped
/// (the same value the master `otherStyle` would otherwise supply).
fn preset_color_hex(name: &str) -> &'static str {
    match name {
        "black" | "dkBlack" => "000000",
        "white" | "ltWhite" => "FFFFFF",
        "red" => "FF0000",
        "green" | "lime" => "00FF00",
        "blue" => "0000FF",
        "yellow" => "FFFF00",
        "magenta" | "fuchsia" => "FF00FF",
        "cyan" | "aqua" => "00FFFF",
        "gray" | "grey" => "808080",
        "maroon" => "800000",
        "olive" => "808000",
        "navy" => "000080",
        "purple" => "800080",
        "teal" => "008080",
        "silver" => "C0C0C0",
        "orange" => "FFA500",
        "pink" => "FFC0CB",
        "brown" => "A52A2A",
        "gold" => "FFD700",
        "lightBlue" => "ADD8E6",
        "lightGreen" => "90EE90",
        "lightYellow" => "FFFFE0",
        "lightGray" | "ltGray" | "lightGrey" => "D3D3D3",
        "darkBlue" | "dkBlue" => "00008B",
        "darkGreen" | "dkGreen" => "006400",
        "darkRed" | "dkRed" => "8B0000",
        "darkGray" | "dkGray" | "darkGrey" => "A9A9A9",
        "darkCyan" | "dkCyan" => "008B8B",
        "darkOrange" | "dkOrange" => "FF8C00",
        "darkMagenta" | "dkMagenta" => "8B008B",
        "violet" => "EE82EE",
        "indigo" => "4B0082",
        "cornflowerBlue" => "6495ED",
        "chocolate" => "D2691E",
        "crimson" => "DC143C",
        "salmon" => "FA8072",
        _ => "000000",
    }
}

/// Apply OOXML color modifiers (`tint`/`shade`/`lumMod`/`lumOff`/`satMod`)
/// in document order and compose the final `#RRGGBBAA`.
fn col_with_modifiers(rgb: &str, alpha: Option<f64>, node: &XmlEl) -> Color {
    let hex = rgb.to_ascii_uppercase();
    if hex.len() != 6 {
        return Color(format!("#{hex}"));
    }
    let mut r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0) as f64;
    let mut g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0) as f64;
    let mut b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0) as f64;
    for m in node.children.iter().filter_map(|n| n.as_element()) {
        let Some(p) = attr(m, "val").and_then(|v| v.parse::<f64>().ok()) else {
            continue;
        };
        let p = (p / 100000.0).clamp(0.0, 10.0);
        match m.name.as_str() {
            "tint" => {
                r += (255.0 - r) * p;
                g += (255.0 - g) * p;
                b += (255.0 - b) * p;
            }
            "shade" | "lumMod" => {
                r *= p;
                g *= p;
                b *= p;
            }
            "lumOff" => {
                r += p * 255.0;
                g += p * 255.0;
                b += p * 255.0;
            }
            "satMod" => {
                r = 128.0 + (r - 128.0) * p;
                g = 128.0 + (g - 128.0) * p;
                b = 128.0 + (b - 128.0) * p;
            }
            _ => {}
        }
    }
    let clamp = |v: f64| -> u8 { v.round().clamp(0.0, 255.0) as u8 };
    let out = format!("{:02X}{:02X}{:02X}", clamp(r), clamp(g), clamp(b));
    col(&out, alpha)
}

/// `R:RRGGBB` direct color child of the passed element (for theme slots).
fn color_rgb(el: &XmlEl, _slots: &SlotColors) -> Option<String> {
    if let Some(srgb) = first(el, "srgbClr") {
        return attr(srgb, "val").map(|v| v.to_ascii_uppercase());
    }
    if let Some(sys) = first(el, "sysClr") {
        return attr(sys, "lastClr").map(|v| v.to_ascii_uppercase());
    }
    None
}

fn alpha_of(el: &XmlEl) -> Option<f64> {
    first(el, "alpha").and_then(|a| attr(a, "val")).and_then(|s| s.parse::<f64>().ok()).map(|v| v / 100000.0)
}

fn col(rgb: &str, alpha: Option<f64>) -> Color {
    match alpha {
        Some(a) if a < 0.999 => {
            // Below 100%: carry the alpha through #RRGGBBAA (resolve_color
            // round-trips it); fully transparent is not representable.
            let a8 = (a.clamp(0.0, 1.0) * 255.0).round() as u8;
            Color(format!("#{}{a8:02X}", rgb.to_ascii_uppercase()))
        }
        _ => Color(format!("#{}", rgb.to_ascii_uppercase())),
    }
}

fn gradient_from(grad: &XmlEl, slots: &SlotColors) -> Option<Fill> {
    let mut stops = Vec::new();
    for gs in first(grad, "gsLst")?.children.iter().filter_map(|n| n.as_element()) {
        if gs.name != "gs" {
            continue;
        }
        let pos = attr(gs, "pos").and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) / 100000.0;
        let color = color_from_fill(gs, slots)?;
        stops.push(crate::pptd::shared::ColorStop { position: pos, color });
    }
    if stops.is_empty() {
        return None;
    }
    if let Some(lin) = first(grad, "lin") {
        let angle = attr(lin, "ang").and_then(|s| s.parse::<f64>().ok()).map(|a| a / 60000.0);
        Some(Fill::Gradient {
            gradient_type: GradientType::Linear,
            stops,
            angle,
        })
    } else if first(grad, "path").is_some() {
        Some(Fill::Gradient {
            gradient_type: GradientType::Radial,
            stops,
            angle: None,
        })
    } else {
        None
    }
}

/// Slide or master `<p:bg>` → background fill. `bgRef` resolves through the
/// theme; `bgPr` solid fills map directly; anything else → None.
fn bg_fill(slide_or_master: &XmlEl, slots: &SlotColors) -> Option<Fill> {
    let bg = first(slide_or_master, "cSld").and_then(|c| first(c, "bg"))?;
    if let Some(bg_pr) = first(bg, "bgPr") {
        return fill_and_border(bg_pr, slots);
    }
    if let Some(bg_ref) = first(bg, "bgRef") {
        return color_from_fill(bg_ref, slots).map(|color| Fill::Solid { color });
    }
    None
}

// ---------------------------------------------------------------------------
// Text extraction
// ---------------------------------------------------------------------------

/// Build `TextContent` from `p:txBody`. `None` when the box has no text.
fn text_content(
    tx_body: &XmlEl,
    slots: &SlotColors,
    ctx: &mut PageCtx<'_>,
    _name: &str,
) -> Option<TextContent> {
    let body_pr = first(tx_body, "bodyPr");
    let anchor = body_pr.and_then(|b| attr(b, "anchor")).map(|a| match a {
        "t" => VerticalAlign::Top,
        "ctr" => VerticalAlign::Middle,
        "b" => VerticalAlign::Bottom,
        _ => VerticalAlign::Top,
    });
    let wrap = body_pr
        .and_then(|b| attr(b, "wrap"))
        .map(|w| w != "none");
    let text_direction = body_pr
        .and_then(|b| attr(b, "vert"))
        .map(|v| matches!(v, "eaVert" | "vert" | "vert270") as u8)
        .map(|_| TextDirection::Vertical);
    // Auto-fit: `spAutoFit` / `normAutofit` children of bodyPr.
    let autofit = body_pr.and_then(|b| {
        if first(b, "spAutoFit").is_some() {
            Some(TextAutofit::FitShape)
        } else if first(b, "normAutofit").is_some() {
            Some(TextAutofit::FitText)
        } else {
            None
        }
    });
    // Text insets: OOXML defaults when the attribute is absent are
    // lIns/rIns = 91440 EMU (7.2px) and tIns/bIns = 45720 EMU (3.6px).
    // Using 7.2 for all pushed text ~3.6px too low on boxes with an
    // empty `<a:bodyPr/>` (e.g. slide-3 label boxes 应用背景/行业痛点).
    // Emitting explicit margins makes the rebuild reproduce the source
    // box exactly, while kimi decks keep their explicit zero insets.
    let insets = |key: &str| -> Option<f64> {
        body_pr
            .and_then(|b| attr(b, key))
            .and_then(|s| s.parse::<i64>().ok())
            .map(|v| px(v as f64))
    };
    let (margin_top, margin_left, margin_right, margin_bottom) = (
        insets("tIns").or(Some(3.6)),
        insets("lIns").or(Some(7.2)),
        insets("rIns").or(Some(7.2)),
        insets("bIns").or(Some(3.6)),
    );

    let paragraphs: Vec<&XmlEl> = children(tx_body, "p");
    if paragraphs.is_empty() {
        return None;
    }

    // Per-paragraph data: alignment / spacing + styled runs. Paragraphs whose
    // styles differ from the first one are emitted as PPTD rich text
    // (`<p style>` + `<span style>`) so the rebuild can reproduce each
    // paragraph's own font size, color and line height (WPS title/body mixes
    // 28pt bold headers with 12pt regular body runs in one box).
    struct RichLine {
        align: Option<HorizontalAlign>,
        line_height: Option<f64>,
        margin_top_px: Option<f64>,
        runs: Vec<(Option<RunStyle>, String)>,
    }
    fn css_align(a: HorizontalAlign) -> &'static str {
        match a {
            HorizontalAlign::Left => "left",
            HorizontalAlign::Center => "center",
            HorizontalAlign::Right => "right",
            HorizontalAlign::Justify => "justify",
            HorizontalAlign::Distributed => "distributed",
        }
    }

    fn escape_text(t: &str) -> String {
        t.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    let mut out_lines: Vec<RichLine> = Vec::new();
    for p in &paragraphs {
        let p_pr = first(p, "pPr");
        // A paragraph's own `defRPr` further narrows the master defaults
        // for its runs (WPS boxes regularly carry `sz=1400` there).
        let para_defaults = p_pr
            .and_then(|pp| first(pp, "defRPr"))
            .map(|dr| {
                let mut d = ctx.defaults.clone();
                if let Some(sz) = attr(dr, "sz").and_then(|s| s.parse::<f64>().ok()) {
                    d.sz = Some(sz / 100.0);
                }
                if let Some(c) = color_from_fill(dr, slots) {
                    d.color = Some(c);
                }
                if let Some(tf) = first(dr, "latin")
                    .and_then(|l| attr(l, "typeface"))
                    .filter(|s| !s.is_empty())
                {
                    d.latin_typeface = Some(tf.to_string());
                }
                if let Some(b) = attr(dr, "b").and_then(|s| s.parse::<u8>().ok()) {
                    d.bold = Some(b != 0);
                }
                if let Some(i) = attr(dr, "i").and_then(|s| s.parse::<u8>().ok()) {
                    d.italic = Some(i != 0);
                }
                d
            })
            .unwrap_or_else(|| ctx.defaults.clone());
        let algn = p_pr
            .and_then(|pp| attr(pp, "algn"))
            .map(|a| match a {
                "ctr" => HorizontalAlign::Center,
                "r" => HorizontalAlign::Right,
                "just" => HorizontalAlign::Justify,
                "dist" => HorizontalAlign::Distributed,
                _ => HorizontalAlign::Left,
            });
        let ln = p_pr
            .and_then(|pp| first(pp, "lnSpc"))
            .and_then(|ls| first(ls, "spcPct"))
            .and_then(|sp| attr(sp, "val"))
            .and_then(|v| v.parse::<f64>().ok())
            .map(|v| v / 100000.0);
        let mt = p_pr
            .and_then(|pp| first(pp, "spcBef"))
            .and_then(|b| first(b, "spcPts"))
            .and_then(|s| attr(s, "val"))
            .and_then(|v| v.parse::<f64>().ok())
            .map(|v| v / 100.0); // spcPts/100 → px
        let mut runs: Vec<(Option<RunStyle>, String)> = Vec::new();
        for child in p.children.iter().filter_map(|n| n.as_element()) {
            match child.name.as_str() {
                "r" => {
                    let t = first(child, "t")
                        .and_then(|t| t.get_text())
                        .map(|c| c.to_string())
                        .unwrap_or_default();
                    let st = RunStyle::from_rpr(
                        first(child, "rPr"),
                        slots,
                        &para_defaults,
                        &ctx.fonts,
                    );
                    runs.push((st, t));
                }
                "br" => runs.push((None, "\n".into())),
                "fld" => {
                    let t = first(child, "t")
                        .and_then(|t| t.get_text())
                        .map(|c| c.to_string())
                        .unwrap_or_default();
                    runs.push((None, t));
                }
                _ => {}
            }
        }
        // An empty paragraph (no runs) still renders as an empty line in
        // OOXML — it spaces the following paragraphs apart.
        if runs.is_empty() {
            runs.push((None, String::new()));
        }
        out_lines.push(RichLine {
            align: algn,
            line_height: ln,
            margin_top_px: mt,
            runs,
        });
    }

    // Drop trailing placeholder paragraphs.
    while out_lines
        .last()
        .is_some_and(|l| l.runs.iter().all(|(_, t)| t.is_empty()))
    {
        out_lines.pop();
    }
    if out_lines.is_empty() {
        return None;
    }

    let first = &out_lines[0];
    let align = first.align;
    // Explicit single spacing: the renderer would otherwise default to 120%
    // (kimi's choice), but a source box without any lnSpc renders at 100%.
    let line_height = first.line_height.or(Some(1.0));
    let run_style = first.runs.first().and_then(|(st, _)| st.clone());
    if first.runs.iter().all(|(_, t)| t.is_empty()) && out_lines.len() == 1 {
        return None; // textless box
    }

    // Uniform path: one run per paragraph, identical style → plain text.
    let uniform = out_lines.iter().all(|l| {
        l.runs.len() == 1
            && l.align == align
            && l.line_height == line_height
            && l.margin_top_px.is_none()
            && l.runs[0].0 == run_style
    });

    let text = if uniform {
        out_lines
            .iter()
            .map(|l| l.runs[0].1.clone())
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        let mut markup = String::new();
        for l in &out_lines {
            let mut style_parts: Vec<String> = Vec::new();
            if let Some(a) = l.align {
                style_parts.push(format!("text-align:{}", css_align(a)));
            }
            // Force an explicit value so the renderer never falls back to the
            // 120% default.
            style_parts.push(format!(
                "line-height:{}",
                l.line_height.unwrap_or(1.0)
            ));
            if let Some(mt) = l.margin_top_px {
                style_parts.push(format!("margin-top:{mt}px"));
            }
            let style_attr = if style_parts.is_empty() {
                String::new()
            } else {
                format!(" style=\"{}\"", style_parts.join("; "))
            };
            markup.push_str(&format!("<p{style_attr}>"));
            for (st, t) in &l.runs {
                if t.is_empty() {
                    continue;
                }
                // Runs that match the box-level style (first paragraph's)
                // stay plain; anything else gets an explicit wrapper so the
                // rebuild does not inherit the box style from paragraph 1.
                if st == &run_style && (st.is_none() || !t.contains('\n')) {
                    markup.push_str(&escape_text(t));
                    continue;
                }
                if t == "\n" {
                    markup.push(' ');
                    continue;
                }
                let mut span_style: Vec<String> = Vec::new();
                if let Some(sz) = st.as_ref().and_then(|s| s.font_size) {
                    span_style.push(format!("font-size:{sz}px"));
                }
                if let Some(c) = st.as_ref().and_then(|s| s.color.clone()) {
                    span_style.push(format!("color:{}", c.0));
                }
                if let Some(FontFamily::Single(n)) = st.as_ref().and_then(|s| s.font_family.clone()) {
                    span_style.push(format!("font-family:{n}"));
                }
                if st.as_ref().and_then(|s| s.bold) != run_style.as_ref().and_then(|s| s.bold) {
                    span_style.push(format!(
                        "font-weight:{}",
                        if st.as_ref().is_some_and(|s| s.bold == Some(true)) {
                            "bold"
                        } else {
                            "400"
                        }
                    ));
                }
                if st.as_ref().and_then(|s| s.italic) != run_style.as_ref().and_then(|s| s.italic) {
                    span_style.push(format!(
                        "font-style:{}",
                        if st.as_ref().is_some_and(|s| s.italic == Some(true)) {
                            "italic"
                        } else {
                            "normal"
                        }
                    ));
                }
                let mut inner = escape_text(t);
                if !span_style.is_empty() {
                    inner = format!(
                        "<span style=\"{}\">{inner}</span>",
                        span_style.join("; ")
                    );
                }
                if st.as_ref().is_some_and(|s| s.italic == Some(true)) {
                    inner = format!("<em>{inner}</em>");
                }
                if st.as_ref().is_some_and(|s| s.bold == Some(true)) {
                    inner = format!("<strong>{inner}</strong>");
                }
                markup.push_str(&inner);
            }
            markup.push_str("</p>");
        }
        markup
    };
    if text.trim().is_empty() {
        return None;
    }

    Some(TextContent {
        text,
        style: None,
        color: run_style.as_ref().and_then(|s| s.color.clone()),
        font_size: run_style.as_ref().and_then(|s| s.font_size),
        font_family: run_style.as_ref().and_then(|s| s.font_family.clone()),
        bold: run_style.as_ref().and_then(|s| s.bold),
        italic: run_style.as_ref().and_then(|s| s.italic),
        background_color: None,
        line_height: if uniform { line_height } else { None },
        line_height_px: None,
        letter_spacing: None,
        margin_top,
        margin_left,
        margin_right,
        margin_bottom,
        autofit,
        text_direction,
        wrap,
        align: match (align, anchor) {
            // A paragraph with no explicit `algn` still inherits the box's
            // vertical anchor; keep `align` set whenever the vertical
            // anchor is explicit so it isn't dropped back to Top.
            (Some(h), v) => Some(Alignment {
                horizontal: h,
                vertical: v.unwrap_or_default(),
            }),
            (None, Some(v)) => Some(Alignment {
                horizontal: HorizontalAlign::default(),
                vertical: v,
            }),
            (None, None) => None,
        },
        gradient: None,
        shadow: None,
        bullet_char: None,
        bullet_font: None,
        list_margin: None,
        list_indent: None,
    })
}

/// First run's text properties (others are compared for the flattening warn).
#[derive(Debug, Clone, PartialEq)]
struct RunStyle {
    color: Option<Color>,
    font_size: Option<f64>,
    font_family: Option<FontFamily>,
    bold: Option<bool>,
    italic: Option<bool>,
}

impl RunStyle {
    fn from_rpr(
        rpr: Option<&XmlEl>,
        slots: &SlotColors,
        defaults: &MasterDefaults,
        fonts: &ThemeFonts,
    ) -> Option<Self> {
        let rpr = rpr?;
        // Explicit attributes win; anything missing falls back to the
        // master's otherStyle defaults, and theme aliases are resolved to
        // concrete typefaces. Without this, a run that merely says
        // `<a:rPr/>` would render at the forward writer's 18pt black
        // instead of the master's 28pt + tx1.
        let explicit_sz = attr(rpr, "sz").and_then(|s| s.parse::<f64>().ok());
        let explicit_fill = color_from_fill(rpr, slots);
        let explicit_bold = attr(rpr, "b").and_then(|s| s.parse::<u8>().ok()).map(|v| v != 0);
        let explicit_italic = attr(rpr, "i").and_then(|s| s.parse::<u8>().ok()).map(|v| v != 0);

        let font_size = explicit_sz.map(|s| s / 100.0).or(defaults.sz);
        let color = explicit_fill.or_else(|| defaults.color.clone());
        let bold = explicit_bold.or(defaults.bold);
        let italic = explicit_italic.or(defaults.italic);

        // Family: run latin → master default, each alias-resolved. The
        // `ea`/`cs` slots are dropped: with an explicit latin the visible CJK
        // face is the designer's choice; when only aliases exist the theme
        // face governs.
        let latin: Option<String> = first(rpr, "latin")
            .and_then(|l| attr(l, "typeface"))
            .filter(|s| !s.is_empty())
            .or(defaults.latin_typeface.as_deref())
            .and_then(|tf| resolve_typeface(tf, fonts));
        let font_family = latin.map(FontFamily::Single);

        Some(RunStyle {
            color,
            font_size,
            font_family,
            bold,
            italic,
        })
    }
}

// ---------------------------------------------------------------------------
// Xml helpers
// ---------------------------------------------------------------------------

fn parse_part(zip: &mut zip::ZipArchive<fs::File>, part: &str) -> Result<XmlEl> {
    let mut reader = zip.by_name(part).map_err(|e| {
        Error::Invalid(format!("part {part} missing in package: {e}"))
    })?;
    let mut data = Vec::new();
    reader
        .read_to_end(&mut data)
        .map_err(|e| Error::Invalid(format!("read part {part}: {e}")))?;
    XmlEl::parse(data.as_slice()).map_err(|e| {
        Error::Invalid(format!("parse XML in {part}: {e}"))
    })
}

fn children<'a>(el: &'a XmlEl, name: &str) -> Vec<&'a XmlEl> {
    el.children
        .iter()
        .filter_map(|n| n.as_element())
        .filter(|c| c.name == name)
        .collect()
}

fn children_in<'a>(el: &'a XmlEl, name: &str) -> Vec<&'a XmlEl> {
    children(el, name)
}

fn first<'a>(el: &'a XmlEl, name: &str) -> Option<&'a XmlEl> {
    el.children
        .iter()
        .filter_map(|n| n.as_element())
        .find(|c| c.name == name)
}

/// First element with a matching local name anywhere in the subtree
/// (breadth-first, direct children first).
fn first_descendant<'a>(el: &'a XmlEl, name: &str) -> Option<&'a XmlEl> {
    let mut queue: Vec<&XmlEl> = el.children.iter().filter_map(|n| n.as_element()).collect();
    while let Some(node) = queue.pop() {
        if node.name == name {
            return Some(node);
        }
        queue.extend(node.children.iter().filter_map(|n| n.as_element()));
    }
    None
}

fn attr<'a>(el: &'a XmlEl, name: &str) -> Option<&'a str> {
    el.attributes.get(name).map(String::as_str)
}
#[cfg(test)]
mod tests {
    use super::*;

    fn parse(xml: &str) -> XmlEl {
        XmlEl::parse(xml.as_bytes()).expect("valid test XML")
    }

    fn fonts() -> ThemeFonts {
        ThemeFonts {
            major: Some("Calibri Light".into()),
            minor: Some("Calibri".into()),
        }
    }

    fn defaults() -> MasterDefaults {
        // Mirrors the master otherStyle lvl1 defRPr (sz=2835, tx1, +mn-lt).
        MasterDefaults {
            sz: Some(2835.0 / 100.0),
            color: Some(Color("#000000".into())),
            latin_typeface: Some("+mn-lt".into()),
            bold: None,
            italic: None,
        }
    }

    #[test]
    fn run_without_explicit_attrs_inherits_master_defaults() {
        // `<a:rPr/>` — no sz, no fill, no latin: the effective style must be
        // the master's 28.35pt black Calibri, not the renderer's 18pt black.
        let rpr = parse(r#"<rPr lang="en-US"/>"#);
        let st = RunStyle::from_rpr(Some(&rpr), &SlotColors::new(), &defaults(), &fonts())
            .expect("rPr must yield a style");
        assert_eq!(st.font_size, Some(28.35));
        assert_eq!(st.color.as_ref().map(|c| c.0.as_str()), Some("#000000"));
        assert_eq!(
            st.font_family,
            Some(FontFamily::Single("Calibri".to_string())),
            "+mn-lt alias must resolve to the theme minor font"
        );
    }

    #[test]
    fn explicit_attrs_win_over_defaults() {
        let rpr = parse(r#"<rPr sz="4000" b="1"><solidFill><srgbClr val="0D37D4"/></solidFill><latin typeface="微软雅黑"/></rPr>"#);
        let st = RunStyle::from_rpr(Some(&rpr), &SlotColors::new(), &defaults(), &fonts())
            .expect("rPr must yield a style");
        assert_eq!(st.font_size, Some(40.0));
        assert!(st.bold == Some(true));
        assert_eq!(st.color.as_ref().map(|c| c.0.as_str()), Some("#0D37D4"));
        assert_eq!(
            st.font_family,
            Some(FontFamily::Single("微软雅黑".to_string()))
        );
        // The missing italic falls back to the (empty) default.
        assert_eq!(st.italic, None);
    }

    #[test]
    fn master_default_color_needs_theme_slots() {
        // schemeClr tx1 in the defRPr resolves through the slate of aliases.
        let rpr = parse(r#"<rPr><solidFill><schemeClr val="tx1"/></solidFill></rPr>"#);
        let mut slots = SlotColors::new();
        slots.insert("dk1".into(), "000000".into());
        slots.insert("tx1".into(), "000000".into());
        let st = RunStyle::from_rpr(Some(&rpr), &slots, &MasterDefaults::default(), &fonts())
            .expect("rPr must yield a style");
        assert_eq!(st.color.as_ref().map(|c| c.0.as_str()), Some("#000000"));
    }
}
