//! THM-004 A4: the custom theme library.

mod support;

use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    io,
    path::{Path, PathBuf},
    rc::Rc,
};
use support::*;
use vespertine_theme::{
    import::{ImportAdapter as _, VsCodeAdapter},
    library::{
        EDITABLE_DIR, EntrySource, EntryStatus, InstallMode, LIBRARY_FILE, Library, LibraryEvent,
        LibraryFs, MAX_ENTRIES, MAX_LIBRARY_BYTES, MAX_TOTAL_VARIANTS, StdFs,
    },
    theme::{DEFAULT_DARK_ID, DEFAULT_FAMILY_ID},
};

fn new_library() -> (tempfile::TempDir, Library) {
    let dir = tempfile::tempdir().unwrap();
    let library = Library::load(dir.path());
    (dir, library)
}

fn ids(library: &Library) -> Vec<String> {
    library
        .entries()
        .iter()
        .map(|entry| entry.id.clone())
        .collect()
}

// --- identifiers, re-keying, selection -------------------------------------------------------

#[test]
fn entry_identifiers_are_made_unique_with_numeric_suffixes() {
    let (_dir, mut library) = new_library();
    for expected in ["x", "x-2", "x-3"] {
        let id = library
            .install(&fake_compilation("x", 1), &[], InstallMode::Snapshot)
            .unwrap();
        assert_eq!(id, expected);
    }
    assert_eq!(ids(&library), ["x", "x-2", "x-3"]);
}

#[test]
fn rekeying_replaces_the_family_prefix_or_prefixes_the_new_id() {
    let (_dir, mut library) = new_library();
    library
        .install(&fake_compilation("x", 1), &[], InstallMode::Snapshot)
        .unwrap();
    let mut compilation = fake_compilation("x", 2);
    compilation.family.variants[1].id = "unrelated".into();
    compilation.reports[1].variant_id = "unrelated".into();
    let id = library
        .install(&compilation, &[], InstallMode::Snapshot)
        .unwrap();
    assert_eq!(id, "x-2");
    let entry = library.entry("x-2").unwrap();
    assert_eq!(entry.family.id, "x-2");
    let variant_ids: Vec<&str> = entry
        .family
        .variants
        .iter()
        .map(|v| v.id.as_str())
        .collect();
    assert_eq!(variant_ids, ["x-2-v0", "x-2-unrelated"]);
    assert!(
        entry
            .family
            .variants
            .iter()
            .all(|variant| variant.family_id == "x-2")
    );
    let report_ids: Vec<&str> = entry
        .reports
        .iter()
        .map(|r| r.variant_id.as_str())
        .collect();
    assert_eq!(report_ids, ["x-2-v0", "x-2-unrelated"]);
    assert_eq!(entry.selected_variants, ["x-2-v0", "x-2-unrelated"]);
    // The original entry is untouched.
    assert_eq!(library.entry("x").unwrap().family.variants[0].id, "x-v0");
}

#[test]
fn install_honours_a_variant_subset_and_requires_at_least_one() {
    let (_dir, mut library) = new_library();
    let compilation = fake_compilation("set", 4);
    let id = library
        .install(
            &compilation,
            &["set-v3".to_string(), "set-v1".to_string()],
            InstallMode::Snapshot,
        )
        .unwrap();
    let entry = library.entry(&id).unwrap();
    let kept: Vec<&str> = entry
        .family
        .variants
        .iter()
        .map(|v| v.id.as_str())
        .collect();
    assert_eq!(kept, ["set-v1", "set-v3"], "compilation order is kept");
    assert_eq!(entry.reports.len(), 2);

    let before = library.entries().to_vec();
    let error = library
        .install(&compilation, &["set-v9".to_string()], InstallMode::Snapshot)
        .unwrap_err();
    assert!(error.0.contains("set-v9"));
    assert_eq!(library.entries(), before);

    let mut empty = fake_compilation("none", 1);
    empty.family.variants.clear();
    empty.reports.clear();
    let error = library
        .install(&empty, &[], InstallMode::Snapshot)
        .unwrap_err();
    assert!(error.0.contains("at least one"), "{error}");
    assert_eq!(library.entries(), before);
}

#[test]
fn install_rejects_a_family_with_blocking_validation_issues() {
    let (_dir, mut library) = new_library();
    let mut compilation = fake_compilation("bad", 1);
    compilation.family.variants[0].source.license.clear();
    let error = library
        .install(&compilation, &[], InstallMode::Snapshot)
        .unwrap_err();
    assert!(error.0.contains("validation failed"), "{error}");
    assert!(library.entries().is_empty());

    // A variant that collides with a built-in identifier is blocked by the registry check.
    let mut compilation = fake_compilation("clash", 1);
    compilation.family.variants[0].id = DEFAULT_DARK_ID.to_string();
    compilation.reports[0].variant_id = DEFAULT_DARK_ID.to_string();
    // Re-keying does not apply (id differs from the family id), the id is kept as authored
    // because it does not need a suffix.
    assert!(
        library
            .install(&compilation, &[], InstallMode::Snapshot)
            .is_err()
    );
}

// --- limits ----------------------------------------------------------------------------------

#[test]
fn entry_limit_is_256() {
    let (_dir, mut library) = new_library();
    for index in 0..MAX_ENTRIES {
        library
            .install(
                &fake_compilation(&format!("e{index}"), 1),
                &[],
                InstallMode::Snapshot,
            )
            .unwrap();
    }
    assert_eq!(library.entries().len(), MAX_ENTRIES);
    let error = library
        .install(&fake_compilation("one-more", 1), &[], InstallMode::Snapshot)
        .unwrap_err();
    assert!(error.0.contains("256"), "{error}");
    assert_eq!(library.entries().len(), MAX_ENTRIES);
    // The persisted library is unchanged, too.
    assert_eq!(
        Library::load(library.data_dir()).entries().len(),
        MAX_ENTRIES
    );
}

#[test]
fn variant_limit_is_1024_in_total() {
    let (_dir, mut library) = new_library();
    for index in 0..8 {
        library
            .install(
                &fake_compilation(&format!("v{index}"), 128),
                &[],
                InstallMode::Snapshot,
            )
            .unwrap();
    }
    let total: usize = library
        .entries()
        .iter()
        .map(|entry| entry.family.variants.len())
        .sum();
    assert_eq!(total, MAX_TOTAL_VARIANTS);
    let error = library
        .install(&fake_compilation("extra", 1), &[], InstallMode::Snapshot)
        .unwrap_err();
    assert!(error.0.contains("1024"), "{error}");
    // Duplicating an entry would also exceed the limit.
    assert!(library.duplicate_as_snapshot("v0").is_err());
    assert_eq!(library.entries().len(), 8);
}

#[test]
fn file_limit_is_sixteen_mebibytes() {
    // Measure the serialized size of a library with a one-character name, then pad the family
    // name so the durable file is exactly the limit.
    let measure = tempfile::tempdir().unwrap();
    let mut probe = Library::load(measure.path());
    let mut compilation = fake_compilation("big", 1);
    compilation.family.name = "a".into();
    probe
        .install(&compilation, &[], InstallMode::Snapshot)
        .unwrap();
    let base = std::fs::metadata(measure.path().join(LIBRARY_FILE))
        .unwrap()
        .len() as usize;

    let (dir, mut library) = new_library();
    compilation.family.name = "a".repeat(1 + MAX_LIBRARY_BYTES - base);
    library
        .install(&compilation, &[], InstallMode::Snapshot)
        .unwrap();
    let size = std::fs::metadata(dir.path().join(LIBRARY_FILE))
        .unwrap()
        .len() as usize;
    assert_eq!(size, MAX_LIBRARY_BYTES, "the exact boundary is accepted");
    assert_eq!(Library::load(dir.path()).entries().len(), 1);

    let (dir, mut library) = new_library();
    compilation.family.name = "a".repeat(2 + MAX_LIBRARY_BYTES - base);
    let error = library
        .install(&compilation, &[], InstallMode::Snapshot)
        .unwrap_err();
    assert!(error.0.contains("limit"), "{error}");
    assert!(library.entries().is_empty());
    assert!(!dir.path().join(LIBRARY_FILE).exists());
}

#[test]
fn an_oversized_or_corrupt_library_loads_empty_with_a_persistent_warning() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(LIBRARY_FILE), "{ not json").unwrap();
    let library = Library::load(dir.path());
    assert!(library.entries().is_empty());
    assert!(
        library
            .load_warning()
            .unwrap()
            .contains("could not be loaded")
    );

    let dir = tempfile::tempdir().unwrap();
    let file = std::fs::File::create(dir.path().join(LIBRARY_FILE)).unwrap();
    file.set_len(MAX_LIBRARY_BYTES as u64 + 1).unwrap();
    let library = Library::load(dir.path());
    assert!(library.entries().is_empty());
    assert!(library.load_warning().unwrap().contains("limit"));

    // A missing file is simply an empty library without a warning.
    let dir = tempfile::tempdir().unwrap();
    let library = Library::load(dir.path());
    assert!(library.entries().is_empty());
    assert!(library.load_warning().is_none());
}

// --- durability ------------------------------------------------------------------------------

#[test]
fn a_failed_save_leaves_memory_and_warning_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(LIBRARY_FILE), "{ not json").unwrap();
    // Make the data directory unusable: its "directory" component is a regular file.
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "x").unwrap();
    let mut library = Library::load(dir.path());
    let warning = library.load_warning().map(str::to_string);
    assert!(warning.is_some());
    let mut broken = Library::load(&blocker.join("data"));
    assert!(broken.entries().is_empty());
    let error = broken
        .install(&fake_compilation("x", 1), &[], InstallMode::Snapshot)
        .unwrap_err();
    assert!(error.0.contains("failed"), "{error}");
    assert!(broken.entries().is_empty());

    // Same on a library that has a warning: the warning survives a failed save.
    library = Library::load_with_fs(
        dir.path(),
        Box::new(FailingFs {
            fail_rename_into: Some(dir.path().join(LIBRARY_FILE)),
            fail_restore: false,
            inner: StdFs,
        }),
    );
    let error = library
        .install(&fake_compilation("x", 1), &[], InstallMode::Snapshot)
        .unwrap_err();
    assert!(error.0.contains("failed"), "{error}");
    assert!(library.entries().is_empty());
    assert_eq!(library.load_warning().map(str::to_string), warning);
    // A successful save clears it.
    let mut healthy = Library::load(dir.path());
    healthy
        .install(&fake_compilation("x", 1), &[], InstallMode::Snapshot)
        .unwrap();
    assert!(healthy.load_warning().is_none());
}

/// Delegates to the real file system but fails renames into one path (and optionally the
/// restore rename too).
struct FailingFs {
    fail_rename_into: Option<PathBuf>,
    fail_restore: bool,
    inner: StdFs,
}

impl LibraryFs for FailingFs {
    fn write_synced(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.inner.write_synced(path, bytes)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        let is_restore = from.extension().is_some_and(|extension| extension == "bak");
        if self.fail_rename_into.as_deref() == Some(to) && (!is_restore || self.fail_restore) {
            return Err(io::Error::other("injected rename failure"));
        }
        self.inner.rename(from, to)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.inner.remove_file(path)
    }

    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.inner.read(path)
    }

    fn len(&self, path: &Path) -> io::Result<u64> {
        self.inner.len(path)
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.create_dir_all(path)
    }
}

#[test]
fn a_failed_final_rename_restores_the_previous_library() {
    let dir = tempfile::tempdir().unwrap();
    let primary = dir.path().join(LIBRARY_FILE);
    let mut library = Library::load(dir.path());
    library
        .install(&fake_compilation("first", 1), &[], InstallMode::Snapshot)
        .unwrap();
    let good_bytes = std::fs::read(&primary).unwrap();

    let mut library = Library::load_with_fs(
        dir.path(),
        Box::new(FailingFs {
            fail_rename_into: Some(primary.clone()),
            fail_restore: false,
            inner: StdFs,
        }),
    );
    let error = library
        .install(&fake_compilation("second", 1), &[], InstallMode::Snapshot)
        .unwrap_err();
    assert!(error.0.contains("injected rename failure"), "{error}");
    assert_eq!(ids(&library), ["first"], "memory is unchanged");
    assert_eq!(
        std::fs::read(&primary).unwrap(),
        good_bytes,
        "the old file is restored"
    );
    assert!(!dir.path().join(format!("{LIBRARY_FILE}.bak")).exists());
}

#[test]
fn a_failed_restore_names_the_recovery_copy() {
    let dir = tempfile::tempdir().unwrap();
    let primary = dir.path().join(LIBRARY_FILE);
    let mut library = Library::load(dir.path());
    library
        .install(&fake_compilation("first", 1), &[], InstallMode::Snapshot)
        .unwrap();
    let mut library = Library::load_with_fs(
        dir.path(),
        Box::new(FailingFs {
            fail_rename_into: Some(primary.clone()),
            fail_restore: true,
            inner: StdFs,
        }),
    );
    let error = library
        .install(&fake_compilation("second", 1), &[], InstallMode::Snapshot)
        .unwrap_err();
    let backup = dir.path().join(format!("{LIBRARY_FILE}.bak"));
    assert!(error.0.contains("recovery copy"), "{error}");
    assert!(error.0.contains(&backup.display().to_string()), "{error}");
    assert!(backup.exists());
    assert_eq!(ids(&library), ["first"]);
    // The next load recovers from the backup because the primary is missing.
    assert!(!primary.exists());
    assert_eq!(ids(&Library::load(dir.path())), ["first"]);
}

#[test]
fn loading_recovers_from_the_backup_when_the_primary_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let mut library = Library::load(dir.path());
    library
        .install(&fake_compilation("kept", 1), &[], InstallMode::Snapshot)
        .unwrap();
    // Simulate a crash between the two renames of a save.
    let primary = dir.path().join(LIBRARY_FILE);
    let backup = dir.path().join(format!("{LIBRARY_FILE}.bak"));
    std::fs::rename(&primary, &backup).unwrap();
    let recovered = Library::load(dir.path());
    assert_eq!(ids(&recovered), ["kept"]);
    assert!(recovered.load_warning().is_none());
}

#[test]
fn a_stale_backup_is_removed_by_the_next_save() {
    let dir = tempfile::tempdir().unwrap();
    let mut library = Library::load(dir.path());
    library
        .install(&fake_compilation("first", 1), &[], InstallMode::Snapshot)
        .unwrap();
    let backup = dir.path().join(format!("{LIBRARY_FILE}.bak"));
    assert!(!backup.exists(), "the backup is removed after a save");
    std::fs::write(&backup, "stale").unwrap();
    // The primary exists, so it wins over the stale backup.
    assert_eq!(ids(&Library::load(dir.path())), ["first"]);
    library
        .install(&fake_compilation("second", 1), &[], InstallMode::Snapshot)
        .unwrap();
    assert!(!backup.exists());
    assert!(!dir.path().join(format!("{LIBRARY_FILE}.tmp")).exists());
    assert_eq!(ids(&Library::load(dir.path())), ["first", "second"]);
    // The file is pretty-printed JSON.
    let text = std::fs::read_to_string(dir.path().join(LIBRARY_FILE)).unwrap();
    assert!(text.contains("\n  "));
}

// --- reload, unlink, duplicates, remove, reveal ----------------------------------------------

fn linked_file_library(
    colors: Value,
    mode: InstallMode,
) -> (
    tempfile::TempDir,
    tempfile::TempDir,
    PathBuf,
    Library,
    String,
) {
    let source = tempfile::tempdir().unwrap();
    let path = source.path().join("theme.json");
    write_json(&path, &theme_json(colors));
    let compilation = compile(&path, "lnk").unwrap();
    let (data, mut library) = new_library();
    let id = library.install(&compilation, &[], mode).unwrap();
    (source, data, path, library, id)
}

#[test]
fn reload_replaces_the_family_and_keeps_the_last_good_one_on_failure() {
    let (_source, _data, path, mut library, id) =
        linked_file_library(good_dark_colors(), InstallMode::Link);
    let entry = library.entry(&id).unwrap();
    assert!(matches!(entry.source, EntrySource::LinkedFile { .. }));
    assert_eq!(entry.family.variants[0].colors.background, hex("#1e1e1e"));

    let events = Rc::new(RefCell::new(Vec::new()));
    let observer = library.observe({
        let events = events.clone();
        move |event| events.borrow_mut().push(event.clone())
    });

    // An edit is picked up with the same identifier.
    let mut edited = good_dark_colors();
    edited["editor.background"] = json!("#202830");
    write_json(&path, &theme_json(edited));
    library.reload(&id).unwrap();
    let entry = library.entry(&id).unwrap();
    assert_eq!(entry.family.variants[0].id, "lnk-theme");
    assert_eq!(entry.family.variants[0].colors.background, hex("#202830"));
    assert_eq!(entry.status, EntryStatus::Ready);
    assert_eq!(
        Library::load(library.data_dir()).entry(&id).unwrap().family,
        entry.family
    );

    // Breaking the source keeps the last good family, warns and returns the error.
    std::fs::write(&path, "{ broken").unwrap();
    let error = library.reload(&id).unwrap_err();
    let entry = library.entry(&id).unwrap();
    assert_eq!(entry.family.variants[0].colors.background, hex("#202830"));
    match &entry.status {
        EntryStatus::Warning { message } => assert_eq!(message, &error.0),
        other => panic!("expected a warning, got {other:?}"),
    }
    assert!(error.0.contains("not valid JSON"), "{error}");

    // Fixing it clears the warning.
    write_json(&path, &theme_json(good_dark_colors()));
    library.reload(&id).unwrap();
    assert_eq!(library.entry(&id).unwrap().status, EntryStatus::Ready);

    library.unobserve(observer);
    assert_eq!(
        events.borrow().as_slice(),
        [
            LibraryEvent::Reloaded { id: id.clone() },
            LibraryEvent::ReloadFailed {
                id: id.clone(),
                message: error.0
            },
            LibraryEvent::Reloaded { id: id.clone() },
        ]
    );
}

#[test]
fn reload_of_a_package_ignores_unselected_siblings_and_requires_a_selected_one() {
    let source = tempfile::tempdir().unwrap();
    let root = source.path();
    write_json(&root.join("a.json"), &theme_json(good_dark_colors()));
    write_json(&root.join("b.json"), &theme_json(good_dark_colors()));
    let manifest = |entries: &[(&str, &str)]| {
        package_manifest(
            entries
                .iter()
                .map(|(label, path)| json!({ "label": label, "uiTheme": "vs-dark", "path": path }))
                .collect(),
        )
    };
    write_json(
        &root.join("package.json"),
        &manifest(&[("A", "./a.json"), ("B", "./b.json")]),
    );
    let compilation = compile(root, "pk").unwrap();
    let (_data, mut library) = new_library();
    let id = library
        .install(&compilation, &["pk-a".to_string()], InstallMode::Link)
        .unwrap();
    assert!(matches!(
        library.entry(&id).unwrap().source,
        EntrySource::LinkedPackage { .. }
    ));
    assert_eq!(library.entry(&id).unwrap().family.variants.len(), 1);

    // The unselected sibling breaks: the reload still succeeds.
    std::fs::write(root.join("b.json"), "{ nope").unwrap();
    library.reload(&id).unwrap();
    assert_eq!(library.entry(&id).unwrap().family.variants.len(), 1);
    assert_eq!(library.entry(&id).unwrap().status, EntryStatus::Ready);

    // The selected variant breaks: the reload fails and the last good family stays.
    std::fs::write(root.join("a.json"), "{ nope").unwrap();
    let error = library.reload(&id).unwrap_err();
    assert!(error.0.contains("pk-a"), "{error}");
    assert!(matches!(
        library.entry(&id).unwrap().status,
        EntryStatus::Warning { .. }
    ));
    assert_eq!(
        library.entry(&id).unwrap().family.variants[0]
            .colors
            .background,
        hex("#1e1e1e")
    );

    // The selected variant disappears from the manifest.
    write_json(&root.join("a.json"), &theme_json(good_dark_colors()));
    write_json(&root.join("package.json"), &manifest(&[("B", "./b.json")]));
    std::fs::write(
        root.join("b.json"),
        serde_json::to_vec(&theme_json(good_dark_colors())).unwrap(),
    )
    .unwrap();
    let error = library.reload(&id).unwrap_err();
    assert!(
        error.0.contains("no longer contains any selected variant"),
        "{error}"
    );
}

#[test]
fn a_snapshot_cannot_reload_and_unknown_ids_are_errors() {
    let (_source, _data, _path, mut library, id) =
        linked_file_library(good_dark_colors(), InstallMode::Snapshot);
    assert!(library.reload(&id).unwrap_err().0.contains("snapshot"));
    assert!(library.reload("missing").is_err());
    assert!(library.unlink(&id).is_err(), "only linked entries unlink");
    assert!(library.remove("missing").is_err());
    assert!(library.duplicate_as_snapshot("missing").is_err());
    assert!(library.reveal("missing", |_| {}).is_err());
}

#[test]
fn unlink_turns_a_linked_entry_into_a_snapshot_remembering_the_path() {
    let (_source, _data, path, mut library, id) =
        linked_file_library(good_dark_colors(), InstallMode::Link);
    library.unlink(&id).unwrap();
    let entry = library.entry(&id).unwrap();
    assert_eq!(
        entry.source,
        EntrySource::ImportedSnapshot {
            imported_from: Some(path.canonicalize().unwrap())
        }
    );
    assert_eq!(entry.status, EntryStatus::Ready);
    assert!(library.reload(&id).is_err());
    assert!(matches!(
        Library::load(library.data_dir()).entry(&id).unwrap().source,
        EntrySource::ImportedSnapshot { .. }
    ));
}

#[test]
fn duplicate_as_snapshot_rekeys_and_names_the_copy() {
    let (_source, _data, path, mut library, id) =
        linked_file_library(good_dark_colors(), InstallMode::Link);
    let copy = library.duplicate_as_snapshot(&id).unwrap();
    assert_eq!(copy, "lnk-copy");
    let original = library.entry(&id).unwrap().clone();
    let entry = library.entry(&copy).unwrap();
    assert_eq!(entry.family.name, "lnk Copy");
    assert_eq!(entry.family.variants[0].id, "lnk-copy-theme");
    assert_eq!(entry.family.variants[0].family_id, "lnk-copy");
    assert_eq!(entry.reports[0].variant_id, "lnk-copy-theme");
    assert_eq!(entry.selected_variants, ["lnk-copy-theme"]);
    assert_eq!(entry.status, EntryStatus::Ready);
    assert_eq!(
        entry.source,
        EntrySource::ImportedSnapshot {
            imported_from: Some(path.canonicalize().unwrap())
        }
    );
    assert_eq!(
        entry.family.variants[0].colors,
        original.family.variants[0].colors
    );
    // A second copy gets a numeric suffix.
    assert_eq!(library.duplicate_as_snapshot(&id).unwrap(), "lnk-copy-2");
    assert_eq!(library.entry(&id).unwrap(), &original);
}

#[test]
fn duplicate_as_editable_writes_a_native_file_that_reloads_with_validation() {
    let (_source, data, _path, mut library, id) =
        linked_file_library(good_dark_colors(), InstallMode::Snapshot);
    let copy = library.duplicate_as_editable(&id, data.path()).unwrap();
    let file = data
        .path()
        .join(EDITABLE_DIR)
        .join(format!("{copy}.theme.json"));
    assert!(file.exists());
    assert_eq!(
        library.entry(&copy).unwrap().source,
        EntrySource::EditableFile { path: file.clone() }
    );
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("\n  "), "pretty printed");
    let native: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(native["id"], json!(copy));

    // An edit is picked up; every variant is selected afterwards.
    let mut edited = native;
    edited["variants"][0]["colors"]["background"] = json!("#112233");
    std::fs::write(&file, serde_json::to_vec_pretty(&edited).unwrap()).unwrap();
    library.reload(&copy).unwrap();
    assert_eq!(
        library.entry(&copy).unwrap().family.variants[0]
            .colors
            .background,
        hex("#112233")
    );

    // Changing the family identifier is rejected and the last good family stays.
    let mut renamed = edited.clone();
    renamed["id"] = json!("something-else");
    std::fs::write(&file, serde_json::to_vec_pretty(&renamed).unwrap()).unwrap();
    let error = library.reload(&copy).unwrap_err();
    assert!(error.0.contains("family identifier changed"), "{error}");
    assert_eq!(
        library.entry(&copy).unwrap().family.variants[0]
            .colors
            .background,
        hex("#112233")
    );
    assert!(matches!(
        library.entry(&copy).unwrap().status,
        EntryStatus::Warning { .. }
    ));

    // Invalid JSON and blocking validation errors preserve the last good family too.
    std::fs::write(&file, "{ nope").unwrap();
    assert!(library.reload(&copy).is_err());
    let mut unsourced = edited.clone();
    unsourced["variants"][0]["source"]["license"] = json!("");
    std::fs::write(&file, serde_json::to_vec_pretty(&unsourced).unwrap()).unwrap();
    let error = library.reload(&copy).unwrap_err();
    assert!(error.0.contains("validation failed"), "{error}");
    assert_eq!(
        library.entry(&copy).unwrap().family.variants[0]
            .colors
            .background,
        hex("#112233")
    );

    // Valid again.
    std::fs::write(&file, serde_json::to_vec_pretty(&edited).unwrap()).unwrap();
    library.reload(&copy).unwrap();
    assert_eq!(library.entry(&copy).unwrap().status, EntryStatus::Ready);
}

#[test]
fn remove_deletes_the_entry_and_reports_its_variants() {
    let (_source, _data, _path, mut library, id) =
        linked_file_library(good_dark_colors(), InstallMode::Link);
    let removed = library.remove(&id).unwrap();
    assert_eq!(removed, ["lnk-theme"]);
    assert!(library.entries().is_empty());
    assert!(Library::load(library.data_dir()).entries().is_empty());
    assert!(library.remove(&id).is_err());
}

#[test]
fn reveal_passes_the_source_location_to_the_file_manager_hook() {
    let (_source, _data, path, mut library, id) =
        linked_file_library(good_dark_colors(), InstallMode::Link);
    let revealed = Cell::new(None);
    library
        .reveal(&id, |location| revealed.set(Some(location.to_path_buf())))
        .unwrap();
    assert_eq!(revealed.take(), Some(path.canonicalize().unwrap()));

    // An entry without a location is an error: craft a library file with no `imported from`.
    let copy = library.duplicate_as_snapshot(&id).unwrap();
    let file = library.data_dir().join(LIBRARY_FILE);
    let mut stored: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    for entry in stored["entries"].as_array_mut().unwrap() {
        if entry["id"] == json!(copy) {
            entry["source"]["imported_from"] = Value::Null;
        }
    }
    std::fs::write(&file, serde_json::to_vec(&stored).unwrap()).unwrap();
    let library = Library::load(library.data_dir());
    let error = library
        .reveal(&copy, |_| panic!("no location"))
        .unwrap_err();
    assert!(error.0.contains("no source location"), "{error}");
}

// --- registry events and the runtime registry ------------------------------------------------

#[test]
fn every_change_notifies_observers_and_the_runtime_registry_follows_the_library() {
    let (_data, mut library) = new_library();
    let events = Rc::new(RefCell::new(Vec::new()));
    library.observe({
        let events = events.clone();
        move |event| events.borrow_mut().push(event.clone())
    });
    let id = library
        .install(&fake_compilation("ev", 2), &[], InstallMode::Snapshot)
        .unwrap();
    let copy = library.duplicate_as_snapshot(&id).unwrap();
    let removed = library.remove(&id).unwrap();
    assert_eq!(
        events.borrow().as_slice(),
        [
            LibraryEvent::Installed { id: id.clone() },
            LibraryEvent::Duplicated {
                from: id.clone(),
                id: copy.clone()
            },
            LibraryEvent::Removed {
                id: id.clone(),
                variant_ids: removed
            },
        ]
    );
    // Built-in families first, then the library entries in order.
    let registry = library.runtime_registry();
    let family_ids: Vec<&str> = registry.families().iter().map(|f| f.id.as_str()).collect();
    assert_eq!(family_ids, [DEFAULT_FAMILY_ID, copy.as_str()]);
    // A failed operation notifies nobody.
    let before = events.borrow().len();
    assert!(library.remove("missing").is_err());
    assert_eq!(events.borrow().len(), before);
}

#[test]
fn the_adapter_trait_is_extensible() {
    struct Fixed;
    impl vespertine_theme::import::ImportAdapter for Fixed {
        fn format(&self) -> &'static str {
            "fixed"
        }
        fn compile(
            &self,
            _path: &Path,
            _options: &vespertine_theme::import::ImportOptions,
        ) -> Result<vespertine_theme::import::Compilation, vespertine_theme::import::ImportError>
        {
            Ok(fake_compilation("fixed", 1))
        }
    }
    let compilation = Fixed.compile(Path::new("."), &Default::default()).unwrap();
    assert_eq!(compilation.family.id, "fixed");
    assert_eq!(VsCodeAdapter.format(), "vscode");
    let (_dir, mut library) = new_library();
    library.add_adapter(Box::new(Fixed));
    let mut linked = fake_compilation("fixed", 1);
    linked.format = "fixed".into();
    let id = library.install(&linked, &[], InstallMode::Link).unwrap();
    library.reload(&id).unwrap();
}
