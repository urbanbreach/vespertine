//! CMP-004 A10 and A9 (native part): text tooltips on the base show/hide mechanism.

use std::time::Duration;
use vespertine::{
    Context, InteractiveElement, IntoElement, Modifiers, ParentElement, Render, Styled,
    TestAppContext, VisualTestContext, Window, div, point, px,
};
use vespertine_components::tooltip::text::{TextTooltipExt, TooltipSpec};

struct Page {
    spec: TooltipSpec,
}

impl Render for Page {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            div()
                .id("trigger")
                .debug_selector(|| "trigger".to_string())
                .absolute()
                .left(px(100.))
                .top(px(100.))
                .w(px(40.))
                .h(px(20.))
                .tooltip_spec(self.spec.clone()),
        )
    }
}

fn setup(cx: &mut TestAppContext, spec: TooltipSpec) -> &mut VisualTestContext {
    let (_, vcx) = cx.add_window_view(move |_, _| Page { spec });
    vcx.update(|window, _| window.activate_window());
    vcx.run_until_parked();
    vcx
}

fn pass(cx: &mut VisualTestContext, ms: u64) {
    cx.executor().advance_clock(Duration::from_millis(ms));
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn shown(cx: &mut VisualTestContext) -> bool {
    cx.debug_bounds("text-tooltip").is_some()
}

fn hover(cx: &mut VisualTestContext, x: f32, y: f32) {
    cx.simulate_mouse_move(point(px(x), px(y)), None, Modifiers::default());
}

#[vespertine::test]
fn a_text_tooltip_shows_after_the_default_500_ms_and_a_non_hoverable_one_hides_on_leave(
    cx: &mut TestAppContext,
) {
    let cx = setup(cx, TooltipSpec::new("Save the document"));
    hover(cx, 110., 110.);
    pass(cx, 499);
    assert!(!shown(cx), "not yet at 499 ms");
    pass(cx, 1);
    assert!(shown(cx), "shown at 500 ms");
    hover(cx, 300., 300.);
    pass(cx, 0);
    assert!(
        !shown(cx),
        "a non-hoverable tooltip hides as soon as the pointer leaves"
    );
}

#[vespertine::test]
fn the_show_delay_is_overridable_per_element(cx: &mut TestAppContext) {
    let cx = setup(
        cx,
        TooltipSpec::new("Fast").show_delay(Duration::from_millis(100)),
    );
    hover(cx, 110., 110.);
    pass(cx, 100);
    assert!(shown(cx));
}

#[vespertine::test]
fn a_hoverable_tooltip_stays_while_hovered_and_hides_500_ms_after_leaving(cx: &mut TestAppContext) {
    let cx = setup(cx, TooltipSpec::new("More detail").hoverable());
    hover(cx, 110., 110.);
    pass(cx, 500);
    assert!(shown(cx));
    let bounds = cx.debug_bounds("text-tooltip").expect("tooltip bounds");
    // Move onto the tooltip itself: it stays.
    hover(
        cx,
        f32::from(bounds.center().x),
        f32::from(bounds.center().y),
    );
    pass(cx, 2000);
    assert!(shown(cx), "stays while the pointer is on it");
    // Leave both: it hides after 500 ms and not before.
    hover(cx, 600., 400.);
    pass(cx, 499);
    assert!(shown(cx), "still shown 499 ms after leaving");
    pass(cx, 1);
    assert!(!shown(cx), "hidden 500 ms after leaving");
}

#[vespertine::test]
fn a_returning_pointer_cancels_the_hoverable_hide(cx: &mut TestAppContext) {
    let cx = setup(cx, TooltipSpec::new("More detail").hoverable());
    hover(cx, 110., 110.);
    pass(cx, 500);
    hover(cx, 600., 400.);
    pass(cx, 300);
    hover(cx, 110., 110.);
    pass(cx, 1000);
    assert!(shown(cx));
}

#[vespertine::test]
fn a_press_away_hides_the_tooltip(cx: &mut TestAppContext) {
    let cx = setup(cx, TooltipSpec::new("Press me").hoverable());
    hover(cx, 110., 110.);
    pass(cx, 500);
    assert!(shown(cx));
    cx.simulate_click(point(px(500.), px(300.)), Modifiers::default());
    pass(cx, 0);
    assert!(!shown(cx), "an outside press hides the tooltip");
}

#[vespertine::test]
fn the_above_placement_leaves_a_20_px_clearance_over_the_pointer(cx: &mut TestAppContext) {
    let cx = setup(cx, TooltipSpec::new("Above").above_pointer());
    hover(cx, 110., 110.);
    pass(cx, 500);
    let bounds = cx.debug_bounds("text-tooltip").expect("tooltip bounds");
    assert_eq!(
        f32::from(bounds.bottom()),
        90.0,
        "bottom edge 20 px above the pointer"
    );
    assert_eq!(f32::from(bounds.left()), 110.0);
}

#[vespertine::test]
fn the_default_placement_is_below_and_right_of_the_pointer(cx: &mut TestAppContext) {
    let cx = setup(cx, TooltipSpec::new("Below"));
    hover(cx, 110., 110.);
    pass(cx, 500);
    let bounds = cx.debug_bounds("text-tooltip").expect("tooltip bounds");
    assert_eq!(f32::from(bounds.left()), 111.0);
    assert_eq!(f32::from(bounds.top()), 111.0);
}

#[vespertine::test]
fn a_long_text_wraps_at_the_320_px_maximum(cx: &mut TestAppContext) {
    let long = "word ".repeat(200);
    let cx = setup(cx, TooltipSpec::new(long));
    hover(cx, 110., 110.);
    pass(cx, 500);
    let bounds = cx.debug_bounds("text-tooltip").expect("tooltip bounds");
    assert!(f32::from(bounds.size.width) <= 320.0);
    assert!(
        f32::from(bounds.size.height) > 40.0,
        "the text wrapped onto many lines"
    );
}

#[vespertine::test]
fn wheel_scrolling_away_from_the_tooltip_hides_it(cx: &mut TestAppContext) {
    let cx = setup(cx, TooltipSpec::new("Scroll away").hoverable());
    hover(cx, 110., 110.);
    pass(cx, 500);
    assert!(shown(cx));
    cx.simulate_event(vespertine::ScrollWheelEvent {
        position: point(px(110.), px(110.)),
        delta: vespertine::ScrollDelta::Pixels(point(px(0.), px(-20.))),
        ..Default::default()
    });
    pass(cx, 0);
    assert!(
        !shown(cx),
        "a wheel scroll that is not over the tooltip hides it"
    );
}
