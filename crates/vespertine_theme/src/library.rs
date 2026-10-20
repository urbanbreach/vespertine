//! The custom theme library (THM-004 R7, R9-R13): installed entries, library operations
//! computed on a copy and persisted atomically, limits and durable storage with backup
//! recovery.

use crate::{
    appearance,
    import::{
        Compilation, ImportAdapter, ImportOptions, MAX_SOURCE_BYTES, SourceKind, VariantReport,
        VsCodeAdapter,
    },
    theme::{ThemeFamily, ThemeRegistry, ValidationIssue, has_blocking_issue},
};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    fmt,
    io::Write as _,
    path::{Path, PathBuf},
    rc::Rc,
};

/// Largest durable library file.
pub const MAX_LIBRARY_BYTES: usize = 16 * 1024 * 1024;
/// Most entries.
pub const MAX_ENTRIES: usize = 256;
/// Most variants over all entries.
pub const MAX_TOTAL_VARIANTS: usize = 1024;

/// File name of the library in the data directory.
pub const LIBRARY_FILE: &str = "theme-library.json";
/// Directory (under the data directory) holding editable resolved-family files.
pub const EDITABLE_DIR: &str = "custom-themes";

const LIBRARY_VERSION: u32 = 1;

/// A readable library failure.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LibraryError(pub String);

impl fmt::Display for LibraryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LibraryError {}

fn fail<T>(message: impl Into<String>) -> Result<T, LibraryError> {
    Err(LibraryError(message.into()))
}

/// How an installed entry keeps its source.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InstallMode {
    /// An immutable copy remembering the original path.
    Snapshot,
    /// The entry remembers the file or package location and can be reloaded.
    Link,
}

/// Where an entry's family comes from.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum EntrySource {
    /// An immutable copy.
    ImportedSnapshot {
        /// The path it was imported from, if any.
        imported_from: Option<PathBuf>,
    },
    /// A linked single file.
    LinkedFile {
        /// File location.
        path: PathBuf,
        /// Adapter format.
        format: String,
        /// Options the family was compiled with.
        options: ImportOptions,
    },
    /// A linked package directory.
    LinkedPackage {
        /// Package location.
        path: PathBuf,
        /// Adapter format.
        format: String,
        /// Options the family was compiled with.
        options: ImportOptions,
    },
    /// A native resolved-family JSON file the user edits.
    EditableFile {
        /// File location.
        path: PathBuf,
    },
}

/// Entry status.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum EntryStatus {
    /// Up to date.
    Ready,
    /// The last reload failed; the last good family stays active.
    Warning {
        /// The failure message.
        message: String,
    },
}

/// One installed family.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryEntry {
    /// Unique entry identifier (equal to the family identifier).
    pub id: String,
    /// Where the family comes from.
    pub source: EntrySource,
    /// Status.
    pub status: EntryStatus,
    /// The installed family.
    pub family: ThemeFamily,
    /// Reports of the installed variants.
    pub reports: Vec<VariantReport>,
    /// The variants the entry keeps.
    pub selected_variants: Vec<String>,
}

impl LibraryEntry {
    /// The source location: the linked or editable path, or the original path of a snapshot.
    pub fn location(&self) -> Option<&Path> {
        match &self.source {
            EntrySource::ImportedSnapshot { imported_from } => imported_from.as_deref(),
            EntrySource::LinkedFile { path, .. }
            | EntrySource::LinkedPackage { path, .. }
            | EntrySource::EditableFile { path } => Some(path),
        }
    }
}

/// A change to the installed custom families (THM-004 R13).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum LibraryEvent {
    /// An entry was installed.
    Installed {
        /// Entry identifier.
        id: String,
    },
    /// An entry was reloaded successfully.
    Reloaded {
        /// Entry identifier.
        id: String,
    },
    /// A reload failed; the last good family stays active and the status is a warning.
    ReloadFailed {
        /// Entry identifier.
        id: String,
        /// The failure message.
        message: String,
    },
    /// A linked entry became a snapshot.
    Unlinked {
        /// Entry identifier.
        id: String,
    },
    /// An entry was duplicated.
    Duplicated {
        /// The original entry.
        from: String,
        /// The new entry.
        id: String,
    },
    /// An entry was removed.
    Removed {
        /// Entry identifier.
        id: String,
        /// Identifiers of its variants.
        variant_ids: Vec<String>,
    },
}

/// Identifies an observer registration.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ObserverId(u64);

/// File system operations the durable save uses; a seam so failures can be exercised.
pub trait LibraryFs {
    /// Creates `path` with `bytes` and syncs it to storage.
    fn write_synced(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()>;
    /// Renames a file.
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()>;
    /// Removes a file.
    fn remove_file(&self, path: &Path) -> std::io::Result<()>;
    /// Whether a file exists.
    fn exists(&self, path: &Path) -> bool;
    /// Reads a whole file.
    fn read(&self, path: &Path) -> std::io::Result<Vec<u8>>;
    /// Size of a file.
    fn len(&self, path: &Path) -> std::io::Result<u64>;
    /// Creates a directory and its parents.
    fn create_dir_all(&self, path: &Path) -> std::io::Result<()>;
}

/// The real file system.
#[derive(Clone, Copy, Debug, Default)]
pub struct StdFs;

impl LibraryFs for StdFs {
    fn write_synced(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        let mut file = std::fs::File::create(path)?;
        file.write_all(bytes)?;
        file.sync_all()
    }

    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        std::fs::rename(from, to)
    }

    fn remove_file(&self, path: &Path) -> std::io::Result<()> {
        std::fs::remove_file(path)
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn len(&self, path: &Path) -> std::io::Result<u64> {
        std::fs::metadata(path).map(|metadata| metadata.len())
    }

    fn create_dir_all(&self, path: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(path)
    }
}

#[derive(Serialize, Deserialize)]
struct LibraryFile {
    version: u32,
    entries: Vec<LibraryEntry>,
}

type Observer = (ObserverId, Box<dyn FnMut(&LibraryEvent)>);

/// The installed custom themes, durable in the data directory.
pub struct Library {
    data_dir: PathBuf,
    entries: Vec<LibraryEntry>,
    load_warning: Option<String>,
    adapters: Vec<Box<dyn ImportAdapter>>,
    observers: Vec<Observer>,
    next_observer: u64,
    fs: Box<dyn LibraryFs>,
}

fn primary_path(dir: &Path) -> PathBuf {
    dir.join(LIBRARY_FILE)
}

fn backup_path(dir: &Path) -> PathBuf {
    dir.join(format!("{LIBRARY_FILE}.bak"))
}

fn temporary_path(dir: &Path) -> PathBuf {
    dir.join(format!("{LIBRARY_FILE}.tmp"))
}

fn unique_id(base: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut id = base.to_string();
    let mut suffix = 2;
    while taken(&id) {
        id = format!("{base}-{suffix}");
        suffix += 1;
    }
    id
}

/// Re-keys a family and its reports to `new_id` (THM-004 R9): a variant id that starts with the
/// old family id has that prefix replaced, any other gets `<new id>-` prepended.
fn rekey(family: &mut ThemeFamily, reports: &mut [VariantReport], new_id: &str) {
    let old_id = family.id.clone();
    let rekeyed = |id: &str| match id.strip_prefix(old_id.as_str()) {
        Some(rest) => format!("{new_id}{rest}"),
        None => format!("{new_id}-{id}"),
    };
    for variant in &mut family.variants {
        variant.id = rekeyed(&variant.id);
        variant.family_id = new_id.to_string();
    }
    for report in reports {
        report.variant_id = rekeyed(&report.variant_id);
    }
    family.id = new_id.to_string();
}

fn limits_error(entries: &[LibraryEntry]) -> Option<String> {
    if entries.len() > MAX_ENTRIES {
        return Some(format!(
            "the library would hold {} entries; the limit is {MAX_ENTRIES}",
            entries.len()
        ));
    }
    let mut total = 0usize;
    for entry in entries {
        match total.checked_add(entry.family.variants.len()) {
            Some(sum) => total = sum,
            None => return Some("the library variant count overflows".to_string()),
        }
    }
    if total > MAX_TOTAL_VARIANTS {
        return Some(format!(
            "the library would hold {total} variants; the limit is {MAX_TOTAL_VARIANTS}"
        ));
    }
    None
}

fn blocking_message(issues: &[ValidationIssue]) -> Option<String> {
    if !has_blocking_issue(issues) {
        return None;
    }
    let messages: Vec<String> = issues
        .iter()
        .filter(|issue| issue.is_blocking())
        .map(ToString::to_string)
        .collect();
    Some(format!("validation failed: {}", messages.join("; ")))
}

impl Library {
    /// Loads the library from `data_dir`. A missing file is an empty library; a library that
    /// cannot be loaded yields an empty library plus a warning, never a crash. The primary file
    /// is used if it exists, otherwise the backup (a crash between the two renames of a save).
    pub fn load(data_dir: &Path) -> Self {
        Self::load_with_fs(data_dir, Box::new(StdFs))
    }

    /// [`Library::load`] over a custom file system.
    pub fn load_with_fs(data_dir: &Path, fs: Box<dyn LibraryFs>) -> Self {
        let mut library = Self {
            data_dir: data_dir.to_path_buf(),
            entries: Vec::new(),
            load_warning: None,
            adapters: vec![Box::new(VsCodeAdapter)],
            observers: Vec::new(),
            next_observer: 0,
            fs,
        };
        let primary = primary_path(data_dir);
        let source = if library.fs.exists(&primary) {
            Some(primary)
        } else if library.fs.exists(&backup_path(data_dir)) {
            Some(backup_path(data_dir))
        } else {
            None
        };
        if let Some(source) = source {
            match library.read_entries(&source) {
                Ok(entries) => library.entries = entries,
                Err(message) => {
                    library.load_warning = Some(format!(
                        "the theme library {} could not be loaded and was replaced by an empty library: {message}",
                        source.display()
                    ));
                }
            }
        }
        library
    }

    fn read_entries(&self, path: &Path) -> Result<Vec<LibraryEntry>, String> {
        let length = self.fs.len(path).map_err(|error| error.to_string())?;
        if length > MAX_LIBRARY_BYTES as u64 {
            return Err(format!(
                "the file is {length} bytes; the limit is {MAX_LIBRARY_BYTES} bytes"
            ));
        }
        let bytes = self.fs.read(path).map_err(|error| error.to_string())?;
        if bytes.len() > MAX_LIBRARY_BYTES {
            return Err(format!(
                "the file is {} bytes; the limit is {MAX_LIBRARY_BYTES} bytes",
                bytes.len()
            ));
        }
        let file: LibraryFile =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        if file.version != LIBRARY_VERSION {
            return Err(format!("unsupported library version {}", file.version));
        }
        if let Some(message) = limits_error(&file.entries) {
            return Err(message);
        }
        Ok(file.entries)
    }

    /// Adds an import adapter (reload looks adapters up by format name).
    pub fn add_adapter(&mut self, adapter: Box<dyn ImportAdapter>) {
        self.adapters
            .retain(|existing| existing.format() != adapter.format());
        self.adapters.push(adapter);
    }

    /// The data directory.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Installed entries in library order.
    pub fn entries(&self) -> &[LibraryEntry] {
        &self.entries
    }

    /// An entry by identifier.
    pub fn entry(&self, id: &str) -> Option<&LibraryEntry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    /// The persistent warning left by a library that could not be loaded.
    pub fn load_warning(&self) -> Option<&str> {
        self.load_warning.as_deref()
    }

    /// The families of the entries in library order.
    pub fn families(&self) -> Vec<ThemeFamily> {
        self.entries
            .iter()
            .map(|entry| entry.family.clone())
            .collect()
    }

    /// The runtime registry: built-in families plus the families of the entries, in order.
    pub fn runtime_registry(&self) -> ThemeRegistry {
        ThemeRegistry::with_custom(self.families())
    }

    /// Installs the runtime registry into the appearance state: a forced reinstall (a linked
    /// theme keeps its identifier across a reload while its colors change). `removed` lists
    /// explicitly removed variant identifiers; a selection naming one is repointed to the first
    /// variant of its appearance (THM-003 R9). Returns whether the selection was repointed, in
    /// which case the host persists it.
    pub fn install_runtime(&self, cx: &mut vespertine::App, removed: &[String]) -> bool {
        appearance::apply_registry_change(cx, self.families(), removed)
    }

    /// Registers a callback for registry events (THM-004 R13).
    pub fn observe(&mut self, callback: impl FnMut(&LibraryEvent) + 'static) -> ObserverId {
        let id = ObserverId(self.next_observer);
        self.next_observer += 1;
        self.observers.push((id, Box::new(callback)));
        id
    }

    /// Removes an observer.
    pub fn unobserve(&mut self, id: ObserverId) {
        self.observers.retain(|(observer, _)| *observer != id);
    }

    fn notify(&mut self, event: LibraryEvent) {
        for (_, callback) in &mut self.observers {
            callback(&event);
        }
    }

    fn serialize(entries: &[LibraryEntry]) -> Result<Vec<u8>, LibraryError> {
        let file = LibraryFileRef {
            version: LIBRARY_VERSION,
            entries,
        };
        let bytes = serde_json::to_vec_pretty(&file)
            .map_err(|error| LibraryError(format!("serializing the library failed: {error}")))?;
        if bytes.len() > MAX_LIBRARY_BYTES {
            return fail(format!(
                "the library would be {} bytes; the limit is {MAX_LIBRARY_BYTES} bytes",
                bytes.len()
            ));
        }
        Ok(bytes)
    }

    /// Validates limits and the registry of `entries`, persists them atomically and only then
    /// replaces the in-memory library.
    fn commit(&mut self, entries: Vec<LibraryEntry>) -> Result<(), LibraryError> {
        if let Some(message) = limits_error(&entries) {
            return fail(message);
        }
        let registry = ThemeRegistry::with_custom(entries.iter().map(|entry| entry.family.clone()));
        if let Some(message) = blocking_message(&registry.validate()) {
            return fail(message);
        }
        let bytes = Self::serialize(&entries)?;
        self.save_bytes(&bytes)?;
        self.entries = entries;
        self.load_warning = None;
        Ok(())
    }

    /// Durable save (THM-004 R12): write a temporary file and sync it; if a previous file exists
    /// remove any stale backup, rename the current file to the backup name; rename the
    /// temporary file into place; remove the backup. A failed final rename restores the backup.
    fn save_bytes(&self, bytes: &[u8]) -> Result<(), LibraryError> {
        let fs = &*self.fs;
        let primary = primary_path(&self.data_dir);
        let backup = backup_path(&self.data_dir);
        let temporary = temporary_path(&self.data_dir);
        let io = |what: &str, path: &Path, error: std::io::Error| {
            LibraryError(format!("{what} {} failed: {error}", path.display()))
        };
        fs.create_dir_all(&self.data_dir)
            .map_err(|error| io("creating", &self.data_dir, error))?;
        if let Err(error) = fs.write_synced(&temporary, bytes) {
            fs.remove_file(&temporary).ok();
            return Err(io("writing", &temporary, error));
        }
        if fs.exists(&primary) {
            if fs.exists(&backup) {
                if let Err(error) = fs.remove_file(&backup) {
                    fs.remove_file(&temporary).ok();
                    return Err(io("removing the stale backup", &backup, error));
                }
            }
            if let Err(error) = fs.rename(&primary, &backup) {
                fs.remove_file(&temporary).ok();
                return Err(io("backing up", &primary, error));
            }
            if let Err(error) = fs.rename(&temporary, &primary) {
                return match fs.rename(&backup, &primary) {
                    Ok(()) => {
                        fs.remove_file(&temporary).ok();
                        Err(io("replacing", &primary, error))
                    }
                    Err(_) => Err(LibraryError(format!(
                        "replacing {} failed ({error}) and the previous library could not be restored; the recovery copy is {}",
                        primary.display(),
                        backup.display()
                    ))),
                };
            }
            if let Err(error) = fs.remove_file(&backup) {
                log::warn!("removing {} failed: {error}", backup.display());
            }
        } else if let Err(error) = fs.rename(&temporary, &primary) {
            fs.remove_file(&temporary).ok();
            return Err(io("replacing", &primary, error));
        }
        Ok(())
    }

    /// Persists the current library (for example after the owner changed its state elsewhere).
    pub fn save(&mut self) -> Result<(), LibraryError> {
        let bytes = Self::serialize(&self.entries)?;
        self.save_bytes(&bytes)?;
        self.load_warning = None;
        Ok(())
    }

    fn position(&self, id: &str) -> Result<usize, LibraryError> {
        match self.entries.iter().position(|entry| entry.id == id) {
            Some(position) => Ok(position),
            None => fail(format!("no installed theme `{id}`")),
        }
    }

    /// Installs a compilation (THM-004 R9). `selected` names the variants to install (empty
    /// means all successfully compiled ones); at least one must remain. The entry identifier is
    /// the family id made unique with `-2`, `-3`, ...; the family is re-keyed when it changed.
    /// Validation runs on the re-keyed family. Returns the entry identifier.
    pub fn install(
        &mut self,
        compilation: &Compilation,
        selected: &[String],
        mode: InstallMode,
    ) -> Result<String, LibraryError> {
        let chosen: Vec<usize> = if selected.is_empty() {
            (0..compilation.family.variants.len()).collect()
        } else {
            let mut chosen = Vec::new();
            for id in selected {
                match compilation
                    .family
                    .variants
                    .iter()
                    .position(|variant| &variant.id == id)
                {
                    Some(position) => chosen.push(position),
                    None => {
                        let reason = compilation
                            .failures
                            .iter()
                            .find(|failure| &failure.id == id)
                            .map_or_else(
                                || "it is not part of the compilation".to_string(),
                                |failure| failure.message.clone(),
                            );
                        return fail(format!("variant `{id}` cannot be installed: {reason}"));
                    }
                }
            }
            chosen.sort_unstable();
            chosen.dedup();
            chosen
        };
        if chosen.is_empty() {
            return fail("at least one successfully compiled variant must be installed");
        }
        let mut family = ThemeFamily {
            id: compilation.family.id.clone(),
            name: compilation.family.name.clone(),
            variants: chosen
                .iter()
                .map(|&position| compilation.family.variants[position].clone())
                .collect(),
        };
        let mut reports: Vec<VariantReport> = family
            .variants
            .iter()
            .filter_map(|variant| {
                compilation
                    .reports
                    .iter()
                    .find(|report| report.variant_id == variant.id)
                    .cloned()
            })
            .collect();
        let id = unique_id(&family.id, |candidate| self.entry(candidate).is_some());
        if id != family.id {
            rekey(&mut family, &mut reports, &id);
        }
        if let Some(message) = blocking_message(&family.validate()) {
            return fail(message);
        }
        let source = match (mode, compilation.source_kind) {
            (InstallMode::Snapshot, _) => EntrySource::ImportedSnapshot {
                imported_from: Some(compilation.source_path.clone()),
            },
            (InstallMode::Link, SourceKind::File) => EntrySource::LinkedFile {
                path: compilation.source_path.clone(),
                format: compilation.format.clone(),
                options: compilation.options.clone(),
            },
            (InstallMode::Link, SourceKind::Package) => EntrySource::LinkedPackage {
                path: compilation.source_path.clone(),
                format: compilation.format.clone(),
                options: compilation.options.clone(),
            },
        };
        let entry = LibraryEntry {
            id: id.clone(),
            source,
            status: EntryStatus::Ready,
            selected_variants: family
                .variants
                .iter()
                .map(|variant| variant.id.clone())
                .collect(),
            family,
            reports,
        };
        let mut entries = self.entries.clone();
        entries.push(entry);
        self.commit(entries)?;
        self.notify(LibraryEvent::Installed { id: id.clone() });
        Ok(id)
    }

    fn compile_linked(
        &self,
        entry: &LibraryEntry,
        path: &Path,
        format: &str,
        options: &ImportOptions,
    ) -> Result<(ThemeFamily, Vec<VariantReport>), String> {
        let adapter = self
            .adapters
            .iter()
            .find(|adapter| adapter.format() == format)
            .ok_or_else(|| format!("no import adapter for format `{format}`"))?;
        let options = ImportOptions {
            family_id: entry.id.clone(),
            family_name: entry.family.name.clone(),
            ..options.clone()
        };
        let kept = &entry.selected_variants;
        let compilation = adapter.compile(path, &options).map_err(|error| {
            match error
                .failures
                .iter()
                .find(|failure| kept.contains(&failure.id))
            {
                Some(failure) => {
                    format!("variant `{}` failed: {}", failure.id, failure.message)
                }
                None => error.message,
            }
        })?;
        if let Some(failure) = compilation
            .failures
            .iter()
            .find(|failure| kept.contains(&failure.id))
        {
            return Err(format!(
                "variant `{}` failed: {}",
                failure.id, failure.message
            ));
        }
        let mut family = ThemeFamily {
            id: entry.id.clone(),
            name: entry.family.name.clone(),
            variants: compilation
                .family
                .variants
                .iter()
                .filter(|variant| kept.contains(&variant.id))
                .cloned()
                .collect(),
        };
        if family.variants.is_empty() {
            return Err("the source no longer contains any selected variant".to_string());
        }
        let reports = compilation
            .reports
            .iter()
            .filter(|report| {
                family
                    .variants
                    .iter()
                    .any(|variant| variant.id == report.variant_id)
            })
            .cloned()
            .collect();
        for variant in &mut family.variants {
            variant.family_id = entry.id.clone();
        }
        Ok((family, reports))
    }

    fn read_editable(
        &self,
        entry: &LibraryEntry,
        path: &Path,
    ) -> Result<(ThemeFamily, Vec<VariantReport>), String> {
        let length = self
            .fs
            .len(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if length > MAX_SOURCE_BYTES {
            return Err(format!(
                "{} is {length} bytes; the limit is {MAX_SOURCE_BYTES} bytes",
                path.display()
            ));
        }
        let bytes = self
            .fs
            .read(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let family: ThemeFamily = serde_json::from_slice(&bytes)
            .map_err(|error| format!("{} is not a valid theme family: {error}", path.display()))?;
        if family.id != entry.id {
            return Err(format!(
                "the family identifier changed from `{}` to `{}`; it must stay the same",
                entry.id, family.id
            ));
        }
        let reports = entry
            .reports
            .iter()
            .filter(|report| {
                family
                    .variants
                    .iter()
                    .any(|variant| variant.id == report.variant_id)
            })
            .cloned()
            .collect();
        Ok((family, reports))
    }

    /// Recompiles a linked entry or re-reads an editable file. Any failure keeps the last known
    /// good family, sets the entry status to a warning with the message and returns the error;
    /// success replaces the family and reports and sets the status ready (THM-004 R10).
    pub fn reload(&mut self, id: &str) -> Result<(), LibraryError> {
        let position = self.position(id)?;
        let entry = self.entries[position].clone();
        let outcome: Result<(ThemeFamily, Vec<VariantReport>), String> = match &entry.source {
            EntrySource::ImportedSnapshot { .. } => {
                return fail("an imported snapshot cannot be reloaded");
            }
            EntrySource::LinkedFile {
                path,
                format,
                options,
            }
            | EntrySource::LinkedPackage {
                path,
                format,
                options,
            } => self.compile_linked(&entry, path, format, options),
            EntrySource::EditableFile { path } => self.read_editable(&entry, path),
        };
        let outcome =
            outcome.and_then(
                |(family, reports)| match blocking_message(&family.validate()) {
                    Some(message) => Err(message),
                    None => Ok((family, reports)),
                },
            );
        match outcome {
            Ok((family, reports)) => {
                let mut updated = entry;
                updated.selected_variants = family
                    .variants
                    .iter()
                    .map(|variant| variant.id.clone())
                    .collect();
                updated.family = family;
                updated.reports = reports;
                updated.status = EntryStatus::Ready;
                let mut entries = self.entries.clone();
                entries[position] = updated;
                match self.commit(entries) {
                    Ok(()) => {
                        self.notify(LibraryEvent::Reloaded { id: id.to_string() });
                        Ok(())
                    }
                    Err(error) => self.reload_failed(position, id, error.0),
                }
            }
            Err(message) => self.reload_failed(position, id, message),
        }
    }

    fn reload_failed(
        &mut self,
        position: usize,
        id: &str,
        message: String,
    ) -> Result<(), LibraryError> {
        self.entries[position].status = EntryStatus::Warning {
            message: message.clone(),
        };
        self.notify(LibraryEvent::ReloadFailed {
            id: id.to_string(),
            message: message.clone(),
        });
        Err(LibraryError(message))
    }

    /// Converts a linked entry to a snapshot that keeps the old path as `imported from`.
    pub fn unlink(&mut self, id: &str) -> Result<(), LibraryError> {
        let position = self.position(id)?;
        let mut entry = self.entries[position].clone();
        let path = match &entry.source {
            EntrySource::LinkedFile { path, .. } | EntrySource::LinkedPackage { path, .. } => {
                path.clone()
            }
            _ => return fail(format!("`{id}` is not a linked entry")),
        };
        entry.source = EntrySource::ImportedSnapshot {
            imported_from: Some(path),
        };
        entry.status = EntryStatus::Ready;
        let mut entries = self.entries.clone();
        entries[position] = entry;
        self.commit(entries)?;
        self.notify(LibraryEvent::Unlinked { id: id.to_string() });
        Ok(())
    }

    fn duplicate_entry(&self, id: &str) -> Result<LibraryEntry, LibraryError> {
        let original = &self.entries[self.position(id)?];
        let new_id = unique_id(&format!("{id}-copy"), |candidate| {
            self.entry(candidate).is_some()
        });
        let mut family = original.family.clone();
        let mut reports = original.reports.clone();
        rekey(&mut family, &mut reports, &new_id);
        family.name = format!("{} Copy", original.family.name);
        Ok(LibraryEntry {
            id: new_id,
            source: EntrySource::ImportedSnapshot {
                imported_from: original.location().map(Path::to_path_buf),
            },
            status: EntryStatus::Ready,
            selected_variants: family
                .variants
                .iter()
                .map(|variant| variant.id.clone())
                .collect(),
            family,
            reports,
        })
    }

    /// Duplicates an entry as an immutable snapshot named `<id>-copy` / `<name> Copy`.
    pub fn duplicate_as_snapshot(&mut self, id: &str) -> Result<String, LibraryError> {
        let entry = self.duplicate_entry(id)?;
        let new_id = entry.id.clone();
        let mut entries = self.entries.clone();
        entries.push(entry);
        self.commit(entries)?;
        self.notify(LibraryEvent::Duplicated {
            from: id.to_string(),
            id: new_id.clone(),
        });
        Ok(new_id)
    }

    /// Duplicates an entry as an editable native resolved-family file
    /// `<new id>.theme.json` in the `custom-themes` directory under `data_dir`.
    pub fn duplicate_as_editable(
        &mut self,
        id: &str,
        data_dir: &Path,
    ) -> Result<String, LibraryError> {
        let mut entry = self.duplicate_entry(id)?;
        let directory = data_dir.join(EDITABLE_DIR);
        let path = directory.join(format!("{}.theme.json", entry.id));
        let bytes = serde_json::to_vec_pretty(&entry.family)
            .map_err(|error| LibraryError(format!("serializing the family failed: {error}")))?;
        self.fs.create_dir_all(&directory).map_err(|error| {
            LibraryError(format!("creating {} failed: {error}", directory.display()))
        })?;
        let temporary = directory.join(format!("{}.theme.json.tmp", entry.id));
        self.fs
            .write_synced(&temporary, &bytes)
            .and_then(|()| self.fs.rename(&temporary, &path))
            .map_err(|error| {
                self.fs.remove_file(&temporary).ok();
                LibraryError(format!("writing {} failed: {error}", path.display()))
            })?;
        entry.source = EntrySource::EditableFile { path: path.clone() };
        let new_id = entry.id.clone();
        let mut entries = self.entries.clone();
        entries.push(entry);
        if let Err(error) = self.commit(entries) {
            self.fs.remove_file(&path).ok();
            return Err(error);
        }
        self.notify(LibraryEvent::Duplicated {
            from: id.to_string(),
            id: new_id.clone(),
        });
        Ok(new_id)
    }

    /// Removes an entry; an unknown identifier is an error. Returns the identifiers of the
    /// removed variants, which the host passes to [`Library::install_runtime`] so a selection
    /// naming one is repointed.
    pub fn remove(&mut self, id: &str) -> Result<Vec<String>, LibraryError> {
        let position = self.position(id)?;
        let family = &self.entries[position].family;
        let variant_ids: Vec<String> = family.variants.iter().map(|v| v.id.clone()).collect();
        let mut entries = self.entries.clone();
        entries.remove(position);
        self.commit(entries)?;
        self.notify(LibraryEvent::Removed {
            id: id.to_string(),
            variant_ids: variant_ids.clone(),
        });
        Ok(variant_ids)
    }

    /// Asks the platform (`reveal`) to show the entry's source location in the file manager; an
    /// entry without a location is an error.
    pub fn reveal(&self, id: &str, reveal: impl FnOnce(&Path)) -> Result<(), LibraryError> {
        let entry = &self.entries[self.position(id)?];
        match entry.location() {
            Some(path) => {
                reveal(path);
                Ok(())
            }
            None => fail(format!("`{id}` has no source location")),
        }
    }
}

/// Keeps the appearance resolution in step with `library` (THM-004 R10, R13): after every
/// library change the runtime registry is reinstalled with a forced style generation advance
/// (a reloaded theme keeps its identifier while its colors change) and an explicitly removed
/// theme that was selected is repointed. The reinstall runs as soon as the operation that
/// caused the event has finished. Returns the observer registration.
pub fn bind_runtime(cx: &vespertine::App, library: &Rc<RefCell<Library>>) -> ObserverId {
    let app = cx.to_async();
    let weak = Rc::downgrade(library);
    library.borrow_mut().observe(move |event| {
        let removed = match event {
            LibraryEvent::Removed { variant_ids, .. } => variant_ids.clone(),
            // The last good family stays active: nothing to reinstall.
            LibraryEvent::ReloadFailed { .. } => return,
            _ => Vec::new(),
        };
        let Some(library) = weak.upgrade() else {
            return;
        };
        let app = app.clone();
        let executor = app.foreground_executor().clone();
        executor
            .spawn(async move {
                app.update(|cx| {
                    library.borrow().install_runtime(cx, &removed);
                });
            })
            .detach();
    })
}

#[derive(Serialize)]
struct LibraryFileRef<'a> {
    version: u32,
    entries: &'a [LibraryEntry],
}
