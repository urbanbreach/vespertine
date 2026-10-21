//! Window geometry restore and mode preservation (PLT-008 R3, R4).
//!
//! Applications persist `(mode, ordinary bounds, display uuid)` for a window with
//! [`SavedWindowGeometry::capture`] and map it back onto the displays that exist at the next
//! launch with [`restore_window_geometry`], a pure function over [`DisplayWorkArea`] values so
//! it can be tested without a window system.
//!
//! [`WindowBoundsTracker`] is the other half of the contract: platform backends feed it every
//! bounds observation together with the current [`WindowMode`], and it keeps the ordinary
//! (windowed) bounds unchanged while the window is maximized or fullscreen.
//!
//! On Wayland, clients cannot position windows. A restored [`WindowBounds`] still carries an
//! origin, but the compositor chooses the placement; only the size, the maximized or
//! fullscreen state, and the [`RestoredWindowGeometry::display_id`] output hint for fullscreen
//! take effect.

use crate::{
    App, Bounds, DEFAULT_WINDOW_SIZE, DisplayId, Pixels, PlatformDisplay, Point, Size,
    WindowBounds, point, px, size,
};
use uuid::Uuid;

/// Whether a window is in its ordinary state, maximized, or fullscreen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WindowMode {
    /// The window has its ordinary, user-chosen geometry.
    #[default]
    Windowed,
    /// The window fills the work area of its display.
    Maximized,
    /// The window covers its whole display.
    Fullscreen,
}

impl WindowMode {
    /// The mode that a [`WindowBounds`] variant describes.
    pub fn of(window_bounds: &WindowBounds) -> Self {
        match window_bounds {
            WindowBounds::Windowed(_) => Self::Windowed,
            WindowBounds::Maximized(_) => Self::Maximized,
            WindowBounds::Fullscreen(_) => Self::Fullscreen,
        }
    }

    /// Wraps `bounds` in the [`WindowBounds`] variant for this mode. For maximized and
    /// fullscreen, `bounds` are the ordinary bounds to return to.
    pub fn with_bounds(self, bounds: Bounds<Pixels>) -> WindowBounds {
        match self {
            Self::Windowed => WindowBounds::Windowed(bounds),
            Self::Maximized => WindowBounds::Maximized(bounds),
            Self::Fullscreen => WindowBounds::Fullscreen(bounds),
        }
    }
}

/// A display as [`restore_window_geometry`] sees it: the stable identity, the full bounds and
/// the work area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplayWorkArea {
    /// The session-scoped display id.
    pub id: DisplayId,
    /// The persistent identifier, when the platform can provide one. On Wayland it is absent
    /// when the compositor reports no output name.
    pub uuid: Option<Uuid>,
    /// The full display bounds in logical pixels.
    pub bounds: Bounds<Pixels>,
    /// The part of the display not covered by panels or docks. Equal to `bounds` where the
    /// platform exposes no work area (Wayland).
    pub visible_bounds: Bounds<Pixels>,
}

impl DisplayWorkArea {
    /// Describes a platform display.
    pub fn of(display: &dyn PlatformDisplay) -> Self {
        Self {
            id: display.id(),
            uuid: display.uuid().ok(),
            bounds: display.bounds(),
            visible_bounds: display.visible_bounds(),
        }
    }

    /// Describes every connected display.
    pub fn all(cx: &App) -> Vec<Self> {
        cx.displays()
            .iter()
            .map(|display| Self::of(display.as_ref()))
            .collect()
    }
}

/// The geometry an application persists for a window: the mode, the ordinary bounds, and the
/// display the window was on.
///
/// When `display_uuid` is set, `bounds.origin` is relative to the origin of that display, so
/// that a display which moved in the virtual desktop keeps the same relative placement.
/// Without a uuid the origin is in global logical coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SavedWindowGeometry {
    /// Whether to restore the window as windowed, maximized or fullscreen.
    pub mode: WindowMode,
    /// The ordinary bounds: the ones to return to after leaving maximized or fullscreen.
    pub bounds: Bounds<Pixels>,
    /// The persistent identifier of the display the window was on.
    pub display_uuid: Option<Uuid>,
}

impl SavedWindowGeometry {
    /// Captures a window's [`WindowBounds`] (as reported by `Window::window_bounds`) for
    /// persisting. With a `display` that has a uuid the origin is stored relative to the
    /// display; otherwise it stays in global coordinates.
    pub fn capture(window_bounds: WindowBounds, display: Option<&dyn PlatformDisplay>) -> Self {
        Self::capture_in(window_bounds, display.map(DisplayWorkArea::of).as_ref())
    }

    /// [`Self::capture`] for a display that is already described as a [`DisplayWorkArea`].
    pub fn capture_in(window_bounds: WindowBounds, display: Option<&DisplayWorkArea>) -> Self {
        let mode = WindowMode::of(&window_bounds);
        let mut bounds = window_bounds.get_bounds();
        let display_uuid = display.and_then(|display| {
            let uuid = display.uuid?;
            bounds.origin = bounds.origin - display.bounds.origin;
            Some(uuid)
        });
        Self {
            mode,
            bounds,
            display_uuid,
        }
    }
}

/// The outcome of [`restore_window_geometry`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RestoredWindowGeometry {
    /// The display to open the window on (`WindowOptions::display_id`). `None` only when no
    /// display exists.
    pub display_id: Option<DisplayId>,
    /// The mode and ordinary bounds to open the window with (`WindowOptions::window_bounds`).
    pub window_bounds: WindowBounds,
}

fn intersection_area(a: &Bounds<Pixels>, b: &Bounds<Pixels>) -> f32 {
    if !a.intersects(b) {
        return 0.0;
    }
    let overlap = a.intersect(b).size;
    f32::from(overlap.width) * f32::from(overlap.height)
}

/// Centers [`DEFAULT_WINDOW_SIZE`], clipped to the display bounds, on `display`.
pub fn default_window_bounds(display: &DisplayWorkArea) -> Bounds<Pixels> {
    let clipped = DEFAULT_WINDOW_SIZE.min(&display.bounds.size);
    Bounds::centered_at(display.bounds.center(), clipped)
}

/// Maps saved window geometry onto the connected displays (PLT-008 R3).
///
/// 1. The display is the one whose uuid equals the saved uuid; otherwise the one whose bounds
///    intersect the saved bounds in the largest area (the first listed wins a tie); otherwise
///    `primary`; otherwise the first listed display.
/// 2. When the display matched by uuid, the saved origin is relative to the display's origin
///    and is re-applied to the display's current origin. Otherwise the saved origin is global.
/// 3. Width and height are limited to the display's work area, then raised to `min_size` where
///    set. The minimum size takes precedence over the work area when they conflict.
/// 4. A rectangle that does not intersect the work area is centered on it. Otherwise it is
///    translated by the minimum amount that places it inside the work area on each axis. An
///    axis longer than the work area starts at the work area's start.
///
/// Without saved geometry, [`default_window_bounds`] on the primary display is returned.
/// With no displays at all, the saved bounds (or the default size at the origin) are returned
/// unchanged and `display_id` is `None`.
pub fn restore_window_geometry(
    saved: Option<&SavedWindowGeometry>,
    displays: &[DisplayWorkArea],
    primary: Option<DisplayId>,
    min_size: Option<Size<Pixels>>,
) -> RestoredWindowGeometry {
    let primary_display = || {
        primary
            .and_then(|id| displays.iter().find(|display| display.id == id))
            .or_else(|| displays.first())
    };

    let Some(saved) = saved else {
        return match primary_display() {
            Some(display) => RestoredWindowGeometry {
                display_id: Some(display.id),
                window_bounds: WindowBounds::Windowed(default_window_bounds(display)),
            },
            None => RestoredWindowGeometry {
                display_id: None,
                window_bounds: WindowBounds::Windowed(Bounds::new(
                    Point::default(),
                    DEFAULT_WINDOW_SIZE,
                )),
            },
        };
    };

    let uuid_match = saved
        .display_uuid
        .and_then(|uuid| displays.iter().find(|display| display.uuid == Some(uuid)));
    let (display, by_uuid) = match uuid_match {
        Some(display) => (Some(display), true),
        None => {
            let mut best: Option<(&DisplayWorkArea, f32)> = None;
            for display in displays {
                let overlap = intersection_area(&display.bounds, &saved.bounds);
                if overlap > 0.0 && best.is_none_or(|(_, best_overlap)| overlap > best_overlap) {
                    best = Some((display, overlap));
                }
            }
            (
                best.map(|(display, _)| display).or_else(primary_display),
                false,
            )
        }
    };

    let Some(display) = display else {
        return RestoredWindowGeometry {
            display_id: None,
            window_bounds: saved.mode.with_bounds(saved.bounds),
        };
    };

    let work_area = display.visible_bounds;
    let mut width = f32::from(saved.bounds.size.width).min(f32::from(work_area.size.width));
    let mut height = f32::from(saved.bounds.size.height).min(f32::from(work_area.size.height));
    if let Some(min_size) = min_size {
        width = width.max(f32::from(min_size.width));
        height = height.max(f32::from(min_size.height));
    }
    let clamped = size(px(width), px(height));

    let origin = if by_uuid {
        display.bounds.origin + saved.bounds.origin
    } else {
        saved.bounds.origin
    };
    let candidate = Bounds::new(origin, clamped);
    let origin = if candidate.intersects(&work_area) {
        candidate.origin
    } else {
        Bounds::centered_at(work_area.center(), clamped).origin
    };

    // Translates by the minimum amount that keeps the window inside the work area. An axis
    // longer than the work area (a minimum size larger than it) starts at the work area's start,
    // so the titlebar stays reachable.
    let shift = |start: f32, length: f32, low: f32, high: f32| {
        if start + length > high {
            (high - length).max(low)
        } else {
            start.max(low)
        }
    };
    let x = shift(
        f32::from(origin.x),
        width,
        f32::from(work_area.origin.x),
        f32::from(work_area.origin.x) + f32::from(work_area.size.width),
    );
    let y = shift(
        f32::from(origin.y),
        height,
        f32::from(work_area.origin.y),
        f32::from(work_area.origin.y) + f32::from(work_area.size.height),
    );
    let final_bounds = Bounds::new(point(px(x), px(y)), clamped);

    RestoredWindowGeometry {
        display_id: Some(display.id),
        window_bounds: saved.mode.with_bounds(final_bounds),
    }
}

/// Keeps a window's ordinary bounds while it is maximized or fullscreen (PLT-008 R4).
///
/// Backends call [`WindowBoundsTracker::observe`] with every bounds observation (resize or
/// move) together with the current mode. Only observations made in
/// [`WindowMode::Windowed`] replace the stored ordinary bounds; in the other modes the
/// resulting [`WindowBounds`] carries the ordinary bounds unchanged, so leaving maximized or
/// fullscreen returns to them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowBoundsTracker {
    ordinary: Bounds<Pixels>,
}

impl WindowBoundsTracker {
    /// Starts with `ordinary` as the bounds to return to.
    pub fn new(ordinary: Bounds<Pixels>) -> Self {
        Self { ordinary }
    }

    /// The stored ordinary bounds.
    pub fn ordinary_bounds(&self) -> Bounds<Pixels> {
        self.ordinary
    }

    /// Records a bounds observation made in `mode` and returns the resulting window bounds.
    pub fn observe(&mut self, mode: WindowMode, current: Bounds<Pixels>) -> WindowBounds {
        if mode == WindowMode::Windowed {
            self.ordinary = current;
        }
        mode.with_bounds(self.ordinary)
    }

    /// The window bounds for `mode` without recording an observation.
    pub fn window_bounds(&self, mode: WindowMode) -> WindowBounds {
        mode.with_bounds(self.ordinary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
    }

    fn display(id: u64, uuid: Option<u128>, area: Bounds<Pixels>) -> DisplayWorkArea {
        DisplayWorkArea {
            id: DisplayId::new(id),
            uuid: uuid.map(Uuid::from_u128),
            bounds: area,
            visible_bounds: area,
        }
    }

    fn windowed(bounds: Bounds<Pixels>, uuid: Option<u128>) -> SavedWindowGeometry {
        SavedWindowGeometry {
            mode: WindowMode::Windowed,
            bounds,
            display_uuid: uuid.map(Uuid::from_u128),
        }
    }

    fn two_displays() -> [DisplayWorkArea; 2] {
        [
            display(1, Some(1), bounds(0.0, 0.0, 1920.0, 1080.0)),
            display(2, Some(2), bounds(1920.0, 0.0, 1280.0, 1024.0)),
        ]
    }

    #[test]
    fn uuid_match_keeps_relative_placement_after_display_moved() {
        // Saved on display 2 at relative origin (100, 50); the display now sits at (3000, 200).
        let saved = windowed(bounds(100.0, 50.0, 800.0, 600.0), Some(2));
        let displays = [
            display(1, Some(1), bounds(0.0, 0.0, 1920.0, 1080.0)),
            display(7, Some(2), bounds(3000.0, 200.0, 1280.0, 1024.0)),
        ];
        let restored =
            restore_window_geometry(Some(&saved), &displays, Some(DisplayId::new(1)), None);
        assert_eq!(restored.display_id, Some(DisplayId::new(7)));
        assert_eq!(
            restored.window_bounds,
            WindowBounds::Windowed(bounds(3100.0, 250.0, 800.0, 600.0))
        );
    }

    #[test]
    fn capture_stores_origin_relative_to_the_display() {
        #[derive(Debug)]
        struct Fake(Bounds<Pixels>);
        impl PlatformDisplay for Fake {
            fn id(&self) -> DisplayId {
                DisplayId::new(3)
            }
            fn uuid(&self) -> anyhow::Result<Uuid> {
                Ok(Uuid::from_u128(9))
            }
            fn bounds(&self) -> Bounds<Pixels> {
                self.0
            }
            fn refresh_interval(&self) -> Option<std::time::Duration> {
                None
            }
        }
        let display = Fake(bounds(1920.0, 100.0, 1280.0, 1024.0));
        let saved = SavedWindowGeometry::capture(
            WindowBounds::Maximized(bounds(2020.0, 150.0, 800.0, 600.0)),
            Some(&display),
        );
        assert_eq!(saved.mode, WindowMode::Maximized);
        assert_eq!(saved.display_uuid, Some(Uuid::from_u128(9)));
        assert_eq!(saved.bounds, bounds(100.0, 50.0, 800.0, 600.0));
    }

    #[test]
    fn no_uuid_match_picks_largest_intersection_display() {
        // The window spans 320 px of display 1 and 480 px of display 2.
        let saved = windowed(bounds(1600.0, 100.0, 800.0, 600.0), Some(99));
        let restored =
            restore_window_geometry(Some(&saved), &two_displays(), Some(DisplayId::new(1)), None);
        assert_eq!(restored.display_id, Some(DisplayId::new(2)));
        // Translated right to lie inside display 2, which starts at x = 1920.
        assert_eq!(
            restored.window_bounds,
            WindowBounds::Windowed(bounds(1920.0, 100.0, 800.0, 600.0))
        );
    }

    #[test]
    fn no_overlap_falls_back_to_primary_display() {
        let saved = windowed(bounds(9000.0, 9000.0, 800.0, 600.0), None);
        let restored =
            restore_window_geometry(Some(&saved), &two_displays(), Some(DisplayId::new(2)), None);
        assert_eq!(restored.display_id, Some(DisplayId::new(2)));
        // Completely outside: centered on the primary's work area.
        assert_eq!(
            restored.window_bounds,
            WindowBounds::Windowed(bounds(2160.0, 212.0, 800.0, 600.0))
        );
    }

    #[test]
    fn oversize_is_clamped_to_the_work_area() {
        let mut displays = two_displays();
        displays[0].visible_bounds = bounds(0.0, 40.0, 1920.0, 1040.0);
        let saved = windowed(bounds(0.0, 0.0, 5000.0, 4000.0), None);
        let restored =
            restore_window_geometry(Some(&saved), &displays, Some(DisplayId::new(1)), None);
        assert_eq!(
            restored.window_bounds,
            WindowBounds::Windowed(bounds(0.0, 40.0, 1920.0, 1040.0))
        );
    }

    #[test]
    fn partially_outside_is_translated_inside() {
        let saved = windowed(bounds(1500.0, 800.0, 800.0, 600.0), None);
        let restored =
            restore_window_geometry(Some(&saved), &two_displays(), Some(DisplayId::new(1)), None);
        // Overlaps display 1 (420 x 280) and display 2 (380 x 224): display 1 wins and the
        // window moves left and up by the minimum amount.
        assert_eq!(restored.display_id, Some(DisplayId::new(1)));
        assert_eq!(
            restored.window_bounds,
            WindowBounds::Windowed(bounds(1120.0, 480.0, 800.0, 600.0))
        );
        let negative = windowed(bounds(-300.0, -100.0, 800.0, 600.0), None);
        let restored = restore_window_geometry(
            Some(&negative),
            &two_displays(),
            Some(DisplayId::new(1)),
            None,
        );
        assert_eq!(
            restored.window_bounds,
            WindowBounds::Windowed(bounds(0.0, 0.0, 800.0, 600.0))
        );
    }

    #[test]
    fn rectangle_outside_every_work_area_is_centered() {
        // On the display by uuid, but the relative origin lies far outside the work area.
        let saved = windowed(bounds(5000.0, 5000.0, 400.0, 300.0), Some(1));
        let restored =
            restore_window_geometry(Some(&saved), &two_displays(), Some(DisplayId::new(2)), None);
        assert_eq!(restored.display_id, Some(DisplayId::new(1)));
        assert_eq!(
            restored.window_bounds,
            WindowBounds::Windowed(bounds(760.0, 390.0, 400.0, 300.0))
        );
    }

    #[test]
    fn minimum_size_is_respected() {
        let saved = windowed(bounds(100.0, 100.0, 120.0, 90.0), None);
        let restored = restore_window_geometry(
            Some(&saved),
            &two_displays(),
            Some(DisplayId::new(1)),
            Some(size(px(400.0), px(300.0))),
        );
        assert_eq!(
            restored.window_bounds,
            WindowBounds::Windowed(bounds(100.0, 100.0, 400.0, 300.0))
        );
    }

    #[test]
    fn minimum_size_larger_than_the_work_area_keeps_the_top_left_reachable() {
        let work_area = bounds(0.0, 40.0, 1920.0, 1040.0);
        let displays = [display(1, Some(1), work_area)];
        let min_size = Some(size(px(400.0), px(1200.0)));
        // Inside the work area, and entirely outside it (where the rectangle is centered).
        for (origin, x) in [((100.0, 100.0), 100.0), ((5000.0, 5000.0), 760.0)] {
            let saved = windowed(bounds(origin.0, origin.1, 400.0, 300.0), None);
            let restored = restore_window_geometry(Some(&saved), &displays, None, min_size);
            assert_eq!(
                restored.window_bounds,
                WindowBounds::Windowed(bounds(x, 40.0, 400.0, 1200.0)),
                "saved origin {origin:?}"
            );
        }
    }

    #[test]
    fn mode_is_kept_and_default_is_centered_and_clipped() {
        let saved = SavedWindowGeometry {
            mode: WindowMode::Fullscreen,
            ..windowed(bounds(10.0, 10.0, 500.0, 400.0), Some(2))
        };
        let restored =
            restore_window_geometry(Some(&saved), &two_displays(), Some(DisplayId::new(1)), None);
        assert_eq!(
            restored.window_bounds,
            WindowBounds::Fullscreen(bounds(1930.0, 10.0, 500.0, 400.0))
        );

        // 1536 x 1095 clipped to the 1920 x 1080 display, centered on it.
        let restored =
            restore_window_geometry(None, &two_displays(), Some(DisplayId::new(1)), None);
        assert_eq!(restored.display_id, Some(DisplayId::new(1)));
        assert_eq!(
            restored.window_bounds,
            WindowBounds::Windowed(bounds(192.0, 0.0, 1536.0, 1080.0))
        );
    }

    #[test]
    fn tracker_keeps_ordinary_bounds_through_maximize_and_fullscreen() {
        let ordinary = bounds(100.0, 100.0, 800.0, 600.0);
        let mut tracker = WindowBoundsTracker::new(ordinary);
        let screen = bounds(0.0, 0.0, 1920.0, 1080.0);
        assert_eq!(
            tracker.observe(WindowMode::Maximized, screen),
            WindowBounds::Maximized(ordinary)
        );
        assert_eq!(
            tracker.observe(WindowMode::Fullscreen, screen),
            WindowBounds::Fullscreen(ordinary)
        );
        assert_eq!(tracker.ordinary_bounds(), ordinary);
        let resized = bounds(100.0, 100.0, 900.0, 650.0);
        assert_eq!(
            tracker.observe(WindowMode::Windowed, resized),
            WindowBounds::Windowed(resized)
        );
        assert_eq!(tracker.ordinary_bounds(), resized);
    }
}
