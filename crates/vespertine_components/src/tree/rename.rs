//! Inline rename (CMP-016 R16) and the delete subject text (R17), as pure logic.

use super::model::{file_name, join_path, parent_path};
use std::ops::Range;

/// The message shown when a name fails validation.
pub const INVALID_NAME_MESSAGE: &str = "Enter a single file name without path separators";
/// The message shown when the entry changed since the rename began.
pub const ENTRY_CHANGED_MESSAGE: &str = "Entry changed; cancel and reopen Rename";

/// The byte range of `name` that the rename field selects: the whole name for directories; for
/// files from the start up to the last `.` that is not at index 0 (the whole name if none).
pub fn basename_range(name: &str, is_directory: bool) -> Range<usize> {
    if is_directory {
        return 0..name.len();
    }
    match name.rfind('.') {
        Some(index) if index > 0 => 0..index,
        _ => 0..name.len(),
    }
}

/// Validates a new entry name: non-empty, not `.` or `..`, not `.git` (case-insensitive) and
/// containing none of `/`, `\`, NUL or `:`.
pub fn validate_name(name: &str) -> Result<(), &'static str> {
    let invalid = name.is_empty()
        || name == "."
        || name == ".."
        || name.eq_ignore_ascii_case(".git")
        || name.contains(['/', '\\', '\0', ':']);
    if invalid {
        Err(INVALID_NAME_MESSAGE)
    } else {
        Ok(())
    }
}

/// The subject text of a delete confirmation: one entry `a.txt`; two `2 items (a and b)`;
/// three or more `N items (a, b and K more)` with `K = N - 2`. Names are the last components of
/// `paths` (the action paths: selection roots).
pub fn delete_subject(paths: &[String]) -> String {
    let names: Vec<&str> = paths.iter().map(|path| file_name(path)).collect();
    match names.as_slice() {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [a, b] => format!("2 items ({a} and {b})"),
        [a, b, ..] => format!(
            "{} items ({a}, {b} and {} more)",
            names.len(),
            names.len() - 2
        ),
    }
}

/// What committing the rename field did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenameCommit {
    /// Nothing to do (a commit is already in flight).
    Ignored,
    /// The name is unchanged: just close.
    Unchanged,
    /// The commit is rejected with this message; the field stays.
    Rejected(String),
    /// The owner must rename `from` to `to`.
    Request {
        /// The current path.
        from: String,
        /// The path with the new name.
        to: String,
    },
}

/// The state of one inline rename.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenameSession {
    path: String,
    name: String,
    revision: Option<String>,
    in_flight: bool,
    error: Option<String>,
}

impl RenameSession {
    /// Begins renaming `path`, remembering the entry's revision token.
    pub fn new(path: impl Into<String>, revision: Option<String>) -> Self {
        let path = path.into();
        let name = file_name(&path).to_owned();
        Self {
            path,
            name,
            revision,
            in_flight: false,
            error: None,
        }
    }

    /// The path being renamed.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The name the field starts with.
    pub fn original_name(&self) -> &str {
        &self.name
    }

    /// Whether a commit is in flight.
    pub fn in_flight(&self) -> bool {
        self.in_flight
    }

    /// The message of the last rejected commit.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Re-keys the session after the owner relocated the entry.
    pub fn relocated(&mut self, path: &str) {
        self.path = path.to_owned();
    }

    /// Enter: validates `name` and, when valid and changed, marks a commit in flight. Further
    /// commits are ignored until [`Self::finish`]. `current_revision` is the entry's revision
    /// token now; a changed token blocks the commit.
    pub fn commit(&mut self, name: &str, current_revision: Option<&str>) -> RenameCommit {
        if self.in_flight {
            return RenameCommit::Ignored;
        }
        if name == self.name {
            return RenameCommit::Unchanged;
        }
        if let Err(message) = validate_name(name) {
            self.error = Some(message.to_owned());
            return RenameCommit::Rejected(message.to_owned());
        }
        if self.revision.as_deref() != current_revision {
            self.error = Some(ENTRY_CHANGED_MESSAGE.to_owned());
            return RenameCommit::Rejected(ENTRY_CHANGED_MESSAGE.to_owned());
        }
        self.error = None;
        self.in_flight = true;
        RenameCommit::Request {
            from: self.path.clone(),
            to: join_path(parent_path(&self.path), name),
        }
    }

    /// The owner answered the request: on failure the commit may be retried and `message` is
    /// shown; returns whether the session is over (the commit succeeded).
    pub fn finish(&mut self, result: Result<(), String>) -> bool {
        self.in_flight = false;
        match result {
            Ok(()) => true,
            Err(message) => {
                self.error = Some(message);
                false
            }
        }
    }

    /// Losing focus cancels the rename unless a commit is in flight; returns whether it cancels.
    pub fn cancels_on_blur(&self) -> bool {
        !self.in_flight
    }
}
