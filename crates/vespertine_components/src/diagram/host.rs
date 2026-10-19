//! The diagram host: drives the cache with a background executor and implements the diagram
//! handler of a document (CMP-013 R1, R3, R4, R8, R12).
//!
//! A render never blocks painting: until a result exists the block shows the ordinary source
//! fence, after a failure the failed view. Exactly one source renders at a time on a background
//! executor.

use super::adapter::{DiagramRenderer, MermanRenderer, Palette, render_with};
use super::cache::{DiagramCache, Lookup, Update};
use super::media::{DiagramMedia, View};
use super::{DiagramHandler, DiagramView};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use vespertine::{App, IntoElement, ParentElement, Styled, Window, div, img, px};
use vespertine_theme::document::DocumentTheme;

type UpdateCallback = Rc<dyn Fn(&[String], &mut App)>;

struct State {
    cache: DiagramCache,
    renderer: Arc<dyn DiagramRenderer>,
    palette: Option<Palette>,
    running: bool,
    on_update: Option<UpdateCallback>,
}

/// The diagram host of a document window. Clones share one cache.
#[derive(Clone)]
pub struct DiagramHost {
    state: Rc<RefCell<State>>,
}

impl Default for DiagramHost {
    fn default() -> Self {
        DiagramHost::new()
    }
}

impl DiagramHost {
    /// A host rendering with `merman`.
    pub fn new() -> DiagramHost {
        DiagramHost::with_renderer(Arc::new(MermanRenderer))
    }

    /// A host with another renderer (tests, alternative notations).
    pub fn with_renderer(renderer: Arc<dyn DiagramRenderer>) -> DiagramHost {
        DiagramHost {
            state: Rc::new(RefCell::new(State {
                cache: DiagramCache::new(),
                renderer,
                palette: None,
                running: false,
                on_update: None,
            })),
        }
    }

    /// Called with the row keys to remeasure whenever a render finished or the view changed;
    /// the windows are refreshed afterwards. Without a callback only the refresh happens.
    pub fn on_update(&self, callback: impl Fn(&[String], &mut App) + 'static) {
        self.state.borrow_mut().on_update = Some(Rc::new(callback));
    }

    /// Starts a paint pass: call it once before the rows of the document are built. The pass is
    /// stamped with the style identity of `theme`; when it differs from the previous pass every
    /// cached diagram is discarded and its media released, so a result of the previous theme is
    /// never shown. `view` is the target column width and display scale.
    pub fn begin_pass(&self, theme: &DocumentTheme, view: View, window: &mut Window, cx: &mut App) {
        let palette = Palette::from_theme(theme);
        let (released, update) = {
            let mut state = self.state.borrow_mut();
            let released = state.cache.begin_frame(palette.style_id());
            state.palette = Some(palette);
            (released, state.cache.set_view(view))
        };
        for media in released.into_iter().chain(update.released) {
            cx.drop_image(media.image().clone(), Some(window));
        }
        if !update.rows.is_empty() {
            self.notify(&update.rows, cx);
        }
    }

    /// Prunes the source toggles of rows that no longer exist.
    pub fn retain_rows(&self, is_live: impl Fn(&str) -> bool) {
        self.state.borrow_mut().cache.retain_rows(is_live);
    }

    /// Runs `f` on the cache (inspection and tests).
    pub fn with_cache<R>(&self, f: impl FnOnce(&mut DiagramCache) -> R) -> R {
        f(&mut self.state.borrow_mut().cache)
    }

    /// The diagram handler to put into the document options.
    pub fn handler(&self) -> DiagramHandler {
        let host = self.clone();
        Rc::new(
            move |source: &str,
                  frame_id: &str,
                  theme: &DocumentTheme,
                  window: &mut Window,
                  cx: &mut App| { host.view_for(source, frame_id, theme, window, cx) },
        )
    }

    fn view_for(
        &self,
        source: &str,
        frame_id: &str,
        theme: &DocumentTheme,
        window: &mut Window,
        cx: &mut App,
    ) -> DiagramView {
        if self.state.borrow().palette.is_none() {
            self.begin_pass(theme, View::default(), window, cx);
        }
        let lookup = self.state.borrow_mut().cache.request(source, frame_id);
        self.pump(cx);
        match lookup {
            Lookup::Pending => DiagramView::Source,
            Lookup::Failed(reason) => DiagramView::Failed(reason.into()),
            Lookup::Ready(media) => {
                let (width, height) = media.display_size();
                let body = div()
                    .w(px(width))
                    .h(px(height))
                    .child(img(media.image().clone()).w(px(width)).h(px(height)))
                    .into_any_element();
                let show_source = self.state.borrow().cache.source_visible(frame_id);
                let host = self.clone();
                let frame_id = frame_id.to_string();
                DiagramView::Diagram {
                    body,
                    show_source,
                    toggle: Rc::new(move |_window, cx| {
                        host.state.borrow_mut().cache.toggle_source(&frame_id);
                        cx.refresh_windows();
                    }),
                }
            }
        }
    }

    fn notify(&self, rows: &[String], cx: &mut App) {
        let callback = self.state.borrow().on_update.clone();
        if let Some(callback) = callback {
            callback(rows, cx);
        }
        cx.refresh_windows();
    }

    /// Starts the next render when none is running.
    fn pump(&self, cx: &mut App) {
        let (job, renderer, palette) = {
            let mut state = self.state.borrow_mut();
            state.cache.take_new_requests();
            if state.running {
                return;
            }
            let Some(palette) = state.palette.clone() else {
                return;
            };
            let Some(job) = state.cache.next_job() else {
                return;
            };
            state.running = true;
            (job, state.renderer.clone(), palette)
        };
        let host = self.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |cx| {
            let work = job.clone();
            let result = executor
                .spawn(async move {
                    let svg = render_with(&*renderer, &work.source, &palette)?;
                    DiagramMedia::from_svg(&svg, work.view, work.budget)
                })
                .await;
            cx.update(|cx| host.finish(&job.source, job.style_id, result, cx));
        })
        .detach();
    }

    fn finish(
        &self,
        source: &str,
        style_id: u64,
        result: Result<DiagramMedia, String>,
        cx: &mut App,
    ) {
        let update: Option<Update> = {
            let mut state = self.state.borrow_mut();
            state.running = false;
            state.cache.finish(source, style_id, result)
        };
        match update {
            Some(update) => {
                for media in &update.released {
                    cx.drop_image(media.image().clone(), None);
                }
                self.notify(&update.rows, cx);
            }
            // A stale or forgotten result: repaint so that rows that still want the diagram
            // request it again under the current style.
            None => cx.refresh_windows(),
        }
        self.pump(cx);
    }
}
