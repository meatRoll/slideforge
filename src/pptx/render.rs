//! Element → drawingml rendering inside a slide's `cSld/spTree`.
//!
//! Supported: `text`, `shape`, `line`, `icon` (Font Awesome glyphs as custom
//! geometry) and `image` (blip media parts). Unsupported (`table`, `chart`)
//! fail with an explicit [`Error::Unsupported`]; shape shadows are currently
//! dropped silently (documented limitation).

use crate::pptd::shared::{
    Alignment, Border, Bounds, Color, Fill, FontFamily, GradientType, HorizontalAlign,
    ImageFitMode, LineStyle, VerticalAlign,
};
use crate::pptd::{
    Element, Icon, Image, Line, LineCurve, Shape, Text, TextContent, TextDirection, Theme,
};
use crate::{Error, Result};

use super::media::ImageSize;
use super::theme::{ResolvedColor, resolve_color};
use super::xml::Xml;
use super::{fa, svg_path};

/// Rendering context shared while painting one slide.
pub struct RenderCtx<'a> {
    pub theme: Option<&'a Theme>,
    shape_id: usize,
    /// Media sources used by the current slide, in usage order.
    pub media: Vec<String>,
    /// Media source → pixel dimensions (for `contain`/`cover` cropping).
    pub image_sizes: Vec<(String, ImageSize)>,
}

impl<'a> RenderCtx<'a> {
    pub fn new(theme: Option<&'a Theme>) -> Self {
        Self {
            theme,
            shape_id: 1,
            media: Vec::new(),
            image_sizes: Vec::new(),
        }
    }

    fn next_id(&mut self) -> usize {
        self.shape_id += 1;
        self.shape_id
    }

    /// Relationship id for a media source: `rId2` onwards (rId1 = layout).
    pub fn media_rid(&self, src: &str) -> Option<String> {
        self.media
            .iter()
            .position(|s| s == src)
            .map(|p| format!("rId{}", p + 2))
    }

    pub fn image_size(&self, src: &str) -> Option<ImageSize> {
        self.image_sizes
            .iter()
            .find(|(s, _)| s == src)
            .map(|(_, size)| *size)
    }
}

/// Render one page element into the slide's `spTree`.
pub fn render_element(
    xml: &mut Xml,
    ctx: &mut RenderCtx<'_>,
    element: &Element,
    page_index: usize,
) -> Result<()> {
    match element {
        Element::Text(text) => render_text(xml, ctx, text, page_index),
        Element::Shape(shape) => render_shape(xml, ctx, shape),
        Element::Line(line) => render_line(xml, ctx, line),
        Element::Icon(icon) => render_icon(xml, ctx, icon),
        Element::Image(image) => render_image(xml, ctx, image),
        other => Err(Error::Unsupported(format!(
            "element `{}` of type `{}` on page {} is not supported yet \
             (supported so far: text, shape, line, icon, image)",
            other.element_id(),
            other.type_name(),
            page_index + 1
        ))),
    }
}

/// px → EMU (1px = 1pt = 12700 EMU).
pub fn emu(px: f64) -> i64 {
    (px * 12700.0).round() as i64
}

/// `a:xfrm` from bounds / rotation / flip.
/// `a:xfrm` from bounds / rotation / flip. `rot` / `flipH` / `flipV` are
/// attributes of the transform (CT_Transform2D), never child elements.
fn xfrm(xml: &mut Xml, bounds: Bounds, rotation: Option<f64>, flip: Option<(bool, bool)>) {
    let x = emu(bounds.x).to_string();
    let y = emu(bounds.y).to_string();
    let cx = emu(bounds.width).to_string();
    let cy = emu(bounds.height).to_string();
    let rot = rotation
        .filter(|deg| deg.abs() > 1e-9)
        .map(|deg| ((deg * 60000.0).round()).to_string());
    let (flip_h, flip_v) = flip.unwrap_or((false, false));

    let mut attrs: Vec<(&str, &str)> = Vec::new();
    if let Some(rot) = rot.as_deref() {
        attrs.push(("rot", rot));
    }
    if flip_h {
        attrs.push(("flipH", "1"));
    }
    if flip_v {
        attrs.push(("flipV", "1"));
    }
    xml.start("a:xfrm", &attrs);
    xml.leaf("a:off", &[("x", &x), ("y", &y)]);
    xml.leaf("a:ext", &[("cx", &cx), ("cy", &cy)]);
    xml.end("a:xfrm");
}

/// `srgbClr` optionally carrying an alpha override.
fn srgb(xml: &mut Xml, rgb: &str, alpha: Option<f64>) {
    xml.start("a:srgbClr", &[("val", rgb)]);
    if let Some(alpha) = alpha {
        let alpha = ((alpha * 100000.0).round() as u64).to_string();
        xml.leaf("a:alpha", &[("val", &alpha)]);
    }
    xml.end("a:srgbClr");
}

fn emit_fill_color(xml: &mut Xml, color: &ResolvedColor) {
    xml.start("a:solidFill", &[]);
    srgb(xml, &color.rgb, color.alpha);
    xml.end("a:solidFill");
}

/// `a:ln` from a border spec (style / width / color).
/// `a:ln` from a border spec. Schema order inside `a:ln`: fill, then
/// `a:prstDash`, then the join. `prstDash` must never be an attribute.
fn border_xml(xml: &mut Xml, theme: Option<&Theme>, border: &Border) -> Result<()> {
    let width = emu(border.width.unwrap_or(1.0)).to_string();
    let dash: Option<&str> = border.style.map(|style| match style {
        LineStyle::Solid => "solid",
        LineStyle::Dash => "dash",
        LineStyle::Dot => "dot",
    });
    xml.start("a:ln", &[("w", &width)]);
    if let Some(color) = &border.color {
        let resolved = resolve_color(theme, color)?;
        emit_fill_color(xml, &resolved);
    } else {
        emit_fill_color(
            xml,
            &ResolvedColor {
                rgb: "000000".to_owned(),
                alpha: None,
            },
        );
    }
    if let Some(dash) = dash {
        xml.leaf("a:prstDash", &[("val", dash)]);
    }
    xml.end("a:ln");
    Ok(())
}

/// Fill (`solid` / `gradient` / `image`) → drawingml, optionally multiplied
/// by an element-level opacity.
pub fn fill_xml(
    xml: &mut Xml,
    theme: Option<&Theme>,
    fill: &Fill,
    opacity: Option<f64>,
) -> Result<()> {
    match fill {
        Fill::Solid { color } => {
            let mut resolved = resolve_color(theme, color)?;
            if let Some(op) = opacity {
                let base = resolved.alpha.unwrap_or(1.0);
                resolved.alpha = Some(base * op);
            }
            emit_fill_color(xml, &resolved);
        }
        Fill::Gradient {
            gradient_type,
            stops,
            angle,
        } => {
            xml.start("a:gradFill", &[("rotWithShape", "1")]);
            xml.start("a:gsLst", &[]);
            for stop in stops {
                let mut resolved = resolve_color(theme, &stop.color)?;
                if let Some(op) = opacity {
                    let base = resolved.alpha.unwrap_or(1.0);
                    resolved.alpha = Some(base * op);
                }
                let pos = ((stop.position * 100000.0).round() as u64).to_string();
                xml.start("a:gs", &[("pos", &pos)]);
                srgb(xml, &resolved.rgb, resolved.alpha);
                xml.end("a:gs");
            }
            xml.end("a:gsLst");
            match gradient_type {
                GradientType::Linear => {
                    let angle = (angle.unwrap_or(0.0) * 60000.0).round().to_string();
                    xml.start("a:lin", &[("ang", &angle), ("scaled", "0")]);
                    xml.end("a:lin");
                }
                GradientType::Radial => {
                    xml.start("a:path", &[("path", "circle")]);
                    xml.leaf(
                        "a:fillToRect",
                        &[
                            ("l", "50000"),
                            ("t", "50000"),
                            ("r", "50000"),
                            ("b", "50000"),
                        ],
                    );
                    xml.end("a:path");
                }
            }
            xml.end("a:gradFill");
        }
        Fill::Image { src, .. } => {
            return Err(Error::Unsupported(format!(
                "image fill `{src}` is not supported yet (only solid and gradient fills)"
            )));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

/// Effective text style after the inheritance chain
/// (content fields > `content.style` → theme.textStyles > defaults).
pub struct EffTextStyle {
    pub color: Option<Color>,
    pub font_size: Option<f64>,
    pub font_family: Option<FontFamily>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub line_height: Option<f64>,
    pub line_height_px: Option<f64>,
    pub align: Option<Alignment>,
    pub wrap: Option<bool>,
    pub direction: Option<TextDirection>,
}

pub fn effective_text_style(theme: Option<&Theme>, content: &TextContent) -> EffTextStyle {
    let base = content
        .style
        .as_deref()
        .and_then(|style| theme.and_then(|t| t.text_styles.get(style.trim_start_matches('$'))));
    EffTextStyle {
        color: content
            .color
            .clone()
            .or_else(|| base.and_then(|b| b.color.clone())),
        font_size: content.font_size.or_else(|| base.and_then(|b| b.font_size)),
        font_family: content
            .font_family
            .clone()
            .or_else(|| base.and_then(|b| b.font_family.clone())),
        bold: content.bold.or_else(|| base.and_then(|b| b.bold)),
        italic: content.italic.or_else(|| base.and_then(|b| b.italic)),
        line_height: content
            .line_height
            .or_else(|| base.and_then(|b| b.line_height)),
        line_height_px: content
            .line_height_px
            .or_else(|| base.and_then(|b| b.line_height_px)),
        align: content.align,
        wrap: content.wrap,
        direction: content.text_direction,
    }
}

fn render_text(
    xml: &mut Xml,
    ctx: &mut RenderCtx<'_>,
    text: &Text,
    page_index: usize,
) -> Result<()> {
    let id = ctx.next_id().to_string();
    let name = text.common.element_id.clone();
    let style = effective_text_style(ctx.theme, &text.content);

    // Rich text (`<p>`/`<span>`/`<strong>` ...) parsing is a later milestone;
    // flag it explicitly instead of emitting raw markup as literal text.
    if text.content.text.contains('<') {
        return Err(Error::Unsupported(format!(
            "rich text on element `{}` on page {} is not supported yet              (plain text only)",
            text.common.element_id,
            page_index + 1
        )));
    }

    xml.start("p:sp", &[]);
    xml.start("p:nvSpPr", &[]);
    xml.start("p:cNvPr", &[("id", &id), ("name", &name)]);
    xml.end("p:cNvPr");
    xml.leaf("p:cNvSpPr", &[("txBox", "1")]);
    xml.leaf("p:nvPr", &[]);
    xml.end("p:nvSpPr");

    xml.start("p:spPr", &[]);
    xfrm(
        xml,
        text.common.bounds,
        text.common.rotation,
        text.common.flip,
    );
    xml.start("a:prstGeom", &[("prst", "rect")]);
    xml.leaf("a:avLst", &[]);
    xml.end("a:prstGeom");
    // Text boxes must not inherit the automatic shape fill (kimi parity).
    xml.leaf("a:noFill", &[]);
    xml.end("p:spPr");

    let anchor = match style
        .align
        .map(|a| a.vertical)
        .unwrap_or(VerticalAlign::Top)
    {
        VerticalAlign::Top => "t",
        VerticalAlign::Middle => "ctr",
        VerticalAlign::Bottom => "b",
    };
    xml.start("p:txBody", &[]);
    let wrap = if style.wrap == Some(false) {
        "none"
    } else {
        "square"
    };
    xml.leaf("a:bodyPr", &[("wrap", wrap), ("anchor", anchor)]);
    xml.leaf("a:lstStyle", &[]);

    // Plain text: every line becomes one paragraph (the `<p>` equivalence).
    for line in text.content.text.split('\n') {
        xml.start("a:p", &[]);

        let align = style
            .align
            .map(|a| a.horizontal)
            .unwrap_or(HorizontalAlign::Left);
        let algn = match align {
            HorizontalAlign::Left => "l",
            HorizontalAlign::Center => "ctr",
            HorizontalAlign::Right => "r",
            HorizontalAlign::Justify => "just",
            HorizontalAlign::Distributed => "dist",
        };
        let line_height_pct: Option<String> = style
            .line_height
            .map(|multiple| ((multiple * 100000.0).round() as u64).to_string());
        let line_height_pts: Option<String> = style
            .line_height_px
            .map(|px| ((px * 100.0).round() as u64).to_string());

        let emit_line_spacing = line_height_pct.is_some() || line_height_pts.is_some();
        if algn != "l" || emit_line_spacing {
            xml.start("a:pPr", &[("algn", algn)]);
            if let Some(pct) = line_height_pct {
                xml.start("a:lnSpc", &[]);
                xml.leaf("a:spcPct", &[("val", &pct)]);
                xml.end("a:lnSpc");
            } else if let Some(pts) = line_height_pts {
                xml.start("a:lnSpc", &[]);
                xml.leaf("a:spcPts", &[("val", &pts)]);
                xml.end("a:lnSpc");
            }
            xml.end("a:pPr");
        }

        emit_run(xml, ctx.theme, &style, line);
        xml.end("a:p");
    }

    xml.end("p:txBody");
    xml.end("p:sp");
    Ok(())
}

fn emit_run(xml: &mut Xml, theme: Option<&Theme>, style: &EffTextStyle, text: &str) {
    let font_size = style.font_size.unwrap_or(18.0);
    let font_family = style
        .font_family
        .clone()
        .unwrap_or(FontFamily::Single("MiSans".to_owned()));
    let color = style.color.clone().unwrap_or(Color("#000000".to_owned()));

    let sz = (font_size * 100.0).round().to_string();
    let mut attrs: Vec<(&str, &str)> = vec![("lang", "en-US"), ("sz", &sz)];
    if style.bold == Some(true) {
        attrs.push(("b", "1"));
    }
    if style.italic == Some(true) {
        attrs.push(("i", "1"));
    }

    xml.start("a:r", &[]);
    xml.start("a:rPr", &attrs);
    let resolved = resolve_color(theme, &color).unwrap_or(ResolvedColor {
        rgb: "000000".to_owned(),
        alpha: None,
    });
    emit_fill_color(xml, &resolved);
    match &font_family {
        FontFamily::Single(name) => {
            xml.leaf("a:latin", &[("typeface", name)]);
            xml.leaf("a:ea", &[("typeface", name)]);
        }
        FontFamily::Bilingual { latin, ea } => {
            xml.leaf("a:latin", &[("typeface", latin)]);
            xml.leaf("a:ea", &[("typeface", ea)]);
        }
    }
    xml.end("a:rPr");
    xml.text_elem("a:t", text);
    xml.end("a:r");
}

// ---------------------------------------------------------------------------
// Shape
// ---------------------------------------------------------------------------

fn render_shape(xml: &mut Xml, ctx: &mut RenderCtx<'_>, shape: &Shape) -> Result<()> {
    let id = ctx.next_id().to_string();
    let name = shape.common.element_id.clone();
    if shape.shape_name == "custom" {
        return Err(Error::Unsupported(format!(
            "custom path geometry on element `{}` is not supported yet",
            shape.common.element_id
        )));
    }

    xml.start("p:sp", &[]);
    xml.start("p:nvSpPr", &[]);
    xml.start("p:cNvPr", &[("id", &id), ("name", &name)]);
    xml.end("p:cNvPr");
    xml.leaf("p:cNvSpPr", &[]);
    xml.leaf("p:nvPr", &[]);
    xml.end("p:nvSpPr");

    xml.start("p:spPr", &[]);
    xfrm(
        xml,
        shape.common.bounds,
        shape.common.rotation,
        shape.common.flip,
    );
    xml.start("a:prstGeom", &[("prst", &shape.shape_name)]);
    xml.start("a:avLst", &[]);
    if let Some(adjustments) = &shape.adjustments {
        for (i, &adj) in adjustments.iter().take(10).enumerate() {
            let name = if i == 0 {
                "adj".to_owned()
            } else {
                format!("adj{}", i + 1)
            };
            let value = (adj.round() as i64).to_string();
            let fmla = format!("val {value}");
            xml.leaf("a:gd", &[("name", &name), ("fmla", &fmla)]);
        }
    }
    xml.end("a:avLst");
    xml.end("a:prstGeom");
    if let Some(fill) = &shape.fill {
        fill_xml(xml, ctx.theme, fill, shape.common.opacity)?;
    }
    if let Some(border) = &shape.border {
        border_xml(xml, ctx.theme, border)?;
    }
    // TODO(shape shadow): map `Shape.shadow` to `a:effectLst > a:outerShdw`;
    // currently dropped (documented in the module docs).
    xml.end("p:spPr");

    xml.start("p:txBody", &[]);
    xml.leaf("a:bodyPr", &[]);
    xml.leaf("a:lstStyle", &[]);
    xml.start("a:p", &[]);
    xml.leaf("a:endParaRPr", &[("lang", "en-US")]);
    xml.end("a:p");
    xml.end("p:txBody");
    xml.end("p:sp");
    Ok(())
}

// ---------------------------------------------------------------------------
// Line
// ---------------------------------------------------------------------------

fn line_join(style: LineCurve) -> &'static str {
    match style {
        LineCurve::Sharp => "miter",
        LineCurve::Round => "round",
        LineCurve::Smooth => "round",
    }
}

/// Parse `"x,y x,y ..."` into points.
fn parse_points(points: &str) -> Result<Vec<(f64, f64)>> {
    points
        .split_whitespace()
        .map(|pair| {
            let (x, y) = pair.split_once(',').ok_or_else(|| {
                Error::Invalid(format!("line points `{points}`: `{pair}` is not `x,y`"))
            })?;
            let x = x
                .parse()
                .map_err(|_| Error::Invalid(format!("line points `{points}`: bad x `{x}`")))?;
            let y = y
                .parse()
                .map_err(|_| Error::Invalid(format!("line points `{points}`: bad y `{y}`")))?;
            Ok((x, y))
        })
        .collect()
}

fn render_line(xml: &mut Xml, ctx: &mut RenderCtx<'_>, line: &Line) -> Result<()> {
    let id = ctx.next_id().to_string();
    let name = line.common.element_id.clone();
    let points = parse_points(&line.points)?;
    if points.len() < 2 {
        return Err(Error::Invalid(format!(
            "line `{}` needs at least 2 points",
            line.common.element_id
        )));
    }

    xml.start("p:cxnSp", &[]);
    xml.start("p:nvCxnSpPr", &[]);
    xml.start("p:cNvPr", &[("id", &id), ("name", &name)]);
    xml.end("p:cNvPr");
    xml.leaf("p:cNvCxnSpPr", &[]);
    xml.leaf("p:nvPr", &[]);
    xml.end("p:nvCxnSpPr");

    xml.start("p:spPr", &[]);
    xfrm(
        xml,
        line.common.bounds,
        line.common.rotation,
        line.common.flip,
    );

    let path_w = emu(line.view_box.0).to_string();
    let path_h = emu(line.view_box.1).to_string();
    xml.start("a:custGeom", &[]);
    xml.leaf("a:avLst", &[]);
    xml.leaf("a:gdLst", &[]);
    xml.leaf("a:ahLst", &[]);
    xml.leaf("a:cxnLst", &[]);
    xml.leaf("a:rect", &[("l", "0"), ("t", "0"), ("r", "0"), ("b", "0")]);
    xml.start("a:pathLst", &[]);
    xml.start("a:path", &[("w", &path_w), ("h", &path_h)]);

    let smooth = matches!(line.curve, Some(LineCurve::Smooth));
    // Every point is shrunk to the viewBox; the element-level transform
    // (bounds) then scales it to the target geometry.
    let (first, rest) = points.split_first().expect("len >= 2 checked above");
    let first_x = emu(first.0).to_string();
    let first_y = emu(first.1).to_string();
    xml.start("a:moveTo", &[]);
    xml.leaf("a:pt", &[("x", &first_x), ("y", &first_y)]);
    xml.end("a:moveTo");

    if smooth {
        for chunk in rest.chunks_exact(3) {
            let p1 = emu(chunk[0].0).to_string();
            let p1y = emu(chunk[0].1).to_string();
            let p2 = emu(chunk[1].0).to_string();
            let p2y = emu(chunk[1].1).to_string();
            let p3 = emu(chunk[2].0).to_string();
            let p3y = emu(chunk[2].1).to_string();
            xml.start("a:cubicBezTo", &[]);
            xml.leaf("a:pt", &[("x", &p1), ("y", &p1y)]);
            xml.leaf("a:pt", &[("x", &p2), ("y", &p2y)]);
            xml.leaf("a:pt", &[("x", &p3), ("y", &p3y)]);
            xml.end("a:cTo");
        }
    } else {
        for (x, y) in rest {
            let x = emu(*x).to_string();
            let y = emu(*y).to_string();
            xml.start("a:lnTo", &[]);
            xml.leaf("a:pt", &[("x", &x), ("y", &y)]);
            xml.end("a:lnTo");
        }
    }
    xml.end("a:path");
    xml.end("a:pathLst");
    xml.end("a:custGeom");

    if let Some(border) = &line.border {
        border_xml(xml, ctx.theme, border)?;
    } else {
        let w = emu(1.0).to_string();
        let join = line.curve.map(line_join).unwrap_or("round");
        xml.start("a:ln", &[("w", &w), ("cmpd", "sng"), ("cap", "rnd")]);
        xml.leaf(&format!("a:{join}"), &[]);
        xml.end("a:ln");
    }
    xml.end("p:spPr");
    xml.end("p:cxnSp");
    Ok(())
}

// ---------------------------------------------------------------------------
// Icon
// ---------------------------------------------------------------------------

/// Base64url (RFC 4648 §5, unpadded) — used for the `pptd:icon` extension.
fn base64url(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = ((chunk[0] as u32) << 16)
            | ((chunk.get(1).copied().unwrap_or(0) as u32) << 8)
            | (chunk.get(2).copied().unwrap_or(0) as u32);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(n >> 6) as usize & 63] as char);
        }
        if chunk.len() > 2 {
            out.push(TABLE[n as usize & 63] as char);
        }
    }
    out
}

fn render_icon(xml: &mut Xml, ctx: &mut RenderCtx<'_>, icon: &Icon) -> Result<()> {
    let glyph = fa::lookup(&icon.icon_name).ok_or_else(|| {
        Error::Unsupported(format!(
            "icon `{}` is not in the embedded Font Awesome dataset \
             (see assets/README.md to regenerate it)",
            icon.icon_name
        ))
    })?;
    let id = ctx.next_id().to_string();
    let name = icon.common.element_id.clone();

    xml.start("p:sp", &[]);
    xml.start("p:nvSpPr", &[]);
    xml.start("p:cNvPr", &[("id", &id), ("name", &name)]);
    xml.end("p:cNvPr");
    xml.leaf("p:cNvSpPr", &[]);
    // Keep the PPTD identity for round-tripping (mirrors the Kimi exporter).
    let payload = base64url(format!("{{\"iconName\":\"{}\"}}", icon.icon_name).as_bytes());
    xml.start("p:nvPr", &[]);
    xml.start("p:extLst", &[]);
    xml.start(
        "p:ext",
        &[("uri", "{F5677B7D-0D2A-4D9B-9A5C-6D8D7D0E5A5D}")],
    );
    xml.start("pptd:icon", &[("encoding", "base64url")]);
    xml.text(&payload);
    xml.end("pptd:icon");
    xml.end("p:ext");
    xml.end("p:extLst");
    xml.end("p:nvPr");
    xml.end("p:nvSpPr");

    xml.start("p:spPr", &[]);
    xfrm(
        xml,
        icon.common.bounds,
        icon.common.rotation,
        icon.common.flip,
    );
    let w = (glyph.w as i64).to_string();
    let h = (glyph.h as i64).to_string();
    xml.start("a:custGeom", &[]);
    xml.leaf("a:avLst", &[]);
    xml.leaf("a:gdLst", &[]);
    xml.leaf("a:ahLst", &[]);
    xml.leaf("a:cxnLst", &[]);
    xml.leaf("a:rect", &[("l", "l"), ("t", "t"), ("r", "r"), ("b", "b")]);
    xml.start("a:pathLst", &[]);
    xml.start("a:path", &[("w", &w), ("h", &h)]);
    svg_path::emit_path_children(xml, &glyph.d)?;
    xml.end("a:path");
    xml.end("a:pathLst");
    xml.end("a:custGeom");
    if let Some(fill) = &icon.fill {
        fill_xml(xml, ctx.theme, fill, icon.common.opacity)?;
    } else {
        let resolved = ResolvedColor {
            rgb: "000000".to_owned(),
            alpha: None,
        };
        emit_fill_color(xml, &resolved);
    }
    xml.start("a:ln", &[]);
    xml.leaf("a:noFill", &[]);
    xml.end("a:ln");
    xml.end("p:spPr");
    xml.end("p:sp");
    Ok(())
}

// ---------------------------------------------------------------------------
// Image
// ---------------------------------------------------------------------------

/// Compute the `a:srcRect` crop/pad per `fit` + `crop`. Positive values crop
/// inward, negative pad outward with transparent pixels; unit 100000 = 100%.
fn src_rect(image: &Image, size: Option<ImageSize>) -> (i64, i64, i64, i64) {
    let bw = image.common.bounds.width;
    let bh = image.common.bounds.height;
    let mode = image.fit.map(|f| f.mode).unwrap_or(ImageFitMode::Cover);

    let (mut l, mut t, mut r, mut b) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    if let Some(ImageSize { width, height }) = size {
        let (iw, ih) = (width as f64, height as f64);
        if iw > 0.0 && ih > 0.0 && bw > 0.0 && bh > 0.0 {
            let ratio_box = bw / bh;
            let ratio_img = iw / ih;
            match mode {
                ImageFitMode::Fill => {}
                ImageFitMode::Contain => {
                    if ratio_img > ratio_box {
                        // Letterbox left/right: expand the source rect.
                        let scale = bh / ih;
                        let alpha = (bw / (iw * scale) - 1.0) / 2.0;
                        l = -alpha;
                        r = -alpha;
                    } else if ratio_img < ratio_box {
                        let scale = bw / iw;
                        let alpha = (bh / (ih * scale) - 1.0) / 2.0;
                        t = -alpha;
                        b = -alpha;
                    }
                }
                ImageFitMode::Cover => {
                    if ratio_img > ratio_box {
                        // Crop left/right.
                        let scale = bh / ih;
                        let frac = (iw * scale - bw) / 2.0 / (iw * scale);
                        l = frac.max(0.0);
                        r = frac.max(0.0);
                    } else if ratio_img < ratio_box {
                        let scale = bw / iw;
                        let frac = (ih * scale - bh) / 2.0 / (ih * scale);
                        t = frac.max(0.0);
                        b = frac.max(0.0);
                    }
                }
            }
        }
    }
    if let Some(crop) = &image.crop {
        l += crop.left.unwrap_or(0.0);
        t += crop.top.unwrap_or(0.0);
        r += crop.right.unwrap_or(0.0);
        b += crop.bottom.unwrap_or(0.0);
    }
    let to_per = |f: f64| -> i64 { (f * 100000.0).round() as i64 };
    (to_per(l), to_per(t), to_per(r), to_per(b))
}

fn render_image(xml: &mut Xml, ctx: &mut RenderCtx<'_>, image: &Image) -> Result<()> {
    let id = ctx.next_id().to_string();
    let name = image.common.element_id.clone();
    let rid = ctx.media_rid(&image.src).ok_or_else(|| {
        Error::Invalid(format!(
            "internal error: media `{}` was not registered before rendering",
            image.src
        ))
    })?;

    xml.start("p:pic", &[]);
    xml.start("p:nvPicPr", &[]);
    xml.start("p:cNvPr", &[("id", &id), ("name", &name)]);
    xml.end("p:cNvPr");
    xml.leaf("p:cNvPicPr", &[]);
    xml.leaf("p:nvPr", &[]);
    xml.end("p:nvPicPr");

    xml.start("p:blipFill", &[]);
    xml.start("a:blip", &[("r:embed", &rid)]);
    xml.end("a:blip");
    let (l, t, r, b) = src_rect(image, ctx.image_size(&image.src));
    if l != 0 || t != 0 || r != 0 || b != 0 {
        let mut attrs: Vec<(&str, String)> = Vec::new();
        if l != 0 {
            attrs.push(("l", l.to_string()));
        }
        if t != 0 {
            attrs.push(("t", t.to_string()));
        }
        if r != 0 {
            attrs.push(("r", r.to_string()));
        }
        if b != 0 {
            attrs.push(("b", b.to_string()));
        }
        let attrs_ref: Vec<(&str, &str)> = attrs.iter().map(|(k, v)| (*k, v.as_str())).collect();
        xml.start("a:srcRect", &attrs_ref);
        xml.end("a:srcRect");
    }
    xml.start("a:stretch", &[]);
    xml.leaf("a:fillRect", &[]);
    xml.end("a:stretch");
    xml.end("p:blipFill");

    xml.start("p:spPr", &[]);
    xfrm(
        xml,
        image.common.bounds,
        image.common.rotation,
        image.common.flip,
    );
    match &image.crop_shape {
        Some(shape) => {
            xml.start("a:prstGeom", &[("prst", &shape.shape_name)]);
            xml.start("a:avLst", &[]);
            if let Some(adjustments) = &shape.adjustments {
                for (i, &adj) in adjustments.iter().take(10).enumerate() {
                    let adj_name = if i == 0 {
                        "adj".to_owned()
                    } else {
                        format!("adj{}", i + 1)
                    };
                    let value = (adj.round() as i64).to_string();
                    let fmla = format!("val {value}");
                    xml.leaf("a:gd", &[("name", &adj_name), ("fmla", &fmla)]);
                }
            }
            xml.end("a:avLst");
            xml.end("a:prstGeom");
        }
        None => {
            xml.start("a:prstGeom", &[("prst", "rect")]);
            xml.leaf("a:avLst", &[]);
            xml.end("a:prstGeom");
        }
    }
    if let Some(border) = &image.border {
        border_xml(xml, ctx.theme, border)?;
    } else {
        xml.start("a:ln", &[]);
        xml.leaf("a:noFill", &[]);
        xml.end("a:ln");
    }
    xml.end("p:spPr");
    xml.end("p:pic");
    Ok(())
}
