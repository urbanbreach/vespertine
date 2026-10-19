//! Tests of dependency-driven retained frames (PRF-001).

use crate::{
    AnyWindowHandle, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, Styled as _, TestAppContext, Window, div, px,
};

struct Panel {
    value: u32,
}

impl Render for Panel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(100.))
            .h(px(40.))
            .child(format!("panel {:02}", self.value))
    }
}

struct Dashboard {
    panels: Vec<Entity<Panel>>,
}

impl Render for Dashboard {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_wrap()
            .size_full()
            .children(self.panels.iter().cloned())
    }
}

fn draw(cx: &mut TestAppContext, window: AnyWindowHandle) {
    cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
}

fn stats(cx: &mut TestAppContext, window: AnyWindowHandle) -> crate::RetentionStats {
    cx.update_window(window, |_, window, _| window.retention_stats())
        .unwrap()
}

fn reset(cx: &mut TestAppContext, window: AnyWindowHandle) {
    cx.update_window(window, |_, window, _| window.reset_retention_stats())
        .unwrap();
}

#[crate::test]
fn unchanged_window_builds_no_views(cx: &mut TestAppContext) {
    let panels: Vec<Entity<Panel>> = (0..8).map(|ix| cx.new(|_| Panel { value: ix })).collect();
    let window = cx.add_window(move |_, _| Dashboard { panels });
    let window: AnyWindowHandle = window.into();
    draw(cx, window);
    draw(cx, window);
    reset(cx, window);
    draw(cx, window);
    let s = stats(cx, window);
    assert_eq!(s.views_rendered, 0, "{s:?}");
    assert!(s.views_reused >= 1, "{s:?}");
}

#[crate::test]
fn notified_panel_is_the_only_one_built(cx: &mut TestAppContext) {
    let panels: Vec<Entity<Panel>> = (0..8).map(|ix| cx.new(|_| Panel { value: ix })).collect();
    let window = cx.add_window({
        let panels = panels.clone();
        move |_, _| Dashboard { panels }
    });
    let window: AnyWindowHandle = window.into();
    draw(cx, window);
    draw(cx, window);
    reset(cx, window);
    panels[3].update(cx, |panel, cx| {
        panel.value = 99;
        cx.notify();
    });
    draw(cx, window);
    let s = stats(cx, window);
    assert_eq!(s.views_rendered, 1, "{s:?}");
}

// ---------------------------------------------------------------------------
// Dependency rules
// ---------------------------------------------------------------------------

use std::{cell::Cell, rc::Rc};

#[derive(Clone, Default)]
struct Flag(u32);
impl crate::Global for Flag {}

struct Counter {
    renders: Rc<Cell<u32>>,
}

struct GlobalValueView(Counter);
impl Render for GlobalValueView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.0.renders.set(self.0.renders.get() + 1);
        let value = cx.try_global::<Flag>().map_or(0, |flag| flag.0);
        div().w(px(50.)).h(px(10.)).child(format!("value {value}"))
    }
}

struct GlobalPresenceView(Counter);
impl Render for GlobalPresenceView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.0.renders.set(self.0.renders.get() + 1);
        let present = cx.has_global::<Flag>();
        div()
            .w(px(50.))
            .h(px(10.))
            .child(format!("present {present}"))
    }
}

struct TwoViews {
    value: Entity<GlobalValueView>,
    presence: Entity<GlobalPresenceView>,
}
impl Render for TwoViews {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(self.value.clone())
            .child(self.presence.clone())
    }
}

fn counter() -> Rc<Cell<u32>> {
    Rc::new(Cell::new(0))
}

/// A4: a presence-only global read is invalidated by insertion and removal,
/// not by a value write that keeps the global present.
#[crate::test]
fn presence_only_global_dependency(cx: &mut TestAppContext) {
    let (value_renders, presence_renders) = (counter(), counter());
    let value = cx.new({
        let renders = value_renders.clone();
        move |_| GlobalValueView(Counter { renders })
    });
    let presence = cx.new({
        let renders = presence_renders.clone();
        move |_| GlobalPresenceView(Counter { renders })
    });
    let window = cx.add_window(move |_, _| TwoViews { value, presence });
    let window: AnyWindowHandle = window.into();
    draw(cx, window);
    draw(cx, window);
    let baseline = (value_renders.get(), presence_renders.get());

    cx.update(|cx| cx.set_global(Flag(1)));
    draw(cx, window);
    let after_insert = (value_renders.get(), presence_renders.get());
    assert_eq!(
        after_insert.0,
        baseline.0 + 1,
        "value reader sees insertion"
    );
    assert_eq!(
        after_insert.1,
        baseline.1 + 1,
        "presence reader sees insertion"
    );

    cx.update(|cx| cx.set_global(Flag(2)));
    draw(cx, window);
    let after_write = (value_renders.get(), presence_renders.get());
    assert_eq!(
        after_write.0,
        after_insert.0 + 1,
        "value reader sees the write"
    );
    assert_eq!(
        after_write.1, after_insert.1,
        "presence reader ignores the write"
    );

    cx.update(|cx| {
        cx.remove_global::<Flag>();
    });
    draw(cx, window);
    let after_remove = (value_renders.get(), presence_renders.get());
    assert_eq!(after_remove.0, after_write.0 + 1);
    assert_eq!(
        after_remove.1,
        after_write.1 + 1,
        "presence reader sees removal"
    );
}

struct Model {
    value: u32,
}

struct ModelReader {
    model: Entity<Model>,
    renders: Rc<Cell<u32>>,
}
impl Render for ModelReader {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        let value = self.model.read(cx).value;
        div().w(px(50.)).h(px(10.)).child(format!("model {value}"))
    }
}

struct ModelParent {
    ordinary: Entity<ModelReader>,
    cached: Entity<ModelReader>,
}
impl Render for ModelParent {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.ordinary.clone()).child(
            self.cached
                .clone()
                .cached(crate::StyleRefinement::default().w(px(50.)).h(px(10.))),
        )
    }
}

/// A5: an unnotified update does not invalidate an explicitly cached view but
/// does invalidate an ordinary descendant when an ancestor was notified.
#[crate::test]
fn unnotified_update_rules(cx: &mut TestAppContext) {
    let model = cx.new(|_| Model { value: 0 });
    let (ordinary_renders, cached_renders) = (counter(), counter());
    let handle = cx.add_window({
        let (model, ordinary_renders, cached_renders) = (
            model.clone(),
            ordinary_renders.clone(),
            cached_renders.clone(),
        );
        move |_, cx| {
            let ordinary = cx.new({
                let model = model.clone();
                move |_| ModelReader {
                    model,
                    renders: ordinary_renders,
                }
            });
            let cached = cx.new({
                let model = model.clone();
                move |_| ModelReader {
                    model,
                    renders: cached_renders,
                }
            });
            ModelParent { ordinary, cached }
        }
    });
    let parent = handle.root(cx).unwrap();
    let window: AnyWindowHandle = handle.into();
    draw(cx, window);
    draw(cx, window);
    let baseline = (ordinary_renders.get(), cached_renders.get());

    // Updated without notification, no ancestor notified: nothing rebuilds.
    model.update(cx, |model, _| model.value = 1);
    draw(cx, window);
    assert_eq!((ordinary_renders.get(), cached_renders.get()), baseline);

    // The ancestor is notified: the ordinary descendant re-evaluates the
    // update, the cached view keeps the upstream rule.
    parent.update(cx, |_, cx| cx.notify());
    draw(cx, window);
    assert_eq!(ordinary_renders.get(), baseline.0 + 1);
    assert_eq!(cached_renders.get(), baseline.1);
}

struct SelfWriter {
    renders: Rc<Cell<u32>>,
    state: u32,
}
impl Render for SelfWriter {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        self.state += 1;
        div().w(px(50.)).h(px(10.))
    }
}

struct WriterReader {
    writer: Entity<SelfWriter>,
    renders: Rc<Cell<u32>>,
}
impl Render for WriterReader {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        let state = self.writer.read(cx).state;
        div().w(px(50.)).h(px(10.)).child(format!("state {state}"))
    }
}

struct WriterParent {
    reader: Entity<WriterReader>,
    writer: Entity<SelfWriter>,
}
impl Render for WriterParent {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(self.reader.clone())
            .child(self.writer.clone())
    }
}

/// A6: a view writing its own construction state does not invalidate itself;
/// another view that read that state is invalidated.
#[crate::test]
fn writing_own_state_does_not_self_invalidate(cx: &mut TestAppContext) {
    let (writer_renders, reader_renders) = (counter(), counter());
    let writer = cx.new({
        let renders = writer_renders.clone();
        move |_| SelfWriter { renders, state: 0 }
    });
    let reader = cx.new({
        let (writer, renders) = (writer.clone(), reader_renders.clone());
        move |_| WriterReader { writer, renders }
    });
    let window = cx.add_window(move |_, _| WriterParent { reader, writer });
    let window: AnyWindowHandle = window.into();
    for _ in 0..4 {
        draw(cx, window);
    }
    // The reader read the writer before the writer wrote: it rebuilds once
    // more; the writer never invalidates itself.
    assert_eq!(
        writer_renders.get(),
        1,
        "the writer did not invalidate itself"
    );
    assert_eq!(reader_renders.get(), 2, "the reader sees the write once");
}

/// A7: refresh, resize and the per-window disable rebuild every view.
#[crate::test]
fn forced_reconstruction(cx: &mut TestAppContext) {
    let panels: Vec<Entity<Panel>> = (0..4).map(|ix| cx.new(|_| Panel { value: ix })).collect();
    let window = cx.add_window(move |_, _| Dashboard { panels });
    let window: AnyWindowHandle = window.into();
    draw(cx, window);
    draw(cx, window);

    reset(cx, window);
    cx.update_window(window, |_, window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    })
    .unwrap();
    assert_eq!(stats(cx, window).views_rendered, 5, "refresh rebuilds all");

    draw(cx, window);
    reset(cx, window);
    cx.simulate_window_resize(window, crate::size(px(300.), px(200.)));
    draw(cx, window);
    assert_eq!(stats(cx, window).views_rendered, 5, "resize rebuilds all");

    draw(cx, window);
    cx.update_window(window, |_, window, _| window.set_retention_enabled(false))
        .unwrap();
    for _ in 0..3 {
        draw(cx, window);
    }
    reset(cx, window);
    draw(cx, window);
    assert_eq!(
        stats(cx, window).views_rendered,
        5,
        "disabled: always rebuild"
    );
    cx.update_window(window, |_, window, _| window.set_retention_enabled(true))
        .unwrap();
    draw(cx, window);
    draw(cx, window);
    reset(cx, window);
    draw(cx, window);
    assert_eq!(stats(cx, window).views_rendered, 0);
}

// ---------------------------------------------------------------------------
// Oracle: retained window versus a window that rebuilds every frame (A1)
// ---------------------------------------------------------------------------

struct Item {
    model: Entity<Model>,
}
impl Render for Item {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let value = self.model.read(cx).value;
        let present = cx.has_global::<Flag>();
        div()
            .w(px(60.))
            .h(px(18.))
            .bg(if present { crate::red() } else { crate::blue() })
            // The label grows from one digit count to the next (9 -> 10, 99 -> 100), so a text
            // measurement that is carried over must be measured again.
            .child(format!("item {value}"))
    }
}

struct Group {
    items: Vec<Entity<Item>>,
    order: Vec<usize>,
}
impl Render for Group {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .p(px(2.))
            .children(self.order.iter().map(|ix| self.items[*ix].clone()))
    }
}

struct OracleRoot {
    groups: Vec<Entity<Group>>,
}
impl Render for OracleRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let flag = cx.try_global::<Flag>().map_or(0, |flag| flag.0);
        let shift = window.modifiers().shift;
        let pointer = window.mouse_position();
        div()
            .size_full()
            .flex()
            .child(format!(
                "flag {flag} shift {shift} x {}",
                (pointer.x.0 / 40.0).floor()
            ))
            .children(self.groups.iter().cloned())
    }
}

fn oracle_signature(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
) -> (Vec<String>, Vec<Vec<crate::Bounds<crate::Pixels>>>) {
    cx.update_window(window, |_, window, _| {
        let hits = [(10., 10.), (70., 30.), (150., 40.), (210., 60.)]
            .into_iter()
            .map(|(x, y)| window.hit_test_bounds(crate::point(px(x), px(y))))
            .collect();
        (window.painted_primitives(), hits)
    })
    .unwrap()
}

#[crate::test]
fn retained_and_full_windows_agree(cx: &mut TestAppContext) {
    use crate::InputEvent as _;
    let models: Vec<Entity<Model>> = [8, 98, 5, 40, 97, 9]
        .into_iter()
        .map(|value| cx.new(|_| Model { value }))
        .collect();
    let items: Vec<Entity<Item>> = models
        .iter()
        .map(|model| {
            let model = model.clone();
            cx.new(move |_| Item { model })
        })
        .collect();
    let groups: Vec<Entity<Group>> = (0..2)
        .map(|g| {
            let items = items[g * 3..g * 3 + 3].to_vec();
            cx.new(move |_| Group {
                items,
                order: vec![0, 1, 2],
            })
        })
        .collect();
    let make_root = {
        let groups = groups.clone();
        move |_: &mut Window, _: &mut Context<OracleRoot>| OracleRoot {
            groups: groups.clone(),
        }
    };
    let retained = cx.add_window(make_root.clone());
    let full = cx.add_window(make_root);
    let (root_retained, root_full) = (retained.root(cx).unwrap(), full.root(cx).unwrap());
    let retained: AnyWindowHandle = retained.into();
    let full: AnyWindowHandle = full.into();
    cx.update_window(full, |_, window, _| window.set_retention_enabled(false))
        .unwrap();

    let mut rng = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move |bound: u64| {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng % bound
    };
    let mut reused_something = false;
    let mut log: Vec<String> = Vec::new();
    for step in 0..120 {
        for window in [retained, full] {
            cx.update_window(window, |_, window, _| {
                window.reset_retention_stats();
                window.reset_layout_stats();
            })
            .unwrap();
        }
        let action = next(10);
        log.push(format!("{step}: action {action}"));
        match action {
            0 | 1 => {
                let model = &models[next(6) as usize];
                log.push(format!(
                    " model notified {}",
                    models.iter().position(|m| m == model).unwrap()
                ));
                model.update(cx, |model, cx| {
                    model.value += 1;
                    cx.notify();
                });
            }
            2 => {
                let model = &models[next(6) as usize];
                model.update(cx, |model, _| model.value += 1);
                for root in [&root_retained, &root_full] {
                    root.update(cx, |_, cx| cx.notify());
                }
            }
            3 => cx.update(|cx| cx.set_global(Flag(next(5) as u32))),
            4 => cx.update(|cx| {
                if cx.has_global::<Flag>() {
                    cx.remove_global::<Flag>();
                }
            }),
            5 => {
                let shift = next(2) == 0;
                for window in [retained, full] {
                    cx.update_window(window, |_, window, cx| {
                        window.dispatch_event(
                            crate::ModifiersChangedEvent {
                                modifiers: crate::Modifiers {
                                    shift,
                                    ..Default::default()
                                },
                                capslock: Default::default(),
                            }
                            .to_platform_input(),
                            cx,
                        );
                    })
                    .unwrap();
                }
            }
            6 | 7 => {
                let x = next(240) as f32;
                for window in [retained, full] {
                    cx.update_window(window, |_, window, cx| {
                        window.dispatch_event(
                            crate::MouseMoveEvent {
                                position: crate::point(px(x), px(20.)),
                                pressed_button: None,
                                modifiers: Default::default(),
                            }
                            .to_platform_input(),
                            cx,
                        );
                    })
                    .unwrap();
                }
            }
            8 => {
                let group = &groups[next(2) as usize];
                let (a, b) = (next(3) as usize, next(3) as usize);
                group.update(cx, |group, cx| {
                    group.order.swap(a, b);
                    cx.notify();
                });
            }
            _ => {
                for window in [retained, full] {
                    cx.update_window(window, |_, window, _| window.refresh())
                        .unwrap();
                }
            }
        }
        draw(cx, retained);
        draw(cx, full);
        for (name, window) in [("retained", retained), ("full", full)] {
            let layout = cx
                .update_window(window, |_, window, _| window.layout_stats())
                .unwrap();
            let views = cx
                .update_window(window, |_, window, _| window.retention_stats())
                .unwrap();
            log.push(format!(
                " {name}: created {} reused {} writes {}/{}/{} computed {} skipped {} rendered {} reused {}",
                layout.nodes_created,
                layout.nodes_reused,
                layout.style_writes,
                layout.children_writes,
                layout.measure_writes,
                layout.computed_layouts,
                layout.skipped_layouts,
                views.views_rendered,
                views.views_reused
            ));
        }
        reused_something |= stats(cx, retained).views_reused > 0;
        let (retained_scene, retained_hits) = oracle_signature(cx, retained);
        let (full_scene, full_hits) = oracle_signature(cx, full);
        if retained_scene != full_scene {
            let at = retained_scene
                .iter()
                .zip(&full_scene)
                .position(|(a, b)| a != b);
            panic!(
                "scene differs at step {step} (first difference at {at:?}) after {:?}:\nRETAINED {:?}\nFULL {:?}",
                log.iter().rev().take(9).collect::<Vec<_>>(),
                retained_scene
                    .iter()
                    .map(|line| line.chars().take(150).collect::<String>())
                    .collect::<Vec<_>>(),
                full_scene
                    .iter()
                    .map(|line| line.chars().take(150).collect::<String>())
                    .collect::<Vec<_>>(),
            );
        }
        assert_eq!(retained_hits, full_hits, "hit tests differ at step {step}");
    }
    assert!(reused_something, "the history never exercised replay");
}

// ---------------------------------------------------------------------------
// Splices, moved views, nesting depth and input queries
// ---------------------------------------------------------------------------

struct SizedChild {
    model: Entity<Model>,
    renders: Rc<Cell<u32>>,
}
impl Render for SizedChild {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        let value = self.model.read(cx).value;
        div().w(px(40. + value as f32)).h(px(10.))
    }
}

struct ChildHolder {
    child: Entity<SizedChild>,
}
impl Render for ChildHolder {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().flex().child(self.child.clone())
    }
}

struct HolderRoot {
    holder: Entity<ChildHolder>,
}
impl Render for HolderRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.holder.clone())
    }
}

/// A2: a splice that fails because the changed child's size changed constructs that child
/// exactly once; the fallback reconstruction of its parents adopts it (R6).
#[crate::test]
fn failed_splice_adopts_the_child_instead_of_rendering_it_twice(cx: &mut TestAppContext) {
    let model = cx.new(|_| Model { value: 0 });
    let renders = counter();
    let window = cx.add_window({
        let (model, renders) = (model.clone(), renders.clone());
        move |_, cx| {
            let child = cx.new(move |_| SizedChild { model, renders });
            let holder = cx.new(move |_| ChildHolder { child });
            HolderRoot { holder }
        }
    });
    let window: AnyWindowHandle = window.into();
    draw(cx, window);
    draw(cx, window);
    let baseline = renders.get();
    reset(cx, window);

    model.update(cx, |model, cx| {
        model.value = 25;
        cx.notify();
    });
    draw(cx, window);
    assert_eq!(
        renders.get(),
        baseline + 1,
        "the resized child was constructed once"
    );
    let s = stats(cx, window);
    assert!(s.splice_failures >= 1, "{s:?}");

    // The window is stable again afterwards.
    reset(cx, window);
    draw(cx, window);
    assert_eq!(stats(cx, window).views_rendered, 0);
}

struct Spacer {
    height: Entity<Model>,
    panel: Entity<MovedPanel>,
}
impl Render for Spacer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let height = self.height.read(cx).value as f32;
        div()
            .size_full()
            .child(div().w(px(10.)).h(px(10. + height)))
            .child(self.panel.clone())
    }
}

struct MovedPanel {
    inner: Entity<ModelReader>,
}
impl Render for MovedPanel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(80.)).h(px(30.)).child(self.inner.clone())
    }
}

/// R6: a clean parent that moved while one of its children is dirty is built again in place
/// and adopts the child built for the abandoned splice (the child is rendered once).
#[crate::test]
fn moved_parent_with_a_dirty_child_renders_the_child_once(cx: &mut TestAppContext) {
    let height = cx.new(|_| Model { value: 0 });
    let inner_model = cx.new(|_| Model { value: 0 });
    let inner_renders = counter();
    let window = cx.add_window({
        let (height, inner_model, inner_renders) =
            (height.clone(), inner_model.clone(), inner_renders.clone());
        move |_, cx| {
            let inner = cx.new(move |_| ModelReader {
                model: inner_model,
                renders: inner_renders,
            });
            let panel = cx.new(move |_| MovedPanel { inner });
            Spacer { height, panel }
        }
    });
    let window: AnyWindowHandle = window.into();
    draw(cx, window);
    draw(cx, window);
    let baseline = inner_renders.get();

    // Both updates are applied before the window draws.
    cx.update(|cx| {
        height.update(cx, |model, cx| {
            model.value = 12;
            cx.notify();
        });
        inner_model.update(cx, |model, cx| {
            model.value = 5;
            cx.notify();
        });
    });
    draw(cx, window);
    assert_eq!(
        inner_renders.get(),
        baseline + 1,
        "the dirty child was rendered once"
    );
}

struct Leaf {
    model: Entity<Model>,
}
impl Render for Leaf {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let value = self.model.read(cx).value;
        div()
            .w(px(60.))
            .h(px(12.))
            .child(format!("leaf {:02}", value % 100))
    }
}

struct Middle {
    leaf: Entity<Leaf>,
}
impl Render for Middle {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(100.))
            .h(px(40.))
            .p(px(4.))
            .child(self.leaf.clone())
    }
}

struct Outer {
    middles: Vec<Entity<Middle>>,
}
impl Render for Outer {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_wrap()
            .size_full()
            .children(self.middles.iter().cloned())
    }
}

/// A8: three levels of views stay replayable (nested records rebased) over several
/// consecutive frames in which only the innermost view is dirty.
#[crate::test]
fn nested_views_replay_correctly_over_consecutive_frames(cx: &mut TestAppContext) {
    let models: Vec<Entity<Model>> = (0..4).map(|ix| cx.new(|_| Model { value: ix })).collect();
    let build = {
        let models = models.clone();
        move |_: &mut Window, cx: &mut Context<Outer>| Outer {
            middles: models
                .iter()
                .map(|model| {
                    let model = model.clone();
                    let leaf = cx.new(move |_| Leaf { model });
                    cx.new(move |_| Middle { leaf })
                })
                .collect(),
        }
    };
    let retained = cx.add_window(build.clone());
    let full = cx.add_window(build);
    let (retained, full): (AnyWindowHandle, AnyWindowHandle) = (retained.into(), full.into());
    cx.update_window(full, |_, window, _| window.set_retention_enabled(false))
        .unwrap();
    draw(cx, retained);
    draw(cx, full);

    for step in 0..6u32 {
        let target = &models[(step % 4) as usize];
        reset(cx, retained);
        target.update(cx, |model, cx| {
            model.value += 10;
            cx.notify();
        });
        draw(cx, retained);
        draw(cx, full);
        let s = stats(cx, retained);
        assert_eq!(
            s.views_rendered, 1,
            "step {step}: only the leaf renders: {s:?}"
        );
        assert!(s.views_reused >= 1, "step {step}: {s:?}");
        let (retained_scene, retained_hits) = oracle_signature(cx, retained);
        let (full_scene, full_hits) = oracle_signature(cx, full);
        assert_eq!(retained_scene, full_scene, "step {step}");
        assert_eq!(retained_hits, full_hits, "step {step}");
    }
}

/// A7: an active drag and the first frame after accessibility activation each rebuild every
/// view (R7).
#[crate::test]
fn active_drag_and_first_accessibility_frame_rebuild_every_view(cx: &mut TestAppContext) {
    let panels: Vec<Entity<Panel>> = (0..4).map(|ix| cx.new(|_| Panel { value: ix })).collect();
    let window = cx.add_window(move |_, _| Dashboard { panels });
    let window: AnyWindowHandle = window.into();
    draw(cx, window);
    draw(cx, window);

    cx.update(|cx| {
        let view = cx.new(|_| crate::EmptyView).into();
        cx.active_drag = Some(crate::AnyDrag {
            view,
            value: std::sync::Arc::new(1u32),
            cursor_offset: Default::default(),
            cursor_style: None,
            external_payload_source: None,
        });
    });
    reset(cx, window);
    draw(cx, window);
    assert_eq!(
        stats(cx, window).views_rendered,
        6,
        "an active drag rebuilds all four panels, the dashboard and renders the dragged view"
    );
    cx.update(|cx| cx.active_drag = None);
    draw(cx, window);
    draw(cx, window);

    cx.update_window(window, |_, window, _| window.a11y.set_active_for_test(true))
        .unwrap();
    reset(cx, window);
    draw(cx, window);
    assert_eq!(
        stats(cx, window).views_rendered,
        5,
        "the first frame after accessibility activation rebuilds all"
    );
    reset(cx, window);
    draw(cx, window);
    assert_eq!(
        stats(cx, window).views_rendered,
        5,
        "while accessibility is active no view is replayed: replayed views would not contribute \
         their accessibility nodes until A11Y-002 records them"
    );
    cx.update_window(window, |_, window, _| {
        window.a11y.set_active_for_test(false)
    })
    .unwrap();
    draw(cx, window);
    reset(cx, window);
    draw(cx, window);
    assert_eq!(
        stats(cx, window).views_rendered,
        0,
        "frames are retained again once accessibility is inactive"
    );
}

struct QueryField {
    focus_handle: crate::FocusHandle,
    notify_on_query: Rc<Cell<bool>>,
    queries: u32,
}
impl Render for QueryField {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let focus_handle = self.focus_handle.clone();
        div()
            .w(px(100.))
            .h(px(20.))
            .track_focus(&self.focus_handle)
            .child(
                crate::canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        window.handle_input(
                            &focus_handle,
                            crate::ElementInputHandler::new(bounds, view),
                            cx,
                        );
                    },
                )
                .size_full(),
            )
    }
}
impl crate::EntityInputHandler for QueryField {
    fn text_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: &mut Option<std::ops::Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<crate::UTF16Selection> {
        // A read-only query that nevertheless takes `&mut self`.
        self.queries += 1;
        if self.notify_on_query.get() {
            cx.notify();
        }
        None
    }
    fn marked_text_range(
        &self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        None
    }
    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}
    fn replace_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        _: &str,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        _: &str,
        _: Option<std::ops::Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
    }
    fn bounds_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: crate::Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<crate::Bounds<Pixels>> {
        None
    }
    fn character_index_for_point(
        &mut self,
        _: crate::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

struct FieldWatcher {
    field: Entity<QueryField>,
    renders: Rc<Cell<u32>>,
}
impl Render for FieldWatcher {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        let queries = self.field.read(cx).queries;
        div()
            .w(px(80.))
            .h(px(10.))
            .child(format!("queries {queries}"))
    }
}

struct FieldWindow {
    field: Entity<QueryField>,
    watcher: Entity<FieldWatcher>,
}
impl Render for FieldWindow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(self.field.clone())
            .child(self.watcher.clone())
    }
}

/// A3: read-only selection/IME queries keep retention active; they invalidate readers of the
/// input only when the query explicitly notifies (R4).
#[crate::test]
fn input_queries_invalidate_only_when_they_notify(cx: &mut TestAppContext) {
    use crate::PlatformWindow as _;
    let notify_on_query = Rc::new(Cell::new(false));
    let renders = counter();
    let handle = cx.add_window({
        let (notify_on_query, renders) = (notify_on_query.clone(), renders.clone());
        move |_, cx| {
            let field = cx.new({
                let notify_on_query = notify_on_query.clone();
                move |cx| QueryField {
                    focus_handle: cx.focus_handle(),
                    notify_on_query,
                    queries: 0,
                }
            });
            let watcher = cx.new({
                let field = field.clone();
                move |_| FieldWatcher { field, renders }
            });
            FieldWindow { field, watcher }
        }
    });
    let root = handle.root(cx).unwrap();
    let mut test_window = cx.test_window(handle.into());
    let window: AnyWindowHandle = handle.into();
    let focus_handle = root.read_with(cx, |root, cx| root.field.read(cx).focus_handle.clone());
    cx.update_window(window, |_, window, cx| window.focus(&focus_handle, cx))
        .unwrap();
    draw(cx, window);
    draw(cx, window);
    let baseline = renders.get();

    let mut query = |cx: &mut TestAppContext| {
        let mut handler = test_window
            .take_input_handler()
            .expect("the focused field registered an input handler");
        handler.selected_text_range(true);
        test_window.set_input_handler(handler);
        reset(cx, window);
        draw(cx, window);
    };
    query(cx);
    query(cx);
    assert_eq!(
        renders.get(),
        baseline,
        "queries alone do not invalidate readers"
    );
    assert_eq!(stats(cx, window).views_rendered, 0);

    notify_on_query.set(true);
    query(cx);
    assert_eq!(
        renders.get(),
        baseline + 1,
        "a notifying query invalidates its readers"
    );
}
