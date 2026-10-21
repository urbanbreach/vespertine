//! PRF-011 acceptance tests: carried and replayed text measurements.

use super::*;
use crate::{
    AvailableSpace, Div, FontFeatures, Hsla, IntoElement, LayoutStats, ParentElement, Styled,
    StyledText, TestAppContext, UnderlineStyle, VisualTestContext, div, font, point, px, size,
};

/// The test platform measures every character as 0.6 em, so equal-length strings have equal
/// widths and an extra digit widens the text.
const CELL_FONT_SIZE: f32 = 14.;

fn draw(window: &mut VisualTestContext, width: f32, root: impl IntoElement) {
    window.draw(
        point(px(0.), px(0.)),
        size(px(width), px(2000.)),
        move |_, _| root.into_element(),
    );
}

fn layout_stats(window: &mut VisualTestContext) -> LayoutStats {
    window.update(|window, _| window.layout_stats())
}

fn reset(window: &mut VisualTestContext) {
    window.update(|window, _| window.reset_layout_stats());
    reset_text_measure_stats();
}

fn table(values: &[String], width: f32, color: Hsla) -> impl IntoElement {
    div()
        .w(px(width))
        .flex()
        .flex_row()
        .flex_wrap()
        .gap(px(8.))
        .text_size(px(CELL_FONT_SIZE))
        .line_height(px(18.))
        .text_color(color)
        .children(
            values
                .iter()
                .map(|value| div().px(px(6.)).child(SharedString::from(value.clone()))),
        )
}

fn numbers(frame: usize, count: usize, digits: usize) -> Vec<String> {
    (0..count)
        .map(|cell| {
            let n = (frame * 104_729 + cell * 7_919 + 12_345) % 10usize.pow(digits as u32 - 1);
            format!(
                "{:0width$}",
                n + 10usize.pow(digits as u32 - 1),
                width = digits
            )
        })
        .collect()
}

#[vespertine::test]
fn equal_length_updates_keep_ancestor_layouts_clean(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let color = crate::black();
    // The first frames fill the probe logs.
    for frame in 0..4 {
        draw(window, 600., table(&numbers(frame, 24, 7), 600., color));
    }
    for frame in 4..10 {
        reset(window);
        draw(window, 600., table(&numbers(frame, 24, 7), 600., color));
        let layout = layout_stats(window);
        let measure = text_measure_stats();
        assert_eq!(
            layout.computed_layouts, 0,
            "frame {frame}: no ancestor layout may be computed"
        );
        assert!(layout.skipped_layouts >= 1, "frame {frame}");
        assert_eq!(layout.replays_failed, 0, "frame {frame}");
        assert_eq!(
            layout.replays_succeeded, 24,
            "frame {frame}: one replay per changed cell"
        );
        assert_eq!(measure.carried, 0, "frame {frame}: every string changed");
    }

    // A digit count change widens one cell: ancestors are laid out again.
    reset(window);
    let mut grown = numbers(10, 24, 7);
    grown[5].insert(0, '1');
    draw(window, 600., table(&grown, 600., color));
    let layout = layout_stats(window);
    assert_eq!(layout.computed_layouts, 1);
    assert_eq!(layout.replays_failed, 1);

    // The cell reverts on the next frame: another size change, another layout.
    reset(window);
    draw(window, 600., table(&numbers(11, 24, 7), 600., color));
    assert_eq!(layout_stats(window).computed_layouts, 1);

    // A wrap height change invalidates as well: a narrow table wraps its cells.
    for frame in 12..14 {
        draw(window, 200., table(&numbers(frame, 24, 7), 200., color));
    }
    reset(window);
    let mut wider = numbers(14, 24, 7);
    wider[0].push_str("0000000000");
    draw(window, 200., table(&wider, 200., color));
    assert_eq!(layout_stats(window).computed_layouts, 1);
}

#[vespertine::test]
fn unchanged_text_is_carried_without_rebinding_or_shaping(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let color = crate::black();
    let values = numbers(0, 24, 7);
    for _ in 0..3 {
        draw(window, 600., table(&values, 600., color));
    }
    reset(window);
    draw(window, 600., table(&values, 600., color));
    let layout = layout_stats(window);
    let measure = text_measure_stats();
    assert_eq!(layout.measure_writes, 0, "no closure rebinds");
    assert_eq!(layout.computed_layouts, 0);
    assert_eq!(measure.carried, 24);
    assert_eq!(measure.measure_calls, 0);
    assert_eq!(measure.reshapes, 0);
    assert_eq!(
        measure.lines_moved, 24,
        "uniquely owned previous layouts give their lines up"
    );
}

fn runs(lens: &[usize], mut decorate: impl FnMut(usize) -> TextRun) -> Vec<TextRun> {
    lens.iter()
        .enumerate()
        .map(|(ix, len)| TextRun {
            len: *len,
            ..decorate(ix)
        })
        .collect()
}

fn run(color: Hsla) -> TextRun {
    TextRun {
        strikethrough: None,
        underline: None,
        background_color: None,
        color,
        font: font("Test"),
        letter_spacing: px(0.),
        len: 0,
    }
}

fn styled(text: &str, runs: Vec<TextRun>) -> (StyledText, TextLayout) {
    let styled = StyledText::new(text.to_string()).with_runs(runs);
    let layout = styled.layout().clone();
    (styled, layout)
}

fn decorations(layout: &TextLayout) -> Vec<(u32, Hsla, Option<Hsla>, bool, bool)> {
    let inner = layout.0.borrow();
    inner
        .as_ref()
        .expect("measured")
        .lines
        .iter()
        .flat_map(|line| line.decoration_runs.iter())
        .map(|run| {
            (
                run.len,
                run.color,
                run.background_color,
                run.underline.is_some(),
                run.strikethrough.is_some(),
            )
        })
        .collect()
}

fn draw_text(window: &mut VisualTestContext, text: StyledText, extra: impl FnOnce(Div) -> Div) {
    draw(
        window,
        400.,
        extra(div().text_size(px(CELL_FONT_SIZE)).line_height(px(18.))).child(text),
    );
}

#[vespertine::test]
fn decoration_only_changes_rewrite_in_place_without_reshaping(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let text = "alpha beta gamma";
    let (red, blue) = (crate::red(), crate::blue());
    let base = |first: Hsla, second: Hsla| {
        runs(&[5, 1, 4, 1, 5], |ix| {
            run(if ix % 2 == 0 { first } else { second })
        })
    };

    let (element, _) = styled(text, base(red, blue));
    draw_text(window, element, |div| div);
    reset(window);

    // Color.
    let (element, layout) = styled(text, base(blue, red));
    draw_text(window, element, |div| div);
    let measure = text_measure_stats();
    assert_eq!(measure.decoration_rewrites, 1);
    assert_eq!(measure.measure_calls, 0, "no reshaping");
    assert_eq!(measure.replays, 0);
    assert!(
        decorations(&layout)
            .iter()
            .enumerate()
            .all(|(ix, (_, color, ..))| { *color == if ix % 2 == 0 { blue } else { red } })
    );

    // Background, underline and strikethrough, with the same segmentation pattern (every run
    // differs from its predecessor).
    let decorated = runs(&[5, 1, 4, 1, 5], |ix| TextRun {
        background_color: (ix == 2).then_some(red),
        underline: (ix == 2).then_some(UnderlineStyle {
            thickness: px(1.),
            color: None,
            wavy: false,
        }),
        strikethrough: (ix == 0).then_some(crate::StrikethroughStyle {
            thickness: px(1.),
            color: None,
        }),
        ..run(if ix % 2 == 0 { blue } else { red })
    });
    reset(window);
    let (element, layout) = styled(text, decorated);
    draw_text(window, element, |div| div);
    let measure = text_measure_stats();
    assert_eq!(measure.measure_calls, 0, "no reshaping");
    assert_eq!(measure.decoration_rewrites, 1);
    // Every run differs from its predecessor, so none merge; the list covers the whole line and
    // carries each decoration.
    let list = decorations(&layout);
    assert_eq!(
        list.iter().map(|entry| entry.0).sum::<u32>(),
        text.len() as u32
    );
    assert!(
        list.iter()
            .any(|(_, _, background, underline, _)| background.is_some() && *underline)
    );
    assert!(list.iter().any(|(.., strikethrough)| *strikethrough));
}

#[vespertine::test]
fn changed_segmentation_fonts_and_features_reshape(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let text = "alpha beta gamma";
    let red = crate::red();
    let base = runs(&[5, 1, 4, 1, 5], |_| run(red));
    let (element, _) = styled(text, base);
    draw_text(window, element, |div| div);

    // Moved decoration boundary: [6, 4, ...] changes the color segmentation.
    let blue = crate::blue();
    let moved = runs(&[5, 1, 4, 1, 5], |ix| run(if ix == 1 { blue } else { red }));
    let (element, _) = styled(text, moved.clone());
    draw_text(window, element, |div| div);
    reset(window);
    let regrouped = runs(&[6, 4, 6], |ix| run(if ix == 1 { blue } else { red }));
    let (element, _) = styled(text, regrouped);
    draw_text(window, element, |div| div);
    let measure = text_measure_stats();
    assert_eq!(measure.replays, 1);
    assert!(measure.measure_calls > 0, "reshaped");
    assert_eq!(measure.decoration_rewrites, 0);

    // A different run font, weight or feature reshapes.
    for variant in 0..3 {
        let (element, _) = styled(text, moved.clone());
        draw_text(window, element, |div| div);
        reset(window);
        let changed = runs(&[5, 1, 4, 1, 5], |ix| {
            let mut next = run(if ix == 1 { blue } else { red });
            match variant {
                0 => next.font = font("Other"),
                1 => next.font = font("Test").bold(),
                _ => {
                    next.font.features = FontFeatures(Arc::new(vec![("tnum".into(), 1)]));
                }
            }
            next
        });
        let (element, _) = styled(text, changed);
        draw_text(window, element, |div| div);
        let measure = text_measure_stats();
        assert_eq!(measure.replays, 1, "variant {variant}");
        assert_eq!(
            measure.carried + measure.decoration_rewrites,
            0,
            "variant {variant}"
        );
    }
}

#[vespertine::test]
fn truncating_text_reshapes_when_policy_or_decoration_changes(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let text = "a rather long string that cannot fit";
    let (red, blue) = (crate::red(), crate::blue());
    let truncating = |color: Hsla| runs(&[text.len()], |_| run(color));

    let (element, _) = styled(text, truncating(red));
    draw_text(window, element, |div| {
        div.w(px(60.)).whitespace_nowrap().text_ellipsis()
    });
    reset(window);

    // Recoloring truncating text must not rewrite in place (its shaped string is rewritten).
    let (element, layout) = styled(text, truncating(blue));
    draw_text(window, element, |div| {
        div.w(px(60.)).whitespace_nowrap().text_ellipsis()
    });
    let measure = text_measure_stats();
    assert_eq!(measure.decoration_rewrites, 0);
    assert_eq!(measure.replays, 1);
    assert!(measure.measure_calls > 0);
    assert!(
        decorations(&layout)
            .iter()
            .all(|(_, color, ..)| *color == blue)
    );
    let shaped: usize = layout
        .0
        .borrow()
        .as_ref()
        .unwrap()
        .lines
        .iter()
        .map(|line| line.text.len())
        .sum();
    assert!(shaped < text.len(), "the text was truncated");

    // Switching the truncation policy reshapes and shows the whole text again.
    reset(window);
    let (element, layout) = styled(text, truncating(blue));
    draw_text(window, element, |div| div.w(px(60.)).whitespace_nowrap());
    let measure = text_measure_stats();
    assert_eq!(measure.replays, 1);
    assert_eq!(measure.carried, 0);
    let shaped: usize = layout
        .0
        .borrow()
        .as_ref()
        .unwrap()
        .lines
        .iter()
        .map(|line| line.text.len())
        .sum();
    assert_eq!(shaped, text.len());
}

/// Runs `body` inside a window draw so measuring functions have a window and an app.
fn with_measures(cx: &mut TestAppContext, body: impl FnOnce(&mut Window, &mut App) + 'static) {
    let window = cx.add_empty_window();
    window.update(|window, cx| body(window, cx));
}

fn measure_of(window: &mut Window, text: &str) -> TextMeasure {
    TextMeasure {
        layout: TextLayout::default(),
        text: SharedString::from(text.to_string()),
        style: {
            let mut style = (*window.text_style_shared()).clone();
            style.font_size = px(CELL_FONT_SIZE).into();
            Arc::new(style)
        },
        runs: RunSource::Plain,
        font_size: px(CELL_FONT_SIZE),
        line_height: px(18.),
        font_generation: window.text_system().font_generation(),
        rem_size: px(16.),
    }
}

#[vespertine::test]
fn intrinsic_probes_after_a_wrap_recover_the_unwrapped_size(cx: &mut TestAppContext) {
    with_measures(cx, |window, cx| {
        let mut measure = measure_of(window, "the quick brown fox jumps over the lazy dog");
        let free = Size {
            width: None,
            height: None,
        };
        let unwrapped = measure.measure(
            free,
            size(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
            window,
            cx,
        );
        let wrapped = measure.measure(
            free,
            size(
                AvailableSpace::Definite(px(40.)),
                AvailableSpace::MaxContent,
            ),
            window,
            cx,
        );
        assert!(wrapped.width <= px(40.) && wrapped.height > unwrapped.height);
        for space in [AvailableSpace::MinContent, AvailableSpace::MaxContent] {
            let again = measure.measure(free, size(space, AvailableSpace::MaxContent), window, cx);
            assert_eq!(
                again, unwrapped,
                "{space:?} must not answer with the wrapped size"
            );
        }
        // The kept wrap answers a probe at the width it was taken at without shaping.
        reset_text_measure_stats();
        let wide = measure.measure(
            free,
            size(
                AvailableSpace::Definite(px(1000.)),
                AvailableSpace::MaxContent,
            ),
            window,
            cx,
        );
        assert_eq!(wide, unwrapped, "unwrapped text answers a wider probe");
        assert_eq!(text_measure_stats().reshapes, 0);
    });
}

#[vespertine::test]
fn carry_moves_lines_from_a_unique_owner_and_copies_from_a_shared_one(cx: &mut TestAppContext) {
    with_measures(cx, |window, cx| {
        for shared in [false, true] {
            let mut previous = measure_of(window, "carried text");
            let free = Size {
                width: None,
                height: None,
            };
            previous.measure(
                free,
                size(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
                window,
                cx,
            );
            previous.layout.prepaint(
                Bounds::new(point(px(3.), px(4.)), size(px(50.), px(18.))),
                "carried text",
            );
            let _holder = shared.then(|| previous.layout.clone());

            reset_text_measure_stats();
            let mut new = measure_of(window, "carried text");
            assert_eq!(new.reconcile(&mut previous), MeasureDecision::Keep);
            let stats = text_measure_stats();
            assert_eq!(stats.lines_moved, u64::from(!shared));
            assert_eq!(stats.lines_copied, u64::from(shared));

            // The new element's handle has the lines and no painted bounds.
            let inner = new.layout.0.borrow();
            let inner = inner.as_ref().expect("lines carried");
            assert_eq!(inner.lines.len(), 1);
            assert!(inner.bounds.is_none());
            // The installed measurement writes into the new handle from now on.
            assert!(Rc::ptr_eq(&previous.layout.0, &new.layout.0));
        }
    });
}

#[vespertine::test]
fn a_shared_previous_layout_stays_valid_after_the_carry(cx: &mut TestAppContext) {
    with_measures(cx, |window, cx| {
        let mut previous = measure_of(window, "shared owner");
        previous.measure(
            Size {
                width: None,
                height: None,
            },
            size(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
            window,
            cx,
        );
        previous.layout.prepaint(
            Bounds::new(point(px(1.), px(2.)), size(px(30.), px(18.))),
            "shared owner",
        );
        let old_cell = previous.layout.clone();
        let mut new = measure_of(window, "shared owner");
        assert_eq!(new.reconcile(&mut previous), MeasureDecision::Keep);
        let old = old_cell.0.borrow();
        let old = old
            .as_ref()
            .expect("the other owner still has its measurement");
        assert_eq!(old.lines.len(), 1);
        assert!(
            old.bounds.is_none(),
            "painted bounds are cleared in both layouts"
        );
        assert!(new.layout.0.borrow().as_ref().unwrap().bounds.is_none());
    });
}

#[vespertine::test]
fn replay_history_overflow_invalidates_conservatively(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    let paragraph = |text: &str, width: f32| {
        div()
            .w(px(width))
            .text_size(px(CELL_FONT_SIZE))
            .line_height(px(18.))
            .child(SharedString::from(text.to_string()))
    };
    // Seventeen distinct widths probe the paragraph under seventeen wrap widths: the log
    // overflows.
    let text = "the quick brown fox jumps over the lazy dog and keeps running";
    for step in 0..17 {
        draw(window, 400., paragraph(text, 300. + step as f32 * 7.));
    }
    reset(window);
    draw(
        window,
        400.,
        paragraph(
            "the quick brown fox jumps over the lazy dog and keeps walking",
            300.,
        ),
    );
    let layout = layout_stats(window);
    assert!(layout.replays_failed >= 1, "{layout:?}");
    assert!(layout.computed_layouts >= 1, "{layout:?}");
}

#[vespertine::test]
fn an_unequal_size_dirties_the_node_and_its_ancestors_and_measures_correctly(
    cx: &mut TestAppContext,
) {
    let window = cx.add_empty_window();
    let color = crate::black();
    for frame in 0..3 {
        draw(window, 600., table(&numbers(frame, 6, 7), 600., color));
    }
    reset(window);
    let mut changed = numbers(3, 6, 7);
    changed[2] = "x".repeat(40);
    draw(window, 600., table(&changed, 600., color));
    let layout = layout_stats(window);
    assert_eq!(layout.replays_failed, 1);
    assert_eq!(
        layout.computed_layouts, 1,
        "the ancestors were laid out again"
    );

    // The wide cell shrinks again on the next frame: another size change.
    reset(window);
    draw(window, 600., table(&numbers(4, 6, 7), 600., color));
    assert_eq!(layout_stats(window).replays_failed, 1);
}
