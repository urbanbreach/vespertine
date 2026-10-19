//! Word-level diff following the behavior of the third-party jsdiff library
//! (<https://github.com/kpdecker/jsdiff>, BSD-3-Clause, Copyright (c) 2009-2015, Kevin Decker),
//! `diffWords` (CMP-015 R31).
//!
//! Tokens are runs of word characters, runs of punctuation, runs of whitespace and the single
//! characters `( ) [ ] { } ' "`; whitespace tokens equal each other (whitespace is never a
//! changed token by itself). The tokens are compared with the same Myers shortest-edit-script
//! walk that jsdiff uses (including its diagonal bounds and tie-breaking), and adjacent
//! components of one kind are joined. Unchanged text between changes is kept as common parts,
//! taken from the new string like jsdiff does.

/// What a part of a word diff is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartKind {
    /// Text present in both strings.
    Common,
    /// Text only in the new string.
    Added,
    /// Text only in the old string.
    Removed,
}

/// One part of a word diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    /// The text of the part.
    pub text: String,
    /// Common, added or removed.
    pub kind: PartKind,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_single(c: char) -> bool {
    matches!(
        c,
        '(' | ')' | '[' | ']' | '{' | '}' | '\'' | '"' | '\r' | '\n'
    )
}

fn is_blank(c: char) -> bool {
    c.is_whitespace() && c != '\r' && c != '\n'
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Class {
    Word,
    Blank,
    Punct,
}

/// Splits a string into the tokens of `diffWords`.
pub fn tokenize(text: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = 0;
    let mut class: Option<Class> = None;
    for (index, c) in text.char_indices() {
        let next = if is_single(c) {
            None
        } else if is_blank(c) {
            Some(Class::Blank)
        } else if is_word(c) {
            Some(Class::Word)
        } else {
            Some(Class::Punct)
        };
        let continues = next.is_some() && next == class;
        if !continues {
            if index > start {
                tokens.push(&text[start..index]);
            }
            start = index;
        }
        class = next;
        if next.is_none() {
            tokens.push(&text[start..index + c.len_utf8()]);
            start = index + c.len_utf8();
        }
    }
    if start < text.len() {
        tokens.push(&text[start..]);
    }
    tokens
}

fn is_whitespace_token(token: &str) -> bool {
    !token.is_empty() && token.chars().all(char::is_whitespace)
}

fn equal(a: &str, b: &str) -> bool {
    a == b || (is_whitespace_token(a) && is_whitespace_token(b))
}

#[derive(Clone)]
struct Component {
    count: usize,
    kind: PartKind,
}

#[derive(Clone)]
struct Path {
    old_pos: isize,
    components: Vec<Component>,
}

fn push_component(path: &mut Path, kind: PartKind, count: usize) {
    match path.components.last_mut() {
        Some(last) if last.kind == kind => last.count += count,
        _ => path.components.push(Component { count, kind }),
    }
}

fn extract_common(path: &mut Path, new: &[&str], old: &[&str], diagonal: isize) -> isize {
    let new_len = new.len() as isize;
    let old_len = old.len() as isize;
    let mut old_pos = path.old_pos;
    let mut new_pos = old_pos - diagonal;
    let mut common = 0;
    while new_pos + 1 < new_len
        && old_pos + 1 < old_len
        && equal(new[(new_pos + 1) as usize], old[(old_pos + 1) as usize])
    {
        new_pos += 1;
        old_pos += 1;
        common += 1;
    }
    if common > 0 {
        push_component(path, PartKind::Common, common);
    }
    path.old_pos = old_pos;
    new_pos
}

/// The components (kind and token count) of the shortest edit script, in jsdiff's order.
fn components(old: &[&str], new: &[&str]) -> Vec<Component> {
    let new_len = new.len() as isize;
    let old_len = old.len() as isize;
    let mut best: Vec<Option<Path>> = vec![None; (new_len + old_len + 3) as usize * 2 + 3];
    let offset = new_len + old_len + 2;
    let slot = |diagonal: isize| (diagonal + offset) as usize;
    let mut seed = Path {
        old_pos: -1,
        components: Vec::new(),
    };
    let new_pos = extract_common(&mut seed, new, old, 0);
    if seed.old_pos + 1 >= old_len && new_pos + 1 >= new_len {
        return seed.components;
    }
    best[slot(0)] = Some(seed);
    let mut min_diagonal = isize::MIN;
    let mut max_diagonal = isize::MAX;
    let mut edit_length: isize = 1;
    loop {
        let from = min_diagonal.max(-edit_length);
        let to = max_diagonal.min(edit_length);
        let mut diagonal = from;
        while diagonal <= to {
            let remove_path = best[slot(diagonal - 1)].take();
            let add_path = best[slot(diagonal + 1)].clone();
            let can_add = add_path.as_ref().is_some_and(|path| {
                let add_new_pos = path.old_pos - diagonal;
                0 <= add_new_pos && add_new_pos < new_len
            });
            let can_remove = remove_path
                .as_ref()
                .is_some_and(|path| path.old_pos + 1 < old_len);
            if !can_add && !can_remove {
                best[slot(diagonal)] = None;
                diagonal += 2;
                continue;
            }
            let mut base = if !can_remove
                || (can_add
                    && remove_path.as_ref().map(|p| p.old_pos)
                        < add_path.as_ref().map(|p| p.old_pos))
            {
                let mut path = add_path.expect("can_add");
                push_component(&mut path, PartKind::Added, 1);
                path
            } else {
                let mut path = remove_path.expect("can_remove");
                path.old_pos += 1;
                push_component(&mut path, PartKind::Removed, 1);
                path
            };
            let new_pos = extract_common(&mut base, new, old, diagonal);
            if base.old_pos + 1 >= old_len && new_pos + 1 >= new_len {
                return base.components;
            }
            if base.old_pos + 1 >= old_len {
                max_diagonal = max_diagonal.min(diagonal - 1);
            }
            if new_pos + 1 >= new_len {
                min_diagonal = min_diagonal.max(diagonal + 1);
            }
            best[slot(diagonal)] = Some(base);
            diagonal += 2;
        }
        edit_length += 1;
        if edit_length > new_len + old_len + 1 {
            // Cannot happen for a finite edit graph; stop rather than loop forever.
            return Vec::new();
        }
    }
}

/// Diffs two strings word by word, like jsdiff's `diffWords`. Parts alternate in jsdiff's output
/// order (removals before the additions that replace them); text in common parts is taken from
/// the new string.
pub fn diff_words(old: &str, new: &str) -> Vec<Part> {
    let old_tokens = tokenize(old);
    let new_tokens = tokenize(new);
    let mut parts: Vec<Part> = Vec::new();
    let mut comps = components(&old_tokens, &new_tokens);
    // Removals are output before additions (jsdiff swaps an add that precedes a remove).
    let mut index = 1;
    while index < comps.len() {
        if comps[index - 1].kind == PartKind::Added && comps[index].kind == PartKind::Removed {
            comps.swap(index - 1, index);
        }
        index += 1;
    }
    // Positions advance per kind, so reordering adjacent add/remove components never changes
    // which tokens a component covers.
    let mut cursor_old = 0usize;
    let mut cursor_new = 0usize;
    for component in &comps {
        let text = match component.kind {
            PartKind::Removed => {
                let text: String = old_tokens[cursor_old..cursor_old + component.count].concat();
                cursor_old += component.count;
                text
            }
            PartKind::Added => {
                let text: String = new_tokens[cursor_new..cursor_new + component.count].concat();
                cursor_new += component.count;
                text
            }
            PartKind::Common => {
                let text: String = new_tokens[cursor_new..cursor_new + component.count].concat();
                cursor_new += component.count;
                cursor_old += component.count;
                text
            }
        };
        match parts.last_mut() {
            Some(last) if last.kind == component.kind => last.text.push_str(&text),
            _ => parts.push(Part {
                text,
                kind: component.kind,
            }),
        }
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(parts: &[Part]) -> Vec<(&str, PartKind)> {
        parts.iter().map(|p| (p.text.as_str(), p.kind)).collect()
    }

    #[test]
    fn tokens_split_words_punctuation_whitespace_and_brackets() {
        assert_eq!(tokenize("same old"), ["same", " ", "old"]);
        assert_eq!(
            tokenize("foo(bar, baz)"),
            ["foo", "(", "bar", ",", " ", "baz", ")"]
        );
        assert_eq!(tokenize("a->b == c"), ["a", "->", "b", " ", "==", " ", "c"]);
        assert_eq!(tokenize("x_1  y"), ["x_1", "  ", "y"]);
        assert_eq!(tokenize("[\"a\"]"), ["[", "\"", "a", "\"", "]"]);
        assert_eq!(tokenize(""), Vec::<&str>::new());
        assert_eq!(tokenize("it's"), ["it", "'", "s"]);
    }

    #[test]
    fn changed_word_is_a_removed_and_an_added_part_around_common_text() {
        let parts = diff_words("same old", "same new");
        assert_eq!(
            kinds(&parts),
            [
                ("same ", PartKind::Common),
                ("old", PartKind::Removed),
                ("new", PartKind::Added),
            ]
        );
    }

    #[test]
    fn identical_strings_are_one_common_part() {
        assert_eq!(
            kinds(&diff_words("a b", "a b")),
            [("a b", PartKind::Common)]
        );
        assert!(diff_words("", "").is_empty());
    }

    #[test]
    fn whitespace_is_never_a_changed_token_by_itself() {
        let parts = diff_words("a  b", "a b");
        assert_eq!(kinds(&parts), [("a b", PartKind::Common)]);
        let parts = diff_words("a b", "a\tb");
        assert!(parts.iter().all(|p| p.kind == PartKind::Common));
    }

    #[test]
    fn insertions_and_deletions() {
        let parts = diff_words("a c", "a b c");
        assert_eq!(
            kinds(&parts),
            [
                ("a ", PartKind::Common),
                ("b ", PartKind::Added),
                ("c", PartKind::Common),
            ]
        );
        let parts = diff_words("a b c", "a c");
        assert_eq!(
            kinds(&parts),
            [
                ("a ", PartKind::Common),
                ("b ", PartKind::Removed),
                ("c", PartKind::Common),
            ]
        );
    }

    #[test]
    fn punctuation_runs_and_words_are_separate_tokens() {
        let parts = diff_words("f(x) -> y", "f(z) => y");
        let removed: Vec<_> = parts
            .iter()
            .filter(|p| p.kind == PartKind::Removed)
            .map(|p| p.text.as_str())
            .collect();
        let added: Vec<_> = parts
            .iter()
            .filter(|p| p.kind == PartKind::Added)
            .map(|p| p.text.as_str())
            .collect();
        assert_eq!(removed, ["x", "->"]);
        assert_eq!(added, ["z", "=>"]);
    }

    #[test]
    fn completely_different_strings_still_terminate() {
        let parts = diff_words("alpha beta", "gamma delta epsilon");
        assert!(parts.iter().any(|p| p.kind == PartKind::Removed));
        assert!(parts.iter().any(|p| p.kind == PartKind::Added));
        let old_text: String = parts
            .iter()
            .filter(|p| p.kind != PartKind::Added)
            .map(|p| p.text.as_str())
            .collect();
        assert_eq!(old_text.split_whitespace().count(), 2);
    }

    #[test]
    fn rebuilding_both_sides_from_the_parts_reproduces_the_inputs_up_to_whitespace() {
        for (old, new) in [
            ("let x = 1;", "let y = 2;"),
            ("one two three", "one three four"),
            ("", "added"),
            ("removed", ""),
        ] {
            let parts = diff_words(old, new);
            let rebuilt_new: String = parts
                .iter()
                .filter(|p| p.kind != PartKind::Removed)
                .map(|p| p.text.as_str())
                .collect();
            assert_eq!(rebuilt_new, new);
            let rebuilt_old: String = parts
                .iter()
                .filter(|p| p.kind != PartKind::Added)
                .map(|p| p.text.as_str())
                .collect();
            assert_eq!(rebuilt_old.replace(' ', ""), old.replace(' ', ""));
        }
    }
}
