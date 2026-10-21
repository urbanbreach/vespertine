//! Link destinations: validated against a caller policy before any route is chosen.
//!
//! A [`LinkTarget`] keeps the display label, the destination exactly as written and the result
//! of validating it once when the target was created. [`LinkRoutes`] classifies a destination
//! into a route (web, mail, file) together with its normalized navigation address.

use crate::{App, ClipboardItem, SharedString};
use std::{
    fmt,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

/// Release must happen within this distance of the press for a pointer click to activate a link.
pub const LINK_CLICK_SLOP: f64 = 4.0;

/// Disclosed destinations are broken into lines of at most this many characters (display only).
pub const LINK_DISCLOSURE_LINE_CHARS: usize = 64;

const UNSUPPORTED_SCHEME: &str = "only explicit http/https links are supported";
const INVALID_ADDRESS: &str = "invalid address";
const INVALID_ESCAPE: &str = "invalid escape";
const CREDENTIALS_NOT_ALLOWED: &str = "credentials in addresses are not allowed";

/// The validated navigation of a destination: a normalized address, or the reason it was
/// rejected.
pub type LinkNavigation = Result<SharedString, SharedString>;

/// A caller-supplied destination policy: original text in, normalized address or rejection
/// reason out. The default accepts only explicit `http` and `https` destinations.
#[derive(Clone)]
pub struct LinkPolicy(Arc<dyn Fn(&str) -> LinkNavigation + Send + Sync>);

impl LinkPolicy {
    /// Wraps a validation function.
    pub fn new(validate: impl Fn(&str) -> LinkNavigation + Send + Sync + 'static) -> Self {
        Self(Arc::new(validate))
    }

    /// Validates `original` with this policy.
    pub fn validate(&self, original: &str) -> LinkNavigation {
        (self.0)(original)
    }
}

impl Default for LinkPolicy {
    fn default() -> Self {
        Self::new(validate_web_destination)
    }
}

impl fmt::Debug for LinkPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LinkPolicy")
    }
}

/// The default destination policy.
///
/// Accepts only explicit `http`/`https` destinations (the scheme is matched case-insensitively)
/// and normalizes them with URL parsing. Rejects control and whitespace characters anywhere, a
/// backslash, an empty authority, a `%` that does not start a two-digit hexadecimal escape,
/// embedded credentials, a missing host and every other scheme, including scheme-less text. It
/// never guesses a scheme and never strips characters.
pub fn validate_web_destination(original: &str) -> LinkNavigation {
    let invalid = |reason: &'static str| Err(SharedString::from(reason));

    if original
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return invalid(INVALID_ADDRESS);
    }

    let Some((scheme, rest)) = original.split_once(':') else {
        return invalid(UNSUPPORTED_SCHEME);
    };
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")) {
        return invalid(UNSUPPORTED_SCHEME);
    }

    let Some(after_slashes) = rest.strip_prefix("//") else {
        return invalid(INVALID_ADDRESS);
    };
    let authority_end = after_slashes
        .find(['/', '?', '#'])
        .unwrap_or(after_slashes.len());
    let authority = &after_slashes[..authority_end];
    if authority.is_empty() {
        return invalid(INVALID_ADDRESS);
    }

    if has_invalid_percent_escape(original) {
        return invalid(INVALID_ESCAPE);
    }
    if authority.contains('@') {
        return invalid(CREDENTIALS_NOT_ALLOWED);
    }

    let Ok(url) = url::Url::parse(original) else {
        return invalid(INVALID_ADDRESS);
    };
    if !url.username().is_empty() || url.password().is_some() {
        return invalid(CREDENTIALS_NOT_ALLOWED);
    }
    if url.host_str().is_none_or(str::is_empty) {
        return invalid(INVALID_ADDRESS);
    }
    Ok(SharedString::from(url.as_str().to_owned()))
}

fn has_invalid_percent_escape(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.iter().enumerate().any(|(ix, byte)| {
        *byte == b'%'
            && !(bytes.get(ix + 1).is_some_and(u8::is_ascii_hexdigit)
                && bytes.get(ix + 2).is_some_and(u8::is_ascii_hexdigit))
    })
}

/// Which route a validated destination belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkRoute {
    /// A web address: embedded browser or system browser.
    Web,
    /// A `mailto:` address, opened only with the mail handler.
    Mail,
    /// A local file, with its resolved absolute path.
    File {
        /// The resolved path.
        path: PathBuf,
    },
}

/// A richer destination policy with optional mail and file routes next to the web policy.
#[derive(Clone, Debug, Default)]
pub struct LinkRoutes {
    web: LinkPolicy,
    mail: bool,
    file_roots: Option<Vec<PathBuf>>,
}

impl LinkRoutes {
    /// Web destinations only, with the default web policy.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the web policy.
    pub fn web_policy(mut self, policy: LinkPolicy) -> Self {
        self.web = policy;
        self
    }

    /// Accepts `mailto:` destinations on the mail route (and nowhere else).
    pub fn with_mail(mut self) -> Self {
        self.mail = true;
        self
    }

    /// Accepts absolute local paths on the file route. Relative paths are resolved against the
    /// first of `roots` that contains them; a relative path that climbs out of its root with
    /// `..` or exists in none of the roots is rejected.
    pub fn with_files(mut self, roots: impl IntoIterator<Item = PathBuf>) -> Self {
        self.file_roots = Some(roots.into_iter().collect());
        self
    }

    /// Validates `original` and picks its route.
    pub fn classify(&self, original: &str) -> (LinkNavigation, LinkRoute) {
        if has_scheme(original) {
            if self.mail && starts_with_ignore_case(original, "mailto:") {
                return (validate_mailto(original), LinkRoute::Mail);
            }
            return (self.web.validate(original), LinkRoute::Web);
        }
        if let Some(roots) = &self.file_roots {
            match resolve_file(original, roots) {
                FileResolution::Resolved(path) => {
                    let navigation = url::Url::from_file_path(&path)
                        .map(|url| SharedString::from(url.as_str().to_owned()))
                        .map_err(|()| SharedString::from("invalid file path"));
                    return (navigation, LinkRoute::File { path });
                }
                FileResolution::Rejected(reason) => {
                    return (Err(SharedString::from(reason)), LinkRoute::Web);
                }
                FileResolution::NotAPath => {}
            }
        }
        (self.web.validate(original), LinkRoute::Web)
    }
}

fn has_scheme(text: &str) -> bool {
    let Some((scheme, _)) = text.split_once(':') else {
        return false;
    };
    let mut characters = scheme.chars();
    characters.next().is_some_and(|c| c.is_ascii_alphabetic())
        && characters.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

fn validate_mailto(original: &str) -> LinkNavigation {
    if original
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return Err(INVALID_ADDRESS.into());
    }
    if has_invalid_percent_escape(original) {
        return Err(INVALID_ESCAPE.into());
    }
    match url::Url::parse(original) {
        Ok(url) if url.scheme() == "mailto" && !url.path().is_empty() => {
            Ok(url.as_str().to_owned().into())
        }
        _ => Err(INVALID_ADDRESS.into()),
    }
}

enum FileResolution {
    Resolved(PathBuf),
    Rejected(&'static str),
    NotAPath,
}

fn resolve_file(original: &str, roots: &[PathBuf]) -> FileResolution {
    if original.is_empty()
        || original
            .chars()
            .any(|c| c.is_control() && c != '\t' || c == '\\')
    {
        return FileResolution::NotAPath;
    }
    let path = Path::new(original);
    if path.is_absolute() {
        return FileResolution::Resolved(path.to_owned());
    }
    if roots.is_empty() {
        return FileResolution::NotAPath;
    }
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return FileResolution::Rejected("file path leaves its root");
    }
    roots
        .iter()
        .map(|root| root.join(path))
        .find(|candidate| candidate.exists())
        .map_or(FileResolution::NotAPath, FileResolution::Resolved)
}

/// A link destination: label, original text and the validation result, computed once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkTarget {
    label: SharedString,
    original: SharedString,
    navigation: LinkNavigation,
    route: LinkRoute,
}

impl LinkTarget {
    /// Validates `original` with `policy`; the original text is never altered.
    pub fn new(
        label: impl Into<SharedString>,
        original: impl Into<SharedString>,
        policy: &LinkPolicy,
    ) -> Self {
        let original = original.into();
        Self {
            label: label.into(),
            navigation: policy.validate(&original),
            original,
            route: LinkRoute::Web,
        }
    }

    /// Validates `original` with the default policy.
    pub fn with_default_policy(
        label: impl Into<SharedString>,
        original: impl Into<SharedString>,
    ) -> Self {
        Self::new(label, original, &LinkPolicy::default())
    }

    /// Validates `original` with a richer policy that may add the mail and file routes.
    pub fn with_routes(
        label: impl Into<SharedString>,
        original: impl Into<SharedString>,
        routes: &LinkRoutes,
    ) -> Self {
        let original = original.into();
        let (navigation, route) = routes.classify(&original);
        Self {
            label: label.into(),
            original,
            navigation,
            route,
        }
    }

    /// The text shown for the link. It can say anything, so it is never used to navigate.
    pub fn label(&self) -> &SharedString {
        &self.label
    }

    /// The destination exactly as written.
    pub fn original(&self) -> &SharedString {
        &self.original
    }

    /// The normalized address, or the reason the destination was rejected.
    pub fn navigation(&self) -> &LinkNavigation {
        &self.navigation
    }

    /// The route the destination belongs to.
    pub fn route(&self) -> &LinkRoute {
        &self.route
    }

    /// The resolved local path of a file-route target.
    pub fn file_path(&self) -> Option<&Path> {
        match &self.route {
            LinkRoute::File { path } => Some(path),
            _ => None,
        }
    }

    /// Writes the original destination to the clipboard, even when navigation was rejected.
    pub fn copy_original(&self, cx: &mut App) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.original.to_string()));
    }

    /// The actions a menu for this target should offer on `surface`.
    pub fn actions(&self, surface: &LinkSurface) -> Vec<LinkAction> {
        if self.navigation.is_err() {
            return vec![LinkAction::Copy];
        }
        let mut actions = vec![LinkAction::Primary];
        match &self.route {
            LinkRoute::Web => {
                if surface.embedded {
                    actions.push(LinkAction::Internal);
                }
                actions.push(LinkAction::External);
            }
            LinkRoute::Mail => actions.push(LinkAction::Mail),
            LinkRoute::File { .. } => {
                actions.extend([LinkAction::OpenFile, LinkAction::RevealFile]);
            }
        }
        actions.push(LinkAction::Copy);
        if self.file_path().is_some() {
            actions.push(LinkAction::CopyPath);
        }
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_target(original: &str) -> LinkTarget {
        LinkTarget::with_default_policy("label", original.to_owned())
    }

    #[test]
    fn default_policy_rejects_every_unsafe_destination() {
        let rejected = [
            "https://",
            "javascript:alert(1)",
            "example.com",
            "data:text/plain,hi",
            "https:///path",
            "mailto:a@b.com",
            "\nhttps://example.com",
            "https://user:pass@example.com",
            "https://example.com/%GG",
            "https://exam\nple.com",
        ];
        for original in rejected {
            let target = default_target(original);
            assert!(target.navigation().is_err(), "{original:?} was accepted");
            assert_eq!(target.original().as_ref(), original);
            assert_eq!(target.route(), &LinkRoute::Web);
        }
    }

    #[test]
    fn default_policy_accepts_and_normalizes_http_and_https() {
        let target = default_target("HTTPS://EXAMPLE.COM");
        assert_eq!(
            target.navigation().as_ref().map(|a| a.as_ref()),
            Ok("https://example.com/")
        );
        assert_eq!(target.original().as_ref(), "HTTPS://EXAMPLE.COM");
        assert_eq!(
            default_target("http://Example.com:8080/a?b#c")
                .navigation()
                .as_ref()
                .map(|a| a.as_ref()),
            Ok("http://example.com:8080/a?b#c")
        );
    }

    #[test]
    fn rejection_reasons_are_short_and_readable() {
        let reason = |original: &str| default_target(original).navigation().clone().unwrap_err();
        assert_eq!(
            reason("javascript:alert(1)").as_ref(),
            "only explicit http/https links are supported"
        );
        assert_eq!(
            reason("example.com").as_ref(),
            "only explicit http/https links are supported"
        );
        assert_eq!(reason("https://").as_ref(), "invalid address");
        assert_eq!(reason("https://a b").as_ref(), "invalid address");
        assert_eq!(reason("https://a.com/%GG").as_ref(), "invalid escape");
        assert_eq!(reason("https://a.com/%4").as_ref(), "invalid escape");
        assert_eq!(reason("https://a.com\\b").as_ref(), "invalid address");
    }

    #[test]
    fn caller_policy_replaces_the_default() {
        let policy = LinkPolicy::new(|original| {
            original
                .strip_prefix("app:")
                .map(|rest| SharedString::from(format!("app://{rest}")))
                .ok_or_else(|| SharedString::from("not an app link"))
        });
        let target = LinkTarget::new("x", "app:home", &policy);
        assert_eq!(
            target.navigation().as_ref().map(|a| a.as_ref()),
            Ok("app://home")
        );
        assert!(
            LinkTarget::new("x", "https://example.com", &policy)
                .navigation()
                .is_err()
        );
    }

    #[test]
    fn mail_and_file_routes_are_separate_from_web_routes() {
        let routes = LinkRoutes::new()
            .with_mail()
            .with_files([PathBuf::from("/srv/docs")]);

        let mail = LinkTarget::with_routes("mail", "MAILTO:a@b.com", &routes);
        assert_eq!(mail.route(), &LinkRoute::Mail);
        assert_eq!(
            mail.navigation().as_ref().map(|a| a.as_ref()),
            Ok("mailto:a@b.com")
        );
        // Without the mail route a mailto: address is rejected like any other scheme.
        assert!(
            LinkTarget::with_routes("m", "mailto:a@b.com", &LinkRoutes::new())
                .navigation()
                .is_err()
        );

        let file = LinkTarget::with_routes("file", "/my dir/notes.txt", &routes);
        assert_eq!(file.file_path(), Some(Path::new("/my dir/notes.txt")));
        assert_eq!(
            file.navigation().as_ref().map(|a| a.as_ref()),
            Ok("file:///my%20dir/notes.txt")
        );
        // A file target is never a web target.
        assert!(default_target("/my dir/notes.txt").navigation().is_err());
    }

    #[test]
    fn relative_paths_resolve_against_roots_and_cannot_escape() {
        let root = std::env::temp_dir().join(format!("vespertine-link-{}", std::process::id()));
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("docs/a.md"), "x").unwrap();
        let routes = LinkRoutes::new().with_files([root.clone()]);

        let found = LinkTarget::with_routes("a", "docs/a.md", &routes);
        assert_eq!(found.file_path(), Some(root.join("docs/a.md").as_path()));
        let escaped = LinkTarget::with_routes("a", "../etc/passwd", &routes);
        assert!(escaped.navigation().is_err());
        // Text that is neither a file nor a web link falls through to the web policy.
        let missing = LinkTarget::with_routes("a", "docs/missing.md", &routes);
        assert!(missing.navigation().is_err() && missing.file_path().is_none());
        std::fs::remove_dir_all(root).ok();
    }

    #[crate::test]
    fn copy_writes_the_exact_original_even_when_rejected_or_misleading(
        cx: &mut crate::TestAppContext,
    ) {
        let misleading =
            LinkTarget::with_default_policy("https://bank.example", "javascript:alert(1)");
        cx.update(|cx| misleading.copy_original(cx));
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("javascript:alert(1)")
        );

        let odd = LinkTarget::with_default_policy("x", "  HTTPS://Example.COM/%zz \n");
        cx.update(|cx| odd.copy_original(cx));
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("  HTTPS://Example.COM/%zz \n")
        );
    }
}
