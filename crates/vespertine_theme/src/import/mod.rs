//! Theme import (THM-004 R1-R5, R7): the adapter interface, reports and the format-independent
//! compiler that maps authored candidates to complete, hardened variants.

mod compile;
mod vscode;

pub use compile::{
    AccentCandidates, Candidate, RawVariant, Role, compile_variant,
    detect_appearance_from_background,
};
pub use vscode::VsCodeAdapter;

use crate::theme::ThemeFamily;
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    path::{Path, PathBuf},
};

/// Largest accepted source file.
pub const MAX_SOURCE_BYTES: u64 = 4 * 1024 * 1024;
/// Deepest accepted include chain (hops from the root file).
pub const MAX_INCLUDE_DEPTH: usize = 32;
/// Most variants a package may declare.
pub const MAX_PACKAGE_VARIANTS: usize = 128;

/// Whether the source is a single file or a package directory.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    /// A single theme file.
    File,
    /// A directory with a manifest declaring variants.
    Package,
}

/// What the owner supplies with an import.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct ImportOptions {
    /// Source URL text; a blank value records the local path.
    pub source_url: String,
    /// Family display name.
    pub family_name: String,
    /// License text; blank records `User supplied`.
    pub license: String,
    /// Family identifier.
    pub family_id: String,
    /// Revision; blank records `local`.
    pub revision: String,
}

impl ImportOptions {
    /// Options for a user-supplied local path: revision `local`, license `User supplied` and the
    /// path as the URL (THM-004 R3).
    pub fn local(
        family_id: impl Into<String>,
        family_name: impl Into<String>,
        path: &Path,
    ) -> Self {
        Self {
            family_id: family_id.into(),
            family_name: family_name.into(),
            source_url: path.display().to_string(),
            revision: "local".to_string(),
            license: "User supplied".to_string(),
        }
    }
}

/// One mapping used by the compiler: source key -> semantic role, with the value taken.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Mapping {
    /// The source key (for example `editor.background`).
    pub source_key: String,
    /// The semantic role it feeds.
    pub role: String,
    /// The value taken, in color text form.
    pub value: String,
}

/// An adjustment the compiler made; recorded only when the value actually changed.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Adjustment {
    /// The semantic role.
    pub role: String,
    /// The value before.
    pub original: String,
    /// The value after.
    pub resolved: String,
    /// Why.
    pub reason: String,
}

/// An accent color found in the source.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct AccentCandidate {
    /// The source key.
    pub source_key: String,
    /// The value in color text form.
    pub value: String,
}

/// The report of one compiled variant.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct VariantReport {
    /// The variant identifier.
    pub variant_id: String,
    /// Mappings used.
    pub mappings: Vec<Mapping>,
    /// Adjustments made.
    pub adjustments: Vec<Adjustment>,
    /// Accent candidates in preference order.
    pub accent_candidates: Vec<AccentCandidate>,
    /// Readable warnings.
    pub warnings: Vec<String>,
    /// Readable errors (for example contrast failures that could not be fixed).
    pub errors: Vec<String>,
}

/// A variant that could not be compiled.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct VariantFailure {
    /// Readable message.
    pub message: String,
    /// The source file.
    pub path: PathBuf,
    /// Display name.
    pub name: String,
    /// The identifier the variant would have had.
    pub id: String,
}

/// The result of compiling a source.
#[derive(Clone, Debug)]
pub struct Compilation {
    /// File or package.
    pub source_kind: SourceKind,
    /// The compiled family; it holds at least one variant.
    pub family: ThemeFamily,
    /// The path that was compiled (canonicalized).
    pub source_path: PathBuf,
    /// The adapter format that compiled it.
    pub format: String,
    /// The options the compilation used.
    pub options: ImportOptions,
    /// One report per compiled variant, in family order.
    pub reports: Vec<VariantReport>,
    /// Variants that failed; the valid ones are unaffected.
    pub failures: Vec<VariantFailure>,
}

/// Import failure: nothing usable was produced. The active theme is never touched.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ImportError {
    /// Readable message.
    pub message: String,
    /// Per-variant failures, when the source was a package or a single failing file.
    pub failures: Vec<VariantFailure>,
}

impl ImportError {
    /// An error without per-variant failures.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            failures: Vec::new(),
        }
    }
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ImportError {}

/// A format adapter: turns a path (a theme file or a package directory) into complete
/// variants. Format-specific keys never leak out of the adapter.
pub trait ImportAdapter {
    /// The format name recorded in the source metadata.
    fn format(&self) -> &'static str;

    /// Compiles `path` (THM-004 R1).
    fn compile(&self, path: &Path, options: &ImportOptions) -> Result<Compilation, ImportError>;
}
