//! The window's root view: draws the retained host tree through the native scene.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use vespertine::{Context, IntoElement, Render, Window};
use vespertine_host_tree::HostTree;

use crate::runtime::EventSink;
use crate::scene::{EmitEvent, HostScene};

/// The retained tree shared between the JavaScript thread (which mutates it) and the UI
/// thread (which draws it).
pub type SharedTree = Arc<Mutex<HostTree>>;

/// An [`EmitEvent`] that flattens each event into `{ type, id, ...payload }` and hands it to the
/// JavaScript sink.
pub fn sink_emitter(sink: Option<EventSink>) -> EmitEvent {
    std::rc::Rc::new(move |_, id, name, payload| {
        let Some(sink) = &sink else { return };
        let mut event = match payload {
            Value::Object(map) => map,
            _ => serde_json::Map::new(),
        };
        event.insert("type".into(), Value::String(name.to_string()));
        event.insert("id".into(), Value::from(id));
        sink(Ok(Value::Object(event)));
    })
}

pub struct HostView {
    tree: SharedTree,
    renders: Arc<AtomicU64>,
    scene: HostScene,
}

impl HostView {
    /// `renders` is incremented every time the view builds its elements for a frame;
    /// `generation` is the identity of window-level key events; `sink` receives native events.
    pub fn new(
        tree: SharedTree,
        renders: Arc<AtomicU64>,
        sink: Option<EventSink>,
        generation: Arc<AtomicU64>,
    ) -> Self {
        Self {
            tree,
            renders,
            scene: HostScene::new(sink_emitter(sink), generation),
        }
    }

    /// The scene (for UI-thread operations on nodes).
    pub fn scene(&mut self) -> &mut HostScene {
        &mut self.scene
    }

    /// The retained tree.
    pub fn tree(&self) -> &SharedTree {
        &self.tree
    }
}

impl Render for HostView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.fetch_add(1, Ordering::Relaxed);
        let tree = self
            .tree
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.scene.render(&tree, window, cx)
    }
}
