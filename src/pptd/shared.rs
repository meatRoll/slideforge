//! Small value types shared across the PPTD AST.
//!
//! Every type mirrors a section of the PPTD specification; the YAML forms
//! (tuples, inline styles, ...) are converted by serde, so the Rust code can
//! work with named fields and unit-testable invariants.

use serde::{Deserialize, Serialize};

/// Canvas/page size in px. YAML form: `[width, height]`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(from = "(f64, f64)", into = "(f64, f64)")]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

impl From<(f64, f64)> for Size {
    fn from((width, height): (f64, f64)) -> Self {
        Self { width, height }
    }
}

impl From<Size> for (f64, f64) {
    fn from(size: Size) -> Self {
        (size.width, size.height)
    }
}

/// Element box in px: `[x, y, width, height]`, origin in the top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(from = "(f64, f64, f64, f64)", into = "(f64, f64, f64, f64)")]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl From<(f64, f64, f64, f64)> for Bounds {
    fn from((x, y, width, height): (f64, f64, f64, f64)) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

impl From<Bounds> for (f64, f64, f64, f64) {
    fn from(bounds: Bounds) -> Self {
        (bounds.x, bounds.y, bounds.width, bounds.height)
    }
}

/// A color: opaque HEX6 (`#RRGGBB`), alpha HEX8 (`#RRGGBBAA`) or a theme
/// reference such as `$primary`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Color(pub String);

impl Color {
    /// True when the value is a theme reference such as `$primary`.
    pub fn is_theme_ref(&self) -> bool {
        self.0.starts_with('$')
    }

    /// The theme key for a reference, e.g. `"primary"` for `"$primary"`.
    pub fn theme_key(&self) -> Option<&str> {
        self.0.strip_prefix('$')
    }
}

/// A font family: one family for everything, or distinct Latin / EA fonts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FontFamily {
    /// Chinese and English use the same font uniformly.
    Single(String),
    /// Explicit Latin and East Asian families.
    Bilingual { latin: String, ea: String },
}

/// Horizontal text alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HorizontalAlign {
    #[default]
    Left,
    Center,
    Right,
    Justify,
    Distributed,
}

/// Vertical text alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerticalAlign {
    #[default]
    Top,
    Middle,
    Bottom,
}

/// Text alignment `[horizontal, vertical]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(
    from = "(HorizontalAlign, VerticalAlign)",
    into = "(HorizontalAlign, VerticalAlign)"
)]
pub struct Alignment {
    pub horizontal: HorizontalAlign,
    pub vertical: VerticalAlign,
}

impl From<(HorizontalAlign, VerticalAlign)> for Alignment {
    fn from((horizontal, vertical): (HorizontalAlign, VerticalAlign)) -> Self {
        Self {
            horizontal,
            vertical,
        }
    }
}

impl From<Alignment> for (HorizontalAlign, VerticalAlign) {
    fn from(alignment: Alignment) -> Self {
        (alignment.horizontal, alignment.vertical)
    }
}

/// Line style used for borders, grid lines, chart lines, ...
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineStyle {
    Solid,
    Dash,
    Dot,
}

/// A border around a shape, cell, chart frame, ...
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Border {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<LineStyle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
    /// Gradient outline (overrides `color` when present).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gradient: Option<GradientFill>,
}

/// Per-side border spec: `null`, a uniform [`Border`], `[top-bottom,
/// left-right]` or `[top, right, bottom, left]` (clockwise). A `null` inside
/// the array clears the corresponding side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BorderSpec {
    /// `null`: no border on any of the four sides (explicit clear).
    Clear,
    /// One border on all four sides.
    Uniform(Border),
    /// `[top-bottom, left-right]`.
    VerticalHorizontal([Option<Border>; 2]),
    /// `[top, right, bottom, left]` (clockwise).
    Clockwise([Option<Border>; 4]),
}

/// A drop shadow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Shadow {
    /// Blur radius.
    pub blur: f64,
    pub color: Color,
    /// `[x, y]` offset; defaults to `[0, 0]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<(f64, f64)>,
    /// Scale factor (1.0 = 100%); mirrors OOXML `outerShdw` `sx`/`sy`. When
    /// > 1.0 the shadow peeks out beyond the shape edge (a centered halo).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
    /// SlideForge extension: an inner shadow (`a:innerShdw`) renders inside
    /// the shape bounds (inset) instead of outside (`a:outerShdw`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inner: Option<bool>,
}

/// One stop of a gradient, with `position` in `[0, 1]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColorStop {
    pub position: f64,
    pub color: Color,
}

/// Gradient orientation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GradientType {
    Linear,
    Radial,
}

/// A linear or radial gradient used as a fill or text decoration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GradientFill {
    #[serde(rename = "gradientType")]
    pub gradient_type: GradientType,
    /// At least 2 stops.
    pub stops: Vec<ColorStop>,
    /// Only effective for linear gradients; `0` = left to right, clockwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle: Option<f64>,
}

/// How a [`Fill::Image`] adapts to its container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFitMode {
    /// Stretch to fill; may distort.
    Fill,
    /// Fit completely; may leave blank space.
    Contain,
    /// Fill and keep aspect ratio; may crop.
    #[default]
    Cover,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageFit {
    #[serde(default)]
    pub mode: ImageFitMode,
}

/// Proportional cropping; positive values crop inward, negative values pad
/// outward with transparent pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageCrop {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bottom: Option<f64>,
}

/// Background / foreground fill: solid color, gradient or image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Fill {
    /// `type: solid`
    Solid { color: Color },
    /// `type: gradient`
    Gradient {
        #[serde(rename = "gradientType")]
        gradient_type: GradientType,
        stops: Vec<ColorStop>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        angle: Option<f64>,
    },
    /// `type: image`; `src` may be a relative path or `https://` URL.
    Image {
        src: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fit: Option<ImageFit>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        crop: Option<ImageCrop>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        opacity: Option<f64>,
    },
}
