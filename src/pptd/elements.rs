//! Page elements: the `Element` enum plus its typed variants.
//!
//! The `Element` enum is tagged by the `elementType` field. serde's
//! internally-tagged representation merges newtype-variant payloads into the
//! same map as the tag, so a plain `#[derive]` produces exactly the PPTD YAML
//! shape.

use serde::{Deserialize, Serialize};

use super::chart::Chart;
use super::shared::{Alignment, Border, Bounds, Fill, ImageCrop, ImageFit, Shadow};
use super::theme::TableStyleRef;

/// `ElementBase` fields: element id + bounds + optional geometry transforms.
///
/// Flattened into every concrete element struct (Rust 2024 does not allow
/// `macro_rules!` to expand to struct fields, so the common surface is a
/// struct instead).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ElementCommon {
    /// Unique element id within the page.
    pub element_id: String,
    /// Position and size `[x, y, width, height]` in px.
    pub bounds: Bounds,
    /// Clockwise rotation in degrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<f64>,
    /// Opacity in [0, 1].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    /// `[horizontal flip, vertical flip]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flip: Option<(bool, bool)>,
    /// SlideForge extension: id of the `<p:grpSp>` group this element
    /// belongs to (membership is recorded so the writer can reconstruct the
    /// group — preserving WPS's group selection box). The element stays in
    /// the flat `elements` array; `bounds` remains slide-space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    /// SlideForge extension: the element's `a:xfrm` in the **group's child
    /// space** (`[off.x, off.y, ext.cx, ext.cy]`), emitted verbatim inside
    /// the reconstructed `<p:grpSp>`. Only meaningful with `groupId`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_bounds: Option<Bounds>,
}

/// Any element on a page, discriminated by the `elementType` field:
/// internally-tagged, payload merged into the same map as the tag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "elementType", rename_all = "snake_case")]
pub enum Element {
    Text(Text),
    Shape(Shape),
    Line(Line),
    Image(Image),
    Icon(Icon),
    Table(Table),
    /// Charts are the largest element; boxed to keep the enum small.
    Chart(Box<Chart>),
}

impl Element {
    /// The `elementType` value of this element.
    pub fn type_name(&self) -> &'static str {
        match self {
            Element::Text(_) => "text",
            Element::Shape(_) => "shape",
            Element::Line(_) => "line",
            Element::Image(_) => "image",
            Element::Icon(_) => "icon",
            Element::Table(_) => "table",
            Element::Chart(_) => "chart",
        }
    }

    pub fn element_id(&self) -> &str {
        match self {
            Element::Text(e) => &e.common.element_id,
            Element::Shape(e) => &e.common.element_id,
            Element::Line(e) => &e.common.element_id,
            Element::Image(e) => &e.common.element_id,
            Element::Icon(e) => &e.common.element_id,
            Element::Table(e) => &e.common.element_id,
            Element::Chart(e) => &e.common.element_id,
        }
    }

    pub fn bounds(&self) -> Bounds {
        match self {
            Element::Text(e) => e.common.bounds,
            Element::Shape(e) => e.common.bounds,
            Element::Line(e) => e.common.bounds,
            Element::Image(e) => e.common.bounds,
            Element::Icon(e) => e.common.bounds,
            Element::Table(e) => e.common.bounds,
            Element::Chart(e) => e.common.bounds,
        }
    }

    pub fn opacity(&self) -> Option<f64> {
        match self {
            Element::Text(e) => e.common.opacity,
            Element::Shape(e) => e.common.opacity,
            Element::Line(e) => e.common.opacity,
            Element::Image(e) => e.common.opacity,
            Element::Icon(e) => e.common.opacity,
            Element::Table(e) => e.common.opacity,
            Element::Chart(e) => e.common.opacity,
        }
    }

    /// Borrow the common fields (id/bounds/rotation/flip/groupId/groupBounds).
    pub fn common(&self) -> &ElementCommon {
        match self {
            Element::Text(e) => &e.common,
            Element::Shape(e) => &e.common,
            Element::Line(e) => &e.common,
            Element::Image(e) => &e.common,
            Element::Icon(e) => &e.common,
            Element::Table(e) => &e.common,
            Element::Chart(e) => &e.common,
        }
    }

    /// Mutably borrow the common fields (used by the writer to swap in
    /// `groupBounds` when emitting inside a reconstructed `<p:grpSp>`).
    pub fn common_mut(&mut self) -> &mut ElementCommon {
        match self {
            Element::Text(e) => &mut e.common,
            Element::Shape(e) => &mut e.common,
            Element::Line(e) => &mut e.common,
            Element::Image(e) => &mut e.common,
            Element::Icon(e) => &mut e.common,
            Element::Table(e) => &mut e.common,
            Element::Chart(e) => &mut e.common,
        }
    }
}

/// Text direction of a text box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextDirection {
    Horizontal,
    Vertical,
}

/// Auto-fit mode of a text box (`a:bodyPr` child).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAutofit {
    /// `spAutoFit`: resize the shape to fit the text.
    FitShape,
    /// `normAutofit`: shrink the font to fit the fixed shape.
    FitText,
}

/// Rich text content of a [`Text`] element.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextContent {
    /// Rich text string; may contain `<p>`, `<span>`, `<strong>`, ... tags.
    pub text: String,
    /// Reference to `theme.textStyles`, e.g. `"$title"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<super::shared::Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<super::shared::FontFamily>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_color: Option<super::shared::Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_height: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_height_px: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub letter_spacing: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin_top: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin_left: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin_right: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin_bottom: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autofit: Option<TextAutofit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_direction: Option<TextDirection>,
    /// Whether the text wraps; defaults to `true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrap: Option<bool>,
    /// `[horizontal, vertical]` alignment; defaults to `["left", "top"]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<Alignment>,
    /// List bullet glyph (e.g. `•`); absent → no bullet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bullet_char: Option<String>,
    /// Bullet typeface (e.g. `Arial`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bullet_font: Option<String>,
    /// Paragraph left margin (`marL`) in px — the bullet's text offset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_margin: Option<f64>,
    /// Hanging indent in px (negative pulls the bullet left of `marL`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_indent: Option<f64>,
    /// Text gradient (applied to the text itself).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gradient: Option<super::shared::GradientFill>,
    /// Text shadow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<Shadow>,
}

/// `elementType: text` — a text box.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Text {
    #[serde(flatten)]
    pub common: ElementCommon,
    pub content: TextContent,
    /// Background fill of the box (solid/gradient). Absent → transparent
    /// (a plain text box must not inherit the automatic shape fill).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Fill>,
    /// Box outline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<Border>,
    /// SlideForge extension: the layout placeholder type this text box
    /// inherits geometry/run-style defaults from (e.g. `"title"`). Resolved
    /// against [`crate::pptd::layout::LayoutDef::placeholders`] on the page's
    /// referenced layout. See `docs/pptd-layout-extension.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

/// `elementType: shape` — a built-in or custom geometry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Shape {
    #[serde(flatten)]
    pub common: ElementCommon,
    pub shape_name: String,
    /// Geometry adjustments; reuse the OOXML parameter order and count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adjustments: Option<Vec<f64>>,
    /// View box `[w, h]`; required when `shape_name == "custom"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view_box: Option<(f64, f64)>,
    /// SVG path; required when `shape_name == "custom"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Fill>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<Border>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<Shadow>,
}

/// Arrowhead style for a [`Line`] element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArrowType {
    Arrow,
    Stealth,
    Diamond,
    Oval,
}

/// Connection curve of a [`Line`] element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineCurve {
    /// Sharp joins.
    Sharp,
    /// Rounded joins.
    Round,
    /// Bezier smooth curve.
    Smooth,
}

/// `elementType: line` — a bezier or connected line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    #[serde(flatten)]
    pub common: ElementCommon,
    /// Path coordinate system `[w, h]`; points live inside it.
    pub view_box: (f64, f64),
    /// Bezier path points, e.g. `"0,0 0.2,0 0.8,1 1,1"`.
    pub points: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curve: Option<LineCurve>,
    /// `[start arrow, end arrow]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrow: Option<(Option<ArrowType>, Option<ArrowType>)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<Border>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<Shadow>,
}

/// Shape definition reused by `Image.cropShape` and `Bar.symbol`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShapeDef {
    /// `"custom"` selects the path-based geometry.
    pub shape_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adjustments: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view_box: Option<(f64, f64)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// `elementType: image`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Image {
    #[serde(flatten)]
    pub common: ElementCommon,
    /// URL or local relative path.
    pub src: String,
    /// Shape used to clip the image; defaults to a rectangle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop_shape: Option<ShapeDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit: Option<ImageFit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<ImageCrop>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<Border>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<Shadow>,
}

/// `elementType: icon` — a Font Awesome 7.x icon, `iconName` is `style:name`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Icon {
    #[serde(flatten)]
    pub common: ElementCommon,
    /// e.g. `"fas:lightbulb"`.
    pub icon_name: String,
    /// Defaults to black solid fill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Fill>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<Border>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<Shadow>,
}

/// One table cell (see the spec for merged-cell layout rules).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cell {
    /// Rich text; defaults to an empty cell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Reference to `theme.textStyles`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<super::shared::Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<super::shared::FontFamily>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_color: Option<super::shared::Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_height: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_height_px: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub letter_spacing: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin_top: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Fill>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<super::shared::BorderSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<Alignment>,
    /// Merge range; covered cells are omitted from the `rows` array.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_span: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub col_span: Option<u32>,
}

/// `elementType: table`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Table {
    #[serde(flatten)]
    pub common: ElementCommon,
    /// Column-width ratios; each in `[0, 1]` and summing to `1`.
    pub column_widths: Vec<f64>,
    /// Row-height ratios; each in `[0, 1]` and summing to `1`.
    pub row_heights: Vec<f64>,
    /// Cells with merged cells omitted (see the spec).
    pub rows: Vec<Vec<Cell>>,
    /// `$key` reference or inline table style.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<TableStyleRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Fill>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<Shadow>,
}

// ---------------------------------------------------------------------------
// SlideForge group extension (flat PPTD + reconstructed `<p:grpSp>).
// ---------------------------------------------------------------------------

/// A reconstructed `<p:grpSp>` group. Stored in [`crate::pptd::ast::Page::groups`];
/// member elements carry `groupId` + `groupBounds` (child-space) on
/// [`ElementCommon`]. The PPTD stays flat; the writer rebuilds the nesting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupDef {
    /// The group's raw OOXML transform (slide-space `off`/`ext` + child-space
    /// `chOff`/`chExt`), emitted verbatim as `<a:xfrm>`.
    pub xfrm: GroupXfrm,
    /// The original `p:cNvPr name` (e.g. `组合 22`); falls back to a generic
    /// name at write time when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Parent group id for nested groups; `None` for a top-level group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

/// A group `<a:xfrm>`: slide-space offset/extent + child-space offset/extent,
/// all in px (the writer converts to EMU). `rot`/`flip` apply to the group box.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupXfrm {
    /// Slide-space offset `[x, y]`.
    pub off: (f64, f64),
    /// Slide-space extent `[cx, cy]`.
    pub ext: (f64, f64),
    /// Child-space offset `[x, y]`.
    pub ch_off: (f64, f64),
    /// Child-space extent `[cx, cy]`.
    pub ch_ext: (f64, f64),
    /// Clockwise rotation in degrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rot: Option<f64>,
    /// `[horizontal flip, vertical flip]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flip: Option<(bool, bool)>,
}
