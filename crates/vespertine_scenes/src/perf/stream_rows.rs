//! PERF-STREAM shapes its growing live message incrementally (shaping of appended text). Tests
//! only; the scene itself is `perf/stream.rs`.

#[cfg(test)]
mod tests {
    use crate::perf::stream::StreamScene;
    use crate::scene::Scene;
    use std::sync::Arc;
    use vespertine::{
        AnyWindowHandle, AppContext as _, Context, IntoElement, Render, TestAppContext,
        TestDispatcher, Window,
    };

    struct Harness {
        scene: StreamScene,
        frame: u64,
    }

    impl Render for Harness {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let element = self.scene.frame(self.frame, window, cx);
            self.frame += 1;
            window.request_animation_frame();
            element
        }
    }

    /// Frames of the scene's first live message and the splice that appends the next one.
    const FRAMES: usize = 250;

    #[vespertine::test]
    fn the_live_message_is_shaped_incrementally_every_frame(_cx: &mut TestAppContext) {
        let mut cx = TestAppContext::build_with_text_system(
            TestDispatcher::new(0),
            None,
            Arc::new(vespertine_wgpu::CosmicTextSystem::new("Noto Sans")),
        );
        let handle: AnyWindowHandle = cx
            .add_window(|_, _| Harness {
                scene: StreamScene::new(),
                frame: 0,
            })
            .into();
        for _ in 0..FRAMES {
            cx.update_window(handle, |_, window, cx| {
                window.simulate_next_frame(cx);
            })
            .unwrap();
            cx.update_window(handle, |_, window, cx| window.draw(cx).clear(cx))
                .unwrap();
        }
        // The validation of the incremental shaping never turns it off, and most frames shape
        // only the new words of the live message.
        let shaping = cx.update(|cx| cx.text_system().shape_stats());
        assert!(!shaping.append_disabled, "{shaping:?}");
        assert!(
            shaping.appended_lines as usize >= FRAMES / 2,
            "most frames shape only the new words: {shaping:?}"
        );
    }
}
