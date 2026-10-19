//! FID-CMP-031-1 and FID-CMP-031-2: classic scrollbar thumbs on a plain scroll container.

use crate::common::hex;
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, InteractiveElement, IntoElement, ParentElement, Point, ScrollHandle,
    StatefulInteractiveElement, Styled, Window, div, point, px,
};

struct ClassicThumbs {
    handle: ScrollHandle,
    content: (f32, f32),
    offset: (f32, f32),
}

impl ClassicThumbs {
    fn new(content: (f32, f32), offset: (f32, f32)) -> Self {
        Self {
            handle: ScrollHandle::new(),
            content,
            offset,
        }
    }
}

impl Scene for ClassicThumbs {
    fn size(&self) -> (f32, f32) {
        (360.0, 300.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        let offset: Point<_> = point(px(self.offset.0), px(self.offset.1));
        self.handle.set_offset(offset);
        div()
            .size_full()
            .relative()
            .bg(hex(0x101010))
            .child(
                div()
                    .id("viewport")
                    .absolute()
                    .left(px(20.0))
                    .top(px(20.0))
                    .w(px(300.0))
                    .h(px(240.0))
                    .bg(hex(0x202020))
                    .overflow_scroll()
                    .scrollbar_color(hex(0xa0a0a0))
                    .scrollbar_width(px(6.0))
                    .track_scroll(&self.handle)
                    .child(
                        div()
                            .flex_none()
                            .w(px(self.content.0))
                            .h(px(self.content.1)),
                    ),
            )
            .into_any_element()
    }
}

register_scene!("FID-CMP-031-1", || ClassicThumbs::new(
    (300.0, 960.0),
    (0.0, -360.0)
));
register_scene!("FID-CMP-031-2", || ClassicThumbs::new(
    (600.0, 960.0),
    (-150.0, -360.0)
));
