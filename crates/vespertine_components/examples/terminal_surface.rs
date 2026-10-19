//! CMP-021 A16: the terminal surface hosted in a bottom dock.
//!
//! The terminal shows colored cells (the 16 ANSI colors, the 256-color cube and ramp, true
//! color), wide and combining characters and box drawing, echoes typed input locally (there is
//! no session: the demo host feeds the bytes the surface emits back as output, control bytes as
//! caret notation so the encoding is visible), accepts bracketed paste with control+shift+v,
//! and copies a selected range with control+shift+c (drag, double-click a word or triple-click
//! a line to select). The wheel scrolls the scrollback; dragging a selection past the top or
//! bottom edge scrolls on a timer. The dock above is plain content; drag nothing, resize the
//! window to see the grid follow.

use vespertine::{
    App, AppContext, Bounds, Context, Entity, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Render, Styled, Window, WindowBounds, WindowOptions, div, px, rgb, size,
};
use vespertine_components::terminal::{
    BottomDock, TabId, TerminalPanel, TerminalTheme, exit_status_line,
};
use vespertine_platform::application;
use vespertine_theme::builtin::neutral_variant;
use vespertine_theme::theme::Appearance;

fn banner() -> String {
    let mut text = String::new();
    text.push_str("\x1b[1mVespertine terminal surface\x1b[0m  (CMP-021)\r\n");
    text.push_str("16 colors:  ");
    for index in 0..8 {
        text.push_str(&format!("\x1b[4{index}m  \x1b[0m"));
    }
    text.push(' ');
    for index in 0..8 {
        text.push_str(&format!("\x1b[10{index}m  \x1b[0m"));
    }
    text.push_str("\r\n256 cube:  ");
    for index in (16..232).step_by(6) {
        text.push_str(&format!("\x1b[48;5;{index}m \x1b[0m"));
    }
    text.push_str("\r\nramp:      ");
    for index in 232..256 {
        text.push_str(&format!("\x1b[48;5;{index}m \x1b[0m"));
    }
    text.push_str("\r\ntrue color: ");
    for step in 0..32 {
        text.push_str(&format!(
            "\x1b[48;2;{};{};{}m \x1b[0m",
            step * 8,
            255 - step * 8,
            128
        ));
    }
    text.push_str("\r\n\x1b[1mbold\x1b[0m \x1b[2mdim\x1b[0m \x1b[3mitalic\x1b[0m ");
    text.push_str("\x1b[4munderline\x1b[0m \x1b[7minverse\x1b[0m \x1b[31;1mred\x1b[0m\r\n");
    text.push_str(
        "wide: \u{65e5}\u{672c}\u{8a9e} ok   box: \u{250c}\u{2500}\u{2500}\u{2510} ok   ",
    );
    text.push_str("combining: e\u{301}a\u{308} ok\r\n");
    text.push_str("\u{2502}ab\u{2502}\u{4e16}\u{2502}cd\u{2502}  <- columns stay aligned\r\n");
    text.push_str("Type to echo. Paste with ctrl-shift-v, copy with ctrl-shift-c.\r\n");
    text.push_str("\x1b[?2004h$ ");
    text
}

/// Echo of the bytes the surface emitted: control bytes as caret notation, Enter as a new
/// prompt, Backspace as an erase.
fn echo(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for &byte in bytes {
        match byte {
            b'\r' => out.extend_from_slice(b"\r\n$ "),
            0x7f => out.extend_from_slice(b"\x08 \x08"),
            0x1b => out.extend_from_slice(b"^["),
            b'\t' => out.extend_from_slice(b"    "),
            byte if byte < 0x20 => {
                out.push(b'^');
                out.push(byte + 0x40);
            }
            byte => out.push(byte),
        }
    }
    out
}

struct Demo {
    dock: Entity<BottomDock>,
}

impl Render for Demo {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.dock.clone())
    }
}

fn open_demo_tab(panel: &Entity<TerminalPanel>, cx: &mut App, exited: bool) -> TabId {
    panel.update(cx, |panel, cx| {
        let tab = panel.open_tab(cx);
        panel.feed(tab, banner().as_bytes(), cx);
        if exited {
            panel.feed(tab, &exit_status_line(0), cx);
            panel.mark_exited(tab, 0, cx);
        }
        tab
    })
}

fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(920.), px(600.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| {
                let panel = cx.new(|cx| {
                    let mut panel = TerminalPanel::new(cx);
                    // A host following the theme system leaves this unset; the demo pins a
                    // fixed-width family that is common on Linux desktops.
                    let mut theme = TerminalTheme::from_variant(&neutral_variant(Appearance::Dark));
                    theme.font_family = "Liberation Mono".into();
                    panel.set_theme(Some(theme), cx);
                    panel
                });
                let weak = panel.downgrade();
                panel.update(cx, |panel, _| {
                    panel.on_input(move |tab, bytes, cx| {
                        // The callback runs inside an update of the panel: feed the echo later.
                        let weak = weak.clone();
                        let reply = echo(bytes);
                        cx.defer(move |cx| {
                            weak.update(cx, |panel, cx| panel.feed(tab, &reply, cx)).ok();
                        });
                    });
                    panel.on_resize(|tab, cols, rows, _| {
                        println!("tab {tab:?}: grid {cols} x {rows}");
                    });
                    panel.on_title(|tab, title, _| println!("tab {tab:?}: title {title:?}"));
                });
                open_demo_tab(&panel, cx, false);
                panel.update(cx, |panel, cx| panel.request_focus(cx));
                let tabs = panel.clone();
                let dock = cx.new(|_| {
                    let strip_panel = tabs.clone();
                    BottomDock::new(panel.clone(), move |_, _| {
                        let new_tab = strip_panel.clone();
                        let first = strip_panel.clone();
                        div()
                            .size_full()
                            .bg(rgb(0x20242C))
                            .p(px(16.))
                            .text_color(rgb(0xD0D4DC))
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .child("Content above the terminal dock; the dock shares its height with this area.")
                            .child(
                                div()
                                    .id("open-tab")
                                    .cursor_pointer()
                                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                        open_demo_tab(&new_tab, cx, false);
                                    })
                                    .child("[ open another terminal tab ]"),
                            )
                            .child(
                                div()
                                    .id("first-tab")
                                    .cursor_pointer()
                                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                        first.update(cx, |panel, cx| panel.select_tab(0, cx));
                                    })
                                    .child("[ show the first tab ]"),
                            )
                            .into_any_element()
                    })
                    .with_height(300.0)
                });
                let _ = window;
                cx.new(|_| Demo { dock })
            },
        )
        .expect("window");
        cx.activate(true);
    });
}
