//! CMP-028: badges, notices and review cards in the offscreen test host.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use vespertine::{
    AnyElement, AppContext as _, Bounds, Context, Entity, Focusable, InteractiveElement,
    IntoElement, ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled,
    Window, canvas, div, px, rgb,
};
use vespertine_components::badges::{
    BADGE_HEIGHT, BadgeCards, BadgeDescriptor, BadgeTokens, CardCallbacks, CommentContent,
    DRAFT_HEIGHT, DRAFT_INPUT_HEIGHT, DetailRow, ICON_COMMENT, IconAssets, NoticeAction,
    NoticeVariant, Severity, card_height, draft_input, notice_chip, render_adder, render_badge,
    render_card, render_draft,
};
use vespertine_components::dialog::Dialog;
use vespertine_components::text::{TextArea, area::CHROME_PADDING_Y};
use vespertine_richtext::text_root;
use vespertine_test_host::{DEFAULT_SCALE_FACTOR, HostOptions, TestHost};

type Sink = Rc<Cell<Option<Bounds<Pixels>>>>;

fn sink() -> Sink {
    Rc::new(Cell::new(None))
}

/// Wraps `child` so that its painted bounds are recorded in `sink`.
fn probed(sink: &Sink, child: impl IntoElement) -> AnyElement {
    let sink = sink.clone();
    div()
        .relative()
        .flex()
        .flex_row()
        .min_w_0()
        .flex_shrink_1()
        .child(child)
        .child(
            canvas(move |bounds, _, _| sink.set(Some(bounds)), |_, _, _, _| {})
                .absolute()
                .size_full(),
        )
        .into_any_element()
}

fn options(width: f64, height: f64) -> HostOptions {
    let mut options = HostOptions::with_size(width, height);
    options.assets = Some(Arc::new(IconAssets));
    options
}

fn bounds(sink: &Sink) -> Bounds<Pixels> {
    sink.get().expect("probe painted")
}

fn contains_text(host: &TestHost, needle: &str) -> usize {
    host.painted_text()
        .iter()
        .map(|text| text.matches(needle).count())
        .sum()
}

// --- A2 ---------------------------------------------------------------------------------------

#[test]
fn badge_height_is_fixed_and_a_long_label_truncates_without_moving_neighbors() {
    if !vespertine_test_host::is_available() {
        return;
    }
    let label: Rc<RefCell<String>> = Rc::new(RefCell::new("ok".to_string()));
    let (badge_sink, neighbor_sink) = (sink(), sink());
    let tokens = BadgeTokens::neutral();
    let mut host = TestHost::with_element(options(400.0, 120.0), {
        let label = label.clone();
        let (badge_sink, neighbor_sink) = (badge_sink.clone(), neighbor_sink.clone());
        move |_, _| {
            div()
                .size_full()
                .p(px(10.))
                .flex()
                .flex_row()
                .items_start()
                .gap(px(8.))
                .child(div().w(px(120.)).flex().child(probed(
                    &badge_sink,
                    render_badge(
                        "badge",
                        BadgeDescriptor::new(ICON_COMMENT, label.borrow().clone()).count(3),
                        &tokens,
                    ),
                )))
                .child(probed(
                    &neighbor_sink,
                    div().w(px(40.)).h(px(24.)).bg(rgb(0x335577)),
                ))
                .into_any_element()
        }
    })
    .expect("host");
    host.flush();
    let short_height = bounds(&badge_sink).size.height;
    let short_neighbor = bounds(&neighbor_sink).origin;
    assert_eq!(f32::from(short_height), BADGE_HEIGHT);

    *label.borrow_mut() = "a label that is far too long for the width the parent allots".repeat(3);
    host.paint_frame();
    let long = bounds(&badge_sink);
    assert_eq!(
        f32::from(long.size.height),
        BADGE_HEIGHT,
        "height never depends on the label"
    );
    assert!(
        f32::from(long.size.width) <= 120.01,
        "the badge shrinks to its allotment"
    );
    assert_eq!(
        bounds(&neighbor_sink).origin,
        short_neighbor,
        "neighbors never reflow"
    );
    assert!(
        host.painted_text()
            .iter()
            .any(|text| text.contains('…') || text.contains("...")),
        "the label truncates with an ellipsis: {:?}",
        host.painted_text()
    );
}

// --- A3 ---------------------------------------------------------------------------------------

#[test]
fn hover_details_show_one_card_after_280_ms_and_never_before() {
    if !vespertine_test_host::is_available() {
        return;
    }
    let (with_sink, without_sink) = (sink(), sink());
    let tokens = BadgeTokens::neutral();
    let mut host = TestHost::with_element(options(600.0, 300.0), {
        let (with_sink, without_sink) = (with_sink.clone(), without_sink.clone());
        move |_, _| {
            div()
                .size_full()
                .p(px(10.))
                .flex()
                .flex_row()
                .items_start()
                .gap(px(40.))
                .child(probed(
                    &with_sink,
                    render_badge(
                        "with",
                        BadgeDescriptor::new(ICON_COMMENT, "with details").details(vec![
                            DetailRow::new("src/a.rs:1", "UNIQUE-DETAIL-BODY").tag("tag"),
                        ]),
                        &tokens,
                    ),
                ))
                .child(probed(
                    &without_sink,
                    render_badge(
                        "without",
                        BadgeDescriptor::new(ICON_COMMENT, "no details"),
                        &tokens,
                    ),
                ))
                .into_any_element()
        }
    })
    .expect("host");
    host.flush();
    let center = |b: Bounds<Pixels>| {
        (
            f64::from(f32::from(b.origin.x + b.size.width / 2.0)),
            f64::from(f32::from(b.origin.y + b.size.height / 2.0)),
        )
    };
    let (x, y) = center(bounds(&without_sink));
    host.mouse_move(x, y, None, "").expect("move");
    host.advance_executor_time(700.0).expect("clock");
    assert_eq!(contains_text(&host, "UNIQUE-DETAIL-BODY"), 0);
    assert_eq!(
        host.painted_text().len(),
        2,
        "a badge without details carries no card"
    );

    let (x, y) = center(bounds(&with_sink));
    host.mouse_move(x, y, None, "").expect("move");
    host.advance_executor_time(1.0).expect("clock");
    host.mouse_move(x + 1.0, y, None, "").expect("move");
    host.advance_executor_time(270.0).expect("clock");
    assert_eq!(
        contains_text(&host, "UNIQUE-DETAIL-BODY"),
        0,
        "not before 280 ms"
    );
    host.advance_executor_time(20.0).expect("clock");
    assert_eq!(
        contains_text(&host, "UNIQUE-DETAIL-BODY"),
        1,
        "exactly one card after 280 ms"
    );
}

struct SharedCards {
    cards: BadgeCards,
    tokens: BadgeTokens,
    sinks: [Sink; 2],
}

impl Render for SharedCards {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let badge = |id: &'static str, label: &str, body: &str, sink: &Sink| {
            probed(
                sink,
                render_badge(
                    id,
                    BadgeDescriptor::new(ICON_COMMENT, label.to_string())
                        .details(vec![DetailRow::new("src/a.rs:1", body.to_string())]),
                    &self.tokens,
                )
                .cards(&self.cards),
            )
        };
        div()
            .size_full()
            .p(px(10.))
            .flex()
            .flex_row()
            .items_start()
            .gap(px(40.))
            .child(badge("first", "first", "FIRST-DETAIL-BODY", &self.sinks[0]))
            .child(badge(
                "second",
                "second",
                "SECOND-DETAIL-BODY",
                &self.sinks[1],
            ))
            .child(self.cards.layer())
    }
}

#[test]
fn badges_sharing_a_card_host_use_the_hover_card_rules() {
    if !vespertine_test_host::is_available() {
        return;
    }
    let sinks = [sink(), sink()];
    let mut host = TestHost::with_view(options(600.0, 300.0), {
        let sinks = sinks.clone();
        move |_, cx| {
            let tokens = BadgeTokens::neutral();
            let cards = BadgeCards::new(&tokens, cx);
            cx.new(|_| SharedCards {
                cards,
                tokens,
                sinks,
            })
        }
    })
    .expect("host");
    host.flush();
    let center = |b: Bounds<Pixels>| {
        (
            f64::from(f32::from(b.origin.x + b.size.width / 2.0)),
            f64::from(f32::from(b.origin.y + b.size.height / 2.0)),
        )
    };
    let (first_x, first_y) = center(bounds(&sinks[0]));
    let (second_x, second_y) = center(bounds(&sinks[1]));

    host.mouse_move(first_x, first_y, None, "").expect("move");
    host.advance_executor_time(1.0).expect("clock");
    host.mouse_move(first_x + 1.0, first_y, None, "")
        .expect("move");
    host.advance_executor_time(270.0).expect("clock");
    assert_eq!(
        contains_text(&host, "FIRST-DETAIL-BODY"),
        0,
        "not before 280 ms"
    );
    host.advance_executor_time(20.0).expect("clock");
    assert_eq!(contains_text(&host, "FIRST-DETAIL-BODY"), 1);

    // A neighbor opens at once while a card is shown (the warm window of CMP-004 R12) and the
    // first card gives way: exactly one card is on screen.
    host.mouse_move(second_x, second_y, None, "").expect("move");
    host.advance_executor_time(1.0).expect("clock");
    assert_eq!(contains_text(&host, "SECOND-DETAIL-BODY"), 1);
    assert_eq!(contains_text(&host, "FIRST-DETAIL-BODY"), 0);

    // A press dismisses cold: hovering again waits for the dwell.
    host.click(second_x, second_y, 0, "").expect("click");
    host.advance_executor_time(1.0).expect("clock");
    assert_eq!(contains_text(&host, "SECOND-DETAIL-BODY"), 0);
}

// --- A5 ---------------------------------------------------------------------------------------

#[test]
fn the_copy_action_copies_the_full_message_and_does_not_reach_the_parent() {
    if !vespertine_test_host::is_available() {
        return;
    }
    let message = "exit status 2\nstderr: error: something went wrong while compiling the crate and the whole text must stay readable even when it is long enough to wrap onto several lines of the chip";
    let chip_sink = sink();
    let parent_clicks = Rc::new(Cell::new(0));
    let tokens = BadgeTokens::neutral();
    let mut host = TestHost::with_element(options(400.0, 400.0), {
        let chip_sink = chip_sink.clone();
        let parent_clicks = parent_clicks.clone();
        move |_, _| {
            let clicks = parent_clicks.clone();
            let chip = notice_chip(
                "notice",
                &tokens,
                Severity::Error,
                "Failed",
                message,
                NoticeVariant::Plain,
                None,
            )
            .w(px(240.))
            .on_click(move |_, _, _| clicks.set(clicks.get() + 1));
            text_root(
                div()
                    .size_full()
                    .p(px(20.))
                    .flex()
                    .flex_row()
                    .items_start()
                    .child(probed(&chip_sink, chip)),
            )
            .into_any_element()
        }
    })
    .expect("host");
    host.flush();
    let chip = bounds(&chip_sink);
    assert!(
        f32::from(chip.size.height) > 8.0 + 20.0 + 6.0 + 16.0 * 4.0 + 8.0,
        "the long multi-line message wraps instead of truncating: {:?}",
        chip.size
    );
    // The copy button sits at the right end of the header row.
    let copy_x = f64::from(f32::from(chip.right())) - 12.0 - 10.0;
    let copy_y = f64::from(f32::from(chip.top())) + 8.0 + 10.0;
    host.click(copy_x, copy_y, 0, "").expect("click");
    host.flush();
    let copied = host
        .app()
        .update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(copied.as_deref(), Some(message));
    assert_eq!(
        parent_clicks.get(),
        0,
        "the copy click never reaches the parent handler"
    );
    // A click on the message body does reach the parent.
    host.click(
        copy_x - 60.0,
        f64::from(f32::from(chip.bottom())) - 12.0,
        0,
        "",
    )
    .expect("click");
    host.flush();
    assert_eq!(parent_clicks.get(), 1);
}

#[test]
fn a_notice_action_runs_its_callback() {
    if !vespertine_test_host::is_available() {
        return;
    }
    let chip_sink = sink();
    let retried = Rc::new(Cell::new(0));
    let tokens = BadgeTokens::neutral();
    let mut host = TestHost::with_element(options(400.0, 300.0), {
        let chip_sink = chip_sink.clone();
        let retried = retried.clone();
        move |_, _| {
            let retried = retried.clone();
            let chip = notice_chip(
                "notice",
                &tokens,
                Severity::Warning,
                "Slow",
                "took long",
                NoticeVariant::Tile,
                Some(NoticeAction::new("Retry", move |_, _, _| {
                    retried.set(retried.get() + 1)
                })),
            )
            .w(px(300.));
            text_root(
                div()
                    .p(px(20.))
                    .flex()
                    .flex_row()
                    .items_start()
                    .child(probed(&chip_sink, chip)),
            )
            .into_any_element()
        }
    })
    .expect("host");
    host.flush();
    let chip = bounds(&chip_sink);
    host.click(
        f64::from(f32::from(chip.right())) - 30.0,
        f64::from(f32::from(chip.bottom())) - 8.0 - 11.0,
        0,
        "",
    )
    .expect("click");
    host.flush();
    assert_eq!(retried.get(), 1);
}

// --- A6, A7, A9: review cards ------------------------------------------------------------------

struct Review {
    body: SharedString,
    editing: bool,
    input: Entity<TextArea>,
    tokens: BadgeTokens,
    card_sink: Sink,
    draft_sink: Sink,
    cancels: usize,
}

impl Render for Review {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity();
        let element = if self.editing {
            let (cancel, commit) = (this.clone(), this);
            probed(
                &self.draft_sink,
                render_draft(
                    "c1",
                    "src/a.rs:3",
                    self.input.clone(),
                    true,
                    move |_, cx| {
                        cancel.update(cx, |review, cx| {
                            review.editing = false;
                            review.cancels += 1;
                            cx.notify();
                        })
                    },
                    move |_, cx| {
                        commit.update(cx, |review, cx| {
                            review.body = review.input.read(cx).source().to_string().into();
                            review.editing = false;
                            cx.notify();
                        })
                    },
                    None,
                    &self.tokens,
                ),
            )
        } else {
            let (edit, remove) = (this.clone(), this);
            probed(
                &self.card_sink,
                render_card(
                    CommentContent {
                        id: "c1".into(),
                        location: "src/a.rs:3".into(),
                        body: self.body.clone(),
                    },
                    CardCallbacks::new(
                        move |_, window, cx| {
                            edit.update(cx, |review, cx| {
                                review.editing = true;
                                let body = review.body.clone();
                                review
                                    .input
                                    .update(cx, |input, cx| input.set_value(&body, window, cx));
                                let handle = review.input.focus_handle(cx);
                                window.focus(&handle, cx);
                                cx.notify();
                            })
                        },
                        move |_, _, cx| {
                            remove.update(cx, |review, cx| {
                                review.body = "removed".into();
                                cx.notify();
                            })
                        },
                    ),
                    None,
                    &self.tokens,
                ),
            )
        };
        div()
            .size_full()
            .p(px(10.))
            .child(div().w(px(400.)).child(element))
    }
}

/// `(bottom, mask bottom, width, height, alpha)` of a glyph or icon sprite, in logical pixels.
type Sprite = (f32, f32, f32, f32, f32);

struct ReviewHarness {
    host: TestHost,
    review: Entity<Review>,
    card_sink: Sink,
    draft_sink: Sink,
    sprites: Rc<RefCell<Vec<Sprite>>>,
}

fn review_harness(body: &str) -> Option<ReviewHarness> {
    if !vespertine_test_host::is_available() {
        return None;
    }
    let (card_sink, draft_sink) = (sink(), sink());
    let sprites: Rc<RefCell<Vec<Sprite>>> = Rc::default();
    let slot: Rc<RefCell<Option<Entity<Review>>>> = Rc::default();
    let body = body.to_string();
    let mut options = options(500.0, 400.0);
    let observed = sprites.clone();
    options.scene_observer = Some(Box::new(move |scene| {
        let scale = DEFAULT_SCALE_FACTOR;
        let mut sprites = observed.borrow_mut();
        sprites.clear();
        for sprite in &scene.monochrome_sprites {
            sprites.push((
                sprite.bounds.bottom().0 / scale,
                sprite.content_mask.bounds.bottom().0 / scale,
                sprite.bounds.size.width.0 / scale,
                sprite.bounds.size.height.0 / scale,
                sprite.color.a,
            ));
        }
        for sprite in &scene.subpixel_sprites {
            sprites.push((
                sprite.bounds.bottom().0 / scale,
                sprite.content_mask.bounds.bottom().0 / scale,
                sprite.bounds.size.width.0 / scale,
                sprite.bounds.size.height.0 / scale,
                sprite.color.a,
            ));
        }
    }));
    let host = TestHost::with_view(options, {
        let (card_sink, draft_sink) = (card_sink.clone(), draft_sink.clone());
        let slot = slot.clone();
        move |window, cx| {
            let tokens = BadgeTokens::neutral();
            let input = cx.new(|cx| draft_input("", &tokens, window, cx));
            let review = cx.new(|_| Review {
                body: body.into(),
                editing: false,
                input,
                tokens,
                card_sink,
                draft_sink,
                cancels: 0,
            });
            *slot.borrow_mut() = Some(review.clone());
            review
        }
    })
    .expect("host");
    let review = slot.borrow_mut().take().expect("review");
    let mut harness = ReviewHarness {
        host,
        review,
        card_sink,
        draft_sink,
        sprites,
    };
    harness.host.flush();
    Some(harness)
}

impl ReviewHarness {
    fn read<R>(&mut self, f: impl FnOnce(&Review) -> R) -> R {
        let review = self.review.clone();
        self.host.app().update(|cx| f(review.read(cx)))
    }

    fn input_text(&mut self) -> String {
        let review = self.review.clone();
        self.host
            .app()
            .update(|cx| review.read(cx).input.read(cx).source().to_string())
    }

    /// Presents the last drawn frame so the scene observer sees it.
    fn present(&mut self) {
        self.host.flush();
        let window = self.host.window();
        self.host
            .app()
            .update_window(window, |_, window, _| window.present_cached_frame())
            .expect("window");
    }

    /// Visible icon sprites (about 12 px square with real alpha).
    fn visible_icons(&mut self) -> usize {
        self.present();
        self.sprites
            .borrow()
            .iter()
            .filter(|(_, _, w, h, alpha)| {
                (9.0..=14.0).contains(w) && (9.0..=14.0).contains(h) && *alpha > 0.5
            })
            .count()
    }
}

#[test]
fn a_long_body_keeps_the_analytic_height_and_never_paints_outside_the_card() {
    let long = (0..20)
        .map(|n| format!("paragraph {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let Some(mut h) = review_harness(&long) else {
        return;
    };
    h.present();
    let card = bounds(&h.card_sink);
    assert_eq!(f32::from(card.size.height), card_height(&long));
    assert_eq!(card_height(&long), 192.0);
    let bottom = f32::from(card.bottom());
    let sprites = h.sprites.borrow();
    assert!(!sprites.is_empty(), "text was painted");
    for (sprite_bottom, mask_bottom, ..) in sprites.iter() {
        assert!(
            *sprite_bottom <= bottom + 0.5 || *mask_bottom <= bottom + 0.5,
            "a glyph reaches below the card without being clipped: {sprite_bottom} / {mask_bottom} vs {bottom}"
        );
    }
}

fn header_y(card: Bounds<Pixels>) -> f64 {
    f64::from(f32::from(card.top())) + 10.0 + 11.0
}

#[test]
fn edit_then_cancel_restores_the_card_and_the_draft_is_116_px() {
    let Some(mut h) = review_harness("original body") else {
        return;
    };
    let card = bounds(&h.card_sink);
    assert_eq!(f32::from(card.size.height), card_height("original body"));
    // Header: [icon][location][edit 16][6][remove 16] inside 16 px of padding.
    let edit_x = f64::from(f32::from(card.right())) - 16.0 - 16.0 - 6.0 - 8.0;
    h.host
        .mouse_move(edit_x, header_y(card), None, "")
        .expect("move");
    h.host.flush();
    h.host.click(edit_x, header_y(card), 0, "").expect("click");
    h.host.flush();
    assert!(
        h.read(|review| review.editing),
        "the edit action opened the draft"
    );
    assert_eq!(f32::from(bounds(&h.draft_sink).size.height), DRAFT_HEIGHT);
    assert_eq!(
        h.host
            .painted_text()
            .iter()
            .filter(|text| text.as_str() == "Save")
            .count(),
        1,
        "the primary action reads Save when editing"
    );
    assert_eq!(
        h.input_text(),
        "original body",
        "the editor starts with the card's body"
    );
    h.host.simulate_keystrokes("x y", None).expect("keys");
    assert_eq!(h.input_text(), "original bodyxy");
    h.host.simulate_keystrokes("escape", None).expect("keys");
    h.host.flush();
    assert!(!h.read(|review| review.editing), "Escape cancels the draft");
    assert_eq!(h.read(|review| review.cancels), 1);
    assert_eq!(h.read(|review| review.body.to_string()), "original body");
    assert_eq!(
        f32::from(bounds(&h.card_sink).size.height),
        card_height("original body")
    );
}

#[test]
fn the_cancel_button_discards_the_edits_and_a_new_draft_reads_comment() {
    let Some(mut h) = review_harness("kept") else {
        return;
    };
    let card = bounds(&h.card_sink);
    let edit_x = f64::from(f32::from(card.right())) - 16.0 - 16.0 - 6.0 - 8.0;
    h.host
        .mouse_move(edit_x, header_y(card), None, "")
        .expect("move");
    h.host.flush();
    h.host.click(edit_x, header_y(card), 0, "").expect("click");
    h.host.flush();
    h.host.simulate_keystrokes("z", None).expect("keys");
    // Cancel is the left of the two buttons in the right-aligned action row at the bottom.
    let draft = bounds(&h.draft_sink);
    let y = f64::from(f32::from(draft.bottom())) - 10.0 - 14.0;
    let cancel_x = f64::from(f32::from(draft.right())) - 16.0 - 60.0 - 6.0 - 30.0;
    h.host.click(cancel_x, y, 0, "").expect("click");
    h.host.flush();
    assert!(!h.read(|review| review.editing));
    assert_eq!(h.read(|review| review.body.to_string()), "kept");
}

struct DialogDraft {
    input: Entity<TextArea>,
    editing: bool,
    cancels: usize,
    tokens: BadgeTokens,
}

impl Render for DialogDraft {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity();
        let draft = self.editing.then(|| {
            let (cancel, commit) = (this.clone(), this);
            render_draft(
                "c1",
                "src/lib.rs:3",
                self.input.clone(),
                false,
                move |_, cx| {
                    cancel.update(cx, |draft, cx| {
                        draft.editing = false;
                        draft.cancels += 1;
                        cx.notify();
                    })
                },
                move |_, cx| commit.update(cx, |draft, _| draft.editing = false),
                None,
                &self.tokens,
            )
        });
        div().w(px(380.)).children(draft)
    }
}

#[test]
fn escape_in_a_draft_inside_a_dialog_cancels_the_draft_before_the_dialog() {
    if !vespertine_test_host::is_available() {
        return;
    }
    let slot: Rc<RefCell<Option<(Entity<Dialog>, Entity<DialogDraft>)>>> = Rc::default();
    let mut host = TestHost::with_view(options(600.0, 500.0), {
        let slot = slot.clone();
        move |window, cx| {
            let tokens = BadgeTokens::neutral();
            let input = cx.new(|cx| draft_input("", &tokens, window, cx));
            let draft = cx.new(|_| DialogDraft {
                input,
                editing: true,
                cancels: 0,
                tokens,
            });
            let dialog = cx.new(|cx| Dialog::new("dialog", "Lorem ipsum", cx).child(draft.clone()));
            *slot.borrow_mut() = Some((dialog.clone(), draft));
            cx.new(|_| DialogPage { dialog })
        }
    })
    .expect("host");
    let (dialog, draft) = slot.borrow_mut().take().expect("entities");
    let window = host.window();
    host.app()
        .update_window(window, |_, window, cx| {
            dialog.update(cx, |dialog, cx| dialog.open(window, cx))
        })
        .unwrap();
    host.flush();
    host.app()
        .update_window(window, |_, window, cx| {
            let handle = draft.read(cx).input.focus_handle(cx);
            handle.focus(window, cx);
        })
        .unwrap();
    host.flush();
    host.key_down("escape", false, None).unwrap();
    host.flush();
    assert_eq!(host.app().update(|cx| draft.read(cx).cancels), 1);
    assert!(
        host.app().update(|cx| dialog.read(cx).is_open()),
        "the dialog stays open"
    );
    // With the draft gone, the next Escape reaches the dialog.
    host.key_down("escape", false, None).unwrap();
    host.flush();
    assert!(!host.app().update(|cx| dialog.read(cx).is_open()));
}

struct DialogPage {
    dialog: Entity<Dialog>,
}

impl Render for DialogPage {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.dialog.clone())
    }
}

#[test]
fn a_long_draft_scrolls_inside_the_editor_and_keeps_the_116_px_height() {
    let Some(mut h) = review_harness("short") else {
        return;
    };
    let card = bounds(&h.card_sink);
    let edit_x = f64::from(f32::from(card.right())) - 16.0 - 16.0 - 6.0 - 8.0;
    h.host
        .mouse_move(edit_x, header_y(card), None, "")
        .expect("move");
    h.host.flush();
    h.host.click(edit_x, header_y(card), 0, "").expect("click");
    h.host.flush();
    let long = (0..30)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let review = h.review.clone();
    let window = h.host.window();
    h.host
        .app()
        .update_window(window, |_, window, cx| {
            let input = review.read(cx).input.clone();
            input.update(cx, |input, cx| input.set_value(&long, window, cx));
        })
        .expect("window");
    h.host.flush();
    h.host.flush();
    assert_eq!(f32::from(bounds(&h.draft_sink).size.height), DRAFT_HEIGHT);
    let review = h.review.clone();
    let viewport = h
        .host
        .app()
        .update(|cx| review.read(cx).input.read(cx).viewport_bounds())
        .expect("the editor painted");
    assert!(
        f32::from(viewport.size.height) + CHROME_PADDING_Y <= DRAFT_INPUT_HEIGHT + 0.01,
        "the editor stays inside the 46 px input region: {viewport:?}"
    );
}

#[test]
fn review_actions_show_on_hover_or_keyboard_focus_and_keep_their_layout_space() {
    let Some(mut h) = review_harness("body") else {
        return;
    };
    let hidden = h.visible_icons();
    // Hidden actions stay in the accessibility tree.
    let tree = h.host.accessibility_tree().expect("tree");
    let dump = serde_json::to_string(&tree).expect("json");
    assert!(
        dump.contains("Edit comment") && dump.contains("Remove comment"),
        "{dump}"
    );
    let card = bounds(&h.card_sink);
    let height_hidden = card.size.height;

    // Hover the card: the actions appear and nothing moves.
    h.host
        .mouse_move(
            f64::from(f32::from(card.left())) + 100.0,
            header_y(card),
            None,
            "",
        )
        .expect("move");
    h.host.flush();
    let hovered = h.visible_icons();
    assert_eq!(hovered, hidden + 2, "the two action icons became visible");
    assert_eq!(bounds(&h.card_sink).size.height, height_hidden);

    // Leave and use the keyboard instead.
    h.host.mouse_move(480.0, 390.0, None, "").expect("move");
    h.host.flush();
    assert_eq!(h.visible_icons(), hidden);
    let window = h.host.window();
    h.host
        .app()
        .update_window(window, |_, window, cx| window.focus_next(cx))
        .expect("window");
    h.host.flush();
    assert_eq!(
        h.visible_icons(),
        hidden + 2,
        "keyboard focus on an action reveals them"
    );
}

// --- The add affordance -------------------------------------------------------------------------

#[test]
fn the_adder_opens_without_reaching_the_row_and_is_labelled() {
    if !vespertine_test_host::is_available() {
        return;
    }
    let opened = Rc::new(Cell::new(0));
    let row_clicks = Rc::new(Cell::new(0));
    let tokens = BadgeTokens::neutral();
    let adder_sink = sink();
    let mut host = TestHost::with_element(options(200.0, 100.0), {
        let (opened, row_clicks, adder_sink) =
            (opened.clone(), row_clicks.clone(), adder_sink.clone());
        move |_, _| {
            let (opened, row_clicks) = (opened.clone(), row_clicks.clone());
            div()
                .id("row")
                .size_full()
                .p(px(10.))
                .flex()
                .flex_row()
                .items_start()
                .on_click(move |_, _, _| row_clicks.set(row_clicks.get() + 1))
                .child(probed(
                    &adder_sink,
                    render_adder("adder", move |_, _| opened.set(opened.get() + 1), &tokens),
                ))
                .into_any_element()
        }
    })
    .expect("host");
    host.flush();
    let adder = bounds(&adder_sink);
    assert_eq!(f32::from(adder.size.width), 16.0);
    assert_eq!(f32::from(adder.size.height), 16.0);
    host.click(
        f64::from(f32::from(adder.left())) + 8.0,
        f64::from(f32::from(adder.top())) + 8.0,
        0,
        "",
    )
    .expect("click");
    host.flush();
    assert_eq!(opened.get(), 1);
    assert_eq!(row_clicks.get(), 0);
    let tree = host.accessibility_tree().expect("tree");
    assert!(
        serde_json::to_string(&tree)
            .expect("json")
            .contains("Add comment")
    );
}
