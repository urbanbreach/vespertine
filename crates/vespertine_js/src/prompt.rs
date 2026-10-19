//! Path prompt options and results (APP-005 R8).

use std::path::PathBuf;

use serde::Deserialize;
use vespertine::PathPromptOptions;

use crate::error::HostError;

/// Prompt options as received from JavaScript.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct PromptConfig {
    pub files: Option<bool>,
    pub directories: Option<bool>,
    pub multiple: Option<bool>,
    /// Label of the confirmation button.
    pub prompt: Option<String>,
}

impl PromptConfig {
    /// Applies the defaults (`directories` false, `multiple` false, `files` true unless
    /// `directories` is true) and the validation rules. `mixed_supported` is whether the
    /// platform chooser can select files and directories together (false on Linux).
    pub fn validate(&self, mixed_supported: bool) -> Result<PathPromptOptions, HostError> {
        let directories = self.directories.unwrap_or(false);
        let files = self.files.unwrap_or(!directories);
        if !files && !directories {
            return Err(HostError::Invalid(
                "promptForPaths requires files or directories".into(),
            ));
        }
        if files && directories && !mixed_supported {
            return Err(HostError::Invalid(
                "promptForPaths cannot select files and directories together on this platform"
                    .into(),
            ));
        }
        Ok(PathPromptOptions {
            files,
            directories,
            multiple: self.multiple.unwrap_or(false),
            prompt: self.prompt.clone().map(Into::into),
        })
    }
}

/// Converts the chooser result: an empty selection is a cancellation (`None`), and a path that
/// is not valid Unicode rejects instead of being returned lossily.
pub fn paths_to_strings(selection: Option<Vec<PathBuf>>) -> Result<Option<Vec<String>>, HostError> {
    let Some(paths) = selection else {
        return Ok(None);
    };
    if paths.is_empty() {
        return Ok(None);
    }
    paths
        .into_iter()
        .map(|path| {
            path.into_os_string().into_string().map_err(|path| {
                HostError::Failed(format!(
                    "selected path is not valid Unicode: {}",
                    path.to_string_lossy()
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_select_a_single_file() {
        let options = PromptConfig::default().validate(false).unwrap();
        assert!(options.files && !options.directories && !options.multiple);
        assert!(options.prompt.is_none());
    }

    #[test]
    fn directories_only_turns_files_off_by_default() {
        let config = PromptConfig {
            directories: Some(true),
            ..Default::default()
        };
        let options = config.validate(false).unwrap();
        assert!(!options.files && options.directories);
    }

    #[test]
    fn explicitly_disabling_both_rejects() {
        let config = PromptConfig {
            files: Some(false),
            directories: Some(false),
            ..Default::default()
        };
        let error = config.validate(true).unwrap_err();
        assert!(error.to_string().contains("requires files or directories"));
        let only_files_false = PromptConfig {
            files: Some(false),
            ..Default::default()
        };
        assert!(only_files_false.validate(true).is_err());
    }

    #[test]
    fn both_enabled_rejects_where_mixed_selection_is_unsupported() {
        let config = PromptConfig {
            files: Some(true),
            directories: Some(true),
            ..Default::default()
        };
        assert!(config.validate(false).is_err());
        assert!(config.validate(true).is_ok());
    }

    #[test]
    fn multiple_and_prompt_label_are_forwarded() {
        let config: PromptConfig =
            serde_json::from_str(r#"{"multiple": true, "prompt": "Pick"}"#).unwrap();
        let options = config.validate(false).unwrap();
        assert!(options.multiple);
        assert_eq!(options.prompt.as_deref(), Some("Pick"));
    }

    #[test]
    fn empty_selection_and_cancellation_resolve_to_none() {
        assert_eq!(paths_to_strings(None), Ok(None));
        assert_eq!(paths_to_strings(Some(vec![])), Ok(None));
    }

    #[test]
    fn selected_paths_are_returned_as_strings() {
        let selected = Some(vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]);
        assert_eq!(
            paths_to_strings(selected),
            Ok(Some(vec!["/tmp/a".to_string(), "/tmp/b".to_string()]))
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_path_that_is_not_valid_unicode_rejects() {
        use std::os::unix::ffi::OsStringExt;
        let bad = PathBuf::from(std::ffi::OsString::from_vec(vec![b'/', 0xff, 0xfe]));
        let error = paths_to_strings(Some(vec![PathBuf::from("/ok"), bad])).unwrap_err();
        assert!(error.to_string().contains("not valid Unicode"));
    }
}
