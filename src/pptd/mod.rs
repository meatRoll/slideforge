//! The PPTD language: a typed AST, a parser and a validator.
//!
//! PPTD (PPT-DSL) is a YAML-based abstraction layer over OOXML used to
//! describe, generate and edit presentations. The AST in [`ast`] mirrors the
//! format specification so that a project can be parsed, validated and
//! eventually written back losslessly.

pub mod animation;
pub mod ast;
pub mod chart;
pub mod elements;
pub mod parser;
pub mod shared;
pub mod theme;
pub mod validate;

pub use animation::Animation;
pub use ast::{Page, Presentation};
pub use chart::*;
pub use elements::*;
pub use parser::load_project;
pub use shared::*;
pub use theme::*;
pub use validate::{Diagnostic, validate_project};

use std::path::PathBuf;

/// A fully loaded PPTD project: the main entry file plus every page it
/// references, in declaration order.
#[derive(Debug, Clone)]
pub struct Project {
    /// Directory that contains the `.pptd` entry file. All paths inside the
    /// project are relative to this directory.
    pub root_dir: PathBuf,
    /// The parsed main entry file.
    pub presentation: Presentation,
    /// Paths of the page files, parallel to [`Project::pages`].
    pub page_paths: Vec<PathBuf>,
    /// Parsed pages, in the order declared by `presentation.pages`.
    pub pages: Vec<Page>,
}
