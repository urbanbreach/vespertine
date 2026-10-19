//! Acceptance tests for APP-002: reactive views and owned background work.

#[path = "../examples/example_support/reactive_background.rs"]
mod reactive_background;

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use reactive_background::ReactiveDemo;
use vespertine::{
    App, Asset, AssetSource, Context, Empty, Entity, EventEmitter, Global, IntoElement,
    NoopTextSystem, Render, SharedString, StyleRefinement, Task, TestApp, TestAppContext, Window,
    WindowHandle, div, point, prelude::*, px,
};

// ---------------------------------------------------------------------------------------------
// A1: a background result is published while an independent counter keeps updating.
// ---------------------------------------------------------------------------------------------

#[vespertine::test]
fn app_002_a1_input_counter_is_never_delayed_by_background_work(cx: &mut TestAppContext) {
    cx.update(reactive_background::bind_keys);
    let delay = Duration::from_secs(3);
    let (demo, cx) = cx.add_window_view(|window, cx| ReactiveDemo::new(delay, 42, window, cx));
    let (counter, result) =
        demo.read_with(cx, |demo, _| (demo.counter.clone(), demo.result.clone()));
    cx.run_until_parked();

    // The timer is pending on the background executor and has not elapsed.
    assert_eq!(result.read_with(cx, |result, _| result.value), None);

    // Keystrokes and clicks update the counter at once, while the result is still pending.
    for expected in 1..=3 {
        cx.simulate_keystrokes("space");
        assert_eq!(counter.read_with(cx, |counter, _| counter.count), expected);
        assert_eq!(result.read_with(cx, |result, _| result.value), None);
    }
    cx.simulate_click(point(px(20.), px(20.)), Default::default());
    assert_eq!(counter.read_with(cx, |counter, _| counter.count), 4);
    assert_eq!(result.read_with(cx, |result, _| result.value), None);

    // Half of the wait elapses: still pending, input still immediate.
    cx.executor().advance_clock(delay / 2);
    cx.run_until_parked();
    cx.simulate_keystrokes("space");
    assert_eq!(counter.read_with(cx, |counter, _| counter.count), 5);
    assert_eq!(result.read_with(cx, |result, _| result.value), None);

    // Once the timer elapses the value is published to the result view only.
    cx.executor().advance_clock(delay);
    cx.run_until_parked();
    assert_eq!(result.read_with(cx, |result, _| result.value), Some(42));
    assert_eq!(counter.read_with(cx, |counter, _| counter.count), 5);

    cx.simulate_keystrokes("space");
    assert_eq!(counter.read_with(cx, |counter, _| counter.count), 6);
}

// ---------------------------------------------------------------------------------------------
// A2: a result arriving after its owner was destroyed is discarded.
// ---------------------------------------------------------------------------------------------

struct Owner {
    value: Option<u32>,
    task: Option<Task<()>>,
}

impl Render for Owner {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

#[derive(Default)]
struct Outcome {
    delivered: Cell<bool>,
    update_failed: Cell<bool>,
    update_ran: Cell<bool>,
    owner_released: Cell<bool>,
}

fn start_in_flight(owner: &Entity<Owner>, detach: bool, outcome: &Rc<Outcome>, cx: &mut App) {
    owner.update(cx, |this, cx| {
        let executor = cx.background_executor().clone();
        let background = cx.background_spawn(async move {
            executor.timer(Duration::from_secs(1)).await;
            7u32
        });
        let outcome = outcome.clone();
        let task = cx.spawn(async move |this, cx| {
            let value = background.await;
            outcome.delivered.set(true);
            match this.update(cx, |this, _| {
                this.value = Some(value);
            }) {
                Ok(()) => outcome.update_ran.set(true),
                Err(_) => outcome.update_failed.set(true),
            }
        });
        if detach {
            task.detach();
        } else {
            this.task = Some(task);
        }
    });
}

#[vespertine::test]
fn app_002_a2_owner_destroyed_before_result_cancels_the_owned_task(cx: &mut TestAppContext) {
    let outcome = Rc::new(Outcome::default());
    let owner = cx.new(|_| Owner {
        value: None,
        task: None,
    });
    let weak = owner.downgrade();
    cx.update(|cx| start_in_flight(&owner, false, &outcome, cx));
    cx.run_until_parked();
    assert!(!outcome.delivered.get());

    // Dropping the last strong handle releases the owner and the task it owns.
    drop(owner);
    // Dropping the last handle releases the entity (and the task it owns) at the next flush.
    cx.update(|_| {});
    cx.run_until_parked();
    assert!(weak.upgrade().is_none());

    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    assert!(
        !outcome.delivered.get(),
        "the owned task is cancelled with its owner"
    );
    assert!(!outcome.update_ran.get());
}

#[vespertine::test]
fn app_002_a2_late_result_for_detached_task_fails_to_update_released_entity(
    cx: &mut TestAppContext,
) {
    let outcome = Rc::new(Outcome::default());
    let owner = cx.new(|_| Owner {
        value: None,
        task: None,
    });
    let weak = owner.downgrade();
    cx.update(|cx| {
        let released = outcome.clone();
        cx.observe_release(&owner, move |_, _| released.owner_released.set(true))
            .detach();
        start_in_flight(&owner, true, &outcome, cx);
    });
    cx.run_until_parked();
    assert!(!outcome.delivered.get());

    drop(owner);
    cx.update(|_| {});
    cx.run_until_parked();
    assert!(outcome.owner_released.get());
    assert!(weak.upgrade().is_none());

    // The detached task still runs to completion, but its weak update reports an error and
    // touches nothing.
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    assert!(outcome.delivered.get());
    assert!(outcome.update_failed.get());
    assert!(!outcome.update_ran.get());
    assert!(weak.upgrade().is_none());
    cx.update(|cx| assert!(weak.read_with(cx, |_, _| ()).is_err()));
}

#[vespertine::test]
fn app_002_a2_view_removed_with_window_discards_late_result(cx: &mut TestAppContext) {
    let outcome = Rc::new(Outcome::default());
    let window: WindowHandle<Owner> = cx.add_window(|_, _| Owner {
        value: None,
        task: None,
    });
    let owner = window.root(cx).unwrap();
    let weak = owner.downgrade();
    cx.update(|cx| start_in_flight(&owner, false, &outcome, cx));
    drop(owner);
    cx.run_until_parked();
    assert!(!outcome.delivered.get());

    cx.update(|cx| {
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
    });
    cx.run_until_parked();
    assert!(weak.upgrade().is_none());

    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    assert!(!outcome.delivered.get());
    assert!(!outcome.update_ran.get());
}

// ---------------------------------------------------------------------------------------------
// A3: application-wide refresh re-renders every window once, bypassing retained paint state.
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
struct Palette(u32);

impl Global for Palette {}

struct PaletteLeaf {
    renders: Rc<Cell<usize>>,
    seen: Rc<Cell<u32>>,
}

impl Render for PaletteLeaf {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        self.seen.set(cx.global::<Palette>().0);
        div().size_full()
    }
}

struct PaletteRoot {
    leaf: Entity<PaletteLeaf>,
    renders: Rc<Cell<usize>>,
}

impl Render for PaletteRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div().size_full().child(
            self.leaf
                .clone()
                .cached(StyleRefinement::default().w(px(100.)).h(px(100.))),
        )
    }
}

struct PaletteWindow {
    root_renders: Rc<Cell<usize>>,
    leaf_renders: Rc<Cell<usize>>,
    leaf_seen: Rc<Cell<u32>>,
    leaf: Entity<PaletteLeaf>,
}

fn open_palette_window(cx: &mut TestAppContext) -> PaletteWindow {
    let root_renders = Rc::new(Cell::new(0));
    let leaf_renders = Rc::new(Cell::new(0));
    let leaf_seen = Rc::new(Cell::new(0));
    let leaf_slot = Rc::new(RefCell::new(None));
    let window = cx.add_window({
        let (root_renders, leaf_renders, leaf_seen, leaf_slot) = (
            root_renders.clone(),
            leaf_renders.clone(),
            leaf_seen.clone(),
            leaf_slot.clone(),
        );
        move |_, cx| {
            let leaf = cx.new(|_| PaletteLeaf {
                renders: leaf_renders,
                seen: leaf_seen,
            });
            *leaf_slot.borrow_mut() = Some(leaf.clone());
            PaletteRoot {
                leaf,
                renders: root_renders,
            }
        }
    });
    let _ = window;
    cx.run_until_parked();
    let leaf = leaf_slot.borrow_mut().take().unwrap();
    PaletteWindow {
        root_renders,
        leaf_renders,
        leaf_seen,
        leaf,
    }
}

#[vespertine::test]
fn app_002_a3_refresh_windows_rerenders_every_window_once_including_cached_views(
    cx: &mut TestAppContext,
) {
    cx.set_global(Palette(1));
    let first = open_palette_window(cx);
    let second = open_palette_window(cx);
    for window in [&first, &second] {
        assert_eq!(window.root_renders.get(), 1);
        assert_eq!(window.leaf_renders.get(), 1);
        assert_eq!(window.leaf_seen.get(), 1);
    }

    // A global change alone is not entity data: nothing is invalidated until the refresh.
    cx.update(|cx| cx.set_global(Palette(2)));
    cx.run_until_parked();
    for window in [&first, &second] {
        assert_eq!(window.leaf_renders.get(), 1);
        assert_eq!(window.leaf_seen.get(), 1);
    }

    // Any number of refresh requests in one update cycle coalesce into one redraw per window,
    // and the cached leaf whose tracked entities did not change is re-rendered in each.
    cx.update(|cx| {
        cx.refresh_windows();
        cx.refresh_windows();
        cx.refresh_windows();
    });
    cx.run_until_parked();
    for window in [&first, &second] {
        assert_eq!(window.root_renders.get(), 2);
        assert_eq!(window.leaf_renders.get(), 2);
        assert_eq!(window.leaf_seen.get(), 2);
    }

    // The refreshing flag is cleared after the frame: a later frame reuses the cached leaf.
    cx.update(|cx| {
        let other = cx.new(|_| ());
        cx.notify(other.entity_id());
        for window in cx.windows() {
            window.update(cx, |_, window, _| window.refresh()).ok();
        }
    });
    cx.run_until_parked();
    for window in [&first, &second] {
        assert_eq!(
            window.leaf_renders.get(),
            3,
            "explicit window refresh bypasses retention"
        );
    }

    cx.update(|cx| {
        let entity = first.leaf.clone();
        cx.notify(entity.entity_id());
    });
    cx.run_until_parked();
    assert_eq!(
        first.leaf_renders.get(),
        4,
        "a notified cached view re-renders"
    );
    assert_eq!(second.leaf_renders.get(), 3);
    assert_eq!(first.root_renders.get(), 4);
}

// ---------------------------------------------------------------------------------------------
// A4: notify invalidates only readers; typed events reach only matching subscribers.
// ---------------------------------------------------------------------------------------------

struct Model {
    value: u32,
}

struct ModelReader {
    model: Entity<Model>,
    renders: Rc<Cell<usize>>,
    seen: Rc<Cell<u32>>,
}

impl Render for ModelReader {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        self.seen.set(self.model.read(cx).value);
        div().size_full()
    }
}

struct TwoReaders {
    a: Entity<ModelReader>,
    b: Entity<ModelReader>,
}

impl Render for TwoReaders {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let style = || StyleRefinement::default().w(px(50.)).h(px(50.));
        div()
            .size_full()
            .child(self.a.clone().cached(style()))
            .child(self.b.clone().cached(style()))
    }
}

struct Reader {
    renders: Rc<Cell<usize>>,
    seen: Rc<Cell<u32>>,
}

impl Reader {
    fn new() -> Self {
        Self {
            renders: Rc::new(Cell::new(0)),
            seen: Rc::new(Cell::new(0)),
        }
    }
}

fn open_two_readers(
    cx: &mut TestAppContext,
    model_a: &Entity<Model>,
    model_b: &Entity<Model>,
    a: &Reader,
    b: &Reader,
) {
    let (model_a, model_b) = (model_a.clone(), model_b.clone());
    let (ar, a_seen, br, b_seen) = (
        a.renders.clone(),
        a.seen.clone(),
        b.renders.clone(),
        b.seen.clone(),
    );
    cx.add_window(move |_, cx| TwoReaders {
        a: cx.new(|_| ModelReader {
            model: model_a,
            renders: ar,
            seen: a_seen,
        }),
        b: cx.new(|_| ModelReader {
            model: model_b,
            renders: br,
            seen: b_seen,
        }),
    });
    cx.run_until_parked();
}

#[vespertine::test]
fn app_002_a4_notify_rerenders_only_views_that_read_the_entity(cx: &mut TestAppContext) {
    let model_a = cx.new(|_| Model { value: 1 });
    let model_b = cx.new(|_| Model { value: 10 });

    // Two windows each display one view of model A and one of model B.
    let (a1, b1, a2, b2) = (Reader::new(), Reader::new(), Reader::new(), Reader::new());
    open_two_readers(cx, &model_a, &model_b, &a1, &b1);
    open_two_readers(cx, &model_a, &model_b, &a2, &b2);
    for reader in [&a1, &b1, &a2, &b2] {
        assert_eq!(reader.renders.get(), 1);
    }

    cx.update(|cx| {
        model_a.update(cx, |model, cx| {
            model.value = 2;
            cx.notify();
        })
    });
    cx.run_until_parked();

    // Views of model A re-render in every window; views of model B do not.
    assert_eq!((a1.renders.get(), a1.seen.get()), (2, 2));
    assert_eq!((a2.renders.get(), a2.seen.get()), (2, 2));
    assert_eq!((b1.renders.get(), b1.seen.get()), (1, 10));
    assert_eq!((b2.renders.get(), b2.seen.get()), (1, 10));

    // Mutating without notifying invalidates nothing.
    cx.update(|cx| model_b.update(cx, |model, _| model.value = 11));
    cx.run_until_parked();
    assert_eq!(b1.renders.get(), 1);

    cx.update(|cx| model_b.update(cx, |_, cx| cx.notify()));
    cx.run_until_parked();
    assert_eq!((b1.renders.get(), b1.seen.get()), (2, 11));
    assert_eq!((b2.renders.get(), b2.seen.get()), (2, 11));
    assert_eq!(a1.renders.get(), 2);
    assert_eq!(a2.renders.get(), 2);
}

#[derive(Clone, Debug, PartialEq)]
struct Ping(u32);

#[derive(Clone, Debug, PartialEq)]
struct Pong(u32);

struct Emitter;

impl EventEmitter<Ping> for Emitter {}
impl EventEmitter<Pong> for Emitter {}

#[derive(Default)]
struct Subscriber {
    pings: Vec<u32>,
    pongs: Vec<u32>,
}

#[vespertine::test]
fn app_002_a4_typed_events_reach_only_matching_emitter_and_type(cx: &mut TestAppContext) {
    let first = cx.new(|_| Emitter);
    let second = cx.new(|_| Emitter);
    let on_first_pings = cx.new(|_| Subscriber::default());
    let on_first_pongs = cx.new(|_| Subscriber::default());
    let on_second_pings = cx.new(|_| Subscriber::default());

    cx.update(|cx| {
        cx.subscribe(&first, {
            let subscriber = on_first_pings.clone();
            move |_emitter, event: &Ping, cx| {
                subscriber.update(cx, |subscriber, _| subscriber.pings.push(event.0));
            }
        })
        .detach();
        cx.subscribe(&first, {
            let subscriber = on_first_pongs.clone();
            move |_emitter, event: &Pong, cx| {
                subscriber.update(cx, |subscriber, _| subscriber.pongs.push(event.0));
            }
        })
        .detach();
        cx.subscribe(&second, {
            let subscriber = on_second_pings.clone();
            move |_emitter, event: &Ping, cx| {
                subscriber.update(cx, |subscriber, _| subscriber.pings.push(event.0));
            }
        })
        .detach();
    });

    cx.update(|cx| {
        first.update(cx, |_, cx| {
            cx.emit(Ping(1));
            cx.emit(Pong(2));
            cx.emit(Ping(3));
        });
        second.update(cx, |_, cx| cx.emit(Ping(4)));
        second.update(cx, |_, cx| cx.emit(Pong(5)));
    });

    cx.read(|cx| {
        assert_eq!(on_first_pings.read(cx).pings, vec![1, 3]);
        assert!(on_first_pings.read(cx).pongs.is_empty());
        assert_eq!(on_first_pongs.read(cx).pongs, vec![2]);
        assert!(on_first_pongs.read(cx).pings.is_empty());
        assert_eq!(on_second_pings.read(cx).pings, vec![4]);
        assert!(on_second_pings.read(cx).pongs.is_empty());
    });
}

struct StatefulSubscriber {
    received: Vec<u32>,
}

#[vespertine::test]
fn app_002_a4_subscriber_callback_has_access_to_its_own_state(cx: &mut TestAppContext) {
    let emitter = cx.new(|_| Emitter);
    let subscriber = cx.new(|_| StatefulSubscriber { received: vec![] });
    let _owned = subscriber.update(cx, |_, cx| {
        let emitter = emitter.clone();
        cx.subscribe(&emitter, |this, _, event: &Ping, _| {
            this.received.push(event.0 * 10);
        })
    });
    cx.update(|cx| emitter.update(cx, |_, cx| cx.emit(Ping(1))));
    cx.update(|cx| emitter.update(cx, |_, cx| cx.emit(Ping(2))));
    assert_eq!(
        subscriber.read_with(cx, |s, _| s.received.clone()),
        vec![10, 20]
    );
}

// ---------------------------------------------------------------------------------------------
// A5: dropping a subscription stops callbacks; deferred work runs after the scheduling update.
// ---------------------------------------------------------------------------------------------

#[vespertine::test]
fn app_002_a5_dropping_subscriptions_stops_callbacks(cx: &mut TestAppContext) {
    let emitter = cx.new(|_| Emitter);
    let model = cx.new(|_| Model { value: 0 });
    let events = Rc::new(Cell::new(0));
    let notifications = Rc::new(Cell::new(0));
    let globals = Rc::new(Cell::new(0));

    let (event_subscription, observe_subscription, global_subscription) = cx.update(|cx| {
        let events = events.clone();
        let event_subscription = cx.subscribe(&emitter, move |_, _: &Ping, _| {
            events.set(events.get() + 1);
        });
        let notifications = notifications.clone();
        let observe_subscription = cx.observe(&model, move |_, _| {
            notifications.set(notifications.get() + 1);
        });
        let globals = globals.clone();
        let global_subscription = cx.observe_global::<Palette>(move |_| {
            globals.set(globals.get() + 1);
        });
        (
            event_subscription,
            observe_subscription,
            global_subscription,
        )
    });

    let fire = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            emitter.update(cx, |_, cx| cx.emit(Ping(0)));
            model.update(cx, |_, cx| cx.notify());
            cx.set_global(Palette(0));
        });
    };
    fire(cx);
    assert_eq!(
        (events.get(), notifications.get(), globals.get()),
        (1, 1, 1)
    );

    drop(event_subscription);
    fire(cx);
    assert_eq!(
        (events.get(), notifications.get(), globals.get()),
        (1, 2, 2)
    );

    drop(observe_subscription);
    fire(cx);
    assert_eq!(
        (events.get(), notifications.get(), globals.get()),
        (1, 2, 3)
    );

    drop(global_subscription);
    fire(cx);
    assert_eq!(
        (events.get(), notifications.get(), globals.get()),
        (1, 2, 3)
    );
}

#[vespertine::test]
fn app_002_a5_releasing_a_view_stops_its_own_subscriptions(cx: &mut TestAppContext) {
    let emitter = cx.new(|_| Emitter);
    let subscriber = cx.new(|_| StatefulSubscriber { received: vec![] });
    let weak = subscriber.downgrade();
    let calls = Rc::new(Cell::new(0));
    subscriber.update(cx, |_, cx| {
        let calls = calls.clone();
        cx.subscribe(&emitter, move |this, _, event: &Ping, _| {
            calls.set(calls.get() + 1);
            this.received.push(event.0);
        })
        .detach();
    });
    cx.update(|cx| emitter.update(cx, |_, cx| cx.emit(Ping(1))));
    assert_eq!(calls.get(), 1);

    drop(subscriber);
    cx.run_until_parked();
    assert!(weak.upgrade().is_none());
    cx.update(|cx| emitter.update(cx, |_, cx| cx.emit(Ping(2))));
    assert_eq!(calls.get(), 1, "no callback reaches a released subscriber");
}

#[vespertine::test]
fn app_002_a5_deferred_work_runs_after_the_scheduling_update(cx: &mut TestAppContext) {
    let model = cx.new(|_| Model { value: 0 });
    let log = Rc::new(RefCell::new(Vec::<&'static str>::new()));

    cx.update(|cx| {
        model.update(cx, |model, cx| {
            let log_outer = log.clone();
            let model_handle = cx.entity();
            cx.defer(move |cx| {
                // The entity that scheduled the work has been returned to the app by now.
                log_outer.borrow_mut().push("deferred");
                assert_eq!(model_handle.read(cx).value, 5);
            });
            log.borrow_mut().push("update body");
            model.value = 5;
        });
        log.borrow_mut().push("after update call");
    });
    assert_eq!(
        *log.borrow(),
        vec!["update body", "after update call", "deferred"]
    );
}

struct ActionHost {
    focus: vespertine::FocusHandle,
    log: Rc<RefCell<Vec<&'static str>>>,
}

vespertine::actions!(app_002_tests, [ScheduleDeferred]);

impl Render for ActionHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = window;
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ScheduleDeferred, window, cx| {
                this.log.borrow_mut().push("handler start");
                let log = this.log.clone();
                window.defer(cx, move |_window, _cx| log.borrow_mut().push("deferred"));
                this.log.borrow_mut().push("handler end");
            }))
    }
}

#[vespertine::test]
fn app_002_a5_work_deferred_during_an_action_runs_after_the_action(cx: &mut TestAppContext) {
    let log = Rc::new(RefCell::new(Vec::new()));
    let (host, cx) = cx.add_window_view({
        let log = log.clone();
        move |window, cx| {
            let focus = cx.focus_handle();
            focus.focus(window, cx);
            ActionHost { focus, log }
        }
    });
    let _ = host;
    cx.dispatch_action(ScheduleDeferred);
    cx.run_until_parked();
    assert_eq!(
        *log.borrow(),
        vec!["handler start", "handler end", "deferred"]
    );
}

// ---------------------------------------------------------------------------------------------
// A6: embedded asset source, custom async loader (and, in vespertine_tokio, the executor bridge).
// ---------------------------------------------------------------------------------------------

struct CountingAssets {
    loads: Arc<AtomicUsize>,
}

impl AssetSource for CountingAssets {
    fn load(&self, path: &str) -> anyhow::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        Ok(match path {
            "greeting.txt" => Some(std::borrow::Cow::Borrowed(b"hello")),
            _ => None,
        })
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(match path {
            "" => vec!["greeting.txt".into()],
            _ => vec![],
        })
    }
}

static DOUBLER_LOADS: AtomicUsize = AtomicUsize::new(0);

struct Doubler;

impl Asset for Doubler {
    type Source = u32;
    type Output = u32;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        DOUBLER_LOADS.fetch_add(1, Ordering::SeqCst);
        let executor = cx.background_executor().clone();
        async move {
            executor.timer(Duration::from_millis(10)).await;
            source * 2
        }
    }
}

#[test]
fn app_002_a6_embedded_asset_source_and_custom_async_loader_load_once_each() {
    let source_loads = Arc::new(AtomicUsize::new(0));
    let mut app = TestApp::with_text_system_and_assets(
        Arc::new(NoopTextSystem),
        Arc::new(CountingAssets {
            loads: source_loads.clone(),
        }),
    );

    // The embedded asset source supplied at construction answers load-by-path and list-by-path.
    let (greeting, missing, listing) = app.read(|cx| {
        (
            cx.asset_source().load("greeting.txt").unwrap(),
            cx.asset_source().load("missing.txt").unwrap(),
            cx.asset_source().list("").unwrap(),
        )
    });
    assert_eq!(greeting.as_deref(), Some(&b"hello"[..]));
    assert!(missing.is_none());
    assert_eq!(listing, vec![SharedString::from("greeting.txt")]);
    assert_eq!(source_loads.load(Ordering::SeqCst), 2);

    // A custom async loader is cached by type and source: two requests, one load.
    let before = DOUBLER_LOADS.load(Ordering::SeqCst);
    assert_eq!(app.update(|cx| cx.fetch_asset::<Doubler>(&21)), None);
    assert_eq!(app.update(|cx| cx.fetch_asset::<Doubler>(&21)), None);
    app.advance_clock(Duration::from_millis(10));
    app.run_until_parked();
    assert_eq!(app.update(|cx| cx.fetch_asset::<Doubler>(&21)), Some(42));
    assert_eq!(app.update(|cx| cx.fetch_asset::<Doubler>(&21)), Some(42));
    assert_eq!(DOUBLER_LOADS.load(Ordering::SeqCst) - before, 1);

    // A different source is a different cache entry.
    assert_eq!(app.update(|cx| cx.fetch_asset::<Doubler>(&5)), None);
    app.advance_clock(Duration::from_millis(10));
    app.run_until_parked();
    assert_eq!(app.update(|cx| cx.fetch_asset::<Doubler>(&5)), Some(10));
    assert_eq!(DOUBLER_LOADS.load(Ordering::SeqCst) - before, 2);
}

#[test]
fn app_002_a6_view_using_a_custom_loader_rerenders_when_the_load_completes() {
    struct ViewDoubler;
    impl Asset for ViewDoubler {
        type Source = u32;
        type Output = u32;

        fn load(
            source: Self::Source,
            cx: &mut App,
        ) -> impl Future<Output = Self::Output> + Send + 'static {
            let executor = cx.background_executor().clone();
            async move {
                executor.timer(Duration::from_millis(10)).await;
                source + 1
            }
        }
    }

    struct ViewDoublerView {
        renders: Rc<Cell<usize>>,
        value: Rc<Cell<Option<u32>>>,
    }

    impl Render for ViewDoublerView {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);
            self.value.set(window.use_asset::<ViewDoubler>(&1, cx));
            div()
        }
    }

    let renders = Rc::new(Cell::new(0));
    let value = Rc::new(Cell::new(None));
    let mut app = TestApp::new();
    let window = app.open_window({
        let (renders, value) = (renders.clone(), value.clone());
        move |_, _| ViewDoublerView { renders, value }
    });
    let _ = window;
    app.run_until_parked();
    assert_eq!(value.get(), None);
    app.advance_clock(Duration::from_millis(10));
    app.run_until_parked();
    assert_eq!(value.get(), Some(2));
    assert_eq!(
        renders.get(),
        2,
        "one render while loading, one after the result arrived"
    );
}
