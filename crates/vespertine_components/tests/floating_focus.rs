//! CMP-002 R9-R11: opening focus after attach, closing focus restoration and the focus-target
//! contract, through the dismissal-layer API the dialog, select and palette components use.

use std::{cell::Cell, rc::Rc};
use vespertine::{
    AppContext as _, Context, FocusHandle, InteractiveElement, IntoElement, ParentElement, Render,
    Styled, Window, div, px,
};
use vespertine_components::floating::{
    EscapeOutcome, FocusPick, FocusTarget, LayerHandle, LayerOptions, layers, open_layer,
};
use vespertine_test_host::{HostOptions, TestHost, require_backend};

struct Focusables {
    prior: FocusHandle,
    trigger: FocusHandle,
    layer: FocusHandle,
    other: FocusHandle,
    show_prior: Rc<Cell<bool>>,
}

impl Render for Focusables {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(div().id("trigger").size(px(20.)).track_focus(&self.trigger))
            .child(div().id("other").size(px(20.)).track_focus(&self.other))
            .child(div().id("layer").size(px(20.)).track_focus(&self.layer))
            .when(self.show_prior.get(), |d| {
                d.child(div().id("prior").size(px(20.)).track_focus(&self.prior))
            })
    }
}

use vespertine::prelude::FluentBuilder as _;

struct Rig {
    host: TestHost,
    view: vespertine::Entity<Focusables>,
    show_prior: Rc<Cell<bool>>,
}

fn rig() -> Rig {
    let show_prior = Rc::new(Cell::new(true));
    let flag = show_prior.clone();
    let slot = Rc::new(Cell::new(None));
    let capture = slot.clone();
    let host = TestHost::with_view(HostOptions::with_size(200., 100.), move |_, cx| {
        let view = cx.new(|cx| Focusables {
            prior: cx.focus_handle(),
            trigger: cx.focus_handle(),
            layer: cx.focus_handle(),
            other: cx.focus_handle(),
            show_prior: flag,
        });
        capture.set(Some(view.clone()));
        view
    })
    .expect("host");
    let view = slot.take().expect("view");
    let mut rig = Rig {
        host,
        view,
        show_prior,
    };
    rig.host.flush();
    rig
}

impl Rig {
    fn handles(&mut self) -> [FocusHandle; 4] {
        let view = self.view.clone();
        self.host.app().update(|cx| {
            let v = view.read(cx);
            [
                v.prior.clone(),
                v.trigger.clone(),
                v.layer.clone(),
                v.other.clone(),
            ]
        })
    }

    fn focus(&mut self, handle: &FocusHandle) {
        let window = self.host.window();
        let handle = handle.clone();
        self.host
            .app()
            .update_window(window, |_, window, cx| handle.focus(window, cx))
            .unwrap();
        self.host.flush();
    }

    fn is_focused(&mut self, handle: &FocusHandle) -> bool {
        let window = self.host.window();
        let handle = handle.clone();
        self.host
            .app()
            .update_window(window, |_, window, _| handle.is_focused(window))
            .unwrap()
    }

    fn nothing_focused(&mut self) -> bool {
        let window = self.host.window();
        self.host
            .app()
            .update_window(window, |_, window, cx| window.focused(cx).is_none())
            .unwrap()
    }

    fn open(&mut self, initial: FocusTarget, finally: FocusTarget) -> LayerHandle {
        self.open_owned(initial, finally, true)
    }

    fn open_owned(
        &mut self,
        initial: FocusTarget,
        finally: FocusTarget,
        with_trigger: bool,
    ) -> LayerHandle {
        let [_, trigger, layer, _] = self.handles();
        let window = self.host.window();
        let handle = self
            .host
            .app()
            .update_window(window, |_, window, cx| {
                open_layer(
                    window,
                    cx,
                    LayerOptions {
                        escape: Rc::new(|_, _| EscapeOutcome::Dismissed),
                        dismiss: Rc::new(|_, _| {}),
                        layer_focus: layer,
                        trigger: with_trigger.then_some(trigger),
                        initial_focus: initial,
                        final_focus: finally,
                        trigger_first: false,
                    },
                )
            })
            .unwrap();
        self.host.flush();
        handle
    }

    fn close(&mut self, handle: &LayerHandle) -> bool {
        let window = self.host.window();
        let handle = handle.clone();
        let closed = self
            .host
            .app()
            .update_window(window, |_, window, cx| handle.close(window, cx))
            .unwrap();
        self.host.flush();
        closed
    }
}

#[test]
fn default_focus_moves_into_the_layer_after_attach_and_returns_on_close() {
    require_backend!();
    let mut rig = rig();
    let [prior, _, layer, _] = rig.handles();
    rig.focus(&prior);
    let handle = rig.open(FocusTarget::Default, FocusTarget::Default);
    assert!(rig.is_focused(&layer), "the layer's own default");
    assert_eq!(handle.prior_focus(), Some(&prior), "captured at open time");
    assert!(rig.close(&handle));
    assert!(rig.is_focused(&prior), "back to the captured owner");
    assert!(!rig.close(&handle), "closing twice does nothing");
}

#[test]
fn initial_focus_targets_follow_the_contract() {
    require_backend!();
    let mut rig = rig();
    let [prior, _, layer, other] = rig.handles();

    // A handle.
    rig.focus(&prior);
    let handle = rig.open(FocusTarget::Handle(other.clone()), FocusTarget::Default);
    assert!(rig.is_focused(&other));
    rig.close(&handle);

    // Suppression (null / false): focus stays where it was.
    rig.focus(&prior);
    let handle = rig.open(FocusTarget::Suppress, FocusTarget::Suppress);
    assert!(rig.is_focused(&prior));
    rig.close(&handle);
    assert!(rig.is_focused(&prior));

    // A function: receives the captured prior focus; `Default` selects the layer's default.
    rig.focus(&prior);
    let seen = Rc::new(Cell::new(false));
    let probe = seen.clone();
    let expected = prior.clone();
    let handle = rig.open(
        FocusTarget::With(Rc::new(move |captured, _, _| {
            probe.set(captured == Some(&expected));
            FocusPick::Default
        })),
        FocusTarget::Default,
    );
    assert!(seen.get());
    assert!(rig.is_focused(&layer));
    rig.close(&handle);

    // A function returning null leaves focus alone.
    rig.focus(&prior);
    let handle = rig.open(
        FocusTarget::With(Rc::new(|_, _, _| FocusPick::Suppress)),
        FocusTarget::Default,
    );
    assert!(rig.is_focused(&prior));
    rig.close(&handle);
}

#[test]
fn final_focus_overrides_and_suppression_leaves_focus_in_the_layer() {
    require_backend!();
    let mut rig = rig();
    let [prior, _, layer, other] = rig.handles();
    rig.focus(&prior);
    let handle = rig.open(FocusTarget::Default, FocusTarget::Handle(other.clone()));
    rig.close(&handle);
    assert!(rig.is_focused(&other), "final focus handle wins");

    rig.focus(&prior);
    let handle = rig.open(FocusTarget::Default, FocusTarget::Suppress);
    assert!(rig.is_focused(&layer));
    rig.close(&handle);
    assert!(
        rig.is_focused(&layer),
        "a null result leaves focus unchanged"
    );
}

#[test]
fn closing_keeps_focus_that_moved_elsewhere_and_falls_back_when_the_owner_is_gone() {
    require_backend!();
    let mut rig = rig();
    let [prior, trigger, _, other] = rig.handles();
    rig.focus(&prior);
    let handle = rig.open(FocusTarget::Default, FocusTarget::Default);
    rig.focus(&other);
    rig.close(&handle);
    assert!(
        rig.is_focused(&other),
        "a press that moved focus keeps what it obtained"
    );

    // The captured owner disappears: the owning trigger takes focus.
    rig.focus(&prior);
    let handle = rig.open(FocusTarget::Default, FocusTarget::Default);
    rig.show_prior.set(false);
    rig.host.invalidate();
    rig.host.flush();
    rig.close(&handle);
    assert!(rig.is_focused(&trigger), "fallback: the owning trigger");

    // Neither the captured owner nor a trigger: the window's default (no focus).
    rig.show_prior.set(true);
    rig.host.invalidate();
    rig.host.flush();
    rig.focus(&prior);
    let handle = rig.open_owned(FocusTarget::Default, FocusTarget::Default, false);
    rig.show_prior.set(false);
    rig.host.invalidate();
    rig.host.flush();
    rig.close(&handle);
    assert!(
        rig.nothing_focused(),
        "fallback: the window's default focus target"
    );
}

#[test]
fn layers_register_in_order_and_leave_the_stack_on_close() {
    require_backend!();
    let mut rig = rig();
    let first = rig.open(FocusTarget::Default, FocusTarget::Default);
    let second = rig.open(FocusTarget::Default, FocusTarget::Default);
    let window = rig.host.window();
    let stack = rig
        .host
        .app()
        .update_window(window, |_, window, cx| layers(window, cx))
        .unwrap();
    assert_eq!(stack, vec![first.id(), second.id()]);
    rig.close(&second);
    let stack = rig
        .host
        .app()
        .update_window(window, |_, window, cx| layers(window, cx))
        .unwrap();
    assert_eq!(stack, vec![first.id()]);
    rig.close(&first);
}
