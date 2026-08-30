//! SlideForge — edit and build PPTX presentations from the PPTD language.
//!
//! The crate is split in two halves, mirroring the two sides of the
//! conversion:
//!
//! * [`pptd`] — a typed AST, a parser and a validator for the PPTD format
//!   (the editable YAML-based presentation language).
//! * [`pptx`] — the OOXML / OPC output pipeline that turns a validated
//!   [`pptd::Project`] into a `.pptx` package (work in progress).
//!
//! The binary entry point lives in [`cli`].

pub mod cli;
pub mod error;
pub mod hash;
pub mod pptd;
pub mod pptx;

pub use error::{Error, Result};
