//! FID-TXT-001-1 (Latin) and FID-TXT-001-2 (CJK): punctuation-aware line wrapping (spec TXT-001).

use crate::common::{WHITE, hex};
use crate::scene::{Scene, register_scene};
use vespertine::{AnyElement, App, IntoElement, ParentElement, Styled, Window, div, px};

/// One absolutely positioned, fixed-width text box.
struct TextBox {
    x: f32,
    y: f32,
    width: f32,
    text: &'static str,
}

/// A window holding wrapped text boxes that share one font setup.
pub struct WrapScene {
    size: (f32, f32),
    family: &'static str,
    font_size: f32,
    line_height: f32,
    color: u32,
    boxes: &'static [TextBox],
}

const LATIN_BOXES: [TextBox; 5] = [
    TextBox { x: 20.0, y: 20.0, width: 76.0, text: "aaa aaaa.\"" },
    TextBox { x: 140.0, y: 20.0, width: 140.0, text: "aaa aaaa.\"" },
    TextBox { x: 20.0, y: 100.0, width: 76.0, text: "aaa aaaaa!" },
    TextBox { x: 140.0, y: 100.0, width: 76.0, text: "aaa (aaaa)" },
    TextBox { x: 20.0, y: 180.0, width: 76.0, text: "aaaa \"bbbb" },
];

const T1: &str = "今天的天气非常好，我们一起去公园散步吧。";
const T2: &str = "我用（新方法）重写了程序：速度提升了８％，超出预期……";
const T3: &str = "好吗？我们走吧。「新」方法真好！";

const CJK_BOXES: [TextBox; 5] = [
    TextBox { x: 20.0, y: 20.0, width: 170.0, text: T1 },
    TextBox { x: 210.0, y: 20.0, width: 70.0, text: T2 },
    TextBox { x: 300.0, y: 20.0, width: 130.0, text: T2 },
    TextBox { x: 450.0, y: 20.0, width: 110.0, text: T2 },
    TextBox { x: 580.0, y: 20.0, width: 110.0, text: T3 },
];

impl WrapScene {
    /// FID-TXT-001-1: Liberation Mono, 13.3333 px, line height 20 px, #000000, 360 x 260.
    pub fn latin() -> Self {
        Self {
            size: (360.0, 260.0),
            family: "Liberation Mono",
            font_size: 13.3333,
            line_height: 20.0,
            color: 0x000000,
            boxes: &LATIN_BOXES,
        }
    }

    /// FID-TXT-001-2: Noto Sans CJK SC, 20 px, line height 30 px, #202020, 700 x 350.
    pub fn cjk() -> Self {
        Self {
            size: (700.0, 350.0),
            family: "Noto Sans CJK SC",
            font_size: 20.0,
            line_height: 30.0,
            color: 0x202020,
            boxes: &CJK_BOXES,
        }
    }
}

impl Scene for WrapScene {
    fn size(&self) -> (f32, f32) {
        self.size
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        let mut root = div()
            .relative()
            .size_full()
            .bg(hex(WHITE))
            .font_family(self.family)
            .text_size(px(self.font_size))
            .line_height(px(self.line_height))
            .text_color(hex(self.color));
        for text_box in self.boxes {
            root = root.child(
                div()
                    .absolute()
                    .left(px(text_box.x))
                    .top(px(text_box.y))
                    .w(px(text_box.width))
                    .child(text_box.text),
            );
        }
        root.into_any_element()
    }
}

register_scene!("FID-TXT-001-1", WrapScene::latin);
register_scene!("FID-TXT-001-2", WrapScene::cjk);
