//! Shared surface plumbing for host elements (ELM-007 R8, R9).
//!
//! * [`BoundsTracker`] / [`track_bounds`]: record the last painted border box of an element
//!   without influencing layout, so automation and bounds queries work like for ordinary hosts.
//! * [`HostEvent`] and [`wire_standard_events`]: the native payloads of the standard host events
//!   (click, enter, leave, file drop) and the handlers that deliver them from any stateful
//!   interactive element, including leaf elements such as images.

use std::{cell::Cell, collections::BTreeSet, path::PathBuf, rc::Rc};

use crate::{
    AnyElement, App, Bounds, Element, ExternalPaths, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, Modifiers, MouseButton, Pixels, Point, SharedString,
    StatefulInteractiveElement, Window,
};

/// Event name of the primary-button click.
pub const EVENT_CLICK: &str = "click";
/// Event name of the pointer entering the host.
pub const EVENT_ENTER: &str = "enter";
/// Event name of the pointer leaving the host.
pub const EVENT_LEAVE: &str = "leave";
/// Event name of an external file drop onto the host.
pub const EVENT_FILE_DROP: &str = "fileDrop";

/// The standard events every surface can deliver, in declaration order.
pub const STANDARD_EVENTS: [&str; 4] = [EVENT_CLICK, EVENT_ENTER, EVENT_LEAVE, EVENT_FILE_DROP];

/// A click on the primary button, delivered on mouse-up.
#[derive(Clone, Debug, PartialEq)]
pub struct HostClickEvent {
    /// Window-local pointer position in logical pixels.
    pub position: Point<Pixels>,
    /// Mouse button; always 0 (left) for the standard click.
    pub button: u8,
    /// Click count of the release.
    pub click_count: usize,
    /// Modifier keys held at the release.
    pub modifiers: Modifiers,
}

/// A pointer entering or leaving the host.
#[derive(Clone, Debug, PartialEq)]
pub struct HostHoverEvent {
    /// Window-local pointer position in logical pixels.
    pub position: Point<Pixels>,
    /// Modifier keys held when the edge was observed.
    pub modifiers: Modifiers,
}

/// A drop of external files onto the host.
#[derive(Clone, Debug, PartialEq)]
pub struct HostFileDropEvent {
    /// Absolute filesystem paths, all valid Unicode and never empty.
    pub paths: Vec<String>,
    /// Window-local drop position in logical pixels.
    pub position: Point<Pixels>,
}

/// An event produced by a host adapter.
#[derive(Clone, Debug, PartialEq)]
pub enum HostEvent {
    /// See [`HostClickEvent`]; event name [`EVENT_CLICK`].
    Click(HostClickEvent),
    /// See [`HostHoverEvent`]; event name [`EVENT_ENTER`].
    Enter(HostHoverEvent),
    /// See [`HostHoverEvent`]; event name [`EVENT_LEAVE`].
    Leave(HostHoverEvent),
    /// See [`HostFileDropEvent`]; event name [`EVENT_FILE_DROP`].
    FileDrop(HostFileDropEvent),
    /// An adapter-specific event with a JSON payload.
    Custom {
        /// The declared event name.
        name: SharedString,
        /// The event payload.
        payload: serde_json::Value,
    },
}

impl HostEvent {
    /// The event name used for listener registration and filtering.
    pub fn name(&self) -> &str {
        match self {
            HostEvent::Click(_) => EVENT_CLICK,
            HostEvent::Enter(_) => EVENT_ENTER,
            HostEvent::Leave(_) => EVENT_LEAVE,
            HostEvent::FileDrop(_) => EVENT_FILE_DROP,
            HostEvent::Custom { name, .. } => name,
        }
    }
}

/// The callback adapters use to emit events to the host's listeners.
pub type EmitEvent = Rc<dyn Fn(HostEvent, &mut Window, &mut App)>;

/// The set of event names a host both subscribed to and the adapter declares.
pub type EventSet = BTreeSet<SharedString>;

/// Shared record of the last painted border box of an element.
///
/// Clone the tracker and pass one clone to [`track_bounds`]; read it from anywhere.
#[derive(Clone, Debug, Default)]
pub struct BoundsTracker(Rc<Cell<Option<Bounds<Pixels>>>>);

impl BoundsTracker {
    /// A new tracker that has not recorded any bounds.
    pub fn new() -> Self {
        Self::default()
    }

    /// The border box recorded by the most recent paint, if the element has painted.
    pub fn get(&self) -> Option<Bounds<Pixels>> {
        self.0.get()
    }

    /// Forgets the recorded bounds (the element is no longer painted).
    pub fn clear(&self) {
        self.0.set(None);
    }

    fn set(&self, bounds: Bounds<Pixels>) {
        self.0.set(Some(bounds));
    }
}

/// Wraps `child` so that its final border box is recorded into `tracker` every time it paints.
///
/// The wrapper adds no layout node: it adopts the child's layout, so it works for in-flow,
/// absolute and deferred children alike and never changes positioning.
pub fn track_bounds(tracker: &BoundsTracker, child: impl IntoElement) -> TrackBounds {
    TrackBounds {
        tracker: tracker.clone(),
        child: child.into_any_element(),
    }
}

/// See [`track_bounds`].
pub struct TrackBounds {
    tracker: BoundsTracker,
    child: AnyElement,
}

impl Element for TrackBounds {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<crate::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.tracker.set(bounds);
        self.child.paint(window, cx);
    }
}

impl IntoElement for TrackBounds {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// Installs a handler for every standard event in `events` on `element` (R9).
///
/// * click fires on the primary button's release with the pointer position, button 0, the click
///   count and modifiers; the accessible click action of the element emits the same event with
///   click count 1 at the center of the last tracked bounds;
/// * enter and leave share one hover listener and each edge is delivered only if subscribed;
/// * file drop delivers absolute paths and the drop position, and is *not* emitted when the drop
///   holds no paths or any path is not valid Unicode.
///
/// Events not in `events` get no handler. `tracker` supplies the position of accessible clicks.
pub fn wire_standard_events<E: StatefulInteractiveElement>(
    mut element: E,
    events: &EventSet,
    emit: &EmitEvent,
    tracker: &BoundsTracker,
) -> E {
    if events.contains(EVENT_CLICK) {
        let emit_up = emit.clone();
        element = element.on_mouse_up(MouseButton::Left, move |event, window, cx| {
            emit_up(
                HostEvent::Click(HostClickEvent {
                    position: event.position,
                    button: 0,
                    click_count: event.click_count,
                    modifiers: event.modifiers,
                }),
                window,
                cx,
            );
        });
        let emit_a11y = emit.clone();
        let tracker = tracker.clone();
        element = element.on_a11y_action(accesskit::Action::Click, move |_, window, cx| {
            let position = tracker
                .get()
                .map(|bounds| bounds.center())
                .unwrap_or_else(|| window.mouse_position());
            emit_a11y(
                HostEvent::Click(HostClickEvent {
                    position,
                    button: 0,
                    click_count: 1,
                    modifiers: Modifiers::default(),
                }),
                window,
                cx,
            );
        });
    }

    let on_enter = events.contains(EVENT_ENTER);
    let on_leave = events.contains(EVENT_LEAVE);
    if on_enter || on_leave {
        let emit = emit.clone();
        element = element.on_hover(move |hovered, window, cx| {
            if *hovered && !on_enter || !*hovered && !on_leave {
                return;
            }
            let event = HostHoverEvent {
                position: window.mouse_position(),
                modifiers: window.modifiers(),
            };
            emit(
                if *hovered {
                    HostEvent::Enter(event)
                } else {
                    HostEvent::Leave(event)
                },
                window,
                cx,
            );
        });
    }

    if events.contains(EVENT_FILE_DROP) {
        let emit = emit.clone();
        element = element.on_drop::<ExternalPaths>(move |dropped, window, cx| {
            let Some(paths) = utf8_paths(dropped.paths()) else {
                return;
            };
            emit(
                HostEvent::FileDrop(HostFileDropEvent {
                    paths,
                    position: window.mouse_position(),
                }),
                window,
                cx,
            );
        });
    }
    element
}

/// Converts dropped paths to strings; `None` when empty or any path is not valid Unicode (a
/// lossy conversion would invent a path that does not exist).
fn utf8_paths(paths: &[PathBuf]) -> Option<Vec<String>> {
    if paths.is_empty() {
        return None;
    }
    paths
        .iter()
        .map(|path| path.to_str().map(str::to_owned))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_paths_rejects_empty_and_non_unicode() {
        assert_eq!(utf8_paths(&[]), None);
        assert_eq!(
            utf8_paths(&[PathBuf::from("/a/b"), PathBuf::from("/c")]),
            Some(vec!["/a/b".to_owned(), "/c".to_owned()])
        );
        #[cfg(unix)]
        {
            use std::{ffi::OsString, os::unix::ffi::OsStringExt};
            let bad = PathBuf::from(OsString::from_vec(vec![b'/', 0xff, 0xfe]));
            assert_eq!(utf8_paths(&[PathBuf::from("/ok"), bad]), None);
        }
    }
}
