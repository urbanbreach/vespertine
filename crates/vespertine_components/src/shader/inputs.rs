//! The uniform inputs of a shader frame (CMP-026 R1, R3, R4): pointer table, time table and the
//! packed uniform block. Everything here is pure so the tables are tested without a GPU.

use bytemuck::{Pod, Zeroable};
use vespertine::{Pixels, Point, Size};

/// Smallest rendered side in device pixels.
pub const MIN_RENDER_SIDE: u32 = 1;
/// Largest rendered side in device pixels.
pub const MAX_RENDER_SIDE: u32 = 8192;

/// The rendered pixel size: `round(logical size x scale)` per axis, clamped to 1 to 8192.
pub fn render_size(logical: Size<Pixels>, scale: f32) -> (u32, u32) {
    let axis = |v: Pixels| {
        let pixels = (f32::from(v) * scale).round();
        if pixels.is_finite() {
            (pixels as i64).clamp(MIN_RENDER_SIDE as i64, MAX_RENDER_SIDE as i64) as u32
        } else {
            MIN_RENDER_SIDE
        }
    };
    (axis(logical.width), axis(logical.height))
}

/// The mouse value (R3). Coordinates are device pixels from the bottom-left:
/// `x = dx x scale`, `y = device height - dy x scale`. The value is
/// `(pointer x, pointer y, press x, press y)` while pressed and hovered,
/// `(pointer x, pointer y, -1, -1)` while hovered and not pressed and `(0, 0, -1, -1)` otherwise.
pub fn mouse_table(
    pointer: Option<Point<Pixels>>,
    press: Option<Point<Pixels>>,
    device_height: f32,
    scale: f32,
) -> [f32; 4] {
    let device = |p: Point<Pixels>| {
        [
            f32::from(p.x) * scale,
            device_height - f32::from(p.y) * scale,
        ]
    };
    match (pointer, press) {
        (Some(pointer), Some(press)) => {
            let [px, py] = device(pointer);
            let [qx, qy] = device(press);
            [px, py, qx, qy]
        }
        (Some(pointer), None) => {
            let [px, py] = device(pointer);
            [px, py, -1.0, -1.0]
        }
        (None, _) => [0.0, 0.0, -1.0, -1.0],
    }
}

/// The time inputs of one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimeInputs {
    /// Seconds.
    pub time: f32,
    /// Seconds since the previous frame started (0 for the first frame).
    pub delta: f32,
    /// Frames started before this one (0 for the first).
    pub frame: u32,
}

/// Time bookkeeping of one surface (R4): the delta and the counter advance when a frame is
/// started, not when it lands; a non-animated surface keeps the time of its first frame.
#[derive(Clone, Debug, Default)]
pub struct TimeState {
    first_time: Option<f32>,
    previous_start: Option<f32>,
    started: u32,
}

impl TimeState {
    /// Starts a frame at `now` seconds since the host view was created.
    pub fn begin_frame(&mut self, now: f32, animated: bool) -> TimeInputs {
        let time = if animated {
            now
        } else {
            *self.first_time.get_or_insert(now)
        };
        let delta = self
            .previous_start
            .map_or(0.0, |previous| (time - previous).max(0.0));
        let frame = self.started;
        self.previous_start = Some(time);
        self.started += 1;
        TimeInputs { time, delta, frame }
    }

    /// Frames started so far.
    pub fn frames_started(&self) -> u32 {
        self.started
    }
}

/// The uniform block the programs read (std140, 208 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct UniformBlock {
    /// Width, height, pixel ratio, unused.
    pub resolution: [f32; 4],
    /// Time, time delta, unused, unused.
    pub time: [f32; 4],
    /// Frame counter, unused.
    pub frame: [i32; 4],
    /// The mouse value.
    pub mouse: [f32; 4],
    /// Yaw (spin included), pitch, zoom, unused.
    pub camera: [f32; 4],
    /// Width, height and 1 of each of the four channels.
    pub channel_resolution: [[f32; 4]; 4],
    /// Up to 16 named inputs, four per slot in declaration order.
    pub inputs: [[f32; 4]; 4],
}

/// The uniform/input descriptor of a frame.
#[derive(Clone, Debug, PartialEq)]
pub struct UniformInputs {
    /// Width and height in device pixels.
    pub size: (u32, u32),
    /// Pixel ratio.
    pub ratio: f32,
    /// Time inputs.
    pub time: TimeInputs,
    /// The mouse value.
    pub mouse: [f32; 4],
    /// Yaw including the turntable, pitch, zoom.
    pub camera: [f32; 3],
    /// Values of the named inputs in declaration order (at most 16).
    pub inputs: Vec<f32>,
    /// Width and height of each feedback channel; `(1, 1)` for a channel without a buffer.
    pub channel_sizes: [(f32, f32); 4],
}

impl UniformInputs {
    /// Packs the descriptor into the uniform block: the named inputs four per slot in
    /// declaration order, the channel sizes with a third component of 1.
    pub fn to_block(&self) -> UniformBlock {
        let mut inputs = [[0.0; 4]; 4];
        for (index, value) in self.inputs.iter().take(16).enumerate() {
            inputs[index / 4][index % 4] = *value;
        }
        let mut channel_resolution = [[0.0; 4]; 4];
        for (slot, (w, h)) in self.channel_sizes.iter().enumerate() {
            channel_resolution[slot] = [*w, *h, 1.0, 0.0];
        }
        UniformBlock {
            resolution: [self.size.0 as f32, self.size.1 as f32, self.ratio, 0.0],
            time: [self.time.time, self.time.delta, 0.0, 0.0],
            frame: [self.time.frame as i32, 0, 0, 0],
            mouse: self.mouse,
            camera: [self.camera[0], self.camera[1], self.camera[2], 0.0],
            channel_resolution,
            inputs,
        }
    }
}
