//! CMP-013: themed diagram media with source fallback.

use std::io::ErrorKind;
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use vespertine_components::diagram::{
    COMPLEXITY_LIMIT, DiagramCache, DiagramMedia, DiagramRenderer, Lookup, MAX_BYTES, MAX_ENTRIES,
    OUTPUT_LIMIT, Palette, RENDER_PANIC, Status, View, check_limits, count_tokens, render,
    render_with,
};
use vespertine_theme::color::Color;
use vespertine_theme::document::DocumentTheme;

fn dark() -> Palette {
    Palette::from_theme(&DocumentTheme::dark())
}

fn svg_of(width: u32, height: u32) -> String {
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}"><rect width="{width}" height="{height}" fill="#336699"/></svg>"##
    )
}

fn media(width: u32, height: u32) -> DiagramMedia {
    DiagramMedia::from_svg(&svg_of(width, height), View::default(), MAX_BYTES).expect("media")
}

// ---- A2 -------------------------------------------------------------------------------------

const FAMILIES: &[(&str, &str)] = &[
    (
        "flowchart",
        "flowchart TD\n  A[Start] --> B{Decision}\n  B -->|yes| C[Done]\n  B -->|no| D((Again))\n  C --> E\n  D --> A\n  subgraph S\n    E[End]\n  end\n",
    ),
    (
        "sequence",
        "sequenceDiagram\n  participant A as Alice\n  participant B as Bob\n  A->>B: Hello\n  activate B\n  Note over A,B: A note\n  B-->>A: Hi\n  deactivate B\n",
    ),
    (
        "class",
        "classDiagram\n  class Animal {\n    +String name\n    +eat()\n  }\n  class Dog\n  Animal <|-- Dog\n",
    ),
    (
        "state",
        "stateDiagram-v2\n  [*] --> Idle\n  Idle --> Running: start\n  Running --> Idle: stop\n  Running --> [*]\n",
    ),
    (
        "entity-relationship",
        "erDiagram\n  CUSTOMER ||--o{ ORDER : places\n  ORDER ||--|{ LINE_ITEM : contains\n  CUSTOMER {\n    string name\n    int id\n  }\n",
    ),
    (
        "gantt",
        "gantt\n  title Plan\n  dateFormat YYYY-MM-DD\n  section Build\n  Design :a1, 2024-01-01, 10d\n  Code :after a1, 5d\n",
    ),
];

#[test]
fn every_supported_family_renders_and_rasterizes() {
    let palette = dark();
    for (name, source) in FAMILIES {
        let svg = render(source, &palette).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(svg.starts_with("<svg"), "{name}");
        let media = DiagramMedia::from_svg(&svg, View::default(), MAX_BYTES)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let (w, h) = media.raster_size();
        assert!(w > 20 && h > 20, "{name}: {w}x{h}");
        // Something was drawn (text, shapes, lines).
        let pixels = media.image().as_bytes(0).unwrap();
        assert!(pixels.chunks_exact(4).any(|p| p[3] > 0), "{name} is blank");
    }
}

#[test]
fn an_unrecognized_family_fails_with_a_reason_instead_of_rendering() {
    let error = render("notADiagram\n  foo", &dark()).unwrap_err();
    assert!(!error.is_empty());
    assert!(render("", &dark()).is_err());
}

#[test]
fn the_gantt_border_is_the_accent_line_and_default_elements_are_restyled() {
    let palette = dark();
    let svg = render(FAMILIES[5].1, &palette).unwrap();
    let hex = palette.accent_line.to_hex();
    assert!(
        svg.to_ascii_lowercase().contains(&hex),
        "the gantt border color {hex} is in the SVG"
    );
    assert!(!svg.contains(r#"rx="3""#), "corner radius 3 became 8");
    assert!(svg.contains(r#"rx="8""#));
    assert!(!svg.to_ascii_lowercase().contains("e2e8f0"));
    assert!(
        !svg.contains("background-color"),
        "the root plate is dropped"
    );

    // A default-styled decision diamond takes the accent wash and the accent line.
    let flow = render(FAMILIES[0].1, &palette).unwrap();
    let diamond = flow
        .match_indices("<polygon")
        .map(|(ix, _)| &flow[ix..ix + flow[ix..].find('>').unwrap()])
        .find(|tag| tag.contains("label-container"))
        .expect("the decision diamond");
    assert!(diamond.contains(&palette.accent_wash.to_hex()), "{diamond}");
    assert!(diamond.contains(&palette.accent_line.to_hex()), "{diamond}");

    // Author styling survives: an explicit fill on the node is not repainted.
    let styled = render(
        "flowchart TD\n  A{Decision}\n  style A fill:#ff0000,stroke:#00ff00\n",
        &palette,
    )
    .unwrap();
    assert!(styled.contains("#ff0000"));
    assert!(!styled.contains(&palette.accent_wash.to_hex()) || !styled.contains("<polygon"));
}

#[test]
fn the_palette_follows_the_specified_derivation() {
    let theme = DocumentTheme::dark();
    let palette = Palette::from_theme(&theme);
    let bg = Color::from_rgba(theme.bg);
    let plate = Color::WHITE.with_alpha(0.035).blend_over(bg);
    assert_eq!(palette.plate, plate);
    assert_eq!(palette.canvas, plate.blend_over(bg));
    assert_eq!(
        palette.node,
        Color::WHITE.with_alpha(0.06).blend_over(plate)
    );
    assert_eq!(
        palette.group,
        Color::WHITE.with_alpha(0.03).blend_over(palette.canvas)
    );
    assert_eq!(
        palette.accent_line,
        Color::from_rgba(theme.accent)
            .with_alpha(0.6)
            .blend_over(palette.canvas)
    );
    assert_eq!(
        palette.accent_wash,
        Color::from_rgba(theme.accent)
            .with_alpha(0.12)
            .blend_over(palette.node)
    );
    assert_eq!(palette.font_family, theme.font_sans.as_ref());
    assert_eq!(
        (
            palette.font_size,
            palette.node_spacing,
            palette.rank_spacing
        ),
        (14.0, 36.0, 40.0)
    );
    assert_eq!((palette.padding_x, palette.padding_y), (18.0, 10.0));
    // Light themes use the background token as the node color and black ink.
    let light_theme = DocumentTheme::light();
    let light = Palette::from_theme(&light_theme);
    assert!(!light.dark);
    assert_eq!(light.node, Color::from_rgba(light_theme.bg));
    assert_eq!(
        light.plate,
        Color::BLACK
            .with_alpha(0.035)
            .blend_over(Color::from_rgba(light_theme.bg))
    );
    assert_ne!(palette.style_id(), light.style_id());
    // The renderer's base theme follows the appearance.
    let config = palette.mermaid_config(&vespertine_components::diagram::Family::Flowchart);
    assert_eq!(config["themeVariables"]["darkMode"], true);
    assert_eq!(
        config["themeVariables"]["noteBkgColor"],
        palette.accent_wash.to_hex()
    );
    assert_eq!(
        config["themeVariables"]["noteBorderColor"],
        palette.accent_line.to_hex()
    );
    assert_eq!(
        config["themeVariables"]["activationBkgColor"],
        palette.accent_wash.to_hex()
    );
    let gantt = palette.mermaid_config(&vespertine_components::diagram::Family::Gantt);
    assert_eq!(
        gantt["themeVariables"]["primaryBorderColor"],
        palette.accent_line.to_hex()
    );
    assert_eq!(
        config["themeVariables"]["primaryBorderColor"],
        palette.border.to_hex()
    );
}

// ---- A3 -------------------------------------------------------------------------------------

struct Fake<F: Fn(&str) -> Result<String, String> + Send + Sync>(F, AtomicUsize);

impl<F: Fn(&str) -> Result<String, String> + Send + Sync> DiagramRenderer for Fake<F> {
    fn render(&self, source: &str, _: &Palette) -> Result<String, String> {
        self.1.fetch_add(1, Ordering::SeqCst);
        (self.0)(source)
    }
}

#[test]
fn input_limits_accept_the_boundary_and_reject_one_more() {
    assert!(check_limits(&"x".repeat(16384)).is_ok());
    assert_eq!(
        check_limits(&"x".repeat(16385)).unwrap_err(),
        COMPLEXITY_LIMIT
    );
    assert!(check_limits(&"a\n".repeat(256)).is_ok());
    assert_eq!(
        check_limits(&"a\n".repeat(257)).unwrap_err(),
        COMPLEXITY_LIMIT
    );
    assert_eq!(count_tokens(&"a ".repeat(2048)), 2048);
    assert!(check_limits(&"a ".repeat(2048)).is_ok());
    assert_eq!(
        check_limits(&"a ".repeat(2049)).unwrap_err(),
        COMPLEXITY_LIMIT
    );
    // The characters ; > { } split tokens too, and are not tokens themselves.
    assert_eq!(count_tokens(&"a;>{}".repeat(2048)), 2048);
    assert!(check_limits(&"a;>{}".repeat(2048)).is_ok());
    assert_eq!(
        check_limits(&"a;>{}".repeat(2049)).unwrap_err(),
        COMPLEXITY_LIMIT
    );
    assert_eq!(
        check_limits(&"a;".repeat(2049)).unwrap_err(),
        COMPLEXITY_LIMIT
    );
    // The check happens before any rendering.
    let calls = Fake(|_| Ok(svg_of(10, 10)), AtomicUsize::new(0));
    assert!(render_with(&calls, &"x".repeat(16385), &dark()).is_err());
    assert_eq!(calls.1.load(Ordering::SeqCst), 0);
    assert!(render_with(&calls, "x", &dark()).is_ok());
    assert_eq!(calls.1.load(Ordering::SeqCst), 1);
}

#[test]
fn a_generated_svg_over_two_mebibytes_is_rejected() {
    let at_limit = Fake(
        |_| {
            Ok(format!(
                "<svg>{}</svg>",
                " ".repeat(2 * 1024 * 1024 - "<svg></svg>".len())
            ))
        },
        AtomicUsize::new(0),
    );
    assert!(render_with(&at_limit, "x", &dark()).is_ok());
    let over = Fake(
        |_| Ok(format!("<svg>{}</svg>", " ".repeat(2 * 1024 * 1024))),
        AtomicUsize::new(0),
    );
    assert_eq!(render_with(&over, "x", &dark()).unwrap_err(), OUTPUT_LIMIT);
}

#[test]
fn a_panicking_renderer_is_a_recoverable_error_and_the_next_render_works() {
    let renderer = Fake(
        |source| {
            if source == "panic" {
                panic!("renderer bug");
            }
            if source == "error" {
                return Err("Parse error on line 2".to_string());
            }
            Ok(svg_of(10, 10))
        },
        AtomicUsize::new(0),
    );
    assert_eq!(
        render_with(&renderer, "panic", &dark()).unwrap_err(),
        RENDER_PANIC
    );
    assert_eq!(
        render_with(&renderer, "error", &dark()).unwrap_err(),
        "Parse error on line 2",
        "renderer error text is the failure reason"
    );
    assert!(
        render_with(&renderer, "fine", &dark()).is_ok(),
        "the lock is not poisoned"
    );
}

// ---- A4 -------------------------------------------------------------------------------------

#[test]
fn pending_sources_not_requested_in_the_latest_two_passes_are_forgotten() {
    let mut cache = DiagramCache::new();
    cache.begin_frame(1);
    cache.request("old", "row-a-mermaid-0");
    cache.begin_frame(1);
    // Requested one pass ago: still kept.
    assert!(cache.next_job().is_some_and(|job| job.source == "old"));
    let mut cache = DiagramCache::new();
    cache.begin_frame(1);
    cache.request("old", "row-a-mermaid-0");
    cache.request("kept", "row-b-mermaid-0");
    cache.begin_frame(1);
    cache.request("kept", "row-b-mermaid-0");
    cache.begin_frame(1);
    // "old" was last requested two passes ago and is forgotten; "kept" one pass ago.
    let job = cache.next_job().expect("a job");
    assert_eq!(job.source, "kept");
    assert_eq!(cache.status("old"), None, "forgotten entries are gone");
    assert!(cache.next_job().is_none());
    // A forgotten source is requested again when its row returns.
    assert!(matches!(
        cache.request("old", "row-a-mermaid-0"),
        Lookup::Pending
    ));
    assert_eq!(
        cache.take_new_requests(),
        vec!["old".to_string(), "kept".to_string(), "old".to_string()]
    );
}

#[test]
fn the_most_recently_requested_pending_source_renders_first_and_one_at_a_time() {
    let mut cache = DiagramCache::new();
    cache.begin_frame(7);
    for name in ["a", "b", "c"] {
        cache.request(name, &format!("row-{name}-mermaid-0"));
    }
    cache.request("a", "row-a-mermaid-0");
    assert_eq!(
        cache.next_job().unwrap().source,
        "a",
        "a was requested last"
    );
    assert_eq!(cache.next_job().unwrap().source, "c");
    assert_eq!(cache.next_job().unwrap().source, "b");
    assert!(cache.next_job().is_none(), "nothing is chosen twice");
}

#[test]
fn a_theme_change_drains_the_cache_and_a_late_result_of_the_old_style_returns_nothing() {
    let mut cache = DiagramCache::new();
    cache.begin_frame(1);
    cache.request("ready", "row-1-mermaid-0");
    cache.request("slow", "row-2-mermaid-0");
    let job = cache.next_job().unwrap();
    assert_eq!(job.source, "slow");
    assert!(cache.finish("ready", 1, Ok(media(20, 20))).is_some());
    assert_eq!(cache.status("ready"), Some(Status::Ready));
    // The theme changes while "slow" renders: everything is discarded, the media released.
    let released = cache.begin_frame(2);
    assert_eq!(released.len(), 1);
    assert!(cache.is_empty());
    assert_eq!(cache.retained_bytes(), 0);
    // The late result carries the old identity: dropped silently, nothing stored or remeasured.
    assert!(
        cache
            .finish("slow", job.style_id, Ok(media(20, 20)))
            .is_none()
    );
    assert!(cache.is_empty());
    // Under the new identity the source renders again.
    assert!(matches!(
        cache.request("slow", "row-2-mermaid-0"),
        Lookup::Pending
    ));
    let again = cache.next_job().unwrap();
    assert_eq!(again.style_id, 2);
    let update = cache
        .finish("slow", 2, Ok(media(20, 20)))
        .expect("current style");
    assert_eq!(
        update.rows,
        vec!["row-2".to_string()],
        "the row is remeasured"
    );
    assert!(matches!(
        cache.request("slow", "row-2-mermaid-0"),
        Lookup::Ready(_)
    ));
    // Failures are results like any other and are cached.
    cache.request("bad", "row-3-mermaid-0");
    cache.next_job();
    assert!(cache.finish("bad", 2, Err("nope".into())).is_some());
    assert!(
        matches!(cache.request("bad", "row-3-mermaid-0"), Lookup::Failed(reason) if reason == "nope")
    );
}

// ---- A5 -------------------------------------------------------------------------------------

#[test]
fn more_than_sixty_four_settled_entries_evict_the_least_recently_painted_first() {
    let mut cache = DiagramCache::new();
    cache.begin_frame(1);
    let total = MAX_ENTRIES + 6;
    for ix in 0..total {
        cache.request(&format!("s{ix}"), &format!("row{ix}-mermaid-0"));
    }
    let mut released = 0;
    for ix in 0..total {
        let update = cache.finish(&format!("s{ix}"), 1, Ok(media(4, 4))).unwrap();
        released += update.released.len();
    }
    assert_eq!(
        released, 0,
        "everything was painted in the latest pass: nothing is evicted"
    );
    assert_eq!(cache.len(), total);
    // Pass 2 repaints the last ten and requests a new source that stays pending.
    cache.begin_frame(1);
    for ix in total - 10..total {
        cache.request(&format!("s{ix}"), &format!("row{ix}-mermaid-0"));
    }
    cache.request("pending", "rowp-mermaid-0");
    // Pass 3: the entries of pass 2 are still protected, pass 1 is not.
    cache.begin_frame(1);
    cache.request("fresh", "rowf-mermaid-0");
    let update = cache.finish("fresh", 1, Ok(media(4, 4))).unwrap();
    // 70 settled + pending + fresh = 72 entries; eight least recently painted settled entries go.
    assert_eq!(cache.len(), MAX_ENTRIES);
    assert_eq!(update.released.len(), total + 2 - MAX_ENTRIES);
    for ix in 0..total + 2 - MAX_ENTRIES {
        assert_eq!(
            cache.status(&format!("s{ix}")),
            None,
            "s{ix} was evicted first"
        );
    }
    for ix in total - 10..total {
        assert!(
            cache.status(&format!("s{ix}")).is_some(),
            "painted in a protected pass"
        );
    }
    assert_eq!(
        cache.status("pending"),
        Some(Status::Pending),
        "pending entries are never evicted"
    );
    assert_eq!(cache.status("fresh"), Some(Status::Ready));
}

#[test]
fn rasters_beyond_the_byte_bound_evict_unprotected_entries_and_return_their_media() {
    // 100 x 100 rasters are 40000 bytes each; the bound holds two.
    let mut cache = DiagramCache::with_limits(64, 100_000);
    cache.begin_frame(1);
    for name in ["a", "b", "c"] {
        cache.request(name, &format!("row-{name}-mermaid-0"));
    }
    for name in ["a", "b", "c"] {
        let update = cache.finish(name, 1, Ok(media(100, 100))).unwrap();
        assert!(
            update.released.is_empty(),
            "all painted in the latest passes"
        );
    }
    assert_eq!(
        cache.retained_bytes(),
        120_000,
        "the bound is soft while all are on screen"
    );
    cache.begin_frame(1);
    cache.begin_frame(1);
    cache.request("d", "row-d-mermaid-0");
    let update = cache.finish("d", 1, Ok(media(100, 100))).unwrap();
    assert_eq!(
        update.released.len(),
        2,
        "the two least recently painted go"
    );
    assert_eq!(cache.retained_bytes(), 80_000);
    assert!(cache.status("a").is_none() && cache.status("b").is_none());
    assert!(cache.status("c").is_some() && cache.status("d").is_some());
}

#[test]
fn a_view_change_redoes_every_raster_within_the_remaining_budget() {
    let mut cache = DiagramCache::with_limits(64, 100_000);
    cache.begin_frame(1);
    cache.request("a", "row-a-mermaid-0");
    cache.request("b", "row-b-mermaid-0");
    cache.finish("a", 1, Ok(media(100, 100))).unwrap();
    cache.finish("b", 1, Ok(media(100, 100))).unwrap();
    assert_eq!(cache.retained_bytes(), 80_000);
    // Doubling the scale would take 160000 bytes each: each raster is limited to the budget
    // that remains after the other entries.
    let update = cache.set_view(View {
        width: 1000.0,
        scale: 2.0,
    });
    assert_eq!(update.released.len(), 2, "superseded rasters are released");
    assert_eq!(update.rows.len(), 2, "both rows are remeasured");
    assert!(
        cache.retained_bytes() <= 100_000,
        "{}",
        cache.retained_bytes()
    );
    assert!(cache.retained_bytes() > 80_000, "redone at a larger size");
    // The same view again is a no-op.
    assert!(cache.set_view(cache.view()).released.is_empty());
    // A narrower column scales the diagram down.
    let narrow = media(400, 200)
        .raster_for_view(
            View {
                width: 100.0,
                scale: 1.0,
            },
            MAX_BYTES,
        )
        .unwrap();
    assert_eq!(narrow.raster_size(), (100, 50));
    assert_eq!(narrow.display_size(), (100.0, 50.0));
}

// ---- A6 -------------------------------------------------------------------------------------

#[test]
fn source_toggles_are_per_frame_identifier_and_pruned_with_their_rows() {
    let mut cache = DiagramCache::new();
    let (a, b, c) = ("row-1-mermaid-0", "row-1-mermaid-1", "row-2-mermaid-0");
    assert!(!cache.source_visible(a));
    assert!(cache.toggle_source(a));
    assert!(cache.source_visible(a));
    assert!(
        !cache.source_visible(b),
        "another block of the same row is independent"
    );
    assert!(!cache.source_visible(c));
    cache.toggle_source(b);
    cache.toggle_source(c);
    cache.retain_rows(|row| row == "row-1");
    assert!(cache.source_visible(a) && cache.source_visible(b));
    assert!(!cache.source_visible(c), "row-2 no longer exists");
    assert!(!cache.toggle_source(a), "toggling back shows the diagram");
    assert!(!cache.source_visible(a));
    // A theme change drains the diagrams but keeps the toggles.
    cache.begin_frame(1);
    cache.begin_frame(2);
    assert!(cache.source_visible(b));
}

// ---- A7 -------------------------------------------------------------------------------------

#[test]
fn an_svg_with_external_references_renders_without_any_resource_load() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let dir = tempfile::tempdir().unwrap();
    let png = dir.path().join("red.png");
    let mut pixmap = resvg::tiny_skia::Pixmap::new(40, 40).unwrap();
    pixmap.fill(resvg::tiny_skia::Color::from_rgba8(255, 0, 0, 255));
    pixmap.save_png(&png).unwrap();
    let file_url = format!("file://{}", png.display());
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="200" height="100" viewBox="0 0 200 100">
<style>@import url("http://127.0.0.1:{port}/style.css"); @font-face {{ font-family: "Remote"; src: url("http://127.0.0.1:{port}/font.ttf"); }} text {{ font-family: "Remote"; }}</style>
<image href="http://127.0.0.1:{port}/image.png" x="0" y="0" width="40" height="40"/>
<image xlink:href="{file_url}" x="50" y="0" width="40" height="40"/>
<image href="{path}" x="100" y="0" width="40" height="40"/>
<image href="red.png" x="150" y="0" width="40" height="40"/>
<use href="http://127.0.0.1:{port}/other.svg#shape" x="0" y="50"/>
<text x="10" y="90" font-size="14">remote font</text>
</svg>"##,
        path = png.display()
    );
    let media = DiagramMedia::from_svg(&svg, View::default(), MAX_BYTES).expect("renders");
    std::thread::sleep(Duration::from_millis(300));
    match listener.accept() {
        Err(error) if error.kind() == ErrorKind::WouldBlock => {}
        other => panic!("the renderer connected to the network: {other:?}"),
    }
    // No image was loaded from a file either: the places of the four images are transparent.
    let (w, _) = media.raster_size();
    let pixels = media.image().as_bytes(0).unwrap();
    for x in [20u32, 70, 120, 170] {
        let ix = (20 * w + x) as usize * 4;
        assert_eq!(pixels[ix + 3], 0, "no image at x={x}");
    }
}
