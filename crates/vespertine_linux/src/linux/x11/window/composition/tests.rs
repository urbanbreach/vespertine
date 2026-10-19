//! End-to-end tests of scene layering on X11 (PLT-001 A10): a real window of the X11 platform
//! enables composition, hosts a native child under a dialog, and an independent X connection
//! inspects the windows, shapes and properties the backend produced.
//!
//! They need a reachable X server with the SHAPE extension, an ARGB visual and a GPU
//! (`DISPLAY`, for example the XWayland instance of a Wayland session) and fail loudly without
//! one. Windows are override-redirect pop-ups, so a window manager cannot move or tile them while
//! a test runs.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};

use vespertine::{
    AnyWindowHandle, App, AppContext as _, Application, AsyncApp, Bounds, Context, DispatchPhase,
    InputForwarding, InteractiveElement as _, IntoElement, MouseMoveEvent, NativeLayer,
    NativePointerEvent, NativeSurface, ParentElement as _, Pixels, Render, Styled as _, Window,
    WindowBounds, WindowKind, WindowOptions, WindowingModes, X11CompositingFacts, black, canvas,
    deferred, div, point, px, rgb, size, white,
};
use x11rb::{
    connection::Connection as _,
    properties::WmHints,
    protocol::{
        shape::{self, ConnectionExt as _},
        xinput::{self, ConnectionExt as _},
        xproto::{self, ConnectionExt as _},
        xtest::ConnectionExt as _,
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

use super::{FORCED_FACTS, WINDOWS};
use crate::linux::XcbAtoms;

fn rect(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(x), px(y)),
        size: size(px(width), px(height)),
    }
}

/// What the scenery view paints.
#[derive(Clone)]
struct Plan {
    native: Bounds<Pixels>,
    native_visible: bool,
    /// A translucent backdrop over the whole window (no hitbox) in a deferred overlay.
    backdrop: bool,
    /// An opaque dialog with a hitbox in the overlay.
    dialog: bool,
    /// The native child is positioned inside the dialog, hosted above the overlay.
    native_in_dialog: bool,
    /// A second native child below the overlay at `ANCHOR`, shown or hidden.
    anchor: bool,
}

const DIALOG: (f32, f32, f32, f32) = (260.0, 100.0, 340.0, 220.0);

struct Scenery {
    plan: Rc<RefCell<Plan>>,
    /// The movable native child and the anchor.
    surfaces: Rc<RefCell<Vec<Rc<NativeSurface>>>>,
    /// The window-content positions of the pointer moves the framework saw, before any dispatch.
    moves: Rc<RefCell<Vec<(f32, f32)>>>,
}

const ANCHOR: (f32, f32, f32, f32) = (20.0, 400.0, 120.0, 40.0);

fn position_native(
    surface: Rc<NativeSurface>,
    bounds: Bounds<Pixels>,
    visible: bool,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |_, _, window: &mut Window, _: &mut App| surface.position(window, bounds, visible),
    )
    .size(px(1.))
}

impl Render for Scenery {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let plan = self.plan.borrow().clone();
        let surfaces = self.surfaces.borrow().clone();
        let surface = surfaces.first().cloned();
        let mut root = div().size_full().bg(rgb(0x101820));
        let moves = self.moves.clone();
        root = root.child(
            canvas(
                |_, _, _| (),
                move |_, _, window: &mut Window, _: &mut App| {
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, _| {
                        if phase == DispatchPhase::Capture {
                            moves
                                .borrow_mut()
                                .push((f32::from(event.position.x), f32::from(event.position.y)));
                        }
                    });
                },
            )
            .size(px(1.)),
        );
        if let Some(anchor) = surfaces.get(1) {
            let (x, y, width, height) = ANCHOR;
            root = root.child(position_native(
                anchor.clone(),
                rect(x, y, width, height),
                plan.anchor,
            ));
        }
        if let Some(surface) = &surface
            && !plan.native_in_dialog
        {
            root = root.child(position_native(
                surface.clone(),
                plan.native,
                plan.native_visible,
            ));
        }
        if plan.backdrop || plan.dialog {
            let mut overlay = div().absolute().top_0().left_0().size_full();
            if plan.backdrop {
                overlay = overlay.child(
                    div()
                        .size_full()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bg(black().opacity(0.2)),
                );
            }
            if plan.dialog {
                let (x, y, width, height) = DIALOG;
                let mut dialog = div()
                    .absolute()
                    .top(px(y))
                    .left(px(x))
                    .w(px(width))
                    .h(px(height))
                    .bg(white())
                    .rounded(px(12.))
                    .occlude();
                if let Some(surface) = &surface
                    && plan.native_in_dialog
                {
                    dialog = dialog.child(position_native(
                        surface.clone(),
                        plan.native,
                        plan.native_visible,
                    ));
                }
                overlay = overlay.child(dialog);
            }
            root = root.child(deferred(overlay));
        }
        root
    }
}

/// An independent connection to the server the windows live on.
struct Inspector {
    connection: RustConnection,
    root: xproto::Window,
    atoms: XcbAtoms,
}

impl Inspector {
    fn new() -> Self {
        let (connection, screen) = x11rb::connect(None)
            .expect("this test needs a reachable X server (DISPLAY, e.g. XWayland)");
        let root = connection.setup().roots[screen].root;
        let atoms = XcbAtoms::new(&connection).unwrap().reply().unwrap();
        Inspector {
            connection,
            root,
            atoms,
        }
    }

    fn children(&self, window: xproto::Window) -> Vec<xproto::Window> {
        self.connection
            .query_tree(window)
            .unwrap()
            .reply()
            .map(|tree| tree.children)
            .unwrap_or_default()
    }

    fn attributes(&self, window: xproto::Window) -> Option<xproto::GetWindowAttributesReply> {
        self.connection
            .get_window_attributes(window)
            .ok()?
            .reply()
            .ok()
    }

    fn geometry(&self, window: xproto::Window) -> Option<xproto::GetGeometryReply> {
        self.connection.get_geometry(window).ok()?.reply().ok()
    }

    fn property(
        &self,
        window: xproto::Window,
        property: xproto::Atom,
        type_: xproto::AtomEnum,
    ) -> Vec<u32> {
        self.connection
            .get_property(false, window, property, type_, 0, 8)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .and_then(|reply| reply.value32().map(|values| values.collect()))
            .unwrap_or_default()
    }

    /// The root-level windows that are transients of `owner`: its overlay windows.
    fn overlays_of(&self, owner: xproto::Window) -> Vec<xproto::Window> {
        self.children(self.root)
            .into_iter()
            .filter(|window| {
                self.property(
                    *window,
                    self.atoms.WM_TRANSIENT_FOR,
                    xproto::AtomEnum::WINDOW,
                ) == [owner]
            })
            .collect()
    }

    fn shape(&self, window: xproto::Window, kind: shape::SK) -> Vec<xproto::Rectangle> {
        self.connection
            .shape_get_rectangles(window, kind)
            .unwrap()
            .reply()
            .map(|reply| reply.rectangles)
            .unwrap_or_default()
    }

    fn root_origin(&self, window: xproto::Window) -> (i16, i16) {
        let reply = self
            .connection
            .translate_coordinates(window, self.root, 0, 0)
            .unwrap()
            .reply()
            .unwrap();
        (reply.dst_x, reply.dst_y)
    }
}

fn covers(rectangles: &[xproto::Rectangle], x: f32, y: f32) -> bool {
    rectangles.iter().any(|r| {
        x >= f32::from(r.x)
            && x < f32::from(r.x) + f32::from(r.width)
            && y >= f32::from(r.y)
            && y < f32::from(r.y) + f32::from(r.height)
    })
}

/// Waits (running the application's event loop) until `probe` returns a value.
async fn eventually<T>(cx: &AsyncApp, what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(value) = probe() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
}

/// Lets a few frames go by, for assertions that something does not happen.
async fn settle(cx: &AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(250))
        .await;
}

struct Scene {
    cx: AsyncApp,
    window: AnyWindowHandle,
    plan: Rc<RefCell<Plan>>,
    surfaces: Vec<Rc<NativeSurface>>,
    owner: xproto::Window,
    child: xproto::Window,
    /// The window of the anchor native child.
    anchor_child: xproto::Window,
    scale: f32,
    inspector: Inspector,
    /// The pointer moves the framework saw.
    moves: Rc<RefCell<Vec<(f32, f32)>>>,
}

impl Scene {
    async fn open(cx: &AsyncApp) -> Scene {
        let plan = Rc::new(RefCell::new(Plan {
            native: rect(180.0, 40.0, 580.0, 340.0),
            native_visible: true,
            backdrop: false,
            dialog: false,
            native_in_dialog: false,
            anchor: false,
        }));
        let moves: Rc<RefCell<Vec<(f32, f32)>>> = Rc::default();
        let slot: Rc<RefCell<Vec<Rc<NativeSurface>>>> = Rc::default();
        let (window, surfaces, scale) = cx.update(|cx: &mut App| {
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(rect(40.0, 40.0, 800.0, 460.0))),
                        kind: WindowKind::PopUp,
                        focus: false,
                        ..Default::default()
                    },
                    {
                        let (plan, slot, moves) = (plan.clone(), slot.clone(), moves.clone());
                        move |window, cx| {
                            window
                                .enable_window_composition()
                                .expect("composition is available on the test server");
                            for _ in 0..2 {
                                let surface = window
                                    .composition()
                                    .unwrap()
                                    .create_native_surface()
                                    .unwrap();
                                slot.borrow_mut().push(Rc::new(surface));
                            }
                            cx.new(|_| Scenery {
                                plan,
                                surfaces: slot,
                                moves,
                            })
                        }
                    },
                )
                .expect("the window opens");
            let window: AnyWindowHandle = window.into();
            let scale = window
                .update(cx, |_, window, _| window.scale_factor())
                .unwrap();
            let surfaces = slot.borrow().clone();
            (window, surfaces, scale)
        });
        let owner = cx.update(|cx: &mut App| {
            window
                .update(cx, |_, window, _| match window.native_composition() {
                    Some(composition) => match composition.layer {
                        NativeLayer::X11 { window } => window,
                        other => panic!("not an X11 layer: {other:?}"),
                    },
                    None => panic!("composition is enabled"),
                })
                .unwrap()
        });
        let layer_of = |surface: &Rc<NativeSurface>| match surface.layer() {
            Some(NativeLayer::X11 { window }) => window,
            other => panic!("the native surface has no X11 window: {other:?}"),
        };
        let (child, anchor_child) = (layer_of(&surfaces[0]), layer_of(&surfaces[1]));
        Scene {
            cx: cx.clone(),
            window,
            plan,
            surfaces,
            owner,
            child,
            anchor_child,
            scale,
            inspector: Inspector::new(),
            moves,
        }
    }

    /// Changes what the view paints and schedules a frame.
    fn paint(&self, change: impl FnOnce(&mut Plan)) {
        change(&mut self.plan.borrow_mut());
        self.cx.update(|cx: &mut App| {
            self.window
                .update(cx, |_, window, _| window.refresh())
                .unwrap()
        });
    }

    fn stats(&self) -> vespertine::CompositionStats {
        self.cx.update(|cx: &mut App| {
            self.window
                .update(cx, |_, window, _| window.composition_stats().unwrap())
                .unwrap()
        })
    }

    fn generation(&self) -> u64 {
        self.cx.update(|cx: &mut App| {
            self.window
                .update(cx, |_, window, _| {
                    window.native_composition().unwrap().generation
                })
                .unwrap()
        })
    }

    fn device(&self, logical: f32) -> f32 {
        logical * self.scale
    }

    /// Moves the X pointer (through the XTEST extension, like a user) to a position in window
    /// content logical pixels.
    fn pointer_to(&self, x: f32, y: f32) {
        let (origin_x, origin_y) = self.inspector.root_origin(self.owner);
        let target = (
            origin_x + self.device(x).round() as i16,
            origin_y + self.device(y).round() as i16,
        );
        self.inspector
            .connection
            .xtest_fake_input(
                xproto::MOTION_NOTIFY_EVENT,
                0,
                x11rb::CURRENT_TIME,
                self.inspector.root,
                target.0,
                target.1,
                0,
            )
            .unwrap();
        self.inspector.connection.flush().unwrap();
    }

    /// Moves the pointer to the position by way of a neighbouring one, so the server sees a
    /// motion even when the pointer already is there.
    fn pointer_nudge_to(&self, x: f32, y: f32) {
        self.pointer_to(x + 2.0, y + 2.0);
        self.pointer_to(x, y);
    }

    /// Whether the framework saw a pointer move at `(x, y)` logical pixels (within a pixel).
    fn saw_move_at(&self, x: f32, y: f32) -> bool {
        self.moves
            .borrow()
            .iter()
            .any(|(mx, my)| (mx - x).abs() <= 1.0 && (my - y).abs() <= 1.0)
    }

    async fn child_mapped_at(&self, bounds: Bounds<Pixels>) {
        self.child_mapped_at_window(self.child, bounds).await;
    }

    async fn child_mapped_at_window(&self, child: xproto::Window, bounds: Bounds<Pixels>) {
        let (x, y, width, height) = (
            f32::from(bounds.origin.x),
            f32::from(bounds.origin.y),
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
        );
        let expected = (
            self.device(x).round() as i16,
            self.device(y).round() as i16,
            self.device(width).round() as u16,
            self.device(height).round() as u16,
        );
        eventually(&self.cx, "the native child to be placed and mapped", || {
            let geometry = self.inspector.geometry(child)?;
            let attributes = self.inspector.attributes(child)?;
            (attributes.map_state == xproto::MapState::VIEWABLE
                && (geometry.x, geometry.y, geometry.width, geometry.height) == expected)
                .then_some(())
        })
        .await;
    }
}

fn dialog_device(scene: &Scene) -> (f32, f32, f32, f32) {
    let (x, y, width, height) = DIALOG;
    (
        scene.device(x),
        scene.device(y),
        scene.device(width),
        scene.device(height),
    )
}

/// Composited mode: the real server (XWayland or a compositing X server).
async fn composited(cx: &AsyncApp) {
    let scene = Scene::open(cx).await;
    let inspector = &scene.inspector;
    let native = scene.plan.borrow().native;
    let initial_generation = scene.generation();

    // R21: the native producer's window is a child of the window itself.
    assert!(inspector.children(scene.owner).contains(&scene.child));
    scene.child_mapped_at(native).await;
    // R7: no overlay content, no overlay window.
    settle(cx).await;
    assert!(
        inspector.overlays_of(scene.owner).is_empty(),
        "no overlay window without overlay content"
    );
    // Unshaped: the child covers its whole area.
    let unshaped = inspector.shape(scene.child, shape::SK::BOUNDING);
    assert!(covers(&unshaped, scene.device(300.0), scene.device(100.0)));

    // R22: translucent backdrop and an opaque dialog in the overlay create the overlay window.
    let before = vespertine_wgpu::gpu_creation_counts();
    scene.paint(|plan| {
        plan.backdrop = true;
        plan.dialog = true;
    });
    let overlay = eventually(cx, "the overlay window", || {
        inspector.overlays_of(scene.owner).first().copied()
    })
    .await;
    let attributes = inspector.attributes(overlay).unwrap();
    assert!(attributes.override_redirect, "override-redirect");
    let geometry = inspector.geometry(overlay).unwrap();
    assert_eq!(geometry.depth, 32, "the parent's ARGB visual");
    assert_eq!(
        attributes.visual,
        inspector.attributes(scene.owner).unwrap().visual
    );
    assert_ne!(
        attributes.colormap,
        inspector.attributes(scene.owner).unwrap().colormap,
        "a colormap created for the overlay window"
    );
    assert_eq!(
        inspector.property(
            overlay,
            inspector.atoms.WM_TRANSIENT_FOR,
            xproto::AtomEnum::WINDOW
        ),
        [scene.owner]
    );
    assert_eq!(
        inspector.property(
            overlay,
            inspector.atoms._NET_WM_WINDOW_TYPE,
            xproto::AtomEnum::ATOM
        ),
        [inspector.atoms._NET_WM_WINDOW_TYPE_DROPDOWN_MENU]
    );
    let hints = WmHints::get(&inspector.connection, overlay)
        .unwrap()
        .reply()
        .unwrap()
        .expect("WM_HINTS");
    assert_eq!(
        hints.input,
        Some(false),
        "the overlay never takes input focus"
    );
    // At the window's root-relative origin, the window's device-pixel size, mapped.
    let owner_geometry = inspector.geometry(scene.owner).unwrap();
    eventually(
        cx,
        "the overlay window to be placed above the window",
        || {
            let geometry = inspector.geometry(overlay)?;
            let attributes = inspector.attributes(overlay)?;
            let (x, y) = inspector.root_origin(scene.owner);
            (attributes.map_state == xproto::MapState::VIEWABLE
                && (geometry.x, geometry.y, geometry.width, geometry.height)
                    == (x, y, owner_geometry.width, owner_geometry.height))
                .then_some(())
        },
    )
    .await;
    // The input shape is the overlay hitbox union (the dialog) in device pixels: events outside
    // pass through to the child and the window below.
    let (dx, dy, dw, dh) = dialog_device(&scene);
    let input = eventually(cx, "the overlay input shape", || {
        let input = inspector.shape(overlay, shape::SK::INPUT);
        covers(&input, dx + dw / 2.0, dy + dh / 2.0).then_some(input)
    })
    .await;
    assert!(
        !covers(&input, 10.0, 10.0),
        "outside the dialog the overlay lets input through"
    );
    assert!(!covers(&input, dx + dw + 5.0, dy + dh / 2.0));
    assert!(covers(&input, dx + 2.0, dy + 2.0) && covers(&input, dx + dw - 2.0, dy + dh - 2.0));
    // R8: the overlay renderer shares the window renderer's device and atlas.
    let created = vespertine_wgpu::gpu_creation_counts().since(before);
    assert_eq!(
        (created.devices, created.atlases),
        (0, 0),
        "no second device or atlas"
    );
    // R23: only the opaque dialog is cut out of the child's shapes; the translucent backdrop
    // is not.
    let center = (dx + dw / 2.0, dy + dh / 2.0);
    let child_x = scene.device(180.0);
    let child_y = scene.device(40.0);
    for kind in [shape::SK::BOUNDING, shape::SK::INPUT] {
        let shape = eventually(cx, "the child's shapes to be cut by the dialog", || {
            let shape = inspector.shape(scene.child, kind);
            (!covers(&shape, center.0 - child_x, center.1 - child_y)).then_some(shape)
        })
        .await;
        assert!(
            covers(&shape, scene.device(20.0), scene.device(20.0)),
            "{kind:?}: the child keeps the area outside the dialog"
        );
    }
    assert_eq!(scene.generation(), initial_generation, "no recreation yet");

    // R22: destroyed (not merely unmapped) when the overlay scene and input are empty.
    scene.paint(|plan| {
        plan.backdrop = false;
        plan.dialog = false;
    });
    eventually(cx, "the overlay window to be destroyed", || {
        (inspector.attributes(overlay).is_none() && inspector.overlays_of(scene.owner).is_empty())
            .then_some(())
    })
    .await;
    let restored = eventually(cx, "the child's shape to be restored", || {
        let shape = inspector.shape(scene.child, shape::SK::INPUT);
        covers(&shape, center.0 - child_x, center.1 - child_y).then_some(shape)
    })
    .await;
    assert!(covers(&restored, scene.device(20.0), scene.device(20.0)));

    // A new overlay window reuses the spare renderer: still no new device or atlas, and the
    // generation is unchanged because nothing was lost.
    scene.paint(|plan| plan.dialog = true);
    let second = eventually(cx, "the second overlay window", || {
        inspector.overlays_of(scene.owner).first().copied()
    })
    .await;
    assert_ne!(second, overlay, "a new overlay window");
    let created = vespertine_wgpu::gpu_creation_counts().since(before);
    assert_eq!(
        (created.devices, created.atlases),
        (0, 0),
        "the spare renderer is reused"
    );
    assert_eq!(scene.generation(), initial_generation);

    // R11: moving the only native child into the dialog hosts it above the overlay. No native
    // child is left below the overlay for an overlay window to blend over, so the overlay
    // window goes away and the window's own pixels (the dialog included) lie under the child.
    let inside = rect(280.0, 120.0, 120.0, 90.0);
    scene.paint(|plan| {
        plan.native = inside;
        plan.native_in_dialog = true;
    });
    scene.child_mapped_at(inside).await;
    eventually(
        cx,
        "the overlay window to go with the last native below it",
        || inspector.attributes(second).is_none().then_some(()),
    )
    .await;
    // With a native child below the overlay again (the anchor), the overlay window comes back
    // with a hole where the hosted child is, so the child stays visible and receives input.
    scene.paint(|plan| plan.anchor = true);
    let (ax, ay, aw, ah) = ANCHOR;
    scene
        .child_mapped_at_window(scene.anchor_child, rect(ax, ay, aw, ah))
        .await;
    let third = eventually(cx, "the overlay window above the anchor", || {
        inspector.overlays_of(scene.owner).first().copied()
    })
    .await;
    let (hx, hy) = (scene.device(340.0), scene.device(165.0));
    for kind in [shape::SK::BOUNDING, shape::SK::INPUT] {
        let shape = eventually(
            cx,
            "a hole for the hosted child in the overlay window",
            || {
                let shape = inspector.shape(third, kind);
                (!shape.is_empty() && !covers(&shape, hx, hy)).then_some(shape)
            },
        )
        .await;
        if kind == shape::SK::BOUNDING {
            assert!(
                covers(&shape, dx + dw - 6.0, dy + dh - 6.0),
                "{kind:?}: the dialog around the child stays"
            );
        }
    }
    // And back below the overlay: the hole closes.
    scene.paint(|plan| {
        plan.native = native;
        plan.native_in_dialog = false;
    });
    scene.child_mapped_at(native).await;
    eventually(cx, "the overlay hole to close", || {
        covers(&inspector.shape(third, shape::SK::BOUNDING), hx, hy).then_some(())
    })
    .await;

    // R14: over the dialog's hitbox the framework gets the pointer events from the overlay
    // window, mapped to window-content coordinates.
    eventually(
        cx,
        "the framework to see the pointer over the dialog",
        || {
            scene.pointer_nudge_to(430.0, 210.0);
            scene.saw_move_at(430.0, 210.0).then_some(())
        },
    )
    .await;
    // Outside every hitbox the events reach the windows below: here the child, which selects
    // no events itself, so they continue to the window and the framework sees them.
    eventually(
        cx,
        "the framework to see the pointer over the bare child",
        || {
            scene.pointer_nudge_to(200.0, 60.0);
            scene.saw_move_at(200.0, 60.0).then_some(())
        },
    )
    .await;

    // R16: an OS-managed child keeps its native event path: a producer that selects pointer
    // events on its window receives them, and the framework does not.
    inspector
        .connection
        .xinput_xi_query_version(2, 0)
        .unwrap()
        .reply()
        .unwrap();
    inspector
        .connection
        .xinput_xi_select_events(
            scene.child,
            &[xinput::EventMask {
                deviceid: 1,
                mask: vec![xinput::XIEventMask::MOTION],
            }],
        )
        .unwrap();
    inspector.connection.flush().unwrap();
    let producer_saw = Cell::new(false);
    eventually(
        cx,
        "the producer to receive the pointer over its window",
        || {
            scene.pointer_nudge_to(220.0, 80.0);
            while let Some(event) = inspector.connection.poll_for_event().unwrap() {
                if let x11rb::protocol::Event::XinputMotion(motion) = event
                    && motion.event == scene.child
                {
                    producer_saw.set(true);
                }
            }
            producer_saw.get().then_some(())
        },
    )
    .await;
    settle(cx).await;
    assert!(
        !scene.saw_move_at(220.0, 80.0),
        "an OS-managed child keeps its pointer events by default"
    );

    // With input forwarding the framework selects the events too and forwards them with the
    // slot origin added: the producer's handler sees window-content positions.
    let forwarded: Rc<RefCell<Vec<(f32, f32)>>> = Rc::default();
    scene.surfaces[0].set_pointer_handler({
        let forwarded = forwarded.clone();
        move |event, _, _| {
            if let NativePointerEvent::Enter(event) | NativePointerEvent::Move(event) = event {
                forwarded
                    .borrow_mut()
                    .push((f32::from(event.position.x), f32::from(event.position.y)));
            }
            false
        }
    });
    scene.surfaces[0].set_input_forwarding(InputForwarding::Framework);
    inspector.connection.sync().unwrap();
    eventually(cx, "the forwarded pointer event", || {
        scene.pointer_nudge_to(240.0, 90.0);
        forwarded
            .borrow()
            .iter()
            .any(|(x, y)| (x - 240.0).abs() <= 1.0 && (y - 90.0).abs() <= 1.0)
            .then_some(())
    })
    .await;
    assert!(scene.saw_move_at(240.0, 90.0));
    scene.surfaces[0].set_input_forwarding(InputForwarding::Native);
    inspector
        .connection
        .xinput_xi_select_events(
            scene.child,
            &[xinput::EventMask {
                deviceid: 1,
                mask: Vec::new(),
            }],
        )
        .unwrap();
    inspector.connection.flush().unwrap();
    // R10/R17: a move plus resize is one write, and unchanged bounds write nothing.
    let writes = scene.stats().bounds_writes;
    settle(cx).await;
    assert_eq!(
        scene.stats().bounds_writes,
        writes,
        "unchanged bounds are not written"
    );
    let moved = rect(200.0, 50.0, 500.0, 300.0);
    scene.paint(|plan| plan.native = moved);
    scene.child_mapped_at(moved).await;
    assert_eq!(
        scene.stats().bounds_writes,
        writes + 1,
        "one write per move"
    );

    // Hidden: unmapped.
    scene.paint(|plan| plan.native_visible = false);
    eventually(cx, "the native child to be unmapped", || {
        (inspector.attributes(scene.child)?.map_state == xproto::MapState::UNMAPPED).then_some(())
    })
    .await;

    // R13: closing the window releases the overlay window and the native child.
    scene.paint(|plan| plan.native_visible = true);
    scene.child_mapped_at(moved).await;

    // R18: a compositor-surface recreation changes the generation and tells the attachments.
    let window_state = WINDOWS
        .with_borrow(|windows| windows.iter().find(|w| w.x_window == scene.owner).cloned())
        .expect("the window is tracked");
    let recreations = Rc::new(Cell::new(0usize));
    for surface in &scene.surfaces {
        let recreations = recreations.clone();
        surface.on_recreated(move |composition| {
            assert!(matches!(composition.layer, NativeLayer::X11 { .. }));
            recreations.set(recreations.get() + 1);
        });
    }
    let lose_overlay_renderer = || {
        let mut state = window_state.state.borrow_mut();
        let overlay = state
            .composition
            .as_mut()
            .unwrap()
            .overlay
            .as_mut()
            .unwrap();
        // A renderer whose GPU resources are gone reports that it is not reusable.
        overlay.renderer.destroy();
        assert!(!overlay.renderer.is_reusable());
    };
    let generation = scene.generation();
    let overlay = inspector.overlays_of(scene.owner)[0];
    lose_overlay_renderer();
    scene.paint(|plan| plan.backdrop = !plan.backdrop);
    let recreated_overlay = eventually(cx, "the overlay window to be recreated", || {
        inspector
            .overlays_of(scene.owner)
            .first()
            .copied()
            .filter(|window| *window != overlay)
    })
    .await;
    assert_eq!(scene.generation(), generation + 1, "the generation changed");
    assert_eq!(recreations.get(), 2, "both attachments learned about it");
    let created = vespertine_wgpu::gpu_creation_counts().since(before);
    assert_eq!(
        (created.devices, created.atlases),
        (0, 0),
        "the new renderer shares device and atlas"
    );
    // Both planes draw again: the new overlay window is placed, mapped and has its input shape.
    eventually(cx, "the recreated overlay window to be shown", || {
        let attributes = inspector.attributes(recreated_overlay)?;
        let input = inspector.shape(recreated_overlay, shape::SK::INPUT);
        (attributes.map_state == xproto::MapState::VIEWABLE && covers(&input, dx + 5.0, dy + 5.0))
            .then_some(())
    })
    .await;

    // Device recovery of the window's own renderer counts once, even though it also made the
    // overlay renderer stale.
    let generation = scene.generation();
    recreations.set(0);
    lose_overlay_renderer();
    window_state.note_device_recovered();
    assert_eq!(scene.generation(), generation + 1);
    assert_eq!(recreations.get(), 2);
    scene.paint(|plan| plan.backdrop = !plan.backdrop);
    eventually(
        cx,
        "the overlay window to be recreated after recovery",
        || {
            inspector
                .overlays_of(scene.owner)
                .first()
                .copied()
                .filter(|window| *window != recreated_overlay)
        },
    )
    .await;
    assert_eq!(
        scene.generation(),
        generation + 1,
        "recovery is not counted twice"
    );

    // A spare renderer that reports a lost device is discarded, not reused.
    scene.paint(|plan| {
        plan.backdrop = false;
        plan.dialog = false;
    });
    eventually(cx, "the overlay window to be destroyed again", || {
        inspector.overlays_of(scene.owner).is_empty().then_some(())
    })
    .await;
    {
        let mut state = window_state.state.borrow_mut();
        let spare = state.composition.as_mut().unwrap().spare.as_mut();
        let spare = spare.expect("the destroyed overlay window left a spare renderer");
        assert!(spare.is_reusable());
        spare.destroy();
    }
    let generation = scene.generation();
    scene.paint(|plan| plan.dialog = true);
    eventually(cx, "an overlay window from a new renderer", || {
        inspector.overlays_of(scene.owner).first().copied()
    })
    .await;
    assert_eq!(
        scene.generation(),
        generation + 1,
        "a discarded spare changes the generation"
    );
    let created = vespertine_wgpu::gpu_creation_counts().since(before);
    assert_eq!((created.devices, created.atlases), (0, 0));
    let overlay = inspector.overlays_of(scene.owner)[0];
    scene.cx.update(|cx: &mut App| {
        scene
            .window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap()
    });
    eventually(
        cx,
        "the window, its child and its overlay window to be destroyed",
        || {
            (inspector.attributes(scene.owner).is_none()
                && inspector.attributes(scene.child).is_none()
                && inspector.attributes(overlay).is_none())
            .then_some(())
        },
    )
    .await;
    drop(scene.surfaces);
}

/// The SHAPE fallback: no ARGB compositing, so no overlay window; everything above a native
/// child is cut out of its shapes, translucent content included.
async fn shape_fallback(cx: &AsyncApp) {
    FORCED_FACTS.set(Some(X11CompositingFacts {
        argb_visual: false,
        xwayland: false,
        compositing_manager: false,
    }));
    let scene = Scene::open(cx).await;
    let inspector = &scene.inspector;
    let native = scene.plan.borrow().native;
    scene.child_mapped_at(native).await;

    // A translucent backdrop over everything cuts the whole child out in this mode (R24)...
    scene.paint(|plan| plan.backdrop = true);
    for kind in [shape::SK::BOUNDING, shape::SK::INPUT] {
        eventually(cx, "the translucent backdrop to cut the child", || {
            inspector.shape(scene.child, kind).is_empty().then_some(())
        })
        .await;
    }
    // ...and there is no overlay window, ever.
    settle(cx).await;
    assert!(inspector.overlays_of(scene.owner).is_empty());

    // The dialog alone cuts out exactly its rectangle.
    scene.paint(|plan| {
        plan.backdrop = false;
        plan.dialog = true;
    });
    let (dx, dy, dw, dh) = dialog_device(&scene);
    let (child_x, child_y) = (scene.device(180.0), scene.device(40.0));
    let shape = eventually(cx, "the dialog to cut the child", || {
        let shape = inspector.shape(scene.child, shape::SK::BOUNDING);
        (!shape.is_empty() && !covers(&shape, dx + dw / 2.0 - child_x, dy + dh / 2.0 - child_y))
            .then_some(shape)
    })
    .await;
    assert!(covers(&shape, scene.device(20.0), scene.device(20.0)));
    assert!(covers(
        &shape,
        dx + dw + 5.0 - child_x,
        dy + dh / 2.0 - child_y
    ));
    assert!(inspector.overlays_of(scene.owner).is_empty());

    scene.paint(|plan| plan.dialog = false);
    eventually(cx, "the child to be whole again", || {
        covers(
            &inspector.shape(scene.child, shape::SK::BOUNDING),
            dx + dw / 2.0 - child_x,
            dy + dh / 2.0 - child_y,
        )
        .then_some(())
    })
    .await;
    scene.cx.update(|cx: &mut App| {
        scene
            .window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap()
    });
    FORCED_FACTS.set(None);
}

#[test]
fn x11_composition_end_to_end() {
    // The platform of this test uses the process-wide clipboard connection that the clipboard
    // tests serialize on.
    let _serial = crate::linux::x11::clipboard::tests::SERIAL.lock();
    Application::with_platform(crate::linux_platform(WindowingModes::X11)).run(|cx: &mut App| {
        cx.spawn(async move |cx: &mut AsyncApp| {
            composited(cx).await;
            shape_fallback(cx).await;
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}
