#[cfg(any(test, feature = "test-support"))]
use crate::NoopTextSystem;
#[cfg(any(test, feature = "test-support"))]
use crate::PathPromptOptions;
use crate::{
    ActivityGuard, AnyWindowHandle, AppLifecyclePhase, BackgroundExecutor, ClipboardItem,
    CursorStyle, DevicePixels, DisplayId, DummyKeyboardMapper, ForegroundExecutor, Keymap,
    OwnedMenu, Platform, PlatformDisplay, PlatformHeadlessRenderer, PlatformKeyboardLayout,
    PlatformKeyboardMapper, PlatformTextSystem, PromptButton, ScreenCaptureFrame,
    ScreenCaptureSource, ScreenCaptureStream, SharedString, SourceMetadata, SystemNotification,
    SystemNotificationDelivery, SystemNotificationResponse, Task, TestDisplay, TestLifecycle,
    TestWindow, ThermalState, WindowAppearance, WindowParams, size,
};
use anyhow::Result;
use futures::channel::oneshot;
use parking_lot::Mutex;
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::{Rc, Weak},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
#[cfg(any(test, feature = "test-support"))]
use vespertine_collections::VecDeque;

/// TestPlatform implements the Platform trait for use in tests.
pub(crate) struct TestPlatform {
    background_executor: BackgroundExecutor,
    foreground_executor: ForegroundExecutor,

    pub(crate) active_window: RefCell<Option<TestWindow>>,
    displays: RefCell<Vec<TestDisplay>>,
    displays_changed_callback: RefCell<Option<Box<dyn FnMut()>>>,
    active_cursor: Mutex<CursorStyle>,
    current_clipboard_item: Mutex<Option<ClipboardItem>>,
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    current_primary_item: Mutex<Option<ClipboardItem>>,
    #[cfg(target_os = "macos")]
    current_find_pasteboard_item: Mutex<Option<ClipboardItem>>,
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) prompts: RefCell<TestPrompts>,
    screen_capture_sources: RefCell<Vec<TestScreenCaptureSource>>,
    pub opened_url: RefCell<Option<String>>,
    pub(crate) external: RefCell<TestExternalOpens>,
    pub(crate) system_notifications: RefCell<TestSystemNotifications>,
    pub text_system: Arc<dyn PlatformTextSystem>,
    pub expect_restart:
        RefCell<Option<oneshot::Sender<(Option<PathBuf>, Vec<std::ffi::OsString>)>>>,
    idle_sleep_prevention_count: Arc<AtomicUsize>,
    idle_sleep_prevention_delay: Cell<Duration>,
    idle_sleep_prevention_fails: Cell<bool>,
    headless_renderer_factory:
        Option<Box<dyn Fn() -> anyhow::Result<Option<Box<dyn PlatformHeadlessRenderer>>>>>,
    weak: Weak<Self>,
    menus: RefCell<Vec<OwnedMenu>>,
    quit_requests: Cell<usize>,
    pub(crate) app_lifecycle_callback: RefCell<Option<Box<dyn FnMut(AppLifecyclePhase)>>>,
    pub(crate) memory_warning_callback: RefCell<Option<Box<dyn FnMut()>>>,
    pub(crate) system_reduced_motion: Cell<bool>,
    /// The desktop titlebar button layout reported by [`Platform::button_layout`].
    pub(crate) button_layout: RefCell<Option<crate::WindowButtonLayout>>,
    pub(crate) lifecycle: TestLifecycle,
}

#[derive(Clone)]
/// A fake screen capture source, used for testing.
pub struct TestScreenCaptureSource {}

/// A fake screen capture stream, used for testing.
pub struct TestScreenCaptureStream {}

impl ScreenCaptureSource for TestScreenCaptureSource {
    fn metadata(&self) -> Result<SourceMetadata> {
        Ok(SourceMetadata {
            id: 0,
            is_main: None,
            label: None,
            resolution: size(DevicePixels(1), DevicePixels(1)),
        })
    }

    fn stream(
        &self,
        _foreground_executor: &ForegroundExecutor,
        _frame_callback: Box<dyn Fn(ScreenCaptureFrame) + Send>,
    ) -> oneshot::Receiver<Result<Box<dyn ScreenCaptureStream>>> {
        let (mut tx, rx) = oneshot::channel();
        let stream = TestScreenCaptureStream {};
        tx.send(Ok(Box::new(stream) as Box<dyn ScreenCaptureStream>))
            .ok();
        rx
    }
}

impl ScreenCaptureStream for TestScreenCaptureStream {
    fn metadata(&self) -> Result<SourceMetadata> {
        TestScreenCaptureSource {}.metadata()
    }
}

/// Everything the application handed to the desktop through the external-opening APIs.
#[derive(Default)]
pub(crate) struct TestExternalOpens {
    pub(crate) urls: Vec<String>,
    pub(crate) opened_paths: Vec<PathBuf>,
    pub(crate) revealed_paths: Vec<PathBuf>,
    pub(crate) schemes: Vec<String>,
}

#[cfg(any(test, feature = "test-support"))]
struct TestPrompt {
    msg: String,
    detail: Option<String>,
    answers: Vec<String>,
    tx: oneshot::Sender<usize>,
}

#[derive(Default)]
pub(crate) struct TestSystemNotifications {
    pub(crate) app_identity: Option<(SharedString, SharedString)>,
    pub(crate) shown: Vec<SystemNotification>,
    pub(crate) delivered: Vec<SystemNotification>,
    pub(crate) dismissed: Vec<SharedString>,
    /// Every request passed to `post_system_notification`, in order,
    /// whatever its scripted result.
    pub(crate) posted: Vec<SystemNotification>,
    /// Results the next posts resolve to; when empty a post is delivered if
    /// an app identity is set and unavailable otherwise.
    pub(crate) scripted_deliveries: std::collections::VecDeque<SystemNotificationDelivery>,
    /// `Some(false)` simulates a service without activation support.
    pub(crate) activation_supported: Option<bool>,
    response_callback: Option<Box<dyn FnMut(SystemNotificationResponse)>>,
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
pub(crate) struct TestPrompts {
    multiple_choice: VecDeque<TestPrompt>,
    new_path: VecDeque<(PathBuf, oneshot::Sender<Result<Option<PathBuf>>>)>,
    paths: VecDeque<(
        PathPromptOptions,
        oneshot::Sender<Result<Option<Vec<PathBuf>>>>,
    )>,
}

impl TestPlatform {
    /// How many times the application asked the platform to quit.
    pub(crate) fn quit_requests(&self) -> usize {
        self.quit_requests.get()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn new(executor: BackgroundExecutor, foreground_executor: ForegroundExecutor) -> Rc<Self> {
        Self::with_platform(
            executor,
            foreground_executor,
            Arc::new(NoopTextSystem),
            None,
        )
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn with_text_system(
        executor: BackgroundExecutor,
        foreground_executor: ForegroundExecutor,
        text_system: Arc<dyn PlatformTextSystem>,
    ) -> Rc<Self> {
        Self::with_platform(executor, foreground_executor, text_system, None)
    }

    pub(crate) fn simulate_display_added(
        &self,
        display_id: DisplayId,
        refresh_interval: Option<Duration>,
    ) {
        self.connect_display(display_id, refresh_interval);
        self.report_displays_changed();
    }

    /// Adds a display without telling App yet, as when the platform's
    /// notifications about a window arrive before those about displays.
    pub(crate) fn connect_display(
        &self,
        display_id: DisplayId,
        refresh_interval: Option<Duration>,
    ) {
        let mut display = TestDisplay::with_id(display_id);
        display.refresh_interval = refresh_interval;
        self.displays.borrow_mut().push(display);
    }

    pub(crate) fn simulate_display_removed(&self, display_id: DisplayId) {
        self.displays
            .borrow_mut()
            .retain(|display| display.id() != display_id);
        self.report_displays_changed();
    }

    pub(crate) fn simulate_display_refresh_interval_change(
        &self,
        display_id: DisplayId,
        refresh_interval: Option<Duration>,
    ) {
        for display in self.displays.borrow_mut().iter_mut() {
            if display.id() == display_id {
                display.refresh_interval = refresh_interval;
            }
        }
        self.report_displays_changed();
    }

    fn report_displays_changed(&self) {
        let callback = self.displays_changed_callback.borrow_mut().take();
        if let Some(mut callback) = callback {
            callback();
            self.displays_changed_callback
                .borrow_mut()
                .get_or_insert(callback);
        }
    }

    /// Connects a display with the given geometry and tells App.
    pub(crate) fn simulate_display_connected(&self, display: TestDisplay) {
        self.displays.borrow_mut().push(display);
        self.report_displays_changed();
    }

    /// Replaces the display with the same id as `display` and tells App.
    pub(crate) fn simulate_display_replaced(&self, display: TestDisplay) {
        let mut displays = self.displays.borrow_mut();
        if let Some(slot) = displays.iter_mut().find(|slot| slot.id() == display.id()) {
            *slot = display;
        }
        drop(displays);
        self.report_displays_changed();
    }

    /// Delivers an incoming URL list as the platform's activation mechanism would.
    pub(crate) fn simulate_open_urls(&self, urls: Vec<String>) {
        TestLifecycle::invoke(&self.lifecycle.open_urls, urls);
    }

    /// Delivers a reopen notification.
    pub(crate) fn simulate_reopen(&self) {
        let callback = self.lifecycle.reopen.borrow_mut().take();
        if let Some(mut callback) = callback {
            callback();
            self.lifecycle.reopen.borrow_mut().get_or_insert(callback);
        }
    }

    /// Delivers an activation request (the token of an `Activate` call, if any).
    pub(crate) fn simulate_activation_request(&self, request: crate::ActivationRequest) {
        TestLifecycle::invoke(&self.lifecycle.activation_request, request);
    }

    /// Delivers an activated desktop action.
    pub(crate) fn simulate_activate_action(&self, action: &str) {
        TestLifecycle::invoke(&self.lifecycle.activate_action, action.to_owned());
    }

    /// Delivers a termination request, as Linux does for `SIGTERM`.
    pub(crate) fn simulate_quit_request(&self) {
        let callback = self.lifecycle.quit_requested.borrow_mut().take();
        if let Some(mut callback) = callback {
            callback();
            self.lifecycle
                .quit_requested
                .borrow_mut()
                .get_or_insert(callback);
        }
    }

    /// How many times the platform was asked to stop the event loop.
    pub(crate) fn quit_count(&self) -> usize {
        self.lifecycle.quit_count.get()
    }

    pub fn with_platform(
        executor: BackgroundExecutor,
        foreground_executor: ForegroundExecutor,
        text_system: Arc<dyn PlatformTextSystem>,
        headless_renderer_factory: Option<
            Box<dyn Fn() -> anyhow::Result<Option<Box<dyn PlatformHeadlessRenderer>>>>,
        >,
    ) -> Rc<Self> {
        Rc::new_cyclic(|weak| TestPlatform {
            background_executor: executor,
            foreground_executor,
            #[cfg(any(test, feature = "test-support"))]
            prompts: Default::default(),
            screen_capture_sources: Default::default(),
            active_cursor: Default::default(),
            displays: RefCell::new(vec![TestDisplay::new()]),
            displays_changed_callback: Default::default(),
            active_window: Default::default(),
            expect_restart: Default::default(),
            current_clipboard_item: Mutex::new(None),
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            current_primary_item: Mutex::new(None),
            #[cfg(target_os = "macos")]
            current_find_pasteboard_item: Mutex::new(None),
            idle_sleep_prevention_count: Arc::new(AtomicUsize::new(0)),
            idle_sleep_prevention_delay: Cell::new(Duration::ZERO),
            idle_sleep_prevention_fails: Cell::new(false),
            weak: weak.clone(),
            opened_url: Default::default(),
            external: Default::default(),
            system_notifications: Default::default(),
            text_system,
            headless_renderer_factory,
            menus: Default::default(),
            quit_requests: Cell::new(0),
            app_lifecycle_callback: Default::default(),
            memory_warning_callback: Default::default(),
            system_reduced_motion: Cell::new(false),
            button_layout: Default::default(),
            lifecycle: Default::default(),
        })
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn simulate_new_path_selection(
        &self,
        select_path: impl FnOnce(&std::path::Path) -> Option<std::path::PathBuf>,
    ) {
        let (path, tx) = self
            .prompts
            .borrow_mut()
            .new_path
            .pop_front()
            .expect("no pending new path prompt");
        tx.send(Ok(select_path(&path))).ok();
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn simulate_path_prompt_response(
        &self,
        select_paths: impl FnOnce(&PathPromptOptions) -> Option<Vec<std::path::PathBuf>>,
    ) {
        let (options, tx) = self
            .prompts
            .borrow_mut()
            .paths
            .pop_front()
            .expect("no pending paths prompt");
        let selection = select_paths(&options);
        if let Some(paths) = &selection
            && !options.multiple
            && paths.len() > 1
        {
            panic!(
                "selected {} paths for a prompt that does not allow multiple selection",
                paths.len()
            );
        }
        tx.send(Ok(selection)).ok();
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn did_prompt_for_paths(&self) -> bool {
        !self.prompts.borrow().paths.is_empty()
    }

    #[cfg(any(test, feature = "test-support"))]
    #[track_caller]
    pub(crate) fn simulate_prompt_answer(&self, response: &str) {
        let prompt = self
            .prompts
            .borrow_mut()
            .multiple_choice
            .pop_front()
            .expect("no pending multiple choice prompt");
        let Some(ix) = prompt.answers.iter().position(|a| a == response) else {
            panic!(
                "PROMPT: {}\n{:?}\n{:?}\nCannot respond with {}",
                prompt.msg, prompt.detail, prompt.answers, response
            )
        };
        prompt.tx.send(ix).ok();
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn has_pending_prompt(&self) -> bool {
        !self.prompts.borrow().multiple_choice.is_empty()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn pending_prompt(&self) -> Option<(String, String)> {
        let prompts = self.prompts.borrow();
        let prompt = prompts.multiple_choice.front()?;
        Some((
            prompt.msg.clone(),
            prompt.detail.clone().unwrap_or_default(),
        ))
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn set_screen_capture_sources(&self, sources: Vec<TestScreenCaptureSource>) {
        *self.screen_capture_sources.borrow_mut() = sources;
    }

    /// Queues the prompt so a test can later inspect or answer it through
    /// [`Self::pending_prompt`] and [`Self::simulate_prompt_answer`].
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn prompt(
        &self,
        msg: &str,
        detail: Option<&str>,
        answers: &[PromptButton],
    ) -> oneshot::Receiver<usize> {
        let (tx, rx) = oneshot::channel();
        let answers: Vec<String> = answers.iter().map(|s| s.label().to_string()).collect();
        self.prompts
            .borrow_mut()
            .multiple_choice
            .push_back(TestPrompt {
                msg: msg.to_string(),
                detail: detail.map(|s| s.to_string()),
                answers,
                tx,
            });
        rx
    }

    /// Benchmarks have no API to answer a prompt, so this doesn't retain it
    /// for later inspection; dropping the sender immediately cancels the
    /// returned receiver instead of leaving it pending indefinitely.
    #[cfg(not(any(test, feature = "test-support")))]
    pub(crate) fn prompt(
        &self,
        _msg: &str,
        _detail: Option<&str>,
        _answers: &[PromptButton],
    ) -> oneshot::Receiver<usize> {
        oneshot::channel().1
    }

    pub(crate) fn set_active_window(&self, window: Option<TestWindow>) {
        let executor = self.foreground_executor();
        let previous_window = self.active_window.borrow_mut().take();
        self.active_window.borrow_mut().clone_from(&window);

        executor
            .spawn(async move {
                if let Some(previous_window) = previous_window {
                    if let Some(window) = window.as_ref()
                        && Rc::ptr_eq(&previous_window.0, &window.0)
                    {
                        return;
                    }
                    previous_window.simulate_active_status_change(false);
                }
                if let Some(window) = window {
                    window.simulate_active_status_change(true);
                }
            })
            .detach();
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn did_prompt_for_new_path(&self) -> bool {
        !self.prompts.borrow().new_path.is_empty()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn active_cursor_style(&self) -> crate::CursorStyle {
        *self.active_cursor.lock()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn active_idle_sleep_preventions(&self) -> usize {
        self.idle_sleep_prevention_count.load(Ordering::SeqCst)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn set_idle_sleep_prevention_delay(&self, delay: Duration) {
        self.idle_sleep_prevention_delay.set(delay);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn set_idle_sleep_prevention_fails(&self, fails: bool) {
        self.idle_sleep_prevention_fails.set(fails);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn app_identity(&self) -> Option<(SharedString, SharedString)> {
        self.system_notifications.borrow().app_identity.clone()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn shown_system_notifications(&self) -> Vec<SystemNotification> {
        self.system_notifications.borrow().shown.clone()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn delivered_system_notifications(&self) -> Vec<SystemNotification> {
        self.system_notifications.borrow().delivered.clone()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn dismissed_system_notifications(&self) -> Vec<SharedString> {
        self.system_notifications.borrow().dismissed.clone()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn posted_system_notifications(&self) -> Vec<SystemNotification> {
        self.system_notifications.borrow().posted.clone()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn script_system_notification_deliveries(
        &self,
        results: impl IntoIterator<Item = SystemNotificationDelivery>,
    ) {
        self.system_notifications
            .borrow_mut()
            .scripted_deliveries
            .extend(results);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn set_system_notification_activation_supported(&self, supported: bool) {
        self.system_notifications.borrow_mut().activation_supported = Some(supported);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn simulate_system_notification_response(
        &self,
        response: SystemNotificationResponse,
    ) {
        if self.system_notifications.borrow().activation_supported == Some(false) {
            return;
        }
        let callback = self
            .system_notifications
            .borrow_mut()
            .response_callback
            .take();
        if let Some(mut callback) = callback {
            callback(response);
            self.system_notifications
                .borrow_mut()
                .response_callback
                .get_or_insert(callback);
        }
    }
}

impl Platform for TestPlatform {
    fn background_executor(&self) -> BackgroundExecutor {
        self.background_executor.clone()
    }

    fn foreground_executor(&self) -> ForegroundExecutor {
        self.foreground_executor.clone()
    }

    fn text_system(&self) -> Arc<dyn PlatformTextSystem> {
        self.text_system.clone()
    }

    fn keyboard_layout(&self) -> Box<dyn PlatformKeyboardLayout> {
        Box::new(TestKeyboardLayout)
    }

    fn keyboard_mapper(&self) -> Rc<dyn PlatformKeyboardMapper> {
        Rc::new(DummyKeyboardMapper)
    }

    fn on_keyboard_layout_change(&self, _: Box<dyn FnMut()>) {}

    fn on_thermal_state_change(&self, _: Box<dyn FnMut()>) {}

    fn thermal_state(&self) -> ThermalState {
        ThermalState::Nominal
    }

    fn prevent_idle_sleep(&self, reason: &str) -> Task<Result<ActivityGuard>> {
        let count = self.idle_sleep_prevention_count.clone();
        let fails = self.idle_sleep_prevention_fails.get();
        let reason = reason.to_owned();
        let acquire = move || {
            if fails {
                anyhow::bail!("Idle sleep prevention for {reason:?} is set to fail in this test");
            }
            count.fetch_add(1, Ordering::SeqCst);
            Ok(ActivityGuard::new(move || {
                count.fetch_sub(1, Ordering::SeqCst);
            }))
        };

        let delay = self.idle_sleep_prevention_delay.get();
        if delay.is_zero() {
            Task::ready(acquire())
        } else {
            let delay = self.background_executor.timer(delay);
            self.foreground_executor.spawn(async move {
                delay.await;
                acquire()
            })
        }
    }

    /// Launches the application by invoking the launch callback exactly once and returning
    /// immediately, which is the shape [`Application::run_embedded`](crate::Application::run_embedded)
    /// requires. The test platform owns no event loop and schedules no work of its own; tasks
    /// advance only when the harness drives the test dispatcher.
    fn run(&self, on_finish_launching: Box<dyn FnOnce()>) {
        on_finish_launching();
    }

    fn quit(&self) {
        self.quit_requests.set(self.quit_requests.get() + 1);
        self.lifecycle
            .quit_count
            .set(self.lifecycle.quit_count.get() + 1);
    }

    fn restart(&self, path: Option<PathBuf>, arguments: Vec<std::ffi::OsString>) {
        if let Some(tx) = self.expect_restart.take() {
            tx.send((path, arguments)).unwrap();
        }
    }

    fn activate(&self, _ignoring_other_apps: bool) {
        //
    }

    fn hide(&self) {
        unimplemented!()
    }

    fn hide_other_apps(&self) {
        unimplemented!()
    }

    fn unhide_other_apps(&self) {
        unimplemented!()
    }

    fn displays(&self) -> Vec<std::rc::Rc<dyn crate::PlatformDisplay>> {
        self.lifecycle.assert_display_enumeration_allowed();
        self.displays
            .borrow()
            .iter()
            .map(|display| Rc::new(display.clone()) as Rc<dyn PlatformDisplay>)
            .collect()
    }

    fn primary_display(&self) -> Option<std::rc::Rc<dyn crate::PlatformDisplay>> {
        self.lifecycle.assert_display_enumeration_allowed();
        self.displays
            .borrow()
            .first()
            .map(|display| Rc::new(display.clone()) as Rc<dyn PlatformDisplay>)
    }

    fn on_displays_changed(&self, callback: Box<dyn FnMut()>) {
        *self.displays_changed_callback.borrow_mut() = Some(callback);
    }

    fn is_screen_capture_supported(&self) -> bool {
        true
    }

    fn screen_capture_sources(
        &self,
    ) -> oneshot::Receiver<Result<Vec<Rc<dyn ScreenCaptureSource>>>> {
        let (mut tx, rx) = oneshot::channel();
        tx.send(Ok(self
            .screen_capture_sources
            .borrow()
            .iter()
            .map(|source| Rc::new(source.clone()) as Rc<dyn ScreenCaptureSource>)
            .collect()))
            .ok();
        rx
    }

    fn active_window(&self) -> Option<crate::AnyWindowHandle> {
        self.active_window
            .borrow()
            .as_ref()
            .map(|window| window.0.lock().handle)
    }

    fn open_window(
        &self,
        handle: AnyWindowHandle,
        params: WindowParams,
    ) -> anyhow::Result<Box<dyn crate::PlatformWindow>> {
        let renderer = match self.headless_renderer_factory.as_ref() {
            Some(factory) => factory()?,
            None => None,
        };
        let display = params
            .display_id
            .and_then(|id| {
                self.displays
                    .borrow()
                    .iter()
                    .find(|display| display.id() == id)
                    .map(|display| Rc::new(display.clone()) as Rc<dyn PlatformDisplay>)
            })
            .or_else(|| self.primary_display())
            .unwrap_or_else(|| Rc::new(TestDisplay::new()));
        let window = TestWindow::new(handle, params, self.weak.clone(), display, renderer);
        Ok(Box::new(window))
    }

    fn window_appearance(&self) -> WindowAppearance {
        WindowAppearance::Light
    }

    fn system_reduced_motion(&self) -> Task<bool> {
        Task::ready(self.system_reduced_motion.get())
    }

    fn button_layout(&self) -> Option<crate::WindowButtonLayout> {
        *self.button_layout.borrow()
    }

    fn open_url(&self, url: &str) {
        self.external.borrow_mut().urls.push(url.to_string());
        *self.opened_url.borrow_mut() = Some(url.to_string())
    }

    fn on_open_urls(&self, callback: Box<dyn FnMut(Vec<String>)>) {
        *self.lifecycle.open_urls.borrow_mut() = Some(callback);
    }

    /// Queues the prompt so a test can later answer it through
    /// [`Self::simulate_path_prompt_response`].
    #[cfg(any(test, feature = "test-support"))]
    fn prompt_for_paths(
        &self,
        options: crate::PathPromptOptions,
    ) -> oneshot::Receiver<Result<Option<Vec<std::path::PathBuf>>>> {
        let (tx, rx) = oneshot::channel();
        self.prompts.borrow_mut().paths.push_back((options, tx));
        rx
    }

    /// Benchmarks have no API to answer a path prompt, so this doesn't
    /// retain it for later inspection; dropping the sender immediately
    /// cancels the returned receiver instead of leaving it pending
    /// indefinitely.
    #[cfg(not(any(test, feature = "test-support")))]
    fn prompt_for_paths(
        &self,
        _options: crate::PathPromptOptions,
    ) -> oneshot::Receiver<Result<Option<Vec<std::path::PathBuf>>>> {
        oneshot::channel().1
    }

    /// Queues the prompt so a test can later answer it through
    /// [`Self::simulate_new_path_selection`].
    #[cfg(any(test, feature = "test-support"))]
    fn prompt_for_new_path(
        &self,
        directory: &std::path::Path,
        _suggested_name: Option<&str>,
        _filters: &[crate::PathPromptFilter],
    ) -> oneshot::Receiver<Result<Option<std::path::PathBuf>>> {
        let (tx, rx) = oneshot::channel();
        self.prompts
            .borrow_mut()
            .new_path
            .push_back((directory.to_path_buf(), tx));
        rx
    }

    /// Benchmarks have no API to answer a new-path prompt, so this doesn't
    /// retain it for later inspection; dropping the sender immediately
    /// cancels the returned receiver instead of leaving it pending
    /// indefinitely.
    #[cfg(not(any(test, feature = "test-support")))]
    fn prompt_for_new_path(
        &self,
        _directory: &std::path::Path,
        _suggested_name: Option<&str>,
        _filters: &[crate::PathPromptFilter],
    ) -> oneshot::Receiver<Result<Option<std::path::PathBuf>>> {
        oneshot::channel().1
    }

    fn can_select_mixed_files_and_dirs(&self) -> bool {
        true
    }

    fn reveal_path(&self, path: &std::path::Path) {
        self.external
            .borrow_mut()
            .revealed_paths
            .push(path.to_path_buf());
    }

    fn on_quit(&self, _callback: Box<dyn FnMut() -> bool>) {}

    fn on_reopen(&self, callback: Box<dyn FnMut()>) {
        *self.lifecycle.reopen.borrow_mut() = Some(callback);
    }

    fn on_activation_request(&self, callback: Box<dyn FnMut(crate::ActivationRequest)>) {
        *self.lifecycle.activation_request.borrow_mut() = Some(callback);
    }

    fn on_activate_action(&self, callback: Box<dyn FnMut(String)>) {
        *self.lifecycle.activate_action.borrow_mut() = Some(callback);
    }

    fn on_quit_requested(&self, callback: Box<dyn FnMut()>) {
        *self.lifecycle.quit_requested.borrow_mut() = Some(callback);
    }

    fn on_system_sleep(&self, _callback: Box<dyn FnMut()>) {}

    fn on_system_wake(&self, _callback: Box<dyn FnMut()>) {}

    fn on_app_lifecycle(&self, callback: Box<dyn FnMut(AppLifecyclePhase)>) {
        *self.app_lifecycle_callback.borrow_mut() = Some(callback);
    }

    fn on_memory_warning(&self, callback: Box<dyn FnMut()>) {
        *self.memory_warning_callback.borrow_mut() = Some(callback);
    }

    fn set_app_identity(&self, identifier: &str, name: &str) {
        self.system_notifications.borrow_mut().app_identity =
            Some((identifier.to_string().into(), name.to_string().into()));
    }

    fn show_system_notification(&self, notification: SystemNotification) {
        let mut system_notifications = self.system_notifications.borrow_mut();
        if system_notifications.app_identity.is_none() {
            return;
        }

        let delivered = system_notifications
            .delivered
            .iter_mut()
            .find(|delivered| delivered.tag == notification.tag);
        if let Some(delivered) = delivered {
            *delivered = notification.clone();
        } else {
            system_notifications.delivered.push(notification.clone());
        }
        system_notifications.shown.push(notification);
    }

    fn post_system_notification(
        &self,
        notification: SystemNotification,
    ) -> Task<SystemNotificationDelivery> {
        let scripted = {
            let mut system_notifications = self.system_notifications.borrow_mut();
            system_notifications.posted.push(notification.clone());
            system_notifications.scripted_deliveries.pop_front()
        };
        let delivery = scripted.unwrap_or_else(|| {
            if self.system_notifications.borrow().app_identity.is_some() {
                SystemNotificationDelivery::Delivered
            } else {
                SystemNotificationDelivery::Unavailable
            }
        });
        if delivery == SystemNotificationDelivery::Delivered {
            let mut system_notifications = self.system_notifications.borrow_mut();
            let delivered = system_notifications
                .delivered
                .iter_mut()
                .find(|delivered| delivered.tag == notification.tag);
            if let Some(delivered) = delivered {
                *delivered = notification;
            } else {
                system_notifications.delivered.push(notification);
            }
        }
        Task::ready(delivery)
    }

    fn system_notification_activation_supported(&self) -> Task<bool> {
        let supported = self
            .system_notifications
            .borrow()
            .activation_supported
            .unwrap_or(true);
        Task::ready(supported)
    }

    fn dismiss_system_notification(&self, tag: &str) {
        let mut system_notifications = self.system_notifications.borrow_mut();
        system_notifications
            .delivered
            .retain(|notification| notification.tag != tag);
        system_notifications
            .dismissed
            .push(SharedString::from(tag.to_string()));
    }

    fn on_system_notification_response(
        &self,
        callback: Box<dyn FnMut(SystemNotificationResponse)>,
    ) {
        self.system_notifications.borrow_mut().response_callback = Some(callback);
    }

    fn set_menus(&self, menus: Vec<crate::Menu>, _keymap: &Keymap) {
        *self.menus.borrow_mut() = menus.into_iter().map(|menu| menu.owned()).collect()
    }

    fn get_menus(&self) -> Option<Vec<OwnedMenu>> {
        Some(self.menus.borrow().clone())
    }

    fn set_dock_menu(&self, _menu: Vec<crate::MenuItem>, _keymap: &Keymap) {}

    fn add_recent_document(&self, _paths: &Path) {}

    fn on_app_menu_action(&self, _callback: Box<dyn FnMut(&dyn crate::Action)>) {}

    fn on_will_open_app_menu(&self, _callback: Box<dyn FnMut()>) {}

    fn on_validate_app_menu_command(&self, _callback: Box<dyn FnMut(&dyn crate::Action) -> bool>) {}

    fn app_path(&self) -> Result<std::path::PathBuf> {
        unimplemented!()
    }

    fn path_for_auxiliary_executable(&self, _name: &str) -> Result<std::path::PathBuf> {
        unimplemented!()
    }

    fn set_cursor_style(&self, style: crate::CursorStyle) {
        *self.active_cursor.lock() = style;
    }

    fn hide_cursor_until_mouse_moves(&self) {}

    fn is_cursor_visible(&self) -> bool {
        true
    }

    fn should_auto_hide_scrollbars(&self) -> bool {
        false
    }

    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        self.current_clipboard_item.lock().clone()
    }

    fn write_to_clipboard(&self, item: ClipboardItem) {
        *self.current_clipboard_item.lock() = Some(item);
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn read_from_primary(&self) -> Option<ClipboardItem> {
        self.current_primary_item.lock().clone()
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn write_to_primary(&self, item: ClipboardItem) {
        *self.current_primary_item.lock() = Some(item);
    }

    #[cfg(target_os = "macos")]
    fn read_from_find_pasteboard(&self) -> Option<ClipboardItem> {
        self.current_find_pasteboard_item.lock().clone()
    }

    #[cfg(target_os = "macos")]
    fn write_to_find_pasteboard(&self, item: ClipboardItem) {
        *self.current_find_pasteboard_item.lock() = Some(item);
    }

    fn write_credentials(&self, _url: &str, _username: &str, _password: &[u8]) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }

    fn read_credentials(&self, _url: &str) -> Task<Result<Option<(String, Vec<u8>)>>> {
        Task::ready(Ok(None))
    }

    fn delete_credentials(&self, _url: &str) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }

    fn register_url_scheme(&self, scheme: &str) -> Task<anyhow::Result<()>> {
        self.external.borrow_mut().schemes.push(scheme.to_string());
        Task::ready(Ok(()))
    }

    fn open_with_system(&self, path: &Path) {
        self.external
            .borrow_mut()
            .opened_paths
            .push(path.to_path_buf());
    }
}

impl TestScreenCaptureSource {
    /// Create a fake screen capture source, for testing.
    #[cfg(any(test, feature = "test-support"))]
    pub fn new() -> Self {
        Self {}
    }
}

struct TestKeyboardLayout;

impl PlatformKeyboardLayout for TestKeyboardLayout {
    fn id(&self) -> &str {
        "zed.keyboard.example"
    }

    fn name(&self) -> &str {
        "zed.keyboard.example"
    }
}
