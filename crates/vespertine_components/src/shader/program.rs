//! The program convention and its validation (CMP-026 R1, R2).
//!
//! A surface program is a fragment program in the GLSL 4.50 dialect that defines
//! `void mainImage(out vec4 fragColor, in vec2 fragCoord)`. The host wraps the author's text with
//! a fixed header declaring the built-in inputs and a fixed trailer that calls the entry with the
//! color initialized to `(0, 0, 0, 1)` and writes the result. [`validate_program`] compiles the
//! assembled text with the same front end and validator the renderer uses, so authors learn about
//! an error before display; the first error is reported with its line number in the author's own
//! text (the header lines are subtracted).

use std::fmt;
use wgpu::naga;

/// Longest program source in bytes.
pub const MAX_SOURCE_BYTES: usize = 65_536;
/// Most named float inputs.
pub const MAX_INPUTS: usize = 16;
/// Most feedback buffer passes.
pub const MAX_BUFFERS: usize = 4;
/// Most image textures.
pub const MAX_TEXTURES: usize = 4;
/// The entry function every program defines.
pub const ENTRY_NAME: &str = "mainImage";
/// Prefixes reserved by the host (`vsp_` for the host declarations) and by the shading language
/// (`gl_`); an input name must not start with any of them.
pub const RESERVED_PREFIXES: [&str; 3] = ["vsp_", "_vsp", "gl_"];
/// The built-in identifiers (shader playground convention): resolution, time, time delta, frame,
/// mouse, camera, the four feedback channels with their sizes, the four image textures and the
/// noise texture.
pub const BUILTIN_NAMES: [&str; 18] = [
    "iResolution",
    "iTime",
    "iTimeDelta",
    "iFrame",
    "iMouse",
    "iCamera",
    "iChannel0",
    "iChannel1",
    "iChannel2",
    "iChannel3",
    "iChannelResolution",
    "iTexture0",
    "iTexture1",
    "iTexture2",
    "iTexture3",
    "iNoise",
    "main",
    ENTRY_NAME,
];

/// A validation or compilation failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramError {
    /// The 1-based line in the author's own text; `None` for errors outside it (the header, the
    /// trailer, limits).
    pub line: Option<u32>,
    /// The message.
    pub message: String,
}

impl ProgramError {
    fn new(line: Option<u32>, message: impl Into<String>) -> Self {
        Self {
            line,
            message: message.into(),
        }
    }
}

impl fmt::Display for ProgramError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "line {line}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for ProgramError {}

/// Checks one input name: an identifier (letter or underscore, then letters, digits and
/// underscores) that does not start with a reserved prefix and is no built-in identifier.
pub fn check_input_name(name: &str) -> Result<(), ProgramError> {
    let mut chars = name.chars();
    let valid = chars
        .next()
        .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric());
    if !valid {
        return Err(ProgramError::new(
            None,
            format!("input name `{name}` is not an identifier"),
        ));
    }
    if let Some(prefix) = RESERVED_PREFIXES.iter().find(|p| name.starts_with(**p)) {
        return Err(ProgramError::new(
            None,
            format!("input name `{name}` starts with the reserved prefix `{prefix}`"),
        ));
    }
    if BUILTIN_NAMES.contains(&name) {
        return Err(ProgramError::new(
            None,
            format!("input name `{name}` is a built-in identifier"),
        ));
    }
    Ok(())
}

/// Checks the limits on the names of the named inputs: at most 16, each valid and unique.
pub fn check_input_names(names: &[String]) -> Result<(), ProgramError> {
    if names.len() > MAX_INPUTS {
        return Err(ProgramError::new(
            None,
            format!("{} inputs exceed the limit of {MAX_INPUTS}", names.len()),
        ));
    }
    for (index, name) in names.iter().enumerate() {
        check_input_name(name)?;
        if names[..index].contains(name) {
            return Err(ProgramError::new(
                None,
                format!("input name `{name}` is declared twice"),
            ));
        }
    }
    Ok(())
}

/// A program wrapped for compilation.
#[derive(Clone, Debug)]
pub struct Assembled {
    /// The complete source: header, the author's text, trailer.
    pub source: String,
    /// Lines before the author's text.
    pub header_lines: u32,
    /// Lines of the author's text.
    pub author_lines: u32,
}

const HEADER: &str = "#version 450
layout(set = 0, binding = 0, std140) uniform VspUniforms {
    vec4 vsp_resolution;
    vec4 vsp_time;
    ivec4 vsp_frame;
    vec4 vsp_mouse;
    vec4 vsp_camera;
    vec4 vsp_chres[4];
    vec4 vsp_inputs[4];
};
layout(set = 0, binding = 1) uniform texture2D vsp_ch0;
layout(set = 0, binding = 2) uniform texture2D vsp_ch1;
layout(set = 0, binding = 3) uniform texture2D vsp_ch2;
layout(set = 0, binding = 4) uniform texture2D vsp_ch3;
layout(set = 0, binding = 5) uniform texture2D vsp_tx0;
layout(set = 0, binding = 6) uniform texture2D vsp_tx1;
layout(set = 0, binding = 7) uniform texture2D vsp_tx2;
layout(set = 0, binding = 8) uniform texture2D vsp_tx3;
layout(set = 0, binding = 9) uniform texture2D vsp_noise;
layout(set = 0, binding = 10) uniform sampler vsp_clamp;
layout(set = 0, binding = 11) uniform sampler vsp_repeat;
layout(location = 0) out vec4 vsp_out;
#define iResolution vsp_resolution.xyz
#define iTime vsp_time.x
#define iTimeDelta vsp_time.y
#define iFrame vsp_frame.x
#define iMouse vsp_mouse
#define iCamera vsp_camera
#define iChannelResolution vsp_chres
#define iChannel0 sampler2D(vsp_ch0, vsp_clamp)
#define iChannel1 sampler2D(vsp_ch1, vsp_clamp)
#define iChannel2 sampler2D(vsp_ch2, vsp_clamp)
#define iChannel3 sampler2D(vsp_ch3, vsp_clamp)
#define iTexture0 sampler2D(vsp_tx0, vsp_repeat)
#define iTexture1 sampler2D(vsp_tx1, vsp_repeat)
#define iTexture2 sampler2D(vsp_tx2, vsp_repeat)
#define iTexture3 sampler2D(vsp_tx3, vsp_repeat)
#define iNoise sampler2D(vsp_noise, vsp_repeat)
";

const TRAILER: &str = "
void main() {
    vec4 vsp_color = vec4(0.0, 0.0, 0.0, 1.0);
    mainImage(vsp_color, gl_FragCoord.xy);
    vsp_out = vsp_color;
}
";

/// Whether a line is a precision statement (after optional whitespace).
fn is_precision_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed
        .strip_prefix("precision")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(|c: char| c.is_whitespace()))
}

/// Wraps `text` with the fixed header (plus one definition per named input) and trailer. Lines
/// beginning with a precision statement are removed (left blank, so line numbers stay).
pub fn assemble(text: &str, input_names: &[String]) -> Result<Assembled, ProgramError> {
    if text.len() > MAX_SOURCE_BYTES {
        return Err(ProgramError::new(
            None,
            format!(
                "program of {} bytes exceeds the limit of {MAX_SOURCE_BYTES} bytes",
                text.len()
            ),
        ));
    }
    check_input_names(input_names)?;
    let mut source = String::from(HEADER);
    for (index, name) in input_names.iter().enumerate() {
        let lane = ["x", "y", "z", "w"][index % 4];
        source.push_str(&format!(
            "#define {name} vsp_inputs[{}].{lane}\n",
            index / 4
        ));
    }
    let header_lines = source.matches('\n').count() as u32;
    let mut author_lines = 0;
    for line in text.split('\n') {
        author_lines += 1;
        if !is_precision_line(line) {
            source.push_str(line);
        }
        source.push('\n');
    }
    source.push_str(TRAILER);
    Ok(Assembled {
        source,
        header_lines,
        author_lines,
    })
}

fn defines_entry(text: &str) -> bool {
    let mut rest = text;
    while let Some(at) = rest.find(ENTRY_NAME) {
        let before_ok = rest[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !(c == '_' || c.is_ascii_alphanumeric()));
        let after = &rest[at + ENTRY_NAME.len()..];
        let after_ok = after
            .chars()
            .next()
            .is_none_or(|c| !(c == '_' || c.is_ascii_alphanumeric()));
        if before_ok && after_ok && after.trim_start().starts_with('(') {
            return true;
        }
        rest = after;
    }
    false
}

/// Maps a byte offset of the assembled source to a line of the author's text.
fn author_line(assembled: &Assembled, offset: usize) -> Option<u32> {
    let line = assembled.source[..offset.min(assembled.source.len())]
        .matches('\n')
        .count() as u32
        + 1;
    (line > assembled.header_lines && line <= assembled.header_lines + assembled.author_lines)
        .then(|| line - assembled.header_lines)
}

/// Compiles an assembled program to a validated module, reporting the first error.
pub fn compile(assembled: &Assembled) -> Result<naga::Module, ProgramError> {
    let mut frontend = naga::front::glsl::Frontend::default();
    let options = naga::front::glsl::Options::from(naga::ShaderStage::Fragment);
    let module = frontend
        .parse(&options, &assembled.source)
        .map_err(|errors| {
            let first = errors.errors.first();
            match first {
                Some(error) => ProgramError::new(
                    author_line(assembled, error.meta.to_range().map_or(0, |r| r.start)),
                    error.kind.to_string(),
                ),
                None => ProgramError::new(None, "the program does not compile"),
            }
        })?;
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    );
    validator.validate(&module).map_err(|error| {
        // The most specific span (the innermost construct) inside the author's text.
        let line = error
            .spans()
            .filter_map(|(span, _)| span.to_range())
            .filter(|range| author_line(assembled, range.start).is_some())
            .min_by_key(|range| range.end - range.start)
            .and_then(|range| author_line(assembled, range.start));
        let mut message = error.as_inner().to_string();
        let mut source = std::error::Error::source(error.as_inner());
        while let Some(cause) = source {
            message.push_str(": ");
            message.push_str(&cause.to_string());
            source = cause.source();
        }
        ProgramError::new(line, message)
    })?;
    Ok(module)
}

/// Validates a program and its input names before display: the limits (65,536 bytes, 16
/// inputs), the entry function and a full compile with validation. Returns the first error with
/// its line number in the author's own text.
pub fn validate_program(text: &str, input_names: &[String]) -> Result<(), ProgramError> {
    let assembled = assemble(text, input_names)?;
    if !defines_entry(text) {
        return Err(ProgramError::new(
            None,
            format!("the program does not define the entry function `{ENTRY_NAME}`"),
        ));
    }
    compile(&assembled).map(|_| ())
}
