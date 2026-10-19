//! Inspection of the retained tree and of the frame that was actually painted.

use crate::{
    HostError, HostResult, TestHost,
    types::{ListAnchor, PaintedWash, Rect, SyntaxCacheCounters, TreeNode},
};
use serde_json::Value;

impl TestHost {
    /// The retained tree from the root, serialized with the painted bounds of the last frame
    /// attached to every node that was painted. `None` for an empty tree.
    pub fn serialize_tree(&mut self) -> HostResult<Option<TreeNode>> {
        let snapshot = self.on_tree(|tree, _window, _cx| tree.snapshot())?;
        Ok(snapshot.map(|mut root| {
            root.walk_mut(&mut |node| node.bounds = self.log.bounds_of(node.id));
            root
        }))
    }

    /// The retained root element, if any.
    pub fn root_element(&mut self) -> HostResult<Option<TreeNode>> {
        self.serialize_tree()
    }

    /// The number of retained elements.
    pub fn element_count(&mut self) -> HostResult<usize> {
        self.on_tree(|tree, _window, _cx| tree.element_count())
    }

    fn collect(&mut self, mut matches: impl FnMut(&TreeNode) -> bool) -> HostResult<Vec<TreeNode>> {
        let mut found = Vec::new();
        if let Some(root) = self.serialize_tree()? {
            root.walk(&mut |node| {
                if matches(node) {
                    found.push(node.clone());
                }
            });
        }
        Ok(found)
    }

    /// The retained element with identity `id`.
    pub fn element_by_id(&mut self, id: u64) -> HostResult<Option<TreeNode>> {
        Ok(self.collect(|node| node.id == id)?.into_iter().next())
    }

    /// Retained elements of exactly this type, in document order.
    pub fn elements_by_type(&mut self, element_type: &str) -> HostResult<Vec<TreeNode>> {
        self.collect(|node| node.element_type == element_type)
    }

    /// Retained elements whose own text contains `text` (case-sensitive; the empty string
    /// matches every element), in document order.
    pub fn elements_by_text(&mut self, text: &str) -> HostResult<Vec<TreeNode>> {
        self.collect(|node| node.text.as_deref().unwrap_or("").contains(text))
    }

    /// Retained elements with exactly this test identifier, in document order.
    pub fn elements_by_test_id(&mut self, test_id: &str) -> HostResult<Vec<TreeNode>> {
        self.collect(|node| node.test_id.as_deref() == Some(test_id))
    }

    /// The strings painted in the last frame, in paint order, including text painted by custom
    /// elements that has no retained text node.
    pub fn painted_text(&self) -> Vec<String> {
        self.log.painted_text()
    }

    /// The highlight washes painted in the last frame with their geometry.
    pub fn painted_washes(&self) -> Vec<PaintedWash> {
        self.log.painted_washes()
    }

    /// The border-box bounds of element `id` in the last painted frame; `None` when it was not
    /// painted (hidden, display-none, or only speculatively painted).
    pub fn element_bounds(&self, id: u64) -> Option<Rect> {
        self.log.bounds_of(id)
    }

    /// Identities of the elements whose last-painted bounds contain `(x, y)`, topmost first.
    pub fn elements_at(&self, x: f64, y: f64) -> Vec<u64> {
        self.log.elements_at(x, y)
    }

    /// The scroll offset of a scroll container, `None` for elements that do not scroll.
    pub fn scroll_offset(&mut self, id: u64) -> HostResult<Option<(f64, f64)>> {
        self.on_tree(|tree, _window, cx| tree.scroll_offset(id, cx))
    }

    /// The logical scroll anchor of a virtual list: item index (equal to the item count for the
    /// at-end sentinel), offset within the item in pixels and viewport height in pixels.
    pub fn list_anchor(&mut self, id: u64) -> HostResult<Option<ListAnchor>> {
        self.on_tree(|tree, _window, cx| tree.list_anchor(id, cx))
    }

    /// Scrolls a scroll container to the absolute offset `(x, y)`, then flushes.
    pub fn scroll_to(&mut self, id: u64, x: f64, y: f64) -> HostResult<()> {
        self.flush();
        self.on_tree(|tree, window, cx| tree.scroll_to(id, x, y, window, cx))??;
        self.invalidate();
        self.flush();
        Ok(())
    }

    /// Scrolls a virtual list to an item with an optional offset within the item, then flushes.
    pub fn scroll_to_item(&mut self, id: u64, item_index: usize, offset: f64) -> HostResult<()> {
        self.flush();
        self.on_tree(|tree, window, cx| tree.scroll_to_item(id, item_index, offset, window, cx))??;
        self.invalidate();
        self.flush();
        Ok(())
    }

    /// Scrolls so that element `id` is visible within its nearest scrolling ancestor.
    pub fn scroll_into_view(&mut self, id: u64) -> HostResult<()> {
        self.flush();
        self.on_tree(|tree, window, cx| tree.scroll_into_view(id, window, cx))??;
        self.invalidate();
        self.flush();
        Ok(())
    }

    /// The current selected document text.
    pub fn selected_text(&mut self) -> HostResult<Option<String>> {
        self.on_tree(|tree, _window, cx| tree.selected_text(cx))
    }

    /// Clears the document selection.
    pub fn clear_selection(&mut self) -> HostResult<()> {
        self.flush();
        self.on_tree(|tree, window, cx| tree.clear_selection(window, cx))??;
        self.invalidate();
        self.flush();
        Ok(())
    }

    /// Counters of the shared syntax coloration cache: hits, misses, cached documents.
    pub fn syntax_cache_counters(&mut self) -> HostResult<SyntaxCacheCounters> {
        self.on_tree(|tree, _window, cx| tree.syntax_cache_counters(cx))?
            .ok_or_else(|| {
                HostError::Unsupported(
                    "this retained host tree has no syntax coloration cache".to_owned(),
                )
            })
    }

    /// Sets an image element's content from encoded bytes, then flushes.
    pub fn set_image_bytes(&mut self, id: u64, bytes: &[u8]) -> HostResult<()> {
        self.on_tree(|tree, window, cx| tree.set_image_bytes(id, bytes, window, cx))??;
        self.flush();
        Ok(())
    }

    /// Replaces an image element's content with packed pixels (`bgra` by default, or `rgba`),
    /// then flushes.
    pub fn set_image_pixels(
        &mut self,
        id: u64,
        width: f64,
        height: f64,
        bytes: &[u8],
        format: Option<&str>,
    ) -> HostResult<()> {
        self.on_tree(|tree, window, cx| {
            tree.set_image_pixels(id, width, height, bytes, format, window, cx)
        })??;
        self.flush();
        Ok(())
    }

    /// The native accessibility tree of the last frame as structured data: the window's
    /// accessibility tree update serialized to JSON (nodes with role, label, value, bounds and
    /// children). Connects a simulated assistive technology on first use, which makes the
    /// window build its accessibility tree from then on. `None` when accessibility is disabled
    /// for the application.
    pub fn accessibility_tree(&mut self) -> HostResult<Option<Value>> {
        let window = self.window;
        let connected = self
            .cx
            .simulate_accessibility_activation(window)
            .map_err(|error| HostError::Window(format!("{error:#}")))?;
        if !connected {
            return Ok(None);
        }
        self.flush();
        self.invalidate();
        self.flush();
        let update = self
            .cx
            .last_accessibility_update(window)
            .map_err(|error| HostError::Window(format!("{error:#}")))?;
        update
            .map(|update| {
                serde_json::to_value(&update)
                    .map_err(|error| HostError::Window(format!("accessibility tree: {error}")))
            })
            .transpose()
    }
}
