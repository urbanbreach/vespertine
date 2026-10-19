//! Highlight declarations (TXT-009 R1-R2): specification types, a typed builder API, and the
//! lenient parser for dynamically typed declarations.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::Range;

use serde_json::{Map, Value};
use vespertine::{Hsla, Rgba};

/// Default wash corner radius in logical pixels.
pub const DEFAULT_RADIUS: f32 = 2.0;

/// Parses a hex color (`#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`). Hosts with a richer notation
/// parser (THM-001) pass their own function to [`HighlightDeclaration::from_value`].
pub fn default_hex_color(text: &str) -> Option<Hsla> {
    Rgba::try_from(text).ok().map(Hsla::from)
}

/// One highlight specification.
#[derive(Clone, Debug, PartialEq)]
pub struct HighlightSpec {
    /// Literal text to search for (empty: no query matches).
    pub query: String,
    /// Explicit `[start, end)` ranges in UTF-16 code units of the declaration's document. Every
    /// range is already known to satisfy `start < end`.
    pub ranges: Vec<Range<usize>>,
    /// Wash color; `None` means the theme default (accent with alpha 0.30).
    pub color: Option<Hsla>,
    /// Active wash color; `None` means the theme default (accent with alpha 0.65).
    pub active_color: Option<Hsla>,
    pub case_sensitive: bool,
    pub whole_word: bool,
    /// Displayed number of the active match, if any.
    pub active_index: Option<usize>,
    /// Number of matches that precede this subtree in the caller's document.
    pub match_index_offset: usize,
    /// Wash corner radius in logical pixels.
    pub radius: f32,
}

impl Default for HighlightSpec {
    fn default() -> Self {
        Self {
            query: String::new(),
            ranges: Vec::new(),
            color: None,
            active_color: None,
            case_sensitive: false,
            whole_word: false,
            active_index: None,
            match_index_offset: 0,
            radius: DEFAULT_RADIUS,
        }
    }
}

impl HighlightSpec {
    /// A specification with the given query and all other fields at their defaults.
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            ..Self::default()
        }
    }

    /// A specification with explicit ranges only.
    pub fn from_ranges(ranges: impl IntoIterator<Item = Range<usize>>) -> Self {
        Self::default().with_ranges(ranges)
    }

    /// Appends ranges; reversed or empty ranges are discarded (R2).
    pub fn with_ranges(mut self, ranges: impl IntoIterator<Item = Range<usize>>) -> Self {
        self.ranges
            .extend(ranges.into_iter().filter(|range| range.start < range.end));
        self
    }

    pub fn with_color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    pub fn with_active_color(mut self, color: Hsla) -> Self {
        self.active_color = Some(color);
        self
    }

    pub fn with_case_sensitive(mut self, case_sensitive: bool) -> Self {
        self.case_sensitive = case_sensitive;
        self
    }

    pub fn with_whole_word(mut self, whole_word: bool) -> Self {
        self.whole_word = whole_word;
        self
    }

    pub fn with_active_index(mut self, active_index: Option<usize>) -> Self {
        self.active_index = active_index;
        self
    }

    pub fn with_match_index_offset(mut self, offset: usize) -> Self {
        self.match_index_offset = offset;
        self
    }

    pub fn with_radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    /// True when the specification has no query and no valid ranges (R2: it is dropped).
    pub fn is_empty(&self) -> bool {
        self.query.is_empty() && self.ranges.is_empty()
    }

    /// The displayed number of the match with per-specification ordinal `ordinal` (R7).
    pub fn display_number(&self, ordinal: usize) -> usize {
        self.match_index_offset.saturating_add(ordinal)
    }

    /// Whether the match with per-specification ordinal `ordinal` is the active one (R7): its
    /// displayed number equals `activeIndex`. Nothing is active when no mounted match has it.
    pub fn is_active(&self, ordinal: usize) -> bool {
        self.active_index == Some(self.display_number(ordinal))
    }

    fn same_matcher(&self, other: &Self) -> bool {
        self.query == other.query
            && self.case_sensitive == other.case_sensitive
            && self.whole_word == other.whole_word
            && self.ranges == other.ranges
    }

    fn hash_matcher(&self, state: &mut impl Hasher) {
        self.query.hash(state);
        self.case_sensitive.hash(state);
        self.whole_word.hash(state);
        self.ranges.hash(state);
    }
}

/// An ordered list of specifications. Empty specifications never appear in it, so a
/// specification's index is its position among the kept ones.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HighlightDeclaration {
    specs: Vec<HighlightSpec>,
}

impl HighlightDeclaration {
    /// An empty declaration (it still owns its subtree, R3).
    pub fn new() -> Self {
        Self::default()
    }

    /// One specification (dropped when it has no query and no ranges).
    pub fn from_spec(spec: HighlightSpec) -> Self {
        Self::from_specs([spec])
    }

    /// An ordered list of specifications; empty ones are dropped (R2).
    pub fn from_specs(specs: impl IntoIterator<Item = HighlightSpec>) -> Self {
        Self {
            specs: specs.into_iter().filter(|spec| !spec.is_empty()).collect(),
        }
    }

    /// Appends a specification unless it is empty.
    pub fn with_spec(mut self, spec: HighlightSpec) -> Self {
        if !spec.is_empty() {
            self.specs.push(spec);
        }
        self
    }

    pub fn specs(&self) -> &[HighlightSpec] {
        &self.specs
    }

    /// True when nothing is declared; the declaration paints nothing and counts nothing.
    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }

    /// True when any specification carries explicit ranges.
    pub fn has_ranges(&self) -> bool {
        self.specs.iter().any(|spec| !spec.ranges.is_empty())
    }

    /// Hash of everything that decides where matches are: queries, case flags, whole-word flags
    /// and ranges of all specifications (R10). Colors, `activeIndex`, `matchIndexOffset` and
    /// `radius` are excluded. Deterministic across runs.
    pub fn matcher_hash(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.specs.len().hash(&mut hasher);
        for spec in &self.specs {
            spec.hash_matcher(&mut hasher);
        }
        hasher.finish()
    }

    /// Exact comparison of the matcher-relevant fields (what [`matcher_hash`] covers).
    ///
    /// [`matcher_hash`]: Self::matcher_hash
    pub fn same_matcher(&self, other: &Self) -> bool {
        self.specs.len() == other.specs.len()
            && self
                .specs
                .iter()
                .zip(&other.specs)
                .all(|(a, b)| a.same_matcher(b))
    }

    /// Parses a dynamically typed declaration (R1-R2): a keyed object is one specification, a
    /// list holds specifications (non-object entries are dropped), anything else is an empty
    /// declaration. Keys are camelCase. Fields of the wrong type fall back to their defaults,
    /// except `matchIndexOffset`: when present (including `null`) and not a non-negative
    /// integer the whole specification is rejected with a warning. Integral floats such as
    /// `2.0` count as integers (they cannot be told apart from integers in script values).
    ///
    /// `parse_color` converts a color notation; `None` means unparsable and the field falls back
    /// to its default.
    pub fn from_value(value: &Value, parse_color: &dyn Fn(&str) -> Option<Hsla>) -> Self {
        let mut specs = Vec::new();
        match value {
            Value::Object(object) => specs.extend(parse_spec(object, parse_color)),
            Value::Array(items) => {
                for item in items {
                    if let Value::Object(object) = item {
                        specs.extend(parse_spec(object, parse_color));
                    }
                }
            }
            _ => {}
        }
        Self::from_specs(specs)
    }
}

/// A non-negative integer from a JSON number (`3`, or an integral float like `3.0`).
fn non_negative_integer(value: &Value) -> Option<usize> {
    let Value::Number(number) = value else {
        return None;
    };
    if let Some(unsigned) = number.as_u64() {
        return usize::try_from(unsigned).ok();
    }
    // Largest integer an f64 represents exactly; beyond it "integer" is meaningless.
    const MAX_EXACT: f64 = 9_007_199_254_740_991.0;
    let float = number.as_f64()?;
    (float.is_finite() && float >= 0.0 && float.fract() == 0.0 && float <= MAX_EXACT)
        .then_some(float as usize)
}

fn parse_color_field(
    object: &Map<String, Value>,
    key: &str,
    parse_color: &dyn Fn(&str) -> Option<Hsla>,
) -> Option<Hsla> {
    object.get(key)?.as_str().and_then(parse_color)
}

fn parse_ranges(value: Option<&Value>) -> Vec<Range<usize>> {
    let Some(Value::Array(pairs)) = value else {
        return Vec::new();
    };
    pairs
        .iter()
        .filter_map(|pair| {
            let Value::Array(pair) = pair else {
                return None;
            };
            let [start, end] = pair.as_slice() else {
                return None;
            };
            let (start, end) = (non_negative_integer(start)?, non_negative_integer(end)?);
            (start < end).then_some(start..end)
        })
        .collect()
}

fn parse_spec(
    object: &Map<String, Value>,
    parse_color: &dyn Fn(&str) -> Option<Hsla>,
) -> Option<HighlightSpec> {
    let match_index_offset = match object.get("matchIndexOffset") {
        None => 0,
        Some(value) => match non_negative_integer(value) {
            Some(offset) => offset,
            None => {
                log::warn!(
                    "highlight specification rejected: matchIndexOffset {value} is not a non-negative integer"
                );
                return None;
            }
        },
    };
    let flag = |key: &str| object.get(key).and_then(Value::as_bool).unwrap_or(false);
    let radius = object
        .get("radius")
        .and_then(Value::as_f64)
        .filter(|radius| radius.is_finite() && *radius >= 0.0)
        .map_or(DEFAULT_RADIUS, |radius| radius as f32);
    Some(HighlightSpec {
        query: object
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        ranges: parse_ranges(object.get("ranges")),
        color: parse_color_field(object, "color", parse_color),
        active_color: parse_color_field(object, "activeColor", parse_color),
        case_sensitive: flag("caseSensitive"),
        whole_word: flag("wholeWord"),
        active_index: object.get("activeIndex").and_then(non_negative_integer),
        match_index_offset,
        radius,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: Value) -> HighlightDeclaration {
        HighlightDeclaration::from_value(&value, &default_hex_color)
    }

    #[test]
    fn single_object_and_list_parse_with_camel_case_keys() {
        let single = parse(json!({
            "query": "red", "caseSensitive": true, "wholeWord": true, "activeIndex": 1,
            "matchIndexOffset": 10, "radius": 4, "color": "#F0C04066", "activeColor": "#F08040AA",
            "ranges": [[0, 3], [5, 9]],
        }));
        let spec = &single.specs()[0];
        assert_eq!(single.specs().len(), 1);
        assert_eq!(spec.query, "red");
        assert!(spec.case_sensitive && spec.whole_word);
        assert_eq!(spec.active_index, Some(1));
        assert_eq!(spec.match_index_offset, 10);
        assert_eq!(spec.radius, 4.0);
        assert_eq!(spec.ranges, vec![0..3, 5..9]);
        assert_eq!(spec.color, default_hex_color("#F0C04066"));
        assert_eq!(spec.active_color, default_hex_color("#F08040AA"));

        let list = parse(json!([{"query": "a"}, {"query": "b"}]));
        assert_eq!(list.specs().len(), 2);
        assert_eq!(list.specs()[1].query, "b");
        // Defaults.
        assert_eq!(list.specs()[0], HighlightSpec::new("a"));
        assert_eq!(list.specs()[0].radius, 2.0);
    }

    #[test]
    fn unparsable_colors_fall_back_to_the_default() {
        let decl = parse(json!({"query": "a", "color": "not a color", "activeColor": 7}));
        assert_eq!(decl.specs()[0].color, None);
        assert_eq!(decl.specs()[0].active_color, None);
    }

    #[test]
    fn invalid_ranges_are_discarded_and_empty_specs_dropped() {
        let decl = parse(
            json!({"ranges": [[1, 3], [3, 3], [4, 2], [-1, 5], [1.5, 4], ["1", 4], [1], [0, 2, 3], "x"]}),
        );
        assert_eq!(decl.specs()[0].ranges, vec![1..3]);

        assert!(parse(json!({"query": "", "ranges": [[3, 3], [-1, 2]]})).is_empty());
        assert!(parse(json!({})).is_empty());
        let mixed = parse(json!([{"query": ""}, {"query": "kept"}, 5, "x", null]));
        assert_eq!(mixed.specs().len(), 1);
        assert_eq!(mixed.specs()[0].query, "kept");
    }

    #[test]
    fn shapes_other_than_object_or_list_are_empty() {
        for value in [json!(null), json!(true), json!(3), json!("red")] {
            assert!(parse(value).is_empty());
        }
        assert!(parse(json!([])).is_empty());
    }

    #[test]
    fn malformed_match_index_offset_rejects_the_whole_specification() {
        for offset in [json!(-1), json!(1.5), json!("2"), json!(null), json!(true)] {
            let decl = parse(json!({"query": "a", "matchIndexOffset": offset}));
            assert!(decl.is_empty(), "{offset} must reject the specification");
        }
        // Only the offending specification of a list is rejected.
        let list = parse(
            json!([{"query": "bad", "matchIndexOffset": -1}, {"query": "good", "matchIndexOffset": 2}]),
        );
        assert_eq!(list.specs().len(), 1);
        assert_eq!(list.specs()[0].query, "good");
        assert_eq!(list.specs()[0].match_index_offset, 2);
        // An integral float is an integer.
        assert_eq!(
            parse(json!({"query": "a", "matchIndexOffset": 3.0})).specs()[0].match_index_offset,
            3
        );
        assert_eq!(
            parse(json!({"query": "a", "matchIndexOffset": 0}))
                .specs()
                .len(),
            1
        );
    }

    #[test]
    fn active_index_must_be_a_non_negative_integer_or_it_is_absent() {
        for active in [json!(-1), json!(1.5), json!("2"), json!(null)] {
            let decl = parse(json!({"query": "a", "activeIndex": active}));
            assert_eq!(decl.specs()[0].active_index, None);
        }
    }

    #[test]
    fn matcher_hash_covers_matcher_fields_only() {
        let base = HighlightSpec::new("red").with_ranges([2..5]);
        let hash = HighlightDeclaration::from_spec(base.clone()).matcher_hash();
        let same =
            |spec: HighlightSpec| HighlightDeclaration::from_spec(spec).matcher_hash() == hash;

        assert!(same(
            base.clone().with_color(default_hex_color("#fff").unwrap())
        ));
        assert!(same(
            base.clone()
                .with_active_color(default_hex_color("#fff").unwrap())
        ));
        assert!(same(base.clone().with_active_index(Some(3))));
        assert!(same(base.clone().with_match_index_offset(9)));
        assert!(same(base.clone().with_radius(8.0)));

        let mut other_query = base.clone();
        other_query.query = "blue".into();
        assert!(!same(other_query));
        assert!(!same(base.clone().with_case_sensitive(true)));
        assert!(!same(base.clone().with_whole_word(true)));
        assert!(!same(base.clone().with_ranges([7..8])));
        assert_ne!(
            HighlightDeclaration::from_specs([base.clone(), base]).matcher_hash(),
            hash
        );
    }

    #[test]
    fn builder_drops_invalid_ranges_and_empty_specs() {
        let spec = HighlightSpec::new("").with_ranges([Range { start: 4, end: 2 }, 3..3, 1..2]);
        assert_eq!(spec.ranges, vec![1..2]);
        let decl = HighlightDeclaration::new()
            .with_spec(HighlightSpec::default())
            .with_spec(spec);
        assert_eq!(decl.specs().len(), 1);
        assert!(decl.has_ranges());
        assert!(HighlightDeclaration::from_spec(HighlightSpec::default()).is_empty());
    }

    #[test]
    fn display_number_and_active_follow_the_offset() {
        let spec = HighlightSpec::new("a")
            .with_match_index_offset(10)
            .with_active_index(Some(11));
        assert_eq!(spec.display_number(1), 11);
        assert!(!spec.is_active(0));
        assert!(spec.is_active(1));
        assert!(!spec.with_active_index(None).is_active(1));
    }
}
