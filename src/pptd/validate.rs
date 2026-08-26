//! Semantic validation of a loaded [`Project`].
//!
//! The loader guarantees the document parses and the version is `v2`; this
//! module checks the *semantics* the spec requires: unique element ids,
//! positive bounds, resolvable theme references, well-formed tables and
//! coherent chart data. Findings are reported as [`Diagnostic`]s rather than
//! fatal errors, so `slideforge check` can list every issue in one pass.

use std::collections::HashSet;

use super::chart::Chart;
use super::elements::Element;
use super::theme::{TableStyleRef, Theme};
use super::{Page, Project};

/// A single validation finding.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    /// One-based page index if the finding belongs to a page.
    pub page: Option<usize>,
    /// Human-readable description.
    pub message: String,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.page {
            Some(page) => write!(f, "page {page}: {}", self.message),
            None => write!(f, "{}", self.message),
        }
    }
}

/// Run every check over the project; an empty result means "valid".
pub fn validate_project(project: &Project) -> Vec<Diagnostic> {
    let mut out = Vec::new();

    let size = &project.presentation.size;
    if !size.width.is_finite()
        || !size.height.is_finite()
        || size.width <= 0.0
        || size.height <= 0.0
    {
        out.push(Diagnostic {
            page: None,
            message: format!("size must be positive and finite, got {size:?}"),
        });
    }

    for (idx, page) in project.pages.iter().enumerate() {
        validate_page(project, idx, page, &mut out);
    }

    out
}

fn validate_page(project: &Project, page_idx: usize, page: &Page, out: &mut Vec<Diagnostic>) {
    let theme = project.presentation.theme.as_ref();

    let mut ids: HashSet<&str> = HashSet::new();
    for element in &page.elements {
        let element_id = element.element_id();
        if !ids.insert(element_id) {
            diag(out, page_idx, format!("duplicate elementId `{element_id}`"));
        }

        let bounds = element.bounds();
        let degenerate = match element {
            // A straight connector is legitimately degenerate in one axis
            // (horizontal: height 0, vertical: width 0), so a flattened
            // box is allowed as long as at least one axis is non-zero.
            Element::Line(_) => {
                let w = bounds.width;
                let h = bounds.height;
                !(w.is_finite() && h.is_finite() && (w > 0.0 || h > 0.0))
            }
            _ => {
                !(bounds.width.is_finite()
                    && bounds.height.is_finite()
                    && bounds.width > 0.0
                    && bounds.height > 0.0)
            }
        };
        if degenerate {
            diag(
                out,
                page_idx,
                format!(
                    "element `{element_id}` must have positive width and height, got {bounds:?}"
                ),
            );
        }

        if let Some(opacity) = element.opacity() {
            if !(0.0..=1.0).contains(&opacity) {
                diag(
                    out,
                    page_idx,
                    format!("element `{element_id}` opacity must be in [0, 1], got {opacity}"),
                );
            }
        }

        validate_element(page_idx, element, theme, out);
    }

    if let Some(animations) = &page.animations {
        for animation in animations {
            if !ids.contains(animation.element_id.as_str()) {
                diag(
                    out,
                    page_idx,
                    format!(
                        "animation references unknown elementId `{}`",
                        animation.element_id
                    ),
                );
            }
        }
    }
}

fn validate_element(
    page_idx: usize,
    element: &Element,
    theme: Option<&Theme>,
    out: &mut Vec<Diagnostic>,
) {
    match element {
        Element::Text(text) => {
            check_text_style_ref(
                theme,
                text.content.style.as_deref(),
                page_idx,
                out,
                format!("text `{}`", text.common.element_id),
            );
        }
        Element::Table(table) => validate_table(page_idx, table, theme, out),
        Element::Chart(chart) => validate_chart(page_idx, chart, out),
        // TODO: walk fill/border colors on shapes, lines, images, icons and
        // text content to resolve every `$color` reference against the theme.
        _ => {}
    }
}

/// `style: "$key"` must exist in `theme.textStyles`.
fn check_text_style_ref(
    theme: Option<&Theme>,
    style: Option<&str>,
    page_idx: usize,
    out: &mut Vec<Diagnostic>,
    context: String,
) {
    if let Some(style) = style {
        let key = style.trim_start_matches('$');
        let resolved = theme.is_some_and(|theme| theme.text_styles.contains_key(key));
        if !resolved {
            diag(
                out,
                page_idx,
                format!("style `{style}` is not defined in theme.textStyles ({context})"),
            );
        }
    }
}

fn validate_table(
    page_idx: usize,
    table: &super::elements::Table,
    theme: Option<&Theme>,
    out: &mut Vec<Diagnostic>,
) {
    let element_id = &table.common.element_id;

    if let Some(TableStyleRef::Ref(style)) = &table.style {
        let key = style.trim_start_matches('$');
        let resolved = theme.is_some_and(|theme| theme.table_styles.contains_key(key));
        if !resolved {
            diag(
                out,
                page_idx,
                format!(
                    "table style `{style}` is not defined in theme.tableStyles (`{element_id}`)"
                ),
            );
        }
    }

    check_ratios(
        &table.column_widths,
        "columnWidths",
        page_idx,
        out,
        element_id,
    );
    check_ratios(&table.row_heights, "rowHeights", page_idx, out, element_id);

    let columns = table.column_widths.len();
    if table.rows.len() != table.row_heights.len() {
        diag(
            out,
            page_idx,
            format!(
                "table `{element_id}` has {} rows of data but {} rowHeights",
                table.rows.len(),
                table.row_heights.len()
            ),
        );
    }
    for (row_idx, row) in table.rows.iter().enumerate() {
        if row.len() > columns {
            diag(
                out,
                page_idx,
                format!(
                    "table `{element_id}` row {row_idx} has {} cells but the table only has \
                     {columns} columns (merged cells should be omitted)",
                    row.len()
                ),
            );
        }
    }
}

/// Each ratio must be within `(0, 1]` and the whole slice must sum to `1`.
fn check_ratios(
    ratios: &[f64],
    field: &str,
    page_idx: usize,
    out: &mut Vec<Diagnostic>,
    element_id: &str,
) {
    if ratios.is_empty() {
        diag(
            out,
            page_idx,
            format!("table `{element_id}` has no {field}"),
        );
        return;
    }
    for (i, ratio) in ratios.iter().enumerate() {
        if *ratio <= 0.0 || *ratio > 1.0 || !ratio.is_finite() {
            diag(
                out,
                page_idx,
                format!("table `{element_id}` {field}[{i}] must be in (0, 1], got {ratio}"),
            );
        }
    }
    let sum: f64 = ratios.iter().sum();
    if (sum - 1.0).abs() > 1e-3 {
        diag(
            out,
            page_idx,
            format!("table `{element_id}` {field} must sum to 1, got {sum}"),
        );
    }
}

fn validate_chart(page_idx: usize, chart: &Chart, out: &mut Vec<Diagnostic>) {
    let element_id = &chart.common.element_id;

    if chart.series.is_empty() {
        diag(
            out,
            page_idx,
            format!("chart `{element_id}` must define at least one series"),
        );
    }

    let cols = &chart.data.cols;
    let mut seen: HashSet<&str> = HashSet::new();
    for col in cols {
        if col.is_empty() {
            diag(
                out,
                page_idx,
                format!("chart `{element_id}` has an empty column name"),
            );
        }
        if !seen.insert(col.as_str()) {
            diag(
                out,
                page_idx,
                format!("chart `{element_id}` has duplicate column `{col}`"),
            );
        }
    }

    for (row_idx, row) in chart.data.rows.iter().enumerate() {
        if row.len() != cols.len() {
            diag(
                out,
                page_idx,
                format!(
                    "chart `{element_id}` row {row_idx} has {} cells, expected {}",
                    row.len(),
                    cols.len()
                ),
            );
        }
    }

    for series in &chart.series {
        for col in series.encode_columns() {
            if !cols.iter().any(|c| c == col) {
                diag(
                    out,
                    page_idx,
                    format!(
                        "chart `{element_id}` {} series references unknown column `{col}`",
                        series.type_name()
                    ),
                );
            }
        }
        // TODO: check stacking-mode consistency, numeric channels, theme
        // color references in series fills, and the per-type series
        // constraints from the spec.
    }
}

fn diag(out: &mut Vec<Diagnostic>, page: usize, message: String) {
    out.push(Diagnostic {
        page: Some(page + 1),
        message,
    });
}
