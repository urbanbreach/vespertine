//! The PERF-TEXT corpus, also used by PERF-STREAM.

const WORDS: [&str; 64] = [
    "let", "fn", "struct", "impl", "match", "self", "return", "mut", "pub", "use", "mod", "enum",
    "trait", "where", "async", "await", "loop", "while", "for", "in", "if", "else", "const",
    "static", "type", "move", "ref", "box", "dyn", "unsafe", "crate", "super", "true", "false",
    "some", "none", "ok", "err", "vec", "string", "option", "result", "into", "from", "clone",
    "copy", "debug", "default", "hash", "order", "write", "read", "build", "render", "frame",
    "layout", "paint", "text", "glyph", "window", "event", "focus", "scroll", "theme",
];

pub const LINE_LEN: usize = 120;

/// Words of corpus line `k`, concatenated with single spaces until at least 120 characters
/// (before truncation).
pub fn line_words(k: usize) -> Vec<&'static str> {
    let mut words = Vec::new();
    let mut length = 0;
    let mut w = 0;
    while length < LINE_LEN {
        let word = WORDS[(k * 7 + w * 13) % 64];
        if w > 0 {
            length += 1;
        }
        length += word.len();
        words.push(word);
        w += 1;
    }
    words
}

/// Corpus line `k`, exactly 120 characters.
pub fn line(k: usize) -> String {
    let mut text = line_words(k).join(" ");
    text.truncate(LINE_LEN);
    text
}

/// Token `t`: four characters of corpus line `t / 30`.
pub fn token(t: usize) -> String {
    let text = line(t / 30);
    let start = (t % 30) * 4;
    text[start..start + 4].to_string()
}
