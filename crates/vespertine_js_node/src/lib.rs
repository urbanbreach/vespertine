//! Node-API addon exposing the Vespertine JavaScript host (APP-005) to Bun and Node.
//!
//! Every method is a thin conversion layer over [`vespertine_js::HostRenderer`]: JavaScript
//! values are decoded with serde, errors become JavaScript exceptions (or promise
//! rejections) carrying the core's messages, and UI-thread events are delivered to the
//! constructor callback through a thread-safe function so the UI thread never blocks.

pub mod updater;

use std::sync::Arc;

use napi::bindgen_prelude::*;
use napi::threadsafe_function::ThreadsafeFunctionCallMode;
use napi_derive::napi;
use serde_json::Value;
use vespertine_js::{HostError, HostRenderer, PromptConfig, WindowConfig};

fn to_napi(error: HostError) -> Error {
    Error::from_reason(error.to_string())
}

fn decode<T: serde::de::DeserializeOwned + Default>(what: &str, value: Option<Value>) -> Result<T> {
    match value {
        None | Some(Value::Null) => Ok(T::default()),
        Some(value) => serde_json::from_value(value)
            .map_err(|error| Error::from_reason(format!("invalid {what}: {error}"))),
    }
}

/// The renderer a JavaScript process constructs, initializes and drives.
#[napi]
pub struct Renderer {
    inner: Arc<HostRenderer>,
}

#[napi]
impl Renderer {
    /// `callback(error, event)` is called from the UI thread with an error or an event payload.
    #[napi(constructor)]
    pub fn new(callback: Option<Function<'_, Value, ()>>) -> Result<Self> {
        let sink: Option<vespertine_js::EventSink> = match callback {
            None => None,
            Some(callback) => {
                let tsfn = callback
                    .build_threadsafe_function::<Value>()
                    .callee_handled::<true>()
                    // Events never keep the process alive; the caller's frame loop does.
                    .weak::<true>()
                    .build()?;
                Some(Arc::new(move |event| {
                    let value = event.map_err(Error::from_reason);
                    tsfn.call(value, ThreadsafeFunctionCallMode::NonBlocking);
                }))
            }
        };
        Ok(Self {
            inner: Arc::new(HostRenderer::new(sink)),
        })
    }

    /// Opens the window; waits for the UI thread to report that it opened.
    #[napi]
    pub fn init(&self, options: Option<Value>) -> Result<()> {
        let config: WindowConfig = decode("window options", options)?;
        self.inner.init(&config).map_err(to_napi)
    }

    /// Whether the UI loop is still running; false after the last window closed.
    #[napi]
    pub fn tick(&self) -> Result<bool> {
        self.inner.tick().map_err(to_napi)
    }

    #[napi]
    pub fn is_initialized(&self) -> bool {
        self.inner.is_initialized()
    }

    /// Linux: the UI loop runs on its own thread, so `tick` is a liveness check.
    #[napi]
    pub fn requires_tick(&self) -> bool {
        true
    }

    /// Registers a frame-wake callback; reports whether the platform supports frame wakes
    /// (false on Linux, where callers fall back to timed pumping).
    #[napi]
    pub fn set_frame_wake_callback(&self, _callback: Option<Unknown<'_>>) -> bool {
        false
    }

    #[napi]
    pub fn get_window_size(&self) -> Result<Value> {
        let size = self.inner.window_size().map_err(to_napi)?;
        serde_json::to_value(size).map_err(|error| Error::from_reason(error.to_string()))
    }

    #[napi]
    pub fn get_window_insets(&self) -> Result<Value> {
        let insets = self.inner.window_insets().map_err(to_napi)?;
        serde_json::to_value(insets).map_err(|error| Error::from_reason(error.to_string()))
    }

    #[napi]
    pub fn activate_window(&self) -> Result<()> {
        self.inner.activate_window().map_err(to_napi)
    }

    #[napi]
    pub fn minimize_window(&self) -> Result<()> {
        self.inner.minimize_window().map_err(to_napi)
    }

    #[napi]
    pub fn zoom_window(&self) -> Result<()> {
        self.inner.zoom_window().map_err(to_napi)
    }

    #[napi]
    pub fn toggle_fullscreen(&self) -> Result<()> {
        self.inner.toggle_fullscreen().map_err(to_napi)
    }

    /// Closes the window; once the last window is closed `tick` reports false.
    #[napi]
    pub fn close_window(&self) -> Result<()> {
        self.inner.close_window().map_err(to_napi)
    }

    #[napi]
    pub fn set_window_title(&self, title: String) -> Result<()> {
        self.inner.set_window_title(title).map_err(to_napi)
    }

    /// Opens the platform path chooser without blocking JavaScript: a promise of an array of
    /// absolute paths, or null when the user cancelled.
    #[napi]
    pub fn prompt_for_paths<'env>(
        &self,
        env: &'env Env,
        options: Option<Value>,
    ) -> Result<PromiseRaw<'env, Option<Vec<String>>>> {
        let outcome = decode::<PromptConfig>("path prompt options", options)
            .and_then(|config| self.inner.prompt_for_paths(&config).map_err(to_napi));
        env.spawn_future(async move {
            match outcome {
                Ok(answer) => answer.wait().await.map_err(to_napi),
                Err(error) => Err(error),
            }
        })
    }

    /// Queues a repaint request: the window rebuilds its elements and paints a frame.
    #[napi]
    pub fn request_repaint(&self) -> Result<()> {
        self.inner.request_repaint().map_err(to_napi)
    }

    /// Frames the window view has built since the renderer was constructed (diagnostics).
    #[napi]
    pub fn get_render_count(&self) -> f64 {
        self.inner.render_count() as f64
    }

    /// Applies a serialized mutation batch; returns the destroyed identities.
    #[napi]
    pub fn apply_batch(&self, batch: String) -> Result<Vec<f64>> {
        let outcome = self.inner.apply_batch(&batch).map_err(to_napi)?;
        Ok(outcome.destroyed.into_iter().map(|id| id as f64).collect())
    }
}
