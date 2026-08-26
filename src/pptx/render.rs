//! Element → drawingml rendering inside a slide's `cSld/spTree`.
//!
//! Supported in this milestone: `text`, `shape`, `line` plus page
//! backgrounds. Unsupported element kinds (`table`, `chart`, `image`,
//! `icon`) fail with an explicit [`Error::Unsupported`]; shape shadows are
//! currently dropped silently (documented limitation).

use crate::pptd::shared::{
    Alignment, Border, Bounds, Color, Fill, FontFamily, GradientType, HorizontalAlign, LineStyle,
    VerticalAlign,
};
use crate::pptd::{Element, Line, LineCurve, Shape, Text, TextContent, TextDirection, Theme};
use crate::{Error, Result};

use super::theme::{ResolvedColor, resolve_color};
use super::xml::Xml;

/// Rendering context shared while painting one slide.
pub struct RenderCtx<'a> {
    pub theme: Option<&'a Theme>,
    shape_id: usize,
}

impl<'a> RenderCtx<'a> {
    pub fn new(theme: Option<&'a Theme>) -> Self {
        Self { theme, shape_id: 1 }
    }

    fn next_id(&mut self) -> usize {
        self.shape_id += 1;
        self.shape_id
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
        other => Err(Error::Unsupported(format!(
            "element `{}` of type `{}` on page {} is not supported yet \
             (supported so far: text, shape, line)",
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
fn xfrm(xml: &mut Xml, bounds: Bounds, rotation: Option<f64>, flip: Option<(bool, bool)>) {
    let x = emu(bounds.x).to_string();
    let y = emu(bounds.y).to_string();
    let cx = emu(bounds.width).to_string();
    let cy = emu(bounds.height).to_string();

    xml.start("a:xfrm", &[]);
    if let Some(rot) = rotation.filter(|deg| deg.abs() > 1e-9) {
        let rot = (rot * 60000.0).round().to_string();
        xml.leaf("a:rot", &[("val", &rot)]);
    }
    if let Some((flip_h, flip_v)) = flip {
        if flip_h {
            xml.leaf("a:flipH", &[]);
        }
        if flip_v {
            xml.leaf("a:flipV", &[]);
        }
    }
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
fn border_xml(xml: &mut Xml, theme: Option<&Theme>, border: &Border) -> Result<()> {
    let width = emu(border.width.unwrap_or(1.0)).to_string();
    let dash: Option<String> = border.style.map(|style| {
        let value = match style {
            LineStyle::Solid => "solid",
            LineStyle::Dash => "dash",
            LineStyle::Dot => "dot",
        };
        value.to_owned()
    });
    let mut attrs: Vec<(&str, &str)> = vec![("w", &width)];
    if let Some(dash) = dash.as_deref() {
        attrs.push(("prstDash", dash));
    }
    xml.start("a:ln", &attrs);
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
            xml.start("a:cTo", &[]);
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
