//! The match-count callback (TXT-009 R9).

use super::resolve::Resolution;

/// Reports `matchCount` when the match identity changes.
///
/// The identity covers the total, every run's ranges, the specification index and the ordinal;
/// it does not cover colors, radius or `activeIndex`, so moving the find cursor never fires. A
/// different query that yields the same count has different ranges and does fire. The initial
/// empty result does not fire (the reporter starts out as if it had already reported nothing).
pub struct CountReporter<F: FnMut(usize)> {
    last_identity: u64,
    callback: F,
}

impl<F: FnMut(usize)> CountReporter<F> {
    pub fn new(callback: F) -> Self {
        Self {
            last_identity: Resolution::empty().identity,
            callback,
        }
    }

    /// Compares `resolution` with the previous one and fires the callback with its total when it
    /// differs. Returns whether it fired.
    pub fn report(&mut self, resolution: &Resolution) -> bool {
        if resolution.identity == self.last_identity {
            return false;
        }
        self.last_identity = resolution.identity;
        (self.callback)(resolution.total);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::resolve::{GroupInput, Resolver};
    use crate::search::spec::{HighlightDeclaration, HighlightSpec, default_hex_color};
    use std::cell::RefCell;
    use std::rc::Rc;

    fn declaration(spec: HighlightSpec) -> HighlightDeclaration {
        HighlightDeclaration::from_spec(spec)
    }

    #[test]
    fn fires_only_when_the_identity_changes() {
        let counts = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&counts);
        let mut reporter = CountReporter::new(move |count| sink.borrow_mut().push(count));
        let mut resolver = Resolver::new();
        let groups = [GroupInput::new(1, ["red blue"])];
        let mut report = |reporter: &mut CountReporter<_>, spec: HighlightSpec| {
            reporter.report(&resolver.resolve(&declaration(spec), &groups))
        };

        // The initial empty result (no declaration, and an empty query) does not fire.
        assert!(!reporter.report(&Resolver::new().resolve(&HighlightDeclaration::new(), &groups)));
        assert!(!reporter.report(&Resolution::empty()));
        assert!(counts.borrow().is_empty());

        assert!(report(&mut reporter, HighlightSpec::new("red")));
        assert_eq!(*counts.borrow(), vec![1]);

        // activeIndex, colors, radius and offset changes do not fire.
        let color = default_hex_color("#F0C04066").unwrap();
        assert!(!report(
            &mut reporter,
            HighlightSpec::new("red").with_active_index(Some(0))
        ));
        assert!(!report(
            &mut reporter,
            HighlightSpec::new("red").with_active_index(Some(5))
        ));
        assert!(!report(
            &mut reporter,
            HighlightSpec::new("red").with_color(color)
        ));
        assert!(!report(
            &mut reporter,
            HighlightSpec::new("red").with_active_color(color)
        ));
        assert!(!report(
            &mut reporter,
            HighlightSpec::new("red").with_radius(9.0)
        ));
        assert!(!report(
            &mut reporter,
            HighlightSpec::new("red").with_match_index_offset(3)
        ));
        assert_eq!(*counts.borrow(), vec![1]);

        // A different query with the same count is a new result.
        assert!(report(&mut reporter, HighlightSpec::new("blue")));
        assert_eq!(*counts.borrow(), vec![1, 1]);

        // Back to nothing is a change too.
        assert!(report(&mut reporter, HighlightSpec::new("zzz")));
        assert_eq!(*counts.borrow(), vec![1, 1, 0]);
        assert!(!report(&mut reporter, HighlightSpec::new("yyy")));
    }

    #[test]
    fn changing_group_text_so_the_matches_move_fires() {
        let counts = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&counts);
        let mut reporter = CountReporter::new(move |count| sink.borrow_mut().push(count));
        let mut resolver = Resolver::new();
        let decl = declaration(HighlightSpec::new("a"));
        reporter.report(&resolver.resolve(&decl, &[GroupInput::new(1, ["a"])]));
        reporter.report(&resolver.resolve(&decl, &[GroupInput::new(1, ["xa"])]));
        assert_eq!(*counts.borrow(), vec![1, 1]);
    }
}
