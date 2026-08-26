//! Theme and style configuration types (`Theme` in the main entry file).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::shared::{Alignment, BorderSpec, Color, Fill, FontFamily};

/// A custom font loaded from a Google Fonts CSS URL.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomFont {
    pub family: String,
    pub src: String,
}

/// Text style configuration, referenceable as `$key` from `theme.textStyles`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextStyleConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<FontFamily>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_color: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_height: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_height_px: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub letter_spacing: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin_top: Option<f64>,
}

/// Cell style: a text style plus cell-level fill / border / alignment.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CellStyle {
    #[serde(flatten)]
    pub text: TextStyleConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Fill>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<BorderSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<Alignment>,
}

/// Table style: per-cell baseline plus row / column category overrides.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableStyleConfig {
    /// Baseline cell style for the whole table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell_style: Option<CellStyle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_row_style: Option<CellStyle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_row_style: Option<CellStyle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_column_style: Option<CellStyle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_column_style: Option<CellStyle>,
    /// Styles cycled over the data rows between the first and last row.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub body_styles: Vec<CellStyle>,
    /// Whether the row style wins over the column style; defaults to `true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_over_column: Option<bool>,
}

/// A `Table.style` value: either a `$key` reference into `theme.tableStyles`
/// or an inline [`TableStyleConfig`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TableStyleRef {
    /// Theme reference, e.g. `"$default"`.
    Ref(String),
    /// Inline style object.
    Inline(Box<TableStyleConfig>),
}

/// The deck-level theme registry referenced by `$key` everywhere.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Theme {
    /// Named colors, e.g. `primary: "#2563EB"`.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub colors: HashMap<String, Color>,
    /// Named text styles, e.g. `title: {fontSize: 40, color: "$primary"}`.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub text_styles: HashMap<String, TextStyleConfig>,
    /// Named table styles, e.g. `default: {firstRowStyle: {...}}`.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub table_styles: HashMap<String, TableStyleConfig>,
}
