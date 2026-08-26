//! Media assets: image dimension sniffing and the package media registry.
//!
//! Image dimensions are read straight from the PNG/JPEG headers (no
//! dependencies) — they are needed to compute `contain`/`cover` cropping for
//! `a:srcRect`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::package::ContentType;
use crate::{Error, Result};

/// Pixel dimensions of a raster image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageSize {
    pub width: u32,
    pub height: u32,
}

/// Sniff PNG or JPEG dimensions from the file bytes.
pub fn sniff_size(data: &[u8]) -> Option<ImageSize> {
    png_size(data).or_else(|| jpeg_size(data))
}

fn png_size(data: &[u8]) -> Option<ImageSize> {
    const SIG: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    if data.len() >= 24 && data[0..8] == SIG && &data[12..16] == b"IHDR" {
        let width = u32::from_be_bytes(data[16..20].try_into().ok()?);
        let height = u32::from_be_bytes(data[20..24].try_into().ok()?);
        Some(ImageSize { width, height })
    } else {
        None
    }
}

fn jpeg_size(data: &[u8]) -> Option<ImageSize> {
    if data.get(0..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2usize;
    while i + 3 < data.len() {
        while i < data.len() && data[i] == 0xFF {
            i += 1;
        }
        let marker = *data.get(i)?;
        i += 1;
        match marker {
            0xD9 => break, // EOI
            0x01 => continue,
            0xD0..=0xD8 => continue, // RSTn / SOI / EOI handled above
            _ => {}
        }
        if i + 2 > data.len() {
            break;
        }
        let len = u16::from_be_bytes([data[i], data[i + 1]]) as usize;
        if len < 2 {
            break;
        }
        // SOF markers share the same header layout: precision, height, width.
        let is_sof = (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC); // DHT / JPG / DAC
        if is_sof && i + 2 + 5 <= data.len() {
            let height = u16::from_be_bytes([data[i + 3], data[i + 4]]) as u32;
            let width = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
            return Some(ImageSize { width, height });
        }
        i += 2 + len.saturating_sub(2);
    }
    None
}

/// One raster part staged for the OPC package.
#[derive(Debug, Clone)]
pub struct MediaPart {
    /// Original source path inside the deck (used for deduplication).
    pub src: String,
    /// Path in the ZIP archive, e.g. `ppt/media/image3.png`.
    pub package_path: String,
    pub extension: String,
    pub content_type: ContentType,
    pub data: Vec<u8>,
    pub size: ImageSize,
}

/// Deduplicates and numbers the media parts referenced by a build.
#[derive(Debug)]
pub struct MediaRegistry {
    root_dir: PathBuf,
    parts: Vec<MediaPart>,
    index: HashMap<String, usize>,
}

impl MediaRegistry {
    pub fn new(root_dir: &Path) -> Self {
        Self {
            root_dir: root_dir.to_path_buf(),
            parts: Vec::new(),
            index: HashMap::new(),
        }
    }

    /// Register `src` (relative to the deck directory) and return its global
    /// part index. Repeated sources reuse the existing part.
    pub fn index_of(&mut self, src: &str) -> Result<usize> {
        if let Some(&index) = self.index.get(src) {
            return Ok(index);
        }

        let path = self.root_dir.join(src);
        let data = std::fs::read(&path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;

        let extension = match Path::new(&path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
        {
            Some(ext) if ext == "png" || ext == "jpg" || ext == "jpeg" => ext,
            Some(other) => {
                return Err(Error::Unsupported(format!(
                    "media `{src}` uses unsupported image extension `{other}` (png/jpg/jpeg)"
                )));
            }
            None => {
                return Err(Error::Unsupported(format!(
                    "media `{src}` has no image extension (png/jpg/jpeg)"
                )));
            }
        };
        let content_type = if extension == "png" {
            ContentType::ImagePng
        } else {
            ContentType::ImageJpeg
        };
        let size = sniff_size(&data).ok_or_else(|| {
            Error::Unsupported(format!("media `{src}` is not a readable PNG/JPEG image"))
        })?;

        let index = self.parts.len();
        let package_path = format!("ppt/media/image{}.{extension}", index + 1);
        self.parts.push(MediaPart {
            src: src.to_owned(),
            package_path,
            extension,
            content_type,
            data,
            size,
        });
        self.index.insert(src.to_owned(), index);
        Ok(index)
    }

    /// The registered part at `index`.
    pub fn part(&self, index: usize) -> &MediaPart {
        &self.parts[index]
    }

    /// Consume the registry and return all staged media parts.
    pub fn into_parts(self) -> Vec<MediaPart> {
        self.parts
    }

    /// Unique lowercase extensions used (`png` / `jpg` / `jpeg`).
    pub fn extensions(&self) -> Vec<String> {
        let mut seen: Vec<String> = Vec::new();
        for part in &self.parts {
            if !seen.contains(&part.extension) {
                seen.push(part.extension.clone());
            }
        }
        seen
    }
}
