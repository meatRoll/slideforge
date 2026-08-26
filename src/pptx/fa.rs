//! Font Awesome solid-icon glyph dataset.
//!
//! The SVG path data comes from the Font Awesome 6.7.2 metadata and is
//! embedded at compile time (`assets/fa-solid-icons.json`, CC BY 4.0 — see
//! `assets/README.md` for the source and a regeneration recipe). Icons
//! missing from the dataset fail the build with an explicit error.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

const ASSET: &str = include_str!("../../assets/fa-solid-icons.json");

/// One icon glyph: viewBox size + SVG path `d`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct FaIcon {
    pub w: f64,
    pub h: f64,
    pub d: String,
}

static ICONS: OnceLock<HashMap<String, FaIcon>> = OnceLock::new();

/// The embedded dataset (`"mobile-screen" -> FaIcon`).
fn icons() -> &'static HashMap<String, FaIcon> {
    ICONS.get_or_init(|| {
        // The asset is a JSON document, which is valid YAML; parse it once.
        serde_yaml::from_str(ASSET).unwrap_or_default()
    })
}

/// Look up an icon by its PPTD name (`"fas:mobile-screen"`); unknown style
/// prefixes or names return `None`.
pub fn lookup(name: &str) -> Option<&'static FaIcon> {
    let key = name.split_once(':').map(|(_, key)| key).unwrap_or(name);
    icons().get(key)
}

/// Number of icons in the embedded dataset (exposed for tests).
pub fn dataset_len() -> usize {
    icons().len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pptx::svg_path;

    #[test]
    fn every_embedded_icon_parses() {
        assert!(
            dataset_len() >= 32,
            "dataset unexpectedly small: {}",
            dataset_len()
        );
        for (name, icon) in icons() {
            svg_path::parse(&icon.d)
                .unwrap_or_else(|err| panic!("icon `{name}` fails to parse: {err}"));
        }
    }
}
