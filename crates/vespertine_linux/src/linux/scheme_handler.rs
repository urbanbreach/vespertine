//! Registering the application as the user's handler for a custom URL scheme.
//!
//! The application's installed desktop entry (its desktop file id is the application id plus
//! `.desktop`) must already declare the scheme. Registration only reads desktop entries and
//! asks the freedesktop MIME-apps mechanism (`xdg-mime default`) to record the default, which
//! writes the user's own `mimeapps.list` and nothing else.

use anyhow::{Context as _, Result, bail};
use std::path::{Path, PathBuf};

/// The system services registration needs.
pub(super) trait SchemeEnvironment {
    /// The contents of the installed desktop entry with this desktop file id, if one exists.
    fn desktop_entry(&self, desktop_id: &str) -> Option<String>;
    /// The desktop file id currently the user's default handler for `mime_type`.
    fn default_handler(&self, mime_type: &str) -> Result<Option<String>>;
    /// Makes `desktop_id` the user's default handler for `mime_type`.
    fn set_default_handler(&self, desktop_id: &str, mime_type: &str) -> Result<()>;
}

/// Registers `scheme` (already validated and lowercased) for the application `app_id`.
pub(super) fn register_scheme(
    environment: &dyn SchemeEnvironment,
    app_id: Option<&str>,
    scheme: &str,
) -> Result<()> {
    let Some(app_id) = app_id.filter(|id| !id.is_empty()) else {
        bail!(
            "can't register the {scheme:?} URL scheme: the application id is not set (call set_app_identity first)"
        );
    };
    let desktop_id = format!("{app_id}.desktop");
    let mime_type = format!("x-scheme-handler/{scheme}");

    let Some(entry) = environment.desktop_entry(&desktop_id) else {
        bail!(
            "can't register the {scheme:?} URL scheme: no installed desktop entry {desktop_id:?} was found in the XDG applications directories"
        );
    };
    let entry = DesktopEntry::parse(&entry);
    if !entry.mime_types.contains(&mime_type) {
        bail!(
            "can't register the {scheme:?} URL scheme: the desktop entry {desktop_id:?} does not list {mime_type} in MimeType"
        );
    }
    if !entry.exec.as_deref().is_some_and(exec_takes_url) {
        bail!(
            "can't register the {scheme:?} URL scheme: the Exec line of the desktop entry {desktop_id:?} must take %u or %U"
        );
    }

    if environment
        .default_handler(&mime_type)
        .context("querying the default handler")?
        .as_deref()
        == Some(desktop_id.as_str())
    {
        return Ok(());
    }
    environment
        .set_default_handler(&desktop_id, &mime_type)
        .with_context(|| format!("setting {desktop_id} as the default handler for {mime_type}"))
}

/// The two keys of the `[Desktop Entry]` group registration cares about.
#[derive(Debug, Default, PartialEq, Eq)]
struct DesktopEntry {
    mime_types: Vec<String>,
    exec: Option<String>,
}

impl DesktopEntry {
    fn parse(contents: &str) -> Self {
        let mut entry = Self::default();
        let mut in_main_group = false;
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(group) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                in_main_group = group == "Desktop Entry";
                continue;
            }
            if !in_main_group {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key.trim() {
                "MimeType" => entry.mime_types.extend(
                    value
                        .split(';')
                        .map(str::trim)
                        .filter(|mime| !mime.is_empty())
                        .map(str::to_owned),
                ),
                "Exec" => entry.exec = Some(value.trim().to_owned()),
                _ => {}
            }
        }
        entry
    }
}

/// Whether an `Exec` value contains a `%u` or `%U` field code (`%%` is a literal percent).
fn exec_takes_url(exec: &str) -> bool {
    let mut characters = exec.chars();
    while let Some(character) = characters.next() {
        if character == '%' {
            match characters.next() {
                Some('u' | 'U') => return true,
                Some(_) => {}
                None => return false,
            }
        }
    }
    false
}

/// The desktop entry lookup directories: `$XDG_DATA_HOME/applications` first, then each of
/// `$XDG_DATA_DIRS`.
pub(super) fn applications_directories(
    data_home: Option<PathBuf>,
    data_dirs: Option<std::ffi::OsString>,
    home: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    match data_home.filter(|path| path.is_absolute()) {
        Some(path) => directories.push(path.join("applications")),
        None => {
            if let Some(home) = home {
                directories.push(home.join(".local/share/applications"));
            }
        }
    }
    let data_dirs = data_dirs.filter(|dirs| !dirs.is_empty());
    match data_dirs {
        Some(dirs) => directories.extend(
            std::env::split_paths(&dirs)
                .filter(|path| path.is_absolute())
                .map(|path| path.join("applications")),
        ),
        None => {
            directories.push(PathBuf::from("/usr/local/share/applications"));
            directories.push(PathBuf::from("/usr/share/applications"));
        }
    }
    directories
}

/// Finds the desktop entry for a desktop file id. An id maps to a path by replacing `/` with
/// `-`, so `a-b.desktop` may live at `a-b.desktop` or `a/b.desktop`.
pub(super) fn find_desktop_entry(directories: &[PathBuf], desktop_id: &str) -> Option<String> {
    directories.iter().find_map(|directory| {
        candidate_paths(directory, desktop_id)
            .into_iter()
            .find_map(|path| std::fs::read_to_string(path).ok())
    })
}

fn candidate_paths(directory: &Path, desktop_id: &str) -> Vec<PathBuf> {
    let mut candidates = vec![directory.join(desktop_id)];
    let dashes: Vec<usize> = desktop_id.match_indices('-').map(|(ix, _)| ix).collect();
    // Every subset of the dashes may stand for a directory separator.
    for mask in 1u32..(1u32 << dashes.len().min(10)) {
        let mut relative = String::with_capacity(desktop_id.len());
        let mut last = 0;
        for (bit, ix) in dashes.iter().take(10).enumerate() {
            relative.push_str(&desktop_id[last..*ix]);
            relative.push(if mask & (1 << bit) != 0 { '/' } else { '-' });
            last = ix + 1;
        }
        relative.push_str(&desktop_id[last..]);
        candidates.push(directory.join(relative));
    }
    candidates
}

/// The real system: XDG directories from the process environment and the `xdg-mime` tool.
pub(super) struct SystemSchemeEnvironment {
    pub(super) applications: Vec<PathBuf>,
}

impl SystemSchemeEnvironment {
    pub(super) fn from_process_environment() -> Self {
        Self {
            applications: applications_directories(
                std::env::var_os("XDG_DATA_HOME").map(PathBuf::from),
                std::env::var_os("XDG_DATA_DIRS"),
                std::env::var_os("HOME").map(PathBuf::from),
            ),
        }
    }
}

impl SchemeEnvironment for SystemSchemeEnvironment {
    fn desktop_entry(&self, desktop_id: &str) -> Option<String> {
        find_desktop_entry(&self.applications, desktop_id)
    }

    fn default_handler(&self, mime_type: &str) -> Result<Option<String>> {
        let mut command = vespertine_util::new_std_command("xdg-mime");
        command.args(["query", "default", mime_type]);
        #[allow(
            clippy::disallowed_methods,
            reason = "running on a background thread, so blocking is fine"
        )]
        let output = command.output().context("running xdg-mime query default")?;
        if !output.status.success() {
            bail!(
                "xdg-mime query default failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let handler = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        Ok((!handler.is_empty()).then_some(handler))
    }

    fn set_default_handler(&self, desktop_id: &str, mime_type: &str) -> Result<()> {
        let mut command = vespertine_util::new_std_command("xdg-mime");
        command.args(["default", desktop_id, mime_type]);
        #[allow(
            clippy::disallowed_methods,
            reason = "running on a background thread, so blocking is fine"
        )]
        let output = command.output().context("running xdg-mime default")?;
        if !output.status.success() {
            bail!(
                "xdg-mime default failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    #[derive(Default)]
    struct FakeEnvironment {
        entries: HashMap<String, String>,
        default: RefCell<HashMap<String, String>>,
        set_calls: RefCell<Vec<(String, String)>>,
    }

    impl SchemeEnvironment for FakeEnvironment {
        fn desktop_entry(&self, desktop_id: &str) -> Option<String> {
            self.entries.get(desktop_id).cloned()
        }

        fn default_handler(&self, mime_type: &str) -> Result<Option<String>> {
            Ok(self.default.borrow().get(mime_type).cloned())
        }

        fn set_default_handler(&self, desktop_id: &str, mime_type: &str) -> Result<()> {
            self.set_calls
                .borrow_mut()
                .push((desktop_id.to_owned(), mime_type.to_owned()));
            self.default
                .borrow_mut()
                .insert(mime_type.to_owned(), desktop_id.to_owned());
            Ok(())
        }
    }

    const ENTRY: &str = "[Desktop Entry]\nName=Demo\nExec=demo %U\nMimeType=text/plain;x-scheme-handler/demo;\n\n[Desktop Action new]\nExec=demo --new\n";

    fn environment_with(entry: &str) -> FakeEnvironment {
        FakeEnvironment {
            entries: HashMap::from([("org.example.Demo.desktop".to_owned(), entry.to_owned())]),
            ..Default::default()
        }
    }

    #[test]
    fn registration_sets_the_default_handler_once() {
        let environment = environment_with(ENTRY);
        register_scheme(&environment, Some("org.example.Demo"), "demo").unwrap();
        register_scheme(&environment, Some("org.example.Demo"), "demo").unwrap();
        assert_eq!(
            *environment.set_calls.borrow(),
            vec![(
                "org.example.Demo.desktop".to_owned(),
                "x-scheme-handler/demo".to_owned()
            )],
            "the second registration must be a no-op"
        );
    }

    #[test]
    fn missing_application_id_is_a_readable_error() {
        let environment = environment_with(ENTRY);
        for app_id in [None, Some("")] {
            let error = register_scheme(&environment, app_id, "demo")
                .unwrap_err()
                .to_string();
            assert!(error.contains("application id is not set"), "{error}");
        }
        assert!(environment.set_calls.borrow().is_empty());
    }

    #[test]
    fn missing_desktop_entry_is_a_readable_error() {
        let environment = FakeEnvironment::default();
        let error = register_scheme(&environment, Some("org.example.Demo"), "demo")
            .unwrap_err()
            .to_string();
        assert!(error.contains("no installed desktop entry"), "{error}");
        assert!(environment.set_calls.borrow().is_empty());
    }

    #[test]
    fn entry_must_list_the_scheme_and_take_a_url() {
        let without_mime =
            environment_with("[Desktop Entry]\nExec=demo %u\nMimeType=text/plain;\n");
        let error = register_scheme(&without_mime, Some("org.example.Demo"), "demo")
            .unwrap_err()
            .to_string();
        assert!(error.contains("MimeType"), "{error}");

        let without_url = environment_with(
            "[Desktop Entry]\nExec=demo --flag %%u\nMimeType=x-scheme-handler/demo;\n",
        );
        let error = register_scheme(&without_url, Some("org.example.Demo"), "demo")
            .unwrap_err()
            .to_string();
        assert!(error.contains("%u or %U"), "{error}");

        // Keys of other groups don't count.
        let action_only = environment_with(
            "[Desktop Entry]\nExec=demo\nMimeType=x-scheme-handler/demo;\n[Desktop Action a]\nExec=demo %u\n",
        );
        assert!(register_scheme(&action_only, Some("org.example.Demo"), "demo").is_err());
        assert!(without_mime.set_calls.borrow().is_empty());
    }

    #[test]
    fn exec_field_codes() {
        assert!(exec_takes_url("demo %u"));
        assert!(exec_takes_url("demo --open=%U"));
        assert!(!exec_takes_url("demo %f"));
        assert!(!exec_takes_url("demo 100%%u"));
        assert!(!exec_takes_url("demo %"));
    }

    #[test]
    fn lookup_directories_follow_the_xdg_rules() {
        assert_eq!(
            applications_directories(
                Some(PathBuf::from("/data")),
                Some("/a:/b".into()),
                Some(PathBuf::from("/home/me"))
            ),
            vec![
                PathBuf::from("/data/applications"),
                PathBuf::from("/a/applications"),
                PathBuf::from("/b/applications")
            ]
        );
        assert_eq!(
            applications_directories(None, None, Some(PathBuf::from("/home/me"))),
            vec![
                PathBuf::from("/home/me/.local/share/applications"),
                PathBuf::from("/usr/local/share/applications"),
                PathBuf::from("/usr/share/applications")
            ]
        );
        // Relative entries are ignored.
        assert_eq!(
            applications_directories(Some("rel".into()), Some("rel:/b".into()), None),
            vec![PathBuf::from("/b/applications")]
        );
    }

    #[test]
    fn desktop_entries_are_found_in_priority_order_and_subdirectories() {
        let root = std::env::temp_dir().join(format!("vespertine-scheme-{}", std::process::id()));
        let user = root.join("user");
        let system = root.join("system");
        std::fs::create_dir_all(user.join("org")).unwrap();
        std::fs::create_dir_all(&system).unwrap();
        std::fs::write(user.join("org/demo.desktop"), "user").unwrap();
        std::fs::write(system.join("org-demo.desktop"), "system").unwrap();
        std::fs::write(system.join("other.desktop"), "other").unwrap();
        let directories = [user, system];

        assert_eq!(
            find_desktop_entry(&directories, "org-demo.desktop").as_deref(),
            Some("user")
        );
        assert_eq!(
            find_desktop_entry(&directories, "other.desktop").as_deref(),
            Some("other")
        );
        assert_eq!(find_desktop_entry(&directories, "absent.desktop"), None);
        std::fs::remove_dir_all(root).ok();
    }
}
