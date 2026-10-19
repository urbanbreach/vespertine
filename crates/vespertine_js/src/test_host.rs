//! The retained host tree for the offscreen test host (DEV-006): the same tree, scene and event
//! path as the live window, rendered into the test host's offscreen window.

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use serde_json::{Map, Value};
use vespertine::{AnyElement, App, FocusHandle, Window};
use vespertine_host_tree::{HostTree, NodeRef, TreeOptions};
use vespertine_test_host::{HostError, HostEvents, HostResult, NativeEvent, PaintLog, RetainedHost, TreeNode};

use crate::scene::{EmitEvent, HostScene};

/// A [`RetainedHost`] over a [`HostTree`] and a [`HostScene`].
pub struct HostTreeHost {
    tree: HostTree,
    scene: HostScene,
    generation: Arc<AtomicU64>,
}

fn validate_style(value: &Value) -> Result<(), String> {
    vespertine::HostStyle::from_json(value, "setStyle")
        .map(|_| ())
        .map_err(|error| error.message().to_string())
}

impl HostTreeHost {
    /// A host with an empty tree. `generation` is the identity window-level key events carry.
    pub fn new(generation: Arc<AtomicU64>) -> Self {
        let emit: EmitEvent = Rc::new(|cx, id, name, payload| {
            HostEvents::emit(
                cx,
                NativeEvent {
                    element_id: id,
                    event_type: name.to_string(),
                    payload,
                },
            );
        });
        let mut scene = HostScene::new(emit, generation.clone());
        scene.set_painted_hook(Some(Rc::new(|id, bounds, cx| {
            PaintLog::record_bounds(cx, id, bounds);
        })));
        Self {
            tree: HostTree::with_options(TreeOptions {
                style_validator: validate_style,
                ..TreeOptions::default()
            }),
            scene,
            generation,
        }
    }

    /// The identity window-level key events carry.
    pub fn generation(&self) -> &Arc<AtomicU64> {
        &self.generation
    }

    fn snapshot_node(&self, node: NodeRef<'_>) -> TreeNode {
        let style = match node.style().map(|style| &**style) {
            Some(Value::Object(map)) => map.clone(),
            _ => Map::new(),
        };
        TreeNode {
            id: node.id(),
            element_type: node.element_type().to_owned(),
            text: node.text().map(str::to_owned),
            test_id: node.test_id().map(str::to_owned),
            style,
            events: node.listeners().map(str::to_owned).collect(),
            custom_props: node
                .props()
                .map(|(key, value)| (key.to_owned(), value.clone()))
                .collect(),
            bounds: None,
            children: node
                .children()
                .filter_map(|child| self.tree.node(child))
                .map(|child| self.snapshot_node(child))
                .collect(),
        }
    }
}

impl RetainedHost for HostTreeHost {
    fn apply_batch(&mut self, batch: &str) -> Result<Vec<u64>, String> {
        self.tree
            .apply_batch(batch)
            .map(|result| result.destroyed)
            .map_err(|error| error.to_string())
    }

    fn render(&mut self, window: &mut Window, cx: &mut App) -> AnyElement {
        self.scene.render(&self.tree, window, cx)
    }

    fn root(&self) -> Option<u64> {
        self.tree.root()
    }

    fn element_count(&self) -> usize {
        self.tree.len()
    }

    fn snapshot(&self) -> Option<TreeNode> {
        let root = self.tree.node(self.tree.root()?)?;
        Some(self.snapshot_node(root))
    }

    fn focus_handle(&self, id: u64, _cx: &App) -> Option<FocusHandle> {
        self.scene.focus_handle(id)
    }

    fn focused_element(&self, window: &Window, cx: &App) -> Option<u64> {
        self.scene.focused(window, cx)
    }

    fn focus_within(
        &mut self,
        id: u64,
        forward: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> HostResult<bool> {
        let before = self.scene.focused(window, cx);
        self.scene
            .focus_traverse(&self.tree, Some(id), forward, window, cx);
        Ok(self.scene.focused(window, cx) != before)
    }

    fn scroll_offset(&self, id: u64, _cx: &App) -> Option<(f64, f64)> {
        self.scene
            .scroll_offset(id)
            .map(|(x, y)| (f64::from(x), f64::from(y)))
    }

    fn scroll_to(
        &mut self,
        id: u64,
        x: f64,
        y: f64,
        window: &mut Window,
        _cx: &mut App,
    ) -> HostResult<()> {
        if self.scene.scroll_to(id, x as f32, y as f32, window) {
            Ok(())
        } else {
            Err(HostError::Unsupported(format!("element {id} does not scroll")))
        }
    }

    fn scroll_into_view(&mut self, id: u64, window: &mut Window, _cx: &mut App) -> HostResult<()> {
        if self.scene.scroll_into_view(&self.tree, id, window) {
            Ok(())
        } else {
            Err(HostError::Unsupported(format!(
                "element {id} has no scrolling ancestor"
            )))
        }
    }

    fn set_image_bytes(
        &mut self,
        id: u64,
        bytes: &[u8],
        window: &mut Window,
        cx: &mut App,
    ) -> HostResult<()> {
        self.scene
            .set_image(id, bytes, window, cx)
            .map_err(HostError::Unsupported)
    }
}

