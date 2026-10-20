//! The deterministic workbench script.
//!
//! A script is a timeline of steps on a 50 ms tick, built up front from the run length alone, so a
//! run is a pure function of the tick index: the same replies stream in the same chunks, the same
//! keys arrive on the same ticks. Every step drives the window the way a user would (pointer
//! presses at recorded geometry, typed keys, wheel notches, a seam drag); between steps the
//! script asserts invariants, and the first violated one stops the run with a message.
//!
//! One cycle is: open a session, click into the input, type a two-line prompt, submit, stream the
//! reply, scroll the history away, stream a second reply while detached, jump back to the end,
//! switch through every tab, and resize a pane. A run of N seconds repeats cycles while another
//! fits (the first always runs), then idles to the end.

use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicUsize, Ordering},
};

use vespertine::{
    App, Entity, Focusable as _, Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, PlatformInput, Point, ScrollDelta, ScrollWheelEvent, Window, point, px,
};

use crate::app::{SIDEBAR, TAB_CHAT, TAB_NOTES, TAB_PLAN, WORDS_PER_CHUNK, Workbench};
use crate::content::{self, PROMPTS, REPLIES, SESSIONS};

/// Virtual time of one tick.
pub const TICK_MS: u64 = 50;
/// Ticks per second.
pub const TICKS_PER_SECOND: u64 = 1000 / TICK_MS;

/// Panics seen by the hook installed with [`watch_panics`].
static PANICS: AtomicUsize = AtomicUsize::new(0);

/// Counts every panic of every thread (the previous hook still runs), so "no panics" is
/// checkable even when the panicking thread is not the one that drives the script.
pub fn watch_panics() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        PANICS.fetch_add(1, Ordering::SeqCst);
        previous(info);
    }));
}

/// Where the script runs: it gets the window and the app to act in.
pub trait Stage {
    fn run(&mut self, f: &mut dyn FnMut(&mut Window, &mut App));
}

#[derive(Clone, Copy, Debug)]
enum Target {
    Session(usize),
    Tab(usize),
    Input,
    Notes,
    Jump,
}

#[derive(Clone, Debug)]
enum Step {
    Click(Target),
    Type(char),
    Key(&'static str),
    Wheel(f32),
    /// Feeds one chunk of the streaming reply.
    Feed,
    SeamDown,
    /// Moves the pressed pointer this many pixels from where the seam was pressed.
    SeamMove(f32),
    /// Releases the pointer this many pixels from where the seam was pressed.
    SeamUp(f32),
    /// The tab the strip must show from now on.
    ExpectTab(&'static str),
    /// Whether the transcript must stay pinned while it streams.
    ExpectPinned(bool),
    Check(Check),
}

impl Step {
    /// The kind of input the step drives; `None` for bookkeeping and assertions.
    fn kind(&self) -> Option<&'static str> {
        Some(match self {
            Step::Click(Target::Session(_)) => "sidebar click",
            Step::Click(Target::Tab(_)) => "tab click",
            Step::Click(Target::Input | Target::Notes) => "editor click",
            Step::Click(Target::Jump) => "jump button click",
            Step::Type(_) => "typing",
            Step::Key("enter") => "submit",
            Step::Key("ctrl-tab" | "ctrl-shift-tab") => "tab cycling",
            Step::Key(_) => "line break",
            Step::Wheel(_) => "history scroll",
            Step::Feed => "token streaming",
            Step::SeamMove(_) => "seam drag",
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
enum Check {
    Session(usize),
    Tab(&'static str),
    InputFocused,
    NotesFocused,
    InputIs(String),
    InputGrewTaller,
    Submitted { prompt: String, rows_before: usize },
    StreamDone,
    AtBottom,
    Detached,
    Anchored,
    NotesContain(String),
    SidebarResized(f32),
    JumpVisible,
    Final,
}

pub struct Script {
    workbench: Entity<Workbench>,
    steps: Vec<(u64, Step)>,
    next: usize,
    tick: u64,
    total_ticks: u64,
    expected_tab: &'static str,
    expect_pinned: bool,
    anchor: Option<(usize, f32)>,
    sidebar_before: f32,
    resizes_before: usize,
    seam_start: Point<Pixels>,
    one_line_height: f32,
    worst_lag: f32,
    checks: u64,
    exercised: BTreeSet<&'static str>,
}

fn ok_if(condition: bool, what: impl FnOnce() -> String) -> Result<(), String> {
    if condition { Ok(()) } else { Err(what()) }
}

impl Script {
    pub fn new(workbench: Entity<Workbench>, seconds: u64) -> Self {
        let total_ticks = seconds.max(1) * TICKS_PER_SECOND;
        Self {
            workbench,
            steps: timeline(total_ticks),
            next: 0,
            tick: 0,
            total_ticks,
            expected_tab: TAB_CHAT,
            expect_pinned: true,
            anchor: None,
            sidebar_before: 0.0,
            resizes_before: 0,
            seam_start: point(px(0.0), px(0.0)),
            one_line_height: 0.0,
            worst_lag: 0.0,
            checks: 0,
            exercised: BTreeSet::new(),
        }
    }

    /// The tick a run of this length ends on.
    pub fn end_tick(&self) -> u64 {
        self.steps
            .last()
            .map_or(0, |(at, _)| *at)
            .max(self.total_ticks)
    }

    /// Invariant and assertion evaluations that passed.
    pub fn checks(&self) -> u64 {
        self.checks
    }

    /// Ticks run so far.
    pub fn ticks(&self) -> u64 {
        self.tick
    }

    /// The largest distance from the end of the transcript seen on a pinned streaming tick.
    pub fn worst_lag(&self) -> f32 {
        self.worst_lag
    }

    /// The kinds of input the script has driven so far (for example `"submit"`, `"seam drag"`).
    pub fn exercised(&self) -> &BTreeSet<&'static str> {
        &self.exercised
    }

    /// Runs the steps due on the current tick and the per-tick invariants, then advances.
    /// `Ok(true)` once the last tick has run.
    pub fn tick(&mut self, stage: &mut dyn Stage) -> Result<bool, String> {
        let now = self.tick;
        let mut outcome: Result<(), String> = Ok(());
        while outcome.is_ok()
            && let Some((at, step)) = self.steps.get(self.next).cloned()
            && at <= now
        {
            self.next += 1;
            stage.run(&mut |window, cx| {
                if outcome.is_ok() {
                    outcome = self.step(&step, window, cx);
                }
            });
        }
        if outcome.is_ok() {
            stage.run(&mut |window, cx| outcome = self.invariants(window, cx));
        }
        outcome.map_err(|message| format!("tick {now}: {message}"))?;
        self.tick += 1;
        Ok(self.tick > self.end_tick())
    }

    fn invariants(&mut self, _window: &mut Window, cx: &mut App) -> Result<(), String> {
        let panics = PANICS.load(Ordering::SeqCst);
        ok_if(panics == 0, || format!("{panics} panic(s) were raised"))?;
        let workbench = self.workbench.read(cx);
        let tab = workbench.selected_tab(cx);
        ok_if(tab.as_deref() == Some(self.expected_tab), || {
            format!("active tab is {tab:?}, expected {:?}", self.expected_tab)
        })?;
        if workbench.is_streaming() && self.expect_pinned {
            let follow = workbench.follow();
            let distance = follow.distance_from_bottom();
            ok_if(follow.is_pinned(), || {
                format!("the transcript left the bottom while streaming ({distance:.1} px away)")
            })?;
            // The view glides after a growing tail but never trails it by more than a screen.
            let viewport = follow.list().viewport_bounds().size.height.as_f32();
            self.worst_lag = self.worst_lag.max(distance);
            ok_if(distance <= viewport, || {
                format!(
                    "the streaming tail is {distance:.1} px from the end, more than the {viewport:.0} px viewport"
                )
            })?;
        }
        self.checks += 1;
        Ok(())
    }

    fn step(&mut self, step: &Step, window: &mut Window, cx: &mut App) -> Result<(), String> {
        if let Some(kind) = step.kind() {
            self.exercised.insert(kind);
        }
        match step {
            Step::Click(target) => {
                let at = self.locate(*target, cx)?;
                press(window, cx, at);
            }
            Step::Type(ch) => {
                window.dispatch_keystroke(keystroke_for(*ch), cx);
            }
            Step::Key(key) => {
                let keystroke = Keystroke::parse(key).map_err(|error| error.to_string())?;
                window.dispatch_keystroke(keystroke, cx);
            }
            Step::Wheel(dy) => {
                let bounds = self
                    .workbench
                    .read(cx)
                    .probes
                    .transcript
                    .get()
                    .ok_or("the transcript has not been painted")?;
                window.dispatch_event(
                    PlatformInput::ScrollWheel(ScrollWheelEvent {
                        position: bounds.center(),
                        delta: ScrollDelta::Pixels(point(px(0.0), px(*dy))),
                        ..Default::default()
                    }),
                    cx,
                );
            }
            Step::Feed => {
                self.workbench
                    .update(cx, |workbench, cx| workbench.stream_step(window, cx));
            }
            Step::SeamDown => {
                let workbench = self.workbench.read(cx);
                self.resizes_before = workbench.resizes;
                let panes = workbench.panes.read(cx);
                self.sidebar_before = panes
                    .size(&SIDEBAR.into())
                    .ok_or("the sidebar pane is missing")?
                    .as_f32();
                let seam = panes
                    .metrics()
                    .frame()
                    .seams
                    .into_iter()
                    .find(|seam| seam.pane == SIDEBAR.into())
                    .ok_or("the sidebar seam has not been painted")?;
                self.seam_start = seam.bounds.center();
                pointer(window, cx, self.seam_start, None);
                mouse(window, cx, self.seam_start, true);
            }
            Step::SeamMove(dx) => {
                let at = point(self.seam_start.x + px(*dx), self.seam_start.y);
                pointer(window, cx, at, Some(MouseButton::Left));
            }
            Step::SeamUp(dx) => {
                let at = point(self.seam_start.x + px(*dx), self.seam_start.y);
                mouse(window, cx, at, false);
            }
            Step::ExpectTab(tab) => self.expected_tab = tab,
            Step::ExpectPinned(pinned) => self.expect_pinned = *pinned,
            Step::Check(check) => self.check(check, window, cx)?,
        }
        Ok(())
    }

    fn locate(&self, target: Target, cx: &App) -> Result<Point<Pixels>, String> {
        let workbench = self.workbench.read(cx);
        match target {
            Target::Session(ix) => workbench
                .probes
                .session_rows
                .borrow()
                .get(ix)
                .map(|bounds| bounds.center())
                .ok_or_else(|| format!("session row {ix} has not been painted")),
            Target::Tab(ix) => {
                let strip = workbench.tabs.read(cx).scroll().bounds();
                // Chips are 116 px apart, the first centred 56 px from the strip's left edge.
                Ok(point(
                    strip.origin.x + px(56.0 + 116.0 * ix as f32),
                    strip.center().y,
                ))
            }
            Target::Input => workbench
                .input
                .read(cx)
                .viewport_bounds()
                .map(|bounds| bounds.center())
                .ok_or_else(|| "the input has not been painted".to_owned()),
            Target::Notes => workbench
                .notes
                .read(cx)
                .viewport_bounds()
                .map(|bounds| bounds.center())
                .ok_or_else(|| "the notes editor has not been painted".to_owned()),
            Target::Jump => workbench
                .probes
                .jump
                .get()
                .map(|bounds| bounds.center())
                .ok_or_else(|| "the jump button is not shown".to_owned()),
        }
    }

    fn check(&mut self, check: &Check, window: &mut Window, cx: &mut App) -> Result<(), String> {
        let workbench = self.workbench.clone();
        let workbench = workbench.read(cx);
        match check {
            Check::Session(ix) => ok_if(workbench.active == *ix, || {
                format!("active session is {}, expected {ix}", workbench.active)
            }),
            Check::Tab(tab) => {
                let selected = workbench.selected_tab(cx);
                ok_if(selected.as_deref() == Some(tab), || {
                    format!("selected tab is {selected:?}, expected {tab}")
                })
            }
            Check::InputFocused => {
                ok_if(workbench.input.focus_handle(cx).is_focused(window), || {
                    "the input is not focused".to_owned()
                })
            }
            Check::NotesFocused => {
                ok_if(workbench.notes.focus_handle(cx).is_focused(window), || {
                    "the notes editor is not focused".to_owned()
                })
            }
            Check::InputIs(text) => {
                let source = workbench.input.read(cx).source().to_owned();
                if text.is_empty() {
                    self.one_line_height = workbench.input.read(cx).measured_height().as_f32();
                }
                ok_if(&source == text, || {
                    format!("the input holds {source:?}, expected {text:?}")
                })
            }
            Check::InputGrewTaller => {
                let height = workbench.input.read(cx).measured_height().as_f32();
                ok_if(height > self.one_line_height, || {
                    format!("the input stayed {height} px tall with two lines")
                })
            }
            Check::Submitted {
                prompt,
                rows_before,
            } => {
                let area = workbench.input.read(cx);
                ok_if(area.is_empty(), || {
                    format!("the input was not cleared on submit: {:?}", area.source())
                })?;
                ok_if(workbench.submitted.last() == Some(prompt), || {
                    format!("the submitted text is {:?}", workbench.submitted.last())
                })?;
                let rows = workbench.message_count(workbench.active);
                ok_if(rows == rows_before + 2, || {
                    format!(
                        "the transcript has {rows} rows, expected {}",
                        rows_before + 2
                    )
                })?;
                ok_if(workbench.is_streaming(), || {
                    "no reply started streaming".to_owned()
                })
            }
            Check::StreamDone => ok_if(!workbench.is_streaming(), || {
                "the reply is still streaming".to_owned()
            }),
            Check::AtBottom => {
                let follow = workbench.follow();
                let distance = follow.distance_from_bottom();
                ok_if(follow.is_pinned() && distance <= 2.0, || {
                    format!(
                        "expected the end of the transcript, pinned {} and {distance:.1} px away",
                        follow.is_pinned()
                    )
                })
            }
            Check::Detached => {
                let follow = workbench.follow();
                ok_if(!follow.is_pinned(), || {
                    "wheeling up did not detach the transcript".to_owned()
                })?;
                let ix = workbench.sessions[workbench.active]
                    .list
                    .logical_scroll_top()
                    .item_ix;
                let top = workbench.sessions[workbench.active]
                    .list
                    .bounds_for_item(ix)
                    .map(|bounds| bounds.top().as_f32())
                    .ok_or("the anchor row is not mounted")?;
                self.anchor = Some((ix, top));
                Ok(())
            }
            Check::Anchored => {
                let Some((ix, before)) = self.anchor else {
                    return Err("no anchor was recorded".to_owned());
                };
                let follow = workbench.follow();
                ok_if(!follow.is_pinned(), || {
                    "the transcript re-pinned by itself".to_owned()
                })?;
                let top = workbench.sessions[workbench.active]
                    .list
                    .bounds_for_item(ix)
                    .map(|bounds| bounds.top().as_f32())
                    .ok_or("the anchor row left the viewport")?;
                ok_if((top - before).abs() <= 1.0, || {
                    format!(
                        "streaming below moved the anchor row by {:.1} px",
                        top - before
                    )
                })
            }
            Check::NotesContain(text) => {
                let source = workbench.notes.read(cx).source().to_owned();
                ok_if(source.contains(text.as_str()), || {
                    format!("the notes hold {source:?}, expected {text:?} in it")
                })
            }
            Check::SidebarResized(delta) => {
                let size = workbench
                    .panes
                    .read(cx)
                    .size(&SIDEBAR.into())
                    .ok_or("the sidebar pane is missing")?
                    .as_f32();
                let target = self.sidebar_before + delta;
                ok_if((size - target).abs() <= 1.0, || {
                    format!("the sidebar is {size:.1} px, expected {target:.1}")
                })?;
                ok_if(workbench.resizes > self.resizes_before, || {
                    "no pane resize was reported".to_owned()
                })
            }
            Check::JumpVisible => ok_if(workbench.follow().show_jump_button(), || {
                "the jump-to-latest button is not offered while detached".to_owned()
            }),
            Check::Final => {
                let expected = workbench.submitted.len();
                ok_if(expected >= 2, || {
                    format!("only {expected} prompts were submitted")
                })?;
                ok_if(!workbench.is_streaming(), || {
                    "a reply is still streaming at the end".to_owned()
                })
            }
        }
    }
}

fn keystroke_for(ch: char) -> Keystroke {
    let mut modifiers = Modifiers::none();
    modifiers.shift = ch.is_uppercase();
    let key = match ch {
        ' ' => "space".to_owned(),
        other => other.to_lowercase().collect(),
    };
    Keystroke {
        modifiers,
        key,
        key_char: Some(ch.to_string()),
    }
}

fn pointer(window: &mut Window, cx: &mut App, at: Point<Pixels>, pressed: Option<MouseButton>) {
    window.dispatch_event(
        PlatformInput::MouseMove(MouseMoveEvent {
            position: at,
            pressed_button: pressed,
            modifiers: Modifiers::none(),
        }),
        cx,
    );
}

fn mouse(window: &mut Window, cx: &mut App, at: Point<Pixels>, down: bool) {
    let event = if down {
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        })
    } else {
        PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
        })
    };
    window.dispatch_event(event, cx);
}

/// A left click: move, press, release.
fn press(window: &mut Window, cx: &mut App, at: Point<Pixels>) {
    pointer(window, cx, at, None);
    mouse(window, cx, at, true);
    mouse(window, cx, at, false);
}

/// Appends `steps` for `count` ticks starting at `at`, one per tick, and returns the next free tick.
fn per_tick(
    steps: &mut Vec<(u64, Step)>,
    mut at: u64,
    items: impl IntoIterator<Item = Step>,
) -> u64 {
    for step in items {
        steps.push((at, step));
        at += 1;
    }
    at
}

fn at(steps: &mut Vec<(u64, Step)>, tick: u64, step: Step) {
    steps.push((tick, step));
}

/// Types `text` one key per tick (a `\n` is Shift+Enter) and returns the next free tick.
fn typing(steps: &mut Vec<(u64, Step)>, start: u64, text: &str) -> u64 {
    per_tick(
        steps,
        start,
        text.chars().map(|ch| {
            if ch == '\n' {
                Step::Key("shift-enter")
            } else {
                Step::Type(ch)
            }
        }),
    )
}

/// The steps of cycle `k` from tick `start`; returns the next free tick.
fn cycle(steps: &mut Vec<(u64, Step)>, k: usize, start: u64, replies: &mut usize) -> u64 {
    let session = k % SESSIONS.len();
    let mut t = start;

    // Open a session and the transcript tab; click into the input.
    at(steps, t, Step::Click(Target::Session(session)));
    at(steps, t + 2, Step::Check(Check::Session(session)));
    at(steps, t + 3, Step::Click(Target::Tab(0)));
    at(steps, t + 3, Step::ExpectTab(TAB_CHAT));
    at(steps, t + 5, Step::Check(Check::Tab(TAB_CHAT)));
    at(steps, t + 6, Step::Click(Target::Input));
    at(steps, t + 8, Step::Check(Check::InputFocused));
    t += 10;

    // Type a two-line prompt, then submit it with Enter.
    let prompt = PROMPTS[k % PROMPTS.len()];
    at(steps, t, Step::Check(Check::InputIs(String::new())));
    t = typing(steps, t + 1, prompt);
    at(steps, t, Step::Check(Check::InputIs(prompt.to_owned())));
    if prompt.contains('\n') {
        at(steps, t, Step::Check(Check::InputGrewTaller));
    }
    // The rows before the submit are those of the session the script opened.
    let rows_before = rows_of(session, k);
    at(steps, t + 1, Step::Key("enter"));
    at(
        steps,
        t + 2,
        Step::Check(Check::Submitted {
            prompt: prompt.to_owned(),
            rows_before,
        }),
    );
    t += 3;

    // Stream the reply; the transcript must stay pinned throughout.
    t = stream(steps, t, replies);
    at(steps, t, Step::Check(Check::StreamDone));
    t += TICKS_PER_SECOND;
    at(steps, t, Step::Check(Check::AtBottom));
    t += 1;

    // Scroll the history away: the transcript detaches and offers the jump button.
    at(steps, t, Step::ExpectPinned(false));
    t = per_tick(steps, t + 1, std::iter::repeat_n(Step::Wheel(140.0), 5));
    t += 6;
    at(steps, t, Step::Check(Check::Detached));
    at(steps, t, Step::Check(Check::JumpVisible));
    t += 1;

    // A second reply streams below the detached view without moving the anchor row.
    at(steps, t, Step::Click(Target::Input));
    t = typing(steps, t + 2, "Continue.");
    at(steps, t, Step::Key("enter"));
    t += 2;
    t = stream_detached(steps, t, replies);
    at(steps, t, Step::Check(Check::StreamDone));
    t += 4;

    // Jump back to the end with the button; the stepper glides there.
    at(steps, t, Step::Click(Target::Jump));
    at(steps, t, Step::ExpectPinned(true));
    t += 2 * TICKS_PER_SECOND;
    at(steps, t, Step::Check(Check::AtBottom));
    t += 1;

    // Every tab by click and keyboard; the notes editor takes typing.
    at(steps, t, Step::Click(Target::Tab(1)));
    at(steps, t, Step::ExpectTab(TAB_PLAN));
    at(steps, t + 3, Step::Check(Check::Tab(TAB_PLAN)));
    at(steps, t + 4, Step::Click(Target::Tab(2)));
    at(steps, t + 4, Step::ExpectTab(TAB_NOTES));
    at(steps, t + 7, Step::Check(Check::Tab(TAB_NOTES)));
    at(steps, t + 8, Step::Click(Target::Notes));
    at(steps, t + 10, Step::Check(Check::NotesFocused));
    let note = format!("cycle {k} done");
    let after_typing = typing(steps, t + 11, &note);
    at(steps, after_typing, Step::Check(Check::NotesContain(note)));
    t = after_typing + 1;
    at(steps, t, Step::Key("ctrl-tab"));
    at(steps, t, Step::ExpectTab(TAB_CHAT));
    at(steps, t + 3, Step::Check(Check::Tab(TAB_CHAT)));
    at(steps, t + 4, Step::Key("ctrl-shift-tab"));
    at(steps, t + 4, Step::ExpectTab(TAB_NOTES));
    at(steps, t + 7, Step::Check(Check::Tab(TAB_NOTES)));
    at(steps, t + 8, Step::Click(Target::Tab(0)));
    at(steps, t + 8, Step::ExpectTab(TAB_CHAT));
    at(steps, t + 11, Step::Check(Check::Tab(TAB_CHAT)));
    at(steps, t + 12, Step::Check(Check::AtBottom));
    t += 14;

    // Drag the sidebar seam 60 px wider, then back 60 px narrower.
    for delta in [60.0, -60.0] {
        at(steps, t, Step::SeamDown);
        t = per_tick(
            steps,
            t + 1,
            [delta / 3.0, delta * 2.0 / 3.0, delta].map(Step::SeamMove),
        );
        at(steps, t, Step::SeamUp(delta));
        at(steps, t + 3, Step::Check(Check::SidebarResized(delta)));
        t += 5;
    }
    t + 5
}

/// Messages in `session` when cycle `k` submits its first prompt: the seed plus four rows for
/// each earlier cycle that used the same session.
fn rows_of(session: usize, k: usize) -> usize {
    let earlier = (0..k).filter(|c| c % SESSIONS.len() == session).count();
    SESSIONS[session].history.len() + 4 * earlier
}

/// One `Feed` per tick until the reply that is next to stream is out, then one more tick for the
/// message to finish. The pinned invariant runs on every tick of it.
fn stream(steps: &mut Vec<(u64, Step)>, start: u64, replies: &mut usize) -> u64 {
    let chunks = content::chunks(REPLIES[*replies % REPLIES.len()], WORDS_PER_CHUNK).len();
    *replies += 1;
    per_tick(steps, start, std::iter::repeat_n(Step::Feed, chunks + 1))
}

/// Like [`stream`], with the anchor check every fifth tick.
fn stream_detached(steps: &mut Vec<(u64, Step)>, start: u64, replies: &mut usize) -> u64 {
    let chunks = content::chunks(REPLIES[*replies % REPLIES.len()], WORDS_PER_CHUNK).len();
    *replies += 1;
    let mut t = start;
    for ix in 0..=chunks {
        at(steps, t, Step::Feed);
        if ix % 5 == 4 {
            at(steps, t + 1, Step::Check(Check::Anchored));
        }
        t += 1;
    }
    t
}

/// The whole timeline of a run of `total_ticks`.
fn timeline(total_ticks: u64) -> Vec<(u64, Step)> {
    let mut steps = Vec::new();
    let mut replies = 0;
    let mut t = 5;
    let mut k = 0;
    loop {
        let mut trial = Vec::new();
        let mut trial_replies = replies;
        let end = cycle(&mut trial, k, t, &mut trial_replies);
        if k > 0 && end > total_ticks {
            break;
        }
        steps.extend(trial);
        replies = trial_replies;
        t = end;
        k += 1;
    }
    steps.push((t, Step::Check(Check::Final)));
    steps.sort_by_key(|(at, _)| *at);
    steps
}
