//! Rich clipboard support: items carrying text, images and file paths, scoped structured
//! copy and paste, owner paste policies, the standard edit actions and asynchronous image
//! helpers.
//!
//! The item types ([`ClipboardItem`], [`ClipboardEntry`], [`ClipboardString`], [`Image`]) live
//! with the platform abstraction; this module adds the behavior built on them:
//!
//! * [`scope`]: copy text together with versioned, scope-tagged projection metadata and
//!   decide at paste time whether the structure may be restored.
//! * [`policy`]: per-owner choice between text, images and file paths.
//! * [`edit`]: the standard cut, copy, paste and select-all actions and their availability.
//! * [`image_tasks`]: image encoding and decoding off the UI thread with stale-result discard.

pub mod edit;
pub mod image_tasks;
pub mod policy;
pub mod scope;

use std::path::{Path, PathBuf};

use crate::{ClipboardEntry, ClipboardItem, ClipboardString, ExternalPaths, Image};

impl ClipboardItem {
    /// Create a new item holding a list of external file paths.
    ///
    /// Platforms that can offer file lists to other applications do so; every platform can
    /// still offer the paths as newline-joined text (see [`ClipboardItem::text`]).
    pub fn new_paths(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            entries: vec![ClipboardEntry::ExternalPaths(ExternalPaths(
                paths.into_iter().collect(),
            ))],
        }
    }

    /// Create an item from several entries, kept in the given order.
    pub fn from_entries(entries: impl IntoIterator<Item = ClipboardEntry>) -> Self {
        Self {
            entries: entries.into_iter().collect(),
        }
    }

    /// Whether the item has at least one string entry.
    pub fn has_text_entry(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| matches!(entry, ClipboardEntry::String(_)))
    }

    /// The string entries of the item, in order.
    pub fn strings(&self) -> impl Iterator<Item = &ClipboardString> {
        self.entries.iter().filter_map(|entry| match entry {
            ClipboardEntry::String(string) => Some(string),
            _ => None,
        })
    }

    /// The images of the item, in order.
    pub fn images(&self) -> impl Iterator<Item = &Image> {
        self.entries.iter().filter_map(|entry| match entry {
            ClipboardEntry::Image(image) => Some(image),
            _ => None,
        })
    }

    /// The external file paths of all path entries of the item, in order.
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.entries
            .iter()
            .filter_map(|entry| match entry {
                ClipboardEntry::ExternalPaths(paths) => Some(paths.paths()),
                _ => None,
            })
            .flatten()
            .map(PathBuf::as_path)
    }

    /// Whether the item carries nothing a consumer could use: no entry, or only empty text.
    pub fn is_empty(&self) -> bool {
        self.entries.iter().all(|entry| match entry {
            ClipboardEntry::String(string) => string.text.is_empty(),
            ClipboardEntry::Image(_) => false,
            ClipboardEntry::ExternalPaths(paths) => paths.paths().is_empty(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ImageFormat;

    #[test]
    fn path_only_item_text_is_newline_joined_absolute_paths() {
        let item = ClipboardItem::new_paths([PathBuf::from("/tmp/a b"), PathBuf::from("/tmp/c")]);
        assert_eq!(item.text().as_deref(), Some("/tmp/a b\n/tmp/c"));
        assert_eq!(item.paths().count(), 2);
    }

    #[test]
    fn text_entries_take_precedence_over_paths_in_text() {
        let item = ClipboardItem::from_entries([
            ClipboardEntry::ExternalPaths(ExternalPaths(["/x".into()].into_iter().collect())),
            ClipboardEntry::String(ClipboardString::new("names".into())),
        ]);
        assert_eq!(item.text().as_deref(), Some("names"));
    }

    #[test]
    fn image_only_item_has_no_text() {
        let image = Image::from_bytes(ImageFormat::Png, vec![1, 2, 3]);
        let item = ClipboardItem::new_image(&image);
        assert_eq!(item.text(), None);
        assert!(!item.is_empty());
        assert_eq!(item.images().count(), 1);
    }

    #[test]
    fn ico_uses_x_icon_with_aliases() {
        assert_eq!(ImageFormat::Ico.mime_type(), "image/x-icon");
        let mimes: Vec<_> = ImageFormat::Ico.mime_types().collect();
        assert_eq!(
            mimes,
            ["image/x-icon", "image/ico", "image/vnd.microsoft.icon"]
        );
        assert_eq!(
            ImageFormat::from_mime_type("image/vnd.microsoft.icon"),
            Some(ImageFormat::Ico)
        );
    }
}
