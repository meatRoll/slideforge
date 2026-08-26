//! Loading PPTD projects from disk: YAML files → typed AST.

use std::fs;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;

use super::Project;
use super::ast::{Page, Presentation};
use crate::{Error, Result};

/// The only version accepted by the loader.
pub const SUPPORTED_VERSION: &str = "v2";

/// Load the `.pptd` main entry file plus every page it references.
///
/// The main entry is required: a `.page` file cannot be loaded on its own,
/// because the page list (and therefore the slide order) lives in the `.pptd`.
pub fn load_project(entry: &Path) -> Result<Project> {
    let root_dir = entry
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let presentation: Presentation = read_yaml_file(entry)?;
    if presentation.version != SUPPORTED_VERSION {
        return Err(Error::Invalid(format!(
            "unsupported version {:?} in {}, expected {SUPPORTED_VERSION:?}",
            presentation.version,
            entry.display()
        )));
    }

    let mut pages = Vec::with_capacity(presentation.pages.len());
    let mut page_paths = Vec::with_capacity(presentation.pages.len());
    for relative in &presentation.pages {
        let path = root_dir.join(relative);
        let page: Page = read_yaml_file(&path)?;
        pages.push(page);
        page_paths.push(path);
    }

    Ok(Project {
        root_dir,
        presentation,
        pages,
        page_paths,
    })
}

/// Read and deserialize one YAML file as `T`.
fn read_yaml_file<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_yaml::from_str(&text).map_err(|source| Error::Yaml {
        path: path.to_path_buf(),
        source,
    })
}
