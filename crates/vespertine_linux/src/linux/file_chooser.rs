//! Native path prompts through the xdg-desktop-portal FileChooser interface.
//!
//! The portal itself sits behind [`FileChooserPortal`] so the request construction and the
//! response interpretation can be exercised with a scripted portal.

use anyhow::{Result, anyhow, bail};
use futures::future::BoxFuture;
use std::path::{Path, PathBuf};
use vespertine::{PathPromptFilter, PathPromptOptions};

pub(super) const FILE_PICKER_PORTAL_MISSING: &str =
    "Couldn't open file picker due to missing xdg-desktop-portal implementation.";

pub(super) const SELECTION_NOT_LOCAL: &str =
    "The selected location is not a local file, so it can't be used.";

/// A filter as handed to the portal: a label and case-insensitive glob patterns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PortalFilter {
    pub(super) name: String,
    pub(super) globs: Vec<String>,
}

/// An OpenFile request, after the framework-level options were resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PortalOpenRequest {
    pub(super) title: &'static str,
    pub(super) accept_label: Option<String>,
    pub(super) multiple: bool,
    pub(super) directory: bool,
    /// The first filter doubles as the default (current) filter.
    pub(super) filters: Vec<PortalFilter>,
}

/// A SaveFile request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PortalSaveRequest {
    pub(super) title: &'static str,
    pub(super) directory: PathBuf,
    pub(super) suggested_name: Option<String>,
    pub(super) filters: Vec<PortalFilter>,
}

/// How a portal request ended without a selection.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum PortalFailure {
    /// No xdg-desktop-portal implementation provides the FileChooser interface.
    Missing,
    /// The request ended otherwise (response code 2, a bus error, ...).
    Other(String),
}

/// The portal's answer to a request that started.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum PortalResponse {
    /// Response code 0 with the selected URIs.
    Selected(Vec<String>),
    /// Response code 1: the user cancelled.
    Cancelled,
}

pub(super) type PortalResult = std::result::Result<PortalResponse, PortalFailure>;

#[cfg(any(feature = "wayland", feature = "x11"))]
pub(super) type WindowIdentifier = ashpd::WindowIdentifier;

/// The FileChooser portal as the prompts see it.
pub(super) trait FileChooserPortal {
    fn open_file(
        &self,
        identifier: Option<WindowIdentifier>,
        request: PortalOpenRequest,
    ) -> BoxFuture<'static, PortalResult>;

    fn save_file(
        &self,
        identifier: Option<WindowIdentifier>,
        request: PortalSaveRequest,
    ) -> BoxFuture<'static, PortalResult>;
}

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
pub(super) fn open_request(options: &PathPromptOptions) -> Result<PortalOpenRequest> {
    let directory = options.directories;
    Ok(PortalOpenRequest {
        title: if directory {
            "Open Folder"
        } else {
            "Open File"
        },
        accept_label: options.prompt.as_ref().map(|label| label.to_string()),
        multiple: options.multiple,
        directory,
        filters: if directory {
            Vec::new()
        } else {
            portal_filters(&options.filters)?
        },
    })
}

pub(super) fn save_request(
    directory: &Path,
    suggested_name: Option<&str>,
    filters: &[PathPromptFilter],
) -> Result<PortalSaveRequest> {
    Ok(PortalSaveRequest {
        title: "Save File",
        directory: directory.to_owned(),
        suggested_name: suggested_name.map(str::to_owned),
        filters: portal_filters(filters)?,
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

/// Runs an open prompt to completion.
pub(super) async fn prompt_for_paths(
    portal: &dyn FileChooserPortal,
    identifier: BoxFuture<'static, Option<WindowIdentifier>>,
    options: &PathPromptOptions,
) -> Result<Option<Vec<PathBuf>>> {
    let request = open_request(options)?;
    let response = portal
        .open_file(identifier.await, request)
        .await
        .map_err(failure_to_error)?;
    match response {
        PortalResponse::Cancelled => Ok(None),
        PortalResponse::Selected(uris) => local_selection(uris),
    }
}

/// Runs a save prompt to completion.
pub(super) async fn prompt_for_new_path(
    portal: &dyn FileChooserPortal,
    identifier: BoxFuture<'static, Option<WindowIdentifier>>,
    directory: &Path,
    suggested_name: Option<&str>,
    filters: &[PathPromptFilter],
) -> Result<Option<PathBuf>> {
    let request = save_request(directory, suggested_name, filters)?;
    let response = portal
        .save_file(identifier.await, request)
        .await
        .map_err(failure_to_error)?;
    match response {
        PortalResponse::Cancelled => Ok(None),
        PortalResponse::Selected(uris) => {
            Ok(local_selection(uris)?.and_then(|paths| paths.into_iter().next()))
        }
    }
}

/// The real portal, reached over the session bus.
pub(super) struct DesktopPortal;

#[cfg(any(feature = "wayland", feature = "x11"))]
fn file_filter(filter: &PortalFilter) -> ashpd::desktop::file_chooser::FileFilter {
    filter.globs.iter().fold(
        ashpd::desktop::file_chooser::FileFilter::new(&filter.name),
        |file_filter, glob| file_filter.glob(glob),
    )
}

#[cfg(any(feature = "wayland", feature = "x11"))]
fn portal_error(error: ashpd::Error) -> PortalFailure {
    match error {
        ashpd::Error::PortalNotFound(_) => PortalFailure::Missing,
        error => PortalFailure::Other(error.to_string()),
    }
}

/// Maps the portal's response: code 1 is a cancellation, other non-zero codes are failures.
#[cfg(any(feature = "wayland", feature = "x11"))]
fn portal_response(
    response: std::result::Result<ashpd::desktop::file_chooser::SelectedFiles, ashpd::Error>,
) -> PortalResult {
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

#[cfg(any(feature = "wayland", feature = "x11"))]
impl FileChooserPortal for DesktopPortal {
    fn open_file(
        &self,
        identifier: Option<WindowIdentifier>,
        request: PortalOpenRequest,
    ) -> BoxFuture<'static, PortalResult> {
        Box::pin(async move {
            let mut builder = ashpd::desktop::file_chooser::OpenFileRequest::default()
                .identifier(identifier)
                .modal(true)
                .title(request.title)
                .accept_label(request.accept_label.as_deref())
                .multiple(request.multiple)
                .directory(request.directory)
                .filters(request.filters.iter().map(file_filter));
            if let Some(first) = request.filters.first() {
                builder = builder.current_filter(file_filter(first));
            }
            let request = builder.send().await.map_err(portal_error)?;
            portal_response(request.response())
        })
    }

    fn save_file(
        &self,
        identifier: Option<WindowIdentifier>,
        request: PortalSaveRequest,
    ) -> BoxFuture<'static, PortalResult> {
        Box::pin(async move {
            let mut builder = ashpd::desktop::file_chooser::SaveFileRequest::default()
                .identifier(identifier)
                .modal(true)
                .title(request.title)
                .current_folder(&request.directory)
                .map_err(|error| {
                    PortalFailure::Other(format!(
                        "the start directory can't be passed to the portal: {error}"
                    ))
                })?
                .filters(request.filters.iter().map(file_filter));
            if let Some(name) = request.suggested_name.as_deref() {
                builder = builder.current_name(name);
            }
            if let Some(first) = request.filters.first() {
                builder = builder.current_filter(file_filter(first));
            }
            let request = builder.send().await.map_err(portal_error)?;
            portal_response(request.response())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use std::cell::RefCell;

    /// A portal that records requests and answers from a script.
    struct ScriptedPortal {
        answer: RefCell<Option<PortalResult>>,
        opens: RefCell<Vec<PortalOpenRequest>>,
        saves: RefCell<Vec<PortalSaveRequest>>,
    }

    impl ScriptedPortal {
        fn answering(answer: PortalResult) -> Self {
            Self {
                answer: RefCell::new(Some(answer)),
                opens: Default::default(),
                saves: Default::default(),
            }
        }
    }

    impl FileChooserPortal for ScriptedPortal {
        fn open_file(
            &self,
            _identifier: Option<WindowIdentifier>,
            request: PortalOpenRequest,
        ) -> BoxFuture<'static, PortalResult> {
            self.opens.borrow_mut().push(request);
            let answer = self.answer.borrow_mut().take().expect("one request");
            Box::pin(std::future::ready(answer))
        }

        fn save_file(
            &self,
            _identifier: Option<WindowIdentifier>,
            request: PortalSaveRequest,
        ) -> BoxFuture<'static, PortalResult> {
            self.saves.borrow_mut().push(request);
            let answer = self.answer.borrow_mut().take().expect("one request");
            Box::pin(std::future::ready(answer))
        }
    }

    fn options(files: bool, directories: bool) -> PathPromptOptions {
        PathPromptOptions {
            files,
            directories,
            multiple: false,
            prompt: None,
            filters: Vec::new(),
        }
    }

    fn no_identifier() -> BoxFuture<'static, Option<WindowIdentifier>> {
        Box::pin(std::future::ready(None))
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
    fn mixed_request_reaches_the_portal_as_a_directory_prompt() {
        let portal = ScriptedPortal::answering(Ok(PortalResponse::Cancelled));
        let result = block_on(prompt_for_paths(
            &portal,
            no_identifier(),
            &options(true, true),
        ));
        assert!(matches!(result, Ok(None)));
        let opens = portal.opens.borrow();
        assert_eq!(opens.len(), 1);
        assert!(opens[0].directory);
        assert_eq!(opens[0].title, "Open Folder");
    }

    #[test]
    fn accept_label_and_multiple_are_forwarded() {
        let portal = ScriptedPortal::answering(Ok(PortalResponse::Cancelled));
        let mut prompt = options(true, false);
        prompt.multiple = true;
        prompt.prompt = Some("Attach".into());
        block_on(prompt_for_paths(&portal, no_identifier(), &prompt)).unwrap();
        let opens = portal.opens.borrow();
        assert_eq!(opens[0].accept_label.as_deref(), Some("Attach"));
        assert!(opens[0].multiple);
    }

    #[test]
    fn missing_portal_is_a_readable_error() {
        let portal = ScriptedPortal::answering(Err(PortalFailure::Missing));
        let error = block_on(prompt_for_paths(
            &portal,
            no_identifier(),
            &options(true, false),
        ))
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
        let portal =
            ScriptedPortal::answering(Err(PortalFailure::Other("bus went away".to_owned())));
        let error = block_on(prompt_for_paths(
            &portal,
            no_identifier(),
            &options(true, false),
        ))
        .unwrap_err()
        .to_string();
        assert!(error.contains("bus went away"), "{error}");

        let portal = ScriptedPortal::answering(Ok(PortalResponse::Cancelled));
        let result = block_on(prompt_for_paths(
            &portal,
            no_identifier(),
            &options(true, false),
        ))
        .unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn non_file_uris_are_dropped_and_all_non_local_is_an_error() {
        let portal = ScriptedPortal::answering(Ok(PortalResponse::Selected(vec![
            "smb://server/share/file.txt".to_owned(),
            "file:///tmp/a%20b.txt".to_owned(),
        ])));
        let result = block_on(prompt_for_paths(
            &portal,
            no_identifier(),
            &options(true, false),
        ))
        .unwrap();
        assert_eq!(result, Some(vec![PathBuf::from("/tmp/a b.txt")]));

        let portal = ScriptedPortal::answering(Ok(PortalResponse::Selected(vec![
            "https://example.com/file.txt".to_owned(),
        ])));
        let error = block_on(prompt_for_paths(
            &portal,
            no_identifier(),
            &options(true, false),
        ))
        .unwrap_err()
        .to_string();
        assert!(error.contains("not a local file"), "{error}");
    }

    #[test]
    fn save_prompt_carries_directory_name_filters_and_title() {
        let portal = ScriptedPortal::answering(Ok(PortalResponse::Selected(vec![
            "file:///home/me/out.png".to_owned(),
        ])));
        let filters = [PathPromptFilter::new("PNG", ["png"])];
        let result = block_on(prompt_for_new_path(
            &portal,
            no_identifier(),
            Path::new("/home/me"),
            Some("out.png"),
            &filters,
        ))
        .unwrap();
        assert_eq!(result, Some(PathBuf::from("/home/me/out.png")));
        let saves = portal.saves.borrow();
        assert_eq!(saves[0].title, "Save File");
        assert_eq!(saves[0].directory, PathBuf::from("/home/me"));
        assert_eq!(saves[0].suggested_name.as_deref(), Some("out.png"));
        assert_eq!(saves[0].filters[0].globs, vec!["*.[pP][nN][gG]".to_owned()]);
    }

    #[test]
    fn save_with_a_non_local_location_is_an_error() {
        let portal = ScriptedPortal::answering(Ok(PortalResponse::Selected(vec![
            "ftp://host/out.png".to_owned(),
        ])));
        let error = block_on(prompt_for_new_path(
            &portal,
            no_identifier(),
            Path::new("/home/me"),
            None,
            &[],
        ))
        .unwrap_err()
        .to_string();
        assert!(error.contains("not a local file"), "{error}");
    }
}
