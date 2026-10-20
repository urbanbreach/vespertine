//! The workbench screen: a session sidebar, tabs over a streaming Markdown transcript, a plan
//! document, a notes editor and a multi-line input, in resizable panes, with every color read
//! from the component theme tokens. The view is shared by the interactive window, the scripted
//! run and the headless smoke test.

use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
    time::Duration,
};

use vespertine::{
    App, AppContext as _, Bounds, Context, Div, Entity, Focusable as _, InteractiveElement,
    IntoElement, ListAlignment, ListState, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, div, list,
    panes::{PaneDescriptor, PaneEvent, PaneGroupState, PaneId, pane_group},
    prelude::FluentBuilder as _,
    px,
};
use vespertine_components::{
    SubmitMode, TextArea, TextAreaEvent,
    controls::Button,
    markdown_doc::{Markdown, MarkdownDocument},
    nav_theme::NavTheme,
    streaming::{Commit, FollowController},
    tabs::{TabId, TabSpec, TabStrip, bind_keys, cycle_on},
    tokens::ComponentTokens,
};
use vespertine_richtext::text_root;

use crate::content::{self, REPLIES, SESSIONS};

pub const SIDEBAR: &str = "sidebar";
pub const CONTEXT: &str = "context";
pub const TAB_CHAT: &str = "chat";
pub const TAB_PLAN: &str = "plan";
pub const TAB_NOTES: &str = "notes";
/// Words per streamed chunk.
pub const WORDS_PER_CHUNK: usize = 3;
/// Entries the activity log keeps.
const LOG_LINES: usize = 14;

/// One transcript message.
pub struct Message {
    pub user: bool,
    pub doc: MarkdownDocument,
    pub streaming: bool,
}

/// One conversation of the sidebar with its own transcript list and bottom-follow controller.
pub struct Session {
    pub title: SharedString,
    pub subtitle: SharedString,
    pub messages: Rc<RefCell<Vec<Message>>>,
    pub list: ListState,
    pub follow: FollowController,
}

impl Session {
    fn seeded(title: &'static str, subtitle: &'static str, history: &[(bool, &str)]) -> Self {
        let messages: Vec<Message> = history
            .iter()
            .map(|(user, markdown)| Message {
                user: *user,
                doc: MarkdownDocument::from_source(markdown),
                streaming: false,
            })
            .collect();
        let list = ListState::new(messages.len(), ListAlignment::Top, px(400.0));
        let follow = FollowController::attach(list.clone());
        Self {
            title: title.into(),
            subtitle: subtitle.into(),
            messages: Rc::new(RefCell::new(messages)),
            list,
            follow,
        }
    }
}

/// The reply being streamed.
struct Job {
    session: usize,
    message: usize,
    chunks: Vec<String>,
    next: usize,
}

/// Geometry the render pass records so the script (and nothing else) can aim real pointer input.
#[derive(Clone, Default)]
pub struct Probes {
    pub session_rows: Rc<RefCell<Vec<Bounds<Pixels>>>>,
    pub transcript: Rc<Cell<Option<Bounds<Pixels>>>>,
    pub jump: Rc<Cell<Option<Bounds<Pixels>>>>,
}

pub struct Workbench {
    pub sessions: Vec<Session>,
    pub active: usize,
    pub panes: Entity<PaneGroupState>,
    pub tabs: Entity<TabStrip>,
    pub input: Entity<TextArea>,
    pub notes: Entity<TextArea>,
    pub plan: MarkdownDocument,
    pub probes: Probes,
    /// Every text the input submitted, oldest first.
    pub submitted: Vec<String>,
    /// How many pane resizes were reported.
    pub resizes: usize,
    job: Option<Job>,
    replies_started: usize,
    log: VecDeque<String>,
}

impl Workbench {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        bind_keys(cx);
        let panes = cx.new(|_| {
            PaneGroupState::new([
                PaneDescriptor::leading(SIDEBAR),
                PaneDescriptor::trailing(CONTEXT).preferred_size(px(360.0)),
            ])
        });
        cx.subscribe(&panes, |this, _, event: &PaneEvent, cx| {
            if let PaneEvent::Resized { pane, size } = event {
                this.resizes += 1;
                this.note(format!(
                    "pane {} resized to {:.0} px",
                    pane.0,
                    size.as_f32()
                ));
            }
            cx.notify();
        })
        .detach();

        let input = cx.new(|cx| {
            TextArea::new(window, cx)
                .placeholder("Message the agent (Enter sends, Shift+Enter adds a line)")
                .rows(1, 6)
                .submit_mode(SubmitMode::EnterSubmits)
        });
        cx.subscribe_in(
            &input,
            window,
            |this, _, event: &TextAreaEvent, window, cx| {
                if let TextAreaEvent::Submitted(text) = event {
                    this.submit(text.clone(), window, cx);
                }
            },
        )
        .detach();
        let notes = cx.new(|cx| {
            TextArea::new(window, cx)
                .placeholder("Notes")
                .rows(8, 24)
                .value(content::NOTES)
        });

        let tabs = cx.new(|_| {
            TabStrip::new("workbench-tabs").tabs(vec![
                TabSpec::new(TAB_CHAT, "Transcript"),
                TabSpec::new(TAB_PLAN, "Plan"),
                TabSpec::new(TAB_NOTES, "Notes"),
            ])
        });
        tabs.update(cx, |strip, cx| {
            strip.select(&TabId::from(TAB_CHAT), cx);
            strip.set_content_focus(TabId::from(TAB_CHAT), input.focus_handle(cx));
            strip.set_content_focus(TabId::from(TAB_NOTES), notes.focus_handle(cx));
        });
        cx.observe(&tabs, |_, _, cx| cx.notify()).detach();

        let sessions = SESSIONS
            .iter()
            .map(|seed| Session::seeded(seed.title, seed.subtitle, seed.history))
            .collect();
        Self {
            sessions,
            active: 0,
            panes,
            tabs,
            input,
            notes,
            plan: MarkdownDocument::from_source(content::PLAN),
            probes: Probes::default(),
            submitted: Vec::new(),
            resizes: 0,
            job: None,
            replies_started: 0,
            log: VecDeque::new(),
        }
    }

    /// Streams queued replies on a 40 ms clock (the interactive window; a scripted run drives
    /// [`Self::stream_step`] itself).
    pub fn start_stream_clock(&self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(40))
                    .await;
                let alive = this.update_in(cx, |this, window, cx| {
                    this.stream_step(window, cx);
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn note(&mut self, line: String) {
        if self.log.len() == LOG_LINES {
            self.log.pop_front();
        }
        self.log.push_back(line);
    }

    pub fn follow(&self) -> &FollowController {
        &self.sessions[self.active].follow
    }

    pub fn selected_tab(&self, cx: &App) -> Option<String> {
        self.tabs
            .read(cx)
            .selected_id()
            .map(|id| id.as_str().to_owned())
    }

    pub fn is_streaming(&self) -> bool {
        self.job.is_some()
    }

    /// The session and message index of the reply being streamed.
    pub fn streaming_target(&self) -> Option<(usize, usize)> {
        self.job.as_ref().map(|job| (job.session, job.message))
    }

    pub fn message_count(&self, session: usize) -> usize {
        self.sessions[session].messages.borrow().len()
    }

    pub fn select_session(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.sessions.len() && ix != self.active {
            self.active = ix;
            self.note(format!("opened {}", self.sessions[ix].title));
        }
        cx.notify();
    }

    pub fn focus_input(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    /// Sends the draft: the text becomes a user message, the draft is cleared and a canned reply
    /// starts streaming under it. Ignored (the draft stays) while a reply is streaming or when
    /// the text is blank.
    pub fn submit(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.job.is_some() || text.trim().is_empty() {
            return;
        }
        self.submitted.push(text.clone());
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        let ix = self.active;
        let reply = REPLIES[self.replies_started % REPLIES.len()];
        self.replies_started += 1;
        let session = &self.sessions[ix];
        let first = {
            let mut messages = session.messages.borrow_mut();
            let first = messages.len();
            messages.push(Message {
                user: true,
                doc: MarkdownDocument::from_source(&text),
                streaming: false,
            });
            messages.push(Message {
                user: false,
                doc: MarkdownDocument::new(),
                streaming: true,
            });
            first
        };
        session.follow.set_tail_streaming(true);
        session.follow.commit(
            Commit::Splice {
                old_range: first..first,
                new_count: 2,
            },
            window,
            cx,
        );
        self.job = Some(Job {
            session: ix,
            message: first + 1,
            chunks: content::chunks(reply, WORDS_PER_CHUNK),
            next: 0,
        });
        self.note(format!("sent {} bytes", text.len()));
        cx.notify();
    }

    /// Feeds the next chunk of the streaming reply; finishes the message after the last one.
    /// Returns whether a reply was streaming.
    pub fn stream_step(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(job) = self.job.as_mut() else {
            return false;
        };
        let session = &self.sessions[job.session];
        let row = job.message;
        let chunk = job.chunks.get(job.next).cloned();
        let done = chunk.is_none();
        {
            let mut messages = session.messages.borrow_mut();
            let message = &mut messages[row];
            match &chunk {
                Some(chunk) => message.doc.append(chunk),
                None => message.streaming = false,
            }
        }
        job.next += 1;
        if done {
            session.follow.set_tail_streaming(false);
        }
        session
            .follow
            .commit(Commit::ReplaceInPlace(row..row + 1), window, cx);
        if done {
            self.job = None;
            self.note("reply finished".to_owned());
        }
        cx.notify();
        true
    }

    fn transcript(&self, tokens: &ComponentTokens, cx: &mut Context<Self>) -> impl IntoElement {
        let session = &self.sessions[self.active];
        let messages = session.messages.clone();
        let session_ix = self.active;
        let colors = tokens.clone();
        let follow = session.follow.clone();
        let jump = follow.show_jump_button();
        let jump_probe = self.probes.jump.clone();
        let transcript_probe = self.probes.transcript.clone();
        jump_probe.set(None);
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(
                list(session.list.clone(), move |ix, _, _| {
                    let messages = messages.borrow();
                    let Some(message) = messages.get(ix) else {
                        return div().into_any_element();
                    };
                    let body = if message.user {
                        div()
                            .justify_end()
                            .flex()
                            .child(
                                div()
                                    .max_w(px(560.0))
                                    .px_3()
                                    .py_2()
                                    .rounded(colors.surface_radius)
                                    .bg(colors.control_surface)
                                    .child(Markdown::document(
                                        format!("user-{session_ix}-{ix}"),
                                        message.doc.clone(),
                                    )),
                            )
                            .into_any_element()
                    } else {
                        Markdown::document(format!("reply-{session_ix}-{ix}"), message.doc.clone())
                            .streaming(message.streaming)
                            .into_any_element()
                    };
                    div()
                        .w_full()
                        .px_4()
                        .py_2()
                        .child(div().max_w(px(780.0)).mx_auto().child(body))
                        .into_any_element()
                })
                .size_full(),
            )
            .children(jump.then(|| {
                let controller = follow.clone();
                div()
                    .id("jump-to-latest")
                    .absolute()
                    .right_4()
                    .bottom_4()
                    .px_3()
                    .py_1()
                    .rounded(tokens.button_radius)
                    .bg(tokens.accent)
                    .text_color(tokens.accent_contrast)
                    .cursor_pointer()
                    .on_painted(move |bounds, _, _| jump_probe.set(Some(bounds)))
                    .on_click(cx.listener(move |_, _, window, cx| {
                        controller.jump_to_bottom(window, cx);
                    }))
                    .child("Jump to latest")
            }))
            .on_painted(move |bounds, _, _| transcript_probe.set(Some(bounds)))
    }

    fn input_bar(&self, tokens: &ComponentTokens, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.is_streaming();
        let input = self.input.clone();
        div()
            .border_t_1()
            .border_color(tokens.border)
            .flex_row()
            .flex()
            .items_end()
            .gap_2()
            .p_3()
            .child(div().flex_1().min_w_0().child(self.input.clone()))
            .child(
                Button::new("send")
                    .label("Send")
                    .busy(busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let text = input.read(cx).source().to_owned();
                        this.submit(text, window, cx);
                    })),
            )
    }

    fn sidebar(&self, tokens: &ComponentTokens, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.probes.session_rows.clone();
        let streaming = self.streaming_target().map(|(session, _)| session);
        let items = self.sessions.iter().enumerate().map(|(ix, session)| {
            let active = ix == self.active;
            let highlight = tokens.highlight;
            div()
                .id(("session", ix))
                .px_3()
                .py_2()
                .rounded(tokens.button_radius)
                .cursor_pointer()
                .when(active, |row| row.bg(highlight))
                .when(!active, |row| row.hover(|row| row.bg(highlight)))
                .on_click(cx.listener(move |this, _, _, cx| this.select_session(ix, cx)))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .child(session.title.clone())
                        .children(
                            (streaming == Some(ix))
                                .then(|| div().text_color(tokens.accent).child("\u{25CF}")),
                        ),
                )
                .child(div().text_xs().text_color(tokens.text_muted).child(format!(
                    "{} \u{00B7} {} messages",
                    session.subtitle,
                    session.messages.borrow().len()
                )))
        });
        panel(tokens).child(caption(tokens, "SESSIONS")).child(
            div()
                .flex_col()
                .flex()
                .gap_1()
                .on_children_prepainted(move |bounds, _, _| *rows.borrow_mut() = bounds)
                .children(items),
        )
    }

    fn context_pane(&self, tokens: &ComponentTokens) -> impl IntoElement {
        let session = &self.sessions[self.active];
        let follow = &session.follow;
        panel(tokens)
            .child(caption(tokens, "CONTEXT"))
            .child(div().child(session.title.clone()))
            .child(div().text_sm().text_color(tokens.text_muted).child(format!(
                "{} messages \u{00B7} {} \u{00B7} {:.0} px from the end",
                session.messages.borrow().len(),
                if follow.is_pinned() {
                    "following"
                } else {
                    "detached"
                },
                follow.distance_from_bottom(),
            )))
            .child(caption(tokens, "ACTIVITY").mt_2())
            .children(self.log.iter().rev().map(|line| {
                div()
                    .text_xs()
                    .text_color(tokens.text_muted)
                    .child(line.clone())
            }))
            .child({
                let panes = self.panes.clone();
                Button::new("toggle-sidebar")
                    .label("Toggle sidebar")
                    .on_click(move |_, _, cx| {
                        panes.update(cx, |panes, cx| {
                            panes.toggle_collapsed(&PaneId::from(SIDEBAR), cx)
                        });
                    })
            })
    }
}

/// The column of a side pane: full size, padded, on the surface fill.
fn panel(tokens: &ComponentTokens) -> Div {
    div()
        .text_color(tokens.text)
        .bg(tokens.surface)
        .p_3()
        .flex_col()
        .gap_2()
        .size_full()
        .flex()
}

/// A small muted heading.
fn caption(tokens: &ComponentTokens, text: &'static str) -> Div {
    div().child(text).text_color(tokens.text_muted).text_sm()
}

impl Render for Workbench {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = ComponentTokens::current(cx);
        let nav = NavTheme::of(cx);
        let selected = self.selected_tab(cx);
        let content = match selected.as_deref() {
            Some(TAB_CHAT) => div()
                .size_full()
                .flex()
                .flex_col()
                .child(self.transcript(&tokens, cx))
                .child(self.input_bar(&tokens, cx))
                .into_any_element(),
            Some(TAB_PLAN) => div()
                .size_full()
                .p_4()
                .child(Markdown::document("plan", self.plan.clone()))
                .into_any_element(),
            Some(TAB_NOTES) => div()
                .size_full()
                .p_4()
                .child(self.notes.clone())
                .into_any_element(),
            _ => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(tokens.text_muted)
                .child("No tab is open")
                .into_any_element(),
        };
        let main = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(nav.background)
            .text_color(tokens.text)
            .child(div().px_2().pt_1().child(self.tabs.clone()))
            .child(cycle_on(div().flex_1().min_h_0().w_full(), &self.tabs).child(content));
        text_root(
            div().size_full().bg(nav.background).child(
                pane_group("workbench-panes", &self.panes)
                    .main(main)
                    .pane(SIDEBAR, self.sidebar(&tokens, cx))
                    .pane(CONTEXT, self.context_pane(&tokens)),
            ),
        )
    }
}
