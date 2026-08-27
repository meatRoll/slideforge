//! SlideForge layout extension over PPTD v2.
//!
//! Canonical PPTD v2 is a flat model with no Slide Master / Layout
//! (see `docs/pptd-spec.md` §0). To round-trip a real PPTX without flattening
//! master/layout decorations onto every slide — which makes full-bleed
//! backgrounds draggable and leaves the WPS layout panel blank — SlideForge
//! adds an **optional** layout layer mirroring the OOXML `slideLayout` concept.
//!
//! The extension is opt-in and backward compatible: a page without `layout`
//! stays fully self-contained. See `docs/pptd-layout-extension.md` for the
//! full design.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::elements::Element;
use super::shared::{Alignment, Bounds, Color, Fill, FontFamily};

/// A named layout declared in the manifest (`Presentation.layouts`).
///
/// Carries the page background, decorative elements (painted *under* the
/// page's own elements) and placeholder geometry + run-style defaults that
/// slide placeholders of the same `type` inherit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayoutDef {
    /// Page background when the page doesn't set its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<Fill>,
    /// Decorative elements, painted below the page's elements.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub elements: Vec<Element>,
    /// Placeholder geometry + run-style defaults, keyed by OOXML placeholder
    /// type (`title`, `body`, `subTitle`, `dt` …).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub placeholders: HashMap<String, PlaceholderDef>,
    /// SlideForge group extension: reconstructed `<p:grpSp>` metadata for the
    /// layout's decorative elements (see [`crate::pptd::elements::GroupDef`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub groups: Option<HashMap<String, crate::pptd::elements::GroupDef>>,
}

/// Geometry + default run-style a slide placeholder of the same `type`
/// inherits when it omits its own. The slide placeholder wins wherever it
/// sets a value; only the omitted fields are filled in from here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceholderDef {
    /// Placeholder geometry (`xfrm`), px. Required.
    pub bounds: Bounds,
    /// References `theme.textStyles`, e.g. `"$title"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    /// Default run colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
    /// Default font size (px = pt).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f64>,
    /// Default font family.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<FontFamily>,
    /// Default bold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    /// Default italic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    /// Default paragraph alignment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<Alignment>,
}
