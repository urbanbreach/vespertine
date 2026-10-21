//! Native path prompts through the xdg-desktop-portal FileChooser interface.
//!
//! Request resolution (filter validation, the mixed files-and-directories rule) and the
//! interpretation of the portal's answer are pure functions, so they can be exercised without a
//! portal; [`prompt_for_paths`] and [`prompt_for_new_path`] wire them to the ashpd builders.

use anyhow::{Result, anyhow, bail};
use ashpd::desktop::file_chooser::{FileFilter, OpenFileRequest, SaveFileRequest, SelectedFiles};
use futures::future::BoxFuture;
use std::path::{Path, PathBuf};
use vespertine::{PathPromptFilter, PathPromptOptions};

pub(super) const FILE_PICKER_PORTAL_MISSING: &str =
    "Couldn't open file picker due to missing xdg-desktop-portal implementation.";

pub(super) const SELECTION_NOT_LOCAL: &str =
    "The selected location is not a local file, so it can't be used.";

/// A validated filter: a label and case-insensitive glob patterns.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PortalFilter {
    name: String,
    globs: Vec<String>,
}

/// An OpenFile request, after the framework-level options were resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
struct OpenRequest {
    title: &'static str,
    directory: bool,
    /// The first filter doubles as the default (current) filter.
    filters: Vec<PortalFilter>,
}

/// How a portal request ended without a selection.
#[derive(Debug, PartialEq, Eq)]
enum PortalFailure {
    /// No xdg-desktop-portal implementation provides the FileChooser interface.
    Missing,
    /// The request ended otherwise (response code 2, a bus error, ...).
    Other(String),
}

/// The portal's answer to a request that started.
#[derive(Debug, PartialEq, Eq)]
enum PortalResponse {
    /// Response code 0 with the selected URIs.
    Selected(Vec<String>),
    /// Response code 1: the user cancelled.
    Cancelled,
}

type PortalResult = std::result::Result<PortalResponse, PortalFailure>;

/// Converts framework filters into portal filters, one glob per extension.
fn portal_filters(filters: &[PathPromptFilter]) -> Result<Vec<PortalFilter>> {
    filters
        .iter()
        .map(|filter| {
            if filter.extensions.is_empty() {
                bail!("file filter {:?} has no extensions", filter.name);
            }
            let mut globs = Vec::with_capacity(filter.extensions.len());
            for extension in &filter.extensions {
                if extension.trim_start_matches('.').is_empty() {
                    bail!("file filter {:?} has an empty extension", filter.name);
                }
                globs.push(PathPromptFilter::extension_glob(extension));
            }
            Ok(PortalFilter {
                name: filter.name.to_string(),
                globs,
            })
        })
        .collect()
}

/// Resolves an open prompt. A request for both files and directories becomes a directory
/// prompt because the portal can't mix them; filters only apply to file prompts.
fn open_request(options: &PathPromptOptions) -> Result<OpenRequest> {
    let directory = options.directories;
    Ok(OpenRequest {
        title: if directory {
            "Open Folder"
        } else {
            "Open File"
        },
        directory,
        filters: if directory {
            Vec::new()
        } else {
            portal_filters(&options.filters)?
        },
    })
}

fn failure_to_error(failure: PortalFailure) -> anyhow::Error {
    match failure {
        PortalFailure::Missing => anyhow!(FILE_PICKER_PORTAL_MISSING),
        PortalFailure::Other(message) => {
            anyhow!("The file picker failed: {message}")
        }
    }
}

/// Keeps the `file://` selections. `Ok(None)` means nothing was selected; a selection with no
/// local entry at all is an error.
fn local_selection(uris: Vec<String>) -> Result<Option<Vec<PathBuf>>> {
    if uris.is_empty() {
        return Ok(None);
    }
    let paths: Vec<PathBuf> = uris
        .iter()
        .filter_map(|uri| url::Url::parse(uri).ok())
        .filter_map(|uri| uri.to_file_path().ok())
        .collect();
    if paths.is_empty() {
        bail!(SELECTION_NOT_LOCAL);
    }
    Ok(Some(paths))
}

/// What an open prompt yields: the local paths, `None` for a cancellation.
fn open_outcome(result: PortalResult) -> Result<Option<Vec<PathBuf>>> {
    match result.map_err(failure_to_error)? {
        PortalResponse::Cancelled => Ok(None),
        PortalResponse::Selected(uris) => local_selection(uris),
    }
}

/// What a save prompt yields: the chosen local path, `None` for a cancellation.
fn save_outcome(result: PortalResult) -> Result<Option<PathBuf>> {
    match result.map_err(failure_to_error)? {
        PortalResponse::Cancelled => Ok(None),
        PortalResponse::Selected(uris) => {
            Ok(local_selection(uris)?.and_then(|paths| paths.into_iter().next()))
        }
    }
}

fn file_filter(filter: &PortalFilter) -> FileFilter {
    filter
        .globs
        .iter()
        .fold(FileFilter::new(&filter.name), |file_filter, glob| {
            file_filter.glob(glob)
        })
}

fn portal_error(error: ashpd::Error) -> PortalFailure {
    match error {
        ashpd::Error::PortalNotFound(_) => PortalFailure::Missing,
        error => PortalFailure::Other(error.to_string()),
    }
}

/// Maps the portal's response: code 1 is a cancellation, other non-zero codes are failures.
fn portal_response(response: std::result::Result<SelectedFiles, ashpd::Error>) -> PortalResult {
    match response {
        Ok(files) => Ok(PortalResponse::Selected(
            files
                .uris()
                .iter()
                .map(|uri| uri.as_str().to_owned())
                .collect(),
        )),
        Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => {
            Ok(PortalResponse::Cancelled)
        }
        Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Other)) => Err(
            PortalFailure::Other("the portal ended the request without a selection".to_owned()),
        ),
        Err(error) => Err(portal_error(error)),
    }
}

/// Runs an open prompt to completion.
pub(super) async fn prompt_for_paths(
    identifier: BoxFuture<'static, Option<ashpd::WindowIdentifier>>,
    options: &PathPromptOptions,
) -> Result<Option<Vec<PathBuf>>> {
    let request = open_request(options)?;
    let result = async {
        let mut builder = OpenFileRequest::default()
            .identifier(identifier.await)
            .modal(true)
            .title(request.title)
            .accept_label(options.prompt.as_deref())
            .multiple(options.multiple)
            .directory(request.directory)
            .filters(request.filters.iter().map(file_filter));
        if let Some(first) = request.filters.first() {
            builder = builder.current_filter(file_filter(first));
        }
        let pending = builder.send().await.map_err(portal_error)?;
        portal_response(pending.response())
    }
    .await;
    open_outcome(result)
}

/// Runs a save prompt to completion.
pub(super) async fn prompt_for_new_path(
    identifier: BoxFuture<'static, Option<ashpd::WindowIdentifier>>,
    directory: &Path,
    suggested_name: Option<&str>,
    filters: &[PathPromptFilter],
) -> Result<Option<PathBuf>> {
    let filters = portal_filters(filters)?;
    let result = async {
        let mut builder = SaveFileRequest::default()
            .identifier(identifier.await)
            .modal(true)
            .title("Save File")
            .current_folder(directory)
            .map_err(|error| {
                PortalFailure::Other(format!(
                    "the start directory can't be passed to the portal: {error}"
                ))
            })?
            .filters(filters.iter().map(file_filter));
        if let Some(name) = suggested_name {
            builder = builder.current_name(name);
        }
        if let Some(first) = filters.first() {
            builder = builder.current_filter(file_filter(first));
        }
        let pending = builder.send().await.map_err(portal_error)?;
        portal_response(pending.response())
    }
    .await;
    save_outcome(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(files: bool, directories: bool) -> PathPromptOptions {
        PathPromptOptions {
            files,
            directories,
            multiple: false,
            prompt: None,
            filters: Vec::new(),
        }
    }

    fn selected(uris: &[&str]) -> PortalResult {
        Ok(PortalResponse::Selected(
            uris.iter().map(|uri| (*uri).to_owned()).collect(),
        ))
    }

    #[test]
    fn extension_globs_are_case_classes() {
        assert_eq!(PathPromptFilter::extension_glob("png"), "*.[pP][nN][gG]");
        assert_eq!(PathPromptFilter::extension_glob("JPG"), "*.[jJ][pP][gG]");
        assert_eq!(PathPromptFilter::extension_glob(".tar"), "*.[tT][aA][rR]");
        assert_eq!(PathPromptFilter::extension_glob("7z"), "*.7[zZ]");
    }

    #[test]
    fn portal_filters_keep_order_and_expand_every_extension() {
        let mut prompt = options(true, false);
        prompt.filters = vec![
            PathPromptFilter::new("Images", ["png", "JPG"]),
            PathPromptFilter::new("Text", [".txt"]),
        ];
        let request = open_request(&prompt).unwrap();
        assert_eq!(
            request.filters,
            vec![
                PortalFilter {
                    name: "Images".into(),
                    globs: vec!["*.[pP][nN][gG]".into(), "*.[jJ][pP][gG]".into()],
                },
                PortalFilter {
                    name: "Text".into(),
                    globs: vec!["*.[tT][xX][tT]".into()],
                },
            ]
        );
        assert_eq!(request.title, "Open File");
    }

    #[test]
    fn filters_without_extensions_are_an_error_not_ignored() {
        let mut prompt = options(true, false);
        prompt.filters = vec![PathPromptFilter::new("Nothing", Vec::<&str>::new())];
        assert!(open_request(&prompt).is_err());
        prompt.filters = vec![PathPromptFilter::new("Dot", ["."])];
        assert!(open_request(&prompt).is_err());
    }

    #[test]
    fn mixed_request_shows_a_directory_prompt() {
        let request = open_request(&options(true, true)).unwrap();
        assert!(request.directory);
        assert_eq!(request.title, "Open Folder");
        let request = open_request(&options(true, false)).unwrap();
        assert!(!request.directory);
        assert_eq!(request.title, "Open File");
    }

    #[test]
    fn missing_portal_is_a_readable_error() {
        let error = open_outcome(Err(PortalFailure::Missing))
            .unwrap_err()
            .to_string();
        assert!(error.contains("file picker"), "{error}");
        assert!(
            error.contains("xdg-desktop-portal implementation"),
            "{error}"
        );
    }

    #[test]
    fn other_failure_is_an_error_and_cancel_is_not() {
        let error = open_outcome(Err(PortalFailure::Other("bus went away".to_owned())))
            .unwrap_err()
            .to_string();
        assert!(error.contains("bus went away"), "{error}");

        assert_eq!(open_outcome(Ok(PortalResponse::Cancelled)).unwrap(), None);
        assert_eq!(save_outcome(Ok(PortalResponse::Cancelled)).unwrap(), None);
    }

    #[test]
    fn non_file_uris_are_dropped_and_all_non_local_is_an_error() {
        let result = open_outcome(selected(&[
            "smb://server/share/file.txt",
            "file:///tmp/a%20b.txt",
        ]))
        .unwrap();
        assert_eq!(result, Some(vec![PathBuf::from("/tmp/a b.txt")]));

        let error = open_outcome(selected(&["https://example.com/file.txt"]))
            .unwrap_err()
            .to_string();
        assert!(error.contains("not a local file"), "{error}");
    }

    #[test]
    fn save_returns_the_first_local_path() {
        let result = save_outcome(selected(&["file:///home/me/out.png"])).unwrap();
        assert_eq!(result, Some(PathBuf::from("/home/me/out.png")));
    }

    #[test]
    fn save_with_a_non_local_location_is_an_error() {
        let error = save_outcome(selected(&["ftp://host/out.png"]))
            .unwrap_err()
            .to_string();
        assert!(error.contains("not a local file"), "{error}");
    }
}
