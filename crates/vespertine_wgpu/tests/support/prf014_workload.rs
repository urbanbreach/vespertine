//! Per-frame strings and sizes of the PERF-PRF-014 workload (size-specific shaping retention).
//!
//! Pure and dependency-free (std only) so that the scene in the scenes crate and the retention
//! tests generate exactly the same content: include it with
//! `#[path = "<relative path>/prf014_workload.rs"] mod prf014_workload;`.
//!
//! Layout of the scene itself (window 880 x 480 logical px at scale factor 2; 100 labels in 5
//! columns by 20 rows; label `i` in a 168 x 20 box with hidden overflow at
//! `(8 + 172 * (i % 5), 8 + 22 * (i / 5))`; Noto Sans 400, #1f2328, line height 18 px) is the
//! scene's concern; this module owns only what changes per frame.

/// The 64-word list of the PERF-TEXT corpus.
pub const WORDS: [&str; 64] = [
    "let", "fn", "struct", "impl", "match", "self", "return", "mut", "pub", "use", "mod", "enum",
    "trait", "where", "async", "await", "loop", "while", "for", "in", "if", "else", "const",
    "static", "type", "move", "ref", "box", "dyn", "unsafe", "crate", "super", "true", "false",
    "some", "none", "ok", "err", "vec", "string", "option", "result", "into", "from", "clone",
    "copy", "debug", "default", "hash", "order", "write", "read", "build", "render", "frame",
    "layout", "paint", "text", "glyph", "window", "event", "focus", "scroll", "theme",
];

/// Number of labels in the scene.
pub const LABEL_COUNT: u32 = 100;

/// Frames of configuration 1 (sizes 16, 24 and 16.0001 in blocks of ten frames): 0..720.
pub const CONFIG1_FRAMES: u32 = 720;

/// First frame of configuration 2 and frame at which its size ramp ends (exclusive); from
/// `CONFIG2_RAMP_END` on the size stays 23.
pub const CONFIG2_START: u32 = CONFIG1_FRAMES;
pub const CONFIG2_RAMP_END: u32 = 1820;

/// Text of label `label` (0..100) in frame `frame`:
/// `W[(label * 7 + frame) % 64]`, a space, `W[(label * 3 + frame * 5) % 64]`,
/// `" office affine "` and the decimal number `frame * 100 + label`.
pub fn label_text(frame: u32, label: u32) -> String {
    let (frame, label) = (u64::from(frame), u64::from(label));
    let first = WORDS[((label * 7 + frame) % 64) as usize];
    let second = WORDS[((label * 3 + frame * 5) % 64) as usize];
    format!("{first} {second} office affine {}", frame * 100 + label)
}

/// Logical font size of every label in frame `frame`.
///
/// Configuration 1 (frames below 720): block `b = frame / 10`; 16 when `b % 3 == 0`, 24 when
/// `b % 3 == 1`, 16.0001 when `b % 3 == 2`. Configuration 2 (frames from 720): `12 + 0.01 *
/// (frame - 720)` as a 32-bit float below frame 1820 (1,100 distinct sizes), 23 afterwards.
pub fn font_size(frame: u32) -> f32 {
    if frame < CONFIG2_START {
        match (frame / 10) % 3 {
            0 => 16.0,
            1 => 24.0,
            _ => 16.0001,
        }
    } else if frame < CONFIG2_RAMP_END {
        (12.0_f64 + 0.01_f64 * f64::from(frame - CONFIG2_START)) as f32
    } else {
        23.0
    }
}
