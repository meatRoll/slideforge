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
    Element, ElementCommon, GroupDef, GroupXfrm, Icon, Image, Line, Shape, ShapeDef, Text,
    TextAutofit, TextContent, TextDirection,
};
use crate::pptd::layout::{LayoutDef, PlaceholderDef};
use crate::pptd::shared::{
    Alignment, Border, Bounds, Color, Fill, FontFamily, GradientFill, GradientType,
    HorizontalAlign, ImageCrop, ImageFit, ImageFitMode, LineStyle, Shadow, VerticalAlign,
};
use crate::pptd::theme::{TextStyleConfig, Theme};
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
    let file = fs::File::open(input)
        .map_err(|e| Error::Invalid(format!("cannot open {}: {e}", input.display())))?;
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
    let master_styles = master_target
        .as_deref()
        .map(|master| master_text_styles(&mut zip, master, &slots, &fonts))
        .unwrap_or_default();
    let master_bg = master_target
        .as_deref()
        .map(|master| read_master_bg(&mut zip, master, &slots))
        .unwrap_or_default();

    // Slides in `sldIdLst` presentation order (not archive order).
    let slide_parts = presentation_slides(&pres, &mut zip, &presentation_part)?;

    fs::create_dir_all(out_dir.join("pages"))
        .map_err(|e| Error::Invalid(format!("create pages dir: {e}")))?;

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
        let master_line_spacing = ldefaults.line_spacing;
        let master_spc_bef = ldefaults.spc_bef;
        let mut ctx = PageCtx {
            page_no,
            slots: &slots,
            defaults: ldefaults,
            master_line_spacing,
            master_spc_bef,
            para_line_spacing: None,
            para_margin_top: None,
            layout_placeholders: lprotos,
            fonts: lfonts,
            used_ids: BTreeSet::new(),
            media: &mut media,
            skipped: &mut skipped,
            fallback_bg: lfbg,
            layout_key: layout_field.clone(),
            groups: HashMap::new(),
            group_seq: 0,
            in_master: false,
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
        fs::create_dir_all(&media_dir)
            .map_err(|e| Error::Invalid(format!("create media dir: {e}")))?;
        fs::write(media_dir.join(basename), data)
            .map_err(|e| Error::Invalid(format!("write media/{basename}: {e}")))?;
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
            text_styles: master_styles,
            table_styles: std::collections::HashMap::new(),
        })
    };

    let presentation = Presentation {
        version: "v2".to_string(),
        title,
        custom_fonts: Vec::new(),
        size,
        theme,
        layouts: if layouts.is_empty() {
            None
        } else {
            Some(layouts)
        },
        pages,
    };
    let deck_yaml = serde_yaml::to_string(&presentation)
        .map_err(|e| Error::Invalid(format!("serialize deck: {e}")))?;
    fs::write(out_dir.join("deck.pptd"), deck_yaml)
        .map_err(|e| Error::Invalid(format!("write deck.pptd: {e}")))?;

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
    /// Inherited paragraph line spacing (fraction) for placeholder text:
    /// `bodyStyle → lvl1pPr → lnSpc` from the master; the `titleStyle`
    /// value lives in `defaults.title_line_spacing`.
    master_line_spacing: Option<f64>,
    /// Line spacing (fraction) the current shape's paragraphs inherit when
    /// their own `<a:pPr>` carries no `<a:lnSpc>` — set per shape in
    /// `import_shape` from the placeholder chain.
    para_line_spacing: Option<f64>,
    /// Space-before (points) the current shape's paragraphs inherit when
    /// their own `<a:pPr>` carries no `<a:spcBef>` — set per shape in
    /// `import_shape` alongside `para_line_spacing`.
    para_margin_top: Option<f64>,
    /// Master `bodyStyle` space-before (points) for placeholder text.
    master_spc_bef: Option<f64>,
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
    /// SlideForge group extension: reconstructed `<p:grpSp>` metadata,
    /// keyed by group id. Member elements carry `groupId` + `groupBounds`.
    groups: HashMap<String, crate::pptd::elements::GroupDef>,
    /// Monotonic group-id counter (per page).
    group_seq: u32,
    /// True while walking the slideMaster's spTree (vs the slideLayout's):
    /// master placeholders are captured as protos for the inheritance chain
    /// but NOT emitted into the layout's `elements` (the layout already
    /// inherits them; duplicating would put two `<p:ph type="title">` in
    /// the rebuilt layout spTree).
    in_master: bool,
}

impl<'a> PageCtx<'a> {
    fn skip(&mut self, name: &str, reason: impl Into<String>) {
        self.skipped.push(Skipped {
            page: self.page_no,
            name: name.to_string(),
            reason: reason.into(),
        });
    }

    /// Next group id for the SlideForge group extension
    /// (`grp1`, `grp2`, …; unique within the page).
    fn next_group_id(&mut self) -> String {
        self.group_seq += 1;
        format!("grp{}", self.group_seq)
    }

    /// Unique element id derived from the drawing name.
    fn unique_id(&mut self, base: &str, fallback: &str) -> String {
        let base = if base.trim().is_empty() {
            fallback
        } else {
            base.trim()
        };
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
    let sld_id_lst = first(pres, "sldIdLst")
        .ok_or_else(|| Error::Invalid("presentation.xml has no p:sldIdLst".into()))?;
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
        let rid =
            attr(sld_id, "id").ok_or_else(|| Error::Invalid("p:sldId missing r:id".into()))?;
        let target = by_id
            .get(rid)
            .ok_or_else(|| Error::Invalid(format!("slide rId {rid} not in presentation rels")))?;
        out.push(target.clone());
    }
    Ok(out)
}

/// `p:sldSz` → design size in px (exact inverse of the writer).
fn parse_size(pres: &XmlEl) -> Result<crate::pptd::shared::Size> {
    let sz = first(pres, "sldSz")
        .ok_or_else(|| Error::Invalid("presentation.xml has no p:sldSz".into()))?;
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
    /// Inherited paragraph line spacing (fraction, `spcPct/100000`) from
    /// the placeholder's `lstStyle → lvl1pPr → lnSpc` — the slide paragraph
    /// inherits it when its own `<a:pPr>` carries no `<a:lnSpc>` (OOXML
    /// inheritance chain). `master_defaults` fills it from the master's
    /// `bodyStyle`; `placeholder_proto` may override it per placeholder
    /// with the layout's `lstStyle` value.
    line_spacing: Option<f64>,
    /// Master `titleStyle → lvl1pPr → lnSpc` (fraction) for title
    /// placeholders, which inherit the title style rather than the body
    /// style.
    title_line_spacing: Option<f64>,
    /// Inherited paragraph space-before (points, `spcBef`) from the
    /// placeholder chain — dropped when the slide paragraph omits
    /// `<a:spcBef>` and the master/layout supplies one (e.g. the standard
    /// body lvl1 `spcPts 1000`).
    spc_bef: Option<f64>,
    /// Master `titleStyle → lvl1pPr → spcBef` (points).
    title_spc_bef: Option<f64>,
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
    // Capture the master's inherited paragraph line spacing for placeholder
    // text: `bodyStyle` for body/subtitle-like placeholders and `titleStyle`
    // for title placeholders. Slide paragraphs omit `<a:lnSpc>` whenever the
    // master style is the intended value — dropping it here (as before this
    // fix) makes the rebuilt deck fall back to the consumer's 100% default
    // instead of the master's e.g. 90%.
    let ln_spc = |style: &str| {
        first_descendant(&master, style)
            .and_then(|s| first(s, "lvl1pPr"))
            .and_then(|p| first(p, "lnSpc"))
            .and_then(|ls| first(ls, "spcPct"))
            .and_then(|sp| attr(sp, "val"))
            .and_then(|v| v.parse::<f64>().ok())
            .map(|v| v / 100000.0)
    };
    let mut d = def_rpr_defaults(rpr, slots);
    d.line_spacing = ln_spc("bodyStyle");
    d.title_line_spacing = ln_spc("titleStyle");
    let body = first_descendant(&master, "bodyStyle");
    let titles = first_descendant(&master, "titleStyle");
    d.spc_bef = body
        .and_then(|s| first(s, "lvl1pPr"))
        .and_then(|l| lvl1_spc_bef(l, d.sz));
    d.title_spc_bef = titles
        .and_then(|s| first(s, "lvl1pPr"))
        .and_then(|l| lvl1_spc_bef(l, d.sz));
    d
}

/// Master `lvl1pPr spcBef` in points: `spcPts` directly; `spcPct` resolved
/// against the style's default run size.
fn lvl1_spc_bef(lvl1: &XmlEl, default_sz: Option<f64>) -> Option<f64> {
    let bef = first(lvl1, "spcBef")?;
    if let Some(pts) = first(bef, "spcPts").and_then(|s| attr(s, "val")) {
        return pts.parse::<f64>().ok().map(|v| v / 100.0);
    }
    let pct = first(bef, "spcPct")
        .and_then(|s| attr(s, "val"))
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.0);
    // 0% → no space; a real percentage scales the default run size.
    Some(pct / 100000.0 * default_sz.unwrap_or(0.0))
}

/// `defRPr` → `MasterDefaults` (shared by the master `otherStyle` path and
/// the layout placeholder `lstStyle` path).
fn def_rpr_defaults(rpr: &XmlEl, slots: &SlotColors) -> MasterDefaults {
    MasterDefaults {
        sz: attr(rpr, "sz")
            .and_then(|s| s.parse::<f64>().ok())
            .map(|s| s / 100.0),
        color: color_from_fill(rpr, slots),
        latin_typeface: first(rpr, "latin")
            .and_then(|l| attr(l, "typeface"))
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        bold: attr(rpr, "b")
            .and_then(|s| s.parse::<u8>().ok())
            .map(|v| v != 0),
        italic: attr(rpr, "i")
            .and_then(|s| s.parse::<u8>().ok())
            .map(|v| v != 0),
        line_spacing: None,
        title_line_spacing: None,
        spc_bef: None,
        title_spc_bef: None,
    }
}

/// Capture the master's `txStyles` (title/body/other) first-level defaults
/// into PPTD `theme.textStyles` (`title`/`body`/`other` keys) so the
/// rebuild can re-emit the master's `txStyles`. Placeholder shapes inherit
/// these paragraph/run defaults per the OOXML chain, and the default run
/// size + line spacing drive placeholder line metrics in every consumer.
fn master_text_styles(
    zip: &mut zip::ZipArchive<fs::File>,
    master_part: &str,
    slots: &SlotColors,
    fonts: &ThemeFonts,
) -> std::collections::HashMap<String, TextStyleConfig> {
    let mut out = std::collections::HashMap::new();
    let Ok(master) = parse_part(zip, master_part) else {
        return out;
    };
    let Some(tx_styles) = first(&master, "txStyles") else {
        return out;
    };
    for (key, style_name) in [
        ("title", "titleStyle"),
        ("body", "bodyStyle"),
        ("other", "otherStyle"),
    ] {
        let Some(style) = first(tx_styles, style_name) else {
            continue;
        };
        let Some(lvl1) = first(style, "lvl1pPr") else {
            continue;
        };
        let rpr = first(lvl1, "defRPr");
        let cfg = TextStyleConfig {
            color: rpr.and_then(|r| color_from_fill(r, slots)),
            font_size: rpr
                .and_then(|r| attr(r, "sz"))
                .and_then(|s| s.parse::<f64>().ok())
                .map(|v| v / 100.0),
            font_family: rpr
                .and_then(|r| first(r, "latin"))
                .and_then(|l| attr(l, "typeface"))
                .filter(|s| !s.is_empty())
                .and_then(|tf| resolve_typeface(tf, fonts))
                .map(FontFamily::Single),
            bold: rpr
                .and_then(|r| attr(r, "b"))
                .and_then(|s| s.parse::<u8>().ok())
                .map(|v| v != 0),
            italic: rpr
                .and_then(|r| attr(r, "i"))
                .and_then(|s| s.parse::<u8>().ok())
                .map(|v| v != 0),
            line_height: first(lvl1, "lnSpc")
                .and_then(|ls| first(ls, "spcPct"))
                .and_then(|sp| attr(sp, "val"))
                .and_then(|v| v.parse::<f64>().ok())
                .map(|v| v / 100000.0),
            margin_top: lvl1_spc_bef(
                lvl1,
                rpr.and_then(|r| attr(r, "sz"))
                    .and_then(|s| s.parse::<f64>().ok())
                    .map(|v| v / 100.0),
            ),
            background_color: None,
            line_height_px: None,
            letter_spacing: None,
        };
        let has_content = cfg.color.is_some()
            || cfg.font_size.is_some()
            || cfg.font_family.is_some()
            || cfg.bold.is_some()
            || cfg.italic.is_some()
            || cfg.line_height.is_some()
            || cfg.margin_top.is_some();
        if has_content {
            out.insert(key.to_string(), cfg);
        }
    }
    out
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
    /// Vertical anchor from the layout placeholder's `bodyPr` (e.g. title
    /// templates are `anchor="ctr"`), inherited by slide placeholders that
    /// omit `<a:bodyPr>` on their own shape.
    anchor: Option<VerticalAlign>,
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
    let mut defaults = first(el, "txBody")
        .and_then(|tb| first(tb, "lstStyle"))
        .and_then(|lst| first(lst, "lvl1pPr"))
        .and_then(|lvl1| first(lvl1, "defRPr"))
        .map(|rpr| def_rpr_defaults(rpr, slots))
        .unwrap_or_default();
    // The layout placeholder's `lstStyle → lvl1pPr → lnSpc` is inherited by
    // slide placeholders as the paragraph line spacing; when present it
    // overrides the master `bodyStyle` value captured via `master_defaults`.
    defaults.line_spacing = first(el, "txBody")
        .and_then(|tb| first(tb, "lstStyle"))
        .and_then(|lst| first(lst, "lvl1pPr"))
        .and_then(|lvl1| first(lvl1, "lnSpc"))
        .and_then(|ls| first(ls, "spcPct"))
        .and_then(|sp| attr(sp, "val"))
        .and_then(|v| v.parse::<f64>().ok())
        .map(|v| v / 100000.0);
    defaults.spc_bef = first(el, "txBody")
        .and_then(|tb| first(tb, "lstStyle"))
        .and_then(|lst| first(lst, "lvl1pPr"))
        .and_then(|lvl1| lvl1_spc_bef(lvl1, defaults.sz));
    let anchor = first(el, "txBody")
        .and_then(|tb| first(tb, "bodyPr"))
        .and_then(|bp| attr(bp, "anchor"))
        .map(|a| match a {
            "t" => VerticalAlign::Top,
            "ctr" => VerticalAlign::Middle,
            "b" => VerticalAlign::Bottom,
            _ => VerticalAlign::Top,
        });
    Some(PlaceholderProto {
        xfrm,
        prst,
        anchor,
        defaults,
    })
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
    let groups: HashMap<String, crate::pptd::elements::GroupDef>;
    {
        let mut lctx = PageCtx {
            page_no: 0,
            slots,
            defaults: defaults.clone(),
            master_line_spacing: defaults.line_spacing,
            master_spc_bef: defaults.spc_bef,
            para_line_spacing: None,
            para_margin_top: None,
            layout_placeholders: BTreeMap::new(),
            fonts: fonts.clone(),
            used_ids: BTreeSet::new(),
            media,
            skipped,
            fallback_bg: fallback_bg.clone(),
            layout_key: None,
            groups: HashMap::new(),
            group_seq: 0,
            in_master: false,
        };
        // Master decorative shapes + placeholders (master inherits to
        // every slide via this layout). Master placeholders are captured as
        // protos only — NOT emitted into the layout's elements (the layout
        // inherits them via the placeholder chain; duplicating would put two
        // `<p:ph type="title">` in the rebuilt layout spTree).
        if let Some(master) = master_part {
            if let Ok(master_el) = parse_part(zip, master) {
                if let Some(tree) = first(&master_el, "cSld").and_then(|c| first(c, "spTree")) {
                    let master_rels = layout_rels(zip, master);
                    lctx.in_master = true;
                    for child in tree.children.iter().filter_map(|n| n.as_element()) {
                        let mut generated = Vec::new();
                        walk_sp_tree_child(
                            child,
                            &master_rels,
                            &mut lctx,
                            None,
                            None,
                            true,
                            &mut generated,
                        )?;
                        elements.extend(generated.into_iter().flatten());
                    }
                    lctx.in_master = false;
                }
            }
        }
        // Layout decorative shapes + placeholders.
        if let Ok(layout_el) = parse_part(zip, layout_part) {
            if let Some(tree) = first(&layout_el, "cSld").and_then(|c| first(c, "spTree")) {
                let layout_rels = layout_rels(zip, layout_part);
                for child in tree.children.iter().filter_map(|n| n.as_element()) {
                    let mut generated = Vec::new();
                    walk_sp_tree_child(
                        child,
                        &layout_rels,
                        &mut lctx,
                        None,
                        None,
                        true,
                        &mut generated,
                    )?;
                    elements.extend(generated.into_iter().flatten());
                }
            }
        }
        protos = std::mem::take(&mut lctx.layout_placeholders);
        groups = std::mem::take(&mut lctx.groups);
    }

    let groups = if groups.is_empty() {
        None
    } else {
        Some(groups)
    };

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
            groups,
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
    let Some(style) = first(sp_el, "style") else {
        return base;
    };
    let Some(fr) = first(style, "fontRef") else {
        return base;
    };
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
/// the bullet glyph / hanging indent. WPS puts the bullet on each `<a:pPr>`
/// (not the lstStyle), so paragraph-level `buChar`/`buFont`/`marL`/`indent`
/// override the lstStyle defaults; `buNone` clears the bullet. The PPTD model
/// carries one bullet per `TextContent`, so the first paragraph with a bullet
/// spec wins (uniform bulleted lists — the common case).
fn lst_style_info(
    base: MasterDefaults,
    tx_body: &XmlEl,
    slots: &SlotColors,
) -> (MasterDefaults, BulletInfo) {
    let mut d = base;
    let mut bullet = BulletInfo::default();
    // 1. lstStyle lvl1pPr: run-style defaults + a fallback bullet.
    if let Some(lst) = first(tx_body, "lstStyle") {
        if let Some(lvl1) = first(lst, "lvl1pPr") {
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
        }
    }
    // 2. Paragraph-level `<a:pPr>` overrides (the common WPS case).
    for p in children(tx_body, "p") {
        let Some(pp) = first(p, "pPr") else {
            continue;
        };
        if first(pp, "buNone").is_some() {
            bullet = BulletInfo::default();
            continue;
        }
        if let Some(v) = attr(pp, "marL").and_then(|s| s.parse::<f64>().ok()) {
            bullet.margin = Some(px(v));
        }
        if let Some(v) = attr(pp, "indent").and_then(|s| s.parse::<f64>().ok()) {
            bullet.indent = Some(px(v));
        }
        if let Some(bf) = first(pp, "buFont").and_then(|e| attr(e, "typeface")) {
            bullet.font = Some(bf.to_string());
        }
        if let Some(bc) = first(pp, "buChar").and_then(|e| attr(e, "char")) {
            bullet.char = Some(bc.to_string());
        }
        break;
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
    Ok(first(&core, "title")
        .and_then(|el| el.get_text())
        .map(|c| c.trim().to_string())
        .filter(|s| !s.is_empty()))
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
    let c_sld = first(slide, "cSld")
        .ok_or_else(|| Error::Invalid(format!("slide part {part} has no p:cSld")))?;
    let sp_tree = first(c_sld, "spTree")
        .ok_or_else(|| Error::Invalid(format!("slide part {part} has no p:spTree")))?;

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
        walk_sp_tree_child(child, rels, ctx, None, None, false, &mut generated)?;
        elements.extend(generated.into_iter().flatten());
    }

    Ok(Page {
        page_type: None,
        layout: ctx.layout_key.clone(),
        background,
        notes: None,
        elements,
        animations: None,
        groups: if ctx.groups.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut ctx.groups))
        },
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
    group_id: Option<&str>,
    in_layout: bool,
    out: &mut Vec<Option<Element>>,
) -> Result<()> {
    match el.name.as_str() {
        "sp" => out.push(map_sp(el, rels, ctx, group, in_layout)?),
        "pic" => out.push(map_pic(el, rels, ctx, group)),
        "cxnSp" => out.push(map_line(el, ctx, group)),
        "grpSp" => {
            flatten_group(el, rels, ctx, group, group_id, None, in_layout, out)?;
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
            ctx.skip(
                &name,
                format!("{kind} elements are not representable in PPTD"),
            );
            out.push(None);
        }
        "oleObj" => {
            ctx.skip(
                cnv_name(el).unwrap_or_else(|| "oleObj".into()).as_str(),
                "embedded objects are not supported",
            );
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

#[allow(clippy::too_many_arguments)]
fn flatten_group(
    el: &XmlEl,
    rels: &BTreeMap<String, String>,
    ctx: &mut PageCtx<'_>,
    outer: Option<&Xform>,
    parent_id: Option<&str>,
    outer_fill: Option<&Fill>,
    in_layout: bool,
    out: &mut Vec<Option<Element>>,
) -> Result<()> {
    // The group's own fill (from `<p:grpSpPr>`, parsed first — a no-xfrm
    // group may still carry a fill its grpFill children inherit). A child
    // using `<a:grpFill>` inherits this; resolve it onto the member so the
    // PPTD (flat model, no group-fill concept) carries a concrete fill.
    let group_fill: Option<Fill> = first(el, "grpSpPr").and_then(|gpr| {
        if first(gpr, "noFill").is_some() {
            return None;
        }
        if first(gpr, "grpFill").is_some() {
            return outer_fill.cloned();
        }
        if let Some(solid) = first(gpr, "solidFill") {
            return color_from_fill(solid, ctx.slots).map(|color| Fill::Solid { color });
        }
        if let Some(grad) = first(gpr, "gradFill") {
            return gradient_from(grad, ctx.slots);
        }
        None
    });
    let Some(grel) = first(el, "grpSpPr").and_then(|p| first(p, "xfrm")) else {
        // No `<a:xfrm>` → passthrough: this group adds no transform. Walk its
        // children with the outer transform + parent_id unchanged so members
        // join the enclosing group (or stay top-level if there is none).
        // WPS emits such header groups inside enlarging card groups; without
        // this passthrough their Freeform+TextBox banner would be dropped.
        for child in el.children.iter().filter_map(|n| n.as_element()) {
            match child.name.as_str() {
                "nvGrpSpPr" | "grpSpPr" => continue,
                "grpSp" => {
                    flatten_group(
                        child,
                        rels,
                        ctx,
                        outer,
                        parent_id,
                        group_fill.as_ref(),
                        in_layout,
                        out,
                    )?;
                }
                _ => {
                    let before = out.len();
                    walk_sp_tree_child(child, rels, ctx, outer, parent_id, in_layout, out)?;
                    for elem in out[before..].iter_mut().flatten() {
                        if let Some(parent) = parent_id {
                            elem.common_mut().group_id = Some(parent.to_owned());
                            if let Some(sp_pr) = first(child, "spPr") {
                                if let Some(xel) = first(sp_pr, "xfrm") {
                                    elem.common_mut().group_bounds =
                                        Some(to_bounds(&Xform::parse(xel)));
                                }
                            }
                        }
                        if let Some(sp_pr) = first(child, "spPr") {
                            if first(sp_pr, "grpFill").is_some() {
                                if let Element::Shape(shape) = elem {
                                    if shape.fill.is_none() {
                                        shape.fill = group_fill.clone();
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        return Ok(());
    };
    let raw = Xform::parse(grel);
    let g = raw.apply(outer);
    // Classify the group. WPS emits two flavours worth distinguishing:
    //  - enlarging stub (`chExt` sub-pixel, `ext` normal): an artifact whose
    //    children are tiny stubs scaled up to full size. PRESERVE as a real
    //    `<p:grpSp>` so WPS's group selection box matches the source.
    //  - everything else (normal groups AND shrinking/zero-size groups):
    //    FLATTEN. A top-level shrinking group composes to ~0-size children
    //    (invisible, matching the source); a shrinking group nested inside an
    //    enlarging stub composes to full-size children (e.g. the card header
    //    banner + title). Keeping these grouped breaks grouped-custGeom
    //    rendering in QuickLook/WPS, so flatten always.
    let ext_sub = raw.ext.0.abs() < 12700.0 && raw.ext.1.abs() < 12700.0;
    let ch_sub = raw.ch_ext.0.abs() < 12700.0 && raw.ch_ext.1.abs() < 12700.0;
    let enlarging_stub = ch_sub && !ext_sub;
    if enlarging_stub {
        // PRESERVE: register the group + tag members (verbatim child-space
        // xfrm) so the writer rebuilds the `<p:grpSp>`.
        let group_id = ctx.next_group_id();
        ctx.groups.insert(
            group_id.clone(),
            GroupDef {
                xfrm: GroupXfrm {
                    off: (px(raw.off.0), px(raw.off.1)),
                    ext: (px(raw.ext.0), px(raw.ext.1)),
                    ch_off: (px(raw.ch_off.0), px(raw.ch_off.1)),
                    ch_ext: (px(raw.ch_ext.0), px(raw.ch_ext.1)),
                    rot: raw.rot,
                    flip: raw.flip.filter(|(h, v)| *h || *v),
                },
                name: cnv_name(el),
                fill: group_fill.clone(),
                parent: parent_id.map(str::to_owned),
            },
        );
        for child in el.children.iter().filter_map(|n| n.as_element()) {
            match child.name.as_str() {
                "nvGrpSpPr" | "grpSpPr" => continue,
                "grpSp" => {
                    flatten_group(
                        child,
                        rels,
                        ctx,
                        Some(&g),
                        Some(&group_id),
                        group_fill.as_ref(),
                        in_layout,
                        out,
                    )?;
                }
                _ => {
                    let before = out.len();
                    walk_sp_tree_child(
                        child,
                        rels,
                        ctx,
                        Some(&g),
                        Some(&group_id),
                        in_layout,
                        out,
                    )?;
                    for elem in out[before..].iter_mut().flatten() {
                        elem.common_mut().group_id = Some(group_id.clone());
                        if let Some(sp_pr) = first(child, "spPr") {
                            if let Some(xel) = first(sp_pr, "xfrm") {
                                elem.common_mut().group_bounds =
                                    Some(to_bounds(&Xform::parse(xel)));
                            }
                            if first(sp_pr, "grpFill").is_some() {
                                if let Element::Shape(shape) = elem {
                                    if shape.fill.is_none() {
                                        shape.fill = group_fill.clone();
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        return Ok(());
    }
    // FLATTEN (normal group): emit children as top-level slide-space shapes
    // (composed xfrm applied), resolving `<a:grpFill>` against this group's
    // fill. No group is registered.
    for child in el.children.iter().filter_map(|n| n.as_element()) {
        match child.name.as_str() {
            "nvGrpSpPr" | "grpSpPr" => continue,
            "grpSp" => {
                flatten_group(
                    child,
                    rels,
                    ctx,
                    Some(&g),
                    parent_id,
                    group_fill.as_ref(),
                    in_layout,
                    out,
                )?;
            }
            _ => {
                let before = out.len();
                walk_sp_tree_child(child, rels, ctx, Some(&g), None, in_layout, out)?;
                for elem in out[before..].iter_mut().flatten() {
                    if let Some(sp_pr) = first(child, "spPr") {
                        if first(sp_pr, "grpFill").is_some() {
                            if let Element::Shape(shape) = elem {
                                if shape.fill.is_none() {
                                    shape.fill = group_fill.clone();
                                }
                            }
                        }
                    }
                }
            }
        }
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
            rot: attr(xfrm, "rot")
                .and_then(|s| s.parse().ok())
                .map(|v: i64| v as f64 / 60000.0),
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
        group_id: None,
        group_bounds: None,
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
    first(nv, "cNvPr")
        .and_then(|p| attr(p, "name"))
        .map(str::to_string)
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
    // After capturing the proto, fall through so the placeholder's prompt
    // text + lstStyle are also captured as a Text element in the layout's
    // spTree — the writer re-emits it as `<p:ph type>` so slide placeholders
    // inherit the prompt + style (e.g. the empty slide title shows the
    // layout's "单击此处编辑标题" in blue/bold 24pt, not the master default).
    if in_layout {
        if let Some(ph_el) = ph {
            if let Some(proto) = placeholder_proto(el, ctx.slots) {
                ctx.layout_placeholders.insert(ph_key(ph_el), proto);
            }
            // Master placeholders are captured as protos only — the layout
            // inherits them; emitting them into the layout spTree would
            // duplicate (e.g. two `<p:ph type="title">`).
            if ctx.in_master {
                return Ok(None);
            }
        }
    }
    let ext = nv_pr
        .and_then(|p| first(p, "extLst"))
        .and_then(|lst| {
            children_in(lst, "ext")
                .into_iter()
                .find(|e| attr(e, "uri") == Some(PPTD_ICON_URI))
        })
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
    let inherited_proto: Option<PlaceholderProto> =
        ph.and_then(|ph_el| lookup_placeholder(&ctx.layout_placeholders, &ph_key(ph_el)).cloned());
    if let Some(proto) = &inherited_proto {
        inherited_defaults = Some(proto.defaults.clone());
    }
    // Vertical anchor inherited from the layout placeholder template (used
    // after `inherited_proto` is consumed by the geometry match below).
    let proto_anchor = inherited_proto.as_ref().and_then(|p| p.anchor);
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
        // Inherited paragraph line spacing for placeholder text: the layout
        // placeholder's `lstStyle lnSpc` wins; otherwise the master
        // `bodyStyle` value (or `titleStyle` for title placeholders). Plain
        // text boxes keep the consumer's default — they never inherited a
        // paragraph style from the master chain.
        let para_line_spacing = if ph.is_some() {
            inherited_defaults
                .as_ref()
                .and_then(|d| d.line_spacing)
                .or_else(|| {
                    if ph
                        .as_ref()
                        .and_then(|p| attr(p, "type"))
                        .is_some_and(|t| t == "title")
                    {
                        ctx.defaults.title_line_spacing.or(ctx.master_line_spacing)
                    } else {
                        ctx.master_line_spacing
                    }
                })
        } else {
            None
        };
        let para_margin_top = if ph.is_some() {
            inherited_defaults
                .as_ref()
                .and_then(|d| d.spc_bef)
                .or_else(|| {
                    if ph
                        .as_ref()
                        .and_then(|p| attr(p, "type"))
                        .is_some_and(|t| t == "title")
                    {
                        ctx.defaults.title_spc_bef.or(ctx.master_spc_bef)
                    } else {
                        ctx.master_spc_bef
                    }
                })
        } else {
            None
        };
        let saved_defaults = ctx.defaults.clone();
        ctx.defaults = base;
        let saved_para = ctx.para_line_spacing;
        ctx.para_line_spacing = para_line_spacing;
        let saved_mtop = ctx.para_margin_top;
        ctx.para_margin_top = para_margin_top;
        let mut content = text_content(tx_body, ctx.slots, ctx, &name);
        ctx.para_margin_top = saved_mtop;
        ctx.para_line_spacing = saved_para;
        ctx.defaults = saved_defaults;
        // Placeholders that omit their own `<a:bodyPr anchor=…>` inherit
        // the layout placeholder template's vertical anchor (title templates
        // are `anchor="ctr"`); without it the box would sit top-aligned.
        // `text_content` already fills `align.vertical` with a default `Top`
        // when the bodyPr carries no anchor, so we can't gate on
        // `c.align.is_none()` — re-check the source bodyPr and override the
        // vertical only when it had no explicit anchor.
        let body_anchor_explicit = first(el, "txBody")
            .and_then(|tb| first(tb, "bodyPr"))
            .and_then(|bp| attr(bp, "anchor"))
            .is_some();
        if let (Some(anchor), Some(c)) = (proto_anchor, content.as_mut()) {
            if !body_anchor_explicit {
                match c.align.as_mut() {
                    Some(a) => a.vertical = anchor,
                    None => {
                        c.align = Some(Alignment {
                            horizontal: HorizontalAlign::default(),
                            vertical: anchor,
                        });
                    }
                }
            }
        }
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
            let placeholder = ph
                .as_ref()
                .map(|p| attr(p, "type").unwrap_or("body").to_string());
            return Ok(Some(Element::Text(Box::new(Text {
                common: common(&x, id),
                content,
                fill,
                border,
                placeholder,
            }))));
        }
        // Empty placeholder (e.g. an unfilled `<p:ph type="title"/>`):
        // keep it as a placeholder shape so the renderer shows the prompt
        // ("Click to edit title"). WPS only renders the prompt for typed
        // `<p:ph>` placeholders with an empty txBody.
        if ph.is_some() {
            let id = ctx.unique_id(&name, "text");
            let placeholder = ph
                .and_then(|p| attr(p, "type"))
                .unwrap_or("body")
                .to_string();
            return Ok(Some(Element::Text(Box::new(Text {
                common: common(&x, id),
                content: TextContent::default(),
                fill: None,
                border: None,
                placeholder: Some(placeholder),
            }))));
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

    // Drop invisible leftovers (no fill, no border, no shadow, no text).
    // Such a shape contributes nothing to the paint order, so deleting it is
    // lossless. A shadowed shape (outer or inner) is kept — the effect is
    // visible even without a fill/border.
    if let Some(Element::Shape(shape)) = &element {
        if shape.fill.is_none() && shape.border.is_none() && shape.shadow.is_none() {
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
    // Prefer outerShdw; fall back to innerShdw (renders inside the shape).
    let (shdw, inner) = match first(effects, "outerShdw") {
        Some(s) => (s, false),
        None => (first(effects, "innerShdw")?, true),
    };
    let blur = attr(shdw, "blurRad")
        .and_then(|s| s.parse::<f64>().ok())
        .map(px)
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
    let inner = if inner { Some(true) } else { None };
    Some(Shadow {
        blur,
        color,
        offset,
        scale,
        inner,
    })
}

fn border_from_el(sp_el: &XmlEl, slots: &SlotColors) -> Option<Border> {
    // Explicit `<a:ln>` in spPr takes precedence; `<a:noFill>` inside it
    // means "no line" and must NOT fall back to the style lnRef.
    if let Some(sp_pr) = first(sp_el, "spPr") {
        if let Some(ln) = first(sp_pr, "ln") {
            if first(ln, "noFill").is_some() {
                return None;
            }
            let width = attr(ln, "w")
                .and_then(|s| s.parse::<i64>().ok())
                .map(|v| px(v as f64));
            let style = first(ln, "prstDash")
                .and_then(|d| attr(d, "val"))
                .map(|v| match v {
                    "dash" | "sysDash" | "dashDot" | "lgDash" | "lgDashDot" | "lgDashDotDot"
                    | "sysDashDot" => LineStyle::Dash,
                    "dot" | "sysDot" => LineStyle::Dot,
                    _ => LineStyle::Solid,
                });
            // A gradient outline is captured verbatim; a solid colour
            // directly. A `<a:ln>` with neither a solidFill nor a gradFill is
            // still preserved as an empty line element (PowerPoint colourises
            // it through `p:style > lnRef`, e.g. the 1pt frame on slide-2
            // label cards) instead of being dropped.
            let gradient = first(ln, "gradFill")
                .and_then(|g| gradient_from(g, slots))
                .and_then(|f| match f {
                    Fill::Gradient {
                        gradient_type,
                        stops,
                        angle,
                        scaled,
                    } => Some(GradientFill {
                        gradient_type,
                        stops,
                        angle,
                        scaled,
                    }),
                    _ => None,
                });
            let color = first(ln, "solidFill").and_then(|s| color_from_fill(s, slots));
            let mut border = Border {
                style,
                width,
                color,
                gradient,
            };
            // A line with neither a solidFill nor a gradFill is colourised
            // by Office through `<p:style><a:lnRef …>` (slide-2 label cards
            // use idx=3 → lt1, a white 1pt frame). Resolve that reference
            // into an explicit colour so PowerPoint _and_ LibreOffice draw
            // the same white frame instead of a renderer-default tint.
            if border.color.is_none() && border.gradient.is_none() {
                if let Some(ln_ref) = ln_ref_border(sp_el, slots) {
                    border.color = ln_ref.color;
                    border.width = border.width.or(ln_ref.width);
                    border.style = border.style.or(ln_ref.style);
                }
            }
            return Some(border);
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
                let [Some(a), Some(b), Some(c)] = args.as_slice() else {
                    return Err(format!("bad */ fmla `{fmla}`"));
                };
                Some(a * b / c)
            }
            "+/" => {
                let [Some(a), Some(b), Some(c)] = args.as_slice() else {
                    return Err(format!("bad +/ fmla `{fmla}`"));
                };
                Some((a + b) / c)
            }
            "+-" => {
                let [Some(a), Some(b), Some(c)] = args.as_slice() else {
                    return Err(format!("bad +- fmla `{fmla}`"));
                };
                Some(a + b - c)
            }
            "--" => {
                let [Some(a), Some(b), Some(c)] = args.as_slice() else {
                    return Err(format!("bad -- fmla `{fmla}`"));
                };
                Some(a - b - c)
            }
            "min" => {
                let [Some(a), Some(b)] = args.as_slice() else {
                    return Err(format!("bad min fmla `{fmla}`"));
                };
                Some(a.min(*b))
            }
            "max" => {
                let [Some(a), Some(b)] = args.as_slice() else {
                    return Err(format!("bad max fmla `{fmla}`"));
                };
                Some(a.max(*b))
            }
            "abs" => args.first().copied().flatten().map(f64::abs),
            "sqrt" => args.first().copied().flatten().map(f64::sqrt),
            "sin" | "cos" | "tan" => {
                // `sin ang hd2 y` → sin(ang) * hd2 + y, ang in 60000ths of a
                // degree.
                let [Some(a), Some(b), Some(c)] = args.as_slice() else {
                    return Err(format!("bad {op} fmla `{fmla}`"));
                };
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
                let [Some(y), Some(x)] = args.as_slice() else {
                    return Err(format!("bad cat2 fmla `{fmla}`"));
                };
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
            let w = p
                .attributes
                .get("w")
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(100.0);
            let h = p
                .attributes
                .get("h")
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(100.0);
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
    for gd in first(geom, "gdLst")
        .into_iter()
        .flat_map(|lst| lst.children.iter().filter_map(|n| n.as_element()))
    {
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
        let w = p
            .attributes
            .get("w")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(rect.2 - rect.0);
        let h = p
            .attributes
            .get("h")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(rect.3 - rect.1);
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
                            rnd(pts[0].0),
                            rnd(pts[0].1),
                            rnd(pts[1].0),
                            rnd(pts[1].1),
                            rnd(pts[2].0),
                            rnd(pts[2].1)
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
                        d.push_str(&format!(
                            "Q {} {} {} {}",
                            rnd(pts[0].0),
                            rnd(pts[0].1),
                            rnd(pts[1].0),
                            rnd(pts[1].1)
                        ));
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
    let gx = x_raw
        .as_deref()
        .and_then(|s| s.parse::<f64>().ok())
        .or_else(|| guides.get(x_raw.as_deref()?));
    let gy = y_raw
        .as_deref()
        .and_then(|s| s.parse::<f64>().ok())
        .or_else(|| guides.get(y_raw.as_deref()?));
    match (gx, gy) {
        (Some(x), Some(y)) => Ok((x, y)),
        _ => Err(format!(
            "path point references an unknown guide (x={x_raw:?} y={y_raw:?})"
        )),
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
                ctx.skip(
                    &name,
                    format!("media extension .{ext} is not supported by the builder"),
                );
                return None;
            }
            let basename = target.rsplit('/').next().unwrap_or(target).to_string();
            // `rels` already holds resolved archive paths (`ppt/media/imageN.png`).
            let part = target.clone();
            ctx.media
                .entry(part.clone())
                .or_insert_with(|| basename.clone());
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
    // Preserve the picture's clip geometry (`prstGeom prst="ellipse"` etc.
    // — round avatar badges / logo tiles) plus its soft-edge effect, so the
    // rebuild clips the picture the same way instead of a plain rectangle.
    let crop_shape = sp_pr
        .and_then(|p| first(p, "prstGeom"))
        .and_then(|g| attr(g, "prst"))
        .filter(|prst| *prst != "rect" && *prst != "custom")
        .map(|prst| ShapeDef {
            shape_name: prst.to_string(),
            adjustments: None,
            view_box: None,
            path: None,
        });
    let soft_edge = sp_pr
        .and_then(|p| first(p, "effectLst"))
        .and_then(|e| first(e, "softEdge"))
        .and_then(|s| attr(s, "rad"))
        .and_then(|v| v.parse::<f64>().ok())
        .map(|emu| emu / EMU_PER_PX);
    Some(Element::Image(Image {
        common: common(&x, id),
        src,
        crop_shape,
        fit,
        crop,
        border: None,
        shadow: None,
        soft_edge,
    }))
}

/// Map `a:srcRect` ppm values to PPTD fit/crop.
fn src_rect_fit(
    blip_fill: Option<&XmlEl>,
    _ctx: &mut PageCtx<'_>,
) -> (Option<ImageFit>, Option<crate::pptd::shared::ImageCrop>) {
    let Some(rect) = blip_fill.and_then(|b| first(b, "srcRect")) else {
        // Office stretches pictures by default; PPTD default would crop.
        return (
            Some(ImageFit {
                mode: ImageFitMode::Fill,
            }),
            None,
        );
    };
    let v = |k: &str| {
        attr(rect, k)
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    let (l, t, r, b) = (v("l"), v("t"), v("r"), v("b"));
    let f = |n: f64| n / 100000.0;
    if l != 0.0 || t != 0.0 || r != 0.0 || b != 0.0 {
        if (t < 0.0 || b < 0.0) && !(l > 0.0 || r > 0.0) {
            // Negative crops pad the slack axis → contain.
            (
                Some(ImageFit {
                    mode: ImageFitMode::Contain,
                }),
                None,
            )
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
            (
                Some(ImageFit {
                    mode: ImageFitMode::Fill,
                }),
                crop,
            )
        }
    } else {
        (
            Some(ImageFit {
                mode: ImageFitMode::Fill,
            }),
            None,
        )
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
    first(el, "alpha")
        .and_then(|a| attr(a, "val"))
        .and_then(|s| s.parse::<f64>().ok())
        .map(|v| v / 100000.0)
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
    for gs in first(grad, "gsLst")?
        .children
        .iter()
        .filter_map(|n| n.as_element())
    {
        if gs.name != "gs" {
            continue;
        }
        let pos = attr(gs, "pos")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0)
            / 100000.0;
        let color = color_from_fill(gs, slots)?;
        stops.push(crate::pptd::shared::ColorStop {
            position: pos,
            color,
        });
    }
    if stops.is_empty() {
        return None;
    }
    if let Some(lin) = first(grad, "lin") {
        let angle = attr(lin, "ang")
            .and_then(|s| s.parse::<f64>().ok())
            .map(|a| a / 60000.0);
        let scaled = attr(lin, "scaled") == Some("1");
        Some(Fill::Gradient {
            gradient_type: GradientType::Linear,
            stops,
            angle,
            scaled,
        })
    } else if first(grad, "path").is_some() {
        Some(Fill::Gradient {
            gradient_type: GradientType::Radial,
            stops,
            angle: None,
            scaled: false,
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
    let wrap = body_pr.and_then(|b| attr(b, "wrap")).map(|w| w != "none");
    let text_direction = body_pr.and_then(|b| attr(b, "vert")).and_then(|v| match v {
        "eaVert" | "vert" | "vert270" => Some(TextDirection::Vertical),
        // `horz` is the default (and previously a bug turned it into
        // Vertical: `matches!() as u8` yielded 0 which the `.map(_ => ..)`
        // then ignored) — keep the PPTD honest here.
        _ => None,
    });
    // Auto-fit: `spAutoFit` / `normAutofit` / `noAutofit` children of bodyPr.
    let autofit = body_pr.and_then(|b| {
        if first(b, "spAutoFit").is_some() {
            Some(TextAutofit::FitShape)
        } else if first(b, "normAutofit").is_some() {
            Some(TextAutofit::FitText)
        } else if first(b, "noAutofit").is_some() {
            Some(TextAutofit::Fixed)
        } else {
            None
        }
    });
    // Preserve unfamilar `a:bodyPr` attributes verbatim (`vertOverflow`,
    // `horzOverflow`, `numCol`, `spcCol`, `fromWordArt`, `anchorCtr`,
    // `forceAA`, `compatLnSpc`, …). These tweak PowerPoint/WPS text layout
    // (most notably the vertical text position inside an anchored box), and
    // dropping them made rebuilds align differently in those renderers.
    let body_pr_extras = body_pr
        .map(|b| {
            b.attributes
                .iter()
                .filter(|(k, _)| {
                    !matches!(
                        k.as_str(),
                        "lIns" | "tIns" | "rIns" | "bIns" | "wrap" | "rtlCol" | "anchor" | "vert"
                    )
                })
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<BTreeMap<_, _>>()
        })
        .filter(|m| !m.is_empty());
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
        let algn = p_pr.and_then(|pp| attr(pp, "algn")).map(|a| match a {
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
            .map(|v| v / 100000.0)
            // Placeholder paragraphs inherit the master/layout `lnSpc`
            // when their own `<a:pPr>` carries none (OOXML chain).
            .or(ctx.para_line_spacing);
        let mt = p_pr
            .and_then(|pp| first(pp, "spcBef"))
            .and_then(|b| first(b, "spcPts"))
            .and_then(|s| attr(s, "val"))
            .and_then(|v| v.parse::<f64>().ok())
            .map(|v| v / 100.0) // spcPts/100 → px
            // Placeholder paragraphs inherit the master/layout `spcBef`
            // when their own `<a:pPr>` carries none (OOXML chain).
            .or(ctx.para_margin_top);
        let mut runs: Vec<(Option<RunStyle>, String)> = Vec::new();
        for child in p.children.iter().filter_map(|n| n.as_element()) {
            match child.name.as_str() {
                "r" => {
                    let t = first(child, "t")
                        .and_then(|t| t.get_text())
                        .map(|c| c.to_string())
                        .unwrap_or_default();
                    let st =
                        RunStyle::from_rpr(first(child, "rPr"), slots, &para_defaults, &ctx.fonts);
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
    // Line height is only carried when the source paragraph set an explicit
    // `<a:lnSpc>`. A box without one renders at the renderer's natural
    // line height (QuickLook/WPS ≈ 1.2× font), and emitting `spcPct=100000`
    // would force a tight 1.0× that doesn't match the source.
    let line_height = first.line_height;
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
            if let Some(lh) = l.line_height {
                style_parts.push(format!("line-height:{lh}"));
            }
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
                if let Some(FontFamily::Single(n)) = st.as_ref().and_then(|s| s.font_family.clone())
                {
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
                    inner = format!("<span style=\"{}\">{inner}</span>", span_style.join("; "));
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
        body_pr_extras,
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
        let explicit_bold = attr(rpr, "b")
            .and_then(|s| s.parse::<u8>().ok())
            .map(|v| v != 0);
        let explicit_italic = attr(rpr, "i")
            .and_then(|s| s.parse::<u8>().ok())
            .map(|v| v != 0);

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
    let mut reader = zip
        .by_name(part)
        .map_err(|e| Error::Invalid(format!("part {part} missing in package: {e}")))?;
    let mut data = Vec::new();
    reader
        .read_to_end(&mut data)
        .map_err(|e| Error::Invalid(format!("read part {part}: {e}")))?;
    XmlEl::parse(data.as_slice()).map_err(|e| Error::Invalid(format!("parse XML in {part}: {e}")))
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
            line_spacing: None,
            title_line_spacing: None,
            spc_bef: None,
            title_spc_bef: None,
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
        let rpr = parse(
            r#"<rPr sz="4000" b="1"><solidFill><srgbClr val="0D37D4"/></solidFill><latin typeface="微软雅黑"/></rPr>"#,
        );
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

    #[test]
    fn master_body_title_line_spacing_is_captured() {
        // A WPS/Office master whose bodyStyle/titleStyle carry `lnSpc 90%`
        // (spcPct 90000): the importer must carry the fraction so slides
        // that omit `<a:lnSpc>` on placeholder paragraphs rebuild with the
        // same 0.9 spacing instead of the consumer's 100% default.
        let master_xml = r#"<?xml version="1.0"?><p:sldMaster xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
<p:txStyles>
  <p:titleStyle>
    <a:lvl1pPr><a:lnSpc><a:spcPct val="90000"/></a:lnSpc><a:defRPr sz="4400"/></a:lvl1pPr>
  </p:titleStyle>
  <p:bodyStyle>
    <a:lvl1pPr><a:lnSpc><a:spcPct val="90000"/></a:lnSpc><a:defRPr sz="2800"/></a:lvl1pPr>
    <a:lvl2pPr><a:lnSpc><a:spcPct val="90000"/></a:lnSpc><a:defRPr sz="2400"/></a:lvl2pPr>
  </p:bodyStyle>
  <p:otherStyle>
    <a:lvl1pPr><a:defRPr sz="1800"/></a:lvl1pPr>
  </p:otherStyle>
</p:txStyles>
</p:sldMaster>"#;
        let path = std::env::temp_dir().join("slideforge-master-defaults-test.pptx");
        use std::io::Write as _;
        let mut w = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        w.start_file(
            "ppt/slideMasters/slideMaster1.xml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        w.write_all(master_xml.as_bytes()).unwrap();
        w.finish().unwrap();
        // p:sldMaster -> path ppt/slideMasters/slideMaster1.xml
        let file = std::fs::File::open(&path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let d = master_defaults(
            &mut archive,
            "ppt/slideMasters/slideMaster1.xml",
            &SlotColors::new(),
        );
        let _ = std::fs::remove_file(&path);
        assert_eq!(d.sz, Some(18.0), "otherStyle defRPr sz=1800 → 18pt");
        assert_eq!(
            d.line_spacing,
            Some(0.9),
            "bodyStyle lvl1pPr lnSpc 90000 → 0.9"
        );
        assert_eq!(
            d.title_line_spacing,
            Some(0.9),
            "titleStyle lvl1pPr lnSpc 90000 → 0.9"
        );
    }

    #[test]
    fn placeholder_proto_captures_body_pr_anchor() {
        // Title placeholder templates carry `anchor="ctr"`; slide title
        // placeholders with an empty `<a:bodyPr/>` must inherit that vertical
        // anchor instead of defaulting to top.
        let el = parse(
            r#"<?xml version="1.0"?><p:sp xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:nvSpPr><p:cNvPr name="Title Placeholder"/></p:nvSpPr>
<p:spPr><a:xfrm><a:off x="1000" y="1000"/><a:ext cx="5000" cy="2000"/></a:xfrm><a:prstGeom prst="rect"/></p:spPr>
<p:txBody><a:bodyPr anchor="ctr" lIns="91440" tIns="45720" rIns="91440" bIns="45720"/><a:p><a:r><a:t>x</a:t></a:r></a:p></p:txBody></p:sp>"#,
        );
        let proto = placeholder_proto(&el, &SlotColors::new()).expect("constant placeholder proto");
        assert_eq!(proto.anchor, Some(VerticalAlign::Middle));
        // And a template without an anchor stays top (default).
        let el2 = parse(
            r#"<?xml version="1.0"?><p:sp xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:nvSpPr><p:cNvPr name="Body Placeholder"/></p:nvSpPr>
<p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1000" cy="1000"/></a:xfrm></p:spPr>
<p:txBody><a:bodyPr/></p:txBody></p:sp>"#,
        );
        let proto2 =
            placeholder_proto(&el2, &SlotColors::new()).expect("constant placeholder proto");
        assert_eq!(proto2.anchor, None);
    }
}
