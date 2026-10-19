//! APP-005 R4/R5/R7 (A4): the mapping of window options onto Vespertine window options,
//! checked against the test platform's display.

use vespertine::layer_shell::{Anchor, KeyboardInteractivity, Layer};
use vespertine::{
    Bounds, TestAppContext, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, px,
};
use vespertine_js::WindowConfig;

fn map(json: &str, cx: &TestAppContext) -> WindowOptions {
    let config: WindowConfig = serde_json::from_str(json).unwrap();
    let resolved = config.resolve().unwrap();
    cx.update(|cx| resolved.to_window_options(cx))
}

fn windowed(options: &WindowOptions) -> Bounds<vespertine::Pixels> {
    match options.window_bounds {
        Some(WindowBounds::Windowed(bounds)) => bounds,
        ref other => panic!("expected a windowed window, got {other:?}"),
    }
}

#[test]
fn the_default_window_is_800_by_600_and_centered_on_the_primary_display() {
    let cx = TestAppContext::single();
    let options = map("{}", &cx);
    let bounds = windowed(&options);
    assert_eq!(bounds.size.width, px(800.));
    assert_eq!(bounds.size.height, px(600.));
    let display = cx.update(|cx| cx.primary_display().unwrap().visible_bounds());
    assert_eq!(bounds.center(), display.center());
    assert!(options.is_resizable && options.focus && options.show);
    assert!(matches!(options.kind, WindowKind::Normal));
    assert_eq!(
        options.window_background,
        WindowBackgroundAppearance::Opaque
    );
    assert!(options.window_min_size.is_none());
}

#[test]
fn the_title_size_and_flags_are_forwarded() {
    let cx = TestAppContext::single();
    let options = map(
        r#"{"title":"Hello","width":300,"height":200,"resizable":false,"focus":false,"show":false,
            "titlebarTransparent":true}"#,
        &cx,
    );
    let bounds = windowed(&options);
    assert_eq!(
        (bounds.size.width, bounds.size.height),
        (px(300.), px(200.))
    );
    let titlebar = options.titlebar.as_ref().unwrap();
    assert_eq!(titlebar.title.as_deref(), Some("Hello"));
    assert!(titlebar.appears_transparent);
    assert!(!options.is_resizable && !options.focus && !options.show);
}

#[test]
fn fullscreen_true_opens_fullscreen() {
    let cx = TestAppContext::single();
    let options = map(r#"{"fullscreen":true}"#, &cx);
    assert!(matches!(
        options.window_bounds,
        Some(WindowBounds::Fullscreen(_))
    ));
}

#[test]
fn backgrounds_follow_the_kind_then_the_transparency_flag() {
    let cx = TestAppContext::single();
    assert_eq!(
        map(r#"{"transparent":true}"#, &cx).window_background,
        WindowBackgroundAppearance::Transparent
    );
    assert_eq!(
        map(r#"{"transparent":true,"windowBackground":"opaque"}"#, &cx).window_background,
        WindowBackgroundAppearance::Opaque
    );
    assert_eq!(
        map(r#"{"windowBackground":"blurred"}"#, &cx).window_background,
        WindowBackgroundAppearance::Blurred
    );
}

#[test]
fn the_minimum_size_applies_only_when_both_values_are_given() {
    let cx = TestAppContext::single();
    assert!(map(r#"{"minWidth":100}"#, &cx).window_min_size.is_none());
    let size = map(r#"{"minWidth":100,"minHeight":50}"#, &cx)
        .window_min_size
        .unwrap();
    assert_eq!((size.width, size.height), (px(100.), px(50.)));
}

#[test]
fn the_application_identity_becomes_the_window_app_id() {
    let cx = TestAppContext::single();
    let options = map(r#"{"appId":"org.example.app"}"#, &cx);
    // The Linux platform sets this as the Wayland `app_id` and the X11 `WM_CLASS`
    // (verified live by the `app_lifecycle` example).
    assert_eq!(options.app_id.as_deref(), Some("org.example.app"));
}

#[test]
fn a_layer_shell_surface_has_no_titlebar_and_opens_at_the_origin() {
    let cx = TestAppContext::single();
    let options = map(
        r#"{"width":400,"height":30,"title":"ignored",
            "layerShell":{"namespace":"bar","layer":"bottom","anchor":["left","right"],
                          "exclusiveZone":30,"exclusiveEdge":"top","margin":[1,2,3,4],
                          "keyboardInteractivity":"exclusive"}}"#,
        &cx,
    );
    assert!(options.titlebar.is_none());
    let bounds = windowed(&options);
    assert_eq!(bounds.origin, vespertine::point(px(0.), px(0.)));
    assert_eq!((bounds.size.width, bounds.size.height), (px(400.), px(30.)));
    let WindowKind::LayerShell(layer) = options.kind else {
        panic!("expected a layer-shell window");
    };
    assert_eq!(layer.namespace, "bar");
    assert_eq!(layer.layer, Layer::Bottom);
    assert_eq!(layer.anchor, Anchor::LEFT | Anchor::RIGHT);
    assert_eq!(layer.exclusive_zone, Some(px(30.)));
    assert_eq!(layer.exclusive_edge, Some(Anchor::TOP));
    assert_eq!(layer.margin, Some((px(1.), px(2.), px(3.), px(4.))));
    assert_eq!(
        layer.keyboard_interactivity,
        KeyboardInteractivity::Exclusive
    );
}
