//! A flash image read as the receivers' CPU32 reads it, and what the
//! tools that read one share: loading it from a file and naming it.

use std::path::Path;
use std::path::PathBuf;

use anyhow::Context as _;
use anyhow::Result;

/// A firmware image, loaded at address zero: a stored pointer is a file
/// offset as it stands.
pub(crate) struct Image<'a>(pub(crate) &'a [u8]);

impl Image<'_> {
    /// The byte at `at`.
    pub(crate) fn u8(&self, at: u32) -> Option<u8> {
        self.0.get(usize::try_from(at).ok()?).copied()
    }

    /// The big-endian word at `at`.
    pub(crate) fn u16(&self, at: u32) -> Option<u16> {
        let at = usize::try_from(at).ok()?;
        Some(u16::from_be_bytes(self.0.get(at..at + 2)?.try_into().ok()?))
    }

    /// The big-endian long at `at`.
    pub(crate) fn u32(&self, at: u32) -> Option<u32> {
        let at = usize::try_from(at).ok()?;
        Some(u32::from_be_bytes(self.0.get(at..at + 4)?.try_into().ok()?))
    }

    /// Whether `at` is inside the image.
    pub(crate) fn holds(&self, at: u32) -> bool {
        usize::try_from(at).is_ok_and(|at| at < self.0.len())
    }

    /// Every place `needle` occurs.
    pub(crate) fn find(&self, needle: &[u8]) -> Vec<u32> {
        self.0
            .windows(needle.len())
            .enumerate()
            .filter(|(_, window)| *window == needle)
            .filter_map(|(at, _)| u32::try_from(at).ok())
            .collect()
    }

    /// Every even place that holds a pointer to `target`.
    pub(crate) fn pointers_to(&self, target: u32) -> Vec<u32> {
        self.find(&target.to_be_bytes())
            .into_iter()
            .filter(|at| at.is_multiple_of(2))
            .collect()
    }
}

/// An image's name: its file name without the extension.
pub(crate) fn name(path: &Path) -> Result<String> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_owned)
        .with_context(|| format!("{} has no usable name", path.display()))
}

/// The image at `path`, read by `parse`.
pub(crate) fn load<T>(path: &Path, parse: impl Fn(&[u8]) -> Result<T>) -> Result<T> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    parse(&bytes).with_context(|| format!("reading {}", path.display()))
}

/// Each image of `paths` by name, read by `parse`.
pub(crate) fn load_all<T>(
    paths: &[PathBuf],
    parse: impl Fn(&[u8]) -> Result<T>,
) -> Result<Vec<(String, T)>> {
    paths
        .iter()
        .map(|path| Ok((name(path)?, load(path, &parse)?)))
        .collect()
}
