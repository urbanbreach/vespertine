//! A native page: one web view in the helper, its stored state, latest frame and event channel
//! (PLT-014 R11, R13, R16; PLT-015 R5 to R12, R16).

use crate::frames::{BufferFrame, Damage, Frame, InlineFrame, Lease, Pool};
use crate::helper::{Helper, HelperShared};
use crate::protocol::{
    Command, INLINE_FRAME_HEADER_LEN, Message, Transfer, clamp_dimension, kind, parse_inline_frame,
};
use crate::state::{InputState, MenuRequest, PageEvent, PageState, Presentation};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

/// Capacity of a page's event channel (PLT-014 Parameters).
pub const EVENT_CAPACITY: usize = 64;

#[derive(Default)]
struct FrameState {
    pool: Option<Arc<Pool>>,
    pending: Option<Frame>,
}

struct Control {
    /// (width px, height px, scale bits) of the last resize command sent.
    last_resize: Option<(u32, u32, u32)>,
    /// Whether the helper was last told the page is hidden (a new page starts visible).
    hidden: bool,
    presentation: Presentation,
}

/// The shared part of a page: referenced by the helper's reader thread (through a weak route) and
/// by the UI.
pub struct PageShared {
    id: u32,
    // Keeps the helper alive while the page exists.
    helper: Arc<Helper>,
    link: Arc<HelperShared>,
    events_tx: async_channel::Sender<PageEvent>,
    events_rx: async_channel::Receiver<PageEvent>,
    state: Mutex<PageState>,
    input: Mutex<InputState>,
    frames: Mutex<FrameState>,
    control: Mutex<Control>,
    release: Arc<dyn Fn(u64, u32) + Send + Sync>,
    closed: AtomicBool,
}

impl PageShared {
    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn helper(&self) -> &Arc<Helper> {
        &self.helper
    }

    /// The channel of notifications for this page (capacity 64, shared by all receivers).
    pub fn events(&self) -> async_channel::Receiver<PageEvent> {
        self.events_rx.clone()
    }

    /// The latest state snapshot.
    pub fn state(&self) -> PageState {
        self.state.lock().clone()
    }

    /// The stored input-method state.
    pub fn input_state(&self) -> InputState {
        self.input.lock().clone()
    }

    /// Takes the single retained frame (with its frame scale and accumulated damage), leaving
    /// none. Buffer frames return their slot to the helper when dropped.
    pub fn take_frame(&self) -> Option<Frame> {
        self.frames.lock().pending.take()
    }

    /// Whether a frame is waiting to be taken.
    pub fn has_pending_frame(&self) -> bool {
        self.frames.lock().pending.is_some()
    }

    /// The pool announced most recently, if any (shared-memory transfer).
    pub fn pool(&self) -> Option<Arc<Pool>> {
        self.frames.lock().pool.clone()
    }

    /// Sends a command for this page.
    pub fn command(&self, command: Command) -> Result<(), String> {
        if self.closed.load(Ordering::Acquire) {
            return Err("The page is closed".to_owned());
        }
        self.link.send(self.id, &command)
    }

    /// Navigates to `url` (the helper enforces the scheme allow-list).
    pub fn load(&self, url: &str) -> Result<(), String> {
        self.command(Command::Load {
            url: url.to_owned(),
        })
    }

    pub fn reload(&self) -> Result<(), String> {
        self.command(Command::Reload)
    }

    /// Steps through history.
    pub fn history(&self, forward: bool) -> Result<(), String> {
        self.command(if forward {
            Command::Forward
        } else {
            Command::Back
        })
    }

    /// Applies a presentation. "visible" is sent only when the hidden/not-hidden status changes.
    pub fn present(&self, presentation: Presentation) {
        let mut control = self.control.lock();
        control.presentation = presentation;
        let hidden = presentation == Presentation::Hidden;
        if hidden != control.hidden && self.command(Command::Visible(!hidden)).is_ok() {
            control.hidden = hidden;
        }
    }

    pub fn presentation(&self) -> Presentation {
        self.control.lock().presentation
    }

    /// Sends a resize when the device-pixel size or scale of the page element changed
    /// (PLT-015 R16): width_px = clamp(round(width x scale), 1, 8192), likewise for height; the
    /// scale is passed unrounded. Returns whether a command was sent.
    pub fn sync(&self, width: f32, height: f32, scale: f32) -> bool {
        let width_px = clamp_dimension(f64::from(width) * f64::from(scale));
        let height_px = clamp_dimension(f64::from(height) * f64::from(scale));
        let triple = (width_px, height_px, scale.to_bits());
        let mut control = self.control.lock();
        if control.last_resize == Some(triple) {
            return false;
        }
        let sent = self
            .command(Command::Resize {
                width: width_px,
                height: height_px,
                scale,
            })
            .is_ok();
        if sent {
            control.last_resize = Some(triple);
        }
        sent
    }

    /// Notifies the UI; lossy notifications are dropped when the channel is full.
    fn notify(&self, event: PageEvent) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        if event.is_lossy() {
            let _ = self.events_tx.try_send(event);
        } else {
            // Data that cannot be reconstructed: wait for room (the channel is closed when the
            // page closes, which ends the wait).
            let _ = self.events_tx.send_blocking(event);
        }
    }

    /// The helper failed or hung: every live page shows `message` and is no longer loading.
    pub(crate) fn helper_failed(&self, message: &str) {
        {
            let mut state = self.state.lock();
            state.loading = false;
            state.error = Some(message.to_owned());
        }
        self.frames.lock().pending = None;
        self.notify(PageEvent::StateChanged);
    }

    fn release_slot(&self, generation: u64, slot: u32) {
        (self.release)(generation, slot);
    }

    /// Handles a message addressed to this page (called on the helper's reader thread).
    pub(crate) fn receive(&self, helper: &Arc<HelperShared>, message: Message) {
        match message.kind {
            kind::STATE => {
                let Some(object) = message.json_object() else {
                    return;
                };
                *self.state.lock() = PageState::from_snapshot(&object);
                self.notify(PageEvent::StateChanged);
            }
            kind::INPUT_METHOD => {
                let Some(object) = message.json_object() else {
                    return;
                };
                self.input.lock().merge(&object);
                self.notify(PageEvent::InputStateChanged);
            }
            kind::MENU => {
                let Some(object) = message.json_object() else {
                    return;
                };
                self.notify(PageEvent::Menu(MenuRequest::from_json(&object)));
            }
            kind::CLIPBOARD => {
                self.notify(PageEvent::Clipboard(
                    String::from_utf8_lossy(&message.payload).into_owned(),
                ));
            }
            kind::NEW_TAB => {
                self.notify(PageEvent::NewTab(
                    String::from_utf8_lossy(&message.payload).into_owned(),
                ));
            }
            kind::EVAL_RESULT => {
                self.notify(PageEvent::EvalResult(
                    String::from_utf8_lossy(&message.payload).into_owned(),
                ));
            }
            kind::FRAME => self.receive_inline_frame(message.payload),
            kind::POOL => self.receive_pool(helper, message),
            kind::BUFFER => self.receive_buffer(&message),
            _ => {}
        }
    }

    fn receive_inline_frame(&self, mut payload: Vec<u8>) {
        let Some(header) = parse_inline_frame(&payload) else {
            return;
        };
        payload.drain(..INLINE_FRAME_HEADER_LEN);
        let frame = Frame::Inline(InlineFrame {
            width: header.width,
            height: header.height,
            scale: header.scale,
            pixels: Arc::new(payload),
        });
        // Latest wins: the replaced frame (and its slot, if any) is released at once.
        let replaced = self.frames.lock().pending.replace(frame);
        drop(replaced);
        self.notify(PageEvent::FrameReady);
    }

    fn receive_pool(&self, helper: &Arc<HelperShared>, message: Message) {
        let Some(object) = message.json_object() else {
            return;
        };
        match Pool::from_announcement(&object, message.fds) {
            Ok(pool) => {
                self.frames.lock().pool = Some(Arc::new(pool));
            }
            Err(reason) => {
                log::warn!(
                    "rejected a shared-memory pool: {reason}; falling back to inline frames"
                );
                self.frames.lock().pool = None;
                helper.fall_back_to_inline();
            }
        }
    }

    fn receive_buffer(&self, message: &Message) {
        let Some(object) = message.json_object() else {
            return;
        };
        let generation = object
            .get("generation")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let slot = object
            .get("slot")
            .and_then(|v| v.as_u64())
            .unwrap_or(u64::MAX);
        let discard = |generation: u64, slot: u64| {
            if let Ok(slot) = u32::try_from(slot) {
                self.release_slot(generation, slot);
            }
        };
        let mut frames = self.frames.lock();
        let Some(pool) = frames
            .pool
            .clone()
            .filter(|pool| pool.generation == generation)
        else {
            drop(frames);
            discard(generation, slot);
            return;
        };
        let number = |key: &str| object.get(key).and_then(|v| v.as_f64());
        let unsigned = |key: &str| object.get(key).and_then(|v| v.as_u64());
        let damage = match object.get("damage").and_then(|v| v.as_array()) {
            Some(values) if values.len() == 4 => {
                let part =
                    |index: usize| values[index].as_u64().and_then(|v| u32::try_from(v).ok());
                match (part(0), part(1), part(2), part(3)) {
                    (Some(x), Some(y), Some(width), Some(height)) => Some(Damage {
                        x,
                        y,
                        width,
                        height,
                    }),
                    _ => None,
                }
            }
            _ => None,
        };
        let valid = (slot as usize) < pool.slot_count()
            && unsigned("width") == Some(u64::from(pool.width))
            && unsigned("height") == Some(u64::from(pool.height))
            && number("scale").is_some_and(|scale| scale as f32 == pool.scale)
            && damage.is_some_and(|damage| damage.fits(pool.width, pool.height));
        if !valid {
            drop(frames);
            discard(generation, slot);
            return;
        }
        let slot = slot as u32;
        let mut damage = damage.expect("validated above");
        let seq = unsigned("seq").unwrap_or(0);
        // At most one pending frame: the earlier one is dropped (its slot released at once) and
        // its damage joins the newer frame's damage.
        if let Some(Frame::Buffer(previous)) = &frames.pending
            && previous.generation() == generation
        {
            damage = damage.union(previous.damage);
        }
        let lease = Lease::new(self.release.clone(), generation, slot);
        let frame = Frame::Buffer(BufferFrame::new(pool, slot, seq, damage, lease));
        let replaced = frames.pending.replace(frame);
        drop(frames);
        drop(replaced);
        self.notify(PageEvent::FrameReady);
    }

    /// Closes the page: sends "close" (the helper destroys the web view), removes the route and
    /// releases the retained frame. Idempotent.
    pub fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let _ = self.link.send(self.id, &Command::Close);
        self.link.unregister_page(self.id);
        self.events_rx.close();
        let mut frames = self.frames.lock();
        frames.pending = None;
        frames.pool = None;
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// The current transfer mode of the page's helper.
    pub fn transfer(&self) -> Transfer {
        self.link.current_transfer()
    }
}

/// The owner of one page. Dropping it sends "close"; the helper itself keeps running.
pub struct NativePage {
    shared: Arc<PageShared>,
}

impl NativePage {
    pub(crate) fn create(
        helper: Arc<Helper>,
        link: Arc<HelperShared>,
        id: u32,
    ) -> Result<NativePage, String> {
        let (events_tx, events_rx) = async_channel::bounded(EVENT_CAPACITY);
        let weak_link = Arc::downgrade(&link);
        let release: Arc<dyn Fn(u64, u32) + Send + Sync> = Arc::new(move |generation, slot| {
            if let Some(link) = Weak::upgrade(&weak_link) {
                let _ = link.send(id, &Command::Release { generation, slot });
            }
        });
        let shared = Arc::new(PageShared {
            id,
            helper,
            link: link.clone(),
            events_tx,
            events_rx,
            state: Mutex::new(PageState::default()),
            input: Mutex::new(InputState::default()),
            frames: Mutex::new(FrameState::default()),
            control: Mutex::new(Control {
                last_resize: None,
                hidden: false,
                presentation: Presentation::Hidden,
            }),
            release,
            closed: AtomicBool::new(false),
        });
        link.register_page(&shared);
        if let Err(error) = link.send(id, &Command::Create) {
            link.unregister_page(id);
            return Err(error);
        }
        Ok(NativePage { shared })
    }

    /// A shared handle for painting and input code; it does not close the page when dropped.
    pub fn handle(&self) -> Arc<PageShared> {
        self.shared.clone()
    }
}

impl std::ops::Deref for NativePage {
    type Target = PageShared;

    fn deref(&self) -> &PageShared {
        &self.shared
    }
}

impl Drop for NativePage {
    fn drop(&mut self) {
        self.shared.close();
    }
}
