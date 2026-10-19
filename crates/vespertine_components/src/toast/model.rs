//! Pure model of the toast stack (CMP-018): the notice queue with its pausable countdowns,
//! the stack geometry, the retargetable glides and the swipe policy. Time is an explicit
//! millisecond argument everywhere, so every rule is testable with a controllable clock.

use vespertine::SharedString;

/// Remaining lifetime of a fresh (or restarted) notice.
pub const LIFETIME_MS: f64 = 4000.0;
/// Time a closing notice stays before it leaves the queue (and the exit fade).
pub const EXIT_MS: f64 = 200.0;
/// Duration of glides and of the entry animation.
pub const GLIDE_MS: f64 = 400.0;
/// Nominal stack width.
pub const NOMINAL_WIDTH: f32 = 356.0;
/// Distance between the stack and the bottom and right window edges.
pub const EDGE_MARGIN: f32 = 24.0;
/// Collapsed vertical step and fan-out gap.
pub const STEP: f32 = 14.0;
/// Depths below this are visible while collapsed.
pub const VISIBLE_DEPTH: usize = 3;
/// Height used before any card is measured.
pub const FALLBACK_HEIGHT: f32 = 52.0;
/// Measured-height changes at most this large are ignored.
pub const MEASURE_EPSILON: f32 = 0.5;
/// Targets that move less than this do not restart a glide.
pub const RETARGET_EPSILON: f32 = 0.01;
/// Rightward swipe displacement that dismisses a card.
pub const SWIPE_DISMISS: f32 = 45.0;
/// Fraction of the nominal width over which a swipe fades the card out.
pub const SWIPE_FADE_FRACTION: f32 = 0.6;
/// Overlay priority of the host in the deferred-drawing order.
pub const OVERLAY_PRIORITY: usize = 4;
/// Opacity at or below which a deep, non-closing card is skipped.
pub const HIDDEN_OPACITY: f32 = 0.001;

/// The kind of a notice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToastKind {
    /// A failure.
    Error,
    /// A success.
    Success,
    /// Neutral information.
    Info,
}

/// The easing curve `cubic-bezier(0.215, 0.61, 0.355, 1)` of every glide, entry and fan-out.
pub fn ease(t: f32) -> f32 {
    const X1: f32 = 0.215;
    const Y1: f32 = 0.61;
    const X2: f32 = 0.355;
    const Y2: f32 = 1.0;
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    // Solve x(s) = t for the curve parameter s by bisection, then evaluate y(s).
    let bezier = |s: f32, a: f32, b: f32| {
        let inverse = 1.0 - s;
        3.0 * inverse * inverse * s * a + 3.0 * inverse * s * s * b + s * s * s
    };
    let (mut low, mut high) = (0.0_f32, 1.0_f32);
    for _ in 0..40 {
        let mid = (low + high) / 2.0;
        if bezier(mid, X1, X2) < t {
            low = mid;
        } else {
            high = mid;
        }
    }
    bezier((low + high) / 2.0, Y1, Y2)
}

/// A retargetable glide: restarts from its current value toward a new target over 400 ms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glide {
    from: f32,
    to: f32,
    start_ms: f64,
}

impl Glide {
    /// A glide at rest on `value` (the first layout of a card snaps to its target).
    pub fn at(value: f32) -> Self {
        Self {
            from: value,
            to: value,
            start_ms: 0.0,
        }
    }

    /// The value at `now_ms`; with reduced motion always the target.
    pub fn value(&self, now_ms: f64, reduced_motion: bool) -> f32 {
        if reduced_motion || self.from == self.to {
            return self.to;
        }
        let t = ((now_ms - self.start_ms) / GLIDE_MS).clamp(0.0, 1.0) as f32;
        if t >= 1.0 {
            return self.to;
        }
        self.from + (self.to - self.from) * ease(t)
    }

    /// The target.
    pub fn target(&self) -> f32 {
        self.to
    }

    /// Whether the glide is still moving.
    pub fn is_animating(&self, now_ms: f64, reduced_motion: bool) -> bool {
        !reduced_motion && self.from != self.to && now_ms - self.start_ms < GLIDE_MS
    }

    /// Restarts toward `to` from the current value. A change smaller than the retarget epsilon
    /// is ignored; with reduced motion the value snaps.
    pub fn retarget(&mut self, to: f32, now_ms: f64, reduced_motion: bool) {
        if (to - self.to).abs() < RETARGET_EPSILON {
            return;
        }
        self.from = if reduced_motion {
            to
        } else {
            self.value(now_ms, false)
        };
        self.to = to;
        self.start_ms = now_ms;
    }
}

/// One notice.
#[derive(Clone, Debug)]
pub struct Toast {
    /// Monotonically increasing identity.
    pub id: u64,
    /// The kind.
    pub kind: ToastKind,
    /// The trimmed message.
    pub message: SharedString,
    /// Creation time.
    pub created_ms: f64,
    remaining_ms: f64,
    running_since: Option<f64>,
    closing_at: Option<f64>,
    measured: Option<f32>,
    swipe: f32,
    press_x: Option<f32>,
    offset: Option<Glide>,
    clip: Option<Glide>,
}

impl Toast {
    /// Whether the notice began closing.
    pub fn is_closing(&self) -> bool {
        self.closing_at.is_some()
    }

    /// Whether its countdown is running.
    pub fn is_running(&self) -> bool {
        self.running_since.is_some()
    }

    /// The lifetime left at `now_ms` (the banked remainder minus the running interval).
    pub fn remaining_at(&self, now_ms: f64) -> f64 {
        match self.running_since {
            Some(since) => (self.remaining_ms - (now_ms - since)).max(0.0),
            None => self.remaining_ms,
        }
    }

    /// The horizontal swipe displacement.
    pub fn swipe(&self) -> f32 {
        self.swipe
    }

    /// The measured natural height, once known.
    pub fn measured_height(&self) -> Option<f32> {
        self.measured
    }

    fn pause(&mut self, now_ms: f64) {
        if let Some(since) = self.running_since.take() {
            self.remaining_ms = (self.remaining_ms - (now_ms - since)).max(0.0);
        }
    }

    fn resume(&mut self, now_ms: f64) {
        if self.running_since.is_none() && self.closing_at.is_none() {
            self.running_since = Some(now_ms);
        }
    }

    fn close(&mut self, now_ms: f64) {
        self.running_since = None;
        self.closing_at = Some(now_ms);
    }
}

/// The swipe fade multiplier of a displacement: `clamp(1 - d / (356 x 0.6), 0, 1)`.
pub fn swipe_opacity(displacement: f32) -> f32 {
    (1.0 - displacement / (NOMINAL_WIDTH * SWIPE_FADE_FRACTION)).clamp(0.0, 1.0)
}

/// The width of the stack in a viewport of `viewport_width`: the nominal width, reduced to the
/// viewport minus two margins when that is smaller, never below 0.
pub fn stack_width(viewport_width: f32) -> f32 {
    NOMINAL_WIDTH
        .min(viewport_width - 2.0 * EDGE_MARGIN)
        .max(0.0)
}

/// The stack's frame `(x, y, width, height)` in a viewport: its bottom-right corner 24 px from
/// the bottom and right edges.
pub fn stack_frame(
    viewport_width: f32,
    viewport_height: f32,
    stack_height: f32,
) -> (f32, f32, f32, f32) {
    let width = stack_width(viewport_width);
    (
        viewport_width - EDGE_MARGIN - width,
        viewport_height - EDGE_MARGIN - stack_height,
        width,
        stack_height,
    )
}

/// The collapsed/fanned targets of one live card.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CardTarget {
    /// Vertical offset from the stack bottom, at fan-out progress `e`.
    pub offset: f32,
    /// Clip (visible) height at `e`.
    pub clip: f32,
}

/// The targets of the live cards (newest first, `heights[0]` is the front) at fan-out
/// progress `e`: offset `c_i + (f_i - c_i) e` with `c_i = 14 i` and `f_i = sum (h_j + 14)`;
/// clip `H_0 + (h_i - H_0) e`.
pub fn stack_targets(heights: &[f32], e: f32) -> Vec<CardTarget> {
    let front = heights.first().copied().unwrap_or(0.0);
    let mut fanned = 0.0;
    heights
        .iter()
        .enumerate()
        .map(|(i, &height)| {
            let collapsed = i as f32 * STEP;
            let target = CardTarget {
                offset: collapsed + (fanned - collapsed) * e,
                clip: front + (height - front) * e,
            };
            fanned += height + STEP;
            target
        })
        .collect()
}

/// The horizontal inset of each side of a card at `depth` with fan-out progress `e`.
pub fn card_inset(depth: usize, e: f32) -> f32 {
    NOMINAL_WIDTH * 0.05 * 0.5 * depth.min(VISIBLE_DEPTH) as f32 * (1.0 - e)
}

/// The depth opacity: depth 3 and beyond are invisible collapsed and fully visible fanned.
pub fn depth_opacity(depth: usize, e: f32) -> f32 {
    if depth >= VISIBLE_DEPTH { e } else { 1.0 }
}

/// What a host does to keep the stack moving.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WakePlan {
    /// Something animates: ask for animation frames.
    Frames,
    /// Nothing animates: one timer for the earliest deadline.
    At(f64),
    /// No notices.
    Idle,
}

/// The frame of one card, ready to paint (oldest first in [`ToasterFrame::cards`]).
#[derive(Clone, Debug, PartialEq)]
pub struct CardFrame {
    /// Identity.
    pub id: u64,
    /// Kind.
    pub kind: ToastKind,
    /// Message.
    pub message: SharedString,
    /// Depth in the live ordering (closing cards use 0).
    pub depth: usize,
    /// Whether the card is closing.
    pub closing: bool,
    /// Distance from the stack bottom to the card's bottom edge, including entry and exit
    /// motion (positive is upward).
    pub bottom: f32,
    /// The card's box height.
    pub clip: f32,
    /// Inset of each side.
    pub inset: f32,
    /// Horizontal swipe displacement.
    pub swipe: f32,
    /// Final opacity: depth x entry x exit x swipe.
    pub opacity: f32,
    /// Whether the card is skipped for painting.
    pub skipped: bool,
}

/// The frame of the whole stack.
#[derive(Clone, Debug, PartialEq)]
pub struct ToasterFrame {
    /// Fan-out progress.
    pub fan: f32,
    /// Stack height.
    pub height: f32,
    /// Cards, oldest first (paint order).
    pub cards: Vec<CardFrame>,
    /// Whether any card, glide or the fan-out is still animating.
    pub animating: bool,
}

/// The process-wide notice queue (R1-R4, R8-R11, R14). One instance lives in the application
/// as a global; every window's host renders it.
pub struct ToastQueue {
    toasts: Vec<Toast>,
    next_id: u64,
    hovered: bool,
    active: bool,
    fan: Glide,
}

impl Default for ToastQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl ToastQueue {
    /// An empty queue.
    pub fn new() -> Self {
        Self {
            toasts: Vec::new(),
            next_id: 1,
            hovered: false,
            active: true,
            fan: Glide::at(0.0),
        }
    }

    /// Publishes a notice. The message is trimmed; an empty message is ignored (nothing is
    /// queued or refreshed) and `None` returned. When the newest live notice has the same kind
    /// and message no card is added: its lifetime is reset to 4000 ms and its countdown
    /// restarts (running unless the stack is hovered). Returns the identity of the added or
    /// refreshed notice.
    pub fn publish(&mut self, kind: ToastKind, message: &str, now_ms: f64) -> Option<u64> {
        let message = message.trim();
        if message.is_empty() {
            return None;
        }
        let runs = !self.hovered && self.active;
        if let Some(front) = self
            .toasts
            .iter_mut()
            .rev()
            .find(|toast| !toast.is_closing())
            && front.kind == kind
            && front.message.as_ref() == message
        {
            front.remaining_ms = LIFETIME_MS;
            front.running_since = runs.then_some(now_ms);
            return Some(front.id);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.toasts.push(Toast {
            id,
            kind,
            message: message.to_owned().into(),
            created_ms: now_ms,
            remaining_ms: LIFETIME_MS,
            running_since: runs.then_some(now_ms),
            closing_at: None,
            measured: None,
            swipe: 0.0,
            press_x: None,
            offset: None,
            clip: None,
        });
        Some(id)
    }

    /// Whether any notice exists.
    pub fn is_empty(&self) -> bool {
        self.toasts.is_empty()
    }

    /// All notices, oldest first (closing ones included).
    pub fn toasts(&self) -> &[Toast] {
        &self.toasts
    }

    /// The notice with identity `id`.
    pub fn get(&self, id: u64) -> Option<&Toast> {
        self.toasts.iter().find(|toast| toast.id == id)
    }

    /// The live (non-closing) message texts, oldest first. A test accessor.
    pub fn live_messages(&self) -> Vec<SharedString> {
        self.toasts
            .iter()
            .filter(|toast| !toast.is_closing())
            .map(|toast| toast.message.clone())
            .collect()
    }

    /// Whether the stack is hovered.
    pub fn is_hovered(&self) -> bool {
        self.hovered
    }

    /// Sets the hover state: hovering pauses every countdown (banking the elapsed time),
    /// leaving resumes them while the rendering window is active. The fan-out glide
    /// retargets.
    pub fn set_hovered(&mut self, hovered: bool, now_ms: f64, reduced_motion: bool) {
        if self.hovered == hovered {
            return;
        }
        self.hovered = hovered;
        self.sync_running(now_ms);
        self.fan
            .retarget(if hovered { 1.0 } else { 0.0 }, now_ms, reduced_motion);
    }

    fn sync_running(&mut self, now_ms: f64) {
        let runs = !self.hovered && self.active;
        for toast in &mut self.toasts {
            if toast.is_closing() {
                continue;
            }
            if runs {
                toast.resume(now_ms);
            } else {
                toast.pause(now_ms);
            }
        }
    }

    /// Advances the queue at `now_ms` for a window whose activity is `window_active`: applies
    /// the pause gating, begins closing notices whose running interval reached their
    /// remaining lifetime, removes notices 200 ms after they began closing, and resets the
    /// hover and fan-out state when the queue becomes empty.
    pub fn tick(&mut self, now_ms: f64, window_active: bool) {
        self.active = window_active;
        self.sync_running(now_ms);
        for toast in &mut self.toasts {
            if toast.is_closing() {
                continue;
            }
            if let Some(since) = toast.running_since
                && now_ms - since >= toast.remaining_ms
            {
                toast.close(now_ms);
            }
        }
        self.toasts
            .retain(|toast| toast.closing_at.is_none_or(|at| now_ms - at < EXIT_MS));
        if self.toasts.is_empty() {
            self.hovered = false;
            self.fan = Glide::at(0.0);
        }
    }

    /// Dismisses a notice (swipe): it begins closing.
    pub fn dismiss(&mut self, id: u64, now_ms: f64) {
        if let Some(toast) = self.toasts.iter_mut().find(|toast| toast.id == id)
            && !toast.is_closing()
        {
            toast.close(now_ms);
        }
    }

    /// Records a measured natural height of a card. Returns whether it changed by more than
    /// 0.5 px (the host then refreshes every window).
    pub fn set_measured(&mut self, id: u64, height: f32) -> bool {
        let Some(toast) = self.toasts.iter_mut().find(|toast| toast.id == id) else {
            return false;
        };
        match toast.measured {
            Some(stored) if (stored - height).abs() <= MEASURE_EPSILON => false,
            _ => {
                toast.measured = Some(height);
                true
            }
        }
    }

    /// The earliest upcoming deadline: a running notice's expiry or a closing notice's removal.
    pub fn next_deadline(&self) -> Option<f64> {
        self.toasts
            .iter()
            .filter_map(|toast| match (toast.closing_at, toast.running_since) {
                (Some(at), _) => Some(at + EXIT_MS),
                (None, Some(since)) => Some(since + toast.remaining_ms),
                (None, None) => None,
            })
            .min_by(f64::total_cmp)
    }

    /// Records a press on a card at pointer `x`.
    pub fn press(&mut self, id: u64, x: f32) {
        if let Some(toast) = self.toasts.iter_mut().find(|toast| toast.id == id)
            && !toast.is_closing()
        {
            toast.press_x = Some(x);
        }
    }

    /// A pointer move over a pressed card: the swipe displacement becomes `max(0, x - press)`
    /// (leftward movement is ignored); a move reporting the button no longer held clears the
    /// press and the displacement.
    pub fn swipe_to(&mut self, id: u64, x: f32, button_held: bool) {
        let Some(toast) = self.toasts.iter_mut().find(|toast| toast.id == id) else {
            return;
        };
        let Some(press) = toast.press_x else {
            return;
        };
        if button_held {
            toast.swipe = (x - press).max(0.0);
        } else {
            toast.press_x = None;
            toast.swipe = 0.0;
        }
    }

    /// The button was released (inside or outside the card): clears the press; a displacement
    /// of at least 45 px dismisses the card, otherwise it returns to rest immediately.
    /// Returns whether the card was dismissed.
    pub fn release(&mut self, id: u64, now_ms: f64) -> bool {
        let Some(toast) = self.toasts.iter_mut().find(|toast| toast.id == id) else {
            return false;
        };
        if toast.press_x.take().is_none() {
            return false;
        }
        if toast.swipe >= SWIPE_DISMISS {
            toast.close(now_ms);
            true
        } else {
            toast.swipe = 0.0;
            false
        }
    }

    /// The fan-out progress at `now_ms`.
    pub fn fan_at(&self, now_ms: f64, reduced_motion: bool) -> f32 {
        self.fan.value(now_ms, reduced_motion)
    }

    /// Computes the frame at `now_ms`: retargets every live card's glides to the geometry of
    /// the current fan-out progress, and derives the entry, exit and swipe motion.
    pub fn frame(&mut self, now_ms: f64, reduced_motion: bool) -> ToasterFrame {
        let fan = self.fan.value(now_ms, reduced_motion);
        let mut animating = self.fan.is_animating(now_ms, reduced_motion);

        // Live cards newest first.
        let live: Vec<usize> = (0..self.toasts.len())
            .rev()
            .filter(|&ix| !self.toasts[ix].is_closing())
            .collect();
        let front_height = live
            .first()
            .and_then(|&ix| self.toasts[ix].measured)
            .unwrap_or(FALLBACK_HEIGHT);
        let heights: Vec<f32> = live
            .iter()
            .map(|&ix| self.toasts[ix].measured.unwrap_or(front_height))
            .collect();
        let targets = stack_targets(&heights, fan);
        for (depth, &ix) in live.iter().enumerate() {
            let toast = &mut self.toasts[ix];
            let target = targets[depth];
            match &mut toast.offset {
                Some(glide) => glide.retarget(target.offset, now_ms, reduced_motion),
                None => toast.offset = Some(Glide::at(target.offset)),
            }
            match &mut toast.clip {
                Some(glide) => glide.retarget(target.clip, now_ms, reduced_motion),
                None => toast.clip = Some(Glide::at(target.clip)),
            }
        }

        let mut stack_height = front_height;
        let mut cards = Vec::with_capacity(self.toasts.len());
        for (ix, toast) in self.toasts.iter().enumerate() {
            let depth = live.iter().position(|&live_ix| live_ix == ix).unwrap_or(0);
            let closing = toast.is_closing();
            let (Some(offset_glide), Some(clip_glide)) = (toast.offset, toast.clip) else {
                continue;
            };
            let offset = offset_glide.value(now_ms, reduced_motion);
            let clip = clip_glide.value(now_ms, reduced_motion);
            animating |= offset_glide.is_animating(now_ms, reduced_motion)
                || clip_glide.is_animating(now_ms, reduced_motion);
            if !closing {
                stack_height = stack_height.max(offset + clip);
            }
            let entry = if reduced_motion {
                1.0
            } else {
                ease(((now_ms - toast.created_ms) / GLIDE_MS) as f32)
            };
            if !reduced_motion && now_ms - toast.created_ms < GLIDE_MS {
                animating = true;
            }
            let exit = toast.closing_at.map_or(0.0, |at| {
                if reduced_motion {
                    0.0
                } else {
                    ((now_ms - at) / EXIT_MS).clamp(0.0, 1.0) as f32
                }
            });
            if closing && !reduced_motion {
                animating = true;
            }
            let depth = if closing { 0 } else { depth };
            let inset = if closing { 0.0 } else { card_inset(depth, fan) };
            let mut bottom = offset - (1.0 - entry) * (clip + EDGE_MARGIN);
            if closing && toast.swipe < 0.5 {
                bottom -= exit * STEP;
            }
            let depth_alpha = if closing {
                1.0
            } else {
                depth_opacity(depth, fan)
            };
            let opacity = depth_alpha * entry * (1.0 - exit) * swipe_opacity(toast.swipe);
            cards.push(CardFrame {
                id: toast.id,
                kind: toast.kind,
                message: toast.message.clone(),
                depth,
                closing,
                bottom,
                clip,
                inset,
                swipe: toast.swipe,
                opacity,
                skipped: !closing && opacity <= HIDDEN_OPACITY && depth >= VISIBLE_DEPTH,
            });
        }
        ToasterFrame {
            fan,
            height: stack_height,
            cards,
            animating,
        }
    }

    /// What the host does after rendering `frame` at `now_ms`: ask for frames only while
    /// something animates; otherwise one timer for the earliest deadline.
    pub fn plan(&self, frame: &ToasterFrame) -> WakePlan {
        if self.toasts.is_empty() {
            WakePlan::Idle
        } else if frame.animating {
            WakePlan::Frames
        } else {
            self.next_deadline().map_or(WakePlan::Idle, WakePlan::At)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn messages(queue: &ToastQueue) -> Vec<String> {
        queue
            .live_messages()
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn countdown_pauses_while_hovered_and_while_the_window_is_inactive() {
        // A1: a notice at 4000 ms hovered after 1 s and for 30 s has not begun closing.
        let mut queue = ToastQueue::new();
        queue.publish(ToastKind::Info, "saved", 0.0);
        queue.tick(1000.0, true);
        queue.set_hovered(true, 1000.0, false);
        queue.tick(31_000.0, true);
        assert!(!queue.toasts()[0].is_closing());
        assert_eq!(queue.toasts()[0].remaining_at(31_000.0), 3000.0);
        // Clearing hover and holding the window inactive for 29 s still leaves it open.
        queue.set_hovered(false, 31_000.0, false);
        queue.tick(31_000.0, false);
        queue.tick(60_000.0, false);
        assert!(!queue.toasts()[0].is_closing());
        // Active again: 3000 ms remain.
        queue.tick(60_000.0, true);
        assert_eq!(queue.toasts()[0].remaining_at(60_000.0), 3000.0);
        queue.tick(62_900.0, true);
        assert!(!queue.toasts()[0].is_closing());
        queue.tick(63_100.0, true);
        assert!(queue.toasts()[0].is_closing());
        // Removed 200 ms after it began closing.
        queue.tick(63_299.0, true);
        assert_eq!(queue.toasts().len(), 1);
        queue.tick(63_300.0, true);
        assert!(queue.is_empty());
        // Empty: hover and fan-out are reset.
        assert!(!queue.is_hovered());
        assert_eq!(queue.fan_at(63_300.0, false), 0.0);
    }

    #[test]
    fn duplicates_of_the_front_notice_restart_it_and_others_add_cards() {
        // A2.
        let mut queue = ToastQueue::new();
        queue.publish(ToastKind::Error, "disk full", 0.0);
        queue.publish(ToastKind::Error, "disk full", 100.0);
        queue.publish(ToastKind::Success, "done", 200.0);
        assert_eq!(messages(&queue), ["disk full", "done"]);
        // The same success again restarts it instead of adding a card.
        queue.tick(3000.0, true);
        queue.publish(ToastKind::Success, "done", 3000.0);
        assert_eq!(messages(&queue), ["disk full", "done"]);
        assert_eq!(queue.toasts()[1].remaining_at(3000.0), LIFETIME_MS);
        // The original error is no longer the front: a new card.
        queue.publish(ToastKind::Error, "disk full", 3100.0);
        assert_eq!(messages(&queue), ["disk full", "done", "disk full"]);
        // A kind change is a new card; a whitespace-only message adds nothing.
        queue.publish(ToastKind::Info, "disk full", 3200.0);
        assert_eq!(queue.live_messages().len(), 4);
        assert_eq!(queue.publish(ToastKind::Info, "  \n\t ", 3300.0), None);
        assert_eq!(queue.live_messages().len(), 4);
        // Ids increase monotonically; messages are trimmed.
        let id = queue
            .publish(ToastKind::Info, "  padded  ", 3400.0)
            .unwrap();
        assert_eq!(queue.get(id).unwrap().message.as_ref(), "padded");
        assert!(
            queue
                .toasts()
                .windows(2)
                .all(|pair| pair[0].id < pair[1].id)
        );
    }

    #[test]
    fn a_duplicate_while_hovered_stays_paused_and_a_closing_notice_is_no_target() {
        let mut queue = ToastQueue::new();
        queue.publish(ToastKind::Info, "same", 0.0);
        queue.set_hovered(true, 500.0, false);
        queue.publish(ToastKind::Info, "same", 600.0);
        assert!(!queue.toasts()[0].is_running());
        assert_eq!(queue.toasts()[0].remaining_at(600.0), LIFETIME_MS);
        // A notice published while hovered starts paused.
        let id = queue.publish(ToastKind::Error, "other", 700.0).unwrap();
        assert!(!queue.get(id).unwrap().is_running());
        queue.set_hovered(false, 800.0, false);
        // A closing notice is not restarted, re-expired, or a duplicate target: the same
        // message again is a new card, and only then a duplicate of that new front card.
        queue.dismiss(id, 900.0);
        queue.publish(ToastKind::Error, "other", 950.0);
        assert_eq!(queue.toasts().len(), 3);
        assert!(queue.get(id).unwrap().is_closing());
        let refreshed = queue.publish(ToastKind::Error, "other", 960.0);
        assert_eq!(queue.toasts().len(), 3);
        assert_ne!(refreshed, Some(id));
    }

    #[test]
    fn five_cards_collapse_to_three_visible_offsets_and_fan_out_when_hovered() {
        // A3.
        let mut queue = ToastQueue::new();
        for ix in 0..5 {
            queue.publish(ToastKind::Info, &format!("card {ix}"), 0.0);
        }
        let ids: Vec<u64> = queue.toasts().iter().map(|toast| toast.id).collect();
        let heights = [40.0, 60.0, 50.0, 70.0, 45.0];
        // Newest first: card 4 is the front.
        for (ix, &id) in ids.iter().enumerate() {
            queue.set_measured(id, heights[ix]);
        }
        let front = heights[4];
        queue.frame(0.0, false);
        let frame = queue.frame(1000.0, false);
        let visible: Vec<&CardFrame> = frame.cards.iter().filter(|card| !card.skipped).collect();
        assert_eq!(visible.len(), 3);
        // Oldest first: depth 4, 3, 2, 1, 0. The three visible are depth 2, 1, 0.
        let by_depth: Vec<(usize, f32, f32, f32)> = visible
            .iter()
            .rev()
            .map(|card| (card.depth, card.bottom, card.clip, card.inset))
            .collect();
        assert_eq!(by_depth[0], (0, 0.0, front, 0.0));
        assert_eq!(by_depth[1].0, 1);
        assert_eq!(by_depth[1].1, 14.0);
        assert!((by_depth[1].3 - 8.9).abs() < 1e-4);
        assert_eq!(visible.len(), 3);
        assert_eq!(by_depth[2].1, 28.0);
        assert!((by_depth[2].3 - 17.8).abs() < 1e-4);
        assert!(visible.iter().all(|card| card.clip == front));
        // Hover and let the glides complete: all five at fanned offsets with their own heights.
        queue.set_hovered(true, 1000.0, false);
        let mut now = 1000.0;
        for _ in 0..80 {
            now += 100.0;
            queue.frame(now, false);
        }
        now += 1000.0;
        let frame = queue.frame(now, false);
        assert_eq!(frame.fan, 1.0);
        assert!(frame.cards.iter().all(|card| !card.skipped));
        let fanned: Vec<(f32, f32)> = frame
            .cards
            .iter()
            .rev()
            .map(|c| (c.bottom, c.clip))
            .collect();
        let expected_offsets = [
            0.0,
            45.0 + 14.0,
            45.0 + 70.0 + 28.0,
            45.0 + 70.0 + 50.0 + 42.0,
            45.0 + 70.0 + 50.0 + 60.0 + 56.0,
        ];
        let expected_heights = [45.0, 70.0, 50.0, 60.0, 40.0];
        for (ix, (bottom, clip)) in fanned.iter().enumerate() {
            assert!(
                (bottom - expected_offsets[ix]).abs() < 0.1,
                "offset {ix}: {bottom}"
            );
            assert!(
                (clip - expected_heights[ix]).abs() < 0.1,
                "clip {ix}: {clip}"
            );
        }
        assert!(frame.cards.iter().all(|card| card.inset.abs() < 0.01));
    }

    #[test]
    fn stack_targets_follow_the_formulas() {
        let heights = [40.0, 60.0, 50.0];
        let collapsed = stack_targets(&heights, 0.0);
        assert_eq!(
            collapsed,
            [
                CardTarget {
                    offset: 0.0,
                    clip: 40.0
                },
                CardTarget {
                    offset: 14.0,
                    clip: 40.0
                },
                CardTarget {
                    offset: 28.0,
                    clip: 40.0
                },
            ]
        );
        let fanned = stack_targets(&heights, 1.0);
        assert_eq!(
            fanned,
            [
                CardTarget {
                    offset: 0.0,
                    clip: 40.0
                },
                CardTarget {
                    offset: 54.0,
                    clip: 60.0
                },
                CardTarget {
                    offset: 128.0,
                    clip: 50.0
                },
            ]
        );
        let half = stack_targets(&heights, 0.5);
        assert_eq!(
            half[1],
            CardTarget {
                offset: 34.0,
                clip: 50.0
            }
        );
        assert!((card_inset(1, 0.0) - 8.9).abs() < 1e-5);
        assert!((card_inset(2, 0.0) - 17.8).abs() < 1e-5);
        assert_eq!(card_inset(5, 0.0), card_inset(3, 0.0));
        assert_eq!(card_inset(2, 1.0), 0.0);
        assert_eq!(depth_opacity(2, 0.0), 1.0);
        assert_eq!(depth_opacity(3, 0.0), 0.0);
        assert_eq!(depth_opacity(4, 0.7), 0.7);
    }

    #[test]
    fn unmeasured_cards_use_the_front_height_or_the_fallback() {
        let mut queue = ToastQueue::new();
        let a = queue.publish(ToastKind::Info, "a", 0.0).unwrap();
        queue.publish(ToastKind::Info, "b", 0.0);
        let frame = queue.frame(0.0, false);
        assert!(frame.cards.iter().all(|card| card.clip == FALLBACK_HEIGHT));
        // The front (newest) card is measured: the older, unmeasured card stands in with it.
        let b = queue.toasts()[1].id;
        assert!(queue.set_measured(b, 70.0));
        let frame = queue.frame(0.0, false);
        assert_eq!(frame.height, 70.0, "at least the front card's height");
        let frame = queue.frame(2000.0, false);
        assert_eq!(frame.height, 70.0 + 14.0);
        assert!(frame.cards.iter().all(|card| card.clip == 70.0));
        // Changes within 0.5 px are ignored.
        assert!(!queue.set_measured(b, 70.4));
        assert!(queue.set_measured(b, 71.0));
        assert!(queue.set_measured(a, 30.0));
    }

    #[test]
    fn swipe_displaces_fades_and_dismisses_at_45_px() {
        // A4.
        let mut queue = ToastQueue::new();
        let id = queue.publish(ToastKind::Info, "swipe me", 0.0).unwrap();
        queue.press(id, 100.0);
        queue.swipe_to(id, 144.0, true);
        assert_eq!(queue.get(id).unwrap().swipe(), 44.0);
        assert!(!queue.release(id, 10.0));
        assert_eq!(queue.get(id).unwrap().swipe(), 0.0);
        assert!(!queue.get(id).unwrap().is_closing());
        queue.press(id, 100.0);
        queue.swipe_to(id, 145.0, true);
        assert!(queue.release(id, 20.0));
        assert!(queue.get(id).unwrap().is_closing());
        // 150 px right gives opacity 1 - 150/213.6 before release; leftward never goes negative.
        let mut queue = ToastQueue::new();
        let id = queue.publish(ToastKind::Info, "x", 0.0).unwrap();
        queue.press(id, 100.0);
        queue.swipe_to(id, 250.0, true);
        let frame = queue.frame(1000.0, false);
        assert!((frame.cards[0].opacity - (1.0 - 150.0 / 213.6)).abs() < 1e-5);
        queue.swipe_to(id, 40.0, true);
        assert_eq!(queue.get(id).unwrap().swipe(), 0.0);
        // A move reporting the button up clears the press and the displacement.
        queue.swipe_to(id, 160.0, true);
        queue.swipe_to(id, 170.0, false);
        assert_eq!(queue.get(id).unwrap().swipe(), 0.0);
        assert!(!queue.release(id, 5.0));
        assert!((swipe_opacity(500.0)).abs() < 1e-6);
    }

    #[test]
    fn a_swiped_card_keeps_its_displacement_and_other_closing_cards_sink() {
        let mut queue = ToastQueue::new();
        let a = queue.publish(ToastKind::Info, "a", 0.0).unwrap();
        let b = queue.publish(ToastKind::Info, "b", 0.0).unwrap();
        queue.frame(0.0, false);
        // b swiped away keeps its displacement and does not sink.
        queue.press(b, 0.0);
        queue.swipe_to(b, 60.0, true);
        queue.release(b, 1000.0);
        // a expires with no displacement: it sinks by t x 14.
        queue.dismiss(a, 1000.0);
        let before = queue.frame(1000.0, false);
        let after = queue.frame(1100.0, false);
        let card =
            |frame: &ToasterFrame, id| frame.cards.iter().find(|c| c.id == id).cloned().unwrap();
        assert_eq!(card(&after, b).swipe, 60.0);
        assert_eq!(card(&before, b).bottom, card(&after, b).bottom);
        assert!((card(&before, a).bottom - card(&after, a).bottom - 7.0).abs() < 1e-3);
        // Closing cards use depth 0 and no inset.
        assert!(
            after
                .cards
                .iter()
                .all(|card| card.depth == 0 && card.inset == 0.0)
        );
        // Opacity fades with (1 - t).
        assert!((card(&after, a).opacity - 0.5).abs() < 1e-3 || card(&after, a).opacity < 1.0);
    }

    #[test]
    fn glides_retarget_without_a_jump_and_reduced_motion_snaps() {
        // A5.
        let mut glide = Glide::at(0.0);
        glide.retarget(100.0, 0.0, false);
        assert_eq!(glide.value(0.0, false), 0.0);
        let mid = glide.value(200.0, false);
        assert!(mid > 0.0 && mid < 100.0);
        // Reverse at the midpoint: it continues from the current value.
        glide.retarget(0.0, 200.0, false);
        assert!((glide.value(200.0, false) - mid).abs() < 1e-4);
        assert!(glide.value(300.0, false) < mid);
        assert_eq!(glide.value(600.0, false), 0.0);
        assert!(!glide.is_animating(600.0, false));
        // A change smaller than 0.01 does not restart it.
        glide.retarget(0.005, 700.0, false);
        assert_eq!(glide.target(), 0.0);
        // Reduced motion: every glide equals its target immediately.
        glide.retarget(50.0, 800.0, true);
        assert_eq!(glide.value(800.0, true), 50.0);
        let mut moving = Glide::at(0.0);
        moving.retarget(10.0, 0.0, false);
        assert_eq!(moving.value(10.0, true), 10.0);
        assert!(!moving.is_animating(10.0, true));
    }

    #[test]
    fn the_easing_curve_is_monotonic_and_anchored() {
        assert!(ease(0.0).abs() < 1e-5);
        assert!((ease(1.0) - 1.0).abs() < 1e-5);
        let mut previous = 0.0;
        for step in 1..=20 {
            let value = ease(step as f32 / 20.0);
            assert!(value >= previous);
            previous = value;
        }
        // cubic-bezier(0.215, 0.61, 0.355, 1) is an ease-out: past the diagonal early on.
        assert!(ease(0.25) > 0.4);
    }

    #[test]
    fn entry_and_exit_animation_follow_the_formulas_and_snap_with_reduced_motion() {
        let mut queue = ToastQueue::new();
        let id = queue.publish(ToastKind::Info, "hello", 1000.0).unwrap();
        queue.set_measured(id, 50.0);
        let start = queue.frame(1000.0, false);
        assert_eq!(start.cards[0].opacity, 0.0);
        assert_eq!(start.cards[0].bottom, -(50.0 + 24.0));
        assert!(start.animating);
        let done = queue.frame(1400.0, false);
        assert_eq!(done.cards[0].opacity, 1.0);
        assert_eq!(done.cards[0].bottom, 0.0);
        assert!(!done.animating);
        // Reduced motion: full opacity at the target from the first frame.
        let mut queue = ToastQueue::new();
        let id = queue.publish(ToastKind::Info, "hello", 1000.0).unwrap();
        queue.set_measured(id, 50.0);
        let start = queue.frame(1000.0, true);
        assert_eq!((start.cards[0].opacity, start.cards[0].bottom), (1.0, 0.0));
        assert!(!start.animating);
        // Exit: opacity x (1 - t), removed without fading under reduced motion.
        queue.dismiss(id, 2000.0);
        let exiting = queue.frame(2100.0, true);
        assert_eq!(exiting.cards[0].opacity, 1.0);
        queue.tick(2200.0, true);
        assert!(queue.is_empty());
        let mut queue = ToastQueue::new();
        let id = queue.publish(ToastKind::Info, "hello", 0.0).unwrap();
        queue.set_measured(id, 50.0);
        queue.frame(500.0, false);
        queue.dismiss(id, 600.0);
        let exiting = queue.frame(700.0, false);
        assert!((exiting.cards[0].opacity - 0.5).abs() < 1e-3);
    }

    #[test]
    fn only_animation_requests_frames_and_idle_stacks_plan_one_wakeup() {
        // A6 (model side).
        let mut queue = ToastQueue::new();
        let a = queue.publish(ToastKind::Info, "a", 0.0).unwrap();
        queue.set_measured(a, 50.0);
        let frame = queue.frame(0.0, false);
        assert_eq!(queue.plan(&frame), WakePlan::Frames);
        // After the entry animation: no frames, a wake-up at the expiry.
        queue.tick(500.0, true);
        let frame = queue.frame(500.0, false);
        assert_eq!(queue.plan(&frame), WakePlan::At(4000.0));
        // A later notice with an earlier deadline cannot exist; a paused queue has none.
        queue.set_hovered(true, 600.0, false);
        let frame = queue.frame(600.0, false);
        assert_eq!(queue.plan(&frame), WakePlan::Frames);
        let frame = queue.frame(1100.0, false);
        assert_eq!(queue.plan(&frame), WakePlan::Idle);
        // Closing notices contribute their removal time; the earliest deadline wins.
        queue.set_hovered(false, 1200.0, false);
        let b = queue.publish(ToastKind::Error, "b", 1200.0).unwrap();
        queue.set_measured(b, 50.0);
        queue.dismiss(a, 1250.0);
        queue.tick(1250.0, true);
        assert_eq!(queue.next_deadline(), Some(1450.0));
        let empty = ToastQueue::new();
        let frame = ToasterFrame {
            fan: 0.0,
            height: 0.0,
            cards: Vec::new(),
            animating: false,
        };
        assert_eq!(empty.plan(&frame), WakePlan::Idle);
    }

    #[test]
    fn the_stack_frame_is_anchored_bottom_right_and_narrows_with_the_viewport() {
        // A9.
        assert_eq!(
            stack_frame(1000.0, 800.0, 120.0),
            (1000.0 - 24.0 - 356.0, 800.0 - 24.0 - 120.0, 356.0, 120.0)
        );
        let (x, _, width, _) = stack_frame(300.0, 800.0, 120.0);
        assert_eq!(width, 252.0);
        assert_eq!(x, 24.0);
        assert_eq!(stack_width(30.0), 0.0);
    }
}
