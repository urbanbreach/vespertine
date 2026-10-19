//! Tests of the X11 composition driver: pure decisions, and the X requests against a real
//! X server with an independent connection inspecting what they did (PLT-001 A10).
//!
//! The server tests need a reachable X server (`DISPLAY`, for example the XWayland instance of a
//! Wayland session) and fail loudly without one.

use super::*;
use vespertine::{SurfaceKind, SurfaceTree, point, px, size};
use x11rb::rust_connection::RustConnection;

fn bounds(x: i32, y: i32, width: i32, height: i32) -> Bounds<DevicePixels> {
    Bounds {
        origin: point(DevicePixels(x), DevicePixels(y)),
        size: size(DevicePixels(width), DevicePixels(height)),
    }
}

fn rectangle(x: i16, y: i16, width: u16, height: u16) -> xproto::Rectangle {
    xproto::Rectangle {
        x,
        y,
        width,
        height,
    }
}

#[test]
fn geometry_places_children_relative_to_their_x_parent_and_never_empty() {
    let parent_origin = point(DevicePixels(100), DevicePixels(50));
    assert_eq!(
        Geometry::relative(
            bounds(180, 90, 320, 180),
            point(DevicePixels(0), DevicePixels(0))
        ),
        Geometry {
            x: 180,
            y: 90,
            width: 320,
            height: 180
        }
    );
    assert_eq!(
        Geometry::relative(bounds(180, 90, 320, 180), parent_origin),
        Geometry {
            x: 80,
            y: 40,
            width: 320,
            height: 180
        }
    );
    // A window cannot be empty on X11: it is one pixel and stays unmapped.
    let empty = bounds(10, 10, 0, 40);
    assert!(Geometry::is_degenerate(empty));
    assert_eq!(Geometry::relative(empty, parent_origin).width, 1);
    assert!(!Geometry::is_degenerate(bounds(0, 0, 1, 1)));
    // Coordinates saturate at the protocol's limits.
    let far = Geometry::relative(
        bounds(1_000_000, -1_000_000, 100_000, 10),
        point(DevicePixels(0), DevicePixels(0)),
    );
    assert_eq!((far.x, far.y, far.width), (i16::MAX, i16::MIN, u16::MAX));
}

#[test]
fn cutouts_are_translated_into_the_child_and_clipped_to_it() {
    let child = bounds(100, 50, 200, 100);
    let cutout = [
        bounds(150, 60, 40, 20),
        // Overlaps the child's left and top edge.
        bounds(90, 40, 30, 30),
        // Entirely outside.
        bounds(0, 0, 10, 10),
        // Reaches beyond the right and bottom edge.
        bounds(280, 140, 100, 100),
    ];
    let local = cutout_in_child(&cutout, child)
        .iter()
        .map(|r| (r.x, r.y, r.width, r.height))
        .collect::<Vec<_>>();
    assert_eq!(local, [(50, 10, 40, 20), (0, 0, 20, 20), (180, 90, 20, 10)]);
}

#[test]
fn overlay_input_shape_is_empty_outside_hitboxes_and_in_device_pixels() {
    let empty = OverlayShape::new((800, 600), &[], 2.0, &[]);
    assert!(empty.input.is_empty() && empty.holes.is_empty());

    let hitbox = Bounds {
        origin: vespertine::Point::new(px(10.25), px(5.0)),
        size: vespertine::Size {
            width: px(20.0),
            height: px(10.1),
        },
    };
    let shape = OverlayShape::new((800, 600), &[hitbox], 2.0, &[bounds(700, 500, 400, 400)]);
    // Floor of the origin, ceiling of the far corner, converted at scale 2.
    assert_eq!(shape.input, [rectangle(20, 10, 41, 21)]);
    // Holes are clipped to the window.
    assert_eq!(shape.holes, [rectangle(700, 500, 100, 100)]);
}

fn layout(
    build: impl FnOnce(&mut SurfaceTree) -> Vec<vespertine::SurfaceId>,
) -> (CompositionLayout, Vec<vespertine::SurfaceId>) {
    let mut tree = SurfaceTree::new();
    let ids = build(&mut tree);
    (tree.layout(), ids)
}

#[test]
fn overlay_window_draws_what_is_above_the_topmost_shown_native() {
    // base, native, overlay.
    let (tree, ids) = layout(|tree| vec![tree.create_below_overlay(SurfaceKind::Native)]);
    let native = ids[0];
    let plan = plan_overlay(&tree, &[native]);
    assert_eq!(plan.window_layers, [tree.overlay]);
    assert!(plan.hosted_above.is_empty());
    assert!(plan.has_window());
    // A hidden native has nothing to be above.
    assert_eq!(plan_overlay(&tree, &[]), OverlayPlan::default());

    // base, native A, framework X, native B, overlay: X lies below B, so only the overlay is
    // drawn into the overlay window.
    let (tree, ids) = layout(|tree| {
        vec![
            tree.create_below_overlay(SurfaceKind::Native),
            tree.create_below_overlay(SurfaceKind::Framework),
            tree.create_below_overlay(SurfaceKind::ExternalGpu),
        ]
    });
    let plan = plan_overlay(&tree, &[ids[0], ids[2]]);
    assert_eq!(plan.window_layers, [tree.overlay]);
    // With only A shown, X is above it too.
    let plan = plan_overlay(&tree, &[ids[0]]);
    assert_eq!(plan.window_layers, [ids[1], tree.overlay]);
}

#[test]
fn natives_above_the_overlay_punch_holes_instead_of_joining_the_window_layers() {
    // base, native A, overlay, native N hosted above the overlay.
    let (tree, ids) = layout(|tree| {
        let a = tree.create_below_overlay(SurfaceKind::Native);
        let n = tree.create_below_overlay(SurfaceKind::Native);
        let overlay = tree.overlay();
        tree.place_relative(n, overlay, true).unwrap();
        vec![a, n]
    });
    let plan = plan_overlay(&tree, &[ids[0], ids[1]]);
    assert_eq!(plan.window_layers, [tree.overlay]);
    assert_eq!(plan.hosted_above, [ids[1]]);
    // Without a native below the overlay there is nothing for an overlay window to be above.
    let plan = plan_overlay(&tree, &[ids[1]]);
    assert!(!plan.has_window());
    assert_eq!(plan.hosted_above, [ids[1]]);
}

/// A top-level override-redirect parent window on the test server, with a second, independent
/// connection that inspects what the code under test did.
struct Server {
    connection: RustConnection,
    inspector: RustConnection,
    screen: usize,
    parent: xproto::Window,
    atoms: XcbAtoms,
}

impl Server {
    fn new() -> Self {
        let (connection, screen) = x11rb::connect(None)
            .expect("these tests need a reachable X server (DISPLAY, e.g. XWayland)");
        let (inspector, _) = x11rb::connect(None).expect("a second connection to the X server");
        let parent = connection.generate_id().unwrap();
        let screen_info = &connection.setup().roots[screen];
        connection
            .create_window(
                screen_info.root_depth,
                parent,
                screen_info.root,
                0,
                0,
                400,
                300,
                0,
                xproto::WindowClass::INPUT_OUTPUT,
                screen_info.root_visual,
                &xproto::CreateWindowAux::new().override_redirect(1),
            )
            .unwrap()
            .check()
            .unwrap();
        let atoms = XcbAtoms::new(&connection).unwrap().reply().unwrap();
        connection.sync().unwrap();
        Server {
            connection,
            inspector,
            screen,
            parent,
            atoms,
        }
    }

    fn screen(&self) -> &xproto::Screen {
        &self.connection.setup().roots[self.screen]
    }

    /// The 32-bit ARGB visual of the screen.
    fn argb_visual(&self) -> (xproto::Visualid, u8) {
        self.screen()
            .allowed_depths
            .iter()
            .filter(|depth| depth.depth == 32)
            .flat_map(|depth| depth.visuals.iter())
            .find(|visual| {
                (visual.red_mask, visual.green_mask, visual.blue_mask) == (0xFF0000, 0xFF00, 0xFF)
            })
            .map(|visual| (visual.visual_id, 32))
            .expect("the test server has no 32-bit ARGB visual")
    }

    /// Flushes this connection's requests and waits until the server processed them.
    fn settle(&self) {
        self.connection.sync().unwrap();
    }

    fn shape(&self, window: xproto::Window, kind: shape::SK) -> Vec<xproto::Rectangle> {
        self.settle();
        self.inspector
            .shape_get_rectangles(window, kind)
            .unwrap()
            .reply()
            .unwrap()
            .rectangles
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.connection.destroy_window(self.parent);
        let _ = self.connection.sync();
    }
}

fn covers(rectangles: &[xproto::Rectangle], x: i32, y: i32) -> bool {
    rectangles.iter().any(|r| {
        x >= i32::from(r.x)
            && x < i32::from(r.x) + i32::from(r.width)
            && y >= i32::from(r.y)
            && y < i32::from(r.y) + i32::from(r.height)
    })
}

#[test]
fn native_child_cutout_cuts_bounding_and_input_shape_and_resets() {
    let server = Server::new();
    let child = create_native_child(&server.connection, server.screen(), server.parent).unwrap();
    place_window(
        &server.connection,
        child,
        Geometry {
            x: 50,
            y: 40,
            width: 200,
            height: 100,
        },
    )
    .unwrap();
    set_mapped(&server.connection, child, true).unwrap();

    // Cut two rectangles out of a child.
    let cutout = [rectangle(10, 10, 50, 50), rectangle(100, 0, 100, 100)];
    set_cutout_shape(&server.connection, child, (200, 100), &cutout).unwrap();
    for kind in [shape::SK::BOUNDING, shape::SK::INPUT] {
        let rectangles = server.shape(child, kind);
        assert!(covers(&rectangles, 5, 5), "{kind:?} keeps the corner");
        assert!(covers(&rectangles, 70, 80), "{kind:?} keeps the lower left");
        assert!(
            !covers(&rectangles, 30, 30),
            "{kind:?} cuts the first rectangle"
        );
        assert!(
            !covers(&rectangles, 150, 50),
            "{kind:?} cuts the second rectangle"
        );
        assert!(!covers(&rectangles, 199, 99), "{kind:?} cuts to the edge");
        assert!(
            !covers(&rectangles, 250, 50),
            "{kind:?} stays inside the child"
        );
    }

    // Resizing the child needs its shapes applied again, for the new size.
    place_window(
        &server.connection,
        child,
        Geometry {
            x: 50,
            y: 40,
            width: 300,
            height: 100,
        },
    )
    .unwrap();
    set_cutout_shape(&server.connection, child, (300, 100), &cutout).unwrap();
    let rectangles = server.shape(child, shape::SK::INPUT);
    assert!(covers(&rectangles, 250, 50), "the grown child is not cut");
    assert!(!covers(&rectangles, 150, 50));

    // No cutout: the unshaped window again.
    set_cutout_shape(&server.connection, child, (300, 100), &[]).unwrap();
    for kind in [shape::SK::BOUNDING, shape::SK::INPUT] {
        let rectangles = server.shape(child, kind);
        assert!(
            covers(&rectangles, 30, 30) && covers(&rectangles, 150, 50),
            "{kind:?}"
        );
    }
}

#[test]
fn native_children_stack_bottom_to_top_in_layout_order() {
    let server = Server::new();
    let children: Vec<_> = (0..3)
        .map(|_| create_native_child(&server.connection, server.screen(), server.parent).unwrap())
        .collect();
    let stacking = |server: &Server| {
        server.settle();
        server
            .inspector
            .query_tree(server.parent)
            .unwrap()
            .reply()
            .unwrap()
            .children
    };
    // New windows stack on top in creation order.
    assert_eq!(stacking(&server), children);

    let wanted = [children[2], children[0], children[1]];
    stack_siblings(&server.connection, &wanted).unwrap();
    assert_eq!(stacking(&server), wanted);
}

#[test]
fn overlay_window_has_the_attributes_the_spec_lists_and_an_empty_input_shape() {
    let server = Server::new();
    let (visual, depth) = server.argb_visual();
    let geometry = Geometry {
        x: 30,
        y: 40,
        width: 320,
        height: 200,
    };
    let ids = create_overlay_window(
        &server.connection,
        &OverlayWindowSpec {
            root: server.screen().root,
            visual,
            depth,
            owner: server.parent,
            geometry,
            atoms: &server.atoms,
        },
    )
    .unwrap();
    server.settle();

    let inspector = &server.inspector;
    let attributes = inspector
        .get_window_attributes(ids.window)
        .unwrap()
        .reply()
        .unwrap();
    assert!(attributes.override_redirect, "override-redirect");
    assert_eq!(attributes.visual, visual, "the parent's ARGB visual");
    assert_eq!(attributes.class, xproto::WindowClass::INPUT_OUTPUT);
    assert_eq!(
        attributes.colormap, ids.colormap,
        "a colormap created for the visual"
    );
    assert_ne!(attributes.colormap, server.screen().default_colormap);
    // Not mapped until the frame needs it.
    assert_eq!(attributes.map_state, xproto::MapState::UNMAPPED);
    let window_geometry = inspector.get_geometry(ids.window).unwrap().reply().unwrap();
    assert_eq!(
        (
            window_geometry.depth,
            window_geometry.x,
            window_geometry.y,
            window_geometry.width,
            window_geometry.height
        ),
        (32, 30, 40, 320, 200)
    );

    let property = |property: xproto::Atom, type_: xproto::AtomEnum| {
        inspector
            .get_property(false, ids.window, property, type_, 0, 8)
            .unwrap()
            .reply()
            .unwrap()
            .value32()
            .map(|values| values.collect::<Vec<_>>())
            .unwrap_or_default()
    };
    assert_eq!(
        property(server.atoms.WM_TRANSIENT_FOR, xproto::AtomEnum::WINDOW),
        [server.parent],
        "transient for the parent"
    );
    assert_eq!(
        property(server.atoms._NET_WM_WINDOW_TYPE, xproto::AtomEnum::ATOM),
        [server.atoms._NET_WM_WINDOW_TYPE_DROPDOWN_MENU],
        "a dropdown-menu type window"
    );
    let hints = WmHints::get(inspector, ids.window)
        .unwrap()
        .reply()
        .unwrap()
        .expect("WM_HINTS are set");
    assert_eq!(hints.input, Some(false), "the input focus hint is false");

    // The framework selected pointer motion, button and enter/leave device events (a client only
    // sees its own selection).
    let selected = server
        .connection
        .xinput_xi_get_selected_events(ids.window)
        .unwrap()
        .reply()
        .unwrap();
    let mask = selected
        .masks
        .iter()
        .find(|mask| mask.deviceid == XINPUT_ALL_DEVICE_GROUPS)
        .expect("events are selected for the device groups");
    let wanted = xinput::XIEventMask::MOTION
        | xinput::XIEventMask::BUTTON_PRESS
        | xinput::XIEventMask::BUTTON_RELEASE
        | xinput::XIEventMask::ENTER
        | xinput::XIEventMask::LEAVE;
    assert_eq!(
        mask.mask.first().map(|m| u32::from(*m) & u32::from(wanted)),
        Some(u32::from(wanted))
    );

    // Empty until set from content: events outside hitboxes pass through to the windows below.
    assert!(
        server.shape(ids.window, shape::SK::INPUT).is_empty(),
        "an empty input shape outside hitboxes"
    );

    // The input shape is the hitbox union in device pixels.
    let region = [
        Bounds {
            origin: vespertine::Point::new(px(10.0), px(20.0)),
            size: vespertine::Size {
                width: px(40.0),
                height: px(30.0),
            },
        },
        Bounds {
            origin: vespertine::Point::new(px(100.0), px(50.0)),
            size: vespertine::Size {
                width: px(20.0),
                height: px(20.0),
            },
        },
    ];
    let shape = OverlayShape::new((320, 200), &region, 2.0, &[]);
    set_overlay_shape(&server.connection, ids.window, &shape).unwrap();
    let input = server.shape(ids.window, shape::SK::INPUT);
    assert!(covers(&input, 30, 50) && covers(&input, 220, 120));
    assert!(!covers(&input, 5, 5) && !covers(&input, 150, 100));
    // No holes: the bounding shape is the whole window.
    let bounding = server.shape(ids.window, shape::SK::BOUNDING);
    assert!(covers(&bounding, 0, 0) && covers(&bounding, 319, 199));

    // A native hosted above the overlay punches a hole into both shapes, so it stays visible
    // and receives input.
    let hole = bounds(60, 40, 100, 60);
    let shape = OverlayShape::new((320, 200), &region, 2.0, &[hole]);
    set_overlay_shape(&server.connection, ids.window, &shape).unwrap();
    let input = server.shape(ids.window, shape::SK::INPUT);
    assert!(covers(&input, 30, 50), "hitbox outside the hole");
    assert!(!covers(&input, 70, 50), "hitbox inside the hole");
    let bounding = server.shape(ids.window, shape::SK::BOUNDING);
    assert!(
        covers(&bounding, 10, 10) && !covers(&bounding, 100, 60) && covers(&bounding, 200, 150)
    );

    // Destroyed, not merely unmapped: window and colormap are gone.
    destroy_overlay_window(&server.connection, ids);
    server.settle();
    assert!(
        inspector
            .get_window_attributes(ids.window)
            .unwrap()
            .reply()
            .is_err()
    );
}

#[test]
fn restacking_the_overlay_above_a_reparented_owner_is_harmless() {
    // The owner of a managed window sits inside a window manager frame: the restack above it
    // fails with BadMatch, which must not fail the frame.
    let server = Server::new();
    let (visual, depth) = server.argb_visual();
    let frame = server.connection.generate_id().unwrap();
    let root = server.screen().root;
    let root_visual = server.screen().root_visual;
    let root_depth = server.screen().root_depth;
    server
        .connection
        .create_window(
            root_depth,
            frame,
            root,
            0,
            0,
            500,
            400,
            0,
            xproto::WindowClass::INPUT_OUTPUT,
            root_visual,
            &xproto::CreateWindowAux::new().override_redirect(1),
        )
        .unwrap();
    server
        .connection
        .reparent_window(server.parent, frame, 0, 0)
        .unwrap();
    let ids = create_overlay_window(
        &server.connection,
        &OverlayWindowSpec {
            root,
            visual,
            depth,
            owner: server.parent,
            geometry: Geometry {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            },
            atoms: &server.atoms,
        },
    )
    .unwrap();
    raise_overlay_window(&server.connection, ids.window, server.parent).unwrap();
    server.settle();
    let attributes = server
        .inspector
        .get_window_attributes(ids.window)
        .unwrap()
        .reply()
        .unwrap();
    assert_eq!(attributes.map_state, xproto::MapState::VIEWABLE);
    destroy_overlay_window(&server.connection, ids);
    server.connection.destroy_window(frame).unwrap();
    server.settle();
}

#[test]
fn shape_is_required_and_present_on_the_test_server() {
    let server = Server::new();
    require_shape(&server.connection).expect("the test server provides SHAPE 1.1");
    // The failure message names the missing extension.
    let error = require_x11_shape_extension(false).unwrap_err();
    assert!(error.to_string().contains("SHAPE"), "{error}");
}

#[test]
fn xwayland_and_the_compositing_manager_feed_the_mode_decision() {
    let server = Server::new();
    let facts = compositing_facts(&server.connection, true, || {
        panic!("a compositing manager is not consulted on XWayland")
    });
    if is_xwayland(&server.connection) {
        assert!(facts.xwayland && !facts.compositing_manager && facts.argb_visual);
        assert_eq!(
            vespertine::decide_x11_composition_mode(facts),
            vespertine::X11CompositionMode::Composited
        );
    } else {
        // A plain X server consults the compositing selection.
        let facts = compositing_facts(&server.connection, true, || true);
        assert!(!facts.xwayland && facts.compositing_manager);
    }
    // Without an ARGB visual the SHAPE fallback applies however the server composites.
    let no_argb = compositing_facts(&server.connection, false, || true);
    assert_eq!(
        vespertine::decide_x11_composition_mode(no_argb),
        vespertine::X11CompositionMode::ShapeFallback
    );
}

/// An XWayland server started without the SHAPE extension, killed on drop.
struct ServerWithoutShape(smol::process::Child, u32);

impl Drop for ServerWithoutShape {
    fn drop(&mut self) {
        let _ = self.0.kill();
        // A killed server leaves its socket and lock behind.
        let _ = std::fs::remove_file(format!("/tmp/.X11-unix/X{}", self.1));
        let _ = std::fs::remove_file(format!("/tmp/.X{}-lock", self.1));
    }
}

#[test]
fn enabling_composition_fails_on_a_server_without_the_shape_extension() {
    use smol::process::{Command, Stdio};

    // A private display number far from the session's own, never chosen by the server: starting
    // an X server must not touch the sockets of the display the other tests use.
    let number = 600 + std::process::id() % 300;
    let name = format!(":{number}");
    let child = Command::new("Xwayland")
        .args([
            name.as_str(),
            "-rootless",
            "-extension",
            "SHAPE",
            "-terminate",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("this test starts an XWayland server (Xwayland in PATH, a Wayland session)");
    let _server = ServerWithoutShape(child, number);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let connection = loop {
        match RustConnection::connect(Some(&name)) {
            Ok((connection, _)) => break connection,
            Err(error) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "connecting to the private XWayland {name}: {error}"
                );
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    };
    assert!(!extension_present(&connection, shape::X11_EXTENSION_NAME));
    let error = require_shape(&connection).unwrap_err();
    assert!(error.to_string().contains("SHAPE"), "{error}");
}
