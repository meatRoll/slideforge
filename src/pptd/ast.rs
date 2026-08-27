//! The root AST node of a PPTD project: [`Presentation`] and [`Page`].

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::animation::Animation;
use super::elements::Element;
use super::layout::LayoutDef;
use super::shared::{Fill, Size};
use super::theme::{CustomFont, Theme};

/// The main entry file (`*.pptd`) of a project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Presentation {
    /// Version identifier. The loader enforces `"v2"`.
    pub version: String,
    /// Optional presentation title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Custom fonts (e.g. Google Fonts CSS URLs) used by the deck.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_fonts: Vec<CustomFont>,
    /// Canvas size in px; e.g. `[960, 540]` for 16:9.
    pub size: Size,
    /// Shared theme: colors, text styles and table styles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<Theme>,
    /// SlideForge extension: named layouts a page may reference via
    /// [`Page::layout`]. See `docs/pptd-layout-extension.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layouts: Option<HashMap<String, LayoutDef>>,
    /// Relative paths to the page files, e.g. `pages/1_cover.page`.
    pub pages: Vec<String>,
}

/// A single slide. Page files are loaded only through the main entry file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    /// Category label; does not affect rendering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_type: Option<String>,
    /// SlideForge extension: references a key in
    /// [`Presentation::layouts`]. When set, the page inherits the layout's
    /// background (unless overridden), decorative `elements` (painted under
    /// the page's own) and `placeholders`. See
    /// `docs/pptd-layout-extension.md`. Unset → fully self-contained
    /// (canonical PPTD v2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    /// Page background; defaults to a white solid fill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<Fill>,
    /// Speaker notes (plain text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Elements, in paint order: the later an element, the higher its layer.
    #[serde(default)]
    pub elements: Vec<Element>,
    /// Optional animation orchestrations referencing `elements[].element_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animations: Option<Vec<Animation>>,
}
