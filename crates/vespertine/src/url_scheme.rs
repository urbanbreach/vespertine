use anyhow::{Result, bail};

/// Validates a custom URL scheme name against RFC 3986 and lowercases it.
///
/// A scheme is an ASCII letter followed by any number of ASCII letters, digits, `+`, `-` or
/// `.`. Anything else is rejected with a readable error; nothing is trimmed or repaired.
pub fn normalize_url_scheme(scheme: &str) -> Result<String> {
    let mut characters = scheme.chars();
    let Some(first) = characters.next() else {
        bail!("invalid URL scheme: the name is empty");
    };
    if !first.is_ascii_alphabetic() {
        bail!("invalid URL scheme {scheme:?}: it must start with an ASCII letter");
    }
    if let Some(invalid) =
        characters.find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
    {
        bail!(
            "invalid URL scheme {scheme:?}: {invalid:?} is not allowed (use letters, digits, '+', '-' or '.')"
        );
    }
    Ok(scheme.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::normalize_url_scheme;

    #[test]
    fn valid_schemes_are_lowercased() {
        assert_eq!(normalize_url_scheme("MyApp").unwrap(), "myapp");
        assert_eq!(normalize_url_scheme("x-y+z.1").unwrap(), "x-y+z.1");
    }

    #[test]
    fn invalid_schemes_are_rejected_with_a_message() {
        for invalid in [
            "", "1app", "-app", "my app", "my_app", "app:", "app://", "é",
        ] {
            let error = normalize_url_scheme(invalid).unwrap_err().to_string();
            assert!(error.contains("invalid URL scheme"), "{invalid:?}: {error}");
        }
    }

    #[crate::test]
    async fn app_registration_with_an_invalid_name_errors_without_reaching_the_platform(
        cx: &mut crate::TestAppContext,
    ) {
        for invalid in ["1bad", "has space", "", "a_b"] {
            let error = cx
                .update(|cx| cx.register_url_scheme(invalid))
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains("invalid URL scheme"), "{invalid:?}: {error}");
        }
        assert!(cx.registered_url_schemes().is_empty());
    }

    #[crate::test]
    async fn app_registration_lowercases_before_reaching_the_platform(
        cx: &mut crate::TestAppContext,
    ) {
        cx.update(|cx| cx.register_url_scheme("MyApp"))
            .await
            .unwrap();
        assert_eq!(cx.registered_url_schemes(), vec!["myapp".to_owned()]);
    }

    #[crate::test]
    fn open_and_reveal_hand_the_platform_the_original_strings(cx: &mut crate::TestAppContext) {
        let url = "HTTPS://Example.COM/a%20b?q=1#frag";
        cx.update(|cx| {
            cx.open_url(url);
            cx.reveal_path(std::path::Path::new("/tmp/a b/notes.txt"));
            cx.open_with_system(std::path::Path::new("/tmp/a b/notes.txt"));
        });
        assert_eq!(cx.opened_urls(), vec![url.to_owned()]);
        assert_eq!(
            cx.revealed_paths(),
            vec![std::path::PathBuf::from("/tmp/a b/notes.txt")]
        );
        assert_eq!(
            cx.opened_paths(),
            vec![std::path::PathBuf::from("/tmp/a b/notes.txt")]
        );
    }
}
