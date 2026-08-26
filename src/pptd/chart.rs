//! The chart element: tabular data plus typed series configurations.
//!
//! Series are discriminated by the `type` field. Like [`Element`], the
//! [`ChartSeries`] enum is bridged through `serde_yaml::Value` because serde's
//! internally-tagged representation does not support newtype variants.
//!
//! TODO: type the remaining eight series kinds (`bubble`, `candlestick`,
//! `radar`, `waterfall`, `heatmap`, `treemap`, `sunburst`, `sankey`) and the
//! axis / data-label configurations instead of keeping them as raw YAML.

use serde::{Deserialize, Serialize, Serializer, de, ser};
use serde_yaml::Value as YamlValue;

use super::elements::ElementCommon;
use super::shared::{Border, Fill, GradientFill, LineStyle, Shadow};

/// Two-column `encode` used by the Cartesian series kinds (`x` + `y`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct XyEncode {
    pub x: String,
    pub y: String,
}

/// `encode` for pie charts (`category` + `value`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryValueEncode {
    pub category: String,
    pub value: String,
}

/// A color or a gradient — the fill form used by most series.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ColorOrGradient {
    Color(super::shared::Color),
    Gradient(GradientFill),
}

/// A chart element (`elementType: chart`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chart {
    #[serde(flatten)]
    pub common: ElementCommon,
    /// Tabular data shared by all series.
    pub data: ChartData,
    /// At least one series.
    pub series: Vec<ChartSeries>,
    /// Per-type defaults merged into every series of that type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_defaults: Option<YamlValue>,
    /// Title: plain string or `TitleConfig` (kept raw for now).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<YamlValue>,
    /// `boolean | LegendConfig` (kept raw for now).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legend: Option<YamlValue>,
    /// `AxisConfig | AxisConfig[]` (kept raw for now).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x_axis: Option<YamlValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y_axis: Option<YamlValue>,
    /// `SpokeAxisConfig`, radar only (kept raw for now).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spoke_axis: Option<YamlValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<super::shared::FontFamily>,
    /// Chart frame (the rectangular container of the whole chart element).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Fill>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<Border>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<Shadow>,
}

/// Tabular chart data (`cols` + `rows`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChartData {
    /// Column names; must be unique and non-empty.
    pub cols: Vec<String>,
    /// Rows whose length must equal `cols.len()`; `null` marks a missing cell.
    pub rows: Vec<Vec<YamlValue>>,
}

/// One series, discriminated by the `type` field.
#[derive(Debug, Clone, PartialEq)]
pub enum ChartSeries {
    Bar(BarSeries),
    Line(LineSeries),
    Area(AreaSeries),
    Pie(PieSeries),
    Scatter(ScatterSeries),
}

impl ChartSeries {
    /// The `type` value of this series.
    pub fn type_name(&self) -> &'static str {
        match self {
            ChartSeries::Bar(_) => "bar",
            ChartSeries::Line(_) => "line",
            ChartSeries::Area(_) => "area",
            ChartSeries::Pie(_) => "pie",
            ChartSeries::Scatter(_) => "scatter",
        }
    }

    /// Column names referenced by this series' `encode`.
    pub fn encode_columns(&self) -> Vec<&str> {
        match self {
            ChartSeries::Bar(s) => vec![s.encode.x.as_str(), s.encode.y.as_str()],
            ChartSeries::Line(s) => vec![s.encode.x.as_str(), s.encode.y.as_str()],
            ChartSeries::Area(s) => vec![s.encode.x.as_str(), s.encode.y.as_str()],
            ChartSeries::Scatter(s) => vec![s.encode.x.as_str(), s.encode.y.as_str()],
            ChartSeries::Pie(s) => vec![s.encode.category.as_str(), s.encode.value.as_str()],
        }
    }
}

impl Serialize for ChartSeries {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let (type_name, value) = match self {
            ChartSeries::Bar(s) => (
                "bar",
                serde_yaml::to_value(s).map_err(<S::Error as ser::Error>::custom)?,
            ),
            ChartSeries::Line(s) => (
                "line",
                serde_yaml::to_value(s).map_err(<S::Error as ser::Error>::custom)?,
            ),
            ChartSeries::Area(s) => (
                "area",
                serde_yaml::to_value(s).map_err(<S::Error as ser::Error>::custom)?,
            ),
            ChartSeries::Pie(s) => (
                "pie",
                serde_yaml::to_value(s).map_err(<S::Error as ser::Error>::custom)?,
            ),
            ChartSeries::Scatter(s) => (
                "scatter",
                serde_yaml::to_value(s).map_err(<S::Error as ser::Error>::custom)?,
            ),
        };
        let mut mapping = match value {
            YamlValue::Mapping(mapping) => mapping,
            _ => return Err(<S::Error as ser::Error>::custom("payload is not a mapping")),
        };
        mapping.insert(
            YamlValue::String("type".to_owned()),
            YamlValue::String(type_name.to_owned()),
        );
        YamlValue::Mapping(mapping).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ChartSeries {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = YamlValue::deserialize(deserializer)?;
        let type_name = value
            .get("type")
            .and_then(YamlValue::as_str)
            .ok_or_else(|| {
                <D::Error as de::Error>::custom("series is missing the required `type` field")
            })?;
        let series = match type_name {
            "bar" => serde_yaml::from_value(value).map(ChartSeries::Bar),
            "line" => serde_yaml::from_value(value).map(ChartSeries::Line),
            "area" => serde_yaml::from_value(value).map(ChartSeries::Area),
            "pie" => serde_yaml::from_value(value).map(ChartSeries::Pie),
            "scatter" => serde_yaml::from_value(value).map(ChartSeries::Scatter),
            other => {
                return Err(<D::Error as de::Error>::custom(format!(
                    "unknown series type `{other}` (typed so far: bar, line, area, pie, \
                     scatter; bubble, candlestick, radar, waterfall, heatmap, treemap, \
                     sunburst, sankey are planned)"
                )));
            }
        };
        series.map_err(<D::Error as de::Error>::custom)
    }
}

/// `type: bar`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BarSeries {
    pub encode: XyEncode,
    /// Legend label; defaults to the `encode.y` column name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<ColorOrGradient>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<Border>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_labels: Option<YamlValue>,
}

/// Curve-class fields shared by `line` / `area` (and later `radar`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinearSeriesBase {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smooth: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_style: Option<LineStyle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    /// `false | MarkerConfig` (kept raw for now).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marker: Option<YamlValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub null_handling: Option<YamlValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_color: Option<ColorOrGradient>,
}

/// `type: line`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineSeries {
    pub encode: XyEncode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(flatten)]
    pub base: LinearSeriesBase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_labels: Option<YamlValue>,
}

/// `type: area`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AreaSeries {
    pub encode: XyEncode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(flatten)]
    pub base: LinearSeriesBase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<YamlValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area_color: Option<ColorOrGradient>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_labels: Option<YamlValue>,
}

/// `type: pie`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PieSeries {
    pub encode: CategoryValueEncode,
    /// `> 0` turns the pie into a donut; constraint `[0, 1]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inner_radius: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_angle: Option<f64>,
    /// Color or gradient, possibly an array cycled by slice (kept raw).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<YamlValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<Border>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_labels: Option<YamlValue>,
}

/// `type: scatter`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScatterSeries {
    pub encode: XyEncode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `MarkerConfig` (kept raw for now); never `false` for scatter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marker: Option<YamlValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<ColorOrGradient>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<Border>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_labels: Option<YamlValue>,
}
