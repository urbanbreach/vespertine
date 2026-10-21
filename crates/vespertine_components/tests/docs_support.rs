//! Shared helpers of the document component tests (code, Markdown, math, diff).
#![allow(dead_code)]

use std::cell::RefCell;
use std::rc::Rc;
use vespertine::{AnyElement, IntoElement, ParentElement, Styled, div, rgb};
use vespertine_richtext::text_root;
use vespertine_test_host::{HostOptions, TestHost};

/// A host rendering `build()` inside a document root on the window background #181818 with the
/// light text color #eeeeee.
pub fn host(width: f64, height: f64, build: impl Fn() -> AnyElement + 'static) -> TestHost {
    TestHost::with_element(HostOptions::with_size(width, height), move |_, _| {
        text_root(
            div()
                .size_full()
                .bg(rgb(0x181818))
                .text_color(rgb(0xeeeeee))
                .child(build()),
        )
        .into_any_element()
    })
    .expect("test host")
}

/// A shared log written by element callbacks.
pub type Log<T> = Rc<RefCell<Vec<T>>>;

/// A fresh empty log.
pub fn log<T>() -> Log<T> {
    Rc::default()
}

/// The text service of the host's window.
pub fn service(host: &mut TestHost) -> vespertine_richtext::TextService {
    let window = host.window();
    host.app()
        .update_window(window, |_, window, cx| {
            vespertine_richtext::TextService::of(window, cx)
        })
        .expect("window")
}

/// Presses at `from`, drags to `to` and releases with a frame painted between each step, and
/// returns the selected text.
pub fn drag(host: &mut TestHost, from: (f64, f64), to: (f64, f64)) -> Option<String> {
    host.mouse_move(from.0, from.1, None, "").unwrap();
    host.paint_frame();
    host.mouse_down(from.0, from.1, 0, "").unwrap();
    host.paint_frame();
    host.mouse_move(to.0, to.1, Some(0), "").unwrap();
    host.paint_frame();
    host.mouse_up(to.0, to.1, 0, "").unwrap();
    host.paint_frame();
    service(host).selected_text()
}

/// The search washes painted in the last frame as `(run text, rects)`.
pub fn washes(host: &mut TestHost) -> Vec<(String, Vec<[f32; 4]>)> {
    service(host)
        .painted_highlights()
        .into_iter()
        .map(|wash| (wash.text, wash.rects))
        .collect()
}
