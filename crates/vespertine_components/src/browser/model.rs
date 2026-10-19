//! Pure address handling and scheme policy of the browser surface (CMP-022 R3 to R6).
//!
//! Nothing here depends on the framework: the functions take and return text so that the rules can
//! be tested exhaustively. The helper enforces the same allow-list on the engine's navigation
//! decisions (PLT-014 R10); these functions are the application-side mirror of it.

use std::net::{Ipv4Addr, Ipv6Addr};
use url::Url;

pub use vespertine_webview::{PageState, Presentation};

/// Validation text of an empty address (R3 step 1).
pub const ERROR_EMPTY: &str = "Enter a website or localhost address.";
/// Validation text of an address with a control character (R3 step 2).
pub const ERROR_CONTROL: &str = "This address contains invalid characters.";
/// Validation text of an address that cannot be parsed (R3 step 4).
pub const ERROR_INVALID: &str = "Enter a valid website or localhost address.";
/// Validation text of an unsupported scheme or a missing host (R3 step 5).
pub const ERROR_SCHEME: &str = "Only http and https addresses are supported.";
/// Validation text of an address with embedded credentials (R3 step 6).
pub const ERROR_CREDENTIALS: &str = "Use an address without an embedded username or password.";
/// Error text of a link without an explicit `http://` or `https://` prefix (R4).
pub const ERROR_LINK_PREFIX: &str = "Only explicit http and https links are supported.";
/// Error text of a link with an invalid address (R4).
pub const ERROR_LINK_ADDRESS: &str = "This link contains an invalid address.";
/// Error text of a link with a bad percent escape (R4).
pub const ERROR_LINK_ESCAPE: &str = "This link contains an invalid escape.";

/// Whether `host` (a host name or address, IPv6 optionally in brackets) is a loopback host: the
/// domain `localhost` or any name ending in `.localhost`, an IPv4 loopback address or the IPv6
/// loopback address (R3).
pub fn loopback(host: &str) -> bool {
    let host = host.trim();
    let host = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);
    let host = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    if let Ok(address) = host.parse::<Ipv4Addr>() {
        return address.is_loopback();
    }
    if let Ok(address) = host.parse::<Ipv6Addr>() {
        return address.is_loopback();
    }
    false
}

/// Whether the text up to the first `/`, `?` or `#` ends in a colon followed by digits only.
fn has_numeric_port(text: &str) -> bool {
    let authority = text.split(['/', '?', '#']).next().unwrap_or(text);
    match authority.rsplit_once(':') {
        Some((_, port)) => !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()),
        None => false,
    }
}

/// Whether typed text names its scheme itself (R3 step 3).
fn explicit_address(text: &str) -> bool {
    text.contains("://")
        || (text.contains(':') && !has_numeric_port(text) && !text.starts_with('['))
}

/// Checks scheme, host and credentials of a parsed address (R3 steps 5 and 6).
fn check_parsed(parsed: &Url) -> Result<(), String> {
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host().is_none() {
        return Err(ERROR_SCHEME.to_owned());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(ERROR_CREDENTIALS.to_owned());
    }
    Ok(())
}

/// Turns user-typed text into a navigable address (R3).
///
/// Steps: trim; reject empty text and control characters; decide whether the text is an explicit
/// address; parse it (or `https://` followed by it); require an http(s) scheme with a host and no
/// credentials; give a loopback host the scheme `http` when the scheme was inferred. The result is
/// the canonical (WHATWG) serialization. Errors are the exact validation texts of the spec.
pub fn normalize_address(input: &str) -> Result<String, String> {
    let text = input.trim();
    if text.is_empty() {
        return Err(ERROR_EMPTY.to_owned());
    }
    if text.chars().any(char::is_control) {
        return Err(ERROR_CONTROL.to_owned());
    }
    let explicit = explicit_address(text);
    let parsed = if explicit {
        Url::parse(text)
    } else {
        Url::parse(&format!("https://{text}"))
    }
    .map_err(|_| ERROR_INVALID.to_owned())?;
    check_parsed(&parsed)?;
    if !explicit && parsed.host_str().is_some_and(loopback) {
        // The inferred scheme becomes http. Parsing again (instead of switching the scheme)
        // keeps an explicitly typed port that happens to be the default of https.
        let local = Url::parse(&format!("http://{text}")).map_err(|_| ERROR_INVALID.to_owned())?;
        check_parsed(&local)?;
        return Ok(local.into());
    }
    Ok(parsed.into())
}

/// The strict variant for links coming from documents (R4): the text must start with `http://` or
/// `https://` (any case), have a non-empty authority, contain no control, whitespace or backslash
/// character and only well-formed percent escapes. It never infers a scheme and never strips
/// characters; valid text is then normalized by [`normalize_address`].
pub fn normalize_explicit_address(text: &str) -> Result<String, String> {
    let starts_with = |prefix: &str| {
        text.get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
    };
    let prefix_length = if starts_with("http://") {
        "http://".len()
    } else if starts_with("https://") {
        "https://".len()
    } else {
        return Err(ERROR_LINK_PREFIX.to_owned());
    };
    let rest = &text[prefix_length..];
    if rest.is_empty()
        || rest.starts_with(['/', '?', '#'])
        || text
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return Err(ERROR_LINK_ADDRESS.to_owned());
    }
    let bytes = text.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'%'
            && !(bytes.get(index + 1).is_some_and(u8::is_ascii_hexdigit)
                && bytes.get(index + 2).is_some_and(u8::is_ascii_hexdigit))
        {
            return Err(ERROR_LINK_ESCAPE.to_owned());
        }
    }
    normalize_address(text)
}

/// Whether `address` may be navigated to at the top level (R5): it parses, its scheme is http or
/// https, it has a host and it has neither a user name nor a password.
pub fn allowed_navigation(address: &str) -> bool {
    Url::parse(address).is_ok_and(|parsed| check_parsed(&parsed).is_ok())
}

/// Whether `address` may be navigated to in a sub-frame (R5): allowed navigation, or an address
/// whose scheme is `about`, `data` or `blob`.
pub fn allowed_frame_navigation(address: &str) -> bool {
    allowed_navigation(address)
        || Url::parse(address)
            .is_ok_and(|parsed| matches!(parsed.scheme(), "about" | "data" | "blob"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_addresses_are_normalized_by_the_accepted_vectors() {
        for (input, expected) in [
            ("localhost:3000", "http://localhost:3000/"),
            ("127.0.0.1:5173/a?x=1", "http://127.0.0.1:5173/a?x=1"),
            ("[::1]:8080", "http://[::1]:8080/"),
            ("[::1]", "http://[::1]/"),
            (" app.localhost:3000 ", "http://app.localhost:3000/"),
            ("example.com/path", "https://example.com/path"),
            ("https://localhost:3000", "https://localhost:3000/"),
            ("http://example.com", "http://example.com/"),
            ("EXAMPLE.com", "https://example.com/"),
        ] {
            assert_eq!(
                normalize_address(input).as_deref(),
                Ok(expected),
                "{input:?}"
            );
        }
    }

    #[test]
    fn typed_addresses_are_rejected_with_the_exact_messages() {
        for (input, message) in [
            ("", ERROR_EMPTY),
            ("   ", ERROR_EMPTY),
            ("javascript:alert(1)", ERROR_SCHEME),
            ("file:///tmp/a", ERROR_SCHEME),
            ("data:text/html,hi", ERROR_SCHEME),
            ("myapp://open/thing", ERROR_SCHEME),
            ("https://user:pass@example.com", ERROR_CREDENTIALS),
            ("https://user@example.com", ERROR_CREDENTIALS),
            ("https://", ERROR_INVALID),
            ("two words", ERROR_INVALID),
            ("https://example.com/\nfoo", ERROR_CONTROL),
            ("exa\tmple.com", ERROR_CONTROL),
        ] {
            assert_eq!(
                normalize_address(input),
                Err(message.to_owned()),
                "{input:?}"
            );
        }
    }

    #[test]
    fn an_inferred_loopback_scheme_keeps_the_typed_port() {
        assert_eq!(
            normalize_address("localhost:443").as_deref(),
            Ok("http://localhost:443/")
        );
        assert_eq!(
            normalize_address("localhost:80").as_deref(),
            Ok("http://localhost/")
        );
        assert_eq!(
            normalize_address("https://127.0.0.1").as_deref(),
            Ok("https://127.0.0.1/")
        );
    }

    #[test]
    fn loopback_hosts_are_recognized() {
        for host in [
            "localhost",
            "LOCALHOST",
            "a.b.localhost",
            "127.0.0.1",
            "127.1.2.3",
            "::1",
            "[::1]",
        ] {
            assert!(loopback(host), "{host}");
        }
        for host in [
            "example.com",
            "localhost.example.com",
            "128.0.0.1",
            "::2",
            "notlocalhost",
        ] {
            assert!(!loopback(host), "{host}");
        }
    }

    #[test]
    fn explicit_links_follow_the_strict_rules() {
        assert_eq!(
            normalize_explicit_address("https://example.com/a%20b").as_deref(),
            Ok("https://example.com/a%20b")
        );
        assert_eq!(
            normalize_explicit_address("HTTP://Example.com").as_deref(),
            Ok("http://example.com/")
        );
        for (input, message) in [
            ("example.com", ERROR_LINK_PREFIX),
            ("", ERROR_LINK_PREFIX),
            ("javascript:alert(1)", ERROR_LINK_PREFIX),
            ("https://", ERROR_LINK_ADDRESS),
            ("https:///path", ERROR_LINK_ADDRESS),
            ("https://a/b c", ERROR_LINK_ADDRESS),
            ("https://a\\b", ERROR_LINK_ADDRESS),
            ("https://a/b\u{7}", ERROR_LINK_ADDRESS),
            (" https://a", ERROR_LINK_PREFIX),
            ("https://a/%zz", ERROR_LINK_ESCAPE),
            ("https://a/%4", ERROR_LINK_ESCAPE),
        ] {
            assert_eq!(
                normalize_explicit_address(input),
                Err(message.to_owned()),
                "{input:?}"
            );
        }
        // Strict links never infer a loopback scheme.
        assert_eq!(
            normalize_explicit_address("https://localhost:3000").as_deref(),
            Ok("https://localhost:3000/")
        );
    }

    #[test]
    fn the_navigation_allow_lists_follow_the_policy() {
        for address in ["http://example.com/", "https://localhost:3000/a?b#c"] {
            assert!(allowed_navigation(address), "{address}");
        }
        for address in [
            "javascript:alert(1)",
            "https://user@example.com/",
            "https://user:pw@example.com/",
            "file:///tmp/a",
            "mailto:a@example.com",
            "about:blank",
            "",
            "not a url",
        ] {
            assert!(!allowed_navigation(address), "{address}");
        }
        for address in [
            "https://example.com/embed",
            "about:blank",
            "about:srcdoc",
            "data:text/html,hi",
            "blob:https://example.com/0f3e",
        ] {
            assert!(allowed_frame_navigation(address), "{address}");
        }
        for address in [
            "ms-settings:privacy",
            "file:///C:/Windows/win.ini",
            "javascript:alert(1)",
            "mailto:a@example.com",
            "",
        ] {
            assert!(!allowed_frame_navigation(address), "{address}");
        }
    }

    #[test]
    fn page_labels_fall_back_from_title_to_host_to_browser() {
        let mut state = PageState::default();
        assert_eq!(state.label(), "Browser");
        state.url = Some("http://localhost:3000/path".into());
        assert_eq!(state.label(), "localhost");
        state.title = "Local preview".into();
        assert_eq!(state.label(), "Local preview");
        state.title = "   ".into();
        assert_eq!(state.label(), "localhost");
    }

    #[test]
    fn presentation_follows_the_host_state() {
        assert_eq!(Presentation::for_host(false, false), Presentation::Hidden);
        assert_eq!(Presentation::for_host(false, true), Presentation::Hidden);
        assert_eq!(
            Presentation::for_host(true, true),
            Presentation::Passthrough
        );
        assert_eq!(Presentation::for_host(true, false), Presentation::Live);
    }
}
