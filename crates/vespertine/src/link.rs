//! Links: destinations validated against a caller policy before any route is chosen.
//!
//! A [`LinkTarget`] keeps the display label, the destination exactly as written and the result
//! of validating it once when the target was created. A [`LinkSurface`] chooses among the routes
//! (embedded browser, system handler, mail handler, file actions) and never routes a rejected
//! target. [`Link`] is the focusable inline link element built on top of them.

use crate::{
    App, ClickEvent, ClipboardItem, CursorStyle, ElementId, FocusHandle, InteractiveElement as _,
    IntoElement, KeyDownEvent, MouseButton, ParentElement as _, Pixels, Point, Refineable as _,
    RenderOnce, Role, SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled,
    Window, deferred, div, rgb,
};
use std::{
    any::Any,
    fmt,
    path::{Component, Path, PathBuf},
    rc::Rc,
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

/// What a user can do with a link.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LinkAction {
    /// The surface's preferred route: internal or external.
    Primary,
    /// Copy the original destination to the clipboard.
    Copy,
    /// Copy the resolved path of a local file.
    CopyPath,
    /// The embedded browser surface, when the surface has one.
    Internal,
    /// The system default handler.
    External,
    /// Open a `mailto:` destination with the mail handler.
    Mail,
    /// Reveal a local file in the file manager.
    RevealFile,
    /// Open a local file with its default application.
    OpenFile,
}

/// The outcome of routing a target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkOutcome {
    /// Navigate the surface's embedded browser to this address.
    Internal(SharedString),
    /// Hand this address to the system default handler.
    External(SharedString),
    /// The destination was rejected; nothing is routed.
    Rejected(SharedString),
}

/// Which route [`LinkAction::Primary`] takes on a surface that has an embedded browser.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LinkPreference {
    /// Open in the embedded browser.
    Internal,
    /// Open with the system default handler.
    #[default]
    External,
}

/// How an owning surface routes links.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinkSurface {
    /// Whether the surface has an embedded browser.
    pub embedded: bool,
    /// The route the primary action takes when the surface is embedded.
    pub preference: LinkPreference,
}

impl LinkSurface {
    /// Routes `action` on `target`.
    ///
    /// Returns `None` for actions that do not navigate and are available for the target (copy,
    /// and reveal/copy-path on file targets). A rejected target is rejected for every other
    /// action: it is never rerouted through a different route.
    pub fn route(&self, target: &LinkTarget, action: LinkAction) -> Option<LinkOutcome> {
        let rejected =
            |reason: &str| Some(LinkOutcome::Rejected(SharedString::from(reason.to_owned())));
        if action == LinkAction::Copy {
            return None;
        }
        let address = match &target.navigation {
            Ok(address) => address.clone(),
            Err(reason) => return Some(LinkOutcome::Rejected(reason.clone())),
        };
        let route = &target.route;
        match action {
            LinkAction::Copy => None,
            LinkAction::Primary => Some(match route {
                LinkRoute::Web if self.embedded && self.preference == LinkPreference::Internal => {
                    LinkOutcome::Internal(address)
                }
                _ => LinkOutcome::External(address),
            }),
            LinkAction::Internal => match route {
                LinkRoute::Web if self.embedded => Some(LinkOutcome::Internal(address)),
                LinkRoute::Web => Some(LinkOutcome::External(address)),
                _ => rejected("only web links open in the embedded browser"),
            },
            LinkAction::External => match route {
                LinkRoute::Web => Some(LinkOutcome::External(address)),
                _ => rejected("only web links open with the system browser"),
            },
            LinkAction::Mail => match route {
                LinkRoute::Mail => Some(LinkOutcome::External(address)),
                _ => rejected("not a mail link"),
            },
            LinkAction::OpenFile => match route {
                LinkRoute::File { .. } => Some(LinkOutcome::External(address)),
                _ => rejected("not a file link"),
            },
            LinkAction::RevealFile | LinkAction::CopyPath => match route {
                LinkRoute::File { .. } => None,
                _ => rejected("not a file link"),
            },
        }
    }

    /// The default handler: routes the activation's action with this surface.
    ///
    /// Actions that do not navigate resolve to a rejection here; [`activate_link`] performs
    /// them itself and only asks the handler for navigation.
    pub fn handler(self) -> LinkHandler {
        LinkHandler::new(move |activation, _, _| {
            self.route(&activation.target, activation.action)
                .unwrap_or_else(|| LinkOutcome::Rejected("not a navigation".into()))
        })
    }
}

/// An activation of a link: the target, the action and optional context from its owner.
#[derive(Clone)]
pub struct LinkActivation {
    /// The activated target.
    pub target: LinkTarget,
    /// The action to perform.
    pub action: LinkAction,
    /// Context the owning surface attached to the link, such as the document or view it is in.
    pub owner: Option<Rc<dyn Any>>,
}

impl LinkActivation {
    /// The owner context as `T`, if it is one.
    pub fn owner<T: 'static>(&self) -> Option<&T> {
        self.owner.as_deref()?.downcast_ref()
    }
}

impl fmt::Debug for LinkActivation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LinkActivation")
            .field("target", &self.target)
            .field("action", &self.action)
            .field("has_owner", &self.owner.is_some())
            .finish()
    }
}

/// A surface-supplied handler that maps an activation to an outcome. It may act on
/// [`LinkOutcome::Internal`] itself (the embedded browser lives in the surface); external
/// outcomes are opened by [`activate_link`].
#[derive(Clone)]
pub struct LinkHandler(Rc<dyn Fn(&LinkActivation, &mut Window, &mut App) -> LinkOutcome>);

impl LinkHandler {
    /// Wraps a handler function.
    pub fn new(
        handler: impl Fn(&LinkActivation, &mut Window, &mut App) -> LinkOutcome + 'static,
    ) -> Self {
        Self(Rc::new(handler))
    }
}

/// Performs an activation: copy and file actions run directly, navigation goes through
/// `handler`, and an [`LinkOutcome::External`] result is opened with the system handler. A
/// rejected outcome does nothing at all.
pub fn activate_link(
    activation: &LinkActivation,
    handler: &LinkHandler,
    window: &mut Window,
    cx: &mut App,
) -> Option<LinkOutcome> {
    let target = &activation.target;
    match activation.action {
        LinkAction::Copy => {
            target.copy_original(cx);
            None
        }
        LinkAction::CopyPath => match target.file_path() {
            Some(path) => {
                cx.write_to_clipboard(ClipboardItem::new_string(path.display().to_string()));
                None
            }
            None => Some(LinkOutcome::Rejected("not a file link".into())),
        },
        LinkAction::RevealFile => match target.file_path() {
            Some(path) => {
                cx.reveal_path(path);
                None
            }
            None => Some(LinkOutcome::Rejected("not a file link".into())),
        },
        _ => {
            let outcome = (handler.0)(activation, window, cx);
            if let LinkOutcome::External(address) = &outcome {
                cx.open_url(address);
            }
            Some(outcome)
        }
    }
}

/// Whether a click activates a link: a primary-button single click released within
/// [`LINK_CLICK_SLOP`] pixels of the press, a keyboard click (Enter or Space on the focused
/// link), or a touch tap. Selection drags, secondary clicks, double clicks and long presses
/// never activate.
pub fn is_link_activation(event: &ClickEvent) -> bool {
    match event {
        ClickEvent::Mouse(click) => {
            click.down.button == MouseButton::Left
                && click.up.button == MouseButton::Left
                && click.down.click_count == 1
                && click.up.click_count == 1
                && (click.up.position - click.down.position).magnitude() <= LINK_CLICK_SLOP
        }
        ClickEvent::Keyboard(_) => true,
        ClickEvent::Touch(tap) => !tap.long_press,
    }
}

/// Splits a destination into display lines at character boundaries. Display only: the
/// destination itself is never changed.
pub fn break_destination(original: &str, max_chars: usize) -> Vec<String> {
    let max_chars = max_chars.max(1);
    let characters: Vec<char> = original.chars().collect();
    characters
        .chunks(max_chars)
        .map(|chunk| chunk.iter().collect())
        .collect()
}

/// What a menu request carries: the target, the actions to offer and where it was requested.
#[derive(Clone)]
pub struct LinkMenuRequest {
    /// The link the menu is for.
    pub target: LinkTarget,
    /// The actions to offer, in order.
    pub actions: Vec<LinkAction>,
    /// The pointer position, or `None` when requested from the keyboard.
    pub position: Option<Point<Pixels>>,
    /// The owner context attached to the link.
    pub owner: Option<Rc<dyn Any>>,
}

type LinkMenuListener = Rc<dyn Fn(&LinkMenuRequest, &mut Window, &mut App)>;

struct LinkState {
    focus: FocusHandle,
    hovered: bool,
    dismissed: bool,
}

/// An inline link: individually focusable, activated by a primary click, Enter/Space or a tap,
/// with its action menu requested by secondary click, long press, the Menu key or Shift+F10.
/// Hovering or focusing it discloses the full original destination as local text; Escape
/// dismisses the disclosure.
#[derive(IntoElement)]
pub struct Link {
    id: ElementId,
    target: LinkTarget,
    surface: LinkSurface,
    handler: Option<LinkHandler>,
    owner: Option<Rc<dyn Any>>,
    on_menu: Option<LinkMenuListener>,
    disclosure: bool,
    style: StyleRefinement,
}

impl Link {
    /// Creates a link element for `target`.
    pub fn new(id: impl Into<ElementId>, target: LinkTarget) -> Self {
        Self {
            id: id.into(),
            target,
            surface: LinkSurface::default(),
            handler: None,
            owner: None,
            on_menu: None,
            disclosure: true,
            style: StyleRefinement::default(),
        }
    }

    /// Sets how the owning surface routes this link (default: external only).
    pub fn surface(mut self, surface: LinkSurface) -> Self {
        self.surface = surface;
        self
    }

    /// Replaces the default handler with the surface's own.
    pub fn handler(mut self, handler: LinkHandler) -> Self {
        self.handler = Some(handler);
        self
    }

    /// Attaches owner context, delivered with every activation.
    pub fn owner(mut self, owner: Rc<dyn Any>) -> Self {
        self.owner = Some(owner);
        self
    }

    /// Registers the listener that shows the action menu.
    pub fn on_menu(
        mut self,
        listener: impl Fn(&LinkMenuRequest, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_menu = Some(Rc::new(listener));
        self
    }

    /// Turns the destination disclosure off.
    pub fn without_disclosure(mut self) -> Self {
        self.disclosure = false;
        self
    }
}

impl Styled for Link {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Link {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, cx| LinkState {
            focus: cx.focus_handle().tab_stop(true),
            hovered: false,
            dismissed: false,
        });
        let (focus, hovered, dismissed) = {
            let state = state.read(cx);
            (state.focus.clone(), state.hovered, state.dismissed)
        };
        let disclosed = self.disclosure && (hovered || focus.is_focused(window)) && !dismissed;

        let handler = self
            .handler
            .clone()
            .unwrap_or_else(|| self.surface.handler());
        let target = self.target.clone();
        let owner = self.owner.clone();
        let surface = self.surface;
        let on_menu = self.on_menu.clone();

        let activate = {
            let (target, owner, handler) = (target.clone(), owner.clone(), handler);
            move |action: LinkAction, window: &mut Window, cx: &mut App| {
                let activation = LinkActivation {
                    target: target.clone(),
                    action,
                    owner: owner.clone(),
                };
                activate_link(&activation, &handler, window, cx);
            }
        };
        let request_menu = {
            let (target, owner, on_menu) = (target.clone(), owner, on_menu);
            move |position: Option<Point<Pixels>>, window: &mut Window, cx: &mut App| {
                if let Some(on_menu) = &on_menu {
                    on_menu(
                        &LinkMenuRequest {
                            target: target.clone(),
                            actions: target.actions(&surface),
                            position,
                            owner: owner.clone(),
                        },
                        window,
                        cx,
                    );
                }
            }
        };

        let mut root = div();
        root.style().refine(&self.style);
        let mut link = root
            .id(self.id.clone())
            .relative()
            .track_focus(&focus)
            .role(Role::Link)
            .aria_label(target.label.clone())
            .cursor(CursorStyle::PointingHand)
            .text_color(rgb(0x2563eb))
            .underline()
            .on_click({
                let activate = activate.clone();
                move |event, window, cx| {
                    if is_link_activation(event) {
                        activate(LinkAction::Primary, window, cx);
                    }
                }
            })
            .on_aux_click({
                let request_menu = request_menu.clone();
                move |event, window, cx| {
                    if event.is_secondary() {
                        request_menu(event.mouse_position(), window, cx);
                    }
                }
            })
            .on_hover({
                let state = state.clone();
                move |hovered, _, cx| {
                    state.update(cx, |state, cx| {
                        state.hovered = *hovered;
                        if *hovered {
                            state.dismissed = false;
                        }
                        cx.notify();
                    });
                }
            })
            .on_key_down({
                move |event: &KeyDownEvent, window, cx| {
                    let keystroke = &event.keystroke;
                    if keystroke.key == "escape" && disclosed {
                        state.update(cx, |state, cx| {
                            state.dismissed = true;
                            cx.notify();
                        });
                        cx.stop_propagation();
                    } else if keystroke.key == "menu"
                        || (keystroke.key == "f10" && keystroke.modifiers.shift)
                    {
                        request_menu(None, window, cx);
                        cx.stop_propagation();
                    }
                }
            })
            .child(target.label.clone());

        if disclosed {
            let lines = break_destination(&target.original, LINK_DISCLOSURE_LINE_CHARS);
            let mut disclosure = div()
                .absolute()
                .top(crate::relative(1.))
                .left_0()
                .px_2()
                .py_1()
                .text_xs()
                .text_color(rgb(0xf9fafb))
                .bg(rgb(0x111827))
                .rounded_md()
                .flex()
                .flex_col()
                .debug_selector(|| "link-disclosure".to_owned());
            for line in lines {
                disclosure = disclosure.child(line);
            }
            link = link.child(deferred(disclosure).with_priority(1));
        }
        link
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Context, Modifiers, MouseDownEvent, MouseUpEvent, Render, TestAppContext,
        VisualTestContext, point, px,
    };
    use std::cell::RefCell;

    fn default_target(original: &str) -> LinkTarget {
        LinkTarget::with_default_policy("label", original.to_owned())
    }

    #[test]
    fn default_policy_rejects_every_unsafe_destination_for_all_navigation_actions() {
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
        let surface = LinkSurface {
            embedded: true,
            preference: LinkPreference::Internal,
        };
        for original in rejected {
            let target = default_target(original);
            assert!(target.navigation().is_err(), "{original:?} was accepted");
            assert_eq!(target.original().as_ref(), original);
            for action in [
                LinkAction::Primary,
                LinkAction::Internal,
                LinkAction::External,
            ] {
                assert!(
                    matches!(
                        surface.route(&target, action),
                        Some(LinkOutcome::Rejected(_))
                    ),
                    "{original:?} reached a route through {action:?}"
                );
            }
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
    fn internal_action_follows_the_surface() {
        let target = default_target("https://example.com/page");
        let embedded = LinkSurface {
            embedded: true,
            preference: LinkPreference::External,
        };
        let plain = LinkSurface::default();
        assert_eq!(
            embedded.route(&target, LinkAction::Internal),
            Some(LinkOutcome::Internal("https://example.com/page".into()))
        );
        assert_eq!(
            plain.route(&target, LinkAction::Internal),
            Some(LinkOutcome::External("https://example.com/page".into()))
        );
        assert_eq!(
            embedded.route(&target, LinkAction::Primary),
            Some(LinkOutcome::External("https://example.com/page".into()))
        );
        let preferring_internal = LinkSurface {
            embedded: true,
            preference: LinkPreference::Internal,
        };
        assert_eq!(
            preferring_internal.route(&target, LinkAction::Primary),
            Some(LinkOutcome::Internal("https://example.com/page".into()))
        );
        assert_eq!(
            LinkSurface {
                embedded: false,
                preference: LinkPreference::Internal
            }
            .route(&target, LinkAction::Primary),
            Some(LinkOutcome::External("https://example.com/page".into()))
        );
    }

    #[test]
    fn mail_and_file_routes_are_separate_from_web_routes() {
        let routes = LinkRoutes::new()
            .with_mail()
            .with_files([PathBuf::from("/srv/docs")]);
        let surface = LinkSurface::default();

        let mail = LinkTarget::with_routes("mail", "MAILTO:a@b.com", &routes);
        assert_eq!(mail.route(), &LinkRoute::Mail);
        assert_eq!(
            surface.route(&mail, LinkAction::Mail),
            Some(LinkOutcome::External("mailto:a@b.com".into()))
        );
        assert!(matches!(
            surface.route(&mail, LinkAction::External),
            Some(LinkOutcome::Rejected(_))
        ));
        assert!(matches!(
            surface.route(&mail, LinkAction::Internal),
            Some(LinkOutcome::Rejected(_))
        ));
        // Without the mail route a mailto: address is rejected like any other scheme.
        assert!(
            LinkTarget::with_routes("m", "mailto:a@b.com", &LinkRoutes::new())
                .navigation()
                .is_err()
        );

        let file = LinkTarget::with_routes("file", "/my dir/notes.txt", &routes);
        assert_eq!(file.file_path(), Some(Path::new("/my dir/notes.txt")));
        assert_eq!(
            surface.route(&file, LinkAction::OpenFile),
            Some(LinkOutcome::External("file:///my%20dir/notes.txt".into()))
        );
        assert_eq!(surface.route(&file, LinkAction::RevealFile), None);
        assert_eq!(surface.route(&file, LinkAction::CopyPath), None);
        assert!(matches!(
            surface.route(&file, LinkAction::External),
            Some(LinkOutcome::Rejected(_))
        ));
        assert!(matches!(
            surface.route(&default_target("https://example.com"), LinkAction::OpenFile),
            Some(LinkOutcome::Rejected(_))
        ));
        assert!(file.actions(&surface).contains(&LinkAction::RevealFile));
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

    #[test]
    fn rejected_target_offers_only_copy() {
        let target = default_target("javascript:alert(1)");
        assert_eq!(
            target.actions(&LinkSurface::default()),
            vec![LinkAction::Copy]
        );
    }

    #[test]
    fn long_destinations_break_at_character_boundaries_for_display() {
        let original = "https://example.com/é".repeat(10);
        let lines = break_destination(&original, 7);
        assert!(lines.iter().all(|line| line.chars().count() <= 7));
        assert_eq!(lines.concat(), original);
        assert_eq!(break_destination("", 4), Vec::<String>::new());
    }

    #[crate::test]
    fn copy_writes_the_exact_original_even_when_rejected_or_misleading(cx: &mut TestAppContext) {
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

    struct LinkView {
        target: LinkTarget,
        activations: Rc<RefCell<Vec<LinkAction>>>,
        menus: Rc<RefCell<Vec<Vec<LinkAction>>>>,
    }

    impl Render for LinkView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let activations = self.activations.clone();
            let menus = self.menus.clone();
            div().size_full().child(
                Link::new("link", self.target.clone())
                    .w(px(300.))
                    .h(px(20.))
                    .surface(LinkSurface {
                        embedded: true,
                        preference: LinkPreference::External,
                    })
                    .handler(LinkHandler::new(move |activation, _, _| {
                        activations.borrow_mut().push(activation.action);
                        LinkSurface {
                            embedded: true,
                            preference: LinkPreference::External,
                        }
                        .route(&activation.target, activation.action)
                        .unwrap_or_else(|| LinkOutcome::Rejected("n/a".into()))
                    }))
                    .on_menu(move |request, _, _| {
                        menus.borrow_mut().push(request.actions.clone());
                    }),
            )
        }
    }

    struct Harness {
        view: crate::Entity<LinkView>,
        activations: Rc<RefCell<Vec<LinkAction>>>,
        menus: Rc<RefCell<Vec<Vec<LinkAction>>>>,
    }

    fn open_link<'a>(
        cx: &'a mut TestAppContext,
        original: &str,
    ) -> (Harness, &'a mut VisualTestContext) {
        let activations = Rc::new(RefCell::new(Vec::new()));
        let menus = Rc::new(RefCell::new(Vec::new()));
        let (view, window) = cx.add_window_view({
            let activations = activations.clone();
            let menus = menus.clone();
            let original = original.to_owned();
            move |_, _| LinkView {
                target: LinkTarget::with_default_policy("Example", original),
                activations,
                menus,
            }
        });
        window.run_until_parked();
        (
            Harness {
                view,
                activations,
                menus,
            },
            window,
        )
    }

    fn down(
        window: &mut VisualTestContext,
        position: Point<Pixels>,
        button: MouseButton,
        count: usize,
    ) {
        window.simulate_event(MouseDownEvent {
            position,
            modifiers: Modifiers::none(),
            button,
            click_count: count,
            first_mouse: false,
        });
    }

    fn up(
        window: &mut VisualTestContext,
        position: Point<Pixels>,
        button: MouseButton,
        count: usize,
    ) {
        window.simulate_event(MouseUpEvent {
            position,
            modifiers: Modifiers::none(),
            button,
            click_count: count,
        });
    }

    #[crate::test]
    async fn pointer_activation_follows_the_click_rules(cx: &mut TestAppContext) {
        let (harness, window) = open_link(cx, "https://example.com");
        let _ = &harness.view;
        let press = point(px(10.), px(10.));

        // 20 px apart: a selection drag, not a click.
        down(window, press, MouseButton::Left, 1);
        up(window, point(px(30.), px(10.)), MouseButton::Left, 1);
        assert!(harness.activations.borrow().is_empty());

        // Secondary click.
        down(window, press, MouseButton::Right, 1);
        up(window, press, MouseButton::Right, 1);
        assert!(harness.activations.borrow().is_empty());

        // Double click.
        down(window, press, MouseButton::Left, 2);
        up(window, press, MouseButton::Left, 2);
        assert!(harness.activations.borrow().is_empty());
        assert!(window.opened_urls().is_empty());

        // 4 px apart activates.
        down(window, press, MouseButton::Left, 1);
        up(window, point(px(14.), px(10.)), MouseButton::Left, 1);
        assert_eq!(*harness.activations.borrow(), vec![LinkAction::Primary]);
        assert_eq!(
            window.opened_urls(),
            vec!["https://example.com/".to_owned()]
        );
    }

    #[crate::test]
    async fn secondary_click_requests_the_menu_without_navigating(cx: &mut TestAppContext) {
        let (harness, window) = open_link(cx, "https://example.com");
        let press = point(px(10.), px(10.));
        down(window, press, MouseButton::Right, 1);
        up(window, press, MouseButton::Right, 1);
        assert!(harness.activations.borrow().is_empty());
        assert_eq!(
            *harness.menus.borrow(),
            vec![vec![
                LinkAction::Primary,
                LinkAction::Internal,
                LinkAction::External,
                LinkAction::Copy
            ]]
        );
        assert!(window.opened_urls().is_empty());
    }

    /// A full key press: key down, then key up (links click on the release).
    fn press(window: &mut VisualTestContext, keystroke: &str) {
        let keystroke = crate::Keystroke::parse(keystroke).unwrap();
        window.simulate_event(crate::KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        window.simulate_event(crate::KeyUpEvent { keystroke });
    }

    #[crate::test]
    async fn keyboard_activates_the_focused_link_and_opens_its_menu(cx: &mut TestAppContext) {
        let (harness, window) = open_link(cx, "https://example.com");
        window.update(|window, cx| window.focus_next(cx));
        press(window, "enter");
        assert_eq!(*harness.activations.borrow(), vec![LinkAction::Primary]);
        press(window, "space");
        assert_eq!(harness.activations.borrow().len(), 2);
        window.simulate_keystrokes("shift-f10");
        assert_eq!(harness.menus.borrow().len(), 1);
        window.simulate_keystrokes("menu");
        assert_eq!(harness.menus.borrow().len(), 2);
    }

    #[crate::test]
    async fn hover_discloses_the_original_destination_and_escape_dismisses_it(
        cx: &mut TestAppContext,
    ) {
        let (_harness, window) = open_link(cx, "https://example.com/some/path");
        // The test window starts with its pointer at the origin, on top of the link.
        window.simulate_mouse_move(point(px(500.), px(300.)), None, Modifiers::none());
        assert!(window.debug_bounds("link-disclosure").is_none());
        window.simulate_mouse_move(point(px(10.), px(10.)), None, Modifiers::none());
        window.update(|window, cx| window.focus_next(cx));
        window.run_until_parked();
        assert!(window.debug_bounds("link-disclosure").is_some());

        window.simulate_keystrokes("escape");
        assert!(window.debug_bounds("link-disclosure").is_none());

        // Moving off and back onto the link discloses it again.
        window.simulate_mouse_move(point(px(500.), px(300.)), None, Modifiers::none());
        window.simulate_mouse_move(point(px(10.), px(10.)), None, Modifiers::none());
        assert!(window.debug_bounds("link-disclosure").is_some());
    }

    #[crate::test]
    async fn rejected_link_never_opens_anything(cx: &mut TestAppContext) {
        let (harness, window) = open_link(cx, "javascript:alert(1)");
        let at = point(px(10.), px(10.));
        down(window, at, MouseButton::Left, 1);
        up(window, at, MouseButton::Left, 1);
        press(window, "enter");
        assert!(window.opened_urls().is_empty());
        assert_eq!(harness.activations.borrow().len(), 2);
    }

    #[test]
    fn click_rule_unit() {
        use crate::{KeyboardClickEvent, MouseClickEvent, TouchClickEvent};
        let mouse = |button, count, dx: f32| {
            ClickEvent::Mouse(MouseClickEvent {
                down: MouseDownEvent {
                    button,
                    position: point(px(0.), px(0.)),
                    click_count: count,
                    ..Default::default()
                },
                up: MouseUpEvent {
                    button,
                    position: point(px(dx), px(0.)),
                    click_count: count,
                    ..Default::default()
                },
            })
        };
        assert!(is_link_activation(&mouse(MouseButton::Left, 1, 4.)));
        assert!(!is_link_activation(&mouse(MouseButton::Left, 1, 4.5)));
        assert!(!is_link_activation(&mouse(MouseButton::Left, 1, 20.)));
        assert!(!is_link_activation(&mouse(MouseButton::Right, 1, 0.)));
        assert!(!is_link_activation(&mouse(MouseButton::Left, 2, 0.)));
        assert!(is_link_activation(&ClickEvent::Keyboard(
            KeyboardClickEvent::default()
        )));
        assert!(is_link_activation(&ClickEvent::Touch(TouchClickEvent {
            tap_count: 1,
            ..Default::default()
        })));
        assert!(!is_link_activation(&ClickEvent::Touch(TouchClickEvent {
            long_press: true,
            ..Default::default()
        })));
    }
}
