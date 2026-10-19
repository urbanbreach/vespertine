//! Saving and restoring window geometry across launches (PLT-008).
//!
//! The example first runs a simulation with fake displays, without a window system: a window
//! on a second display is saved, the display disappears, and the relaunched window opens
//! reachable on the remaining display; a rectangle that no longer intersects any work area
//! is centered. It panics if either result is wrong.
//!
//! It then opens a real window whose geometry comes from the file `window_geometry.txt` in
//! the temporary directory, restored against the connected displays, and saves the
//! window's geometry back when the window closes. Move or maximize the window, close it, and
//! run the example again.
//!
//! * `--hide-saved-display` drops the display the geometry was saved on from the displays
//!   used for restoring, as if it had been disconnected (a fake removal).
//! * `--once` saves immediately after opening and quits, for non-interactive runs.
//! * `--simulate-only` runs just the simulation.

use std::{path::PathBuf, str::FromStr as _};

use uuid::Uuid;
use vespertine::{
    App, Bounds, Context, DisplayId, DisplayWorkArea, IntoElement, Pixels, Render,
    SavedWindowGeometry, Window, WindowBounds, WindowMode, WindowOptions, div, point, prelude::*,
    px, restore_window_geometry, rgb, size,
};
use vespertine_platform::application;

struct Content;

impl Render for Content {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let bounds = window.window_bounds();
        div()
            .size_full()
            .flex()
            .flex_col()
            .justify_center()
            .items_center()
            .bg(rgb(0x1f2933))
            .text_color(rgb(0xffffff))
            .child(format!("{bounds:?}"))
    }
}

fn geometry_path() -> PathBuf {
    std::env::temp_dir().join("window_geometry.txt")
}

fn format_geometry(saved: &SavedWindowGeometry) -> String {
    let mode = match saved.mode {
        WindowMode::Windowed => "windowed",
        WindowMode::Maximized => "maximized",
        WindowMode::Fullscreen => "fullscreen",
    };
    let bounds = saved.bounds;
    format!(
        "{mode} {} {} {} {} {}",
        f32::from(bounds.origin.x),
        f32::from(bounds.origin.y),
        f32::from(bounds.size.width),
        f32::from(bounds.size.height),
        saved
            .display_uuid
            .map(|uuid| uuid.to_string())
            .unwrap_or_else(|| "-".into()),
    )
}

fn parse_geometry(text: &str) -> Option<SavedWindowGeometry> {
    let mut parts = text.split_whitespace();
    let mode = match parts.next()? {
        "windowed" => WindowMode::Windowed,
        "maximized" => WindowMode::Maximized,
        "fullscreen" => WindowMode::Fullscreen,
        _ => return None,
    };
    let mut number = || f32::from_str(parts.next()?).ok();
    let (x, y, width, height) = (number()?, number()?, number()?, number()?);
    let display_uuid = match parts.next()? {
        "-" => None,
        uuid => Some(Uuid::parse_str(uuid).ok()?),
    };
    Some(SavedWindowGeometry {
        mode,
        bounds: Bounds::new(point(px(x), px(y)), size(px(width), px(height))),
        display_uuid,
    })
}

fn is_reachable(bounds: Bounds<Pixels>, work_area: Bounds<Pixels>) -> bool {
    work_area.intersect(&bounds) == bounds
}

/// A window on a second display is saved; that display disappears; the relaunch is reachable.
fn simulate() {
    let primary = DisplayWorkArea {
        id: DisplayId::new(1),
        uuid: Some(Uuid::from_u128(1)),
        bounds: Bounds::new(point(px(0.), px(0.)), size(px(1920.), px(1080.))),
        visible_bounds: Bounds::new(point(px(0.), px(36.)), size(px(1920.), px(1044.))),
    };
    let second = DisplayWorkArea {
        id: DisplayId::new(2),
        uuid: Some(Uuid::from_u128(2)),
        bounds: Bounds::new(point(px(1920.), px(0.)), size(px(1280.), px(1024.))),
        visible_bounds: Bounds::new(point(px(1920.), px(0.)), size(px(1280.), px(1024.))),
    };

    // First launch: both displays exist, the window is on the second one.
    let window_bounds = WindowBounds::Windowed(Bounds::new(
        point(px(2400.), px(300.)),
        size(px(700.), px(600.)),
    ));
    let saved = SavedWindowGeometry::capture_in(window_bounds, Some(&second));
    let text = format_geometry(&saved);
    println!("saved on display 2: {text}");

    // Relaunch with both displays: the window returns to the same place.
    let saved = parse_geometry(&text).expect("the saved geometry parses");
    let restored =
        restore_window_geometry(Some(&saved), &[primary, second], Some(primary.id), None);
    assert_eq!(restored.display_id, Some(second.id));
    assert_eq!(restored.window_bounds, window_bounds);

    // Relaunch after the second display was removed.
    let restored = restore_window_geometry(Some(&saved), &[primary], Some(primary.id), None);
    println!("relaunch without display 2: {:?}", restored.window_bounds);
    assert_eq!(restored.display_id, Some(primary.id));
    assert!(is_reachable(
        restored.window_bounds.get_bounds(),
        primary.visible_bounds
    ));

    // A rectangle that no longer intersects any work area is centered.
    let lost = SavedWindowGeometry {
        mode: WindowMode::Windowed,
        bounds: Bounds::new(point(px(9000.), px(9000.)), size(px(700.), px(600.))),
        display_uuid: None,
    };
    let restored = restore_window_geometry(Some(&lost), &[primary], Some(primary.id), None);
    let centered = restored.window_bounds.get_bounds();
    println!("lost rectangle: {centered:?}");
    assert_eq!(centered.center(), primary.visible_bounds.center());
    assert!(is_reachable(centered, primary.visible_bounds));
    println!("simulation passed");
}

fn run_live(hide_saved_display: bool, once: bool) {
    application().run(move |cx: &mut App| {
        // Own the application id on the session bus so that a second launch reaches this
        // process: `busctl --user call dev.vespertine.examples.WindowGeometry
        // /dev/vespertine/examples/WindowGeometry org.freedesktop.Application Activate 'a{sv}' 0`
        cx.set_app_identity("dev.vespertine.examples.WindowGeometry", "Window geometry");
        cx.on_reopen(|cx| {
            println!("reopen requested; {} window(s) open", cx.windows().len());
        });
        cx.on_open_urls(|urls, _| println!("incoming urls: {urls:?}"));
        let saved = std::fs::read_to_string(geometry_path())
            .ok()
            .and_then(|text| parse_geometry(&text));
        let mut displays = DisplayWorkArea::all(cx);
        if hide_saved_display
            && let Some(uuid) = saved.as_ref().and_then(|saved| saved.display_uuid)
        {
            displays.retain(|display| display.uuid != Some(uuid));
            println!("pretending the display {uuid} was removed");
        }
        let primary = cx.primary_display().map(|display| display.id());
        let restored = restore_window_geometry(saved.as_ref(), &displays, primary, None);
        println!(
            "restoring {:?} on {:?}",
            restored.window_bounds, restored.display_id
        );

        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(restored.window_bounds),
                    display_id: restored.display_id,
                    ..Default::default()
                },
                |_, cx| cx.new(|_| Content),
            )
            .expect("the window opens");

        let save = move |cx: &mut App| {
            window
                .update(cx, |_, window, cx| {
                    let display = window.display(cx);
                    let geometry =
                        SavedWindowGeometry::capture(window.window_bounds(), display.as_deref());
                    let text = format_geometry(&geometry);
                    std::fs::write(geometry_path(), &text).ok();
                    println!("saved {text}");
                })
                .ok();
        };

        if once {
            save(cx);
            cx.quit();
            return;
        }
        cx.on_window_closed(move |cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        window
            .update(cx, |_, window, cx| {
                window.on_window_should_close(cx, move |window, cx| {
                    let display = window.display(cx);
                    let geometry =
                        SavedWindowGeometry::capture(window.window_bounds(), display.as_deref());
                    std::fs::write(geometry_path(), format_geometry(&geometry)).ok();
                    true
                });
            })
            .ok();
    });
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|arg| arg == flag);
    simulate();
    if !has("--simulate-only") {
        run_live(has("--hide-saved-display"), has("--once"));
    }
}
