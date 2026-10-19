//! Owner paste policy: which representation of a multi-representation clipboard item a paste
//! target uses.
//!
//! A file manager offers names as text plus file paths; a browser offers an image plus text.
//! The owner of a paste target chooses with a [`PastePolicy`], which receives the whole item.
//! The default policy of the standard text editors is [text first](PastePolicy::text_first),
//! identical to ordinary text behavior. A [media first](PastePolicy::media_first) owner uses
//! the first usable image or path list and falls back to text when there is none.

use std::{
    path::{Path, PathBuf},
    rc::Rc,
};

use serde::de::DeserializeOwned;

use crate::{
    ClipboardEntry, ClipboardItem, Image, ImageFormat,
    clipboard::scope::{ScopedContent, ScopedPaste, resolve_paste},
};

/// The representation a policy selected from an item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PasteChoice<'a> {
    /// Paste the item's text (with structure restored when the scope allows it).
    Text,
    /// Paste this image.
    Image(&'a Image),
    /// Paste these file paths.
    Paths(Vec<&'a Path>),
}

/// What a paste inserts, with owned data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PasteContent<R> {
    /// Projections recreated from scoped metadata; see
    /// [`scope`](crate::clipboard::scope).
    Structured(ScopedContent<R>),
    /// Plain text.
    Text(String),
    /// An image, in its original format and bytes.
    Image(Image),
    /// External file paths.
    Paths(Vec<PathBuf>),
}

type CustomPolicy = dyn for<'a> Fn(&'a ClipboardItem, &dyn Fn(&str) -> bool) -> PasteChoice<'a>;

#[derive(Clone)]
enum Kind {
    TextFirst,
    MediaFirst {
        /// Accepted image formats; `None` accepts every format.
        image_formats: Option<Vec<ImageFormat>>,
        paths: bool,
    },
    Custom(Rc<CustomPolicy>),
}

/// Per-owner policy choosing the representation to paste from the whole clipboard item.
///
/// The default is [`PastePolicy::text_first`].
#[derive(Clone)]
pub struct PastePolicy {
    kind: Kind,
}

impl Default for PastePolicy {
    fn default() -> Self {
        Self::text_first()
    }
}

impl std::fmt::Debug for PastePolicy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match &self.kind {
            Kind::TextFirst => "TextFirst",
            Kind::MediaFirst { .. } => "MediaFirst",
            Kind::Custom(_) => "Custom",
        };
        formatter.debug_tuple("PastePolicy").field(&name).finish()
    }
}

impl PastePolicy {
    /// Text is used whenever the item has text (a path-only item has its paths as text);
    /// images alone are not pasted. This is the policy of the standard text editors.
    pub fn text_first() -> Self {
        Self {
            kind: Kind::TextFirst,
        }
    }

    /// The first usable image or path list in the item is used; when there is none the
    /// policy falls back to text. Every image format and file paths are usable unless
    /// restricted with [`with_image_formats`](Self::with_image_formats) or
    /// [`without_paths`](Self::without_paths).
    pub fn media_first() -> Self {
        Self {
            kind: Kind::MediaFirst {
                image_formats: None,
                paths: true,
            },
        }
    }

    /// A policy implemented by `choose`, which receives the whole item and the editor's scope
    /// compatibility predicate and returns the representation to paste.
    pub fn custom(
        choose: impl for<'a> Fn(&'a ClipboardItem, &dyn Fn(&str) -> bool) -> PasteChoice<'a> + 'static,
    ) -> Self {
        Self {
            kind: Kind::Custom(Rc::new(choose)),
        }
    }

    /// Restricts a media-first policy to the given image formats; images of other formats
    /// count as unusable and fall back to text. Has no effect on other policies.
    pub fn with_image_formats(mut self, formats: &[ImageFormat]) -> Self {
        if let Kind::MediaFirst { image_formats, .. } = &mut self.kind {
            *image_formats = Some(formats.to_vec());
        }
        self
    }

    /// Makes a media-first policy ignore file paths. Has no effect on other policies.
    pub fn without_paths(mut self) -> Self {
        if let Kind::MediaFirst { paths, .. } = &mut self.kind {
            *paths = false;
        }
        self
    }

    /// Selects the representation of `item` to paste.
    pub fn choose<'a>(
        &self,
        item: &'a ClipboardItem,
        accepts_scope: &dyn Fn(&str) -> bool,
    ) -> PasteChoice<'a> {
        match &self.kind {
            Kind::TextFirst => PasteChoice::Text,
            Kind::MediaFirst {
                image_formats,
                paths,
            } => item
                .entries
                .iter()
                .find_map(|entry| match entry {
                    ClipboardEntry::Image(image)
                        if image_formats
                            .as_ref()
                            .is_none_or(|formats| formats.contains(&image.format)) =>
                    {
                        Some(PasteChoice::Image(image))
                    }
                    ClipboardEntry::ExternalPaths(external) if *paths => {
                        (!external.paths().is_empty())
                            .then(|| PasteChoice::Paths(item.paths().collect()))
                    }
                    _ => None,
                })
                .unwrap_or(PasteChoice::Text),
            Kind::Custom(choose) => choose(item, accepts_scope),
        }
    }

    /// Resolves what pasting `item` inserts, or `None` when there is nothing to paste (an
    /// item without text when the policy selected text).
    pub fn resolve<R: DeserializeOwned>(
        &self,
        item: &ClipboardItem,
        accepts_scope: &dyn Fn(&str) -> bool,
    ) -> Option<PasteContent<R>> {
        match self.choose(item, accepts_scope) {
            PasteChoice::Text => resolve_paste::<R>(item, accepts_scope).map(|paste| match paste {
                ScopedPaste::Structured(content) => PasteContent::Structured(content),
                ScopedPaste::Plain(text) => PasteContent::Text(text),
            }),
            PasteChoice::Image(image) => Some(PasteContent::Image(image.clone())),
            PasteChoice::Paths(paths) => Some(PasteContent::Paths(
                paths.into_iter().map(Path::to_path_buf).collect(),
            )),
        }
    }

    /// Whether pasting `item` would insert something: used for the availability of paste.
    pub fn accepts(&self, item: &ClipboardItem, accepts_scope: &dyn Fn(&str) -> bool) -> bool {
        match self.choose(item, accepts_scope) {
            PasteChoice::Text => item.text().is_some_and(|text| !text.is_empty()),
            PasteChoice::Image(_) => true,
            PasteChoice::Paths(paths) => !paths.is_empty(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(format: ImageFormat) -> Image {
        Image::from_bytes(format, vec![1, 2, 3])
    }

    fn image_with_text() -> ClipboardItem {
        ClipboardItem::from_entries([
            ClipboardEntry::String("alt text".to_owned().into()),
            ClipboardEntry::Image(image(ImageFormat::Png)),
        ])
    }

    fn accept_all(_: &str) -> bool {
        true
    }

    #[test]
    fn default_policy_is_text_first() {
        let item = image_with_text();
        let policy = PastePolicy::default();
        assert_eq!(
            policy.resolve::<()>(&item, &accept_all),
            Some(PasteContent::Text("alt text".into()))
        );
        // A text editor does not paste a bare image.
        let bare = ClipboardItem::new_image(&image(ImageFormat::Png));
        assert_eq!(policy.resolve::<()>(&bare, &accept_all), None);
        assert!(!policy.accepts(&bare, &accept_all));
    }

    #[test]
    fn media_first_prefers_the_image() {
        let item = image_with_text();
        let policy = PastePolicy::media_first();
        assert_eq!(
            policy.resolve::<()>(&item, &accept_all),
            Some(PasteContent::Image(image(ImageFormat::Png)))
        );
        assert!(policy.accepts(
            &ClipboardItem::new_image(&image(ImageFormat::Gif)),
            &accept_all
        ));
    }

    #[test]
    fn media_first_falls_back_to_text_when_the_image_format_is_unusable() {
        let item = ClipboardItem::from_entries([
            ClipboardEntry::String("alt text".to_owned().into()),
            ClipboardEntry::Image(image(ImageFormat::Tiff)),
        ]);
        let policy = PastePolicy::media_first().with_image_formats(&[ImageFormat::Png]);
        assert_eq!(
            policy.resolve::<()>(&item, &accept_all),
            Some(PasteContent::Text("alt text".into()))
        );
    }

    #[test]
    fn media_first_prefers_paths_and_falls_back_when_paths_are_missing() {
        let with_paths = ClipboardItem::from_entries([
            ClipboardEntry::String("a.txt".to_owned().into()),
            ClipboardEntry::ExternalPaths(crate::ExternalPaths(
                ["/a.txt".into()].into_iter().collect(),
            )),
        ]);
        let policy = PastePolicy::media_first();
        assert_eq!(
            policy.resolve::<()>(&with_paths, &accept_all),
            Some(PasteContent::Paths(vec!["/a.txt".into()]))
        );
        assert_eq!(
            policy
                .clone()
                .without_paths()
                .resolve::<()>(&with_paths, &accept_all),
            Some(PasteContent::Text("a.txt".into()))
        );
        let empty_paths = ClipboardItem::from_entries([
            ClipboardEntry::String("a.txt".to_owned().into()),
            ClipboardEntry::ExternalPaths(crate::ExternalPaths(Default::default())),
        ]);
        assert_eq!(
            policy.resolve::<()>(&empty_paths, &accept_all),
            Some(PasteContent::Text("a.txt".into()))
        );
    }

    #[test]
    fn custom_policy_receives_the_whole_item_and_scope_predicate() {
        let policy = PastePolicy::custom(|item, accepts_scope| {
            if accepts_scope("docs") && item.images().count() == 1 {
                PasteChoice::Image(item.images().next().unwrap())
            } else {
                PasteChoice::Text
            }
        });
        let item = image_with_text();
        assert!(matches!(
            policy.resolve::<()>(&item, &|scope| scope == "docs"),
            Some(PasteContent::Image(_))
        ));
        assert!(matches!(
            policy.resolve::<()>(&item, &|_| false),
            Some(PasteContent::Text(_))
        ));
    }
}
