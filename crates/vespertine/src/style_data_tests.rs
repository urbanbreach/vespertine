//! Tests for ELM-006, serializable layout and interactive styling.

use std::{cell::Cell, ops::Range, rc::Rc};

use serde_json::{Value, json};

use crate::{
    AbsoluteLength, AlignContent, AlignItems, AnyElement, AnyWindowHandle, App, AppContext as _,
    Bounds, BoxShadow, ColorSpace, Context, CursorStyle, DefiniteLength, Display,
    ElementInputHandler, EntityInputHandler, FlexDirection, FocusHandle, FontWeight, GridTemplate,
    GridTemplateMinSize, Hsla, InputEvent as _, InteractiveElement as _, IntoElement,
    JustifyContent, KeyDownEvent, Keystroke, Length, MouseBlocking, MouseButton, MouseDownEvent,
    Outline, Overflow, ParentElement as _, Pixels, PlatformInput, Point, PointerEvents, Position,
    Quad, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _, StyleRefinement,
    Styled as _, TestAppContext, TextOverflow, TextStyleRefinement, UTF16Selection, Window, canvas,
    div, linear_color_stop, linear_gradient, point, px, relative, rgb, size,
};

use super::style_data::{
    HostStyle, HostStyleError, StyleStore, StyleUpdate, cursor_style_from_keyword, style_json_eq,
    style_json_mouse_blocking,
};

const OP: &str = "setStyle";

fn parse(value: Value) -> HostStyle {
    HostStyle::from_json(&value, OP).expect("style should be accepted")
}

fn refine(value: Value) -> StyleRefinement {
    parse(value).to_refinement()
}

fn empty() -> StyleRefinement {
    HostStyle::default().to_refinement()
}

fn rejected(value: Value) -> HostStyleError {
    HostStyle::from_json(&value, OP).expect_err("style should be rejected")
}

fn len(pixels: f32) -> Length {
    px(pixels).into()
}

// ----------------------------------------------------------------------------------------
// A2: dimension grammars
// ----------------------------------------------------------------------------------------

#[test]
fn dimensions_accept_numbers_percentages_auto_and_numeric_strings() {
    let r = refine(json!({
        "width": 100, "height": "50%", "minWidth": "auto", "maxHeight": "120",
    }));
    assert_eq!(r.size.width, Some(len(100.)));
    assert_eq!(r.size.height, Some(relative(0.5).into()));
    assert_eq!(r.min_size.width, Some(Length::Auto));
    assert_eq!(r.max_size.height, Some(len(120.)));
    assert_eq!(r.min_size.height, None);
    assert_eq!(r.max_size.width, None);
}

#[test]
fn width_and_height_snap_999_permille_to_full_size() {
    for text in ["100%", "99.9%", "150%"] {
        let r = refine(json!({ "width": text, "height": text }));
        assert_eq!(r.size.width, Some(relative(1.).into()), "{text}");
        assert_eq!(r.size.height, Some(relative(1.).into()), "{text}");
    }
    let r = refine(json!({ "width": "99.8%" }));
    assert_eq!(r.size.width, Some(relative(0.998).into()));
}

#[test]
fn min_and_max_percentages_never_snap() {
    let r = refine(json!({ "minWidth": "99.95%", "maxWidth": "100%", "minHeight": "99.9%" }));
    assert_eq!(r.min_size.width, Some(relative(0.9995).into()));
    assert_eq!(r.max_size.width, Some(relative(1.).into()));
    assert_eq!(r.min_size.height, Some(relative(0.999).into()));
}

#[test]
fn invalid_dimension_string_rejects_the_whole_style() {
    let error = rejected(json!({ "width": "wide", "opacity": 0.5 }));
    assert_eq!(error.operation(), OP);
    assert!(error.to_string().contains(OP), "{error}");
    assert!(error.message().contains("width"), "{error}");
    for bad in [
        json!("12px"),
        json!("%"),
        json!(true),
        json!([1]),
        json!({}),
    ] {
        rejected(json!({ "height": bad }));
    }
}

// ----------------------------------------------------------------------------------------
// A2: grid, aspect ratio
// ----------------------------------------------------------------------------------------

#[test]
fn grid_counts_round_and_clamp() {
    let columns = |value: f64| refine(json!({ "gridTemplateColumns": value })).grid_cols;
    let template = |repeat| {
        Some(GridTemplate {
            repeat,
            min_size: GridTemplateMinSize::Zero,
        })
    };
    assert_eq!(columns(0.4), template(1));
    assert_eq!(columns(2.6), template(3));
    assert_eq!(columns(100.), template(64));
    assert_eq!(columns(-3.), template(1));
    assert_eq!(columns(4.), template(4));
}

#[test]
fn grid_track_minimums_are_separate_for_columns_and_rows() {
    let r = refine(json!({
        "gridTemplateColumns": 2, "gridColumnMin": "min-content",
        "gridTemplateRows": 3, "gridRowMin": "max-content",
    }));
    assert_eq!(
        r.grid_cols,
        Some(GridTemplate {
            repeat: 2,
            min_size: GridTemplateMinSize::MinContent
        })
    );
    assert_eq!(
        r.grid_rows,
        Some(GridTemplate {
            repeat: 3,
            min_size: GridTemplateMinSize::MaxContent
        })
    );
    let r = refine(json!({ "gridTemplateColumns": 2, "gridColumnMin": "bogus" }));
    assert_eq!(r.grid_cols.unwrap().min_size, GridTemplateMinSize::Zero);
}

#[test]
fn aspect_ratio_requires_a_positive_finite_number() {
    assert_eq!(
        refine(json!({ "aspectRatio": 1.5 })).aspect_ratio,
        Some(1.5)
    );
    for ratio in [
        json!(0),
        json!(-2),
        json!(f64::INFINITY),
        json!(f64::NAN),
        Value::Null,
    ] {
        assert_eq!(refine(json!({ "aspectRatio": ratio })).aspect_ratio, None);
    }
    rejected(json!({ "aspectRatio": "wide" }));
}

// ----------------------------------------------------------------------------------------
// A2: text
// ----------------------------------------------------------------------------------------

fn weight(value: Value) -> Option<f32> {
    refine(json!({ "fontWeight": value }))
        .text
        .font_weight
        .map(|weight| weight.0)
}

#[test]
fn font_weight_numbers_and_strings() {
    assert_eq!(weight(json!(450)), Some(450.));
    assert_eq!(weight(json!(0)), Some(1.));
    assert_eq!(weight(json!(5000)), Some(1000.));
    let table = [
        ("normal", 400.),
        ("400", 400.),
        ("bold", 700.),
        ("700", 700.),
        ("thin", 100.),
        ("extralight", 200.),
        ("extra-light", 200.),
        ("200", 200.),
        ("light", 300.),
        ("medium", 500.),
        ("semibold", 600.),
        ("semi-bold", 600.),
        ("extrabold", 800.),
        ("extra-bold", 800.),
        ("black", 900.),
        ("900", 900.),
        ("  BOLD ", 700.),
        ("SemiBold", 600.),
        ("650", 650.),
        ("2000", 1000.),
        ("0", 1.),
        ("heavy", 400.),
        ("", 400.),
        ("nan", 400.),
    ];
    for (text, expected) in table {
        assert_eq!(weight(json!(text)), Some(expected), "{text:?}");
    }
    assert_eq!(weight(Value::Null), None);
    rejected(json!({ "fontWeight": true }));
}

#[test]
fn line_clamp_ignores_below_one_and_truncates() {
    let clamp = |value: f64| refine(json!({ "lineClamp": value })).text.line_clamp;
    assert_eq!(clamp(0.9), None);
    assert_eq!(clamp(0.), None);
    assert_eq!(clamp(-3.), None);
    assert_eq!(clamp(1.), Some(1));
    assert_eq!(clamp(2.7), Some(2));
}

#[test]
fn text_properties_map_onto_the_text_refinement() {
    let r = refine(json!({
        "fontFamily": "Noto Sans", "fontSize": 14, "fontWeight": "bold", "lineHeight": 20,
        "textAlign": "center", "whiteSpace": "nowrap", "textOverflow": "ellipsis",
        "color": "#ff0000", "textDecoration": "underline",
    }));
    let text = r.text;
    assert_eq!(text.font_family, Some(SharedString::from("Noto Sans")));
    assert_eq!(text.font_size, Some(AbsoluteLength::from(px(14.))));
    assert_eq!(text.font_weight, Some(FontWeight(700.)));
    assert_eq!(text.line_height, Some(DefiniteLength::from(px(20.))));
    assert_eq!(text.text_align, Some(crate::TextAlign::Center));
    assert_eq!(text.white_space, Some(crate::WhiteSpace::Nowrap));
    assert_eq!(
        text.text_overflow,
        Some(TextOverflow::Truncate(SharedString::new_static("…")))
    );
    assert_eq!(text.color, Some(Hsla::from(rgb(0xff0000))));
    assert_eq!(text.underline.map(|u| u.thickness), Some(px(1.)));
    assert!(text.strikethrough.is_none());

    let r = refine(json!({ "textOverflow": "ellipsis-start", "textAlign": "start" }));
    assert_eq!(
        r.text.text_overflow,
        Some(TextOverflow::TruncateStart(SharedString::new_static("…")))
    );
    assert_eq!(r.text.text_align, Some(crate::TextAlign::Left));
    assert_eq!(
        refine(json!({ "textAlign": "right" })).text.text_align,
        Some(crate::TextAlign::Right)
    );
    let r = refine(json!({ "textDecoration": "line-through" }));
    assert_eq!(r.text.strikethrough.map(|s| s.thickness), Some(px(1.)));
    // `none` replaces an inherited decoration with one that paints nothing.
    let r = refine(json!({ "textDecoration": "none" }));
    assert_eq!(r.text.underline.map(|u| u.thickness), Some(px(0.)));
    assert_eq!(r.text.strikethrough.map(|s| s.thickness), Some(px(0.)));
    // Non-positive line heights and invalid text colors are ignored.
    let r = refine(json!({ "lineHeight": 0, "color": "not a color" }));
    assert_eq!(r.text, TextStyleRefinement::default());
    assert_eq!(refine(json!({ "lineHeight": -4 })).text.line_height, None);
}

#[test]
fn unknown_enumeration_strings_leave_defaults_untouched() {
    let r = refine(json!({
        "display": "block", "visibility": "visible", "flexDirection": "diagonal",
        "flexWrap": "sideways", "alignItems": "middle", "alignSelf": "top",
        "alignContent": "normal", "justifyContent": "stretch", "position": "sticky",
        "textAlign": "justify", "whiteSpace": "pre", "textOverflow": "clip",
        "textDecoration": "overline", "overflow": "auto", "overflowX": "visible",
        "borderStyle": "dotted", "cursor": "Pointer", "pointerEvents": "all",
        "userSelect": "all", "gridColumnMin": "wide",
    }));
    assert_eq!(r, empty());
}

#[test]
fn layout_enumerations_map_onto_upstream_values() {
    let r = refine(json!({
        "display": "grid", "flexDirection": "column", "flexWrap": "wrap-reverse",
        "alignItems": "flex-end", "alignSelf": "baseline", "alignContent": "space-between",
        "justifyContent": "evenly", "position": "fixed", "visibility": "hidden",
        "flexGrow": 2, "flexShrink": 0, "flexBasis": 30, "opacity": 0.25,
    }));
    assert_eq!(r.display, Some(Display::Grid));
    assert_eq!(r.flex_direction, Some(FlexDirection::Column));
    assert_eq!(r.flex_wrap, Some(crate::FlexWrap::WrapReverse));
    assert_eq!(r.align_items, Some(AlignItems::FlexEnd));
    assert_eq!(r.align_self, Some(crate::AlignSelf::Baseline));
    assert_eq!(r.align_content, Some(AlignContent::SpaceBetween));
    assert_eq!(r.justify_content, Some(JustifyContent::SpaceEvenly));
    assert_eq!(
        r.position,
        Some(Position::Absolute),
        "fixed behaves as absolute"
    );
    assert_eq!(r.visibility, Some(crate::Visibility::Hidden));
    assert_eq!(r.flex_grow, Some(2.));
    assert_eq!(r.flex_shrink, Some(0.));
    assert_eq!(r.flex_basis, Some(len(30.)));
    assert_eq!(r.opacity, Some(0.25));
    assert_eq!(
        refine(json!({ "display": "flex" })).display,
        Some(Display::Flex)
    );
    assert_eq!(
        refine(json!({ "justifyContent": "flex-start" })).justify_content,
        Some(JustifyContent::Start)
    );
}

// ----------------------------------------------------------------------------------------
// A2: backgrounds
// ----------------------------------------------------------------------------------------

fn gradient(angle: Value, first: (&str, Value), second: (&str, Value), space: Value) -> Value {
    json!({
        "type": "linear-gradient", "angle": angle, "colorSpace": space,
        "stops": [
            { "color": first.0, "position": first.1 },
            { "color": second.0, "position": second.1 },
        ],
    })
}

#[test]
fn two_stop_oklab_gradient_background() {
    let r = refine(json!({
        "background": gradient(json!(90), ("#FF0000", json!(0)), ("#0000FF", json!(1)), json!("oklab")),
    }));
    let expected = linear_gradient(
        90.,
        linear_color_stop(rgb(0xFF0000), 0.),
        linear_color_stop(rgb(0x0000FF), 1.),
    )
    .color_space(ColorSpace::Oklab);
    assert_eq!(r.background, Some(expected.into()));

    // The color space defaults to sRGB.
    let r = refine(json!({
        "background": gradient(json!(180.5), ("#fff", json!(0.25)), ("#000", json!(0.75)), Value::Null),
    }));
    let expected = linear_gradient(
        180.5,
        linear_color_stop(rgb(0xFFFFFF), 0.25),
        linear_color_stop(rgb(0x000000), 0.75),
    );
    assert_eq!(r.background, Some(expected.into()));
}

#[test]
fn invalid_backgrounds_are_omitted_while_the_rest_still_applies() {
    let valid = || ("#ff0000", json!(0));
    let end = || ("#0000ff", json!(1));
    let cases = [
        json!("definitely not a color"),
        gradient(json!(90), ("nope", json!(0)), end(), Value::Null),
        gradient(Value::Null, valid(), end(), Value::Null), // a non-finite angle cannot be sent
        gradient(json!("sideways"), valid(), end(), Value::Null),
        gradient(json!(90), ("#ff0000", json!(1.5)), end(), Value::Null),
        gradient(json!(90), ("#ff0000", json!(-0.1)), end(), Value::Null),
        gradient(json!(90), valid(), end(), json!("hsl")),
        json!({ "type": "radial-gradient", "angle": 0, "stops": [] }),
        json!({ "type": "linear-gradient", "angle": 0, "stops": [
            { "color": "#fff", "position": 0 } ] }),
        json!({ "type": "linear-gradient", "angle": 0, "stops": [
            { "color": "#fff", "position": 0 }, { "color": "#000", "position": 0.5 },
            { "color": "#f00", "position": 1 } ] }),
    ];
    for background in cases {
        let r = refine(json!({ "background": background.clone(), "width": 40 }));
        assert_eq!(r.background, None, "{background}");
        assert_eq!(r.size.width, Some(len(40.)), "{background}");
    }
    rejected(json!({ "background": 12 }));
    rejected(json!({ "background": true }));
}

#[test]
fn background_color_overrides_background() {
    let r = refine(json!({ "backgroundColor": "#00ff00", "background": "#ff0000" }));
    assert_eq!(r.background, Some(rgb(0x00ff00).into()));
    let r = refine(json!({ "background": "#ff0000" }));
    assert_eq!(r.background, Some(rgb(0xff0000).into()));
    // An invalid override omits the fill entirely.
    let r = refine(json!({ "backgroundColor": "nope", "background": "#ff0000" }));
    assert_eq!(r.background, None);
    rejected(json!({ "backgroundColor": 4 }));
}

// ----------------------------------------------------------------------------------------
// A2: borders, radii, shadows, outline
// ----------------------------------------------------------------------------------------

#[test]
fn longhands_override_shorthands_regardless_of_key_order() {
    let r = refine(json!({
        "paddingLeft": 1, "padding": 8, "paddingTop": 2,
        "marginRight": 5, "margin": 3,
        "borderBottomWidth": 4, "borderWidth": 1,
        "borderBottomRightRadius": 9, "borderRadius": 6,
        "columnGap": 11, "gap": 7,
        "overflowY": "scroll", "overflow": "hidden",
    }));
    let pad = |pixels: f32| Some(DefiniteLength::from(px(pixels)));
    assert_eq!(r.padding.top, pad(2.));
    assert_eq!(r.padding.right, pad(8.));
    assert_eq!(r.padding.bottom, pad(8.));
    assert_eq!(r.padding.left, pad(1.));
    assert_eq!(r.margin.top, Some(len(3.)));
    assert_eq!(r.margin.right, Some(len(5.)));
    assert_eq!(r.margin.left, Some(len(3.)));
    let width = |pixels: f32| Some(AbsoluteLength::from(px(pixels)));
    assert_eq!(r.border_widths.top, width(1.));
    assert_eq!(r.border_widths.bottom, width(4.));
    assert_eq!(r.corner_radii.top_left, width(6.));
    assert_eq!(r.corner_radii.bottom_right, width(9.));
    assert_eq!(r.corner_radii.bottom_left, width(6.));
    assert_eq!(r.gap.width, pad(11.));
    assert_eq!(r.gap.height, pad(7.));
    assert_eq!(r.overflow.x, Some(Overflow::Hidden));
    assert_eq!(r.overflow.y, Some(Overflow::Scroll));
    let r = refine(json!({ "rowGap": 2 }));
    assert_eq!(r.gap.height, pad(2.));
    assert_eq!(r.gap.width, None);
    let r = refine(json!({ "overflowX": "scroll" }));
    assert_eq!(r.overflow.x, Some(Overflow::Scroll));
    assert_eq!(r.overflow.y, None);
}

#[test]
fn border_widths_clamp_to_zero_and_zero_is_applied() {
    let r = refine(json!({ "borderWidth": 0, "borderLeftWidth": -3 }));
    assert_eq!(r.border_widths.top, Some(AbsoluteLength::from(px(0.))));
    assert_eq!(r.border_widths.left, Some(AbsoluteLength::from(px(0.))));
    let r = refine(json!({ "borderColor": "#808080", "borderStyle": "dashed" }));
    assert_eq!(r.border_color, Some(Hsla::from(rgb(0x808080))));
    assert_eq!(r.border_style, Some(crate::BorderStyle::Dashed));
    assert_eq!(
        refine(json!({ "borderStyle": "solid" })).border_style,
        Some(crate::BorderStyle::Solid)
    );
}

#[test]
fn invalid_shadow_entry_is_dropped_alone() {
    let r = refine(json!({ "boxShadow": [
        { "offsetX": 1, "offsetY": 2, "blurRadius": 3, "spreadRadius": -1, "color": "#00000040", "inset": true },
        { "offsetX": 9, "offsetY": 9, "blurRadius": 9, "color": "not a color" },
        { "offsetY": 4, "blurRadius": -6, "color": "rgba(0, 0, 0, 0.5)" },
        "garbage",
        { "offsetX": "wide", "color": "#000" },
    ]}));
    let shadows = r.box_shadow.expect("shadows applied");
    assert_eq!(shadows.len(), 2);
    assert_eq!(
        shadows[0],
        BoxShadow::new(px(1.), px(2.), rgb_alpha(0, 0x40))
            .blur_radius(px(3.))
            .spread_radius(px(-1.))
            .inset()
    );
    assert_eq!(shadows[1].offset, point(px(0.), px(4.)));
    assert_eq!(
        shadows[1].blur_radius,
        px(0.),
        "negative blur clamps to zero"
    );
    assert!(!shadows[1].inset);

    let r = refine(json!({ "boxShadow": [{ "color": "nope" }] }));
    assert_eq!(r.box_shadow, None, "no entry left means no shadow");
    let r =
        refine(json!({ "boxShadow": { "offsetY": 4, "blurRadius": 12, "color": "#00000033" } }));
    assert_eq!(r.box_shadow.map(|shadows| shadows.len()), Some(1));
    rejected(json!({ "boxShadow": "big" }));
}

fn rgb_alpha(rgb_value: u32, alpha: u8) -> Hsla {
    let mut color: crate::Rgba = rgb(rgb_value);
    color.a = alpha as f32 / 255.;
    color.into()
}

#[test]
fn outline_defaults() {
    let outline = |value: Value| refine(value).outline;
    assert_eq!(outline(json!({})), None);
    assert_eq!(
        outline(json!({ "outlineOffset": 4 })),
        None,
        "offset alone enables nothing"
    );
    // A bare width paints nothing: the color defaults to fully transparent.
    let bare_width = outline(json!({ "outlineWidth": 3 })).unwrap();
    assert_eq!(bare_width.width, px(3.));
    assert!(bare_width.color.is_transparent());
    assert_eq!(bare_width.offset, px(0.));
    assert!(!bare_width.is_visible());
    // A bare color gets width 1 and offset 0.
    assert_eq!(
        outline(json!({ "outlineColor": "#60A5FA" })),
        Some(Outline::new(px(1.), rgb(0x60A5FA), px(0.)))
    );
    // Negative widths clamp to zero, the offset is signed, an invalid color is transparent.
    let negative =
        outline(json!({ "outlineWidth": -2, "outlineColor": "#fff", "outlineOffset": -5 }))
            .unwrap();
    assert_eq!(negative.width, px(0.));
    assert_eq!(negative.offset, px(-5.));
    let invalid = outline(json!({ "outlineWidth": 2, "outlineColor": "bogus" })).unwrap();
    assert!(invalid.color.is_transparent());
    // A zero width is applied (not skipped) so it hides an outline of the base style.
    let hidden = outline(json!({ "outlineWidth": 0 })).unwrap();
    assert_eq!(hidden.width, px(0.));
    assert!(!hidden.is_visible());
    rejected(json!({ "outlineColor": 5 }));
    rejected(json!({ "outlineOffset": "far" }));
}

// ----------------------------------------------------------------------------------------
// A2: cursor
// ----------------------------------------------------------------------------------------

#[test]
fn cursor_keyword_table_is_case_sensitive_with_corrected_diagonals() {
    let table = [
        ("default", CursorStyle::Arrow),
        ("auto", CursorStyle::Arrow),
        ("pointer", CursorStyle::PointingHand),
        ("text", CursorStyle::IBeam),
        ("vertical-text", CursorStyle::IBeamCursorForVerticalLayout),
        ("crosshair", CursorStyle::Crosshair),
        ("grab", CursorStyle::OpenHand),
        ("grabbing", CursorStyle::ClosedHand),
        ("move", CursorStyle::ClosedHand),
        ("all-scroll", CursorStyle::ClosedHand),
        ("col-resize", CursorStyle::ResizeColumn),
        ("row-resize", CursorStyle::ResizeRow),
        ("ew-resize", CursorStyle::ResizeLeftRight),
        ("ns-resize", CursorStyle::ResizeUpDown),
        ("nwse-resize", CursorStyle::ResizeUpLeftDownRight),
        ("nw-resize", CursorStyle::ResizeUpLeftDownRight),
        ("se-resize", CursorStyle::ResizeUpLeftDownRight),
        ("nesw-resize", CursorStyle::ResizeUpRightDownLeft),
        ("ne-resize", CursorStyle::ResizeUpRightDownLeft),
        ("sw-resize", CursorStyle::ResizeUpRightDownLeft),
        ("w-resize", CursorStyle::ResizeLeft),
        ("e-resize", CursorStyle::ResizeRight),
        ("n-resize", CursorStyle::ResizeUp),
        ("s-resize", CursorStyle::ResizeDown),
        ("not-allowed", CursorStyle::OperationNotAllowed),
        ("no-drop", CursorStyle::OperationNotAllowed),
        ("alias", CursorStyle::DragLink),
        ("copy", CursorStyle::DragCopy),
        ("context-menu", CursorStyle::ContextualMenu),
    ];
    for (keyword, cursor) in table {
        assert_eq!(
            cursor_style_from_keyword(keyword),
            Some(cursor),
            "{keyword}"
        );
        assert_eq!(
            refine(json!({ "cursor": keyword })).mouse_cursor,
            Some(cursor)
        );
        let shouting = keyword.to_ascii_uppercase();
        assert_eq!(cursor_style_from_keyword(&shouting), None, "{shouting}");
        assert_eq!(refine(json!({ "cursor": shouting })).mouse_cursor, None);
    }
    assert_eq!(cursor_style_from_keyword("Pointer"), None);
    assert_eq!(cursor_style_from_keyword("wait"), None);
}

// ----------------------------------------------------------------------------------------
// Validation, equality and update economy (A7)
// ----------------------------------------------------------------------------------------

#[test]
fn wrong_types_reject_the_whole_style_naming_the_operation() {
    let cases = [
        json!({ "fontSize": "big" }),
        json!({ "gap": true }),
        json!({ "display": 3 }),
        json!({ "color": 3 }),
        json!({ "keyboardFocusDim": "no" }),
        json!({ "hover": 5 }),
        json!({ "hover": { "width": [] } }),
        json!({ "focusVisible": { "outlineWidth": "thick" } }),
        json!({ "scrollbarWidth": "thin" }),
    ];
    for case in cases {
        let error = rejected(
            json!({ "width": 10, "opacity": 0.5 })
                .as_object()
                .map(|good| {
                    let mut merged = good.clone();
                    merged.extend(case.as_object().unwrap().clone());
                    Value::Object(merged)
                })
                .unwrap(),
        );
        assert_eq!(error.operation(), OP);
        assert!(error.to_string().starts_with(OP), "{error}");
    }
    assert!(HostStyle::from_json(&json!([1, 2]), "batch[3]").is_err());
    assert_eq!(
        HostStyle::from_json(&json!("text"), "batch[3]")
            .unwrap_err()
            .operation(),
        "batch[3]"
    );
}

#[test]
fn unknown_names_and_nulls_are_ignored() {
    let r =
        refine(json!({ "bogusProperty": 1, "anotherOne": { "x": 2 }, "width": null, "gap": null }));
    assert_eq!(r, empty());
    assert_eq!(
        HostStyle::from_json(&Value::Null, OP).unwrap(),
        HostStyle::default()
    );
    assert_eq!(parse(json!({})), HostStyle::default());
}

#[test]
fn equality_ignores_undefined_and_key_order() {
    let a = parse(json!({
        "width": 10, "padding": 4, "hover": { "opacity": 0.5, "color": "#fff" },
    }));
    let b = parse(json!({
        "hover": { "color": "#ffffff", "opacity": 0.5, "extra": null }, "gap": null,
        "padding": 4, "width": 10,
    }));
    assert_eq!(a, b);
    assert_eq!(a.hover().unwrap().refinement().opacity, Some(0.5));
    // A change in a nested state style counts as a change.
    let c =
        parse(json!({ "width": 10, "padding": 4, "hover": { "opacity": 0.6, "color": "#fff" } }));
    assert_ne!(a, c);
    let d = parse(json!({ "width": 10, "padding": 4 }));
    assert_ne!(a, d);

    let x = json!({ "a": 1, "b": { "c": [1, 2], "d": null }, "e": null });
    let y = json!({ "b": { "c": [1.0, 2] }, "a": 1.0 });
    assert!(style_json_eq(&x, &y));
    assert!(!style_json_eq(&x, &json!({ "a": 1, "b": { "c": [2, 1] } })));
    assert!(!style_json_eq(
        &x,
        &json!({ "a": 1, "b": { "c": [1, 2] }, "z": 0 })
    ));
}

#[test]
fn replacing_a_style_reverts_omitted_properties_and_equal_styles_change_nothing() {
    let mut style = HostStyle::default();
    assert!(
        !style.replace(&json!({}), OP).unwrap(),
        "an empty style is the default"
    );
    assert!(
        style
            .replace(&json!({ "width": 10, "backgroundColor": "#f00" }), OP)
            .unwrap()
    );
    assert_eq!(style.to_refinement().size.width, Some(len(10.)));
    // Equal styles (different key order, an extra null) are not changes.
    assert!(
        !style
            .replace(
                &json!({ "backgroundColor": "#ff0000", "width": 10, "gap": null }),
                OP
            )
            .unwrap()
    );
    // A rejected replacement leaves the current style untouched.
    assert!(style.replace(&json!({ "width": "wide" }), OP).is_err());
    assert_eq!(style.to_refinement().size.width, Some(len(10.)));
    // Omitted properties revert to the upstream defaults; an empty style resets everything.
    assert!(style.replace(&json!({ "height": 5 }), OP).unwrap());
    assert_eq!(style.to_refinement().size.width, None);
    assert_eq!(style.to_refinement().background, None);
    assert!(style.replace(&json!({}), OP).unwrap());
    assert_eq!(style, HostStyle::default());
}

#[test]
fn style_store_updates_only_the_changed_element() {
    let mut store: StyleStore<&str> = StyleStore::new();
    let hover = json!({ "width": 10, "hover": { "backgroundColor": "#00f" } });
    assert_eq!(store.update("a", &hover, OP).unwrap(), StyleUpdate::Changed);
    assert_eq!(
        store.update("b", &json!({ "width": 20 }), OP).unwrap(),
        StyleUpdate::Changed
    );
    // Re-sending an equal style, including an equal nested hover style, sends nothing.
    let reordered = json!({ "hover": { "backgroundColor": "#0000ff" }, "width": 10 });
    assert_eq!(
        store.update("a", &reordered, OP).unwrap(),
        StyleUpdate::Unchanged
    );
    assert_eq!(
        store.update("b", &json!({ "width": 20 }), OP).unwrap(),
        StyleUpdate::Unchanged
    );
    // Changing the nested style of one element updates only that element.
    let changed = json!({ "width": 10, "hover": { "backgroundColor": "#00f1" } });
    assert_eq!(
        store.update("a", &changed, OP).unwrap(),
        StyleUpdate::Changed
    );
    assert_eq!(
        store.update("b", &json!({ "width": 20 }), OP).unwrap(),
        StyleUpdate::Unchanged
    );
    assert!(store.update("a", &json!({ "width": false }), OP).is_err());
    assert_eq!(store.get(&"a"), Some(&parse(changed)));
    // Removing a style is equivalent to the empty style.
    assert!(store.remove(&"b").is_some());
    assert_eq!(
        store.update("b", &json!({}), OP).unwrap(),
        StyleUpdate::Unchanged
    );
}

#[test]
fn nested_states_are_one_level_deep() {
    let style = parse(json!({
        "hover": {
            "opacity": 0.5, "keyboardFocusDim": false,
            "hover": { "opacity": 0.1 }, "focusVisible": { "opacity": 0.2 },
        },
        "active": { "backgroundColor": "#123456" },
        "focusVisible": { "outlineWidth": 2, "outlineColor": "#60A5FA", "outlineOffset": 2 },
    }));
    let hover = style.hover().unwrap().refinement();
    assert_eq!(hover.opacity, Some(0.5));
    assert_eq!(
        hover.keyboard_focus_dim, None,
        "keyboardFocusDim is not accepted in a state"
    );
    assert_eq!(hover.pointer_events, None);
    assert_eq!(
        style.active().unwrap().refinement().background,
        Some(rgb(0x123456).into())
    );
    assert_eq!(
        style.focus_visible().unwrap().refinement().outline,
        Some(Outline::new(px(2.), rgb(0x60A5FA), px(2.)))
    );
    // Nested states do not leak into the base style.
    assert_eq!(style.to_refinement(), empty());
}

#[test]
fn scrollbar_selection_and_dim_flags_are_carried_as_data() {
    let style = parse(json!({
        "scrollbarColor": "#808080", "scrollbarWidth": 10, "userSelect": "none",
        "selectionColor": "#3366ff59", "keyboardFocusDim": false,
    }));
    assert_eq!(
        style.base().scrollbar_color(),
        Some(Hsla::from(rgb(0x808080)))
    );
    assert_eq!(style.base().scrollbar_width(), px(10.));
    let r = style.to_refinement();
    assert_eq!(r.text.user_select, Some(false));
    assert!(r.text.selection_color.is_some());
    assert_eq!(r.keyboard_focus_dim, Some(false));
    let default = parse(json!({ "scrollbarColor": "#808080" }));
    assert_eq!(
        default.base().scrollbar_width(),
        px(6.),
        "the thumb thickness defaults to 6"
    );
    assert_eq!(parse(json!({})).base().scrollbar_color(), None);
    for (text, expected) in [("text", true), ("auto", true), ("none", false)] {
        assert_eq!(
            refine(json!({ "userSelect": text })).text.user_select,
            Some(expected)
        );
    }
}

#[test]
fn applying_a_style_is_idempotent_and_sets_state_styles_natively() {
    let style = parse(json!({
        "width": 40, "padding": 3,
        "hover": { "opacity": 0.5 }, "active": { "opacity": 0.25 },
        "focusVisible": { "outlineWidth": 1, "outlineColor": "#fff" },
    }));
    let mut once = style.apply(div().id("element"));
    let mut twice = style.apply(style.apply(div().id("element")));
    assert_eq!(once.style().clone(), twice.style().clone());
    assert_eq!(once.style().size.width, Some(len(40.)));
    let (a, b) = (once.interactivity(), twice.interactivity());
    assert_eq!(a.hover_style, b.hover_style);
    assert_eq!(a.active_style, b.active_style);
    assert_eq!(a.focus_visible_style, b.focus_visible_style);
    assert_eq!(a.hover_style.as_ref().unwrap().opacity, Some(0.5));
    assert_eq!(a.active_style.as_ref().unwrap().opacity, Some(0.25));
    assert!(a.focus_visible_style.is_some());
    assert!(
        a.hover_listener.is_none(),
        "hover is native: no callback is registered"
    );
    // `apply_to_style` refines only the base style.
    let mut base = StyleRefinement::default();
    style.apply_to_style(&mut base);
    style.apply_to_style(&mut base);
    assert_eq!(base, style.to_refinement());
    // Re-applying an empty style replaces the state styles of a previous one.
    let mut cleared = HostStyle::default().apply(style.apply(div().id("element")));
    let interactivity = cleared.interactivity();
    assert!(interactivity.hover_style.is_none());
    assert!(interactivity.active_style.is_none());
    assert!(interactivity.focus_visible_style.is_none());
}

// ----------------------------------------------------------------------------------------
// A5: hit testing policy
// ----------------------------------------------------------------------------------------

#[test]
fn mouse_blocking_policy_function() {
    let blocking = |value: Value| style_json_mouse_blocking(&value, OP).unwrap();
    use MouseBlocking::*;
    // Explicit policies.
    assert_eq!(
        blocking(json!({ "pointerEvents": "none", "position": "absolute", "background": "#f00" })),
        Ignored
    );
    assert_eq!(blocking(json!({ "pointerEvents": "auto" })), All);
    assert_eq!(
        blocking(json!({ "pointerEvents": "auto", "background": "transparent" })),
        All
    );
    // Unset follows the painted surface.
    assert_eq!(blocking(json!({})), Passive);
    assert_eq!(blocking(json!({ "position": "absolute" })), All);
    assert_eq!(
        blocking(json!({ "position": "fixed", "backgroundColor": "transparent" })),
        All
    );
    assert_eq!(blocking(json!({ "background": "#ff0000" })), ExceptWheel);
    assert_eq!(
        blocking(json!({ "backgroundColor": "rgba(255, 0, 0, 0.3)" })),
        ExceptWheel
    );
    assert_eq!(
        blocking(json!({ "position": "absolute", "background": "#ff0000" })),
        All
    );
    // Fully transparent fills do not block.
    assert_eq!(blocking(json!({ "background": "transparent" })), Passive);
    assert_eq!(blocking(json!({ "backgroundColor": "#ff000000" })), Passive);
    assert_eq!(
        blocking(json!({ "backgroundColor": "rgba(1, 2, 3, 0)" })),
        Passive
    );
    let clear = gradient(
        json!(0),
        ("#ff000000", json!(0)),
        ("#0000ff00", json!(1)),
        Value::Null,
    );
    assert_eq!(blocking(json!({ "background": clear })), Passive);
    let tinted = gradient(
        json!(0),
        ("#ff000000", json!(0)),
        ("#0000ff80", json!(1)),
        Value::Null,
    );
    assert_eq!(blocking(json!({ "background": tinted })), ExceptWheel);
    // Invalid fills conservatively block, including invalid gradients.
    assert_eq!(
        blocking(json!({ "background": "no such color" })),
        ExceptWheel
    );
    let invalid = gradient(
        json!(0),
        ("nope", json!(0)),
        ("#000", json!(1)),
        Value::Null,
    );
    assert_eq!(blocking(json!({ "background": invalid })), ExceptWheel);
    assert_eq!(
        blocking(json!({ "background": "#ff0000", "backgroundColor": "nope" })),
        ExceptWheel
    );
    assert_eq!(
        blocking(json!({ "background": "nope", "backgroundColor": "transparent" })),
        Passive
    );
    assert!(style_json_mouse_blocking(&json!({ "pointerEvents": 3 }), "op").is_err());
    assert!(All.blocks_hits() && All.blocks_wheel());
    assert!(ExceptWheel.blocks_hits() && !ExceptWheel.blocks_wheel());
    assert!(!Passive.blocks_hits() && !Ignored.blocks_hits());
    assert_eq!(
        parse(json!({ "pointerEvents": "auto" }))
            .to_refinement()
            .pointer_events,
        Some(PointerEvents::Auto)
    );
}

struct HitView {
    child: HostStyle,
    scroll: ScrollHandle,
}

impl Render for HitView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().p(px(10.)).child(
            div()
                .id("scroller")
                .w(px(100.))
                .h(px(100.))
                .overflow_scroll()
                .track_scroll(&self.scroll)
                .child(self.child.apply(div().id("child"))),
        )
    }
}

struct HitOutcome {
    child_hitbox: bool,
    scroller_hovered: bool,
    scroller_scrolls: bool,
}

fn hit_outcome(cx: &mut TestAppContext, child: Value) -> HitOutcome {
    let child = parse(child);
    let window = cx.add_window(move |_, _| HitView {
        child,
        scroll: ScrollHandle::new(),
    });
    let window = AnyWindowHandle::from(window);
    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
        window.simulate_mouse_move(point(px(20.), px(20.)), cx);
        let child_hitbox = window
            .rendered_frame
            .hitboxes
            .iter()
            .any(|hitbox| hitbox.bounds.size == size(px(60.), px(60.)));
        let scroller = window
            .rendered_frame
            .hitboxes
            .iter()
            .find(|hitbox| hitbox.bounds.size == size(px(100.), px(100.)))
            .cloned()
            .expect("the scroller has a hitbox");
        HitOutcome {
            child_hitbox,
            scroller_hovered: scroller.is_hovered(window),
            scroller_scrolls: scroller.should_handle_scroll(window),
        }
    })
    .unwrap()
}

#[vespertine::test]
fn hit_testing_follows_pointer_events_and_the_painted_surface(cx: &mut TestAppContext) {
    let sized = |extra: Value| {
        let mut style = json!({ "width": 60, "height": 60 });
        style
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        style
    };

    // Unset and in flow with a fill: blocks hits but lets the wheel reach the parent scroller.
    let filled = hit_outcome(cx, sized(json!({ "backgroundColor": "#ff0000" })));
    assert!(filled.child_hitbox);
    assert!(!filled.scroller_hovered);
    assert!(
        filled.scroller_scrolls,
        "a filled in-flow box must not prevent parent scrolling"
    );

    // Explicit auto blocks the wheel as well.
    let auto = hit_outcome(
        cx,
        sized(json!({ "backgroundColor": "#ff0000", "pointerEvents": "auto" })),
    );
    assert!(auto.child_hitbox);
    assert!(!auto.scroller_hovered);
    assert!(!auto.scroller_scrolls);

    // Absolute elements always block everything.
    let absolute = hit_outcome(cx, sized(json!({ "position": "absolute" })));
    assert!(absolute.child_hitbox);
    assert!(!absolute.scroller_hovered);
    assert!(!absolute.scroller_scrolls);

    // pointerEvents none inserts no hitbox: clicks go through, even for a filled box.
    let none = hit_outcome(
        cx,
        sized(json!({ "backgroundColor": "#ff0000", "pointerEvents": "none" })),
    );
    assert!(!none.child_hitbox);
    assert!(none.scroller_hovered);
    assert!(none.scroller_scrolls);

    // Transparent fills do not block.
    for fill in ["transparent", "#ff000000", "rgba(0, 0, 0, 0)"] {
        let transparent = hit_outcome(cx, sized(json!({ "backgroundColor": fill })));
        assert!(!transparent.child_hitbox, "{fill}");
        assert!(transparent.scroller_hovered, "{fill}");
        assert!(transparent.scroller_scrolls, "{fill}");
    }
    let unfilled = hit_outcome(cx, sized(json!({})));
    assert!(unfilled.scroller_hovered && unfilled.scroller_scrolls);

    // An invalid fill conservatively blocks hits (but, in flow, not the wheel).
    let invalid = hit_outcome(cx, sized(json!({ "background": "not a color" })));
    assert!(invalid.child_hitbox);
    assert!(!invalid.scroller_hovered);
    assert!(invalid.scroller_scrolls);
}

// ----------------------------------------------------------------------------------------
// A3 / A6: style as data vs builders; text inheritance
// ----------------------------------------------------------------------------------------

struct Rendered {
    quads: String,
    shadows: String,
    bounds: Vec<(&'static str, Bounds<Pixels>)>,
    font_size: Pixels,
}

fn render_tree(
    cx: &mut TestAppContext,
    selectors: &'static [&'static str],
    build: impl Fn(Rc<Cell<Pixels>>) -> AnyElement + 'static,
) -> Rendered {
    struct Probe(Rc<dyn Fn() -> AnyElement>);
    impl Render for Probe {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            (self.0)()
        }
    }
    let font_size = Rc::new(Cell::new(px(0.)));
    let build: Rc<dyn Fn() -> AnyElement> = {
        let font_size = font_size.clone();
        Rc::new(move || build(font_size.clone()))
    };
    let window = cx.add_window(move |_, _| Probe(build));
    let window = AnyWindowHandle::from(window);
    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
        Rendered {
            quads: format!("{:?}", window.painted_quads()),
            shadows: format!("{:?}", window.rendered_frame.scene.shadows),
            bounds: selectors
                .iter()
                .filter_map(|selector| {
                    window
                        .rendered_frame
                        .debug_bounds
                        .get(*selector)
                        .map(|bounds| (*selector, *bounds))
                })
                .collect(),
            font_size: font_size.get(),
        }
    })
    .unwrap()
}

fn font_size_probe(sink: Rc<Cell<Pixels>>) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |_, _, window, _| {
            let style = window.text_style();
            sink.set(style.font_size.to_pixels(window.rem_size()));
        },
    )
    .size(px(10.))
}

const SELECTORS: &[&str] = &["card", "title", "after"];

fn builder_tree(sink: Rc<Cell<Pixels>>) -> AnyElement {
    div()
        .size_full()
        .flex()
        .flex_col()
        .p(px(10.))
        .gap(px(6.))
        .child(
            div()
                .debug_selector(|| "card".into())
                .w(px(200.))
                .h(px(90.))
                .p(px(8.))
                .bg(rgb(0x336699))
                .border_2()
                .border_color(rgb(0x112233))
                .rounded(px(6.))
                .text_size(px(18.))
                .shadow(vec![
                    BoxShadow::new(px(0.), px(2.), rgb_alpha(0, 0x33)).blur_radius(px(4.)),
                    BoxShadow::new(px(0.), px(1.), rgb_alpha(0, 0x40))
                        .blur_radius(px(3.))
                        .inset(),
                ])
                .outline(Outline::new(px(2.), rgb(0x60A5FA), px(3.)))
                .child(div().debug_selector(|| "title".into()).child("Title"))
                .child(font_size_probe(sink)),
        )
        .child(
            div()
                .debug_selector(|| "after".into())
                .size(px(12.))
                .bg(rgb(0xff0000)),
        )
        .into_any_element()
}

fn data_tree(sink: Rc<Cell<Pixels>>) -> AnyElement {
    let apply = |style: Value, element: crate::Div| parse(style).apply(element.id("host"));
    div()
        .size_full()
        .child(apply(
            json!({
                "display": "flex", "flexDirection": "column", "padding": 10, "gap": 6,
                "width": "100%", "height": "100%",
            }),
            div()
                .child(
                    apply(
                        json!({
                            "width": 200, "height": 90, "padding": 8, "backgroundColor": "#336699",
                            "borderWidth": 2, "borderColor": "#112233", "borderRadius": 6,
                            "fontSize": 18,
                            "boxShadow": [
                                { "offsetY": 2, "blurRadius": 4, "color": "#00000033" },
                                { "offsetY": 1, "blurRadius": 3, "color": "#00000040", "inset": true },
                            ],
                            "outlineWidth": 2, "outlineColor": "#60A5FA", "outlineOffset": 3,
                        }),
                        div().debug_selector(|| "card".into()),
                    )
                    .child(div().debug_selector(|| "title".into()).child("Title"))
                    .child(font_size_probe(sink)),
                )
                .child(apply(
                    json!({ "width": 12, "height": 12, "background": "#ff0000" }),
                    div().debug_selector(|| "after".into()),
                )),
        ))
        .into_any_element()
}

/// A3: a representative style produces the same bounds and primitives as builder code.
#[vespertine::test]
fn style_as_data_matches_builder_code(cx: &mut TestAppContext) {
    let by_builder = render_tree(cx, SELECTORS, builder_tree);
    let by_data = render_tree(cx, SELECTORS, data_tree);
    assert_eq!(by_builder.bounds.len(), SELECTORS.len());
    assert_eq!(by_data.bounds, by_builder.bounds);
    assert_eq!(by_data.shadows, by_builder.shadows);
    assert_eq!(by_data.quads, by_builder.quads);
    assert_eq!(by_data.font_size, px(18.));
    assert_eq!(by_builder.font_size, px(18.));
}

/// A6: a container's font size reaches its text child, and a text host takes padding, width
/// and a background like any other element.
#[vespertine::test]
fn text_properties_cascade_and_text_hosts_accept_layout_styles(cx: &mut TestAppContext) {
    let nested = render_tree(cx, &["host"], |sink| {
        let container = parse(json!({ "fontSize": 31 })).apply(div().id("container"));
        container.child(font_size_probe(sink)).into_any_element()
    });
    assert_eq!(nested.font_size, px(31.));
    let plain = render_tree(cx, &[], |sink| {
        div().child(font_size_probe(sink)).into_any_element()
    });
    assert_ne!(plain.font_size, px(31.));

    let host_style = json!({
        "width": 120, "padding": 7, "backgroundColor": "#224466", "fontSize": 13, "color": "#fff",
    });
    let by_data = render_tree(cx, &["host"], move |_| {
        parse(host_style.clone())
            .apply(div().id("host").debug_selector(|| "host".into()))
            .child("text host")
            .into_any_element()
    });
    let by_builder = render_tree(cx, &["host"], |_| {
        div()
            .debug_selector(|| "host".into())
            .w(px(120.))
            .p(px(7.))
            .bg(rgb(0x224466))
            .text_size(px(13.))
            .text_color(rgb(0xffffff))
            .child("text host")
            .into_any_element()
    });
    assert_eq!(by_data.bounds[0].1.size.width, px(120.));
    assert_eq!(by_data.bounds, by_builder.bounds);
    assert_eq!(by_data.quads, by_builder.quads);
}

// ----------------------------------------------------------------------------------------
// A4: keyboard-focus dim and focus-visible
// ----------------------------------------------------------------------------------------

struct DimView {
    controls: Vec<(FocusHandle, HostStyle)>,
    editor: FocusHandle,
    editor_style: HostStyle,
}

impl Render for DimView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let editor_focus = self.editor.clone();
        let mut row = div().size_full().flex().flex_row().gap(px(12.)).p(px(24.));
        for (index, (handle, style)) in self.controls.iter().enumerate() {
            let selector: &'static str = ["c0", "c1", "c2"][index];
            let id: SharedString = selector.into();
            row = row.child(
                style
                    .apply(div().id(id).debug_selector(move || selector.into()))
                    .track_focus(handle)
                    .child("control"),
            );
        }
        row.child(
            self.editor_style
                .apply(div().id("editor").debug_selector(|| "editor".into()))
                .track_focus(&self.editor)
                .child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, cx| {
                            window.handle_input(
                                &editor_focus,
                                ElementInputHandler::new(bounds, view.clone()),
                                cx,
                            );
                        },
                    )
                    .size_full(),
                ),
        )
    }
}

impl EntityInputHandler for DimView {
    fn text_for_range(
        &mut self,
        _range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        None
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        _text: &str,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        _new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        None
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

fn control_style(extra: Value) -> HostStyle {
    let mut style = json!({
        "width": 140, "height": 44, "backgroundColor": "#303030", "borderRadius": 6,
        "focusVisible": { "outlineWidth": 2, "outlineColor": "#60A5FA", "outlineOffset": 2 },
        "hover": { "backgroundColor": "#404040" },
    });
    style
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    parse(style)
}

struct DimHarness {
    cx: TestAppContext,
    window: AnyWindowHandle,
    handles: Vec<FocusHandle>,
    editor: FocusHandle,
}

impl DimHarness {
    fn new(third: Value) -> Self {
        let mut cx = TestAppContext::single();
        let handles: Vec<FocusHandle> = (0..3)
            .map(|_| cx.update(|cx| cx.focus_handle().tab_stop(true)))
            .collect();
        let editor = cx.update(|cx| cx.focus_handle());
        let window = cx.add_window({
            let controls: Vec<(FocusHandle, HostStyle)> = handles
                .iter()
                .cloned()
                .enumerate()
                .map(|(index, handle)| {
                    let style = if index == 2 {
                        control_style(third.clone())
                    } else {
                        control_style(json!({}))
                    };
                    (handle, style)
                })
                .collect();
            let editor = editor.clone();
            move |_, _| DimView {
                controls,
                editor,
                editor_style: control_style(json!({ "width": 100, "backgroundColor": "#101010" })),
            }
        });
        let mut harness = Self {
            cx,
            window: window.into(),
            handles,
            editor,
        };
        harness.draw();
        harness
    }

    fn draw(&mut self) {
        self.cx.run_until_parked();
        self.cx
            .update_window(self.window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        self.cx.run_until_parked();
        // Settle: the dim may need one more frame once the focused input handler is known.
        self.cx
            .update_window(self.window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    }

    fn focus(&mut self, handle: &FocusHandle) {
        self.cx
            .update_window(self.window, |_, window, cx| window.focus(handle, cx))
            .unwrap();
        self.draw();
    }

    fn key_press(&mut self, key: &str) {
        let keystroke = Keystroke::parse(key).unwrap();
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.dispatch_event(
                    KeyDownEvent {
                        keystroke,
                        is_held: false,
                        prefer_character_input: false,
                    }
                    .to_platform_input(),
                    cx,
                );
            })
            .unwrap();
        self.draw();
    }

    /// Tab navigation: a key press followed by moving focus to the next control.
    fn tab(&mut self) {
        self.key_press("tab");
        self.cx
            .update_window(self.window, |_, window, cx| window.focus_next(cx))
            .unwrap();
        self.draw();
    }

    fn mouse_press(&mut self, selector: &'static str) {
        let bounds = self.bounds(selector);
        let position = bounds.center();
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.simulate_mouse_move(position, cx);
                window.dispatch_event(
                    PlatformInput::MouseDown(MouseDownEvent {
                        click_count: 1,
                        first_mouse: false,
                        modifiers: Default::default(),
                        button: MouseButton::Left,
                        position,
                    }),
                    cx,
                );
            })
            .unwrap();
        self.draw();
    }

    fn bounds(&mut self, selector: &'static str) -> Bounds<Pixels> {
        self.cx
            .update_window(self.window, |_, window, _| {
                window
                    .rendered_frame
                    .debug_bounds
                    .get(selector)
                    .copied()
                    .expect("bounds")
            })
            .unwrap()
    }

    fn quads(&mut self) -> (Vec<Quad>, f32) {
        self.cx
            .update_window(self.window, |_, window, _| {
                (window.painted_quads(), window.scale_factor())
            })
            .unwrap()
    }

    /// The painted fill alpha of the control, i.e. its fill alpha times its subtree opacity.
    fn opacity(&mut self, selector: &'static str) -> f32 {
        let bounds = self.bounds(selector);
        let (quads, scale) = self.quads();
        let fill = quads
            .iter()
            .find(|quad| {
                (quad.bounds.origin.x.0 - bounds.origin.x.0 * scale).abs() < 0.01
                    && (quad.bounds.origin.y.0 - bounds.origin.y.0 * scale).abs() < 0.01
                    && (quad.bounds.size.width.0 - bounds.size.width.0 * scale).abs() < 0.01
                    && !quad.background.is_transparent()
            })
            .expect("control fill painted");
        fill.background.solid.a
    }

    fn opacities(&mut self) -> [f32; 3] {
        [self.opacity("c0"), self.opacity("c1"), self.opacity("c2")]
    }

    fn rings(&mut self) -> usize {
        let (quads, _) = self.quads();
        let ring: Hsla = rgb(0x60A5FA).into();
        quads
            .iter()
            .filter(|quad| {
                quad.background.is_transparent()
                    && quad.border_color.h == ring.h
                    && quad.border_color.s == ring.s
                    && quad.border_color.l == ring.l
            })
            .count()
    }

    fn layout(&mut self) -> Vec<Bounds<Pixels>> {
        ["c0", "c1", "c2", "editor"]
            .map(|selector| self.bounds(selector))
            .to_vec()
    }
}

fn assert_opacities(actual: [f32; 3], expected: [f32; 3]) {
    for (actual, expected) in actual.iter().zip(expected) {
        assert!(
            (actual - expected).abs() < 1e-4,
            "{actual:?} != {expected:?}"
        );
    }
}

/// A4: tab navigation dims every other focusable control and shows the focus-visible style.
#[vespertine::test]
fn keyboard_focus_dims_other_controls_and_shows_focus_visible(_cx: &mut TestAppContext) {
    let mut h = DimHarness::new(json!({}));
    let layout = h.layout();
    assert_opacities(h.opacities(), [1., 1., 1.]);
    assert_eq!(h.rings(), 0);

    h.focus(&h.handles[1].clone());
    // Focus moved programmatically but the last input was never the keyboard.
    assert_opacities(h.opacities(), [1., 1., 1.]);

    h.key_press("a");
    assert_opacities(h.opacities(), [0.4, 1., 0.4]);
    assert!(
        h.rings() > 0,
        "the focused control shows its focus-visible outline"
    );

    // Tab to the next control: the ring and the dim move with focus.
    h.tab();
    let dimmed = h.opacities();
    assert!(
        dimmed
            .iter()
            .filter(|opacity| (**opacity - 0.4).abs() < 1e-4)
            .count()
            == 2
    );
    assert!(
        dimmed
            .iter()
            .filter(|opacity| (**opacity - 1.).abs() < 1e-4)
            .count()
            == 1
    );

    // None of this changes layout.
    assert_eq!(h.layout(), layout);
}

/// A4: a mouse press that focuses a control shows neither the dim nor the focus-visible style.
#[vespertine::test]
fn mouse_focus_never_dims_and_never_shows_focus_visible(_cx: &mut TestAppContext) {
    let mut h = DimHarness::new(json!({}));
    h.mouse_press("c1");
    assert!(h.handles[1].is_focused_in(&mut h.cx, h.window));
    assert_opacities(h.opacities(), [1., 1., 1.]);
    assert_eq!(h.rings(), 0);
    // Keyboard input after the press turns both on.
    h.key_press("b");
    assert_opacities(h.opacities(), [0.4, 1., 0.4]);
    assert!(h.rings() > 0);
    // A new mouse press switches the modality back.
    h.mouse_press("c2");
    assert_opacities(h.opacities(), [1., 1., 1.]);
    assert_eq!(h.rings(), 0);
}

/// A4: a focused text editor does not count as keyboard focus.
#[vespertine::test]
fn focused_text_editor_dims_nothing(_cx: &mut TestAppContext) {
    let mut h = DimHarness::new(json!({}));
    h.focus(&h.editor.clone());
    h.key_press("a");
    assert_opacities(h.opacities(), [1., 1., 1.]);
    // Moving keyboard focus from the editor to a control starts the dim again, and back.
    h.focus(&h.handles[0].clone());
    assert_opacities(h.opacities(), [1., 0.4, 0.4]);
    h.focus(&h.editor.clone());
    assert_opacities(h.opacities(), [1., 1., 1.]);
}

/// A4: a per-element opt-out stays at full opacity, and the window option switches the dim off.
#[vespertine::test]
fn keyboard_focus_dim_opt_out_and_window_option(_cx: &mut TestAppContext) {
    let mut h = DimHarness::new(json!({ "keyboardFocusDim": false }));
    h.focus(&h.handles[0].clone());
    h.key_press("a");
    assert_opacities(h.opacities(), [1., 0.4, 1.]);

    h.cx.update_window(h.window, |_, window, _| {
        assert!(window.keyboard_focus_dim_enabled());
        window.set_keyboard_focus_dim_enabled(false);
    })
    .unwrap();
    h.draw();
    assert_opacities(h.opacities(), [1., 1., 1.]);
    assert!(
        h.rings() > 0,
        "focus-visible still applies with the dim off"
    );
    h.cx.update_window(h.window, |_, window, _| {
        window.set_keyboard_focus_dim_enabled(true)
    })
    .unwrap();
    h.draw();
    assert_opacities(h.opacities(), [1., 0.4, 1.]);
}

/// A4: the dim composes with the control's own opacity and its ancestors stay unchanged.
#[vespertine::test]
fn keyboard_focus_dim_multiplies_the_controls_own_opacity(_cx: &mut TestAppContext) {
    let mut h = DimHarness::new(json!({ "opacity": 0.5 }));
    h.focus(&h.handles[0].clone());
    h.key_press("a");
    assert_opacities(h.opacities(), [1., 0.4, 0.2]);
}

/// A4: native hover applies without any callback, and a window created with the option off
/// never dims.
#[vespertine::test]
fn hover_is_native_and_window_option_defaults_on(_cx: &mut TestAppContext) {
    let mut h = DimHarness::new(json!({}));
    let center = h.bounds("c0").center();
    h.cx.update_window(h.window, |_, window, cx| {
        window.simulate_mouse_move(center, cx)
    })
    .unwrap();
    h.draw();
    let (quads, scale) = h.quads();
    let hovered = quads.iter().any(|quad| {
        (quad.background.solid.l - Hsla::from(rgb(0x404040)).l).abs() < 1e-4
            && (quad.bounds.size.width.0 - 140. * scale).abs() < 0.01
    });
    assert!(hovered, "the nested hover style is applied");
    let options = crate::WindowOptions::default();
    assert!(options.keyboard_focus_dim);
}

trait FocusProbe {
    fn is_focused_in(&self, cx: &mut TestAppContext, window: AnyWindowHandle) -> bool;
}

impl FocusProbe for FocusHandle {
    fn is_focused_in(&self, cx: &mut TestAppContext, window: AnyWindowHandle) -> bool {
        cx.update_window(window, |_, window, _: &mut App| self.is_focused(window))
            .unwrap()
    }
}

// ----------------------------------------------------------------------------------------
// A1: the coverage example
// ----------------------------------------------------------------------------------------

const INVENTORY: &[&str] = &[
    "display",
    "visibility",
    "flexDirection",
    "flexWrap",
    "flexGrow",
    "flexShrink",
    "flexBasis",
    "alignItems",
    "alignSelf",
    "alignContent",
    "justifyContent",
    "gap",
    "rowGap",
    "columnGap",
    "gridTemplateColumns",
    "gridTemplateRows",
    "gridColumnMin",
    "gridRowMin",
    "width",
    "height",
    "minWidth",
    "minHeight",
    "maxWidth",
    "maxHeight",
    "padding",
    "paddingTop",
    "paddingRight",
    "paddingBottom",
    "paddingLeft",
    "margin",
    "marginTop",
    "marginRight",
    "marginBottom",
    "marginLeft",
    "position",
    "top",
    "right",
    "bottom",
    "left",
    "aspectRatio",
    "background",
    "backgroundColor",
    "color",
    "opacity",
    "borderWidth",
    "borderTopWidth",
    "borderRightWidth",
    "borderBottomWidth",
    "borderLeftWidth",
    "borderColor",
    "borderStyle",
    "borderRadius",
    "borderTopLeftRadius",
    "borderTopRightRadius",
    "borderBottomLeftRadius",
    "borderBottomRightRadius",
    "boxShadow",
    "outlineWidth",
    "outlineColor",
    "outlineOffset",
    "fontSize",
    "fontFamily",
    "fontWeight",
    "textAlign",
    "lineHeight",
    "whiteSpace",
    "textOverflow",
    "lineClamp",
    "textDecoration",
    "overflow",
    "overflowX",
    "overflowY",
    "scrollbarColor",
    "scrollbarWidth",
    "cursor",
    "pointerEvents",
    "userSelect",
    "selectionColor",
    "hover",
    "active",
    "focusVisible",
    "keyboardFocusDim",
];

fn collect_keys(value: &Value, keys: &mut std::collections::BTreeSet<String>) {
    if let Value::Object(map) = value {
        for (key, nested) in map {
            keys.insert(key.clone());
            if matches!(key.as_str(), "hover" | "active" | "focusVisible") {
                collect_keys(nested, keys);
            }
        }
    }
}

#[test]
fn coverage_example_exercises_every_property_and_removal_updates() {
    let coverage: Value =
        serde_json::from_str(include_str!("../examples/style_data/coverage.json")).unwrap();
    let panels = coverage["panels"].as_array().unwrap();
    let mut keys = std::collections::BTreeSet::new();
    for panel in panels {
        for part in ["container", "child", "removal"] {
            if let Some(style) = panel.get(part) {
                HostStyle::from_json(style, "coverage").expect("every example style is accepted");
                collect_keys(style, &mut keys);
            }
        }
    }
    for name in INVENTORY {
        assert!(
            keys.contains(*name),
            "the coverage example never uses `{name}`"
        );
    }

    // Longhands override the shorthands in the first panel.
    let mut style = parse(panels[0]["container"].clone());
    let r = style.to_refinement();
    assert_eq!(r.padding.left, Some(DefiniteLength::from(px(20.))));
    assert_eq!(r.padding.bottom, Some(DefiniteLength::from(px(4.))));
    assert_eq!(r.border_widths.left, Some(AbsoluteLength::from(px(4.))));
    assert_eq!(r.corner_radii.top_left, Some(AbsoluteLength::from(px(0.))));
    assert_eq!(
        r.corner_radii.top_right,
        Some(AbsoluteLength::from(px(20.)))
    );
    assert_eq!(r.gap.width, Some(DefiniteLength::from(px(14.))));
    assert_eq!(r.gap.height, Some(DefiniteLength::from(px(4.))));
    assert_eq!(r.overflow.x, Some(Overflow::Scroll));
    assert_eq!(r.overflow.y, Some(Overflow::Hidden));

    // A removal update restores the defaults of everything the new style omits.
    assert!(style.replace(&panels[0]["removal"], "coverage").unwrap());
    let r = style.to_refinement();
    assert_eq!(r.padding.left, None);
    assert_eq!(r.border_widths.left, None);
    assert_eq!(r.corner_radii.top_right, None);
    assert_eq!(r.gap.width, None);
    assert_eq!(r.overflow.x, None);
    assert_eq!(r.size.width, Some(len(200.)));
    // The empty style resets everything.
    assert!(style.replace(&Value::Null, "coverage").unwrap());
    assert_eq!(style, HostStyle::default());
}
