//! Native JavaScript host for Vespertine (APP-005, APP-006 native boundary).
//!
//! A JavaScript process owns a declarative UI while rendering, input and window management
//! run on a dedicated UI thread. This crate is the Node-API-independent core: the renderer
//! object ([`HostRenderer`]), the queued command channel with bounded response waits
//! ([`UiLink`]), window options ([`WindowConfig`]) and path prompts ([`PromptConfig`]). The
//! Node-API binding lives in `vespertine_js_node`.

mod error;
mod link;
mod options;
mod prompt;
mod runtime;
mod scene;
#[cfg(feature = "test-host")]
mod test_host;
mod view;

pub use error::HostError;
pub use link::{RESPONSE_TIMEOUT, Reply, UiLink};
pub use options::{
    DEFAULT_HEIGHT, DEFAULT_WIDTH, LayerShellConfig, OptionsError, ResolvedWindow, WindowConfig,
    parse_edge,
};
pub use prompt::{PromptConfig, paths_to_strings};
pub use runtime::{
    BatchOutcome, EdgeInsets, EventSink, HostRenderer, HostWindow, PromptAnswer, UiTask,
    WindowInsets, WindowSize,
};
pub use scene::{EmitEvent, HostScene, PaintedHook};
#[cfg(feature = "test-host")]
pub use test_host::HostTreeHost;
pub use view::{HostView, SharedTree, sink_emitter};
