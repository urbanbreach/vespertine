//! Sample-driven audio activity visualizations (CMP-027).
//!
//! - [`envelope`]: the amplitude-envelope buffer: irregular loudness samples become one
//!   normalized amplitude per fixed 60 ms slot in a bounded history; no audio is retained.
//! - [`waveform`]: scrolling rounded bars in one canvas with waiting, live and processing modes.
//! - [`geometry`]: an animated-geometry component painting externally generated line and dot
//!   frames on a caller-controlled clock.
//! - [`rolling`]: a keyed label transition that rolls only the changed middle of a text value.

pub mod envelope;
pub mod geometry;
pub mod rolling;
pub mod waveform;

pub use envelope::{Bar, EnvelopeBuffer, format_clock, normalize};
pub use geometry::{
    AnimatedGeometry, Dot, Frame, FrameRequest, FrameSource, InkTheme, Line, host_ticked,
    sanitize_fps,
};
pub use rolling::{roll_text, rolling};
pub use waveform::{Mode, Phase, WaveformColors, waveform};
