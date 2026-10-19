//! File-identity icons (ELM-004 R18-R22).
//!
//! Icon selection is a deterministic pure function of a [`FileIconIdentity`] and an icon theme
//! ([`FileIconTheme`], data in the common editor file-icon-theme manifest shape). No artwork
//! is bundled: the host supplies the SVG artwork through the application's asset source and
//! the manifest that maps names to it.

use crate::{
    App, AssetSource, Global, ImageSource, RenderImage, Resource, SharedString, WindowAppearance,
};
use serde::Deserialize;
use std::{borrow::Cow, collections::HashMap, sync::Arc};

/// Asset path of the generic file icon, used when the theme defines no default file icon or
/// no theme is installed.
pub const GENERIC_FILE_ICON_PATH: &str = "icons/file.svg";
/// Asset path of the generic (closed) folder icon.
pub const GENERIC_FOLDER_ICON_PATH: &str = "icons/folder.svg";
/// Asset path of the generic open folder icon.
pub const GENERIC_FOLDER_OPEN_ICON_PATH: &str = "icons/folder_open.svg";

/// What kind of file-system entry an identity names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileIconKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link (resolves like a file, by its own name).
    SymbolicLink,
}

/// The current appearance for icon selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IconAppearance {
    /// Light appearance: authored colors as they are.
    Light,
    /// Dark appearance: palette colors are remapped (see [`remap_dark_palette`]).
    Dark,
}

impl From<WindowAppearance> for IconAppearance {
    fn from(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Light | WindowAppearance::VibrantLight => IconAppearance::Light,
            WindowAppearance::Dark | WindowAppearance::VibrantDark => IconAppearance::Dark,
        }
    }
}

/// The identity an icon is chosen for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FileIconIdentity {
    /// File, directory or symbolic link.
    pub kind: FileIconKind,
    /// A name or path; only the final component (after the last `/` or `\`) is used.
    pub name: SharedString,
    /// Whether a directory is expanded.
    pub expanded: bool,
    /// Optional syntax-language identifier.
    pub language: Option<SharedString>,
    /// Optional MIME type.
    pub mime_type: Option<SharedString>,
}

impl FileIconIdentity {
    fn new(kind: FileIconKind, name: impl Into<SharedString>, expanded: bool) -> Self {
        Self {
            kind,
            name: name.into(),
            expanded,
            language: None,
            mime_type: None,
        }
    }

    /// A regular file.
    pub fn file(name: impl Into<SharedString>) -> Self {
        Self::new(FileIconKind::File, name, false)
    }

    /// A directory with an expanded flag.
    pub fn directory(name: impl Into<SharedString>, expanded: bool) -> Self {
        Self::new(FileIconKind::Directory, name, expanded)
    }

    /// A symbolic link.
    pub fn symbolic_link(name: impl Into<SharedString>) -> Self {
        Self::new(FileIconKind::SymbolicLink, name, false)
    }

    /// Adds a syntax-language identifier.
    pub fn with_language(mut self, language: impl Into<SharedString>) -> Self {
        self.language = Some(language.into());
        self
    }

    /// Adds a MIME type.
    pub fn with_mime_type(mut self, mime_type: impl Into<SharedString>) -> Self {
        self.mime_type = Some(mime_type.into());
        self
    }
}

/// An icon definition of the manifest: the artwork path.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileIconDefinition {
    /// The artwork path (an asset path of the application's asset source).
    #[serde(default)]
    pub icon_path: Option<String>,
}

/// The file-icon-theme manifest in the common editor shape.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileIconThemeManifest {
    /// Icon ids mapped to their definitions.
    #[serde(default)]
    pub icon_definitions: HashMap<String, FileIconDefinition>,
    /// The default file icon id.
    #[serde(default)]
    pub file: Option<String>,
    /// The default folder icon id.
    #[serde(default)]
    pub folder: Option<String>,
    /// The default open-folder icon id.
    #[serde(default)]
    pub folder_expanded: Option<String>,
    /// File extension (without dot) to icon id.
    #[serde(default)]
    pub file_extensions: HashMap<String, String>,
    /// File name to icon id.
    #[serde(default)]
    pub file_names: HashMap<String, String>,
    /// Folder name to icon id.
    #[serde(default)]
    pub folder_names: HashMap<String, String>,
    /// Folder name to open-folder icon id.
    #[serde(default)]
    pub folder_names_expanded: HashMap<String, String>,
    /// Language identifier to icon id.
    #[serde(default)]
    pub language_ids: HashMap<String, String>,
    /// MIME type to icon id (used by the MIME fallback).
    #[serde(default)]
    pub mime_types: HashMap<String, String>,
}

/// A loaded icon theme: the manifest with its extension, file-name and folder-name keys
/// lowercased, plus optional alias fixes for definitions the theme references but omits.
#[derive(Clone, Debug, Default)]
pub struct FileIconTheme {
    definitions: HashMap<String, String>,
    aliases: HashMap<String, String>,
    file: Option<String>,
    folder: Option<String>,
    folder_expanded: Option<String>,
    file_extensions: HashMap<String, String>,
    file_names: HashMap<String, String>,
    folder_names: HashMap<String, String>,
    folder_names_expanded: HashMap<String, String>,
    language_ids: HashMap<String, String>,
    mime_types: HashMap<String, String>,
}

fn lowercase_keys(table: HashMap<String, String>) -> HashMap<String, String> {
    table
        .into_iter()
        .map(|(key, value)| (key.to_lowercase(), value))
        .collect()
}

impl FileIconTheme {
    /// Builds a theme from a parsed manifest.
    pub fn from_manifest(manifest: FileIconThemeManifest) -> Self {
        Self {
            definitions: manifest
                .icon_definitions
                .into_iter()
                .filter_map(|(id, definition)| {
                    definition
                        .icon_path
                        .filter(|path| !path.is_empty())
                        .map(|path| (id, path))
                })
                .collect(),
            aliases: HashMap::default(),
            file: manifest.file,
            folder: manifest.folder,
            folder_expanded: manifest.folder_expanded,
            file_extensions: lowercase_keys(manifest.file_extensions),
            file_names: lowercase_keys(manifest.file_names),
            folder_names: lowercase_keys(manifest.folder_names),
            folder_names_expanded: lowercase_keys(manifest.folder_names_expanded),
            language_ids: manifest.language_ids.into_iter().collect(),
            mime_types: lowercase_keys(manifest.mime_types),
        }
    }

    /// Parses a manifest from JSON text.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        Ok(Self::from_manifest(serde_json::from_str(json)?))
    }

    /// Makes `missing_id` (referenced by the theme but without artwork of its own) resolve to
    /// the artwork of `existing_id`.
    pub fn with_alias(
        mut self,
        missing_id: impl Into<String>,
        existing_id: impl Into<String>,
    ) -> Self {
        self.aliases.insert(missing_id.into(), existing_id.into());
        self
    }

    /// The artwork of definition `id`; a definition without artwork is absent (unless an
    /// alias fix supplies it).
    fn artwork(&self, id: &str) -> Option<&str> {
        self.definitions
            .get(id)
            .or_else(|| {
                self.aliases
                    .get(id)
                    .and_then(|target| self.definitions.get(target))
            })
            .map(String::as_str)
    }

    fn lookup(&self, table: &HashMap<String, String>, key: &str) -> Option<SharedString> {
        table
            .get(key)
            .and_then(|id| self.artwork(id))
            .map(|path| SharedString::from(path.to_string()))
    }

    /// Resolves the icon for `identity`; the result is an asset path and is the same for both
    /// appearances (the dark variant only remaps palette colors in the artwork, see
    /// [`remap_dark_palette`]).
    ///
    /// Files and symbolic links: exact file name, longest compound extension, language,
    /// MIME type, generic file icon. Directories: folder-name table, then the generic folder
    /// icon; expanded directories prefer the open-folder variant when the theme defines one.
    pub fn resolve(&self, identity: &FileIconIdentity) -> SharedString {
        let basename = basename_lowercase(&identity.name);
        match identity.kind {
            FileIconKind::Directory => self.resolve_directory(&basename, identity.expanded),
            FileIconKind::File | FileIconKind::SymbolicLink => self
                .resolve_by_name(&basename)
                .or_else(|| self.resolve_by_language(identity.language.as_deref()))
                .or_else(|| self.resolve_by_mime(identity.mime_type.as_deref()))
                .unwrap_or_else(|| self.generic_file()),
        }
    }

    fn resolve_by_name(&self, basename: &str) -> Option<SharedString> {
        if let Some(path) = self.lookup(&self.file_names, basename) {
            return Some(path);
        }
        // Every suffix right after a "." is a candidate, from the longest to the shortest.
        for (dot, _) in basename.match_indices('.') {
            let candidate = &basename[dot + 1..];
            if candidate.is_empty() {
                continue;
            }
            if let Some(path) = self.lookup(&self.file_extensions, candidate) {
                return Some(path);
            }
        }
        None
    }

    fn resolve_by_language(&self, language: Option<&str>) -> Option<SharedString> {
        let language = language?;
        self.lookup(&self.language_ids, language).or_else(|| {
            self.language_ids
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(language))
                .and_then(|(_, id)| self.artwork(id))
                .map(|path| SharedString::from(path.to_string()))
        })
    }

    fn resolve_by_mime(&self, mime_type: Option<&str>) -> Option<SharedString> {
        let mime = mime_type?.split(';').next()?.trim().to_lowercase();
        self.lookup(&self.mime_types, &mime)
    }

    fn generic_file(&self) -> SharedString {
        self.file
            .as_deref()
            .and_then(|id| self.artwork(id))
            .unwrap_or(GENERIC_FILE_ICON_PATH)
            .to_string()
            .into()
    }

    fn resolve_directory(&self, basename: &str, expanded: bool) -> SharedString {
        let named_closed = self.lookup(&self.folder_names, basename);
        if expanded {
            if let Some(path) = self.lookup(&self.folder_names_expanded, basename) {
                return path;
            }
            if let Some(path) = named_closed {
                return path;
            }
            if let Some(path) = self
                .folder_expanded
                .as_deref()
                .and_then(|id| self.artwork(id))
            {
                return path.to_string().into();
            }
        } else if let Some(path) = named_closed {
            return path;
        }
        self.folder
            .as_deref()
            .and_then(|id| self.artwork(id))
            .map(|path| path.to_string())
            .unwrap_or_else(|| GENERIC_FOLDER_ICON_PATH.to_string())
            .into()
    }

    /// Whether `file_name` resolves to a specific themed identity (an exact file-name or an
    /// extension entry with artwork) rather than the generic file icon, so prose renderers can
    /// decorate standalone file names without treating every dotted token as a file.
    pub fn is_specific_file_name(&self, file_name: &str) -> bool {
        self.resolve_by_name(&basename_lowercase(file_name))
            .is_some()
    }
}

fn basename_lowercase(name: &str) -> String {
    name.rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .to_lowercase()
}

/// An optional pluggable file-icon resolver: identity and appearance to an asset path.
pub trait FileIconResolver: 'static + Send + Sync {
    /// Chooses the asset path of the icon for `identity`.
    fn resolve(&self, identity: &FileIconIdentity, appearance: IconAppearance) -> SharedString;
}

impl FileIconResolver for FileIconTheme {
    fn resolve(&self, identity: &FileIconIdentity, _appearance: IconAppearance) -> SharedString {
        FileIconTheme::resolve(self, identity)
    }
}

struct GlobalFileIconResolver(Arc<dyn FileIconResolver>);

impl Global for GlobalFileIconResolver {}

/// Installs the file-icon resolver for the application.
pub fn set_file_icon_resolver(cx: &mut App, resolver: Arc<dyn FileIconResolver>) {
    cx.set_global(GlobalFileIconResolver(resolver));
}

/// Resolves the asset path for `identity`: the installed resolver, or the generic file and
/// folder icons when none is installed.
pub fn resolve_file_icon(
    cx: &App,
    identity: &FileIconIdentity,
    appearance: IconAppearance,
) -> SharedString {
    match cx.try_global::<GlobalFileIconResolver>() {
        Some(resolver) => resolver.0.resolve(identity, appearance),
        None => match identity.kind {
            FileIconKind::Directory if identity.expanded => GENERIC_FOLDER_OPEN_ICON_PATH.into(),
            FileIconKind::Directory => GENERIC_FOLDER_ICON_PATH.into(),
            FileIconKind::File | FileIconKind::SymbolicLink => GENERIC_FILE_ICON_PATH.into(),
        },
    }
}

/// The seven palette pairs the dark variant remaps (light color, dark color).
pub const DARK_PALETTE_REMAP: [(&str, &str); 7] = [
    ("#64748B", "#CBD5E1"),
    ("#71717A", "#D4D4D8"),
    ("#2563EB", "#60A5FA"),
    ("#EA580C", "#FB923C"),
    ("#16A34A", "#4ADE80"),
    ("#8B5CF6", "#A78BFA"),
    ("#A855F7", "#C084FC"),
];

/// The dark variant of an icon's SVG text: a case-insensitive hexadecimal replacement of the
/// [`DARK_PALETTE_REMAP`] pairs. Geometry, bright colors, transparency and gradients are
/// untouched.
pub fn remap_dark_palette(svg: &str) -> Cow<'_, str> {
    let lower = svg.to_ascii_lowercase();
    if !DARK_PALETTE_REMAP
        .iter()
        .any(|(from, _)| lower.contains(&from.to_ascii_lowercase()))
    {
        return Cow::Borrowed(svg);
    }
    // ASCII lowercasing preserves byte offsets, so matches in `lower` index into `svg`.
    let mut out = String::with_capacity(svg.len());
    let mut index = 0;
    'scan: while index < svg.len() {
        if svg.as_bytes()[index] == b'#' {
            for (from, to) in DARK_PALETTE_REMAP {
                let end = index + from.len();
                if end <= lower.len() && lower[index..end] == from.to_ascii_lowercase() {
                    out.push_str(to);
                    index = end;
                    continue 'scan;
                }
            }
        }
        let ch = svg[index..]
            .chars()
            .next()
            .expect("index is a char boundary");
        out.push(ch);
        index += ch.len_utf8();
    }
    Cow::Owned(out)
}

/// The SVG artwork of `asset_path` for `appearance` (dark palette remap applied for dark).
fn icon_svg_for_appearance(
    asset_source: &dyn AssetSource,
    asset_path: &str,
    appearance: IconAppearance,
) -> Option<Vec<u8>> {
    let bytes = asset_source.load(asset_path).ok().flatten()?;
    match appearance {
        IconAppearance::Light => Some(bytes.into_owned()),
        IconAppearance::Dark => {
            let text = std::str::from_utf8(&bytes).ok()?;
            Some(remap_dark_palette(text).into_owned().into_bytes())
        }
    }
}

/// Icons rasterized once per asset path and appearance.
#[derive(Default)]
pub struct FileIconRasterCache {
    images: parking_lot::Mutex<HashMap<(SharedString, IconAppearance), Option<Arc<RenderImage>>>>,
}

impl Global for FileIconRasterCache {}

impl FileIconRasterCache {
    /// The icon at `asset_path` for `appearance`, rasterized by `renderer` from
    /// `asset_source` on first use (color-preserving, at twice the declared size) and shared
    /// afterwards. An unavailable icon is cached as such.
    pub fn image(
        &self,
        asset_source: &dyn AssetSource,
        renderer: &crate::SvgRenderer,
        asset_path: &str,
        appearance: IconAppearance,
    ) -> Option<Arc<RenderImage>> {
        let key = (SharedString::from(asset_path.to_string()), appearance);
        if let Some(cached) = self.images.lock().get(&key) {
            return cached.clone();
        }
        let rendered = icon_svg_for_appearance(asset_source, asset_path, appearance)
            .and_then(|bytes| renderer.render_single_frame(&bytes, 1.0).ok());
        self.images.lock().insert(key, rendered.clone());
        rendered
    }
}

/// The icon rasterized once per asset path and appearance, for painting directly inside a text
/// layout where an image element cannot sit.
pub fn file_icon_image(
    cx: &mut App,
    asset_path: &str,
    appearance: IconAppearance,
) -> Option<Arc<RenderImage>> {
    if !cx.has_global::<FileIconRasterCache>() {
        cx.set_global(FileIconRasterCache::default());
    }
    let renderer = cx.svg_renderer();
    let asset_source = cx.asset_source().clone();
    cx.global::<FileIconRasterCache>().image(
        asset_source.as_ref(),
        &renderer,
        asset_path,
        appearance,
    )
}

/// An image source showing the file icon for `identity` through the color-preserving image
/// path. Light icons stream from the asset path; dark icons use the remapped, rasterized
/// artwork. An unavailable icon is a failing source (the image element's fallback shows).
pub fn file_icon_source(
    cx: &mut App,
    identity: &FileIconIdentity,
    appearance: IconAppearance,
) -> ImageSource {
    let path = resolve_file_icon(cx, identity, appearance);
    match file_icon_image(cx, &path, appearance) {
        Some(image) => ImageSource::Render(image),
        None => ImageSource::Resource(Resource::Embedded(path)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SvgRenderer;

    const THEME: &str = r#"{
        "iconDefinitions": {
            "_file": {"iconPath": "icons/file.svg"},
            "_folder": {"iconPath": "icons/folder.svg"},
            "_folder_open": {"iconPath": "icons/folder_open.svg"},
            "_ts": {"iconPath": "icons/ts.svg"},
            "_test_ts": {"iconPath": "icons/test_ts.svg"},
            "_cargo": {"iconPath": "icons/cargo.svg"},
            "_rust": {"iconPath": "icons/rust.svg"},
            "_pdf": {"iconPath": "icons/pdf.svg"},
            "_src": {"iconPath": "icons/src.svg"},
            "_noart": {},
            "_alias_target": {"iconPath": "icons/alias.svg"}
        },
        "file": "_file",
        "folder": "_folder",
        "folderExpanded": "_folder_open",
        "fileExtensions": {"TS": "_ts", "Test.TS": "_test_ts", "weird": "_noart", "gone": "_missing"},
        "fileNames": {"Cargo.toml": "_cargo"},
        "folderNames": {"SRC": "_src"},
        "languageIds": {"rust": "_rust"},
        "mimeTypes": {"application/pdf": "_pdf"}
    }"#;

    fn theme() -> FileIconTheme {
        FileIconTheme::from_json(THEME).expect("manifest")
    }

    fn file(name: &str) -> FileIconIdentity {
        FileIconIdentity::file(name.to_string())
    }

    #[test]
    fn exact_file_name_beats_extensions() {
        let theme = theme();
        assert_eq!(theme.resolve(&file("Cargo.toml")), "icons/cargo.svg");
        assert_eq!(theme.resolve(&file("CARGO.TOML")), "icons/cargo.svg");
        // Not an exact name match: falls to the generic file icon.
        assert_eq!(theme.resolve(&file("cargo.toml.bak")), "icons/file.svg");
    }

    #[test]
    fn longest_compound_extension_wins() {
        let theme = theme();
        assert_eq!(theme.resolve(&file("a.test.ts")), "icons/test_ts.svg");
        assert_eq!(theme.resolve(&file("a.b.test.ts")), "icons/test_ts.svg");
        assert_eq!(theme.resolve(&file("a.ts")), "icons/ts.svg");
        assert_eq!(theme.resolve(&file("a.TS")), "icons/ts.svg");
        assert_eq!(theme.resolve(&file("a.spec.ts")), "icons/ts.svg");
        // A trailing dot gives an empty candidate, which is skipped; "ts." is not "ts".
        assert_eq!(theme.resolve(&file("a.ts.")), "icons/file.svg");
        assert_eq!(theme.resolve(&file("no_extension")), "icons/file.svg");
    }

    #[test]
    fn path_separators_are_stripped() {
        let theme = theme();
        assert_eq!(
            theme.resolve(&file("dir/sub/Cargo.toml")),
            "icons/cargo.svg"
        );
        assert_eq!(
            theme.resolve(&file("C:\\dir\\a.test.ts")),
            "icons/test_ts.svg"
        );
        assert_eq!(theme.resolve(&file("dir.ts/readme")), "icons/file.svg");
    }

    #[test]
    fn language_then_mime_then_generic() {
        let theme = theme();
        let unknown = file("script");
        assert_eq!(
            theme.resolve(&unknown.clone().with_language("rust")),
            "icons/rust.svg"
        );
        assert_eq!(
            theme.resolve(&unknown.clone().with_language("Rust")),
            "icons/rust.svg"
        );
        // A language missing from the table yields nothing; the MIME type is next.
        assert_eq!(
            theme.resolve(
                &unknown
                    .clone()
                    .with_language("cobol")
                    .with_mime_type("Application/PDF; charset=binary")
            ),
            "icons/pdf.svg"
        );
        assert_eq!(
            theme.resolve(&unknown.with_language("cobol").with_mime_type("text/x")),
            "icons/file.svg"
        );
        // Names and extensions come before the language.
        assert_eq!(
            theme.resolve(&file("a.ts").with_language("rust")),
            "icons/ts.svg"
        );
        // Symbolic links resolve like files, by their own name.
        assert_eq!(
            theme.resolve(&FileIconIdentity::symbolic_link("a.test.ts")),
            "icons/test_ts.svg"
        );
    }

    #[test]
    fn definitions_without_artwork_resolve_as_absent() {
        let theme = theme();
        assert_eq!(theme.resolve(&file("x.weird")), "icons/file.svg");
        assert_eq!(theme.resolve(&file("x.gone")), "icons/file.svg");
        let fixed = FileIconTheme::from_json(THEME)
            .unwrap()
            .with_alias("_noart", "_alias_target")
            .with_alias("_missing", "_ts");
        assert_eq!(fixed.resolve(&file("x.weird")), "icons/alias.svg");
        assert_eq!(fixed.resolve(&file("x.gone")), "icons/ts.svg");
    }

    #[test]
    fn directories_use_folder_names_then_generic_with_open_variant() {
        let theme = theme();
        let dir = |name: &str, expanded| FileIconIdentity::directory(name.to_string(), expanded);
        assert_eq!(theme.resolve(&dir("src", false)), "icons/src.svg");
        assert_eq!(theme.resolve(&dir("a/b\\SRC", false)), "icons/src.svg");
        // No open variant for the named folder: the closed named icon.
        assert_eq!(theme.resolve(&dir("src", true)), "icons/src.svg");
        assert_eq!(theme.resolve(&dir("other", false)), "icons/folder.svg");
        assert_eq!(theme.resolve(&dir("other", true)), "icons/folder_open.svg");

        let no_open = FileIconTheme::from_json(
            r#"{"iconDefinitions":{"_f":{"iconPath":"f.svg"}},"folder":"_f"}"#,
        )
        .unwrap();
        assert_eq!(no_open.resolve(&dir("other", true)), "f.svg");
        let named_open = FileIconTheme::from_json(
            r#"{"iconDefinitions":{"_a":{"iconPath":"a.svg"},"_b":{"iconPath":"b.svg"}},
                "folderNames":{"docs":"_a"},"folderNamesExpanded":{"docs":"_b"}}"#,
        )
        .unwrap();
        assert_eq!(named_open.resolve(&dir("Docs", false)), "a.svg");
        assert_eq!(named_open.resolve(&dir("Docs", true)), "b.svg");
        // An empty theme falls back to the generic artwork paths.
        let empty = FileIconTheme::default();
        assert_eq!(empty.resolve(&dir("x", false)), GENERIC_FOLDER_ICON_PATH);
        assert_eq!(empty.resolve(&file("x")), GENERIC_FILE_ICON_PATH);
    }

    #[test]
    fn predicate_reports_specific_identities() {
        let theme = theme();
        assert!(theme.is_specific_file_name("Cargo.toml"));
        assert!(theme.is_specific_file_name("src/main.test.ts"));
        assert!(theme.is_specific_file_name("a.TS"));
        assert!(!theme.is_specific_file_name("version 1.2"));
        assert!(!theme.is_specific_file_name("x.weird"));
        assert!(!theme.is_specific_file_name("plain"));
    }

    #[test]
    fn appearances_share_asset_identity_and_geometry() {
        let theme = theme();
        let identity = file("a.test.ts");
        assert_eq!(
            FileIconResolver::resolve(&theme, &identity, IconAppearance::Light),
            FileIconResolver::resolve(&theme, &identity, IconAppearance::Dark)
        );

        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16">
            <defs><linearGradient id="g"><stop offset="0" stop-color="#2563eb"/><stop offset="1" stop-color="#ffffff" stop-opacity="0.5"/></linearGradient></defs>
            <path d="M0 0h16v16H0z" fill="#64748B"/><path d="M2 2h4v4H2z" fill="#71717a"/>
            <path d="M3 3h4v4H3z" fill="#Ea580C"/><path d="M4 4h4v4H4z" fill="#16A34A" fill-opacity=".5"/>
            <path d="M5 5h4v4H5z" fill="#8b5cf6"/><path d="M6 6h4v4H6z" fill="#A855F7"/>
            <path d="M7 7h4v4H7z" fill="url(#g)" stroke="#FF8800"/></svg>"##;
        let dark = remap_dark_palette(svg);
        for (from, to) in DARK_PALETTE_REMAP {
            assert!(
                !dark
                    .to_ascii_lowercase()
                    .contains(&from.to_ascii_lowercase()),
                "{from}"
            );
            assert!(dark.contains(to), "{to}");
        }
        // Everything but the seven colors is untouched.
        assert!(dark.contains(r#"d="M0 0h16v16H0z""#));
        assert!(dark.contains("#FF8800") && dark.contains("#ffffff"));
        assert!(dark.contains(r#"stop-opacity="0.5""#) && dark.contains(r#"fill-opacity=".5""#));
        assert!(dark.contains("url(#g)"));
        assert_eq!(remap_dark_palette("<svg/>"), "<svg/>");
        assert!(matches!(remap_dark_palette("<svg/>"), Cow::Borrowed(_)));
        // Non-ASCII text survives.
        assert_eq!(remap_dark_palette("é #2563EB ü"), "é #60A5FA ü");
    }

    struct MemoryAssets(Vec<(&'static str, &'static str)>);

    impl AssetSource for MemoryAssets {
        fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
            Ok(self
                .0
                .iter()
                .find(|(name, _)| *name == path)
                .map(|(_, text)| Cow::Borrowed(text.as_bytes())))
        }

        fn list(&self, _path: &str) -> anyhow::Result<Vec<SharedString>> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn icons_are_rasterized_once_per_asset_path_and_appearance() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16" fill="#2563EB"/></svg>"##;
        let assets = MemoryAssets(vec![("icons/a.svg", svg)]);
        let renderer = SvgRenderer::new(Arc::new(()));
        let cache = FileIconRasterCache::default();
        let light = cache
            .image(&assets, &renderer, "icons/a.svg", IconAppearance::Light)
            .expect("light");
        let light_again = cache
            .image(&assets, &renderer, "icons/a.svg", IconAppearance::Light)
            .expect("light again");
        assert!(Arc::ptr_eq(&light, &light_again));
        let dark = cache
            .image(&assets, &renderer, "icons/a.svg", IconAppearance::Dark)
            .expect("dark");
        assert!(!Arc::ptr_eq(&light, &dark));
        // Same geometry, remapped palette: the stored pixel (BGRA) differs only in color.
        assert_eq!(light.size(0), dark.size(0));
        assert_ne!(light.as_bytes(0), dark.as_bytes(0));
        let middle = (light.size(0).width.0 as usize * (light.size(0).height.0 as usize / 2)
            + light.size(0).width.0 as usize / 2)
            * 4;
        assert_eq!(
            &light.as_bytes(0).unwrap()[middle..middle + 4],
            &[0xEB, 0x63, 0x25, 0xFF]
        );
        assert_eq!(
            &dark.as_bytes(0).unwrap()[middle..middle + 4],
            &[0xFA, 0xA5, 0x60, 0xFF]
        );
        assert!(
            cache
                .image(
                    &assets,
                    &renderer,
                    "icons/missing.svg",
                    IconAppearance::Light
                )
                .is_none()
        );
    }

    #[vespertine::test]
    fn pluggable_resolver_or_generic_icons(cx: &mut crate::TestAppContext) {
        cx.update(|cx| {
            let identity = file("a.test.ts");
            assert_eq!(
                resolve_file_icon(cx, &identity, IconAppearance::Light),
                GENERIC_FILE_ICON_PATH
            );
            assert_eq!(
                resolve_file_icon(
                    cx,
                    &FileIconIdentity::directory("x", true),
                    IconAppearance::Dark
                ),
                GENERIC_FOLDER_OPEN_ICON_PATH
            );
            set_file_icon_resolver(cx, Arc::new(theme()));
            assert_eq!(
                resolve_file_icon(cx, &identity, IconAppearance::Dark),
                "icons/test_ts.svg"
            );
        });
    }
}
