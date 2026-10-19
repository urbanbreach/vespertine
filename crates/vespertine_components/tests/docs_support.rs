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

/// The directory of the fidelity reference images.
pub fn reference_dir() -> std::path::PathBuf {
    std::env::var_os("VESPERTINE_FID_REF")
        .map(Into::into)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
                .join("Projects/vespertine-cleanroom/reports/fidelity/ref")
        })
}

/// The fraction of pixels of `image` that differ from the reference `scene` by more than 8/255
/// in any channel; `None` when the reference image is not available on this machine.
pub fn fidelity(image: &image::RgbaImage, scene: &str) -> Option<f64> {
    let path = reference_dir().join(format!("{scene}.png"));
    let reference = image::open(&path).ok()?.to_rgba8();
    assert_eq!(
        (image.width(), image.height()),
        (reference.width(), reference.height()),
        "{scene}: image size"
    );
    let differing = image
        .pixels()
        .zip(reference.pixels())
        .filter(|(a, b)| {
            a.0.iter()
                .zip(b.0.iter())
                .take(3)
                .any(|(x, y)| x.abs_diff(*y) > 8)
        })
        .count();
    Some(differing as f64 / f64::from(image.width() * image.height()))
}

/// Saves `image` next to the target directory for inspection.
pub fn save_capture(image: &image::RgbaImage, name: &str) {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    let _ = image.save(dir.join(format!("{name}.png")));
}
