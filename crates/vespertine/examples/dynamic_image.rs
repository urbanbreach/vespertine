//! PRF-016: a dynamic image that alternates opaque red and opaque blue.
//!
//! The scene is drawn when the window opens. Every later frame gives the image
//! new pixels with `Window::update_image`, which uploads them into the image's
//! existing atlas allocation, and shows them with `Window::present_cached_frame`,
//! which re-presents the recorded scene. The counters printed at the end show
//! zero additional view renders, layouts, prepaints and paints during the updates
//! while the output changed every frame.
//!
//! Resizing the window changes the drawable size, so a cached presentation falls
//! back to a normal draw; those redraws are counted separately.

#[path = "example_support/fonts.rs"]
mod example_support;

use std::{cell::Cell, rc::Rc, sync::Arc, time::Duration};

use anyhow::Result;
use image::{Frame, Rgba, RgbaImage};
use smallvec::smallvec;
use vespertine::{
    App, Bounds, Context, Corners, Element, ElementId, GlobalElementId, InspectorElementId,
    LayoutId, Pixels, RenderImage, Style, Window, WindowBounds, WindowOptions, div, prelude::*, px,
    size, white,
};
use vespertine_platform::application;

/// Image size in device pixels: 1280 x 720, drawn 1:1 on a scale factor 2 display.
const IMAGE_WIDTH: u32 = 1280;
const IMAGE_HEIGHT: u32 = 720;
/// Updates run at 60 per second for two seconds.
const UPDATE_INTERVAL: Duration = Duration::from_micros(16_667);
const UPDATES: usize = 120;

/// Atlas bytes are BGRA.
const RED: [u8; 4] = [0, 0, 255, 255];
const BLUE: [u8; 4] = [255, 0, 0, 255];

#[derive(Default)]
struct Counters {
    renders: Cell<usize>,
    layouts: Cell<usize>,
    prepaints: Cell<usize>,
    paints: Cell<usize>,
    cached_presentations: Cell<usize>,
    redraws: Cell<usize>,
    /// `[renders, layouts, prepaints, paints]` when the first update began.
    baseline: Cell<[usize; 4]>,
}

impl Counters {
    fn bump(counter: &Cell<usize>) {
        counter.set(counter.get() + 1);
    }

    fn element_tree_calls(&self) -> [usize; 4] {
        [
            self.renders.get(),
            self.layouts.get(),
            self.prepaints.get(),
            self.paints.get(),
        ]
    }

    fn report(&self) {
        let [renders, layouts, prepaints, paints] = self.element_tree_calls();
        println!(
            "view renders: {renders}, layouts: {layouts}, prepaints: {prepaints}, paints: {paints} (including window start-up)"
        );
        let [base_renders, base_layouts, base_prepaints, base_paints] = self.baseline.get();
        println!(
            "additional during the {UPDATES} updates: renders: {}, layouts: {}, prepaints: {}, paints: {}",
            renders - base_renders,
            layouts - base_layouts,
            prepaints - base_prepaints,
            paints - base_paints
        );
        println!(
            "cached presentations: {}, fallback redraws: {}",
            self.cached_presentations.get(),
            self.redraws.get()
        );
    }
}

fn solid_image(pixel: [u8; 4]) -> Frame {
    Frame::new(RgbaImage::from_pixel(
        IMAGE_WIDTH,
        IMAGE_HEIGHT,
        Rgba(pixel),
    ))
}

/// Paints the image 1:1 into its bounds and counts each element-tree phase.
struct ImageSurface {
    image: Arc<RenderImage>,
    counters: Rc<Counters>,
}

impl IntoElement for ImageSurface {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for ImageSurface {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        Counters::bump(&self.counters.layouts);
        let mut style = Style::default();
        style.size.width = px(640.).into();
        style.size.height = px(360.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _window: &mut Window,
        _cx: &mut App,
    ) {
        Counters::bump(&self.counters.prepaints);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        _cx: &mut App,
    ) {
        Counters::bump(&self.counters.paints);
        window
            .paint_image(
                bounds,
                bounds,
                Corners::default(),
                self.image.clone(),
                0,
                false,
            )
            .expect("the image has a frame 0");
    }
}

struct DynamicImage {
    image: Arc<RenderImage>,
    counters: Rc<Counters>,
}

impl DynamicImage {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let counters = Rc::new(Counters::default());
        cx.spawn_in(window, async move |this, cx| {
            // Let the window's start-up frames (configure, scale) settle first.
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            for update in 1..=UPDATES {
                cx.background_executor().timer(UPDATE_INTERVAL).await;
                let result = this.update_in(cx, |this, window, cx| this.update(update, window, cx));
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        eprintln!("image update failed: {error:#}");
                        break;
                    }
                    // The window was closed.
                    Err(_) => return,
                }
            }
            this.update(cx, |this, cx| {
                this.counters.report();
                cx.quit();
            })
            .ok();
        })
        .detach();

        Self {
            image: Arc::new(RenderImage::new(smallvec![solid_image(RED)])),
            counters,
        }
    }

    /// Update number `update`: red when even, blue when odd, keeping the identity.
    fn update(&mut self, update: usize, window: &mut Window, cx: &mut Context<Self>) -> Result<()> {
        if update == 1 {
            self.counters
                .baseline
                .set(self.counters.element_tree_calls());
        }
        let pixel = if update.is_multiple_of(2) { RED } else { BLUE };
        let image = Arc::new(
            self.image
                .with_updated_frames(smallvec![solid_image(pixel)]),
        );
        // A later normal render (for example after a resize) must see the new pixels.
        self.image = image.clone();

        if window.update_image(&image)? && window.present_cached_frame() {
            Counters::bump(&self.counters.cached_presentations);
        } else {
            // The geometry is not cached: redraw normally, which records it again.
            Counters::bump(&self.counters.redraws);
            cx.notify();
        }
        Ok(())
    }
}

impl Render for DynamicImage {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        Counters::bump(&self.counters.renders);
        div()
            .size_full()
            .bg(white())
            .pt(px(45.))
            .pl(px(80.))
            .child(ImageSurface {
                image: self.image.clone(),
                counters: self.counters.clone(),
            })
    }
}

fn main() {
    application().run(|cx: &mut App| {
        if !example_support::load_fonts(cx) {
            return;
        }
        let bounds = Bounds::centered(None, size(px(800.), px(450.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| DynamicImage::new(window, cx)),
        )
        .unwrap();
        cx.activate(true);
    });
}
