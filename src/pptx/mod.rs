//! The OOXML / OPC output pipeline: PPTD AST → `.pptx` package.
//!
//! A `.pptx` is a ZIP archive following the OPC (Open Packaging
//! Conventions) rules; the parts we plan to produce are modelled in
//! [`package`], and the AST → OOXML rendering lives in [`writer`].

pub mod package;
pub mod writer;
