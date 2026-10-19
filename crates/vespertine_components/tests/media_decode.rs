//! CMP-020 A8 (decode limits), A9 (SVG raster size), A10 (memory, cache and release).

mod media_support;

use media_support::{animated_gif, jpeg, png, renderer, solid_image};
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vespertine::{
    AppContext, Context, IntoElement, ParentElement, Pixels, Render, Size, Styled, TestAppContext,
    Window, div, px, size,
};
use vespertine_components::media::cache::{
    COLLAPSED_GRACE, MediaCache, MediaClass, MediaKey, MediaStatus, ValidationPolicy, retry_delay,
    start_load, thumbnail_bound,
};
use vespertine_components::media::decode::{
    ATTACHMENT_BYTE_CAP, DecodedMedia, MediaError, WORKSPACE_PREVIEW_BYTE_CAP,
    decode_generated_image, decode_icon, decode_image, decode_image_with_cap, decode_thumbnail,
    raster_retained_bytes,
};
use vespertine_components::media::svg::{enlarged_raster, estimated_bytes, raster_size};
use vespertine_components::media::{ImageView, release};

#[test]
fn a8_oversized_wide_png_is_rejected_without_panicking() {
    let wide = png(5000, 10);
    let error = decode_image("image/png", &wide, &renderer()).unwrap_err();
    assert!(matches!(error, MediaError::Invalid(_)), "{error}");
    let error = decode_generated_image("image/png", &wide, ATTACHMENT_BYTE_CAP).unwrap_err();
    assert!(matches!(error, MediaError::Invalid(_)), "{error}");
}

#[test]
fn a8_input_beyond_the_byte_cap_is_rejected_before_decoding() {
    let big = vec![0u8; 9 * 1024 * 1024];
    let error = decode_generated_image("image/png", &big, WORKSPACE_PREVIEW_BYTE_CAP).unwrap_err();
    assert_eq!(
        error,
        MediaError::TooLarge {
            bytes: 9 * 1024 * 1024,
            cap: WORKSPACE_PREVIEW_BYTE_CAP
        }
    );
    let error = decode_image_with_cap("image/png", &big, WORKSPACE_PREVIEW_BYTE_CAP, &renderer())
        .unwrap_err();
    assert!(matches!(error, MediaError::TooLarge { .. }));
    let error =
        decode_icon("image/png", &big, WORKSPACE_PREVIEW_BYTE_CAP, &renderer()).unwrap_err();
    assert!(matches!(error, MediaError::TooLarge { .. }));
}

#[test]
fn a8_truncated_and_garbage_input_are_recoverable_errors() {
    let full = png(64, 64);
    for cut in [0, 4, 20, full.len() / 2, full.len() - 20] {
        let result = decode_image("image/png", &full[..cut], &renderer());
        assert!(
            matches!(result, Err(MediaError::Invalid(_))),
            "cut {cut}: {result:?}"
        );
    }
    let result = decode_image("image/png", b"not an image at all", &renderer());
    assert!(matches!(result, Err(MediaError::Invalid(_))));
}

#[test]
fn a8_generated_previews_check_the_declared_type_against_the_bytes() {
    let jpeg_bytes = jpeg(32, 32);
    assert_eq!(
        decode_generated_image("image/png", &jpeg_bytes, ATTACHMENT_BYTE_CAP).unwrap_err(),
        MediaError::FormatMismatch
    );
    assert_eq!(
        decode_generated_image("image/bmp", &jpeg_bytes, ATTACHMENT_BYTE_CAP).unwrap_err(),
        MediaError::Unsupported
    );
    assert_eq!(
        decode_generated_image("image/svg+xml", b"<svg/>", ATTACHMENT_BYTE_CAP).unwrap_err(),
        MediaError::Unsupported
    );
    assert!(decode_generated_image("image/jpeg", &jpeg_bytes, ATTACHMENT_BYTE_CAP).is_ok());
}

#[test]
fn a8_an_animated_gif_becomes_one_static_frame() {
    let gif = animated_gif(16, 8);
    let media = decode_generated_image("image/gif", &gif, ATTACHMENT_BYTE_CAP).unwrap();
    assert_eq!(media.image().frame_count(), 1);
    assert_eq!(media.natural_size(), size(px(16.), px(8.)));
    assert!(media.encoded_png().is_some());
    let media = decode_image("image/gif", &gif, &renderer()).unwrap();
    assert_eq!(media.image().frame_count(), 1);
}

#[test]
fn generated_previews_are_reduced_to_the_bound() {
    let media = decode_generated_image("image/png", &png(4000, 1000), ATTACHMENT_BYTE_CAP).unwrap();
    let natural = media.natural_size();
    assert_eq!(f32::from(natural.width), 2048.0);
    assert!(f32::from(natural.height) <= 2048.0);
    let icon = decode_icon(
        "image/png",
        &png(300, 100),
        ATTACHMENT_BYTE_CAP,
        &renderer(),
    )
    .unwrap();
    assert_eq!(f32::from(icon.natural_size().width), 64.0);
    // A small image is never enlarged.
    let small = decode_generated_image("image/png", &png(30, 20), ATTACHMENT_BYTE_CAP).unwrap();
    assert_eq!(small.natural_size(), size(px(30.), px(20.)));
}

const HOSTILE_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="200" height="100" viewBox="0 0 200 100" onload="alert(1)">
  <script>alert('x')</script>
  <foreignObject width="10" height="10"><div xmlns="http://www.w3.org/1999/xhtml">html</div></foreignObject>
  <image href="file:///etc/passwd" x="0" y="0" width="50" height="50"/>
  <image xlink:href="http://example.invalid/a.png" x="50" y="0" width="50" height="50"/>
  <image href="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==" x="100" y="0" width="50" height="50"/>
  <rect x="150" y="50" width="40" height="40" fill="#ff0000"/>
</svg>"##;

#[test]
fn a8_svg_is_sanitized_without_loading_anything() {
    let media = decode_image("image/svg+xml", HOSTILE_SVG.as_bytes(), &renderer()).unwrap();
    let source = media.sanitized_source().unwrap();
    for forbidden in [
        "script",
        "alert",
        "foreignObject",
        "file:",
        "example.invalid",
        "data:",
        "<image",
        "onload",
    ] {
        assert!(
            !source.contains(forbidden),
            "sanitized source contains {forbidden}: {source}"
        );
    }
    assert!(source.contains("<rect") || source.contains("<path"));
    assert_eq!(media.natural_size(), size(px(200.), px(100.)));
    assert!(media.is_vector());
    // The red rect (150..190, 50..90) is rendered; the raster is not empty.
    let image = media.image();
    let dims = image.size(0);
    let bytes = image.as_bytes(0).unwrap();
    let (w, h) = (
        i32::from(dims.width) as usize,
        i32::from(dims.height) as usize,
    );
    let px_at = |x: usize, y: usize| &bytes[(y * w + x) * 4..(y * w + x) * 4 + 4];
    let inside = px_at(w * 85 / 100, h * 70 / 100);
    assert!(
        inside[2] > 200 && inside[3] > 200,
        "red rect visible: {inside:?}"
    );
    let outside = px_at(w / 4, h / 4);
    assert_eq!(
        outside[3], 0,
        "the external image did not load: {outside:?}"
    );
}

#[test]
fn a8_svg_text_survives_an_unavailable_family() {
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="300" height="80" viewBox="0 0 300 80">
        <text x="10" y="50" font-size="40" font-family="NoSuchFamilyXyzzy" fill="#000">Hello</text></svg>"##;
    let media = decode_image("image/svg+xml", svg.as_bytes(), &renderer()).unwrap();
    let source = media.sanitized_source().unwrap();
    assert!(source.contains("<path"), "text became outlines: {source}");
    let image = media.image();
    let bytes = image.as_bytes(0).unwrap();
    assert!(
        bytes.chunks_exact(4).any(|pixel| pixel[3] > 0),
        "the text is visible"
    );
}

#[test]
fn a8_an_svg_over_the_cap_is_rejected() {
    let big = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'><!-- {} --></svg>",
        "x".repeat(2000)
    );
    let error =
        decode_image_with_cap("image/svg+xml", big.as_bytes(), 1000, &renderer()).unwrap_err();
    assert!(matches!(error, MediaError::TooLarge { .. }));
    assert!(decode_image("image/svg+xml", b"<svg", &renderer()).is_err());
}

#[test]
fn a9_svg_raster_size_follows_the_formula() {
    let natural = (1000.0, 500.0);
    // fit 0.9, scale min(1.8, 4.096, 8.192, 1.4482) = 1.4482.
    let (w, h) = raster_size(natural, (900.0, 480.0), 2.0, 1_048_576);
    assert_eq!((w, h), (1448, 724));
    assert!(w as u64 * h as u64 <= 1_048_576);

    let (w, h) = raster_size((8000.0, 4000.0), (900.0, 480.0), 2.0, 1_048_576);
    assert!(
        w <= 4096 && h <= 4096 && (w as u64 * h as u64) <= 1_048_576,
        "{w} x {h}"
    );
    let (w, h) = raster_size((8000.0, 4000.0), (8000.0, 4000.0), 4.0, 100_000_000);
    assert!(w <= 4096 && h <= 4096, "{w} x {h}");
    // The device pixel ratio is clamped to [1, 4].
    assert_eq!(
        raster_size(natural, (100.0, 100.0), 100.0, u64::MAX),
        raster_size(natural, (100.0, 100.0), 4.0, u64::MAX)
    );
    assert_eq!(
        raster_size(natural, (100.0, 100.0), 0.1, u64::MAX),
        raster_size(natural, (100.0, 100.0), 1.0, u64::MAX)
    );
    // An icon preview: viewport 16 x 16, ratio 2, 4096 pixels.
    let (w, h) = raster_size((24.0, 24.0), (16.0, 16.0), 2.0, 4096);
    assert_eq!((w, h), (32, 32));
    // A degenerate sliver keeps at least one pixel per side.
    assert_eq!(raster_size((10_000.0, 1.0), (100.0, 100.0), 1.0, 4096).1, 1);
}

#[test]
fn a9_enlarged_raster_follows_the_budget() {
    // Plenty of budget: capped at 2 x 1,048,576 pixels for the 1600 x 1000 viewport.
    let (w, h) =
        enlarged_raster((1000.0, 500.0), 5000, (1448, 724), 2.0, 32 * 1024 * 1024).unwrap();
    assert!(w as u64 * h as u64 <= 2 * 1_048_576);
    assert!(w >= 1448);
    // A budget that cannot hold the current raster keeps it.
    assert_eq!(
        enlarged_raster((1000.0, 500.0), 5000, (1448, 724), 2.0, 100_000),
        None
    );
    assert_eq!(
        estimated_bytes(5000, (1448, 724)),
        2 * 5000 + 1024 + 1448 * 724 * 8
    );
}

#[test]
fn a9_vector_variants_are_derived_and_reused() {
    let renderer = renderer();
    let media = decode_image("image/svg+xml", HOSTILE_SVG.as_bytes(), &renderer).unwrap();
    let current = media.raster_size().unwrap();
    // Same view: the very same raster.
    let same = media.preview_for_view(2.0, &renderer).unwrap();
    assert_eq!(same.raster_size(), Some(current));
    assert!(std::sync::Arc::ptr_eq(same.image(), media.image()));
    // A smaller view yields a smaller raster.
    let small = media
        .for_view(size(px(100.), px(100.)), 1.0, 1_048_576, &renderer)
        .unwrap();
    assert_ne!(small.raster_size(), Some(current));
    assert!(small.retained_bytes() < media.retained_bytes());
    // Preview within: a larger raster is refused when it exceeds both the current estimate
    // and the memory still available; the current raster stays.
    let kept = small
        .preview_within(size(px(900.), px(480.)), 2.0, 0, &renderer)
        .unwrap();
    assert_eq!(kept.raster_size(), small.raster_size());
    let grown = small
        .preview_within(size(px(900.), px(480.)), 2.0, usize::MAX, &renderer)
        .unwrap();
    assert_eq!(grown.raster_size(), Some(current));
    // Enlarged variant and its cached reuse.
    let enlarged = media
        .enlarged(2.0, 32 * 1024 * 1024, None, &renderer)
        .unwrap();
    let again = media
        .enlarged(2.0, 32 * 1024 * 1024, Some(&enlarged), &renderer)
        .unwrap();
    assert!(std::sync::Arc::ptr_eq(enlarged.image(), again.image()));
    // Raster media never changes.
    let raster = decode_image("image/png", &png(40, 30), &renderer).unwrap();
    assert!(std::sync::Arc::ptr_eq(
        raster.enlarged(2.0, 1, None, &renderer).unwrap().image(),
        raster.image()
    ));
}

#[test]
fn a10_a_raster_of_2400_by_1600_costs_encoded_plus_30_720_000_bytes() {
    assert_eq!(
        raster_retained_bytes(12_345, 2400, 1600),
        12_345 + 30_720_000
    );
    let media = decode_image("image/png", &png(300, 200), &renderer()).unwrap();
    let encoded = media.encoded_png().unwrap().len();
    assert_eq!(media.retained_bytes(), encoded + 300 * 200 * 8);
    assert_eq!(media.natural_size(), size(px(300.), px(200.)));
}

fn media_of(width: u32) -> DecodedMedia {
    decode_image("image/png", &png(width, width), &renderer()).unwrap()
}

#[test]
fn a10_inserting_past_the_budget_evicts_the_least_recently_used_unprotected_entry() {
    let sample = media_of(64);
    let each = sample.retained_bytes();
    let mut cache = MediaCache::with_budgets(each * 2 + each / 2, each * 2 + each / 2);
    let now = Instant::now();
    let policy = ValidationPolicy::WorkspacePreview;
    let (a, b, c, d) = (
        MediaKey::new("a", policy),
        MediaKey::new("b", policy),
        MediaKey::new("c", policy),
        MediaKey::new("d", policy),
    );
    for key in [&a, &b] {
        assert!(cache.begin_load(key, now));
        cache.complete(key, Ok(media_of(64)), now);
    }
    assert!(
        cache.get(&a).is_some(),
        "touching a makes b the least recently used"
    );
    cache.complete(&c, Ok(media_of(64)), now);
    assert_eq!(cache.status(&b, now), MediaStatus::Missing, "b was evicted");
    assert_eq!(cache.status(&a, now), MediaStatus::Ready);
    assert_eq!(cache.status(&c, now), MediaStatus::Ready);
    assert_eq!(cache.pending_free().len(), 1);

    // Protected entries are skipped; the entry just inserted is never the victim.
    cache.set_protected(&a, true);
    cache.set_protected(&c, true);
    cache.complete(&d, Ok(media_of(64)), now);
    assert_eq!(cache.status(&d, now), MediaStatus::Ready);
    assert!(
        cache.used_bytes(MediaClass::Preview) > 2 * each + each / 2,
        "nothing evictable: over budget"
    );
    cache.set_protected(&a, false);
    let e = MediaKey::new("e", policy);
    cache.complete(&e, Ok(media_of(64)), now);
    assert_eq!(
        cache.status(&a, now),
        MediaStatus::Missing,
        "unprotected LRU goes first"
    );
}

#[test]
fn a10_previews_and_attachments_have_separate_budgets_and_policies_separate_keys() {
    let each = media_of(64).retained_bytes();
    let mut cache = MediaCache::with_budgets(each + each / 2, each * 10);
    let now = Instant::now();
    let attachment = MediaKey::new("x", ValidationPolicy::Attachment);
    let workspace = MediaKey::new("x", ValidationPolicy::WorkspacePreview);
    assert_ne!(attachment, workspace);
    cache.begin_load(&attachment, now);
    cache.complete(&attachment, Ok(media_of(64)), now);
    for name in ["p1", "p2", "p3"] {
        let key = MediaKey::new(name, ValidationPolicy::WorkspacePreview);
        cache.begin_load(&key, now);
        cache.complete(&key, Ok(media_of(64)), now);
    }
    assert_eq!(
        cache.status(&attachment, now),
        MediaStatus::Ready,
        "previews cannot starve attachments"
    );
    assert!(cache.used_bytes(MediaClass::Preview) <= each + each / 2 + each);
    // In-flight and error entries are keyed by policy too.
    assert!(cache.begin_load(&workspace, now));
    assert!(!cache.begin_load(&workspace, now), "already loading");
    assert!(cache.begin_load(&MediaKey::new("x", ValidationPolicy::Icon), now));
}

#[test]
fn failed_loads_follow_the_retry_ladder() {
    let ladder: Vec<u64> = (1..=6).map(|n| retry_delay(n).as_secs()).collect();
    assert_eq!(ladder, [2, 4, 8, 15, 15, 15]);
    let mut cache = MediaCache::new();
    let key = MediaKey::new("bad", ValidationPolicy::Attachment);
    let t0 = Instant::now();
    assert!(cache.begin_load(&key, t0));
    cache.complete(&key, Err(MediaError::Unsupported), t0);
    match cache.status(&key, t0) {
        MediaStatus::Failed { retry_in, .. } => assert_eq!(retry_in, Duration::from_secs(2)),
        other => panic!("{other:?}"),
    }
    assert!(!cache.begin_load(&key, t0 + Duration::from_secs(1)));
    assert!(cache.begin_load(&key, t0 + Duration::from_secs(2)));
    cache.complete(
        &key,
        Err(MediaError::Unsupported),
        t0 + Duration::from_secs(2),
    );
    match cache.status(&key, t0 + Duration::from_secs(2)) {
        MediaStatus::Failed { retry_in, .. } => assert_eq!(retry_in, Duration::from_secs(4)),
        other => panic!("{other:?}"),
    }
    // A success resets the ladder.
    assert!(cache.begin_load(&key, t0 + Duration::from_secs(6)));
    cache.complete(&key, Ok(media_of(8)), t0 + Duration::from_secs(6));
    assert_eq!(
        cache.status(&key, t0 + Duration::from_secs(6)),
        MediaStatus::Ready
    );
}

#[test]
fn offscreen_previews_are_dropped_only_over_budget_after_grace_and_staleness() {
    let each = media_of(64).retained_bytes();
    let mut cache = MediaCache::with_budgets(each * 2 + each / 2, each);
    let t0 = Instant::now();
    let policy = ValidationPolicy::Generated { cap: 1 << 20 };
    let key = |name: &'static str| MediaKey::new(name, policy);
    let (visible, collapsed, stale) = (key("visible"), key("collapsed"), key("stale"));
    // Three protected previews exceed the budget; nothing is evictable.
    for key in [&visible, &collapsed, &stale] {
        cache.begin_load(key, t0);
        cache.complete(key, Ok(media_of(64)), t0);
        cache.set_protected(key, true);
    }
    assert!(cache.used_bytes(MediaClass::Preview) > each * 2 + each / 2);
    // The host stops protecting the two it no longer shows; the user collapsed one of them.
    cache.set_protected(&collapsed, false);
    cache.set_protected(&stale, false);
    cache.mark_collapsed(&collapsed, t0);
    // Inside the grace period and inside the 2 s stale window nothing is offscreen.
    assert_eq!(cache.evict_offscreen(t0 + Duration::from_millis(300)), 0);
    // Past the 600 ms grace the collapsed preview goes; that brings the pool under budget.
    assert_eq!(
        cache.evict_offscreen(t0 + COLLAPSED_GRACE + Duration::from_millis(1)),
        1
    );
    assert_eq!(cache.status(&collapsed, t0), MediaStatus::Missing);
    assert_eq!(cache.status(&stale, t0), MediaStatus::Ready);
    assert_eq!(cache.status(&visible, t0), MediaStatus::Ready);
    // Under budget again: even a stale preview stays.
    assert_eq!(cache.evict_offscreen(t0 + Duration::from_secs(60)), 0);
    // Over budget once more, a preview not painted for 2 s is dropped, a recently painted one is not.
    cache.set_protected(&visible, false);
    cache.mark_painted(&visible, t0 + Duration::from_secs(59));
    let extra = key("extra");
    cache.begin_load(&extra, t0);
    cache.set_protected(&stale, true);
    cache.set_protected(&visible, true);
    cache.complete(&extra, Ok(media_of(64)), t0);
    cache.set_protected(&stale, false);
    cache.set_protected(&visible, false);
    cache.set_protected(&extra, true);
    assert!(cache.used_bytes(MediaClass::Preview) > each * 2 + each / 2);
    assert_eq!(cache.evict_offscreen(t0 + Duration::from_secs(60)), 1);
    assert_eq!(
        cache.status(&stale, t0),
        MediaStatus::Missing,
        "stale for 60 s"
    );
    assert_eq!(
        cache.status(&visible, t0),
        MediaStatus::Ready,
        "painted 1 s ago"
    );
}

#[test]
fn decoded_list_thumbnails_fit_within_twice_the_display_frame() {
    let frame = size(px(560.), px(220.));
    let media =
        decode_thumbnail("image/png", &png(3000, 1000), ATTACHMENT_BYTE_CAP, frame).unwrap();
    let natural = media.natural_size();
    assert!(
        f32::from(natural.width) <= 1120.0 && f32::from(natural.height) <= 440.0,
        "{natural:?}"
    );
    assert_eq!(
        f32::from(natural.width),
        1120.0,
        "the width is the limiting side"
    );
    let small = decode_thumbnail("image/png", &png(300, 100), ATTACHMENT_BYTE_CAP, frame).unwrap();
    assert_eq!(
        small.natural_size(),
        size(px(300.), px(100.)),
        "never enlarged"
    );
    assert_eq!(
        decode_thumbnail("image/jpeg", &png(30, 10), ATTACHMENT_BYTE_CAP, frame).unwrap_err(),
        MediaError::FormatMismatch
    );
}

#[vespertine::test]
fn loads_decode_off_the_calling_path_and_land_in_the_cache(cx: &mut TestAppContext) {
    let cache = cx.update(|cx| cx.new(|_| MediaCache::new()));
    let key = MediaKey::new("async", ValidationPolicy::Attachment);
    let bytes = png(40, 30);
    let started = cx.update(|cx| {
        let bytes = bytes.clone();
        start_load(&cache, key.clone(), cx, move || {
            decode_generated_image("image/png", &bytes, ATTACHMENT_BYTE_CAP)
        })
    });
    assert!(started);
    // The load is in flight: nothing is resident yet and a second start is refused.
    cx.read(|cx| {
        assert_eq!(
            cache.read(cx).status(&key, Instant::now()),
            MediaStatus::Loading
        )
    });
    assert!(!cx.update(|cx| start_load(&cache, key.clone(), cx, || Err(MediaError::Unsupported))));
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(
            cache.read(cx).status(&key, Instant::now()),
            MediaStatus::Ready
        )
    });
    // A failing decode becomes an error entry with the retry delay.
    let bad = MediaKey::new("bad", ValidationPolicy::Attachment);
    cx.update(|cx| start_load(&cache, bad.clone(), cx, || Err(MediaError::Unsupported)));
    cx.run_until_parked();
    cx.read(|cx| {
        assert!(matches!(
            cache.read(cx).status(&bad, Instant::now()),
            MediaStatus::Failed {
                error: MediaError::Unsupported,
                ..
            }
        ))
    });
}

#[test]
fn list_thumbnails_are_bounded_to_twice_the_display_frame() {
    let bound: Size<Pixels> = thumbnail_bound(size(px(560.), px(220.)));
    assert_eq!(bound, size(px(1120.), px(440.)));
    assert_eq!(
        vespertine_components::media::cache::MAX_PREVIEWS_PER_ITEM,
        4
    );
}

struct AtlasHost {
    view: ImageView,
    image: std::sync::Arc<vespertine::RenderImage>,
    renders: Rc<Cell<usize>>,
}

impl Render for AtlasHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div().w(px(200.)).h(px(100.)).flex().child(self.view.render(
            self.image.clone(),
            size(px(200.), px(100.)),
            None,
            None,
        ))
    }
}

#[vespertine::test]
fn a10_eviction_drops_the_image_from_every_windows_atlas_after_the_frame(cx: &mut TestAppContext) {
    let media = media_of(32);
    let image = media.image().clone();
    let view = ImageView::new();
    let host_image = image.clone();
    let host_view = view;
    let (_, vcx) = cx.add_window_view(move |_, _| AtlasHost {
        view: host_view,
        image: host_image,
        renders: Rc::new(Cell::new(0)),
    });
    vcx.run_until_parked();
    vcx.update(|window, _| window.refresh());
    vcx.run_until_parked();
    vcx.update(|window, _| {
        assert!(
            window.has_image_atlas_entry(&image),
            "painted into the atlas"
        )
    });

    let each = media.retained_bytes();
    let cache = vcx.update(|_, cx| cx.new(|_| MediaCache::with_budgets(each, each)));
    let key = MediaKey::new("one", ValidationPolicy::WorkspacePreview);
    let other = MediaKey::new("two", ValidationPolicy::WorkspacePreview);
    vcx.update(|window, cx| {
        cache.update(cx, |cache, _| {
            cache.complete(&key, Ok(media.clone()), Instant::now());
            cache.complete(&other, Ok(media_of(32)), Instant::now());
        });
        assert_eq!(
            cache.read(cx).status(&key, Instant::now()),
            MediaStatus::Missing
        );
        cache.update(cx, |cache, cx| cache.flush_pending(cx));
        assert!(
            window.has_image_atlas_entry(&image),
            "the release is deferred until after the frame that evicted it"
        );
    });
    vcx.run_until_parked();
    vcx.update(|window, _| {
        assert!(
            !window.has_image_atlas_entry(&image),
            "dropped from the atlas after the frame"
        )
    });

    // The explicit release function does the same for any media.
    let probe = media_of(24);
    let probe_image = probe.image().clone();
    let view = ImageView::new();
    let (_, second) = cx.add_window_view({
        let image = probe_image.clone();
        move |_, _| AtlasHost {
            view,
            image,
            renders: Rc::new(Cell::new(0)),
        }
    });
    second.run_until_parked();
    second.update(|window, _| window.refresh());
    second.run_until_parked();
    second.update(|window, cx| {
        assert!(window.has_image_atlas_entry(&probe_image));
        release(&probe, cx);
    });
    second.run_until_parked();
    second.update(|window, _| assert!(!window.has_image_atlas_entry(&probe_image)));
    let _ = solid_image;
}
