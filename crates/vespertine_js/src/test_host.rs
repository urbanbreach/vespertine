//! The retained host tree for the offscreen test host (DEV-006): the same tree, scene and event
//! path as the live window, rendered into the test host's offscreen window.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use serde_json::{Map, Value};
use vespertine::{AnyElement, App, FocusHandle, Window};
use vespertine_host_tree::{HostTree, NodeRef, TreeOptions};
use vespertine_test_host::{
    HostError, HostEvents, HostResult, ListAnchor, NativeEvent, PaintLog, PaintedWash, Rect,
    RetainedHost, TreeNode,
};

use crate::scene::{EmitEvent, HostScene};

/// A [`RetainedHost`] over a [`HostTree`] and a [`HostScene`].
pub struct HostTreeHost {
    tree: Rc<RefCell<HostTree>>,
    scene: Rc<RefCell<HostScene>>,
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
        let tree = Rc::new(RefCell::new(HostTree::with_options(TreeOptions {
            style_validator: validate_style,
            ..TreeOptions::default()
        })));
        let scene = HostScene::new(emit, generation.clone(), Rc::new(tree.clone()));
        scene
            .borrow_mut()
            .set_painted_hook(Some(Rc::new(|id, bounds, cx| {
                PaintLog::record_bounds(cx, id, bounds);
            })));
        scene
            .borrow_mut()
            .set_washes_hook(Some(Rc::new(|washes, cx| {
                for wash in washes {
                    PaintLog::record_wash(
                        cx,
                        PaintedWash {
                            element_id: wash.element_id,
                            // Every text node paints one run, with sub-index 0.
                            run_index: 0,
                            text: wash.text.clone(),
                            start_utf16: wash.start_utf16,
                            end_utf16: wash.end_utf16,
                            active: wash.active,
                            rects: wash
                                .rects
                                .iter()
                                .map(|[x, y, width, height]| Rect {
                                    x: f64::from(*x),
                                    y: f64::from(*y),
                                    width: f64::from(*width),
                                    height: f64::from(*height),
                                })
                                .collect(),
                        },
                    );
                }
            })));
        Self {
            tree,
            scene,
            generation,
        }
    }

    /// The flag that enables `windowSelectionChange` events; shared with the scene.
    pub fn selection_notifications(&self) -> Rc<std::cell::Cell<bool>> {
        self.scene.borrow().selection_notifications()
    }

    /// The identity window-level key events carry.
    pub fn generation(&self) -> &Arc<AtomicU64> {
        &self.generation
    }
}

/// The serialized retained tree from the root, without painted bounds; `None` for an empty tree.
/// Shared by the offscreen test host and the live automation host so both report the same
/// shape (DEV-007).
pub(crate) fn snapshot_tree(tree: &HostTree) -> Option<TreeNode> {
    let root = tree.node(tree.root()?)?;
    Some(snapshot_node(tree, root))
}

fn snapshot_node(tree: &HostTree, node: NodeRef<'_>) -> TreeNode {
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
            .filter_map(|child| tree.node(child))
            .map(|child| snapshot_node(tree, child))
            .collect(),
    }
}

impl RetainedHost for HostTreeHost {
    fn apply_batch(&mut self, batch: &str) -> Result<Vec<u64>, String> {
        self.tree
            .borrow_mut()
            .apply_batch(batch)
            .map(|result| result.destroyed)
            .map_err(|error| error.to_string())
    }

    fn render(&mut self, window: &mut Window, cx: &mut App) -> AnyElement {
        self.scene
            .borrow_mut()
            .render(&self.tree.borrow(), window, cx)
    }

    fn root(&self) -> Option<u64> {
        self.tree.borrow().root()
    }

    fn element_count(&self) -> usize {
        self.tree.borrow().len()
    }

    fn snapshot(&self) -> Option<TreeNode> {
        snapshot_tree(&self.tree.borrow())
    }

    fn focus_handle(&self, id: u64, _cx: &App) -> Option<FocusHandle> {
        self.scene.borrow().focus_handle(id)
    }

    fn focused_element(&self, window: &Window, cx: &App) -> Option<u64> {
        self.scene.borrow().focused(window, cx)
    }

    fn focus_within(
        &mut self,
        id: u64,
        forward: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> HostResult<bool> {
        let before = self.scene.borrow().focused(window, cx);
        self.scene
            .borrow()
            .focus_traverse(&self.tree.borrow(), Some(id), forward, window, cx);
        Ok(self.scene.borrow().focused(window, cx) != before)
    }

    fn scroll_offset(&self, id: u64, _cx: &App) -> Option<(f64, f64)> {
        self.scene
            .borrow()
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
        if self
            .scene
            .borrow()
            .scroll_to(id, x as f32, y as f32, window)
        {
            Ok(())
        } else {
            Err(HostError::Unsupported(format!(
                "element {id} does not scroll"
            )))
        }
    }

    fn scroll_into_view(&mut self, id: u64, window: &mut Window, _cx: &mut App) -> HostResult<()> {
        if self
            .scene
            .borrow()
            .scroll_into_view(&self.tree.borrow(), id, window)
        {
            Ok(())
        } else {
            Err(HostError::Unsupported(format!(
                "element {id} has no scrolling ancestor"
            )))
        }
    }

    fn list_anchor(&self, id: u64, _cx: &App) -> Option<ListAnchor> {
        self.scene
            .borrow()
            .list_anchor(id)
            .map(|(item_index, offset, viewport_height)| ListAnchor {
                item_index,
                offset: f64::from(offset),
                viewport_height: f64::from(viewport_height),
            })
    }

    fn scroll_to_item(
        &mut self,
        id: u64,
        item_index: usize,
        offset: f64,
        window: &mut Window,
        _cx: &mut App,
    ) -> HostResult<()> {
        if self
            .scene
            .borrow()
            .scroll_to_item(id, item_index, offset as f32, window)
        {
            Ok(())
        } else {
            Err(HostError::Unsupported(format!(
                "element {id} is not a virtual list"
            )))
        }
    }

    fn selected_text(&self, _cx: &App) -> Option<String> {
        self.scene.borrow().selected_text()
    }

    fn clear_selection(&mut self, window: &mut Window, cx: &mut App) -> HostResult<()> {
        self.scene.borrow().clear_selection(window, cx);
        Ok(())
    }

    fn set_image_bytes(
        &mut self,
        id: u64,
        bytes: &[u8],
        window: &mut Window,
        cx: &mut App,
    ) -> HostResult<()> {
        self.scene
            .borrow_mut()
            .set_image(id, bytes, window, cx)
            .map_err(HostError::Unsupported)
    }

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
        self.scene
            .borrow_mut()
            .set_image_pixels(id, width, height, bytes, format, window, cx)
            .map_err(HostError::Unsupported)
    }
}
