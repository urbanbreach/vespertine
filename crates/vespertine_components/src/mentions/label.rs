//! Chip labels: disambiguation and grapheme compaction (CMP-010 R3).

use super::reference::{ReferenceDescriptor, ReferenceKind};
use std::collections::{BTreeMap, BTreeSet};
use unicode_segmentation::UnicodeSegmentation;

/// A label of at most this many graphemes is shown unchanged.
pub const COMPACT_LIMIT: usize = 32;

/// Separator between the name and the disambiguating suffix of commands and skills.
pub const PREFIX_SEPARATOR: &str = " · ";

/// Compacts `label` by grapheme count: at most `limit` graphemes stay unchanged; a longer label
/// becomes its first `limit * 9 / 16` graphemes, `…`, and its last `limit * 3 / 8` graphemes
/// (18 and 12 for the default limit of 32). Emoji sequences and combining marks are never split.
pub fn compact_label(label: &str, limit: usize) -> String {
    let graphemes: Vec<&str> = label.graphemes(true).collect();
    if graphemes.len() <= limit {
        return label.to_owned();
    }
    let head = limit * 9 / 16;
    let tail = limit * 3 / 8;
    let mut out = String::new();
    out.extend(graphemes[..head].iter().copied());
    out.push('…');
    out.extend(graphemes[graphemes.len() - tail..].iter().copied());
    out
}

fn components(identity: &str) -> Vec<&str> {
    identity
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect()
}

/// The shortest trailing path-component suffix of `identity` that no identity of `others` ends
/// with (component-wise), or the full identity when there is none.
fn unique_suffix(identity: &str, others: &[&str]) -> String {
    let own = components(identity);
    let others: Vec<Vec<&str>> = others.iter().map(|other| components(other)).collect();
    for count in 1..=own.len() {
        let suffix = &own[own.len() - count..];
        let shared = others.iter().any(|other| other.ends_with(suffix));
        if !shared {
            return suffix.join("/");
        }
    }
    identity.to_owned()
}

/// The labels of `descriptors` before compaction: the name, or, where several references of the
/// same kind share a name but differ in identity, the unique identity suffix (`name · suffix` for
/// kinds that show a prefix). Opaque-identity kinds always show their name.
pub fn disambiguated_labels<'a>(
    descriptors: impl IntoIterator<Item = &'a ReferenceDescriptor>,
) -> Vec<String> {
    let items: Vec<&ReferenceDescriptor> = descriptors.into_iter().collect();
    let mut by_group: Vec<(&ReferenceKind, BTreeSet<&str>)> = Vec::new();
    for item in &items {
        if let Some((_, set)) = by_group.iter_mut().find(|(kind, _)| *kind == &item.kind) {
            set.insert(item.identity.as_str());
        } else {
            by_group.push((&item.kind, BTreeSet::from([item.identity.as_str()])));
        }
    }
    items
        .iter()
        .map(|item| {
            if item.kind.has_opaque_identity() {
                return item.name.clone();
            }
            let ambiguous = items.iter().any(|other| {
                other.kind == item.kind
                    && other.name == item.name
                    && other.identity != item.identity
            });
            if !ambiguous {
                return item.name.clone();
            }
            let group = by_group
                .iter()
                .find(|(kind, _)| *kind == &item.kind)
                .map(|(_, set)| set)
                .expect("group registered for every item");
            let others: Vec<&str> = group
                .iter()
                .copied()
                .filter(|identity| *identity != item.identity.as_str())
                .collect();
            let suffix = unique_suffix(&item.identity, &others);
            if item.kind.shows_prefix() {
                format!("{}{PREFIX_SEPARATOR}{suffix}", item.name)
            } else {
                suffix
            }
        })
        .collect()
}

/// Compacts `labels` (R3): each at the default limit; while two labels are equal after
/// compaction although their full labels differ, the limit of the ambiguous ones is doubled and
/// the labels are recomputed.
pub fn compact_labels(labels: &[String]) -> Vec<String> {
    let mut limits: BTreeMap<&str, usize> = labels
        .iter()
        .map(|label| (label.as_str(), COMPACT_LIMIT))
        .collect();
    loop {
        let mut by_compacted: BTreeMap<String, Vec<&str>> = BTreeMap::new();
        for (&full, &limit) in &limits {
            by_compacted
                .entry(compact_label(full, limit))
                .or_default()
                .push(full);
        }
        let mut changed = false;
        for fulls in by_compacted.values().filter(|fulls| fulls.len() > 1) {
            for full in fulls {
                if let Some(limit) = limits.get_mut(full) {
                    *limit *= 2;
                    changed = true;
                }
            }
        }
        if !changed {
            return labels
                .iter()
                .map(|label| compact_label(label, limits[label.as_str()]))
                .collect();
        }
    }
}

/// The final chip labels of `descriptors`, in input order: disambiguated, then compacted.
pub fn reference_labels<'a>(
    descriptors: impl IntoIterator<Item = &'a ReferenceDescriptor>,
) -> Vec<String> {
    compact_labels(&disambiguated_labels(descriptors))
}

#[cfg(test)]
mod tests {
    use super::super::reference::test_support::descriptor;
    use super::*;

    fn file(identity: &str) -> ReferenceDescriptor {
        let name = identity.rsplit('/').next().unwrap();
        descriptor(0..1, ReferenceKind::File, name, identity, None)
    }

    #[test]
    fn same_base_name_uses_shortest_unique_suffix() {
        let items = [file("a/x.rs"), file("b/x.rs")];
        assert_eq!(reference_labels(&items), vec!["a/x.rs", "b/x.rs"]);
    }

    #[test]
    fn unique_base_name_stays() {
        let items = [file("a/x.rs"), file("b/x.rs"), file("src/y.rs")];
        assert_eq!(reference_labels(&items), vec!["a/x.rs", "b/x.rs", "y.rs"]);
    }

    #[test]
    fn suffix_grows_component_wise() {
        let items = [
            file("p/a/lib/x.rs"),
            file("p/b/lib/x.rs"),
            file("q/a/lib/x.rs"),
        ];
        assert_eq!(
            reference_labels(&items),
            vec!["p/a/lib/x.rs", "b/lib/x.rs", "q/a/lib/x.rs"]
        );
        // Component-wise, not character-wise: "ax.rs" does not end with "x.rs" as a component.
        let items = [file("d/x.rs"), file("e/ax.rs")];
        assert_eq!(reference_labels(&items), vec!["x.rs", "ax.rs"]);
    }

    #[test]
    fn nested_identity_falls_back_to_full_identity() {
        let items = [file("x.rs"), file("a/x.rs")];
        assert_eq!(reference_labels(&items), vec!["x.rs", "a/x.rs"]);
    }

    #[test]
    fn identical_references_share_a_label() {
        let items = [file("a/x.rs"), file("a/x.rs")];
        assert_eq!(reference_labels(&items), vec!["x.rs", "x.rs"]);
    }

    #[test]
    fn kinds_are_separate_groups() {
        let dir = descriptor(0..1, ReferenceKind::Directory, "x.rs", "b/x.rs", None);
        let items = [file("a/x.rs"), dir];
        assert_eq!(reference_labels(&items), vec!["x.rs", "x.rs"]);
    }

    #[test]
    fn prefix_kinds_show_name_and_suffix() {
        let a = descriptor(0..1, ReferenceKind::Command, "review", "one/review", None);
        let b = descriptor(0..1, ReferenceKind::Command, "review", "two/review", None);
        let c = descriptor(0..1, ReferenceKind::Skill, "review", "one/review", None);
        assert_eq!(
            reference_labels([&a, &b, &c]),
            vec!["review · one/review", "review · two/review", "review"]
        );
    }

    #[test]
    fn opaque_identity_kinds_always_show_their_title() {
        let a = descriptor(0..1, ReferenceKind::Conversation, "Plan", "id-1", None);
        let b = descriptor(0..1, ReferenceKind::Conversation, "Plan", "id-2", None);
        assert_eq!(reference_labels([&a, &b]), vec!["Plan", "Plan"]);
    }

    #[test]
    fn short_labels_are_unchanged_and_long_ones_compacted() {
        let at_limit = "a".repeat(32);
        assert_eq!(compact_label(&at_limit, COMPACT_LIMIT), at_limit);
        let long: String = (0..40).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
        let compact = compact_label(&long, COMPACT_LIMIT);
        let expected = format!("{}…{}", &long[..18], &long[long.len() - 12..]);
        assert_eq!(compact, expected);
        assert_eq!(compact.graphemes(true).count(), 31);
    }

    #[test]
    fn compaction_never_splits_graphemes() {
        // 40 family emoji: each is one grapheme made of several scalars.
        let family = "👩‍👩‍👧";
        let label = family.repeat(40);
        let compact = compact_label(&label, COMPACT_LIMIT);
        assert_eq!(
            compact,
            format!("{}…{}", family.repeat(18), family.repeat(12))
        );
        // A combining accent stays with its base letter at the cut points.
        let accented = "e\u{301}".repeat(40);
        let compact = compact_label(&accented, COMPACT_LIMIT);
        assert_eq!(
            compact,
            format!("{}…{}", "e\u{301}".repeat(18), "e\u{301}".repeat(12))
        );
    }

    #[test]
    fn labels_equal_after_compaction_are_lengthened_until_distinct() {
        let head = "h".repeat(20);
        let tail = "t".repeat(20);
        let a = format!("{head}AAAA{tail}");
        let b = format!("{head}BBBB{tail}");
        let items = [
            descriptor(0..1, ReferenceKind::Other("x".into()), &a, "1", None),
            descriptor(0..1, ReferenceKind::Other("x".into()), &b, "2", None),
        ];
        // 44 graphemes: 18 + … + 12 hides the difference, so the limit doubles to 64 (full).
        let labels = reference_labels(&items);
        assert_eq!(labels, vec![a, b]);

        // 80 graphemes differing in the middle: limit 64 (36 + … + 24) still hides it, 128 shows.
        let head = "h".repeat(38);
        let tail = "t".repeat(38);
        let a = format!("{head}AAAA{tail}");
        let b = format!("{head}BBBB{tail}");
        let items = [
            descriptor(0..1, ReferenceKind::Other("x".into()), &a, "1", None),
            descriptor(0..1, ReferenceKind::Other("x".into()), &b, "2", None),
        ];
        let labels = reference_labels(&items);
        assert_eq!(labels, vec![a, b]);
    }

    #[test]
    fn only_ambiguous_labels_have_their_limit_doubled() {
        let head = "h".repeat(20);
        let tail = "t".repeat(20);
        let a = format!("{head}AAAA{tail}");
        let b = format!("{head}BBBB{tail}");
        let unrelated = "z".repeat(40);
        let items = [
            descriptor(0..1, ReferenceKind::Other("x".into()), &a, "1", None),
            descriptor(0..1, ReferenceKind::Other("x".into()), &b, "2", None),
            descriptor(
                0..1,
                ReferenceKind::Other("x".into()),
                &unrelated,
                "3",
                None,
            ),
        ];
        let labels = reference_labels(&items);
        assert_eq!(labels[0], a);
        assert_eq!(labels[1], b);
        assert_eq!(labels[2], compact_label(&unrelated, COMPACT_LIMIT));
    }

    #[test]
    fn disambiguation_runs_before_compaction() {
        let long_dir = "d".repeat(40);
        let items = [file(&format!("{long_dir}/x.rs")), file("other/x.rs")];
        let labels = reference_labels(&items);
        assert_eq!(labels[0], compact_label(&format!("{long_dir}/x.rs"), 32));
        assert_eq!(labels[1], "other/x.rs");
    }
}
