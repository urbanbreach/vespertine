//! Target metadata for captured windows (PLT-012 R3 step 4) and the
//! filesystem boundary used for desktop entries, icons and portal files.
//!
//! Metadata is presentation only and never carries identity decisions.

use std::{
    io,
    path::{Path, PathBuf},
};

use super::{
    budget::{encode_png_rgba, png_header_dimensions, validate_dimensions},
    x11::{WindowId, X11Server, select_icon, window_class_name, window_title_text},
};

/// Display name used when nothing else names an X11 application.
pub const FALLBACK_X11_DISPLAY_NAME: &str = "X11 application";
/// Display name of portal-provided captures, which have no verifiable
/// window identity.
pub const PORTAL_DISPLAY_NAME: &str = "Selected window";
/// Identifier of a capture made by the portal's active-window target.
pub const PORTAL_ACTIVE_WINDOW_IDENTIFIER: &str = "portal:active-window";
/// Identifier of a capture made through the portal's window picker.
pub const PORTAL_SELECTION_IDENTIFIER: &str = "portal:window-selection";

/// Largest desktop entry file read.
const MAX_DESKTOP_ENTRY_BYTES: u64 = 256 * 1024;
/// Largest icon file read.
const MAX_ICON_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// Filesystem access for the capture service.
pub trait DesktopFs: Send + Sync {
    /// Length in bytes of the file at `path`.
    fn file_len(&self, path: &Path) -> io::Result<u64>;
    /// Reads at most `limit` bytes from the start of the file.
    fn read_bounded(&self, path: &Path, limit: u64) -> io::Result<Vec<u8>>;
    /// Entries of a directory (full paths); order is not significant.
    fn list_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>>;
    /// The user's home directory.
    fn home_dir(&self) -> Option<PathBuf>;
}

/// [`DesktopFs`] over `std::fs`.
#[derive(Debug, Default, Clone, Copy)]
pub struct StdDesktopFs;

impl DesktopFs for StdDesktopFs {
    fn file_len(&self, path: &Path) -> io::Result<u64> {
        Ok(std::fs::metadata(path)?.len())
    }

    fn read_bounded(&self, path: &Path, limit: u64) -> io::Result<Vec<u8>> {
        use std::io::Read as _;
        let mut data = Vec::new();
        std::fs::File::open(path)?
            .take(limit)
            .read_to_end(&mut data)?;
        Ok(data)
    }

    fn list_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        std::fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect()
    }

    fn home_dir(&self) -> Option<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

/// How a captured window was identified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    /// Portal active-window target; no verifiable window identity.
    PortalActiveWindow,
    /// Portal picker selection; no verifiable window identity.
    PortalSelection,
    /// A window captured natively on X11.
    X11Window,
}

/// Presentation metadata of a captured window. The icon is never serialized
/// into any prompt or context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetMetadata {
    pub kind: TargetKind,
    pub identifier: String,
    pub display_name: String,
    pub title: Option<String>,
    pub class: Option<String>,
    /// `Name` of the matching desktop entry, if any.
    pub desktop_name: Option<String>,
    /// PNG-encoded icon, if one was found.
    pub icon_png: Option<Vec<u8>>,
}

impl TargetMetadata {
    /// Generic metadata for portal captures.
    pub fn portal(kind: TargetKind) -> Self {
        let identifier = match kind {
            TargetKind::PortalSelection => PORTAL_SELECTION_IDENTIFIER,
            _ => PORTAL_ACTIVE_WINDOW_IDENTIFIER,
        };
        Self {
            kind,
            identifier: identifier.to_owned(),
            display_name: PORTAL_DISPLAY_NAME.to_owned(),
            title: None,
            class: None,
            desktop_name: None,
            icon_png: None,
        }
    }

    /// Recomputes the display name after semantic enrichment named the
    /// application: desktop `Name`, semantic name, class, then the literal
    /// fallback.
    pub fn apply_semantic_application_name(&mut self, semantic: &str) {
        self.display_name = display_name(
            self.desktop_name.as_deref(),
            Some(semantic),
            self.class.as_deref(),
        );
    }
}

/// Display name fallback chain: desktop `Name`, semantic application name,
/// class, then "X11 application". Blank candidates are skipped.
pub fn display_name(
    desktop_name: Option<&str>,
    semantic_name: Option<&str>,
    class: Option<&str>,
) -> String {
    [desktop_name, semantic_name, class]
        .into_iter()
        .flatten()
        .find(|name| !name.trim().is_empty())
        .unwrap_or(FALLBACK_X11_DISPLAY_NAME)
        .to_owned()
}

/// A matched `.desktop` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopEntry {
    /// File stem of the `.desktop` file.
    pub id: String,
    pub name: Option<String>,
    pub icon: Option<String>,
    pub startup_wm_class: Option<String>,
}

/// Reads `Name`, `Icon` and `StartupWMClass` from the `[Desktop Entry]` group
/// only.
pub fn parse_desktop_entry(id: &str, text: &str) -> DesktopEntry {
    let mut entry = DesktopEntry {
        id: id.to_owned(),
        name: None,
        icon: None,
        startup_wm_class: None,
    };
    let mut in_group = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_group = line == "[Desktop Entry]";
            continue;
        }
        if !in_group {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let slot = match key.trim() {
            "Name" => &mut entry.name,
            "Icon" => &mut entry.icon,
            "StartupWMClass" => &mut entry.startup_wm_class,
            _ => continue,
        };
        if slot.is_none() {
            *slot = Some(value.trim().to_owned());
        }
    }
    entry
}

fn desktop_entry_directories(fs: &dyn DesktopFs) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(home) = fs.home_dir() {
        directories.push(home.join(".local/share/applications"));
    }
    directories.push(PathBuf::from("/usr/share/applications"));
    directories.push(PathBuf::from("/usr/local/share/applications"));
    directories
}

/// Scans `~/.local/share/applications`, `/usr/share/applications` and
/// `/usr/local/share/applications` in order for a `.desktop` file whose stem
/// equals `class` or whose `StartupWMClass` equals it, both compared
/// case-insensitively. Files within a directory are visited in name order.
pub fn find_desktop_entry(fs: &dyn DesktopFs, class: &str) -> Option<DesktopEntry> {
    let wanted = class.to_lowercase();
    for directory in desktop_entry_directories(fs) {
        let Ok(mut files) = fs.list_dir(&directory) else {
            continue;
        };
        files.retain(|path| path.extension().is_some_and(|ext| ext == "desktop"));
        files.sort();
        for path in files {
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let Ok(bytes) = fs.read_bounded(&path, MAX_DESKTOP_ENTRY_BYTES) else {
                continue;
            };
            let entry = parse_desktop_entry(stem, &String::from_utf8_lossy(&bytes));
            let stem_matches = stem.to_lowercase() == wanted;
            let wm_class_matches = entry
                .startup_wm_class
                .as_deref()
                .is_some_and(|value| value.to_lowercase() == wanted);
            if stem_matches || wm_class_matches {
                return Some(entry);
            }
        }
    }
    None
}

fn valid_png_icon(bytes: Vec<u8>) -> Option<Vec<u8>> {
    let (width, height) = png_header_dimensions(&bytes).ok()?;
    validate_dimensions(u64::from(width), u64::from(height)).ok()?;
    Some(bytes)
}

/// Loads the icon named by a desktop entry `Icon` value: an absolute path is
/// read directly, a name is looked up in the hicolor/pixmaps locations in
/// order. Only PNG data that passes the buffer budgets is returned.
pub fn load_desktop_icon(fs: &dyn DesktopFs, icon: &str) -> Option<Vec<u8>> {
    let icon = icon.trim();
    if icon.is_empty() {
        return None;
    }
    let candidates: Vec<PathBuf> = if Path::new(icon).is_absolute() {
        vec![PathBuf::from(icon)]
    } else {
        let mut paths = Vec::new();
        if let Some(home) = fs.home_dir() {
            paths.push(home.join(format!(".local/share/icons/hicolor/64x64/apps/{icon}.png")));
        }
        paths.push(PathBuf::from(format!("/usr/share/pixmaps/{icon}.png")));
        paths.push(PathBuf::from(format!(
            "/usr/share/icons/hicolor/64x64/apps/{icon}.png"
        )));
        paths.push(PathBuf::from(format!(
            "/usr/share/icons/hicolor/128x128/apps/{icon}.png"
        )));
        paths
    };
    candidates.into_iter().find_map(|path| {
        fs.read_bounded(&path, MAX_ICON_FILE_BYTES)
            .ok()
            .and_then(valid_png_icon)
    })
}

/// Builds the presentation metadata of an X11 window. Blocking.
pub fn x11_target_metadata(
    server: &dyn X11Server,
    fs: &dyn DesktopFs,
    window: WindowId,
) -> TargetMetadata {
    let title = server
        .window_title(window)
        .ok()
        .and_then(|properties| window_title_text(&properties));
    let class = server
        .window_class(window)
        .ok()
        .flatten()
        .and_then(|raw| window_class_name(&raw));
    let entry = class
        .as_deref()
        .and_then(|class| find_desktop_entry(fs, class));

    let identifier = match (&entry, &class) {
        (Some(entry), _) => entry.id.clone(),
        (None, class) => format!(
            "linux-x11:{}",
            class.as_deref().unwrap_or_default().to_lowercase()
        ),
    };
    let desktop_name = entry.as_ref().and_then(|entry| entry.name.clone());

    let icon_png = server
        .window_icon(window)
        .ok()
        .flatten()
        .and_then(|cardinals| select_icon(&cardinals))
        .and_then(|icon| encode_png_rgba(icon.width, icon.height, &icon.rgba).ok())
        .or_else(|| {
            entry
                .as_ref()
                .and_then(|entry| entry.icon.as_deref())
                .and_then(|icon| load_desktop_icon(fs, icon))
        });

    TargetMetadata {
        kind: TargetKind::X11Window,
        identifier,
        display_name: display_name(desktop_name.as_deref(), None, class.as_deref()),
        title,
        class,
        desktop_name,
        icon_png,
    }
}

#[cfg(test)]
mod tests {
    use crate::capture::test_support::MemoryFs;

    use super::*;

    #[test]
    fn desktop_entry_reads_only_the_main_group() {
        let entry = parse_desktop_entry(
            "app",
            "# comment\n[Desktop Action new]\nName=Wrong\nIcon=wrong\n\n[Desktop Entry]\nName=Right\nName[de]=Falsch\nIcon=right-icon\nStartupWMClass=RightClass\n[Other]\nName=Later\n",
        );
        assert_eq!(entry.name.as_deref(), Some("Right"));
        assert_eq!(entry.icon.as_deref(), Some("right-icon"));
        assert_eq!(entry.startup_wm_class.as_deref(), Some("RightClass"));
    }

    #[test]
    fn entries_match_by_stem_or_startup_class_case_insensitively_in_directory_order() {
        let fs = MemoryFs {
            home: Some(PathBuf::from("/home/u")),
            ..Default::default()
        }
        .with(
            "/usr/share/applications/zeta.desktop",
            "[Desktop Entry]\nName=System Zeta\nStartupWMClass=widget\n",
        )
        .with(
            "/usr/local/share/applications/widget.desktop",
            "[Desktop Entry]\nName=Local Widget\n",
        );
        let entry = find_desktop_entry(&fs, "WIDGET").unwrap();
        assert_eq!(entry.id, "zeta");
        assert_eq!(entry.name.as_deref(), Some("System Zeta"));

        let fs = fs.with(
            "/home/u/.local/share/applications/Widget.desktop",
            "[Desktop Entry]\nName=Home Widget\n",
        );
        assert_eq!(
            find_desktop_entry(&fs, "widget").unwrap().name.as_deref(),
            Some("Home Widget")
        );
        assert!(find_desktop_entry(&fs, "nothing").is_none());
    }

    #[test]
    fn display_name_falls_back_in_order() {
        assert_eq!(display_name(Some("Desk"), Some("Sem"), Some("cls")), "Desk");
        assert_eq!(display_name(None, Some("Sem"), Some("cls")), "Sem");
        assert_eq!(display_name(None, None, Some("cls")), "cls");
        assert_eq!(display_name(None, None, None), "X11 application");
        assert_eq!(display_name(Some("  "), None, None), "X11 application");
    }

    #[test]
    fn icon_lookup_order_and_absolute_paths() {
        let png = encode_png_rgba(1, 1, &[1, 2, 3, 4]).unwrap();
        let fs = MemoryFs {
            home: Some(PathBuf::from("/home/u")),
            ..Default::default()
        }
        .with(
            "/usr/share/icons/hicolor/128x128/apps/foo.png",
            b"not a png".to_vec(),
        )
        .with("/usr/share/icons/hicolor/64x64/apps/foo.png", png.clone())
        .with("/opt/icon.png", png.clone());
        assert_eq!(load_desktop_icon(&fs, "foo"), Some(png.clone()));
        assert_eq!(load_desktop_icon(&fs, "/opt/icon.png"), Some(png.clone()));
        assert_eq!(load_desktop_icon(&fs, "/opt/missing.png"), None);
        assert_eq!(load_desktop_icon(&fs, ""), None);

        let fs = fs.with("/usr/share/pixmaps/foo.png", b"junk".to_vec());
        // pixmaps comes before the hicolor locations but holds no valid PNG.
        assert_eq!(load_desktop_icon(&fs, "foo"), Some(png));
    }

    #[test]
    fn portal_metadata_is_generic() {
        let metadata = TargetMetadata::portal(TargetKind::PortalSelection);
        assert_eq!(metadata.display_name, "Selected window");
        assert_eq!(metadata.identifier, PORTAL_SELECTION_IDENTIFIER);
        assert!(metadata.title.is_none() && metadata.icon_png.is_none());
        let active = TargetMetadata::portal(TargetKind::PortalActiveWindow);
        assert_eq!(active.identifier, PORTAL_ACTIVE_WINDOW_IDENTIFIER);
    }
}
