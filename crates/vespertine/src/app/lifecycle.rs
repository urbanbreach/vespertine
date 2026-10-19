//! Application lifecycle decisions and retained-application events (PLT-008 R5..R7).
//!
//! * Quit interception: [`App::on_quit_request`] registers an asynchronous decision that
//!   [`App::quit`] consults once; [`App::force_quit`] skips it.
//! * Retained lifecycle: with [`QuitMode::Explicit`](crate::QuitMode) the event loop keeps
//!   running with no windows. The platform's activation mechanism then delivers
//!   [`App::on_reopen`] notifications, incoming URL lists ([`App::on_open_urls`]) and
//!   activation requests, which [`App::activate_app`] turns into raising a window.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use futures::{FutureExt as _, future::LocalBoxFuture};
use vespertine_collections::VecDeque;

use crate::{App, AppContext as _, Subscription};

type QuitDecisionHandler = Box<dyn FnMut(&mut App) -> LocalBoxFuture<'static, bool>>;
type OpenUrlsHandler = Box<dyn FnMut(Vec<String>, &mut App)>;
type ReopenHandler = Box<dyn FnMut(&mut App)>;
type ActivateActionHandler = Box<dyn FnMut(&str, &mut App)>;

struct QuitDecisionSlot {
    registration: u64,
    handler: QuitDecisionHandler,
}

/// A request to bring the application's window forward, as delivered by the platform's
/// activation mechanism (on Linux the `platform_data` of an `org.freedesktop.Application`
/// call).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActivationRequest {
    /// The activation token (`activation-token`, or `desktop-startup-id`) that lets the
    /// compositor or window manager move focus to the window. Without one the application
    /// only requests attention instead of taking focus.
    pub token: Option<String>,
}

#[derive(Default)]
pub(crate) struct AppLifecycle {
    quit_decision: Rc<RefCell<Option<QuitDecisionSlot>>>,
    next_registration: Cell<u64>,
    quit_pending: Cell<bool>,
    /// Set once a quit decision allowed the quit and the app-quit handlers have run, so that
    /// the platform's own exit path does not run them again.
    pub(crate) quit_completed: Cell<bool>,
    quit_hook_installed: bool,
    open_urls: Option<OpenUrlsHandler>,
    pending_open_urls: VecDeque<Vec<String>>,
    reopen: Option<ReopenHandler>,
    activate_action: Option<ActivateActionHandler>,
    /// Whether a reopen handler was ever registered; it is taken out while it runs.
    reopen_registered: bool,
}

impl AppLifecycle {
    /// Whether the application said how it recreates its main window, so it keeps running
    /// with no windows under [`QuitMode::Default`](crate::QuitMode).
    pub(crate) fn retains_without_windows(&self) -> bool {
        self.reopen_registered
    }
}

impl App {
    /// Registers the quit-request decision, replacing any previous one.
    ///
    /// When a quit is requested through [`App::quit`] (an application menu or action, or the
    /// platform: on Linux `SIGTERM` or `SIGHUP`) `decide` is called once and returns a future
    /// that resolves to whether the quit may proceed. Until it resolves the application stays
    /// alive with its windows open, and further quit requests are coalesced into the pending
    /// one. `true` runs the app-quit handlers ([`Context::on_app_quit`](crate::Context)) and
    /// stops the event loop; `false` cancels the quit and leaves everything as it was.
    /// [`App::force_quit`] skips the decision.
    ///
    /// Dropping the returned subscription removes the decision, unless a later registration
    /// already replaced it.
    pub fn on_quit_request<F, Fut>(&mut self, mut decide: F) -> Subscription
    where
        F: FnMut(&mut App) -> Fut + 'static,
        Fut: Future<Output = bool> + 'static,
    {
        let registration = self.lifecycle.next_registration.get() + 1;
        self.lifecycle.next_registration.set(registration);
        *self.lifecycle.quit_decision.borrow_mut() = Some(QuitDecisionSlot {
            registration,
            handler: Box::new(move |cx| decide(cx).boxed_local()),
        });

        if !self.lifecycle.quit_hook_installed {
            self.lifecycle.quit_hook_installed = true;
            let this = self.this.clone();
            self.platform.on_quit_requested(Box::new(move || {
                let Some(app) = this.upgrade() else {
                    return;
                };
                match app.try_borrow_mut() {
                    Ok(cx) => cx.quit(),
                    Err(_) => log::warn!("quit request arrived while the app was borrowed"),
                }
            }));
        }

        let slot = self.lifecycle.quit_decision.clone();
        Subscription::new(move || {
            let mut slot = slot.borrow_mut();
            if slot
                .as_ref()
                .is_some_and(|slot| slot.registration == registration)
            {
                *slot = None;
            }
        })
    }

    /// Requests that the application quit.
    ///
    /// With a decision registered through [`App::on_quit_request`] the decision is consulted
    /// first, once; requests arriving while it is pending are coalesced into it. Without one
    /// this is [`App::force_quit`]. The handlers registered with
    /// [`Context::on_app_quit`](crate::Context) run when the quit proceeds.
    pub fn quit(&self) {
        if self.lifecycle.quit_decision.borrow().is_none() {
            self.force_quit();
            return;
        }
        if self.lifecycle.quit_pending.replace(true) {
            return;
        }
        let this = self.this.clone();
        self.foreground_executor
            .spawn(async move {
                let Some(app) = this.upgrade() else {
                    return;
                };
                let decision = app.borrow_mut().update(|cx| cx.consult_quit_decision());
                drop(app);
                let allow = match decision {
                    Some(decision) => decision.await,
                    None => true,
                };
                let Some(app) = this.upgrade() else {
                    return;
                };
                app.borrow_mut().update(|cx| cx.finish_quit_request(allow));
            })
            .detach();
    }

    /// Quits the application without consulting the decision registered with
    /// [`App::on_quit_request`]: the platform stops the event loop and the app-quit handlers
    /// run on the way out.
    pub fn force_quit(&self) {
        self.platform.quit();
    }

    fn consult_quit_decision(&mut self) -> Option<LocalBoxFuture<'static, bool>> {
        let quit_decision = self.lifecycle.quit_decision.clone();
        // The handler runs without the slot borrowed so that it may replace itself.
        let mut slot = quit_decision.borrow_mut().take()?;
        let decision = (slot.handler)(self);
        let mut current = quit_decision.borrow_mut();
        if current.is_none() {
            *current = Some(slot);
        }
        Some(decision)
    }

    fn finish_quit_request(&mut self, allow: bool) {
        self.lifecycle.quit_pending.set(false);
        if allow {
            self.shutdown();
            self.lifecycle.quit_completed.set(true);
            self.platform.quit();
        }
    }

    /// Registers the handler for incoming URL lists, replacing any previous one.
    ///
    /// Lists that arrived before a handler existed, such as URLs on the command line of the
    /// first launch, are queued and delivered here once each, in order, exactly as given.
    /// On Linux they also come from the `Open` call of `org.freedesktop.Application`.
    /// Routing and policy are the handler's; see [`Platform::register_url_scheme`] for
    /// claiming URL schemes.
    pub fn on_open_urls(&mut self, handler: impl FnMut(Vec<String>, &mut App) + 'static) {
        self.lifecycle.open_urls = Some(Box::new(handler));
        self.flush_open_urls();
    }

    /// Queues a list of URLs for the handler registered with [`App::on_open_urls`] and
    /// delivers everything queued if there is one.
    pub fn deliver_open_urls(&mut self, urls: Vec<String>) {
        if urls.is_empty() {
            return;
        }
        self.lifecycle.pending_open_urls.push_back(urls);
        self.flush_open_urls();
    }

    fn flush_open_urls(&mut self) {
        let Some(mut handler) = self.lifecycle.open_urls.take() else {
            return;
        };
        while let Some(urls) = self.lifecycle.pending_open_urls.pop_front() {
            handler(urls, self);
        }
        // The handler may have replaced itself.
        if self.lifecycle.open_urls.is_none() {
            self.lifecycle.open_urls = Some(handler);
        }
    }

    /// Registers the handler invoked when an already-running application is launched again:
    /// on Linux an `Activate` call of `org.freedesktop.Application`, on macOS a Dock click.
    /// It is the place to activate or recreate the main window while the application is
    /// retaining its state without windows.
    pub fn on_reopen(&mut self, handler: impl FnMut(&mut App) + 'static) {
        self.lifecycle.reopen_registered = true;
        self.lifecycle.reopen = Some(Box::new(handler));
    }

    pub(crate) fn deliver_reopen(&mut self) {
        let Some(mut handler) = self.lifecycle.reopen.take() else {
            return;
        };
        handler(self);
        if self.lifecycle.reopen.is_none() {
            self.lifecycle.reopen = Some(handler);
        }
    }

    /// Registers the handler invoked with the action name when a desktop action is
    /// activated through the platform's application-activation mechanism (on Linux
    /// `ActivateAction`). Action parameters are not forwarded.
    pub fn on_activate_action(&mut self, handler: impl FnMut(&str, &mut App) + 'static) {
        self.lifecycle.activate_action = Some(Box::new(handler));
    }

    pub(crate) fn deliver_activate_action(&mut self, action: &str) {
        let Some(mut handler) = self.lifecycle.activate_action.take() else {
            log::debug!("no handler for activated action {action:?}");
            return;
        };
        handler(action, self);
        if self.lifecycle.activate_action.is_none() {
            self.lifecycle.activate_action = Some(handler);
        }
    }

    /// Brings the application's window forward in answer to an activation request.
    ///
    /// The target is the active window, otherwise the window opened most recently. A window
    /// that is minimized is also restored. With a token in `request` the window is raised
    /// through the platform's token mechanism (xdg-activation on Wayland, `_NET_STARTUP_ID`
    /// on X11) so that the compositor lets focus move; without one only attention is
    /// requested and focus is not forced. Returns whether a window was targeted.
    pub fn activate_app(&mut self, request: &ActivationRequest) -> bool {
        let target = self
            .active_window()
            .filter(|handle| self.windows.contains_key(handle.window_id()))
            .or_else(|| self.windows().last().copied());
        let Some(target) = target else {
            return false;
        };
        self.update_window(target, |_, window, _| match request.token.as_deref() {
            Some(token) => window.platform_window.activate_with_token(token),
            None => window.platform_window.request_attention(),
        })
        .is_ok()
    }
}

/// Selects the arguments that are URLs: an RFC 3986 scheme of at least two characters, then
/// `:` and a non-empty remainder that is not another `:` (so `module::path` is not a URL),
/// for an argument that is not the name of an existing file. Arguments are returned exactly
/// as given, in order.
pub fn launch_urls_from_args<I, S>(args: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    fn is_url(argument: &str) -> bool {
        let Some((scheme, rest)) = argument.split_once(':') else {
            return false;
        };
        scheme.len() >= 2
            && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
            && !rest.is_empty()
            && !rest.starts_with(':')
            && !std::path::Path::new(argument).exists()
    }

    args.into_iter()
        .filter(|argument| is_url(argument.as_ref()))
        .map(|argument| argument.as_ref().to_owned())
        .collect()
}

impl App {
    /// Queues the URLs on this process's command line for the handler registered with
    /// [`App::on_open_urls`]. Only Linux and FreeBSD launchers pass URLs as arguments; other
    /// platforms deliver them through their own event.
    pub(crate) fn queue_launch_urls(&mut self) {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            let urls = launch_urls_from_args(
                std::env::args_os()
                    .skip(1)
                    .filter_map(|argument| argument.into_string().ok()),
            );
            self.deliver_open_urls(urls);
        }
    }
}
