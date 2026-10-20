//! PERF-IDLE: idle window showing the PERF-STREAM content at frame 0 without any update.

use super::stream::StreamScene;
use crate::scene::{Scene, register_scene};
use vespertine::{AnyElement, App, Window};

pub struct IdleScene {
    content: StreamScene,
}

impl IdleScene {
    pub fn new() -> Self {
        Self {
            content: StreamScene::new(),
        }
    }
}

impl Scene for IdleScene {
    fn size(&self) -> (f32, f32) {
        StreamScene::size()
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        self.content.element()
    }
}

register_scene!("PERF-IDLE", IdleScene::new);
