//! The extension point between the test host and a retained host tree.
//!
//! A retained host tree (APP-006) keeps elements with stable numeric identities, changed by
//! serialized batches, and turns itself into ephemeral elements for every frame. The host drives
//! the tree through [`RetainedHost`]: it applies batches, asks it to render into the offscreen
//! window and reads it back for inspection. Everything beyond that core (focus by identity,
//! scrolling, selection, syntax-cache counters, image content) is an *optional capability*: an
//! implementation provides it by overriding the method; a capability it does not provide is
//! reported to the caller as [`HostError::Unsupported`](crate::HostError::Unsupported) and is
//! never silently ignored.
//!
//! The paint-time contract: while building elements for a frame, an implementation reports the
//! painted border-box bounds of each identified element from its `on_painted` observer
//! (DEV-005, `PaintLog::record_bounds`), and generated text and highlight washes through
//! `PaintLog::record_text` / `PaintLog::record_wash`. Events its handlers produce go through
//! `HostEvents::emit`.

use crate::{
    HostError,
    error::HostResult,
    types::{ListAnchor, SyntaxCacheCounters, TreeNode},
};
use vespertine::{AnyElement, App, FocusHandle, Window};

fn unsupported<T>(capability: &str) -> HostResult<T> {
    Err(HostError::Unsupported(format!(
        "{capability} is not supported by this retained host tree"
    )))
}

/// A retained host tree that a [`TestHost`](crate::TestHost) can render and inspect.
pub trait RetainedHost: 'static {
    /// Applies one serialized batch atomically. On success returns the identities of every
    /// destroyed element, in destruction order; on failure returns the error message and leaves
    /// the tree unchanged.
    fn apply_batch(&mut self, batch: &str) -> Result<Vec<u64>, String>;

    /// Builds the ephemeral elements of one frame from the retained tree. Called from the root
    /// view's render; the tree reports painted geometry through the application's
    /// [`PaintLog`](crate::PaintLog) from the elements it builds.
    fn render(&mut self, window: &mut Window, cx: &mut App) -> AnyElement;

    /// The identity of the root element, if any.
    fn root(&self) -> Option<u64>;

    /// The number of retained elements.
    fn element_count(&self) -> usize;

    /// Serializes the retained tree from the root, without bounds (the host attaches painted
    /// bounds). `None` for an empty tree.
    fn snapshot(&self) -> Option<TreeNode>;

    /// The focus handle of element `id`, if it is focusable.
    fn focus_handle(&self, id: u64, cx: &App) -> Option<FocusHandle> {
        let _ = (id, cx);
        None
    }

    /// The identity of the focused element, if the focus is on a retained element.
    fn focused_element(&self, window: &Window, cx: &App) -> Option<u64> {
        let _ = (window, cx);
        None
    }

    /// Moves focus to the next (`forward`) or previous focusable element within the subtree of
    /// `id`. Returns whether focus moved.
    fn focus_within(
        &mut self,
        id: u64,
        forward: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> HostResult<bool> {
        let _ = (id, forward, window, cx);
        unsupported("subtree focus traversal")
    }

    /// The scroll offset of a scroll container, `None` for elements that do not scroll.
    fn scroll_offset(&self, id: u64, cx: &App) -> Option<(f64, f64)> {
        let _ = (id, cx);
        None
    }

    /// Scrolls a scroll container to an absolute offset.
    fn scroll_to(
        &mut self,
        id: u64,
        x: f64,
        y: f64,
        window: &mut Window,
        cx: &mut App,
    ) -> HostResult<()> {
        let _ = (id, x, y, window, cx);
        unsupported("programmatic scrolling")
    }

    /// The logical scroll anchor of a virtual list, `None` for other elements.
    fn list_anchor(&self, id: u64, cx: &App) -> Option<ListAnchor> {
        let _ = (id, cx);
        None
    }

    /// Scrolls a virtual list to an item index with an optional pixel offset within the item.
    fn scroll_to_item(
        &mut self,
        id: u64,
        item_index: usize,
        offset: f64,
        window: &mut Window,
        cx: &mut App,
    ) -> HostResult<()> {
        let _ = (id, item_index, offset, window, cx);
        unsupported("scrolling to a list item")
    }

    /// Scrolls so that element `id` is visible within its nearest scrolling ancestor.
    fn scroll_into_view(&mut self, id: u64, window: &mut Window, cx: &mut App) -> HostResult<()> {
        let _ = (id, window, cx);
        unsupported("scrolling an element into view")
    }

    /// The currently selected document text, if any.
    fn selected_text(&self, cx: &App) -> Option<String> {
        let _ = cx;
        None
    }

    /// Clears the document selection.
    fn clear_selection(&mut self, window: &mut Window, cx: &mut App) -> HostResult<()> {
        let _ = (window, cx);
        unsupported("clearing the selection")
    }

    /// Counters of the shared syntax coloration cache.
    fn syntax_cache_counters(&self, cx: &App) -> Option<SyntaxCacheCounters> {
        let _ = cx;
        None
    }

    /// Sets an image element's content from encoded bytes.
    fn set_image_bytes(
        &mut self,
        id: u64,
        bytes: &[u8],
        window: &mut Window,
        cx: &mut App,
    ) -> HostResult<()> {
        let _ = (id, bytes, window, cx);
        unsupported("setting image content")
    }

    /// Replaces an image element's content with packed pixels (`bgra` by default, or `rgba`).
    fn set_image_pixels(
        &mut self,
        id: u64,
        width: f64,
        height: f64,
        bytes: &[u8],
        format: Option<&str>,
        window: &mut Window,
        cx: &mut App,
    ) -> HostResult<()> {
        let _ = (id, width, height, bytes, format, window, cx);
        unsupported("setting image pixels")
    }
}
