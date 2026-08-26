//! `Theme` → `theme1.xml` mapping plus color resolution.
//!
//! Mapping rules live in `docs/pptx-layout-synthesis.md` §4. The slot table
//! is a draft: `theme.colors` keys are arbitrary, OOXML `clrScheme` has fixed
//! slots, so semantic names are mapped first and the leftovers fill the
//! remaining accent slots in declaration order (fallback = classic Office
//! palette). Slide styles are always resolved to explicit `srgbClr` values by
//! [`resolve_color`], so nothing at runtime depends on these slots.

use std::collections::BTreeMap;

use crate::pptd::{Theme, shared::Color, shared::FontFamily};
use crate::{Error, Result};

use super::opc::ns;
use super::xml::Xml;

/// Order of the `clrScheme` slots as written into the theme part.
const CLR_SLOT_ORDER: [&str; 12] = [
    "dk1", "lt1", "dk2", "lt2", "accent1", "accent2", "accent3", "accent4", "accent5", "accent6",
    "hlink", "folHlink",
];

/// Fallback palette when `theme.colors` does not fill a slot (Office classic).
const FALLBACK_COLORS: [(&str, &str); 12] = [
    ("dk1", "FFFFFF"),
    ("lt1", "000000"),
    ("dk2", "1F497D"),
    ("lt2", "EEECE1"),
    ("accent1", "4F81BD"),
    ("accent2", "C0504D"),
    ("accent3", "9BBB59"),
    ("accent4", "8064A2"),
    ("accent5", "4BACC6"),
    ("accent6", "F79646"),
    ("hlink", "0000FF"),
    ("folHlink", "800080"),
];

/// Semantic `theme.colors` key → clrScheme slot (case-insensitive).
const SEMANTIC_COLOR_KEYS: [(&str, &str); 12] = [
    ("text", "dk1"),
    ("text2", "dk2"),
    ("background", "lt1"),
    ("background2", "lt2"),
    ("primary", "accent1"),
    ("secondary", "accent2"),
    ("accent", "accent3"),
    ("success", "accent4"),
    ("warning", "accent5"),
    ("danger", "accent6"),
    ("link", "hlink"),
    ("folHlink", "folHlink"),
];

/// A color resolved to OOXML `srgbClr` form, with an optional alpha in [0, 1].
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedColor {
    /// Six hex digits, e.g. `"2563EB"`.
    pub rgb: String,
    /// Alpha in [0, 1]; `Some` when the source was HEX8 or an explicit opacity.
    pub alpha: Option<f64>,
}

/// Resolve a PPTD color (HEX6 / HEX8 / `$theme` reference) to srgb.
pub fn resolve_color(theme: Option<&Theme>, color: &Color) -> Result<ResolvedColor> {
    let raw = if let Some(key) = color.theme_key() {
        let value = theme.and_then(|theme| lookup_color(theme, key));
        value
            .map(|color| color.0.clone())
            .ok_or_else(|| Error::Unsupported(format!("unresolved theme color `{}`", color.0)))?
    } else {
        color.0.clone()
    };

    let hex = raw.trim_start_matches('#');
    match hex.len() {
        6 => Ok(ResolvedColor {
            rgb: hex.to_ascii_uppercase(),
            alpha: None,
        }),
        8 => {
            let rgb = hex[..6].to_ascii_uppercase();
            let alpha = u8::from_str_radix(&hex[6..], 16)
                .map_err(|_| Error::Invalid(format!("invalid color `{raw}`")))?
                as f64
                / 255.0;
            Ok(ResolvedColor {
                rgb,
                alpha: Some(alpha),
            })
        }
        _ => Err(Error::Invalid(format!("invalid color `{raw}`"))),
    }
}

/// Case-insensitive lookup of a color key inside `theme.colors`.
fn lookup_color<'a>(theme: &'a Theme, key: &str) -> Option<&'a Color> {
    theme.colors.get(key).or_else(|| {
        theme
            .colors
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v)
    })
}

/// The resolved clrScheme slots, keyed by slot name (`dk1` ... `folHlink`).
#[derive(Debug, Clone, PartialEq)]
pub struct ColorSlots {
    slots: BTreeMap<&'static str, String>,
}

impl Default for ColorSlots {
    fn default() -> Self {
        Self {
            slots: FALLBACK_COLORS
                .iter()
                .map(|(k, v)| (*k, (*v).to_owned()))
                .collect(),
        }
    }
}

/// Build the clrScheme slots from `theme.colors` (see module docs for rules).
pub fn build_color_slots(theme: Option<&Theme>) -> ColorSlots {
    let mut slots = ColorSlots::default();
    let Some(theme) = theme else { return slots };

    let mut used: BTreeMap<&'static str, String> = BTreeMap::new();
    for (key, slot) in SEMANTIC_COLOR_KEYS {
        if let Some(color) = lookup_color(theme, key) {
            used.insert(slot, color.0.clone());
        }
    }

    // Leftover keys fill the remaining slots in CLR_SLOT_ORDER.
    let mut next = 0;
    for (key, color) in &theme.colors {
        if SEMANTIC_COLOR_KEYS
            .iter()
            .any(|(k, _)| k == &key.to_ascii_lowercase())
        {
            continue;
        }
        while next < CLR_SLOT_ORDER.len() && used.contains_key(CLR_SLOT_ORDER[next]) {
            next += 1;
        }
        if next >= CLR_SLOT_ORDER.len() {
            break;
        }
        used.insert(CLR_SLOT_ORDER[next], color.0.clone());
        next += 1;
    }

    for (slot, color) in used {
        slots.slots.insert(slot, color);
    }
    slots
}

/// Render `ppt/theme/theme1.xml` from the presentation theme.
pub fn theme_xml(theme: Option<&Theme>) -> String {
    let slots = build_color_slots(theme);
    let (latin, ea) = default_fonts(theme);

    let mut x = Xml::new();
    x.start(
        "a:theme",
        &[("xmlns:a", ns::DRAWINGML), ("name", "SlideForge")],
    );
    x.start("a:themeElements", &[]);

    // clrScheme
    x.start("a:clrScheme", &[("name", "SlideForge")]);
    for slot in CLR_SLOT_ORDER {
        x.start(&format!("a:{slot}"), &[]);
        x.leaf(
            "a:srgbClr",
            &[(
                "val",
                slots
                    .slots
                    .get(slot)
                    .map(String::as_str)
                    .unwrap_or("000000"),
            )],
        );
        x.end(&format!("a:{slot}"));
    }
    x.end("a:clrScheme");

    // fontScheme
    x.start("a:fontScheme", &[("name", "SlideForge")]);
    emit_font_scheme(&mut x, "a:majorFont", &latin, &ea);
    emit_font_scheme(&mut x, "a:minorFont", &latin, &ea);
    x.end("a:fontScheme");

    // fmtScheme: standard Office default (fills / lines / effects / backgrounds).
    x.start("a:fmtScheme", &[("name", "SlideForge")]);
    x.start("a:fillStyleLst", &[]);
    x.start("a:solidFill", &[]);
    x.leaf("a:schemeClr", &[("val", "phClr")]);
    x.end("a:solidFill");
    x.start("a:gradFill", &[("rotWithShape", "1")]);
    x.start("a:gsLst", &[]);
    emit_gs(&mut x, "0", Some("50000"), "300000", None);
    emit_gs(&mut x, "35000", Some("37000"), "300000", None);
    emit_gs(&mut x, "100000", Some("15000"), "350000", None);
    x.end("a:gsLst");
    x.start("a:lin", &[("ang", "16200000"), ("scaled", "1")]);
    x.end("a:lin");
    x.end("a:gradFill");
    x.start("a:gradFill", &[("rotWithShape", "1")]);
    x.start("a:gsLst", &[]);
    emit_gs(&mut x, "0", None, "130000", Some("51000"));
    emit_gs(&mut x, "80000", None, "130000", Some("93000"));
    emit_gs(&mut x, "100000", None, "135000", Some("94000"));
    x.end("a:gsLst");
    x.start("a:lin", &[("ang", "16200000"), ("scaled", "0")]);
    x.end("a:lin");
    x.end("a:gradFill");
    x.end("a:fillStyleLst");

    x.start("a:lnStyleLst", &[]);
    emit_ln_style(&mut x, "6350");
    emit_ln_style(&mut x, "12700");
    emit_ln_style(&mut x, "19050");
    x.end("a:lnStyleLst");

    x.start("a:effectStyleLst", &[]);
    emit_effect_style(&mut x, false);
    emit_effect_style(&mut x, false);
    emit_effect_style(&mut x, true);
    x.end("a:effectStyleLst");

    x.start("a:bgFillStyleLst", &[]);
    x.start("a:solidFill", &[]);
    x.leaf("a:schemeClr", &[("val", "phClr")]);
    x.end("a:solidFill");
    x.start("a:gradFill", &[("rotWithShape", "1")]);
    x.start("a:gsLst", &[]);
    emit_gs(&mut x, "0", Some("40000"), "350000", None);
    emit_gs(&mut x, "40000", Some("45000"), "350000", Some("99000"));
    emit_gs(&mut x, "100000", None, "255000", Some("20000"));
    x.end("a:gsLst");
    x.start("a:path", &[("path", "circle")]);
    x.leaf(
        "a:fillToRect",
        &[
            ("l", "50000"),
            ("t", "-80000"),
            ("r", "50000"),
            ("b", "180000"),
        ],
    );
    x.end("a:path");
    x.end("a:gradFill");
    // PowerPoint always ships three background fills; add the standard
    // corner-glow gradient as the third entry.
    x.start("a:gradFill", &[("rotWithShape", "1")]);
    x.start("a:gsLst", &[]);
    emit_gs(&mut x, "0", None, "350000", Some("75000"));
    emit_gs(&mut x, "40000", None, "350000", Some("80000"));
    emit_gs(&mut x, "100000", None, "300000", Some("50000"));
    x.end("a:gsLst");
    x.start("a:path", &[("path", "circle")]);
    x.leaf(
        "a:fillToRect",
        &[
            ("l", "50000"),
            ("t", "80000"),
            ("r", "-80000"),
            ("b", "50000"),
        ],
    );
    x.end("a:path");
    x.end("a:gradFill");
    x.end("a:bgFillStyleLst");

    x.end("a:fmtScheme");
    x.end("a:themeElements");
    x.leaf("a:objectDefaults", &[]);
    x.leaf("a:extraClrSchemeLst", &[]);
    x.end("a:theme");
    x.into_string()
}

/// Default `(latin, ea)` font pair: the first font declared in the theme's
/// text styles, or the spec default `MiSans`.
pub fn default_fonts(theme: Option<&Theme>) -> (String, String) {
    if let Some(font) = theme.and_then(|theme| {
        theme
            .text_styles
            .values()
            .find_map(|style| style.font_family.clone())
    }) {
        return match font {
            FontFamily::Single(name) => (name.clone(), name),
            FontFamily::Bilingual { latin, ea } => (latin, ea),
        };
    }
    ("MiSans".to_owned(), "MiSans".to_owned())
}

fn emit_font_scheme(x: &mut Xml, name: &str, latin: &str, ea: &str) {
    x.start(name, &[]);
    x.leaf("a:latin", &[("typeface", latin)]);
    x.leaf("a:ea", &[("typeface", ea)]);
    x.leaf("a:cs", &[("typeface", "")]);
    x.end(name);
}

fn emit_gs(x: &mut Xml, pos: &str, tint: Option<&str>, sat_mod: &str, shade: Option<&str>) {
    x.start("a:gs", &[("pos", pos)]);
    x.start("a:schemeClr", &[("val", "phClr")]);
    if let Some(tint) = tint {
        x.leaf("a:tint", &[("val", tint)]);
    }
    if let Some(shade) = shade {
        x.leaf("a:shade", &[("val", shade)]);
    }
    x.leaf("a:satMod", &[("val", sat_mod)]);
    x.end("a:schemeClr");
    x.end("a:gs");
}

fn emit_ln_style(x: &mut Xml, width: &str) {
    x.start(
        "a:ln",
        &[
            ("w", width),
            ("cap", "flat"),
            ("cmpd", "sng"),
            ("algn", "ctr"),
        ],
    );
    x.start("a:solidFill", &[]);
    x.leaf("a:schemeClr", &[("val", "phClr")]);
    x.end("a:solidFill");
    x.leaf("a:prstDash", &[("val", "solid")]);
    x.leaf("a:miter", &[("lim", "800000")]);
    x.end("a:ln");
}

fn emit_effect_style(x: &mut Xml, with_shadow: bool) {
    x.start("a:effectStyle", &[]);
    x.start("a:effectLst", &[]);
    if with_shadow {
        x.start(
            "a:outerShdw",
            &[
                ("blurRad", "40000"),
                ("dist", "20000"),
                ("dir", "5400000"),
                ("rotWithShape", "0"),
            ],
        );
        x.start("a:srgbClr", &[("val", "000000")]);
        x.leaf("a:alpha", &[("val", "38000")]);
        x.end("a:srgbClr");
        x.end("a:outerShdw");
    }
    x.end("a:effectLst");
    x.end("a:effectStyle");
}
