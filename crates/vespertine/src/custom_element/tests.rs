use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    sync::Arc,
};

use image::{Frame, ImageBuffer, Rgba};
use serde_json::{Map, Value, json};
use smallvec::SmallVec;

use super::*;
use crate::{
    AppContext as _, Bounds, Context, Entity, ExternalPaths, FileDropEvent, HostClickEvent,
    HostEvent, HostFileDropEvent, InputEvent as _, Modifiers, MouseButton, MouseDownEvent,
    MouseUpEvent, Pixels, PlatformInput, Point, Render, TestAppContext, WeakEntity, WindowHandle,
    canvas, div, point, px, size,
};

#[test]
fn json_equality_is_deep_and_numeric() {
    assert!(json_equal(&json!(1), &json!(1.0)));
    assert!(!json_equal(&json!(1), &json!(2)));
    assert!(json_equal(
        &json!({"a": [1, {"b": 2.0}], "c": null}),
        &json!({"c": null, "a": [1.0, {"b": 2}]})
    ));
    assert!(!json_equal(&json!({"a": 1}), &json!({"a": 1, "b": 1})));
    assert!(!json_equal(&json!([1, 2]), &json!([2, 1])));
    assert!(!json_equal(&json!("1"), &json!(1)));
}

// --- counting factory ------------------------------------------------------------------

/// Ordered log shared by every probe of a test, so cross-adapter ordering is observable.
type OrderLog = Rc<RefCell<Vec<String>>>;

struct Probe {
    name: &'static str,
    order: OrderLog,
    created: Cell<usize>,
    destroyed: Cell<usize>,
    renders: Cell<usize>,
    sets: RefCell<Vec<(String, Value)>>,
    events_seen: RefCell<Vec<Vec<String>>>,
    entities: RefCell<Vec<WeakEntity<()>>>,
}

impl Probe {
    fn new(name: &'static str, order: &OrderLog) -> Rc<Self> {
        Rc::new(Self {
            name,
            order: order.clone(),
            created: Cell::default(),
            destroyed: Cell::default(),
            renders: Cell::default(),
            sets: RefCell::default(),
            events_seen: RefCell::default(),
            entities: RefCell::default(),
        })
    }

    fn log(&self, what: &str, id: HostId) {
        self.order
            .borrow_mut()
            .push(format!("{what}:{}:{id}", self.name));
    }
}

struct CountingFactory {
    name: &'static str,
    probe: Rc<Probe>,
    properties: &'static [&'static str],
    events: &'static [&'static str],
}

impl ElementFactory for CountingFactory {
    fn type_name(&self) -> &'static str {
        self.name
    }

    fn create(&self, id: HostId) -> Box<dyn ElementAdapter> {
        self.probe.created.set(self.probe.created.get() + 1);
        self.probe.log("create", id);
        Box::new(CountingAdapter {
            id,
            probe: self.probe.clone(),
            properties: self.properties,
            events: self.events,
            entity: None,
            image: None,
            replaced: Vec::new(),
        })
    }
}

struct CountingAdapter {
    id: HostId,
    probe: Rc<Probe>,
    properties: &'static [&'static str],
    events: &'static [&'static str],
    entity: Option<Entity<()>>,
    image: Option<Arc<RenderImage>>,
    replaced: Vec<Arc<RenderImage>>,
}

impl ElementAdapter for CountingAdapter {
    fn render(
        &mut self,
        mut context: RenderContext,
        _window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        self.probe.renders.set(self.probe.renders.get() + 1);
        self.probe.log("render", self.id);
        self.probe.events_seen.borrow_mut().push(
            context
                .events
                .iter()
                .map(|event| event.to_string())
                .collect(),
        );
        if self.entity.is_none() {
            let entity = cx.new(|_| ());
            self.probe.entities.borrow_mut().push(entity.downgrade());
            self.entity = Some(entity);
        }

        let mut surface = context.surface().w(px(40.)).h(px(20.));
        surface.extend([
            context.selectable_text(0, "alpha", None).into_any_element(),
            context.selectable_text(1, "beta", None).into_any_element(),
            context.chrome_text(0, "1", None).into_any_element(),
        ]);
        let mut element = div().child(surface);
        if let Some(image) = self.image.clone() {
            element = element.child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        window
                            .paint_image(bounds, bounds, Default::default(), image, 0, false)
                            .unwrap();
                    },
                )
                .w(px(4.))
                .h(px(4.)),
            );
        }
        element.into_any_element()
    }

    fn set_property(&mut self, name: &str, value: &Value) {
        self.probe
            .sets
            .borrow_mut()
            .push((name.to_owned(), value.clone()));
    }

    fn supported_properties(&self) -> &'static [&'static str] {
        self.properties
    }

    fn supported_events(&self) -> &'static [&'static str] {
        self.events
    }

    fn destroy(&mut self, _cx: &mut App) {
        self.probe.destroyed.set(self.probe.destroyed.get() + 1);
        self.probe.log("destroy", self.id);
        self.entity = None;
    }

    fn live_image(&self) -> Option<Arc<RenderImage>> {
        self.image.clone()
    }

    fn take_replaced_image(&mut self) -> Option<Arc<RenderImage>> {
        self.replaced.pop()
    }

    fn replace_live_image(&mut self, image: Arc<RenderImage>) -> Result<Option<Arc<RenderImage>>> {
        let previous = self.image.replace(image);
        if let Some(previous) = &previous {
            self.replaced.push(previous.clone());
        }
        Ok(previous)
    }
}

// --- host view -------------------------------------------------------------------------

struct HostSpec {
    id: u64,
    type_name: &'static str,
    properties: Map<String, Value>,
    events: Vec<&'static str>,
}

fn host(id: u64, type_name: &'static str) -> HostSpec {
    HostSpec {
        id,
        type_name,
        properties: Map::new(),
        events: Vec::new(),
    }
}

struct HostView {
    registry: Rc<RefCell<CustomElementRegistry>>,
    hosts: Vec<HostSpec>,
    emitted: Rc<RefCell<Vec<HostEvent>>>,
}

impl Render for HostView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = div().size_full().flex().flex_col();
        let mut elements = Vec::new();
        for spec in &self.hosts {
            let emitted = self.emitted.clone();
            let emit: EmitEvent = Rc::new(move |event, _, _| emitted.borrow_mut().push(event));
            let mut context = RenderContext::new(HostId(spec.id), emit);
            context.properties = Arc::new(spec.properties.clone());
            context.events = spec.events.iter().map(|event| (*event).into()).collect();
            elements.push((spec.type_name, context));
        }
        // Registry rendering needs the window and app context.
        root = root.child(HostChildren {
            registry: self.registry.clone(),
            hosts: RefCell::new(elements),
        });
        root
    }
}

/// Builds all host elements while the window and app contexts are available.
#[derive(IntoElement)]
struct HostChildren {
    registry: Rc<RefCell<CustomElementRegistry>>,
    hosts: RefCell<Vec<(&'static str, RenderContext)>>,
}

impl crate::RenderOnce for HostChildren {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let mut column = div().flex().flex_col();
        for (type_name, context) in self.hosts.into_inner() {
            let element = self
                .registry
                .borrow_mut()
                .render(type_name, context, window, cx);
            column = column.child(element);
        }
        column
    }
}

struct Harness {
    window: WindowHandle<HostView>,
    registry: Rc<RefCell<CustomElementRegistry>>,
    emitted: Rc<RefCell<Vec<HostEvent>>>,
}

fn harness(cx: &mut TestAppContext, registry: CustomElementRegistry) -> Harness {
    let registry = Rc::new(RefCell::new(registry));
    let emitted = Rc::new(RefCell::new(Vec::new()));
    let window = cx.open_window(size(px(300.), px(200.)), {
        let registry = registry.clone();
        let emitted = emitted.clone();
        move |_, _| HostView {
            registry,
            hosts: Vec::new(),
            emitted,
        }
    });
    Harness {
        window,
        registry,
        emitted,
    }
}

impl Harness {
    /// Replaces the hosts and paints a frame.
    fn set_hosts(&self, cx: &mut TestAppContext, hosts: Vec<HostSpec>) {
        self.window
            .update(cx, |view, _, cx| {
                view.hosts = hosts;
                cx.notify();
            })
            .unwrap();
        self.draw(cx);
    }

    fn draw(&self, cx: &mut TestAppContext) {
        cx.update_window(self.window.into(), |_, window, cx| {
            window.draw(cx).clear(cx)
        })
        .unwrap();
    }

    /// Paints another frame without changing anything.
    fn redraw(&self, cx: &mut TestAppContext) {
        self.window.update(cx, |_, _, cx| cx.notify()).unwrap();
        self.draw(cx);
    }

    fn prune(&self, cx: &mut TestAppContext, live: &[u64]) -> Vec<HostId> {
        let live = live.to_vec();
        self.window
            .update(cx, |_, window, cx| {
                self.registry
                    .borrow_mut()
                    .prune_missing(|id| live.contains(&id.0), window, cx)
            })
            .unwrap()
    }
}

const PROPS: &[&str] = &["color", "size"];
const ALL_EVENTS: &[&str] = &["click", "enter", "leave", "fileDrop"];

fn counting(
    name: &'static str,
    order: &OrderLog,
    properties: &'static [&'static str],
    events: &'static [&'static str],
) -> (CountingFactory, Rc<Probe>) {
    let probe = Probe::new(name, order);
    (
        CountingFactory {
            name,
            probe: probe.clone(),
            properties,
            events,
        },
        probe,
    )
}

fn registry_with(factories: Vec<CountingFactory>) -> CustomElementRegistry {
    let mut registry = CustomElementRegistry::empty();
    for factory in factories {
        registry.register(factory);
    }
    registry
}

// --- A1, A5: lifecycle ---------------------------------------------------------------

#[vespertine::test]
fn a1_one_construction_per_identity_and_one_destruction_on_replacement_and_deletion(
    cx: &mut TestAppContext,
) {
    let order: OrderLog = Rc::default();
    let (first, first_probe) = counting("first", &order, PROPS, ALL_EVENTS);
    let (second, second_probe) = counting("second", &order, PROPS, ALL_EVENTS);
    let h = harness(cx, registry_with(vec![first, second]));

    h.set_hosts(cx, vec![host(1, "first")]);
    h.redraw(cx);
    h.redraw(cx);
    assert_eq!(
        first_probe.created.get(),
        1,
        "constructed once across frames"
    );
    assert!(first_probe.renders.get() >= 3);
    assert_eq!(first_probe.destroyed.get(), 0);
    assert_eq!(h.registry.borrow().adapter_count(), 1);

    // Identity reuse with a different type destroys the previous adapter first (A5).
    h.set_hosts(cx, vec![host(1, "second")]);
    assert_eq!(
        first_probe.destroyed.get(),
        1,
        "destroyed once on type replacement"
    );
    assert_eq!(second_probe.created.get(), 1);
    let order = order.borrow();
    let destroy = order.iter().position(|e| e == "destroy:first:1").unwrap();
    let create = order.iter().position(|e| e == "create:second:1").unwrap();
    let first_render = order.iter().position(|e| e == "render:second:1").unwrap();
    assert!(destroy < create && create < first_render, "{order:?}");
    drop(order);
    assert_eq!(h.registry.borrow().adapter_type(HostId(1)), Some("second"));

    // Deleting the host and pruning destroys its adapter once.
    h.set_hosts(cx, vec![]);
    assert_eq!(h.prune(cx, &[]), vec![HostId(1)]);
    assert_eq!(
        second_probe.destroyed.get(),
        1,
        "destroyed once on deletion"
    );
    assert_eq!(h.prune(cx, &[]), Vec::<HostId>::new());
    assert_eq!(second_probe.destroyed.get(), 1);
    assert_eq!(h.registry.borrow().adapter_count(), 0);
}

#[vespertine::test]
fn registering_a_second_factory_replaces_the_first(cx: &mut TestAppContext) {
    let order: OrderLog = Rc::default();
    let (old, old_probe) = counting("kind", &order, PROPS, ALL_EVENTS);
    let (new, new_probe) = counting("kind", &order, PROPS, ALL_EVENTS);
    let h = harness(cx, registry_with(vec![old, new]));
    assert_eq!(
        h.registry.borrow().type_names(),
        vec![SharedString::from("kind")]
    );
    h.set_hosts(cx, vec![host(1, "kind")]);
    assert_eq!(old_probe.created.get(), 0);
    assert_eq!(new_probe.created.get(), 1);
}

// --- A2: property synchronization -----------------------------------------------------

fn with_props(mut spec: HostSpec, props: Value) -> HostSpec {
    spec.properties = props.as_object().cloned().unwrap();
    spec
}

#[vespertine::test]
fn a2_property_sync_resets_unchanged_and_unsupported(cx: &mut TestAppContext) {
    let order: OrderLog = Rc::default();
    let (factory, probe) = counting("kind", &order, PROPS, ALL_EVENTS);
    let h = harness(cx, registry_with(vec![factory]));
    let sets = || probe.sets.borrow().clone();

    let spec = |props: Value| vec![with_props(host(1, "kind"), props)];
    h.set_hosts(
        cx,
        spec(json!({"color": "red", "size": 3, "extra": {"k": [1, 2]}})),
    );
    assert_eq!(
        sets(),
        vec![
            ("color".to_owned(), json!("red")),
            ("size".to_owned(), json!(3)),
            ("extra".to_owned(), json!({"k": [1, 2]})),
        ],
        "supported first, unsupported forwarded once"
    );

    // Repeated identical frames: zero setter calls.
    h.redraw(cx);
    h.redraw(cx);
    h.set_hosts(
        cx,
        spec(json!({"color": "red", "size": 3, "extra": {"k": [1, 2]}})),
    );
    assert_eq!(sets().len(), 3);

    // Deep equality: 3.0 equals 3, nested value with equal contents is unchanged.
    h.set_hosts(
        cx,
        spec(json!({"color": "red", "size": 3.0, "extra": {"k": [1.0, 2]}})),
    );
    assert_eq!(sets().len(), 3);

    // Changing one property calls the setter for it only.
    h.set_hosts(
        cx,
        spec(json!({"color": "red", "size": 4, "extra": {"k": [1, 2]}})),
    );
    assert_eq!(sets().len(), 4);
    assert_eq!(sets()[3], ("size".to_owned(), json!(4)));

    // Removing a supported property resets it with null, once.
    h.set_hosts(cx, spec(json!({"size": 4, "extra": {"k": [1, 2]}})));
    assert_eq!(sets().len(), 5);
    assert_eq!(sets()[4], ("color".to_owned(), Value::Null));
    h.redraw(cx);
    assert_eq!(sets().len(), 5);

    // Removing an unsupported property resets it with null, once.
    h.set_hosts(cx, spec(json!({"size": 4})));
    assert_eq!(sets().len(), 6);
    assert_eq!(sets()[5], ("extra".to_owned(), Value::Null));
    h.redraw(cx);
    h.redraw(cx);
    assert_eq!(sets().len(), 6);

    // A changed unsupported property is forwarded again.
    h.set_hosts(cx, spec(json!({"size": 4, "extra": 1})));
    h.set_hosts(cx, spec(json!({"size": 4, "extra": 2})));
    assert_eq!(sets().len(), 8);
}

// --- A3: unknown type -------------------------------------------------------------------

#[vespertine::test]
fn a3_unknown_type_is_an_empty_element_with_a_diagnostic(cx: &mut TestAppContext) {
    let h = harness(cx, CustomElementRegistry::empty());
    h.set_hosts(cx, vec![host(9, "missing-widget")]);
    let registry = &mut *h.registry.borrow_mut();
    assert_eq!(registry.adapter_count(), 0);
    assert!(!registry.contains(HostId(9)));
    let diagnostics = registry.take_diagnostics();
    assert!(!diagnostics.is_empty());
    assert!(diagnostics.iter().all(|d| {
        d.level == HostDiagnosticLevel::Warning
            && d.host == HostId(9)
            && d.message.contains("missing-widget")
    }));
    assert!(registry.last_bounds(HostId(9)).is_none());
}

// --- A4: events -------------------------------------------------------------------------

fn with_events(mut spec: HostSpec, events: &[&'static str]) -> HostSpec {
    spec.events = events.to_vec();
    spec
}

fn click_at(
    window: &mut Window,
    cx: &mut App,
    position: Point<Pixels>,
    modifiers: Modifiers,
    count: usize,
) {
    window.dispatch_event(
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position,
            modifiers,
            click_count: count,
            first_mouse: false,
        }),
        cx,
    );
    window.dispatch_event(
        PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position,
            modifiers,
            click_count: count,
        }),
        cx,
    );
}

#[vespertine::test]
fn a4_undeclared_events_never_reach_the_adapter(cx: &mut TestAppContext) {
    let order: OrderLog = Rc::default();
    let (wide, wide_probe) = counting("wide", &order, PROPS, ALL_EVENTS);
    let (narrow, narrow_probe) = counting("narrow", &order, PROPS, &["click"]);
    let h = harness(cx, registry_with(vec![wide, narrow]));
    let subscribed = ["click", "enter", "fileDrop", "keyDown", "customThing"];
    h.set_hosts(
        cx,
        vec![
            with_events(host(1, "wide"), &subscribed),
            with_events(host(2, "narrow"), &subscribed),
        ],
    );
    assert_eq!(
        wide_probe.events_seen.borrow().last().unwrap(),
        &["click", "enter", "fileDrop"],
        "declared and subscribed only (no leave: not subscribed; no keyDown: not declared)"
    );
    assert_eq!(
        narrow_probe.events_seen.borrow().last().unwrap(),
        &["click"]
    );
}

#[vespertine::test]
fn a4_declared_standard_events_fire(cx: &mut TestAppContext) {
    let order: OrderLog = Rc::default();
    let (factory, _probe) = counting("kind", &order, PROPS, ALL_EVENTS);
    let h = harness(cx, registry_with(vec![factory]));
    h.set_hosts(cx, vec![with_events(host(1, "kind"), ALL_EVENTS)]);
    let inside = point(px(10.), px(10.));
    let outside = point(px(200.), px(150.));
    let shift = Modifiers {
        shift: true,
        ..Default::default()
    };

    let emitted = h.emitted.clone();
    cx.update_window(h.window.into(), |_, window, cx| {
        // Click: primary button release with position, button 0, count and modifiers.
        click_at(window, cx, inside, shift, 2);
        let events = emitted.borrow().clone();
        assert!(
            events.contains(&HostEvent::Click(HostClickEvent {
                position: inside,
                button: 0,
                click_count: 2,
                modifiers: shift,
            })),
            "{events:?}"
        );

        // A secondary button release is not a standard click.
        let before = emitted.borrow().len();
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Right,
                position: inside,
                modifiers: Modifiers::default(),
                click_count: 1,
            }),
            cx,
        );
        assert!(
            emitted.borrow()[before..]
                .iter()
                .all(|event| !matches!(event, HostEvent::Click(_)))
        );

        // Hover enter and leave.
        window.draw(cx).clear(cx);
        window.simulate_mouse_move(outside, cx);
        emitted.borrow_mut().clear();
        window.simulate_mouse_move(inside, cx);
        assert!(
            matches!(emitted.borrow().as_slice(), [HostEvent::Enter(e)] if e.position == inside)
        );
        window.simulate_mouse_move(outside, cx);
        assert!(matches!(
            emitted.borrow().as_slice(),
            [HostEvent::Enter(_), HostEvent::Leave(e)] if e.position == outside
        ));

        // File drop with absolute UTF-8 paths and the drop position.
        emitted.borrow_mut().clear();
        let paths = ExternalPaths(
            [PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/dir/b.png")]
                .into_iter()
                .collect(),
        );
        drop_files(window, cx, inside, paths);
        let drops: Vec<_> = emitted
            .borrow()
            .iter()
            .filter_map(|event| match event {
                HostEvent::FileDrop(drop) => Some(drop.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            drops,
            vec![HostFileDropEvent {
                paths: vec!["/tmp/a.txt".to_owned(), "/tmp/dir/b.png".to_owned()],
                position: inside,
            }]
        );

        // No event for an empty drop or one containing a non-Unicode path.
        emitted.borrow_mut().clear();
        drop_files(window, cx, inside, ExternalPaths::default());
        #[cfg(unix)]
        {
            use std::{ffi::OsString, os::unix::ffi::OsStringExt};
            let bad = PathBuf::from(OsString::from_vec(vec![b'/', b'x', 0xff]));
            let mixed = ExternalPaths([PathBuf::from("/ok"), bad].into_iter().collect());
            drop_files(window, cx, inside, mixed);
        }
        assert!(
            emitted
                .borrow()
                .iter()
                .all(|event| !matches!(event, HostEvent::FileDrop(_))),
            "{:?}",
            emitted.borrow()
        );
    })
    .unwrap();
}

fn drop_files(window: &mut Window, cx: &mut App, position: Point<Pixels>, paths: ExternalPaths) {
    window.draw(cx).clear(cx);
    window.dispatch_event(
        FileDropEvent::Entered { position, paths }.to_platform_input(),
        cx,
    );
    window.dispatch_event(FileDropEvent::Submit { position }.to_platform_input(), cx);
    window.dispatch_event(FileDropEvent::Exited.to_platform_input(), cx);
}

#[vespertine::test]
fn a4_events_without_a_subscription_get_no_handlers(cx: &mut TestAppContext) {
    let order: OrderLog = Rc::default();
    let (factory, _probe) = counting("kind", &order, PROPS, ALL_EVENTS);
    let h = harness(cx, registry_with(vec![factory]));
    h.set_hosts(cx, vec![with_events(host(1, "kind"), &["enter"])]);
    let emitted = h.emitted.clone();
    cx.update_window(h.window.into(), |_, window, cx| {
        click_at(window, cx, point(px(10.), px(10.)), Modifiers::default(), 1);
        window.draw(cx).clear(cx);
        window.simulate_mouse_move(point(px(10.), px(10.)), cx);
        window.simulate_mouse_move(point(px(250.), px(150.)), cx);
    })
    .unwrap();
    // Only the subscribed edge is delivered.
    assert!(matches!(emitted.borrow().as_slice(), [HostEvent::Enter(_)]));
}

// --- A6: pruning and image release -------------------------------------------------------

fn test_image() -> Arc<RenderImage> {
    let frame = Frame::new(ImageBuffer::from_pixel(2, 2, Rgba([255, 0, 0, 255])));
    Arc::new(RenderImage::new(SmallVec::from_elem(frame, 1)))
}

#[vespertine::test]
fn a6_pruning_destroys_adapters_of_missing_hosts(cx: &mut TestAppContext) {
    let order: OrderLog = Rc::default();
    let (factory, probe) = counting("kind", &order, PROPS, ALL_EVENTS);
    let h = harness(cx, registry_with(vec![factory]));
    h.set_hosts(cx, vec![host(1, "kind"), host(2, "kind"), host(3, "kind")]);
    assert_eq!(h.registry.borrow().adapter_count(), 3);
    assert_eq!(h.prune(cx, &[2]), vec![HostId(1), HostId(3)]);
    assert_eq!(probe.destroyed.get(), 2);
    assert!(h.registry.borrow().contains(HostId(2)));
    assert!(!h.registry.borrow().contains(HostId(1)));
}

#[vespertine::test]
fn a6_live_and_replaced_images_are_released_from_the_window_cache(cx: &mut TestAppContext) {
    let order: OrderLog = Rc::default();
    let (factory, probe) = counting("img", &order, PROPS, ALL_EVENTS);
    let h = harness(cx, registry_with(vec![factory]));
    let (first, second) = (test_image(), test_image());

    // The live-image entry point creates the image adapter on demand.
    let previous = h
        .window
        .update(cx, |_, window, cx| {
            h.registry
                .borrow_mut()
                .set_live_image(HostId(5), first.clone(), window, cx)
        })
        .unwrap()
        .unwrap();
    assert!(previous.is_none());
    assert_eq!(probe.created.get(), 1);
    h.set_hosts(cx, vec![host(5, "img")]);
    let has = |image: &Arc<RenderImage>, cx: &mut TestAppContext| {
        h.window
            .update(cx, |_, window, _| window.has_image_atlas_entry(image))
            .unwrap()
    };
    assert!(has(&first, cx), "live image is uploaded after painting");

    // Replacing returns the previous image; both stay cached until the host goes away.
    let previous = h
        .window
        .update(cx, |_, window, cx| {
            h.registry
                .borrow_mut()
                .set_live_image(HostId(5), second.clone(), window, cx)
        })
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&previous.unwrap(), &first));
    h.redraw(cx);
    assert!(has(&first, cx) && has(&second, cx));

    // Removing the host releases the live image and the replaced image not yet released.
    h.set_hosts(cx, vec![]);
    h.prune(cx, &[]);
    assert!(!has(&first, cx), "replaced image released");
    assert!(!has(&second, cx), "live image released");
}

#[vespertine::test]
fn a6_set_live_image_without_an_image_factory_fails_descriptively(cx: &mut TestAppContext) {
    let h = harness(cx, CustomElementRegistry::empty());
    let error = h
        .window
        .update(cx, |_, window, cx| {
            h.registry
                .borrow_mut()
                .set_live_image(HostId(1), test_image(), window, cx)
        })
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("image"), "{error}");
    assert_eq!(h.registry.borrow().adapter_count(), 0);
}

#[vespertine::test]
fn a6_application_quit_destroys_every_adapter_while_the_app_context_exists(
    cx: &mut TestAppContext,
) {
    let order: OrderLog = Rc::default();
    let (factory, probe) = counting("kind", &order, PROPS, ALL_EVENTS);
    let h = harness(cx, registry_with(vec![factory]));
    h.set_hosts(cx, vec![host(1, "kind"), host(2, "kind")]);
    let weak: Vec<_> = probe.entities.borrow().clone();
    let _quit = cx.update(|cx| CustomElementRegistry::destroy_on_quit(&h.registry, cx));

    cx.update(|cx| cx.shutdown());
    assert_eq!(probe.destroyed.get(), 2);
    assert_eq!(h.registry.borrow().adapter_count(), 0);
    assert!(weak.iter().all(|entity| entity.upgrade().is_none()));
}

#[vespertine::test]
fn a6_destroy_all_leaves_no_adapter_holding_an_entity(cx: &mut TestAppContext) {
    let order: OrderLog = Rc::default();
    let (factory, probe) = counting("kind", &order, PROPS, ALL_EVENTS);
    let h = harness(cx, registry_with(vec![factory]));
    h.set_hosts(cx, vec![host(1, "kind"), host(2, "kind")]);
    let weak: Vec<_> = probe.entities.borrow().clone();
    assert_eq!(weak.len(), 2);
    assert!(weak.iter().all(|entity| entity.upgrade().is_some()));

    cx.update(|cx| h.registry.borrow_mut().destroy_all(None, cx));
    cx.run_until_parked();
    assert_eq!(probe.destroyed.get(), 2);
    assert_eq!(h.registry.borrow().adapter_count(), 0);
    assert!(
        weak.iter().all(|entity| entity.upgrade().is_none()),
        "no adapter holds an entity handle after destroy-all"
    );
}

// --- A7: hosted element bounds ------------------------------------------------------------
//
// Selectable text runs and chrome text exclusion are verified against the real TXT-008 selection
// registry in `vespertine_richtext` (tests/host_text.rs).

#[vespertine::test]
fn a7_hosted_element_reports_last_painted_bounds(cx: &mut TestAppContext) {
    let order: OrderLog = Rc::default();
    let (factory, _probe) = counting("kind", &order, PROPS, ALL_EVENTS);
    let h = harness(cx, registry_with(vec![factory]));
    assert_eq!(h.registry.borrow().last_bounds(HostId(7)), None);
    h.set_hosts(cx, vec![host(7, "kind")]);
    let bounds = h.registry.borrow().last_bounds(HostId(7)).expect("painted");
    assert_eq!(
        bounds,
        Bounds::new(point(px(0.), px(0.)), size(px(40.), px(20.)))
    );
    h.set_hosts(cx, vec![]);
    h.prune(cx, &[]);
    assert_eq!(h.registry.borrow().last_bounds(HostId(7)), None);
}

#[vespertine::test]
fn text_without_a_provider_is_plain_and_with_a_provider_goes_through_it(cx: &mut TestAppContext) {
    use crate::{AnyElement, App, HostTextProvider, IntoElement as _, SharedString, TextRun};
    struct Recording(RefCell<Vec<String>>);
    impl HostTextProvider for Recording {
        fn selectable_run(
            &self,
            host: HostId,
            sub_index: u32,
            selectable: bool,
            text: SharedString,
            _runs: Option<Vec<TextRun>>,
        ) -> AnyElement {
            self.0
                .borrow_mut()
                .push(format!("sel {host} {sub_index} {selectable} {text}"));
            div().into_any_element()
        }
        fn chrome_run(
            &self,
            host: HostId,
            sub_index: u32,
            text: SharedString,
            _runs: Option<Vec<TextRun>>,
        ) -> AnyElement {
            self.0
                .borrow_mut()
                .push(format!("chrome {host} {sub_index} {text}"));
            div().into_any_element()
        }
        fn selected_text(&self) -> Option<String> {
            None
        }
        fn clear_selection(&self, _: &mut Window, _: &mut App) {}
    }
    let _ = cx;
    let emit: EmitEvent = Rc::new(|_, _, _| {});
    let mut context = RenderContext::new(HostId(3), emit);
    let provider = Rc::new(Recording(RefCell::default()));
    context.text = Some(provider.clone());
    context.selectable_text(2, "abc", None);
    context.selectable = false;
    context.selectable_text(3, "def", None);
    context.chrome_text(0, "1", None);
    assert_eq!(
        *provider.0.borrow(),
        ["sel 3 2 true abc", "sel 3 3 false def", "chrome 3 0 1"]
    );
}
