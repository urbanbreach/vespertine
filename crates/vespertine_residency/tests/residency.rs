//! PRF-017 on the framework test contexts: media residency with atlas release (A6, A8), paint-only
//! fade spans against real shaping (A3), theme and identity checks of asynchronous results
//! (A5, A7).

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use image::{Frame, ImageBuffer, Rgba};
use smallvec::SmallVec;
use vespertine::{
    AppContext as _, Bounds, Context, Corners, Entity, HeadlessAppContext, Hsla, IntoElement,
    ParentElement, Render, RenderImage, Styled, StyledText, TestAppContext, TextLayout, TextRun,
    VisualTestContext, Window, canvas, div, font, px, rgb, size,
};
use vespertine_residency::{
    BlockDocument, BlockModel, BoundedCache, CacheLimits, DependencyKey, DocumentFades,
    IdentitySource, ImageResidency, Inserted, ParsedBlock, ResidencyConfig, ThemeId, ThemeIdentity,
    decoded_bytes,
};
use vespertine_richtext::{RichLayout, plain_run, runs_from_spans};

const MIB: usize = 1024 * 1024;

fn image_of(frames: usize, width: u32, height: u32, shade: u8) -> Arc<RenderImage> {
    let frames: SmallVec<[Frame; 1]> = (0..frames)
        .map(|frame| {
            Frame::new(ImageBuffer::from_pixel(
                width,
                height,
                Rgba([shade, frame as u8 * 40, 200, 255]),
            ))
        })
        .collect();
    Arc::new(RenderImage::new(frames))
}

/// A view that paints every frame of every image in `images` into the atlas.
struct AtlasPainter {
    images: Rc<RefCell<Vec<Arc<RenderImage>>>>,
}

impl Render for AtlasPainter {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let images = self.images.clone();
        div().size_full().child(
            canvas(
                |_, _, _| (),
                move |bounds: Bounds<_>, _, window, _| {
                    for image in images.borrow().iter() {
                        for frame in 0..image.frame_count() {
                            window
                                .paint_image(
                                    bounds,
                                    bounds,
                                    Corners::default(),
                                    image.clone(),
                                    frame,
                                    false,
                                )
                                .ok();
                        }
                    }
                },
            )
            .size_full(),
        )
    }
}

type Residency = ImageResidency<&'static str, u32>;

fn residency(cx: &mut TestAppContext, config: ResidencyConfig) -> Entity<Residency> {
    cx.update(|cx| cx.new(|_| Residency::new(config)))
}

fn paint(vcx: &mut vespertine::VisualTestContext) {
    vcx.update(|window, _| window.refresh());
    vcx.run_until_parked();
}

fn frames_in_atlas(vcx: &mut vespertine::VisualTestContext, image: &RenderImage) -> usize {
    vcx.update(|window, _| window.image_atlas_frame_count(image))
}

#[vespertine::test]
fn a6_grace_reuse_expiry_release_of_every_atlas_frame_and_never_evicting_requested_items(
    cx: &mut TestAppContext,
) {
    let images = Rc::new(RefCell::new(Vec::new()));
    let painter_images = images.clone();
    let handle = cx.add_window(move |_, _| AtlasPainter {
        images: painter_images,
    });
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    let vcx = &mut vcx;
    let residency = residency(cx, ResidencyConfig::PREVIEWS);
    let class = "preview";

    // An expanded preview: a three-frame image is decoded, uploaded and requested.
    let image = image_of(3, 32, 24, 10);
    assert_eq!(decoded_bytes(&image), 3 * 32 * 24 * 4);
    residency.update(vcx, |r, cx| r.insert(&class, 1, image.clone(), cx));
    images.borrow_mut().push(image.clone());
    paint(vcx);
    assert_eq!(frames_in_atlas(vcx, &image), 3, "every frame is uploaded");

    // Collapse, then reopen within the 600 ms grace: reused, no decode, no upload.
    residency.update(vcx, |r, cx| r.release(&class, &1, cx));
    cx.executor().advance_clock(Duration::from_millis(300));
    vcx.run_until_parked();
    let reused = residency.update(vcx, |r, cx| r.request(&class, &1, cx));
    assert!(
        Arc::ptr_eq(
            reused.as_ref().expect("reused within the grace period"),
            &image
        ),
        "the very same decoded image"
    );
    drop(reused);
    // The old deadline passes with the item requested again: nothing is released.
    cx.executor().advance_clock(Duration::from_millis(400));
    vcx.run_until_parked();
    paint(vcx);
    let stats = residency.read_with(vcx, |r, _| r.stats(&class));
    assert_eq!((stats.inserts, stats.reuses, stats.evictions), (1, 1, 0));
    assert_eq!(
        frames_in_atlas(vcx, &image),
        3,
        "no re-upload and no release"
    );
    assert!(residency.read_with(vcx, |r, _| r.stats(&class).requested == 1));

    // Collapse and wait: still resident one millisecond before the grace ends.
    residency.update(vcx, |r, cx| r.release(&class, &1, cx));
    cx.executor().advance_clock(Duration::from_millis(599));
    vcx.run_until_parked();
    assert_eq!(residency.read_with(vcx, |r, _| r.stats(&class).items), 1);
    assert_eq!(frames_in_atlas(vcx, &image), 3);

    // At 600 ms the timer itself releases the item (no other event): every atlas frame is
    // dropped and the decoded data is no longer held by the residency.
    images.borrow_mut().clear();
    cx.executor().advance_clock(Duration::from_millis(1));
    vcx.run_until_parked();
    let stats = residency.read_with(vcx, |r, _| r.stats(&class));
    assert_eq!((stats.items, stats.bytes, stats.evictions), (0, 0, 1));
    assert_eq!(residency.read_with(vcx, |r, _| r.released_images()), 1);
    assert_eq!(frames_in_atlas(vcx, &image), 0, "all three frames released");
    assert_eq!(Arc::strong_count(&image), 1, "decoded data dropped");

    // Reopening after the grace period re-creates the item.
    assert!(
        residency
            .update(vcx, |r, cx| r.request(&class, &1, cx))
            .is_none()
    );
    let again = image_of(3, 32, 24, 11);
    residency.update(vcx, |r, cx| r.insert(&class, 1, again.clone(), cx));
    images.borrow_mut().push(again.clone());
    paint(vcx);
    assert_eq!(frames_in_atlas(vcx, &again), 3);
    assert_eq!(residency.read_with(vcx, |r, _| r.stats(&class).inserts), 2);
}

#[vespertine::test]
fn a6_requested_items_survive_the_budget_and_classes_never_evict_each_other(
    cx: &mut TestAppContext,
) {
    let images = Rc::new(RefCell::new(Vec::new()));
    let painter_images = images.clone();
    let handle = cx.add_window(move |_, _| AtlasPainter {
        images: painter_images,
    });
    let mut vcx = VisualTestContext::from_window(handle.into(), cx);
    let vcx = &mut vcx;
    let each = decoded_bytes(&image_of(2, 16, 16, 0));
    let residency = residency(
        cx,
        ResidencyConfig {
            budget_bytes: 2 * each,
            grace: Duration::from_millis(600),
        },
    );

    // The "other" class holds an attachment, released and idle.
    let other = image_of(2, 16, 16, 90);
    residency.update(vcx, |r, cx| {
        r.insert(&"other", 100, other.clone(), cx);
        r.release(&"other", &100, cx);
    });
    images.borrow_mut().push(other.clone());

    // Four requested items in "mine" exceed its budget of two: none of them is evicted.
    let mine: Vec<_> = (0..4).map(|n| image_of(2, 16, 16, n as u8)).collect();
    for (key, image) in mine.iter().enumerate() {
        residency.update(vcx, |r, cx| {
            r.insert(&"mine", key as u32, image.clone(), cx)
        });
        images.borrow_mut().push(image.clone());
    }
    paint(vcx);
    assert_eq!(residency.read_with(vcx, |r, _| r.stats(&"mine").items), 4);
    for image in &mine {
        assert_eq!(frames_in_atlas(vcx, image), 2);
    }

    // Releasing items brings the class back within its budget, least recently used first, and
    // releases every frame of the evicted images; the other class is untouched.
    for key in 0..3u32 {
        residency.update(vcx, |r, cx| r.release(&"mine", &key, cx));
    }
    images.borrow_mut().clear();
    vcx.run_until_parked();
    let stats = residency.read_with(vcx, |r, _| r.stats(&"mine"));
    assert!(stats.bytes <= 2 * each);
    assert_eq!(stats.requested, 1);
    assert_eq!(stats.budget_evictions, 2);
    assert_eq!(frames_in_atlas(vcx, &mine[0]), 0);
    assert_eq!(frames_in_atlas(vcx, &mine[1]), 0);
    assert!(residency.read_with(vcx, |r, _| r.stats(&"other").items == 1));
    assert!(residency.read_with(vcx, |r, _| r.stats(&"mine").items == 2));
    let _ = other;
}

#[vespertine::test]
fn a8_scrolling_two_hundred_media_rows_keeps_resident_bytes_within_the_budget(
    cx: &mut TestAppContext,
) {
    const ROWS: usize = 200;
    const ROW_HEIGHT: usize = 220;
    const VIEWPORT: usize = 480;
    let budget = 64 * MIB;
    for frame_ms in [16_u64, 1] {
        let residency = residency(cx, ResidencyConfig::PREVIEWS);
        let mut decoded = 0u32;
        let mut requested: Vec<u32> = Vec::new();
        let mut peak = 0usize;
        let mut max_requested = 0usize;
        for frame in 0..440usize {
            let offset = 100 * frame;
            let first = offset / ROW_HEIGHT;
            let last = ((offset + VIEWPORT - 1) / ROW_HEIGHT).min(ROWS - 1);
            let visible: Vec<u32> = (first..=last).map(|row| row as u32).collect();
            for &row in &requested {
                if !visible.contains(&row) {
                    residency.update(cx, |r, cx| r.release(&"preview", &row, cx));
                }
            }
            for &row in &visible {
                let hit = residency.update(cx, |r, cx| r.request(&"preview", &row, cx));
                if hit.is_none() {
                    // 1024 x 384 device pixels: 1,572,864 decoded bytes.
                    let image = image_of(1, 1024, 384, row as u8);
                    assert_eq!(decoded_bytes(&image), 1_572_864);
                    decoded += 1;
                    residency.update(cx, |r, cx| r.insert(&"preview", row, image, cx));
                }
            }
            requested = visible;
            max_requested = max_requested.max(requested.len());
            cx.executor().advance_clock(Duration::from_millis(frame_ms));
            cx.run_until_parked();
            let resident = residency.read_with(cx, |r, _| r.resident_bytes());
            peak = peak.max(resident);
            assert!(
                resident <= budget,
                "frame {frame} at {frame_ms} ms: {resident} resident bytes exceed the budget"
            );
        }
        // Rows scroll by once; each row is decoded once and the residency never grows past
        // the 42 rows of 1.5 MiB that fit.
        assert!(decoded as usize >= ROWS * 4 / 5, "rows decoded: {decoded}");
        assert!(peak <= 42 * 1_572_864, "peak {peak}");
        assert!(max_requested <= 4);
        // Everything released leaves nothing resident once the grace has passed.
        for &row in &requested {
            residency.update(cx, |r, cx| r.release(&"preview", &row, cx));
        }
        cx.executor().advance_clock(Duration::from_millis(600));
        cx.run_until_parked();
        assert_eq!(residency.read_with(cx, |r, _| r.resident_bytes()), 0);
        let stats = residency.read_with(cx, |r, _| r.stats(&"preview"));
        assert_eq!(stats.inserts, decoded as u64);
        assert_eq!(
            residency.read_with(cx, |r, _| r.released_images()),
            decoded as u64,
            "every evicted image was released"
        );
    }
}

#[test]
fn a8_diagram_raster_cache_stays_within_its_declared_budgets() {
    let mut diagrams: BoundedCache<DependencyKey, Arc<Vec<u8>>> =
        BoundedCache::new(CacheLimits::DIAGRAM_RASTER, |raster: &Arc<Vec<u8>>| {
            raster.len()
        });
    let theme = ThemeId(1);
    for row in 0..200u32 {
        let key = DependencyKey::builder()
            .part(&row)
            .part("diagram")
            .theme(theme)
            .build();
        let raster = Arc::new(vec![0u8; 1_572_864]);
        assert!(matches!(
            diagrams.insert(key, raster),
            Inserted::Retained { .. }
        ));
        let stats = diagrams.stats();
        assert!(
            stats.entries <= 64 && stats.bytes <= 64 * MIB,
            "row {row}: {stats:?}"
        );
    }
    let stats = diagrams.stats();
    assert_eq!(stats.entries, 42, "64 MiB holds 42 rasters of 1.5 MiB");
    assert_eq!(stats.evictions, 200 - 42);
}

/// One long document on screen: every block is a `StyledText` with the runs of its fade spans.
struct DocumentView {
    blocks: Vec<(String, Vec<TextRun>)>,
    layouts: Vec<TextLayout>,
}

impl Render for DocumentView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.layouts.clear();
        let mut column = div()
            .w(px(260.))
            .flex()
            .flex_col()
            .font_family("Noto Sans")
            .text_size(px(14.))
            .line_height(px(20.));
        for (text, runs) in &self.blocks {
            let styled = StyledText::new(text.clone()).with_runs(runs.clone());
            self.layouts.push(styled.layout().clone());
            column = column.child(styled);
        }
        column
    }
}

struct Paragraphs;

impl BlockModel for Paragraphs {
    type Block = ();
    type Layout = ();

    fn parse(&self, source: &str, start: usize) -> ParsedBlock<()> {
        match source[start..].find("\n\n") {
            Some(at) => ParsedBlock {
                block: (),
                end: start + at + 2,
                open: false,
            },
            None => ParsedBlock {
                block: (),
                end: source.len(),
                open: true,
            },
        }
    }

    fn measure(&self, _: &(), _: &str) {}
}

fn color(value: u32) -> Hsla {
    rgb(value).into()
}

#[test]
fn a3_fade_spans_never_reshape_or_change_layout() {
    let mut cx = HeadlessAppContext::with_platform(
        Arc::new(vespertine_wgpu::CosmicTextSystem::new("Noto Sans")),
        Arc::new(()),
        vespertine_platform::current_headless_renderer,
    );
    let window = cx
        .open_window(size(px(300.), px(400.)), |_, cx| {
            cx.new(|_| DocumentView {
                blocks: Vec::new(),
                layouts: Vec::new(),
            })
        })
        .unwrap();
    let view = window.entity(&cx).unwrap();
    let plain_color = color(0x1f2328);
    let base_font = font("Noto Sans");

    let mut document = BlockDocument::new(Paragraphs);
    document.update(
        "The first settled paragraph wraps over a couple of lines of text.\n\n\
         A second settled paragraph keeps the first company on the page.\n\n\
         The streaming tail begins here and",
    );
    let start = Instant::now();
    let mut fades = DocumentFades::new(Duration::from_millis(200));

    // The streamed chunk arrives: this changes text and legitimately shapes it.
    let chunk = " keeps growing with every chunk that arrives from the stream.";
    let old_len = document.source().len();
    document.append(chunk);
    fades.arrive(document.blocks_in(old_len..old_len + chunk.len()), start);

    let render_at = |cx: &mut HeadlessAppContext,
                     fades: &DocumentFades,
                     document: &BlockDocument<Paragraphs>,
                     at: Instant| {
        let blocks: Vec<(String, Vec<TextRun>)> = document
            .blocks()
            .iter()
            .map(|entry| {
                let text = document.text(entry).trim_end().to_string();
                let spans = fades
                    .block(entry.id)
                    .map(|spans| spans.color_spans(plain_color, at))
                    .unwrap_or_default();
                let runs = if spans.is_empty() {
                    vec![plain_run(text.len(), &base_font, plain_color)]
                } else {
                    runs_from_spans(&text, &spans, plain_color, &base_font)
                };
                (text, runs)
            })
            .collect();
        cx.update(|app| {
            view.update(app, |view, cx| {
                view.blocks = blocks;
                cx.notify();
            })
        });
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .expect("draw");
        cx.run_until_parked();
    };
    let signature = |cx: &mut HeadlessAppContext| -> Vec<_> {
        let layouts = cx.update(|app| view.read(app).layouts.clone());
        layouts
            .into_iter()
            .map(|layout| {
                let rich = RichLayout::new(layout);
                (
                    rich.line_starts(),
                    rich.unwrapped_width(),
                    rich.bounds().size,
                )
            })
            .collect()
    };
    let shapes = |cx: &mut HeadlessAppContext| cx.update(|app| app.text_system().shape_stats());

    render_at(&mut cx, &fades, &document, start);
    render_at(&mut cx, &fades, &document, start);
    let baseline = signature(&mut cx);
    assert!(
        baseline.len() == 3 && baseline[2].0.len() > 1,
        "the tail wraps: {baseline:?}"
    );
    let before = shapes(&mut cx);
    assert!(before.platform_line_shapes > 0);

    // Advance the fade across frames, then remove the spans: only colors change.
    for ms in [0u64, 16, 50, 120, 199, 200, 400] {
        let now = start + Duration::from_millis(ms);
        render_at(&mut cx, &fades, &document, now);
        assert_eq!(signature(&mut cx), baseline, "layout at {ms} ms");
        assert_eq!(
            shapes(&mut cx).platform_line_shapes,
            before.platform_line_shapes,
            "reshaped at {ms} ms"
        );
    }
    fades.advance(start + Duration::from_millis(400));
    assert!(fades.is_empty());
    render_at(
        &mut cx,
        &fades,
        &document,
        start + Duration::from_millis(500),
    );
    assert_eq!(signature(&mut cx), baseline, "after the spans were removed");
    assert_eq!(
        shapes(&mut cx).platform_line_shapes,
        before.platform_line_shapes
    );
}

#[vespertine::test]
fn a5_a_themed_raster_that_completes_after_the_theme_changed_is_discarded(cx: &mut TestAppContext) {
    let theme = ThemeIdentity::new(1);
    let row = Rc::new(IdentitySource::new(&theme, 4, 77));
    let rasters: Rc<RefCell<BoundedCache<DependencyKey, usize>>> = Rc::new(RefCell::new(
        BoundedCache::new(CacheLimits::DIAGRAM_RASTER, |bytes| *bytes),
    ));
    let key = |theme: u64| {
        DependencyKey::builder()
            .part("graph a-->b")
            .theme(ThemeId(theme))
            .build()
    };

    // A raster for theme 1 is requested; the theme changes to 2 while it is pending.
    let ticket = row.ticket();
    let published = Rc::new(Cell::new(false));
    let (cache, flag) = (rasters.clone(), published.clone());
    let task = cx.spawn(move |cx| async move {
        let bytes = cx.background_executor().spawn(async { 4_096usize }).await;
        let done = ticket.publish(|| {
            cache.borrow_mut().insert(key(1), bytes);
        });
        flag.set(done.is_some());
    });
    theme.set(2);
    cx.run_until_parked();
    task.detach();
    assert!(!published.get(), "the stale result was discarded");
    assert!(rasters.borrow().is_empty());

    // The same work requested under the new theme publishes.
    let ticket = row.ticket();
    let (cache, flag) = (rasters.clone(), published.clone());
    cx.spawn(move |cx| async move {
        let bytes = cx.background_executor().spawn(async { 4_096usize }).await;
        flag.set(
            ticket
                .publish(|| {
                    cache.borrow_mut().insert(key(2), bytes);
                })
                .is_some(),
        );
    })
    .detach();
    cx.run_until_parked();
    assert!(published.get());
    assert!(rasters.borrow().contains(&key(2)));
    assert!(!rasters.borrow().contains(&key(1)));
}

#[vespertine::test]
fn a7_late_results_with_a_changed_row_or_source_hash_are_not_published(cx: &mut TestAppContext) {
    let theme = ThemeIdentity::new(1);
    let row = IdentitySource::new(&theme, 9, 500);
    let published: Rc<RefCell<Vec<&'static str>>> = Rc::default();

    let spawn_for = |cx: &mut TestAppContext, name: &'static str| {
        let ticket = row.ticket();
        let published = published.clone();
        cx.spawn(move |cx| async move {
            cx.background_executor()
                .timer(Duration::from_millis(100))
                .await;
            ticket.publish(|| published.borrow_mut().push(name));
        })
    };
    let current = spawn_for(cx, "current");
    let stale_hash = spawn_for(cx, "stale hash");
    row.set_content(501);
    let stale_row = spawn_for(cx, "stale row");
    row.rebind(10, 501);
    cx.executor().advance_clock(Duration::from_millis(100));
    cx.run_until_parked();
    // Only a task whose captured identity still equals the current one publishes: the ones
    // captured before the content or row changed do not.
    assert_eq!(
        *published.borrow(),
        Vec::<&str>::new(),
        "no task matches the final identity"
    );
    let fresh = spawn_for(cx, "fresh");
    cx.executor().advance_clock(Duration::from_millis(100));
    cx.run_until_parked();
    assert_eq!(*published.borrow(), vec!["fresh"]);
    for task in [current, stale_hash, stale_row, fresh] {
        task.detach();
    }
}

struct MediaRow {
    identity: IdentitySource,
    media: Arc<Vec<u8>>,
}

#[vespertine::test]
fn a7_a_removed_rows_pending_task_publishes_nothing_and_does_not_keep_it_alive(
    cx: &mut TestAppContext,
) {
    let theme = ThemeIdentity::new(0);
    let row = MediaRow {
        identity: IdentitySource::new(&theme, 3, 1),
        media: Arc::new(vec![0u8; 1024]),
    };
    let weak_media = Arc::downgrade(&row.media);
    let mut rows = vec![row];

    let published = Rc::new(Cell::new(0));
    let ticket = rows[0].identity.ticket();
    let counter = published.clone();
    let task = cx.spawn(move |cx| async move {
        // Still pending while the row is removed.
        cx.background_executor().timer(Duration::from_secs(1)).await;
        ticket.publish(|| counter.set(counter.get() + 1));
    });
    cx.run_until_parked();
    assert!(weak_media.upgrade().is_some());

    rows.clear();
    assert!(
        weak_media.upgrade().is_none(),
        "the pending task keeps neither the removed row nor its media alive"
    );
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(
        published.get(),
        0,
        "the removed row's task completed without publishing"
    );
    task.detach();
}
