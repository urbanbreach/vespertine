//! CMP-028 A1: inline badges, notices and review cards.
//!
//! Shows a long badge that truncates without moving its neighbors, two badges with hover details
//! sharing one hover-card host (the first card appears after 280 ms, the neighbor's at once), a
//! notice whose message can be selected and copied with its
//! copy button, an activity pill and a corner count badge, and a review card: hover it, edit the
//! comment (a bounded draft in a text area opens), type, then cancel with Escape or the Cancel
//! button, which restores the card unchanged.

use vespertine::{
    App, AppContext, Bounds, Context, Entity, Focusable, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px,
    rgb, size,
};
use vespertine_components::badges::{
    BadgeCards, BadgeDescriptor, BadgeTokens, CardCallbacks, CommentContent, DetailRow,
    ICON_COMMENT, ICON_WARNING, IconAssets, NoticeAction, NoticeVariant, Severity, activity_pill,
    corner_count_badge, draft_input, notice_chip, render_adder, render_badge, render_card,
    render_draft,
};
use vespertine_components::text::TextArea;
use vespertine_platform::application;
use vespertine_richtext::text_root;

struct Gallery {
    tokens: BadgeTokens,
    body: SharedString,
    editing: bool,
    input: Entity<TextArea>,
    cards: BadgeCards,
    notice_clicks: usize,
    long_label: bool,
}

impl Gallery {
    fn open_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let body = self.body.to_string();
        self.input
            .update(cx, |input, cx| input.set_value(&body, window, cx));
        self.editing = true;
        window.focus(&self.input.focus_handle(cx), cx);
        cx.notify();
    }

    fn close_editor(&mut self, commit: bool, cx: &mut Context<Self>) {
        if commit {
            self.body = self.input.read(cx).source().to_string().into();
        }
        self.editing = false;
        cx.notify();
    }
}

impl Render for Gallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = self.tokens.clone();
        let gallery = cx.entity();
        let label = if self.long_label {
            "a very long badge label that cannot possibly fit the width the strip allots to it"
        } else {
            "short"
        };
        let strip = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .child(
                div().w(px(180.)).flex().child(
                    render_badge(
                        "long-badge",
                        BadgeDescriptor::new(ICON_COMMENT, label).count(120),
                        &tokens,
                    )
                    .on_click({
                        let gallery = gallery.clone();
                        move |_, _, cx| {
                            gallery.update(cx, |gallery, cx| {
                                gallery.long_label = !gallery.long_label;
                                cx.notify();
                            })
                        }
                    }),
                ),
            )
            .child(
                div()
                    .text_color(tokens.muted)
                    .child("<- neighbors stay put"),
            )
            .child(
                render_badge(
                    "details-badge",
                    BadgeDescriptor::new(ICON_WARNING, "hover me")
                        .status("2 findings")
                        .details(vec![
                            DetailRow::new(
                                "src/lib.rs:42",
                                "The detail card appears after 280 ms.",
                            )
                            .tag("note"),
                            DetailRow::new("src/main.rs:7", "Rows are separated by four pixels."),
                        ]),
                    &tokens,
                )
                .cards(&self.cards),
            )
            .child(
                render_badge(
                    "neighbor-badge",
                    BadgeDescriptor::new(ICON_COMMENT, "then me").details(vec![DetailRow::new(
                        "src/lib.rs:50",
                        "A neighbor opens at once while a card is shown.",
                    )]),
                    &tokens,
                )
                .cards(&self.cards),
            )
            .child(self.cards.layer())
            .children(activity_pill("pill", 3, 0.8, &tokens))
            .child(
                div()
                    .relative()
                    .size(px(28.))
                    .rounded(px(6.))
                    .bg(tokens.wash(0.1))
                    .child(corner_count_badge(105, &tokens)),
            );

        let notice = {
            let gallery = gallery.clone();
            notice_chip(
                "notice",
                &tokens,
                Severity::Error,
                "Command failed",
                "exit status 2\nstderr: error: could not compile `demo` (lib) due to 3 previous errors; the whole message wraps instead of truncating and can be selected and copied.",
                NoticeVariant::Plain,
                Some(NoticeAction::new("Retry", |_, _, _| println!("retry"))),
            )
            .w(px(420.))
            .cursor_pointer()
            .on_click(move |_, _, cx| {
                gallery.update(cx, |gallery, _| {
                    gallery.notice_clicks += 1;
                    println!("notice clicked {}", gallery.notice_clicks);
                })
            })
        };

        let review = if self.editing {
            let (cancel, commit) = (gallery.clone(), gallery);
            render_draft(
                "comment-1",
                "src/lib.rs:42",
                self.input.clone(),
                true,
                move |_, cx| cancel.update(cx, |gallery, cx| gallery.close_editor(false, cx)),
                move |_, cx| commit.update(cx, |gallery, cx| gallery.close_editor(true, cx)),
                None,
                &tokens,
            )
            .into_any_element()
        } else {
            let (edit, remove) = (gallery.clone(), gallery);
            render_card(
                CommentContent {
                    id: "comment-1".into(),
                    location: "src/lib.rs:42".into(),
                    body: self.body.clone(),
                },
                CardCallbacks::new(
                    move |_, window, cx| {
                        edit.update(cx, |gallery, cx| gallery.open_editor(window, cx))
                    },
                    move |_, _, cx| {
                        remove.update(cx, |gallery, cx| {
                            gallery.body = "(removed: edit to restore)".into();
                            cx.notify();
                        })
                    },
                ),
                None,
                &tokens,
            )
            .into_any_element()
        };

        text_root(
            div()
                .size_full()
                .bg(rgb(0x181A1F))
                .text_color(tokens.text)
                .text_size(px(13.))
                .p(px(24.))
                .flex()
                .flex_col()
                .gap(px(20.))
                .child(strip)
                .child(notice)
                .child(div().w(px(520.)).child(review))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(render_adder(
                            "adder",
                            |_, _| println!("add comment"),
                            &tokens,
                        ))
                        .child(div().text_color(tokens.muted).child("add affordance")),
                ),
        )
    }
}

fn main() {
    application().with_assets(IconAssets).run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(760.), px(560.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| {
                let tokens = BadgeTokens::neutral();
                let input = cx.new(|cx| draft_input("", &tokens, window, cx));
                let cards = BadgeCards::new(&tokens, cx);
                cx.new(|_| Gallery {
                    tokens,
                    body: "Consider extracting this block into a helper.\nIt is used twice.".into(),
                    editing: false,
                    input,
                    cards,
                    notice_clicks: 0,
                    long_label: true,
                })
            },
        )
        .expect("window");
        cx.activate(true);
    });
}
