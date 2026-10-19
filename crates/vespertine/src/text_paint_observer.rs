//! Test-support observation of what a frame actually paints.
//!
//! Test hosts (DEV-006) need the strings painted by the last frame in paint order, including
//! text that is painted by custom elements and therefore exists in no element tree, and they
//! need to know exactly when a frame starts so that per-frame logs are cleared once per frame.
//! Every [`ShapedLine`](crate::ShapedLine) and [`WrappedLine`](crate::WrappedLine) paint reports
//! its source text, and every [`Window::draw`] reports its start, when an observer is installed.
//! The hooks are compiled only with the `test-support` feature (or for this crate's own tests)
//! and cost one global lookup per painted line or drawn frame when no observer is installed.

use crate::{App, Global, Pixels, Point, SharedString, Window};
use std::rc::Rc;

/// One line of text handed to the painter.
#[derive(Clone, Debug, PartialEq)]
pub struct PaintedText {
    /// The source text of the painted line (for a wrapped line, the whole unwrapped line).
    pub text: SharedString,
    /// The origin the line was painted at, in window coordinates.
    pub origin: Point<Pixels>,
    /// The line height the line was painted with.
    pub line_height: Pixels,
}

/// An application global that observes painting. Install it with [`App::set_global`]; remove
/// it by replacing it with another observer.
#[derive(Clone)]
pub struct PaintObserver {
    /// Called with every painted text line.
    pub on_text: Rc<dyn Fn(&PaintedText)>,
    /// Called at the very start of every frame the window draws, before layout and paint. The
    /// callback may call [`Window::refresh`] to make the frame bypass retained paint state.
    pub on_frame_start: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
}

impl Global for PaintObserver {}

pub(crate) fn notify_frame_started(window: &mut Window, cx: &mut App) {
    let callback = cx
        .try_global::<PaintObserver>()
        .and_then(|observer| observer.on_frame_start.clone());
    if let Some(callback) = callback {
        callback(window, cx);
    }
}

pub(crate) fn notify_text_painted(
    cx: &App,
    text: &SharedString,
    origin: Point<Pixels>,
    line_height: Pixels,
) {
    if let Some(observer) = cx.try_global::<PaintObserver>() {
        let observer = observer.on_text.clone();
        observer(&PaintedText {
            text: text.clone(),
            origin,
            line_height,
        });
    }
}
