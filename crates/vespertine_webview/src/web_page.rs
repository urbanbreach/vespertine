//! The UI-thread side of one embedded page: startup lifecycle (PLT-014 R15), frame upload and
//! painting (PLT-015 R12, R13, R16 to R20), input forwarding (PLT-016) and the text-input
//! handler (PLT-016 R14).

use crate::BrowserContext;
use crate::frames::Frame;
use crate::helper::HelperConfig;
use crate::input::{
    self, KeyDownAction, PointerContext, PointerState, button_number, modifier_mask,
    relative_position,
};
use crate::page::NativePage;
use crate::protocol::Command;
use crate::state::{ERROR_OPEN_PREFIX, MenuRequest, PageEvent, PageState, Presentation};
use image::{Frame as ImageFrame, RgbaImage};
use std::cell::RefCell;
use std::ops::Range;
use std::sync::Arc;
use vespertine::{
    App, Bounds, ClipboardItem, ContentMask, Context, Corners, DevicePixels, DispatchPhase,
    Element, ElementId, ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle,
    GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId, InteractiveElement, IntoElement,
    KeyDownEvent, KeyUpEvent, LayoutId, MouseDownEvent, MouseExitEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, Point, RenderImage, Rgba, ScrollWheelEvent, Style, Styled,
    SurfaceSource, Task, UTF16Selection, Window, div, fill, px, relative, size,
};
use vespertine_wgpu::wgpu;

/// Notifications a [`WebPage`] emits to its embedder.
#[derive(Clone, Debug, PartialEq)]
pub enum WebPageEvent {
    /// The page state, presentation or startup status changed.
    Changed,
    /// The helper asked for a page-provided menu (only emitted while live).
    Menu(MenuRequest),
    /// The page asked for a new surface (only emitted while live).
    NewTab(String),
    /// A script evaluation finished (fixtures and tests).
    EvalResult(String),
}

impl EventEmitter<WebPageEvent> for WebPage {}

/// The frame currently shown: either a persistent GPU texture or a render image, plus the data
/// needed to paint it.
struct Shown {
    width: u32,
    height: u32,
    scale: f32,
    top_left: [u8; 3],
    texture: Option<Arc<wgpu::Texture>>,
    /// Tightly packed BGRA pixels kept for the image path of texture frames.
    shadow: Option<Vec<u8>>,
    image: RefCell<Option<Arc<RenderImage>>>,
}

/// Uploads taken frames and keeps the shown one; releases GPU resources of replaced frames.
#[derive(Default)]
struct FrameView {
    shown: Option<Shown>,
    slot: Option<crate::gpu::TextureSlot>,
}

fn wrap_image(width: u32, height: u32, bgra: Vec<u8>) -> Option<Arc<RenderImage>> {
    let buffer = RgbaImage::from_raw(width, height, bgra)?;
    Some(Arc::new(RenderImage::new(vec![ImageFrame::new(buffer)])))
}

impl FrameView {
    /// Replaces the shown frame with `frame`, uploading it. The previous frame's image is
    /// evicted from the window's image caches so the atlas space is reclaimed.
    fn upload(&mut self, frame: Frame, window: &mut Window) {
        let gpu = if window.gpu_device_lost() == Some(true) {
            None
        } else {
            window
                .gpu_context()
                .and_then(|context| context.downcast::<wgpu::Device, wgpu::Queue>())
        };
        let mut previous = self.shown.take();
        let top_left = frame.top_left_rgb();
        let (width, height, scale) = (frame.width(), frame.height(), frame.scale());
        let shown = match (frame, gpu) {
            (Frame::Inline(inline), _) => {
                self.slot = None;
                // The single wrap: the payload becomes the image's pixels without a copy.
                let pixels =
                    Arc::try_unwrap(inline.pixels).unwrap_or_else(|shared| (*shared).clone());
                Shown {
                    width,
                    height,
                    scale,
                    top_left,
                    texture: None,
                    shadow: None,
                    image: RefCell::new(wrap_image(width, height, pixels)),
                }
            }
            (Frame::Buffer(buffer), None) => {
                // No shared device: the image path built from the mapped pixels.
                self.slot = None;
                let packed = Frame::Buffer(buffer).packed_pixels();
                Shown {
                    width,
                    height,
                    scale,
                    top_left,
                    texture: None,
                    shadow: None,
                    image: RefCell::new(wrap_image(width, height, packed)),
                }
            }
            (Frame::Buffer(buffer), Some((device, queue))) => {
                let mut shadow = previous.as_mut().and_then(|old| old.shadow.take());
                let texture =
                    crate::gpu::upload(&mut self.slot, &device, &queue, &buffer, &mut shadow);
                // The slot goes back to the helper as soon as its pixels are copied.
                buffer.release();
                Shown {
                    width,
                    height,
                    scale,
                    top_left,
                    texture: Some(texture),
                    shadow,
                    image: RefCell::new(None),
                }
            }
        };
        if let Some(old) = previous
            && let Some(image) = old.image.into_inner()
        {
            let _ = window.drop_image(image);
        }
        self.shown = Some(shown);
    }

    /// Drops the shown frame and its GPU resources.
    fn clear(&mut self, window: &mut Window) {
        if let Some(old) = self.shown.take()
            && let Some(image) = old.image.into_inner()
        {
            let _ = window.drop_image(image);
        }
        self.slot = None;
    }

    fn is_empty(&self) -> bool {
        self.shown.is_none()
    }
}

/// Where a frame of `width` x `height` pixels captured at `scale` is drawn in an element at
/// `bounds`: its top-left at the element origin and its logical size the pixel size divided by
/// the frame scale (never stretched to the element; PLT-015 R17).
pub fn frame_placement(
    bounds: Bounds<Pixels>,
    width: u32,
    height: u32,
    scale: f32,
) -> Bounds<Pixels> {
    Bounds {
        origin: bounds.origin,
        size: size(px(width as f32 / scale), px(height as f32 / scale)),
    }
}

/// The paint path of a shown frame (PLT-015 R13, R18).
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum PaintPath {
    /// A GPU texture surface on the shared device.
    Texture,
    /// An ordinary render image.
    Image,
}

/// The texture-surface path does not support rounded clipping, so an enclosing rounded mask
/// (or a frame without a texture) forces the image path.
pub fn select_paint_path(has_texture: bool, rounded_clip: bool) -> PaintPath {
    if has_texture && !rounded_clip {
        PaintPath::Texture
    } else {
        PaintPath::Image
    }
}

/// Whether a rounded clip scope encloses the element being painted. The texture-surface path
/// does not support rounded clipping, so the image path must be used then (PLT-015 R18).
fn rounded_clip_encloses(window: &Window) -> bool {
    window.rounded_clip_active()
}

/// The UI-thread state of one embedded page. It is an entity so that the page element can be its
/// platform text-input handler.
pub struct WebPage {
    context: BrowserContext,
    focus_handle: FocusHandle,
    native: Option<NativePage>,
    startup: Option<Task<()>>,
    startup_channel: Option<async_channel::Receiver<Result<NativePage, String>>>,
    pump: Option<Task<()>>,
    requested: Option<String>,
    presentation: Presentation,
    state: PageState,
    frames: FrameView,
    pointer: PointerState,
    preedit: Option<String>,
    bounds: Option<Bounds<Pixels>>,
    scale: f32,
    focus_sent: bool,
    _subscriptions: Vec<vespertine::Subscription>,
}

impl WebPage {
    /// A page without a native page yet; the first navigation starts the helper.
    pub fn new(context: BrowserContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let subscriptions = vec![
            cx.on_focus(&focus_handle, window, |this, _window, _cx| {
                this.page_focus_changed(true)
            }),
            cx.on_blur(&focus_handle, window, |this, _window, _cx| {
                this.page_focus_changed(false)
            }),
        ];
        Self {
            context,
            focus_handle,
            native: None,
            startup: None,
            startup_channel: None,
            pump: None,
            requested: None,
            presentation: Presentation::Hidden,
            state: PageState::default(),
            frames: FrameView::default(),
            pointer: PointerState::default(),
            preedit: None,
            bounds: None,
            scale: 1.0,
            focus_sent: false,
            _subscriptions: subscriptions,
        }
    }

    /// The configuration used to start helpers.
    pub fn helper_config(&self) -> &HelperConfig {
        self.context.config()
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    /// The merged page state (CMP-022 R6).
    pub fn state(&self) -> &PageState {
        &self.state
    }

    /// The latest address the surface was asked to show.
    pub fn requested_address(&self) -> Option<&str> {
        self.requested.as_deref()
    }

    pub fn presentation(&self) -> Presentation {
        self.presentation
    }

    /// Whether a native page exists.
    pub fn has_native_page(&self) -> bool {
        self.native.is_some()
    }

    /// Whether a startup is in flight.
    pub fn startup_pending(&self) -> bool {
        self.startup.is_some()
    }

    /// The native page, if any.
    pub fn native_page(&self) -> Option<&NativePage> {
        self.native.as_ref()
    }

    /// Whether a frame is shown.
    pub fn has_frame(&self) -> bool {
        !self.frames.is_empty()
    }

    /// The size and frame scale of the shown frame.
    pub fn frame_size(&self) -> Option<(u32, u32, f32)> {
        self.frames
            .shown
            .as_ref()
            .map(|shown| (shown.width, shown.height, shown.scale))
    }

    /// The top-left pixel colour of the shown frame (the backing fill).
    pub fn frame_backing(&self) -> Option<[u8; 3]> {
        self.frames.shown.as_ref().map(|shown| shown.top_left)
    }

    /// The stored input-method "composition" string, if marked text is active.
    pub fn preedit(&self) -> Option<&str> {
        self.preedit.as_deref().filter(|text| !text.is_empty())
    }

    fn usable(&self) -> bool {
        self.native
            .as_ref()
            .is_some_and(|page| !page.is_closed() && !page.helper().is_unusable())
    }

    /// Input reaches the page only while it is live and a native page exists (PLT-016 R1).
    fn eligible(&self) -> bool {
        self.presentation == Presentation::Live && self.usable()
    }

    fn send(&self, command: Command) {
        if let Some(page) = &self.native {
            let _ = page.command(command);
        }
    }

    fn send_all(&self, commands: Vec<Command>) {
        for command in commands {
            self.send(command);
        }
    }

    // ---- lifecycle --------------------------------------------------------------------------

    /// Navigates to an already validated address (CMP-022 R7 after normalization).
    ///
    /// The state shows the address with a cleared title and error. With a usable native page it
    /// is told to load; otherwise startup begins (a pending startup only updates the requested
    /// address, so the page loads the latest one exactly once when startup completes). Keyboard
    /// focus moves to the page target.
    pub fn navigate_to(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        self.requested = Some(url.clone());
        self.state.url = Some(url.clone());
        self.state.title.clear();
        self.state.error = None;
        self.state.loading = true;
        if self.native.is_some() && !self.usable() {
            // The page's helper failed or hung: restart through startup (PLT-014 R13, R15).
            self.drop_native(window);
        }
        if let Some(page) = &self.native {
            if let Err(error) = page.load(&url) {
                self.state.loading = false;
                self.state.error = Some(format!("{ERROR_OPEN_PREFIX}{error}"));
            }
        } else if self.startup.is_none() {
            self.start_startup(window, cx);
        }
        window.focus(&self.focus_handle, cx);
        cx.emit(WebPageEvent::Changed);
        cx.notify();
    }

    /// Reloads (CMP-022 R9): with an error the state's address is loaded again, otherwise the
    /// engine reloads; without a native page (startup failed, helper stopped) startup restarts
    /// with the current address. The error is cleared when the attempt begins.
    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(url) = self.state.url.clone().or_else(|| self.requested.clone()) else {
            return;
        };
        if self.native.is_some() && !self.usable() {
            self.drop_native(window);
        }
        let had_error = self.state.error.is_some();
        self.state.error = None;
        match &self.native {
            Some(page) => {
                self.state.loading = true;
                let result = if had_error {
                    page.load(&url)
                } else {
                    page.reload()
                };
                if let Err(error) = result {
                    self.state.loading = false;
                    self.state.error = Some(format!("{ERROR_OPEN_PREFIX}{error}"));
                }
            }
            None => {
                self.requested = Some(url);
                self.state.loading = true;
                if self.startup.is_none() {
                    self.start_startup(window, cx);
                }
            }
        }
        cx.emit(WebPageEvent::Changed);
        cx.notify();
    }

    /// Steps through history; no-op without a usable native page.
    pub fn history(&mut self, forward: bool) {
        if self.usable()
            && let Some(page) = &self.native
        {
            let _ = page.history(forward);
        }
    }

    /// Closes the page (CMP-022 R17): hidden, frame evicted, pending startup cancelled (its late
    /// completion cannot reattach; the page it created is closed on the helper) and the native
    /// page dropped (sending "close"). A later navigation starts again.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_presentation(Presentation::Hidden, window, cx);
        self.cancel_startup();
        self.drop_native(window);
        self.state.loading = false;
        self.frames.clear(window);
        cx.emit(WebPageEvent::Changed);
        cx.notify();
    }

    /// Abandons a pending startup: the channel closes at once, so a late completion can never
    /// reattach; the startup thread closes the page it created.
    fn cancel_startup(&mut self) {
        self.startup = None;
        if let Some(channel) = self.startup_channel.take() {
            channel.close();
        }
    }

    fn drop_native(&mut self, _window: &mut Window) {
        self.pump = None;
        self.native = None;
        self.preedit = None;
        self.focus_sent = false;
        self.pointer = PointerState::default();
    }

    fn start_startup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = self.context.start_page();
        self.startup_channel = Some(receiver.clone());
        self.startup = Some(cx.spawn_in(window, async move |this, cx| {
            let result = receiver.recv().await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.startup = None;
                this.startup_channel = None;
                match result {
                    Ok(Ok(page)) => this.adopt(page, window, cx),
                    Ok(Err(message)) => this.startup_failed(message, cx),
                    Err(_) => this
                        .startup_failed("The browser startup stopped unexpectedly".to_owned(), cx),
                }
            });
        }));
    }

    fn startup_failed(&mut self, message: String, cx: &mut Context<Self>) {
        self.state.loading = false;
        self.state.error = Some(format!("{ERROR_OPEN_PREFIX}{message}"));
        cx.emit(WebPageEvent::Changed);
        cx.notify();
    }

    /// Takes over a freshly created native page: applies the current presentation, then loads
    /// the latest requested address (PLT-014 R15 (a), (e)).
    pub fn adopt(&mut self, page: NativePage, window: &mut Window, cx: &mut Context<Self>) {
        self.drop_native(window);
        page.present(self.presentation);
        let events = page.events();
        self.native = Some(page);
        self.pump = Some(cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this
                    .update_in(cx, |this, window, cx| {
                        this.handle_page_event(event, window, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
        if self.eligible() && self.focus_handle.is_focused(window) {
            self.page_focus_changed(true);
        }
        if let Some(url) = self.requested.clone()
            && let Some(page) = &self.native
        {
            if let Err(error) = page.load(&url) {
                self.state.loading = false;
                self.state.error = Some(format!("{ERROR_OPEN_PREFIX}{error}"));
            } else {
                self.state.loading = true;
            }
        }
        cx.emit(WebPageEvent::Changed);
        cx.notify();
    }

    fn handle_page_event(&mut self, event: PageEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.refresh(window, cx);
        match event {
            PageEvent::StateChanged | PageEvent::FrameReady => {}
            PageEvent::InputStateChanged => cx.notify(),
            PageEvent::Menu(menu) => {
                if self.presentation == Presentation::Live {
                    cx.emit(WebPageEvent::Menu(menu));
                } else {
                    self.send(Command::DismissMenu);
                }
            }
            PageEvent::NewTab(url) => {
                if self.presentation == Presentation::Live {
                    cx.emit(WebPageEvent::NewTab(url));
                }
            }
            PageEvent::Clipboard(text) => {
                // An empty result must not modify the system clipboard.
                if !text.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }
            PageEvent::EvalResult(result) => cx.emit(WebPageEvent::EvalResult(result)),
        }
    }

    /// Reads the latest snapshot and takes the latest frame (PLT-015 R12; CMP-022 R8).
    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = &self.native else {
            return;
        };
        let snapshot = page.state();
        let frame = page.take_frame();
        let mut next = self.state.clone();
        // An absent url keeps the requested address.
        if snapshot.url.is_some() {
            next.url = snapshot.url;
        }
        next.title = snapshot.title;
        next.error = snapshot.error;
        next.loading = snapshot.loading && next.error.is_none();
        next.can_back = snapshot.can_back;
        next.can_forward = snapshot.can_forward;
        let changed = next != self.state;
        self.state = next;
        if let Some(frame) = frame {
            self.frames.upload(frame, window);
            cx.notify();
        }
        if changed {
            cx.emit(WebPageEvent::Changed);
            cx.notify();
        }
    }

    // ---- presentation -----------------------------------------------------------------------

    /// Applies a presentation (PLT-015 R20): "visible" is sent only when the hidden status
    /// changes; leaving live releases held buttons and the hover.
    pub fn set_presentation(
        &mut self,
        presentation: Presentation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.presentation == presentation {
            return;
        }
        let was_live = self.eligible();
        self.presentation = presentation;
        if let Some(page) = &self.native {
            page.present(presentation);
        }
        if was_live && presentation != Presentation::Live {
            let commands = self.pointer.leave_live();
            self.send_all(commands);
            self.preedit = None;
        }
        if presentation == Presentation::Live
            && self.eligible()
            && self.focus_handle.is_focused(window)
        {
            self.page_focus_changed(true);
        }
        cx.emit(WebPageEvent::Changed);
        cx.notify();
    }

    /// Dismisses a pending page-provided menu without choosing.
    pub fn dismiss_menu(&self) {
        self.send(Command::DismissMenu);
    }

    /// Sends a command to the native page (menu actions, evaluation); returns whether it was
    /// accepted for sending.
    pub fn send_command(&self, command: Command) -> bool {
        self.native
            .as_ref()
            .is_some_and(|page| page.command(command).is_ok())
    }

    // ---- focus ------------------------------------------------------------------------------

    fn page_focus_changed(&mut self, focused: bool) {
        if focused {
            if self.eligible() && !self.focus_sent {
                self.focus_sent = true;
                self.send(Command::Focus(true));
            }
        } else {
            if self.preedit.take().is_some() || self.focus_sent {
                self.send(Command::Unmark);
            }
            if self.focus_sent {
                self.focus_sent = false;
                self.send(Command::Focus(false));
            }
        }
    }

    // ---- pointer ----------------------------------------------------------------------------

    fn pointer_context(&self, cx: &App, hovered: bool) -> PointerContext {
        PointerContext {
            eligible: self.eligible(),
            dragging: cx.has_active_drag(),
            hovered,
        }
    }

    fn origin(&self) -> Point<Pixels> {
        self.bounds.map(|bounds| bounds.origin).unwrap_or_default()
    }

    /// A mouse press; returns whether the page consumed it.
    pub fn pointer_down(
        &mut self,
        event: &MouseDownEvent,
        hovered: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let context = self.pointer_context(cx, hovered);
        let position = relative_position(event.position, self.origin());
        let outcome = self.pointer.press(
            context,
            button_number(event.button),
            position,
            modifier_mask(&event.modifiers),
        );
        let forwarded = !outcome.commands.is_empty();
        self.send_all(outcome.commands);
        if forwarded {
            // A press on the page gives the surface keyboard focus.
            window.focus(&self.focus_handle, cx);
        }
        outcome.consumed
    }

    /// A mouse release; returns whether the page consumed it.
    pub fn pointer_up(&mut self, event: &MouseUpEvent) -> bool {
        let position = relative_position(event.position, self.origin());
        let outcome = self.pointer.release(
            button_number(event.button),
            position,
            modifier_mask(&event.modifiers),
        );
        self.send_all(outcome.commands);
        outcome.consumed
    }

    /// A mouse move.
    pub fn pointer_moved(&mut self, event: &MouseMoveEvent, hovered: bool, cx: &mut Context<Self>) {
        let context = self.pointer_context(cx, hovered);
        let position = relative_position(event.position, self.origin());
        let commands = self
            .pointer
            .moved(context, position, modifier_mask(&event.modifiers));
        self.send_all(commands);
    }

    /// The pointer left the window.
    pub fn pointer_exited(&mut self, _event: &MouseExitEvent) {
        let commands = self.pointer.exited();
        self.send_all(commands);
    }

    /// A wheel event; returns whether the page consumed it (no scroll chaining).
    pub fn wheel(
        &mut self,
        event: &ScrollWheelEvent,
        hovered: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let context = self.pointer_context(cx, hovered);
        let position = relative_position(event.position, self.origin());
        let outcome = self.pointer.scrolled(
            context,
            position,
            &event.delta,
            modifier_mask(&event.modifiers),
        );
        self.send_all(outcome.commands);
        outcome.consumed
    }

    // ---- keyboard and clipboard -------------------------------------------------------------

    /// A key press while the page target has focus; returns whether it was swallowed.
    pub fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        if !self.eligible() {
            return false;
        }
        let accepts_text_input = self
            .native
            .as_ref()
            .is_some_and(|page| page.input_state().focused);
        match input::classify_key_down(
            &event.keystroke.key,
            event.keystroke.key_char.as_deref(),
            &event.keystroke.modifiers,
            accepts_text_input,
        ) {
            KeyDownAction::Commit(text) => {
                self.preedit = None;
                self.send(Command::Commit(text));
            }
            KeyDownAction::Copy => self.send(Command::Copy),
            KeyDownAction::Cut => self.send(Command::Cut),
            KeyDownAction::Paste => self.paste(cx),
            KeyDownAction::Key(command) => self.send(command),
        }
        true
    }

    /// A key release while the page target has focus; returns whether it was swallowed.
    pub fn key_up(&mut self, event: &KeyUpEvent) -> bool {
        if !self.eligible() {
            return false;
        }
        self.send(input::key_up_command(
            &event.keystroke.key,
            event.keystroke.key_char.as_deref(),
            &event.keystroke.modifiers,
        ));
        true
    }

    /// Pastes the system clipboard's plain text; nothing is sent when there is none.
    pub fn paste(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text())
            && !text.is_empty()
        {
            self.send(Command::Text(text));
        }
    }

    // ---- painting ---------------------------------------------------------------------------

    /// Called on every paint of the page element: remembers the element bounds and sends a
    /// resize when the device size or scale changed (PLT-015 R16), using the full element
    /// bounds even when the page is partially clipped.
    fn painted(&mut self, bounds: Bounds<Pixels>, scale: f32) {
        self.bounds = Some(bounds);
        self.scale = scale;
        if let Some(page) = &self.native {
            page.sync(
                bounds.size.width.as_f32(),
                bounds.size.height.as_f32(),
                scale,
            );
        }
    }

    fn paint_frame(&self, bounds: Bounds<Pixels>, window: &mut Window) {
        let Some(shown) = &self.frames.shown else {
            return;
        };
        let backing = Rgba {
            r: f32::from(shown.top_left[0]) / 255.0,
            g: f32::from(shown.top_left[1]) / 255.0,
            b: f32::from(shown.top_left[2]) / 255.0,
            a: 1.0,
        };
        let frame_bounds = frame_placement(bounds, shown.width, shown.height, shown.scale);
        let use_texture = select_paint_path(shown.texture.is_some(), rounded_clip_encloses(window))
            == PaintPath::Texture;
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            // The page's own background fills any gap while the page reflows; the frame is
            // never stretched.
            window.paint_quad(fill(bounds, backing));
            if use_texture {
                let texture = shown.texture.clone().expect("checked above");
                window.paint_surface(
                    frame_bounds,
                    SurfaceSource::gpu_texture(
                        texture,
                        size(
                            DevicePixels(shown.width as i32),
                            DevicePixels(shown.height as i32),
                        ),
                    ),
                );
            } else {
                let image = {
                    let mut cached = shown.image.borrow_mut();
                    if cached.is_none()
                        && let Some(shadow) = &shown.shadow
                    {
                        *cached = wrap_image(shown.width, shown.height, shadow.clone());
                    }
                    cached.clone()
                };
                if let Some(image) = image {
                    let _ = window.paint_image(
                        bounds,
                        frame_bounds,
                        Corners::default(),
                        image,
                        0,
                        false,
                    );
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rounded_enclosing_mask_forces_the_image_path() {
        assert_eq!(select_paint_path(true, false), PaintPath::Texture);
        assert_eq!(select_paint_path(true, true), PaintPath::Image);
        assert_eq!(select_paint_path(false, false), PaintPath::Image);
    }

    #[test]
    fn frames_are_placed_at_their_physical_size() {
        let element = Bounds {
            origin: Point {
                x: px(40.),
                y: px(80.),
            },
            size: size(px(640.), px(360.)),
        };
        assert_eq!(
            frame_placement(element, 960, 540, 1.5).size,
            size(px(640.), px(360.))
        );
        assert_eq!(
            frame_placement(element, 960, 540, 2.0).size,
            size(px(480.), px(270.))
        );
        assert_eq!(
            frame_placement(element, 960, 540, 1.5).origin,
            element.origin
        );
    }
}

// ---- text input handler (PLT-016 R14) -----------------------------------------------------

impl EntityInputHandler for WebPage {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let state = self.native.as_ref()?.input_state();
        let text = state.text.unwrap_or_default();
        let (cut, clamped) = input::text_in_utf16_range(&text, range);
        *adjusted_range = Some(clamped);
        Some(cut)
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let state = self.native.as_ref()?.input_state();
        let (range, reversed) = input::selected_range_of(&state);
        Some(UTF16Selection { range, reversed })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.preedit().map(|text| 0..input::utf16_len(text))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        if !self.eligible() {
            return;
        }
        self.preedit = None;
        self.send(Command::Unmark);
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        if !self.eligible() {
            return;
        }
        self.preedit = None;
        self.send(Command::Commit(text.to_owned()));
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        if !self.eligible() {
            return;
        }
        self.preedit = Some(new_text.to_owned());
        self.send(Command::Preedit(new_text.to_owned()));
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let caret = self
            .native
            .as_ref()
            .and_then(|page| page.input_state().caret);
        Some(input::caret_bounds(
            element_bounds.origin,
            caret,
            self.scale,
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }

    fn accepts_text_input(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        // Only while input reaches the page, and only for an editable focus: non-editable pages
        // keep receiving plain key events.
        self.eligible()
            && self
                .native
                .as_ref()
                .is_some_and(|page| page.input_state().focused)
    }
}

// ---- the page element ------------------------------------------------------------------------

/// The element that paints the page and registers its input listeners. Use [`page_element`].
struct PageCanvas {
    page: Entity<WebPage>,
}

impl IntoElement for PageCanvas {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for PageCanvas {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn request_layout(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size = size(relative(1.).into(), relative(1.).into());
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _layout: &mut (),
        window: &mut Window,
        _cx: &mut App,
    ) -> Hitbox {
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _layout: &mut (),
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        let scale = window.scale_factor();
        self.page.update(cx, |page, _| page.painted(bounds, scale));
        self.page.read(cx).paint_frame(bounds, window);

        let focus_handle = self.page.read(cx).focus_handle.clone();
        // While the page target is focused the element is the platform text-input handler.
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.page.clone()),
            cx,
        );

        let page = self.page.clone();
        let hitbox = hitbox.clone();
        window.on_mouse_event({
            let page = page.clone();
            let hitbox = hitbox.clone();
            move |event: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let hovered = hitbox.is_hovered(window);
                if page.update(cx, |page, cx| page.pointer_down(event, hovered, window, cx)) {
                    cx.stop_propagation();
                }
            }
        });
        window.on_mouse_event({
            let page = page.clone();
            move |event: &MouseUpEvent, phase, _window, cx| {
                if phase != DispatchPhase::Capture {
                    return;
                }
                if page.update(cx, |page, _| page.pointer_up(event)) {
                    cx.stop_propagation();
                }
            }
        });
        window.on_mouse_event({
            let page = page.clone();
            let hitbox = hitbox.clone();
            move |event: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture {
                    return;
                }
                let hovered = hitbox.is_hovered(window);
                page.update(cx, |page, cx| page.pointer_moved(event, hovered, cx));
            }
        });
        window.on_mouse_event({
            let page = page.clone();
            move |event: &MouseExitEvent, phase, _window, cx| {
                if phase != DispatchPhase::Capture {
                    return;
                }
                page.update(cx, |page, _| page.pointer_exited(event));
            }
        });
        window.on_mouse_event({
            move |event: &ScrollWheelEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let hovered = hitbox.is_hovered(window);
                if page.update(cx, |page, cx| page.wheel(event, hovered, cx)) {
                    cx.stop_propagation();
                }
            }
        });
    }
}

/// The page element: a focusable box that fills its parent, paints the page's latest frame and
/// forwards pointer, wheel, key and composition input while the surface is live.
pub fn page_element(page: &Entity<WebPage>, cx: &App) -> impl IntoElement {
    let focus_handle = page.read(cx).focus_handle.clone();
    let key_down = page.clone();
    let key_up = page.clone();
    div()
        .id("web-page")
        .size_full()
        .relative()
        .overflow_hidden()
        .track_focus(&focus_handle)
        .on_key_down(move |event, _window, cx| {
            if key_down.update(cx, |page, cx| page.key_down(event, cx)) {
                cx.stop_propagation();
            }
        })
        .on_key_up(move |event, _window, cx| {
            if key_up.update(cx, |page, _| page.key_up(event)) {
                cx.stop_propagation();
            }
        })
        .child(PageCanvas { page: page.clone() })
}
