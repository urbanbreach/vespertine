//! Language identities and their resolution from aliases, paths and shebangs (TXT-007 R1-R3, R5).

/// A supported language family (TXT-007 R2: sixteen identities).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    Rust = 0,
    JavaScript = 1,
    Jsx = 2,
    TypeScript = 3,
    Tsx = 4,
    Python = 5,
    Go = 6,
    Json = 7,
    /// JSON with comments.
    Jsonc = 8,
    /// Shell scripts (bash, sh, zsh, console).
    Shell = 9,
    Toml = 10,
    Markdown = 11,
    Html = 12,
    Css = 13,
    Yaml = 14,
    C = 15,
}

impl Language {
    /// All sixteen language identities.
    pub const ALL: [Language; 16] = [
        Language::Rust,
        Language::JavaScript,
        Language::Jsx,
        Language::TypeScript,
        Language::Tsx,
        Language::Python,
        Language::Go,
        Language::Json,
        Language::Jsonc,
        Language::Shell,
        Language::Toml,
        Language::Markdown,
        Language::Html,
        Language::Css,
        Language::Yaml,
        Language::C,
    ];

    /// Human-readable name.
    pub fn name(self) -> &'static str {
        match self {
            Language::Rust => "Rust",
            Language::JavaScript => "JavaScript",
            Language::Jsx => "JSX",
            Language::TypeScript => "TypeScript",
            Language::Tsx => "TSX",
            Language::Python => "Python",
            Language::Go => "Go",
            Language::Json => "JSON",
            Language::Jsonc => "JSON with comments",
            Language::Shell => "shell",
            Language::Toml => "TOML",
            Language::Markdown => "Markdown",
            Language::Html => "HTML",
            Language::Css => "CSS",
            Language::Yaml => "YAML",
            Language::C => "C",
        }
    }

    /// Grammar names tried in order (TXT-007 R5).
    pub(crate) fn grammar_names(self) -> &'static [&'static str] {
        match self {
            Language::Rust => &["Rust"],
            Language::JavaScript => &["JavaScript"],
            Language::Jsx => &["TypeScriptReact", "JavaScript"],
            Language::TypeScript => &["TypeScript"],
            Language::Tsx => &["TypeScriptReact"],
            Language::Python => &["Python"],
            Language::Go => &["Go"],
            Language::Json => &["JSON"],
            Language::Jsonc => &["JSON with Comments", "JSONC", "JSON"],
            Language::Shell => &["Bourne Again Shell (bash)", "Bash"],
            Language::Toml => &["TOML"],
            Language::Markdown => &["Markdown"],
            Language::Html => &["HTML"],
            Language::Css => &["CSS"],
            Language::Yaml => &["YAML"],
            Language::C => &["C"],
        }
    }

    /// File extensions tried in order when no grammar name resolves (TXT-007 R5).
    pub(crate) fn grammar_extensions(self) -> &'static [&'static str] {
        match self {
            Language::Rust => &["rs"],
            Language::JavaScript => &["js"],
            Language::Jsx => &["jsx", "tsx"],
            Language::TypeScript => &["ts"],
            Language::Tsx => &["tsx"],
            Language::Python => &["py"],
            Language::Go => &["go"],
            Language::Json | Language::Jsonc => &["json"],
            Language::Shell => &["bash", "sh"],
            Language::Toml => &["toml"],
            Language::Markdown => &["md"],
            Language::Html => &["html"],
            Language::Css => &["css"],
            Language::Yaml => &["yaml", "yml"],
            Language::C => &["c"],
        }
    }
}

/// Resolves a language alias (a fenced-code label or host property): trim, take the first
/// whitespace-separated token, compare ASCII case-insensitively (TXT-007 R2).
pub fn resolve_alias(alias: &str) -> Option<Language> {
    let token = alias.split_whitespace().next()?;
    let mut lower = [0u8; 16];
    let bytes = token.as_bytes();
    if bytes.len() > lower.len() {
        return None;
    }
    for (slot, byte) in lower.iter_mut().zip(bytes) {
        *slot = byte.to_ascii_lowercase();
    }
    let lower = std::str::from_utf8(&lower[..bytes.len()]).ok()?;
    Some(match lower {
        "rust" | "rs" => Language::Rust,
        "javascript" | "js" | "mjs" | "cjs" => Language::JavaScript,
        "jsx" => Language::Jsx,
        "typescript" | "ts" | "mts" | "cts" => Language::TypeScript,
        "tsx" => Language::Tsx,
        "python" | "py" | "python3" => Language::Python,
        "go" | "golang" => Language::Go,
        "json" => Language::Json,
        "jsonc" => Language::Jsonc,
        "bash" | "sh" | "shell" | "zsh" | "console" => Language::Shell,
        "toml" => Language::Toml,
        "markdown" | "md" => Language::Markdown,
        "html" | "htm" => Language::Html,
        "css" => Language::Css,
        "yaml" | "yml" => Language::Yaml,
        "c" | "h" => Language::C,
        _ => return None,
    })
}

/// Resolves a path: first the exact lowercase basename (`cargo.lock`, `cargo.toml`,
/// `pyproject.toml` -> TOML), then the lowercase extension looked up in the alias table
/// (TXT-007 R1). Both `/` and `\` separate path components.
pub fn resolve_path(path: &str) -> Option<Language> {
    let basename = path.rsplit(['/', '\\']).next()?;
    let lower = basename.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "cargo.lock" | "cargo.toml" | "pyproject.toml"
    ) {
        return Some(Language::Toml);
    }
    let (stem, extension) = lower.rsplit_once('.')?;
    if stem.is_empty() {
        return None;
    }
    resolve_alias(extension)
}

/// Resolves a shebang on the first line of a source (TXT-007 R3). Only the text before the
/// first newline is considered.
pub fn resolve_shebang(first_line: &str) -> Option<Language> {
    let line = first_line.split('\n').next().unwrap_or("");
    if !line.starts_with("#!") {
        return None;
    }
    let lower = line.to_ascii_lowercase();
    if lower.contains("python") {
        Some(Language::Python)
    } else if lower.contains("node") {
        Some(Language::JavaScript)
    } else if ["bash", "zsh", "/sh", " sh"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        Some(Language::Shell)
    } else {
        None
    }
}

/// Combined language detection (TXT-007 R1): a recognized alias beats the path, the path beats
/// the shebang; an unrecognized alias lets resolution continue. `None` means plain text.
pub fn detect(
    fence_label: Option<&str>,
    path: Option<&str>,
    first_line: Option<&str>,
) -> Option<Language> {
    fence_label
        .and_then(resolve_alias)
        .or_else(|| path.and_then(resolve_path))
        .or_else(|| first_line.and_then(resolve_shebang))
}
