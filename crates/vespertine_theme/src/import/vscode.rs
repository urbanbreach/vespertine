//! Adapter for the widely used editor theme format: JSON with comments, a `colors` workbench
//! key map, `tokenColors` rules (inline or in an external file), optional
//! `semanticTokenColors` and an `include` chain; packages declare variants under
//! `contributes.themes` in `package.json` (https://code.visualstudio.com/api/references/theme-color).
//!
//! External token files must be JSON(C); property-list (`.tmTheme`) files are rejected with a
//! readable error.

use super::{
    Compilation, ImportAdapter, ImportError, ImportOptions, MAX_INCLUDE_DEPTH,
    MAX_PACKAGE_VARIANTS, MAX_SOURCE_BYTES, SourceKind, VariantFailure, VariantReport,
    compile::{Candidate, RawVariant, Role, compile_variant, detect_appearance_from_background},
};
use crate::{
    color::Color,
    theme::{Appearance, SourceMetadata, SyntaxCategory, ThemeFamily, ThemeVariant},
};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

/// The adapter for the editor theme format described in the module documentation.
#[derive(Clone, Copy, Debug, Default)]
pub struct VsCodeAdapter;

const FORMAT: &str = "vscode";

/// Ordered candidate workbench keys per role.
const ROLE_KEYS: &[(Role, &[&str])] = &[
    (Role::Background, &["editor.background"]),
    (
        Role::Shell,
        &[
            "sideBar.background",
            "activityBar.background",
            "titleBar.activeBackground",
            "statusBar.background",
        ],
    ),
    (
        Role::Raised,
        &[
            "editorWidget.background",
            "tab.inactiveBackground",
            "sideBarSectionHeader.background",
            "panel.background",
        ],
    ),
    (
        Role::Card,
        &[
            "editorGroupHeader.tabsBackground",
            "panel.background",
            "sideBarSectionHeader.background",
        ],
    ),
    (
        Role::Dialog,
        &[
            "quickInput.background",
            "editorWidget.background",
            "menu.background",
        ],
    ),
    (
        Role::Overlay,
        &[
            "editorHoverWidget.background",
            "editorSuggestWidget.background",
            "menu.background",
            "editorWidget.background",
        ],
    ),
    (
        Role::Input,
        &[
            "input.background",
            "dropdown.background",
            "settings.textInputBackground",
        ],
    ),
    (
        Role::Solid,
        &[
            "button.background",
            "badge.background",
            "activityBarBadge.background",
        ],
    ),
    (
        Role::OnSolid,
        &[
            "button.foreground",
            "badge.foreground",
            "activityBarBadge.foreground",
        ],
    ),
    (
        Role::Hover,
        &[
            "list.hoverBackground",
            "toolbar.hoverBackground",
            "editor.hoverHighlightBackground",
        ],
    ),
    (
        Role::Active,
        &[
            "list.activeSelectionBackground",
            "list.inactiveSelectionBackground",
            "toolbar.activeBackground",
        ],
    ),
    (
        Role::Border,
        &[
            "panel.border",
            "editorGroup.border",
            "sideBar.border",
            "contrastBorder",
            "widget.border",
        ],
    ),
    (
        Role::BorderStrong,
        &[
            "focusBorder",
            "contrastActiveBorder",
            "editorWidget.border",
            "input.border",
        ],
    ),
    (
        Role::Text,
        &[
            "editor.foreground",
            "foreground",
            "sideBar.foreground",
            "terminal.foreground",
        ],
    ),
    (
        Role::TextMuted,
        &[
            "descriptionForeground",
            "sideBar.foreground",
            "tab.inactiveForeground",
            "editorLineNumber.foreground",
        ],
    ),
    (
        Role::TextFaint,
        &[
            "disabledForeground",
            "editorLineNumber.foreground",
            "input.placeholderForeground",
            "editorGhostText.foreground",
        ],
    ),
    (
        Role::Danger,
        &[
            "errorForeground",
            "editorError.foreground",
            "terminal.ansiRed",
        ],
    ),
    (
        Role::Warning,
        &[
            "editorWarning.foreground",
            "list.warningForeground",
            "terminal.ansiYellow",
        ],
    ),
    (
        Role::Success,
        &[
            "gitDecoration.addedResourceForeground",
            "testing.iconPassed",
            "terminal.ansiGreen",
            "editorGutter.addedBackground",
        ],
    ),
    (
        Role::Cursor,
        &["editorCursor.foreground", "terminalCursor.foreground"],
    ),
    (
        Role::DiffAdd,
        &[
            "gitDecoration.addedResourceForeground",
            "editorGutter.addedBackground",
            "diffEditor.insertedTextBackground",
            "diffEditor.insertedLineBackground",
        ],
    ),
    (
        Role::DiffDelete,
        &[
            "gitDecoration.deletedResourceForeground",
            "editorGutter.deletedBackground",
            "diffEditor.removedTextBackground",
            "diffEditor.removedLineBackground",
        ],
    ),
    (
        Role::DiffHunk,
        &[
            "editorGutter.modifiedBackground",
            "gitDecoration.modifiedResourceForeground",
            "diffEditor.diagonalFill",
        ],
    ),
    (
        Role::TerminalBackground,
        &[
            "terminal.background",
            "panel.background",
            "editor.background",
        ],
    ),
    (
        Role::TerminalForeground,
        &["terminal.foreground", "editor.foreground", "foreground"],
    ),
    (
        Role::TerminalSelection,
        &["terminal.selectionBackground", "editor.selectionBackground"],
    ),
];

const ACCENT_KEYS: &[&str] = &[
    "focusBorder",
    "button.background",
    "textLink.foreground",
    "activityBarBadge.background",
    "progressBar.background",
    "editorCursor.foreground",
    "tab.activeBorder",
];

const ANSI_NAMES: [&str; 8] = [
    "Black", "Red", "Green", "Yellow", "Blue", "Magenta", "Cyan", "White",
];

/// The VS Code key of ANSI color `index` (0-7 normal, 8-15 bright).
fn ansi_key(index: usize) -> String {
    let prefix = if index < ANSI_NAMES.len() {
        ""
    } else {
        "Bright"
    };
    format!(
        "terminal.ansi{prefix}{}",
        ANSI_NAMES[index % ANSI_NAMES.len()]
    )
}

/// Ordered candidate TextMate scopes per syntax category.
fn syntax_scopes(category: SyntaxCategory) -> &'static [&'static str] {
    use SyntaxCategory::*;
    match category {
        Comment => &["comment"],
        Keyword => &["keyword", "keyword.control", "storage"],
        String => &["string"],
        StringSpecial => &[
            "string.regexp",
            "constant.other.symbol",
            "string.interpolated",
        ],
        Escape => &["constant.character.escape"],
        Number => &["constant.numeric"],
        Boolean => &["constant.language.boolean", "constant.language"],
        TypeName => &[
            "entity.name.type",
            "entity.name.class",
            "support.type",
            "support.class",
        ],
        TypeBuiltin => &[
            "support.type.primitive",
            "storage.type.primitive",
            "support.type",
        ],
        Constructor => &["entity.name.function.constructor", "entity.name.class"],
        Function => &["entity.name.function", "support.function"],
        FunctionBuiltin => &["support.function.builtin", "support.function"],
        MacroName => &[
            "entity.name.function.macro",
            "support.function.macro",
            "meta.preprocessor",
        ],
        Property => &[
            "variable.other.property",
            "variable.other.object.property",
            "support.type.property-name",
            "meta.object-literal.key",
        ],
        Constant => &["variable.other.constant", "constant.other", "constant"],
        Variable => &["variable.other", "variable"],
        VariableSpecial => &["variable.language"],
        Parameter => &["variable.parameter"],
        Operator => &["keyword.operator"],
        Punctuation => &["punctuation", "meta.brace"],
        Tag => &["entity.name.tag", "meta.tag"],
        Attribute => &["entity.other.attribute-name"],
        Label => &["entity.name.label", "entity.name.section"],
        Invalid => &["invalid.illegal", "invalid"],
        MarkupHeading => &["markup.heading", "entity.name.section.markdown"],
        MarkupRaw => &["markup.inline.raw", "markup.raw", "markup.fenced_code"],
        MarkupLink => &["markup.underline.link", "string.other.link"],
        MarkupReference => &[
            "string.other.link.description",
            "constant.other.reference.link",
        ],
        MarkupEmphasis => &["markup.italic"],
        MarkupStrong => &["markup.bold"],
    }
}

/// Semantic token type names mapped to categories (used only where no token rule matched).
fn semantic_category(token_type: &str) -> Option<SyntaxCategory> {
    use SyntaxCategory::*;
    Some(match token_type {
        "comment" => Comment,
        "keyword" => Keyword,
        "string" => String,
        "number" => Number,
        "regexp" => StringSpecial,
        "type" | "class" | "struct" | "interface" | "enum" | "typeParameter" => TypeName,
        "function" | "method" => Function,
        "macro" => MacroName,
        "property" => Property,
        "variable" => Variable,
        "parameter" => Parameter,
        "operator" => Operator,
        "enumMember" => Constant,
        "decorator" => Attribute,
        _ => return None,
    })
}

#[derive(Default)]
struct ThemeData {
    name: Option<String>,
    kind: Option<String>,
    colors: Map<String, Value>,
    rules: Vec<Rule>,
    semantic: Map<String, Value>,
    warnings: Vec<String>,
}

struct Rule {
    scopes: Vec<String>,
    foreground: String,
}

/// Reads files confined to one theme root and hashes everything it reads.
struct Loader {
    root: PathBuf,
    hash: Sha256,
}

impl Loader {
    fn new(root: &Path) -> Result<Self, String> {
        Ok(Self {
            root: root
                .canonicalize()
                .map_err(|error| format!("cannot read theme root {}: {error}", root.display()))?,
            hash: Sha256::new(),
        })
    }

    /// Canonicalizes `base.join(relative)` and requires the result to stay inside the root;
    /// symlinks are fine only when their targets remain inside.
    fn resolve(&self, base: &Path, relative: &str) -> Result<PathBuf, String> {
        let joined = base.join(relative);
        let canonical = joined
            .canonicalize()
            .map_err(|error| format!("cannot read {}: {error}", joined.display()))?;
        if !canonical.starts_with(&self.root) {
            return Err(format!(
                "{} resolves outside the theme root {}",
                joined.display(),
                self.root.display()
            ));
        }
        Ok(canonical)
    }

    fn read_json(&mut self, canonical: &Path) -> Result<Value, String> {
        let length = std::fs::metadata(canonical)
            .map_err(|error| format!("cannot read {}: {error}", canonical.display()))?
            .len();
        if length > MAX_SOURCE_BYTES {
            return Err(format!(
                "{} is {length} bytes; the limit is {MAX_SOURCE_BYTES} bytes",
                canonical.display()
            ));
        }
        let bytes = std::fs::read(canonical)
            .map_err(|error| format!("cannot read {}: {error}", canonical.display()))?;
        if bytes.len() as u64 > MAX_SOURCE_BYTES {
            return Err(format!(
                "{} is {} bytes; the limit is {MAX_SOURCE_BYTES} bytes",
                canonical.display(),
                bytes.len()
            ));
        }
        self.hash.update(&bytes);
        serde_json_lenient::from_slice::<Value>(&bytes)
            .map_err(|error| format!("{} is not valid JSON: {error}", canonical.display()))
    }

    fn finish_hash(self) -> String {
        format!("{:x}", self.hash.finalize())
    }

    fn load_theme(&mut self, file: &Path, stack: &mut Vec<PathBuf>) -> Result<ThemeData, String> {
        if let Some(position) = stack.iter().position(|seen| seen == file) {
            let chain: Vec<String> = stack[position..]
                .iter()
                .chain([&file.to_path_buf()])
                .map(|path| path.display().to_string())
                .collect();
            return Err(format!("include cycle: {}", chain.join(" -> ")));
        }
        if stack.len() > MAX_INCLUDE_DEPTH {
            return Err(format!(
                "include chain deeper than {MAX_INCLUDE_DEPTH} at {}",
                file.display()
            ));
        }
        let value = self.read_json(file)?;
        let Value::Object(object) = value else {
            return Err(format!("{} is not a JSON object", file.display()));
        };
        let directory = file.parent().unwrap_or(&self.root).to_path_buf();
        stack.push(file.to_path_buf());
        let mut data = match object.get("include") {
            Some(Value::String(include)) => {
                let target = self.resolve(&directory, include)?;
                self.load_theme(&target, stack)?
            }
            Some(_) => {
                stack.pop();
                return Err(format!("`include` in {} must be a string", file.display()));
            }
            None => ThemeData::default(),
        };
        stack.pop();
        if let Some(Value::String(name)) = object.get("name") {
            data.name = Some(name.clone());
        }
        if let Some(Value::String(kind)) = object.get("type") {
            data.kind = Some(kind.clone());
        }
        match object.get("colors") {
            Some(Value::Object(colors)) => {
                for (key, value) in colors {
                    data.colors.insert(key.clone(), value.clone());
                }
            }
            Some(_) => return Err(format!("`colors` in {} must be an object", file.display())),
            None => {}
        }
        match object.get("tokenColors") {
            Some(Value::Array(rules)) => parse_rules(rules, &mut data),
            Some(Value::String(path)) => {
                let target = self.resolve(&directory, path)?;
                let loaded = self.read_json(&target).map_err(|error| {
                    format!(
                        "token colors file {}: {error} (only JSON token files are supported, not property lists)",
                        target.display()
                    )
                })?;
                let rules = match &loaded {
                    Value::Array(rules) => rules.as_slice(),
                    Value::Object(object) => object
                        .get("tokenColors")
                        .or_else(|| object.get("settings"))
                        .and_then(Value::as_array)
                        .map(Vec::as_slice)
                        .ok_or_else(|| {
                            format!("{} holds no token color rules", target.display())
                        })?,
                    _ => return Err(format!("{} holds no token color rules", target.display())),
                };
                parse_rules(rules, &mut data);
            }
            Some(_) => {
                return Err(format!(
                    "`tokenColors` in {} must be an array or a path",
                    file.display()
                ));
            }
            None => {}
        }
        match object.get("semanticTokenColors") {
            Some(Value::Object(semantic)) => {
                for (key, value) in semantic {
                    data.semantic.insert(key.clone(), value.clone());
                }
            }
            Some(_) => {
                return Err(format!(
                    "`semanticTokenColors` in {} must be an object",
                    file.display()
                ));
            }
            None => {}
        }
        Ok(data)
    }
}

fn parse_rules(rules: &[Value], data: &mut ThemeData) {
    for rule in rules {
        let Some(foreground) = rule
            .get("settings")
            .and_then(|settings| settings.get("foreground"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let scopes: Vec<String> = match rule.get("scope") {
            Some(Value::String(scope)) => scope.split(',').map(|s| s.trim().to_string()).collect(),
            Some(Value::Array(scopes)) => scopes
                .iter()
                .filter_map(Value::as_str)
                .flat_map(|scope| scope.split(',').map(|s| s.trim().to_string()))
                .collect(),
            _ => continue,
        };
        data.rules.push(Rule {
            scopes: scopes
                .into_iter()
                .filter(|scope| !scope.is_empty())
                .collect(),
            foreground: foreground.to_string(),
        });
    }
}

fn scope_matches(selector: &str, scope: &str) -> bool {
    scope == selector || (scope.starts_with(selector) && scope[selector.len()..].starts_with('.'))
}

/// The most specific rule (longest matching selector, later rule wins ties) for a scope.
fn rule_for_scope<'a>(rules: &'a [Rule], scope: &str) -> Option<(&'a str, &'a str)> {
    let mut best: Option<(&str, &str)> = None;
    for rule in rules {
        for selector in &rule.scopes {
            if scope_matches(selector, scope)
                && best.is_none_or(|(current, _)| selector.len() >= current.len())
            {
                best = Some((selector.as_str(), rule.foreground.as_str()));
            }
        }
    }
    best
}

fn slug(text: &str) -> String {
    let mut slug = String::new();
    for character in text.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_end_matches('-').to_string()
}

fn variant_id(family_id: &str, label: &str, taken: &HashSet<String>) -> String {
    let slug = slug(label);
    let base = if slug.is_empty() || slug == family_id {
        family_id.to_string()
    } else {
        format!("{family_id}-{slug}")
    };
    let mut id = base.clone();
    let mut suffix = 2;
    while taken.contains(&id) {
        id = format!("{base}-{suffix}");
        suffix += 1;
    }
    id
}

fn ui_theme_appearance(ui_theme: &str) -> Option<Appearance> {
    match ui_theme.to_ascii_lowercase().as_str() {
        "vs" | "hc-light" => Some(Appearance::Light),
        "vs-dark" | "hc-black" => Some(Appearance::Dark),
        _ => None,
    }
}

fn kind_appearance(kind: &str) -> Option<Appearance> {
    match kind.to_ascii_lowercase().as_str() {
        "light" => Some(Appearance::Light),
        "dark" => Some(Appearance::Dark),
        _ => None,
    }
}

fn build_raw(
    data: &ThemeData,
    id: String,
    name: String,
    ui_theme: Option<&str>,
    options: &ImportOptions,
    source: SourceMetadata,
) -> RawVariant {
    let mut warnings = data.warnings.clone();
    let mut colors: HashMap<&str, Color> = HashMap::new();
    for (key, value) in &data.colors {
        match value.as_str().map(Color::parse) {
            Some(Ok(color)) => {
                colors.insert(key.as_str(), color);
            }
            _ => warnings.push(format!(
                "colors[{key}] is not a valid color ({value}); ignored"
            )),
        }
    }
    let candidates = |keys: &[&str]| -> Vec<Candidate> {
        keys.iter()
            .filter_map(|key| {
                colors.get(key).map(|color| Candidate {
                    source_key: (*key).to_string(),
                    color: *color,
                })
            })
            .collect()
    };
    let mut roles = HashMap::new();
    for (role, keys) in ROLE_KEYS {
        let found = candidates(keys);
        if !found.is_empty() {
            roles.insert(*role, found);
        }
    }
    let appearance = ui_theme
        .and_then(ui_theme_appearance)
        .or_else(|| data.kind.as_deref().and_then(kind_appearance))
        .unwrap_or_else(|| {
            detect_appearance_from_background(colors.get("editor.background").copied())
        });

    let mut syntax = Vec::new();
    for category in SyntaxCategory::ALL {
        let scopes = syntax_scopes(category);
        let from_rules = scopes.iter().find_map(|scope| {
            let (selector, foreground) = rule_for_scope(&data.rules, scope)?;
            match Color::parse(foreground) {
                Ok(color) => Some(Candidate {
                    source_key: format!("tokenColors[{selector}]"),
                    color,
                }),
                Err(_) => None,
            }
        });
        let candidate = from_rules.or_else(|| {
            data.semantic.iter().find_map(|(key, value)| {
                let token_type = key.split(['.', ':']).next()?;
                if semantic_category(token_type) != Some(category) {
                    return None;
                }
                let text = match value {
                    Value::String(text) => text.as_str(),
                    Value::Object(object) => object.get("foreground")?.as_str()?,
                    _ => return None,
                };
                Some(Candidate {
                    source_key: format!("semanticTokenColors[{key}]"),
                    color: Color::parse(text).ok()?,
                })
            })
        });
        if let Some(candidate) = candidate {
            syntax.push((category, candidate));
        }
    }

    let mut ansi: [Option<Candidate>; 16] = Default::default();
    for (index, slot) in ansi.iter_mut().enumerate() {
        let key = ansi_key(index);
        *slot = colors.get(key.as_str()).map(|color| Candidate {
            source_key: key,
            color: *color,
        });
    }
    RawVariant {
        id,
        name,
        family_id: options.family_id.clone(),
        appearance,
        roles,
        accent: candidates(ACCENT_KEYS),
        syntax,
        ansi,
        warnings,
        source,
    }
}

fn source_metadata(options: &ImportOptions, path: &Path, asset_hash: String) -> SourceMetadata {
    let or_default = |value: &str, default: String| {
        if value.trim().is_empty() {
            default
        } else {
            value.to_string()
        }
    };
    SourceMetadata {
        format: FORMAT.to_string(),
        url: or_default(&options.source_url, path.display().to_string()),
        revision: or_default(&options.revision, "local".to_string()),
        license: or_default(&options.license, "User supplied".to_string()),
        asset_hash,
    }
}

fn compile_file(
    root: &Path,
    file: &Path,
    label: Option<&str>,
    ui_theme: Option<&str>,
    id: String,
    options: &ImportOptions,
) -> Result<(ThemeVariant, VariantReport), String> {
    let mut loader = Loader::new(root)?;
    let canonical = loader.resolve(&loader.root.clone(), &file.to_string_lossy())?;
    let data = loader.load_theme(&canonical, &mut Vec::new())?;
    let name = label
        .map(str::to_string)
        .or_else(|| data.name.clone())
        .or_else(|| {
            canonical
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| id.clone());
    let source = source_metadata(options, file, String::new());
    let raw = build_raw(&data, id, name, ui_theme, options, source);
    let mut raw = raw;
    raw.source.asset_hash = loader.finish_hash();
    Ok(compile_variant(raw))
}

impl ImportAdapter for VsCodeAdapter {
    fn format(&self) -> &'static str {
        FORMAT
    }

    fn compile(&self, path: &Path, options: &ImportOptions) -> Result<Compilation, ImportError> {
        let canonical = path.canonicalize().map_err(|error| {
            ImportError::new(format!("cannot read {}: {error}", path.display()))
        })?;
        if canonical.is_dir() {
            compile_package(&canonical, options)
        } else {
            compile_single(&canonical, options)
        }
    }
}

fn compile_single(file: &Path, options: &ImportOptions) -> Result<Compilation, ImportError> {
    let root = file.parent().unwrap_or(file);
    let stem = file
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let id = variant_id(&options.family_id, &stem, &HashSet::new());
    match compile_file(root, file, None, None, id.clone(), options) {
        Ok((variant, report)) => Ok(Compilation {
            source_kind: SourceKind::File,
            source_path: file.to_path_buf(),
            format: FORMAT.to_string(),
            options: options.clone(),
            family: ThemeFamily {
                id: options.family_id.clone(),
                name: options.family_name.clone(),
                variants: vec![variant],
            },
            reports: vec![report],
            failures: Vec::new(),
        }),
        Err(message) => Err(ImportError {
            message: message.clone(),
            failures: vec![VariantFailure {
                id,
                name: stem,
                path: file.to_path_buf(),
                message,
            }],
        }),
    }
}

fn compile_package(root: &Path, options: &ImportOptions) -> Result<Compilation, ImportError> {
    let mut loader = Loader::new(root).map_err(ImportError::new)?;
    let manifest_path = loader
        .resolve(root, "package.json")
        .map_err(ImportError::new)?;
    let manifest = loader.read_json(&manifest_path).map_err(ImportError::new)?;
    let themes = manifest
        .get("contributes")
        .and_then(|contributes| contributes.get("themes"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ImportError::new(format!(
                "{} declares no themes (contributes.themes)",
                manifest_path.display()
            ))
        })?;
    if themes.len() > MAX_PACKAGE_VARIANTS {
        return Err(ImportError::new(format!(
            "package declares {} variants; the limit is {MAX_PACKAGE_VARIANTS}",
            themes.len()
        )));
    }
    let mut variants = Vec::new();
    let mut reports = Vec::new();
    let mut failures = Vec::new();
    let mut taken = HashSet::new();
    for (index, theme) in themes.iter().enumerate() {
        let relative = theme.get("path").and_then(Value::as_str);
        let label = theme
            .get("label")
            .or_else(|| theme.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                relative.and_then(|relative| {
                    Path::new(relative)
                        .file_stem()
                        .map(|stem| stem.to_string_lossy().into_owned())
                })
            })
            .unwrap_or_else(|| format!("theme {}", index + 1));
        let id = variant_id(&options.family_id, &label, &taken);
        taken.insert(id.clone());
        let Some(relative) = relative else {
            failures.push(VariantFailure {
                id,
                name: label,
                path: root.to_path_buf(),
                message: "the manifest entry has no `path`".to_string(),
            });
            continue;
        };
        let ui_theme = theme.get("uiTheme").and_then(Value::as_str);
        match compile_file(
            root,
            Path::new(relative),
            Some(&label),
            ui_theme,
            id.clone(),
            options,
        ) {
            Ok((variant, report)) => {
                variants.push(variant);
                reports.push(report);
            }
            Err(message) => failures.push(VariantFailure {
                id,
                name: label,
                path: root.join(relative),
                message,
            }),
        }
    }
    if variants.is_empty() {
        return Err(ImportError {
            message: "no theme variant of the package could be compiled".to_string(),
            failures,
        });
    }
    Ok(Compilation {
        source_kind: SourceKind::Package,
        source_path: root.to_path_buf(),
        format: FORMAT.to_string(),
        options: options.clone(),
        family: ThemeFamily {
            id: options.family_id.clone(),
            name: options.family_name.clone(),
            variants,
        },
        reports,
        failures,
    })
}
