//! LAY-001 acceptance tests: persistent keyed layout nodes.

use super::*;
use crate::{
    AnyElement, DrawPhase, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, TestAppContext, VisualTestContext, point, px, size,
};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    panic::Location,
    rc::Rc,
};

type BoundsLog = Rc<RefCell<BTreeMap<String, Bounds<Pixels>>>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Contract {
    None,
    Replay,
}

/// A measuring function with a contract: the same `tag` keeps the measurement, a
/// different one is replayed (or replaced when the contract is `None`).
#[derive(Default)]
struct Calls {
    total: Cell<u32>,
    /// Probes that ask for a size under a constraint (known dimension or intrinsic space),
    /// as opposed to the probe every leaf answers when its parent lays it out.
    sizing: Cell<u32>,
}

#[derive(Clone)]
struct TestMeasure {
    tag: u32,
    content: Size<Pixels>,
    contract: Contract,
    calls: Rc<Calls>,
}

impl LayoutMeasure for TestMeasure {
    fn measure(
        &mut self,
        known_dimensions: Size<Option<Pixels>>,
        available_space: Size<AvailableSpace>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Size<Pixels> {
        self.calls.total.set(self.calls.total.get() + 1);
        let final_layout_probe = known_dimensions.width.is_none()
            && known_dimensions.height.is_none()
            && matches!(available_space.width, AvailableSpace::Definite(_))
            && matches!(available_space.height, AvailableSpace::Definite(_));
        if !final_layout_probe {
            self.calls.sizing.set(self.calls.sizing.get() + 1);
        }
        size(
            known_dimensions.width.unwrap_or(self.content.width),
            known_dimensions.height.unwrap_or(self.content.height),
        )
    }

    fn reconcile(&mut self, previous: &mut Self) -> MeasureDecision {
        if self.tag == previous.tag {
            MeasureDecision::Keep
        } else if self.contract == Contract::Replay {
            MeasureDecision::Replay
        } else {
            MeasureDecision::Replace
        }
    }
}

#[derive(Clone, Default)]
struct OwnerRecord {
    owned: OwnedLayoutNodes,
    child_ids: Vec<LayoutId>,
}

enum Kind {
    Container,
    Measured(TestMeasure),
    Closure {
        content: Size<Pixels>,
        calls: Rc<Cell<u32>>,
    },
    /// Requests three nodes for one key: two leaves and a wrapper.
    Twin(Size<Pixels>, Size<Pixels>),
    /// Creates `roots` prepaint-time layout roots.
    Spawner {
        roots: usize,
    },
}

enum Owner {
    None,
    Record(Rc<RefCell<OwnerRecord>>),
    /// Does not construct its children; marks the recorded nodes live.
    Replay(Rc<RefCell<OwnerRecord>>),
    /// Does not construct its children and keeps nothing alive.
    Forget,
}

struct Node {
    id: Option<ElementId>,
    style: Style,
    children: Vec<AnyElement>,
    kind: Kind,
    owner: Owner,
    label: Option<String>,
    log: Option<BoundsLog>,
    spawned: Vec<AnyElement>,
}

impl Node {
    fn new(style: Style) -> Self {
        Node {
            id: None,
            style,
            children: Vec::new(),
            kind: Kind::Container,
            owner: Owner::None,
            label: None,
            log: None,
            spawned: Vec::new(),
        }
    }

    fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

    fn label(mut self, label: &str, log: &BoundsLog) -> Self {
        self.label = Some(label.to_string());
        self.log = Some(log.clone());
        self
    }

    fn child(mut self, child: Node) -> Self {
        self.children.push(child.into_any_element());
        self
    }

    fn children(mut self, children: impl IntoIterator<Item = Node>) -> Self {
        self.children
            .extend(children.into_iter().map(|child| child.into_any_element()));
        self
    }

    fn kind(mut self, kind: Kind) -> Self {
        self.kind = kind;
        self
    }

    fn owner(mut self, owner: Owner) -> Self {
        self.owner = owner;
        self
    }
}

impl IntoElement for Node {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Node {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        self.id.clone()
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let recording = matches!(self.owner, Owner::Record(_));
        if recording {
            window.begin_layout_owner();
        }
        let style = self.style.clone();
        let layout_id = match &self.kind {
            Kind::Container | Kind::Spawner { .. } => {
                let replay_ids = match &self.owner {
                    Owner::Replay(record) => {
                        let record = record.borrow();
                        window.mark_layout_nodes_live(&record.owned);
                        Some(record.child_ids.clone())
                    }
                    _ => None,
                };
                let child_ids: Vec<LayoutId> = match replay_ids {
                    Some(ids) => ids,
                    None => self
                        .children
                        .iter_mut()
                        .map(|child| child.request_layout(window, cx))
                        .collect(),
                };
                if let Owner::Record(record) = &self.owner {
                    record.borrow_mut().child_ids = child_ids.clone();
                }
                window.request_layout(style, child_ids, cx)
            }
            Kind::Measured(measure) => {
                window.request_measured_layout_with(style, measure.clone(), cx)
            }
            Kind::Closure { content, calls } => {
                let (content, calls) = (*content, calls.clone());
                window.request_measured_layout(style, move |known, _, _, _| {
                    calls.set(calls.get() + 1);
                    size(
                        known.width.unwrap_or(content.width),
                        known.height.unwrap_or(content.height),
                    )
                })
            }
            Kind::Twin(a, b) => {
                let first = window.request_layout(fixed(a.width, a.height), None, cx);
                let second = window.request_layout(fixed(b.width, b.height), None, cx);
                window.request_layout(style, [first, second], cx)
            }
        };
        if let Owner::Record(record) = &self.owner {
            record.borrow_mut().owned = window.end_layout_owner();
        }
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _element_id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _layout_state: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let (Some(label), Some(log)) = (&self.label, &self.log) {
            log.borrow_mut().insert(label.clone(), bounds);
        }
        for child in &mut self.children {
            child.prepaint(window, cx);
        }
        if let Kind::Spawner { roots } = &self.kind {
            for index in 0..*roots {
                let mut root = Node::new(fixed(px(10.), px(10.))).into_any_element();
                root.layout_as_root(size(px(100.), px(100.)).into(), window, cx);
                root.prepaint_at(point(px(index as f32), px(0.)), window, cx);
                self.spawned.push(root);
            }
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        for child in &mut self.children {
            child.paint(window, cx);
        }
        for root in &mut self.spawned {
            root.paint(window, cx);
        }
    }
}

fn fixed(width: Pixels, height: Pixels) -> Style {
    Style {
        size: Size {
            width: width.into(),
            height: height.into(),
        },
        ..Style::default()
    }
}

fn column() -> Style {
    Style {
        display: crate::Display::Flex,
        flex_direction: crate::FlexDirection::Column,
        ..Style::default()
    }
}

fn row() -> Style {
    Style {
        display: crate::Display::Flex,
        ..Style::default()
    }
}

fn full_width(mut style: Style) -> Style {
    style.size.width = crate::relative(1.).into();
    style
}

fn measure(tag: u32, width: f32, height: f32, contract: Contract) -> (Kind, Rc<Calls>) {
    let calls = Rc::new(Calls::default());
    (
        Kind::Measured(TestMeasure {
            tag,
            content: size(px(width), px(height)),
            contract,
            calls: calls.clone(),
        }),
        calls,
    )
}

fn draw(window: &mut VisualTestContext, width: f32, root: Node) {
    window.draw(
        point(px(0.), px(0.)),
        size(px(width), px(2000.)),
        move |_, _| root,
    );
}

fn stats(window: &mut VisualTestContext) -> LayoutStats {
    window.update(|window, _| window.layout_stats())
}

fn reset(window: &mut VisualTestContext) {
    window.update(|window, _| window.reset_layout_stats());
}

fn new_log() -> BoundsLog {
    Rc::default()
}

/// Draws `root` with a fresh layout engine, i.e. a complete from-scratch layout, and
/// returns the bounds; the window's persistent engine is put back afterwards.
fn from_scratch(
    window: &mut VisualTestContext,
    log: &BoundsLog,
    width: f32,
    root: Node,
) -> BTreeMap<String, Bounds<Pixels>> {
    let persistent =
        window.update(|window, _| window.layout_engine.replace(TaffyLayoutEngine::new()));
    log.borrow_mut().clear();
    draw(window, width, root);
    window.update(|window, _| window.layout_engine = persistent);
    log.borrow().clone()
}

#[vespertine::test]
fn alternating_widths_hit_the_cache_for_clean_siblings(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let log = new_log();
    let clean_calls: Vec<Rc<Calls>> = (0..4).map(|_| Rc::new(Calls::default())).collect();
    let changing_calls = Rc::new(Cell::new(0));

    let build = |frame: u32| {
        let mut root = Node::new(full_width(column())).label("root", &log);
        for (index, calls) in clean_calls.iter().enumerate() {
            let leaf = Node::new(full_width(Style::default())).kind(Kind::Measured(TestMeasure {
                tag: 1,
                content: size(px(40.), px(17.)),
                contract: Contract::Replay,
                calls: calls.clone(),
            }));
            root = root.child(
                Node::new(full_width(row()))
                    .id(format!("row-{index}"))
                    .child(leaf),
            );
        }
        // The changing leaf has no contract: it is replaced and dirtied every frame.
        root.child(Node::new(full_width(Style::default())).kind(Kind::Closure {
            content: size(px(40.), px(10. + (frame % 3) as f32)),
            calls: changing_calls.clone(),
        }))
    };

    for frame in 0..8u32 {
        let width = if frame % 2 == 0 { 400. } else { 360. };
        reset(window);
        let before: Vec<u32> = clean_calls.iter().map(|calls| calls.sizing.get()).collect();
        draw(window, width, build(frame));
        let after: Vec<u32> = clean_calls.iter().map(|calls| calls.sizing.get()).collect();
        let delta = stats(window);
        assert_eq!(log.borrow()["root"].size.width, px(width));
        if frame >= 4 {
            assert_eq!(
                before, after,
                "clean siblings were asked for a size again in frame {frame}"
            );
            assert_eq!(delta.nodes_created, 0, "frame {frame}");
            assert_eq!(delta.style_writes, 0, "frame {frame}");
            assert_eq!(delta.children_writes, 0, "frame {frame}");
            assert_eq!(
                delta.measure_writes, 1,
                "only the changing leaf, frame {frame}"
            );
            assert_eq!(delta.computed_layouts, 1, "frame {frame}");
        }
    }
}

#[vespertine::test]
fn reordering_identified_siblings_matches_a_from_scratch_layout(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let log = new_log();
    let reference_log = new_log();
    let sizes = [(10., 12.), (20., 14.), (30., 16.), (40., 18.), (50., 20.)];
    let orders: [[usize; 5]; 6] = [
        [0, 1, 2, 3, 4],
        [4, 3, 2, 1, 0],
        [2, 0, 4, 1, 3],
        [0, 1, 2, 3, 4],
        [1, 2, 3, 4, 0],
        [3, 4, 0, 1, 2],
    ];
    let build = |order: &[usize], log: &BoundsLog| {
        Node::new(row())
            .label("root", log)
            .children(order.iter().map(|&i| {
                let (w, h) = sizes[i];
                Node::new(fixed(px(w), px(h)))
                    .id(format!("item-{i}"))
                    .label(&format!("item-{i}"), log)
            }))
    };
    for order in orders {
        log.borrow_mut().clear();
        draw(window, 300., build(&order, &log));
        let expected = from_scratch(window, &reference_log, 300., build(&order, &reference_log));
        assert_eq!(*log.borrow(), expected, "order {order:?}");
    }
}

#[vespertine::test]
fn inserting_anonymous_siblings_matches_a_from_scratch_layout(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let log = new_log();
    let reference_log = new_log();
    let build = |widths: &[f32], log: &BoundsLog| {
        Node::new(row())
            .label("root", log)
            .children(widths.iter().enumerate().map(|(index, &width)| {
                Node::new(fixed(px(width), px(10. + width / 10.)))
                    .label(&format!("{index}"), log)
                    .child(
                        Node::new(fixed(px(width / 2.), px(5.)))
                            .label(&format!("{index}.child"), log),
                    )
            }))
    };
    let sequences: [&[f32]; 6] = [
        &[10., 20., 30.],
        &[10., 99., 20., 30.],
        &[77., 10., 99., 20., 30., 55.],
        &[20., 30.],
        &[],
        &[31., 32., 33., 34.],
    ];
    for widths in sequences {
        log.borrow_mut().clear();
        draw(window, 400., build(widths, &log));
        let expected = from_scratch(window, &reference_log, 400., build(widths, &reference_log));
        assert_eq!(*log.borrow(), expected, "widths {widths:?}");
    }
}

#[vespertine::test]
fn duplicate_key_claims_get_separate_nodes_and_match_from_scratch(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let log = new_log();
    let reference_log = new_log();
    let build = |a: f32, b: f32, log: &BoundsLog| {
        Node::new(column()).label("root", log).child(
            Node::new(row())
                .kind(Kind::Twin(size(px(a), px(10.)), size(px(b), px(20.))))
                .label("twin", log),
        )
    };
    for (frame, (a, b)) in [(10., 20.), (30., 40.), (30., 40.), (5., 80.)]
        .into_iter()
        .enumerate()
    {
        log.borrow_mut().clear();
        reset(window);
        draw(window, 300., build(a, b, &log));
        let expected = from_scratch(window, &reference_log, 300., build(a, b, &reference_log));
        assert_eq!(*log.borrow(), expected);
        // Three claims of one key: one persistent node and two temporaries.
        let delta = stats(window);
        if frame > 0 {
            assert_eq!((delta.nodes_created, delta.nodes_released), (2, 2));
        }
    }
    // No temporary node outlives its frame: root + container + the persistent twin leaf.
    let live = stats(window).live_nodes;
    assert!(live <= 4, "live nodes {live}");
}

#[vespertine::test]
fn recycled_slots_never_return_stale_bounds(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let log = new_log();
    let reference_log = new_log();
    // Every frame uses fresh element identities, so every frame's nodes occupy slots
    // that earlier frames' nodes released.
    let build = |count: usize, scale: f32, log: &BoundsLog| {
        Node::new(column())
            .label("root", log)
            .children((0..count).map(|index| {
                Node::new(fixed(px(10. + scale * index as f32), px(5. + index as f32)))
                    .id(format!("n-{index}-{scale}"))
                    .label(&format!("n-{index}"), log)
                    .child(Node::new(fixed(px(4.), px(2.))).label(&format!("n-{index}.c"), log))
            }))
    };
    let mut slot_counts = Vec::new();
    for (round, (count, scale)) in [(12, 1.), (3, 2.), (9, 3.), (1, 4.), (12, 5.), (6, 6.)]
        .into_iter()
        .cycle()
        .take(18)
        .enumerate()
    {
        let scale = scale + round as f32;
        log.borrow_mut().clear();
        draw(window, 300., build(count, scale, &log));
        let expected = from_scratch(
            window,
            &reference_log,
            300.,
            build(count, scale, &reference_log),
        );
        assert_eq!(*log.borrow(), expected, "count {count} scale {scale}");
        slot_counts
            .push(window.update(|window, _| window.layout_engine.as_ref().unwrap().slots.len()));
    }
    // Slots are recycled: the engine stops growing once the largest frame pair was seen.
    let plateau = slot_counts[8];
    assert!(
        slot_counts[8..].iter().all(|count| *count == plateau),
        "slot counts {slot_counts:?}"
    );
}

#[vespertine::test]
fn a_recycled_slot_rejects_the_old_handle(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let log = new_log();
    let ids: Rc<RefCell<Vec<LayoutId>>> = Rc::default();
    let capture = Node::new(column())
        .id("captured")
        .owner(Owner::Record(Rc::default()));
    let record = match &capture.owner {
        Owner::Record(record) => record.clone(),
        _ => unreachable!(),
    };
    draw(
        window,
        100.,
        capture.child(Node::new(fixed(px(5.), px(5.))).id("a")),
    );
    *ids.borrow_mut() = record.borrow().owned.nodes.clone();
    assert!(!ids.borrow().is_empty());
    // Frame without the nodes: released.
    draw(
        window,
        100.,
        Node::new(fixed(px(1.), px(1.))).label("other", &log),
    );
    window.update(|window, _| {
        assert!(
            ids.borrow()
                .iter()
                .all(|id| !window.layout_node_is_live(*id))
        );
    });
    // Frame that creates nodes reusing the freed slots: the old handles stay dead.
    draw(
        window,
        100.,
        Node::new(column()).children(
            (0..4).map(|i| Node::new(fixed(px(5.), px(5. + i as f32))).id(format!("b{i}"))),
        ),
    );
    window.update(|window, _| {
        assert!(
            ids.borrow()
                .iter()
                .all(|id| !window.layout_node_is_live(*id))
        );
    });
}

#[vespertine::test]
fn prepaint_roots_do_not_grow_the_live_node_count(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let mut counts = Vec::new();
    for _ in 0..60 {
        draw(
            window,
            200.,
            Node::new(column())
                .kind(Kind::Spawner { roots: 5 })
                .child(Node::new(fixed(px(5.), px(5.)))),
        );
        counts.push(stats(window).live_nodes);
    }
    assert!(counts[0] > 0);
    assert!(
        counts.iter().all(|count| *count == counts[0]),
        "live node counts per frame: {counts:?}"
    );
    // After warm-up, spawning roots again creates no nodes.
    reset(window);
    draw(
        window,
        200.,
        Node::new(column())
            .kind(Kind::Spawner { roots: 5 })
            .child(Node::new(fixed(px(5.), px(5.)))),
    );
    assert_eq!(stats(window).nodes_created, 0);
}

#[vespertine::test]
fn prepaint_roots_and_normal_requests_use_disjoint_key_spaces(_cx: &mut TestAppContext) {
    let mut stack = LayoutKeyStack::new();
    let normal = stack.claim_key();
    stack.begin_root(0);
    let root = stack.claim_key();
    stack.end_root();
    assert_eq!(normal.namespace, KeyNamespace::Normal);
    assert_eq!(root.namespace, KeyNamespace::PrepaintRoot);
    assert_ne!(normal, root);
    // Same hash in different namespaces never names the same node.
    let mut engine = TaffyLayoutEngine::new();
    let a = LayoutKey {
        namespace: KeyNamespace::Normal,
        hash: 7,
    };
    let b = LayoutKey {
        namespace: KeyNamespace::PrepaintRoot,
        hash: 7,
    };
    let style = Style::default();
    let first = engine.request_layout(a, &style, px(16.), 1., &[]);
    let second = engine.request_layout(b, &style, px(16.), 1., &[]);
    assert_ne!(first, second);
    assert_eq!(engine.stats().nodes_created, 2);
}

#[vespertine::test]
fn a_captured_key_context_reproduces_the_keys_of_the_original_build(_cx: &mut TestAppContext) {
    let mut stack = LayoutKeyStack::new();
    stack.enter_element(None);
    stack.enter_element(Some(&ElementId::from("named")));
    let sibling = stack.claim_key();
    stack.exit_element();
    let context = stack.context();
    stack.enter_element(None);
    let anonymous = stack.claim_key();
    stack.exit_element();
    stack.set_item_index(Some(4));
    stack.enter_element(None);
    let indexed = stack.claim_key();
    stack.exit_element();
    stack.exit_element();

    stack.reset();
    stack.begin_root(9);
    stack.enter_element(None);
    stack.push_context(context);
    stack.enter_element(None);
    let replayed = stack.claim_key();
    stack.exit_element();
    stack.pop_context();
    assert_eq!(replayed.namespace, KeyNamespace::Normal);
    assert_eq!(replayed, anonymous);
    assert_ne!(replayed, sibling);
    assert_ne!(replayed, indexed);
    // The current position is restored after the scope.
    assert_eq!(stack.claim_key().namespace, KeyNamespace::PrepaintRoot);
}

#[vespertine::test]
fn a_repeated_probe_moves_to_the_end_of_the_log(_cx: &mut TestAppContext) {
    let probe = |width: f32| Probe {
        known_dimensions: Size {
            width: Some(px(width)),
            height: None,
        },
        available_space: Size {
            width: AvailableSpace::MaxContent,
            height: AvailableSpace::MaxContent,
        },
        result: Size {
            width: width,
            height: 1.0,
        },
    };
    let mut log = ProbeLog::default();
    for width in [1., 2., 3., 2.] {
        log.record(probe(width));
    }
    let order: Vec<f32> = log.entries.iter().map(|entry| entry.result.width).collect();
    assert_eq!(order, [1., 3., 2.]);
    assert!(!log.overflowed);
    for width in 0..PROBE_LOG_CAPACITY {
        log.record(probe(10. + width as f32));
    }
    assert!(log.overflowed);
    assert_eq!(log.entries.len(), PROBE_LOG_CAPACITY);
}

#[vespertine::test]
fn relayout_in_place_fixes_the_size_and_keeps_the_position(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let log = new_log();
    let record = Rc::new(RefCell::new(OwnerRecord::default()));
    draw(
        window,
        300.,
        Node::new(column())
            .child(Node::new(fixed(px(10.), px(25.))).id("above"))
            .child(
                Node::new(Style {
                    size: Size {
                        width: px(100.).into(),
                        height: px(40.).into(),
                    },
                    ..row()
                })
                .id("view")
                .owner(Owner::Record(record.clone()))
                .label("view", &log)
                .child(Node::new(fixed(px(30.), px(10.))).id("fixed"))
                .child(
                    Node::new(Style {
                        flex_grow: 1.,
                        ..Style::default()
                    })
                    .id("fill")
                    .label("fill", &log),
                ),
            ),
    );
    assert_eq!(log.borrow()["view"].origin, point(px(0.), px(25.)));
    assert_eq!(log.borrow()["fill"].size.width, px(70.));
    let view_id = *record.borrow().owned.nodes.last().unwrap();
    let fill_id = record.borrow().child_ids[1];

    reset(window);
    let (view, fill) = window.update(|window, cx| {
        window.invalidator.set_phase(DrawPhase::Prepaint);
        window.relayout_subtree_in_place(view_id, size(px(50.), px(60.)), cx);
        let bounds = (window.layout_bounds(view_id), window.layout_bounds(fill_id));
        window.invalidator.set_phase(DrawPhase::None);
        bounds
    });
    assert_eq!(view.origin, point(px(0.), px(25.)), "the node did not move");
    assert_eq!(view.size, size(px(50.), px(60.)));
    assert_eq!(fill.origin, point(px(30.), px(25.)));
    assert_eq!(fill.size, size(px(20.), px(60.)));
    let delta = stats(window);
    assert_eq!((delta.computed_layouts, delta.nodes_created), (1, 0));
}

#[vespertine::test]
fn measured_to_unmeasured_clears_the_measure_context(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let log = new_log();
    let leaf_id = |window: &mut VisualTestContext| {
        window.update(|window, _| {
            let engine = window.layout_engine.as_ref().unwrap();
            engine
                .slots
                .iter()
                .enumerate()
                .filter(|(_, slot)| slot.live && slot.key.is_some())
                .map(|(index, _)| engine.id_of(index))
                .filter(|id| engine.has_measure(*id))
                .count()
        })
    };
    let (kind, calls) = measure(1, 40., 17., Contract::Replay);
    draw(
        window,
        200.,
        Node::new(column()).child(
            Node::new(Style::default())
                .id("leaf")
                .kind(kind)
                .label("leaf", &log),
        ),
    );
    assert_eq!(leaf_id(window), 1);
    assert_eq!(log.borrow()["leaf"].size, size(px(40.), px(17.)));
    let measured_calls = calls.total.get();
    assert!(measured_calls > 0);

    // The same key now requests a plain node with an explicit size.
    reset(window);
    draw(
        window,
        200.,
        Node::new(column()).child(
            Node::new(fixed(px(7.), px(9.)))
                .id("leaf")
                .label("leaf", &log),
        ),
    );
    assert_eq!(leaf_id(window), 0, "the old measuring function was removed");
    assert_eq!(stats(window).measure_writes, 1);
    assert_eq!(
        calls.total.get(),
        measured_calls,
        "the old measuring function is never called again"
    );
    assert_eq!(log.borrow()["leaf"].size, size(px(7.), px(9.)));
}

#[vespertine::test]
fn replacing_a_measure_without_contract_dirties_the_node(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let log = new_log();
    let first_calls = Rc::new(Cell::new(0));
    let second_calls = Rc::new(Cell::new(0));
    let tree = |calls: &Rc<Cell<u32>>, height: f32| {
        Node::new(column()).label("root", &log).child(
            Node::new(Style::default())
                .id("leaf")
                .label("leaf", &log)
                .kind(Kind::Closure {
                    content: size(px(30.), px(height)),
                    calls: calls.clone(),
                }),
        )
    };
    draw(window, 200., tree(&first_calls, 10.));
    reset(window);
    // A different measuring function without a replay contract is installed and dirties.
    draw(window, 200., tree(&second_calls, 25.));
    let delta = stats(window);
    assert_eq!(delta.measure_writes, 1);
    assert_eq!(delta.computed_layouts, 1);
    assert!(second_calls.get() > 0);
    assert_eq!(log.borrow()["leaf"].size.height, px(25.));
    let first = first_calls.get();
    draw(window, 200., tree(&second_calls, 25.));
    assert_eq!(first_calls.get(), first);
}

#[vespertine::test]
fn a_different_measure_type_without_contract_is_replaced_and_dirties(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let tree = |tag: u32, width: f32| {
        let (kind, _) = measure(tag, width, 17., Contract::None);
        Node::new(column()).child(Node::new(Style::default()).id("leaf").kind(kind))
    };
    draw(window, 200., tree(1, 40.));
    reset(window);
    // The measuring function has no replay contract: a changed one replaces and dirties.
    draw(window, 200., tree(2, 40.));
    let delta = stats(window);
    assert_eq!(
        (
            delta.measure_writes,
            delta.replays_succeeded,
            delta.replays_failed
        ),
        (1, 0, 0)
    );
    assert_eq!(delta.computed_layouts, 1);
    // An identical one is carried.
    reset(window);
    draw(window, 200., tree(2, 40.));
    assert_eq!(stats(window).measure_writes, 0);
}

#[vespertine::test]
fn replay_with_unchanged_sizes_keeps_ancestors_clean(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let log = new_log();
    let tree = |tag: u32| {
        let (kind, _) = measure(tag, 40., 17., Contract::Replay);
        Node::new(column())
            .label("root", &log)
            .child(Node::new(Style::default()).id("leaf").kind(kind))
    };
    draw(window, 200., tree(1));
    draw(window, 200., tree(1));
    reset(window);
    // A different value that measures the same: replayed under the logged probes.
    draw(window, 200., tree(2));
    let delta = stats(window);
    assert_eq!(delta.replays_succeeded, 1);
    assert_eq!(delta.replays_failed, 0);
    assert_eq!(delta.computed_layouts, 0, "ancestors stay clean");
    assert!(delta.skipped_layouts >= 1);
    assert_eq!(delta.measure_writes, 1);
    assert!(delta.measure_calls >= 1);
    // Same tag: carried without any write.
    reset(window);
    draw(window, 200., tree(2));
    let delta = stats(window);
    assert_eq!(delta.measure_writes, 0);
    assert_eq!(delta.measure_calls, 0);
}

#[vespertine::test]
fn replay_with_a_changed_size_dirties_the_node_and_ancestors(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let log = new_log();
    let tree = |tag: u32, width: f32| {
        let (kind, _) = measure(tag, width, 17., Contract::Replay);
        Node::new(column()).label("root", &log).child(
            Node::new(Style::default())
                .id("leaf")
                .label("leaf", &log)
                .kind(kind),
        )
    };
    draw(window, 200., tree(1, 40.));
    reset(window);
    draw(window, 200., tree(2, 55.));
    let delta = stats(window);
    assert_eq!(delta.replays_failed, 1);
    assert_eq!(delta.computed_layouts, 1);
    assert_eq!(log.borrow()["leaf"].size.width, px(55.));
}

#[vespertine::test]
fn a_replay_log_that_overflowed_is_conservative(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let log = new_log();
    let tree = |tag: u32| {
        let (kind, _) = measure(tag, 40., 17., Contract::Replay);
        Node::new(full_width(column())).child(
            Node::new(full_width(Style::default()))
                .id("leaf")
                .label("leaf", &log)
                .kind(kind),
        )
    };
    // Seventeen distinct widths probe the leaf with seventeen distinct known widths.
    for index in 0..=PROBE_LOG_CAPACITY {
        draw(window, 300. + index as f32, tree(1));
    }
    reset(window);
    // The sizes would be unchanged, but the log overflowed: dirty regardless.
    draw(window, 300. + PROBE_LOG_CAPACITY as f32, tree(2));
    let delta = stats(window);
    assert_eq!(delta.replays_failed, 1);
    assert_eq!(delta.replays_succeeded, 0);
    assert_eq!(delta.computed_layouts, 1);
}

#[vespertine::test]
fn unchanged_inputs_are_not_written_and_changed_ones_are_written_independently(
    cx: &mut TestAppContext,
) {
    let window = cx.add_empty_window();
    let log = new_log();
    let tree = |width: f32, extra: bool, tag: u32| {
        let (kind, _) = measure(tag, 40., 17., Contract::Replay);
        let mut root = Node::new(row())
            .child(Node::new(fixed(px(width), px(10.))).id("a"))
            .child(
                Node::new(Style::default())
                    .id("b")
                    .kind(kind)
                    .label("b", &log),
            );
        if extra {
            root = root.child(Node::new(fixed(px(3.), px(3.))).id("c"));
        }
        root
    };
    draw(window, 300., tree(10., false, 1));
    reset(window);
    draw(window, 300., tree(10., false, 1));
    let none = stats(window);
    assert_eq!(
        (
            none.nodes_created,
            none.style_writes,
            none.children_writes,
            none.measure_writes
        ),
        (0, 0, 0, 0)
    );
    assert!(none.nodes_reused >= 4);

    reset(window);
    draw(window, 300., tree(11., false, 1));
    let style_only = stats(window);
    assert_eq!(
        (
            style_only.style_writes,
            style_only.children_writes,
            style_only.measure_writes
        ),
        (1, 0, 0)
    );

    reset(window);
    draw(window, 300., tree(11., true, 1));
    let children_only = stats(window);
    assert_eq!(
        (
            children_only.style_writes,
            children_only.children_writes,
            children_only.measure_writes
        ),
        (0, 1, 0)
    );
    assert_eq!(children_only.nodes_created, 1);

    reset(window);
    draw(window, 300., tree(11., true, 2));
    let measure_only = stats(window);
    assert_eq!(
        (
            measure_only.style_writes,
            measure_only.children_writes,
            measure_only.measure_writes
        ),
        (0, 0, 1)
    );
}

#[cfg(debug_assertions)]
#[vespertine::test]
fn a_fingerprint_collision_is_detected_in_debug_builds(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    draw(window, 100., Node::new(fixed(px(10.), px(10.))).id("a"));
    // Every style now fingerprints identically: a deliberate collision.
    window.update(|window, _| {
        window
            .layout_engine
            .as_mut()
            .unwrap()
            .override_style_fingerprint(Some(0xDEAD_BEEF));
    });
    draw(window, 100., Node::new(fixed(px(10.), px(10.))).id("a"));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        draw(window, 100., Node::new(fixed(px(99.), px(10.))).id("a"));
    }));
    let payload = outcome.expect_err("a collision with unequal styles must be detected");
    let message = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|text| text.to_string()))
        .unwrap_or_default();
    assert!(
        message.contains("style fingerprint collision"),
        "unexpected failure: {message}"
    );
}

#[vespertine::test]
fn a_matching_fingerprint_with_equal_styles_is_not_a_collision(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, _| {
        window
            .layout_engine
            .as_mut()
            .unwrap()
            .override_style_fingerprint(Some(7));
    });
    draw(window, 100., Node::new(fixed(px(10.), px(10.))).id("a"));
    reset(window);
    draw(window, 100., Node::new(fixed(px(10.), px(10.))).id("a"));
    assert_eq!(stats(window).style_writes, 0);
}

#[vespertine::test]
fn styles_differing_only_in_the_final_tag_word_are_both_written(cx: &mut TestAppContext) {
    // The tag stream of this style ends in a partial word whose high bits carry the grid
    // row template; changing its minimum size must still read as a different style.
    let grid = |min_size| Style {
        display: crate::Display::Grid,
        margin: Edges {
            top: Length::Auto,
            right: Length::Auto,
            bottom: Length::Auto,
            left: Length::Auto,
        },
        padding: Edges {
            top: DefiniteLength::Fraction(0.0),
            right: DefiniteLength::Fraction(0.0),
            ..Edges::default()
        },
        grid_rows: Some(crate::GridTemplate {
            repeat: 1,
            min_size,
        }),
        ..Style::default()
    };
    let window = cx.add_empty_window();
    draw(
        window,
        100.,
        Node::new(grid(crate::GridTemplateMinSize::Zero)).id("a"),
    );
    reset(window);
    draw(
        window,
        100.,
        Node::new(grid(crate::GridTemplateMinSize::MaxContent)).id("a"),
    );
    assert_eq!(stats(window).style_writes, 1);
}

#[vespertine::test]
fn retained_nodes_survive_a_frame_without_reconstruction(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let log = new_log();
    let record = Rc::new(RefCell::new(OwnerRecord::default()));
    let tree = |owner: Owner, children: bool| {
        let mut node = Node::new(column())
            .id("view")
            .owner(owner)
            .label("view", &log);
        if children {
            node = node
                .child(Node::new(fixed(px(10.), px(10.))).id("x"))
                .child(
                    Node::new(fixed(px(20.), px(30.)))
                        .id("y")
                        .child(Node::new(fixed(px(2.), px(2.)))),
                );
        }
        node
    };
    draw(window, 100., tree(Owner::Record(record.clone()), true));
    let owned = record.borrow().owned.clone();
    assert_eq!(owned.len(), 4, "x, y, y's child and the view node");
    let first_bounds = log.borrow()["view"];
    window.update(|window, _| {
        assert!(owned.nodes.iter().all(|id| window.layout_node_is_live(*id)));
    });

    // Frame two: the children are not reconstructed; the owner marks its nodes live.
    reset(window);
    let writes_before = window.update(|window, _| window.layout_write_count());
    draw(window, 100., tree(Owner::Replay(record.clone()), false));
    let delta = stats(window);
    let writes_after = window.update(|window, _| window.layout_write_count());
    assert_eq!(writes_after, writes_before, "replay writes nothing");
    assert_eq!((delta.nodes_created, delta.nodes_released), (0, 0));
    assert_eq!(log.borrow()["view"], first_bounds);
    window.update(|window, _| {
        assert!(owned.nodes.iter().all(|id| window.layout_node_is_live(*id)));
        for id in &record.borrow().child_ids {
            assert!(window.layout_node_is_live(*id));
        }
    });

    // Frame three: not reconstructed and not marked: released after this one frame.
    reset(window);
    draw(window, 100., tree(Owner::Forget, false));
    assert_eq!(stats(window).nodes_released, 3, "x, y and y's child");
    window.update(|window, _| {
        for id in &record.borrow().child_ids {
            assert!(!window.layout_node_is_live(*id));
        }
    });
}

#[vespertine::test]
fn retained_nodes_linger_for_the_requested_number_of_frames(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let record = Rc::new(RefCell::new(OwnerRecord::default()));
    draw(
        window,
        100.,
        Node::new(column())
            .id("view")
            .owner(Owner::Record(record.clone()))
            .child(Node::new(fixed(px(10.), px(10.))).id("x")),
    );
    let owned = record.borrow().owned.clone();
    window.update(|window, _| window.retain_layout_nodes(&owned, 3));
    for frame in 0..5 {
        draw(window, 100., Node::new(fixed(px(1.), px(1.))).id("other"));
        let alive = window.update(|window, _| {
            owned
                .nodes
                .iter()
                .filter(|id| window.layout_node_is_live(**id))
                .count()
        });
        // Retained between frames: the upcoming frame counts as the current one.
        if frame <= 3 {
            assert!(alive > 0, "frame {frame}");
        } else {
            assert_eq!(alive, 0, "frame {frame}");
        }
    }
}

#[vespertine::test]
fn nested_layout_owners_keep_their_own_nodes(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let outer = Rc::new(RefCell::new(OwnerRecord::default()));
    let inner = Rc::new(RefCell::new(OwnerRecord::default()));
    draw(
        window,
        100.,
        Node::new(column())
            .id("outer")
            .owner(Owner::Record(outer.clone()))
            .child(Node::new(fixed(px(1.), px(1.))).id("a"))
            .child(
                Node::new(column())
                    .id("inner")
                    .owner(Owner::Record(inner.clone()))
                    .child(Node::new(fixed(px(2.), px(2.))).id("b")),
            ),
    );
    // A node belongs to the innermost owner at the time it is claimed.
    assert_eq!(inner.borrow().owned.len(), 2, "inner node and b");
    assert_eq!(outer.borrow().owned.len(), 2, "outer node and a");
}

/// PERF-LAY-001 (LAY-001 performance workload): 100 rows of 8 numeric labels in a table
/// whose width alternates between 400 and 360.
fn perf_workload(
    window: &mut VisualTestContext,
    frames: u32,
) -> (Vec<LayoutStats>, Vec<u128>, Vec<u32>) {
    let calls = Rc::new(Calls::default());
    let mut sizing = Vec::new();
    let mut counters = [0u32; 100];
    let mut per_frame = Vec::new();
    let mut times = Vec::new();
    for frame in 0..frames {
        if frame >= 31 {
            counters[((frame - 31) % 100) as usize] += 1;
        }
        let width = if frame % 2 == 0 { 400. } else { 360. };
        let table = Node::new(Style {
            overflow: crate::Point {
                x: crate::Overflow::Hidden,
                y: crate::Overflow::Hidden,
            },
            ..{
                let mut style = column();
                style.size.width = px(width).into();
                style
            }
        })
        .children((0..100usize).map(|r| {
            Node::new(Style {
                gap: crate::Size {
                    width: px(4.).into(),
                    height: px(0.).into(),
                },
                size: Size {
                    width: crate::relative(1.).into(),
                    height: px(20.).into(),
                },
                ..row()
            })
            .children((0..8usize).map({
                let calls = calls.clone();
                move |j| {
                    let value = (((r * 8 + j) * 37) as u32 + counters[r]) % 100_000;
                    Node::new(Style::default()).kind(Kind::Measured(TestMeasure {
                        tag: value,
                        content: size(px(40.), px(17.)),
                        contract: Contract::Replay,
                        calls: calls.clone(),
                    }))
                }
            }))
        }));
        reset(window);
        let sizing_before = calls.sizing.get();
        let start = std::time::Instant::now();
        draw(window, 880., table);
        times.push(start.elapsed().as_micros());
        sizing.push(calls.sizing.get() - sizing_before);
        per_frame.push(stats(window));
    }
    (per_frame, times, sizing)
}

#[vespertine::test]
fn perf_lay_001_warmed_frames_create_no_nodes_and_write_only_changed_inputs(
    cx: &mut TestAppContext,
) {
    let window = cx.add_empty_window();
    let (stats, _, sizing) = perf_workload(window, 40);
    for (frame, delta) in stats.iter().enumerate().skip(31) {
        // Only the changed labels' replays ask for sizes under constraints.
        assert!(
            sizing[frame] <= 8 * PROBE_LOG_CAPACITY as u32,
            "frame {frame}"
        );
        assert_eq!(delta.nodes_created, 0, "frame {frame}");
        assert_eq!(delta.children_writes, 0, "frame {frame}");
        assert_eq!(delta.style_writes, 1, "only the table width, frame {frame}");
        assert_eq!(
            delta.measure_writes, 8,
            "the changed row's labels, frame {frame}"
        );
        assert_eq!(delta.replays_succeeded, 8, "frame {frame}");
        assert_eq!(delta.nodes_released, 0, "frame {frame}");
    }
}

#[vespertine::test]
#[ignore = "benchmark: prints PERF-LAY-001 layout counters; run with --ignored --nocapture"]
fn perf_lay_001_benchmark(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let (stats, times, sizing) = perf_workload(window, 31 + 300);
    let measured = &stats[31..];
    let sum = |select: fn(&LayoutStats) -> u64| {
        measured.iter().map(select).sum::<u64>() as f64 / measured.len() as f64
    };
    let mean_us = times[31..].iter().sum::<u128>() as f64 / measured.len() as f64;
    println!(
        "PERF-LAY-001 per frame over 300 frames: created {:.2} reused {:.1} style_writes {:.2} children_writes {:.2} \
         measure_writes {:.2} measure_calls {:.2} sizing_probes {:.2} computed_layouts {:.2} live_nodes {} draw {:.0} us",
        sum(|s| s.nodes_created),
        sum(|s| s.nodes_reused),
        sum(|s| s.style_writes),
        sum(|s| s.children_writes),
        sum(|s| s.measure_writes),
        sum(|s| s.measure_calls),
        sizing[31..].iter().sum::<u32>() as f64 / measured.len() as f64,
        sum(|s| s.computed_layouts),
        measured.last().unwrap().live_nodes,
        mean_us,
    );
}
