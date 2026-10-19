use crate::{TextAlign, TextStyle, TextStyleRefinement};
use std::{cell::RefCell, sync::Arc};
use vespertine_refineable::Refineable as _;

/// The stack of text-style refinements active while elements draw.
///
/// Resolved immutable styles are shared by depth: pushing a refinement invalidates the
/// resolution at the new depth, popping discards every deeper resolution, and a query starts from
/// the deepest depth that is already resolved and applies only the refinements above it.
#[derive(Clone, Default)]
pub(crate) struct TextStyleStack {
    refinements: Vec<TextStyleRefinement>,
    /// `resolved[i]` is the style after applying `refinements[..=i]`.
    resolved: RefCell<Vec<Option<Arc<TextStyle>>>>,
}

impl TextStyleStack {
    pub(crate) fn push(&mut self, refinement: TextStyleRefinement) {
        self.resolved.get_mut().push(None);
        self.refinements.push(refinement);
    }

    pub(crate) fn pop(&mut self) {
        self.resolved.get_mut().pop();
        self.refinements.pop();
    }

    pub(crate) fn clear(&mut self) {
        self.refinements.clear();
        self.resolved.get_mut().clear();
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.refinements.len()
    }

    /// The style composed of every refinement, shared by depth.
    pub(crate) fn resolve_shared(&self) -> Arc<TextStyle> {
        let mut resolved = self.resolved.borrow_mut();
        let deepest = resolved.iter().rposition(Option::is_some);
        let (mut style, first_unresolved) = match deepest {
            Some(depth) => (
                resolved[depth]
                    .clone()
                    .expect("position found a resolved entry"),
                depth + 1,
            ),
            None => (Arc::new(TextStyle::default()), 0),
        };
        for depth in first_unresolved..self.refinements.len() {
            Arc::make_mut(&mut style).refine(&self.refinements[depth]);
            resolved[depth] = Some(style.clone());
        }
        style
    }

    /// The style composed of every refinement.
    pub(crate) fn resolve(&self) -> TextStyle {
        (*self.resolve_shared()).clone()
    }

    /// The text alignment alone: the last refinement that sets it, without resolving the style.
    pub(crate) fn text_align(&self) -> TextAlign {
        self.refinements
            .iter()
            .filter_map(|refinement| refinement.text_align)
            .next_back()
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FontWeight, Hsla, hsla, px};

    fn refinements() -> Vec<TextStyleRefinement> {
        vec![
            TextStyleRefinement {
                font_weight: Some(FontWeight::BOLD),
                ..Default::default()
            },
            TextStyleRefinement {
                color: Some(hsla(0.5, 0.5, 0.5, 1.)),
                text_align: Some(TextAlign::Center),
                ..Default::default()
            },
            TextStyleRefinement {
                font_family: Some("Other".into()),
                ..Default::default()
            },
            TextStyleRefinement {
                text_align: Some(TextAlign::Right),
                font_size: Some(px(20.).into()),
                ..Default::default()
            },
            TextStyleRefinement {
                color: Some(Hsla::black()),
                ..Default::default()
            },
        ]
    }

    fn uncached(refinements: &[TextStyleRefinement]) -> (TextStyle, TextAlign) {
        let mut style = TextStyle::default();
        for refinement in refinements {
            style.refine(refinement);
        }
        let align = style.text_align;
        (style, align)
    }

    #[test]
    fn style_stack_matches_full_resolution_after_every_push_and_pop() {
        let all = refinements();
        let mut stack = TextStyleStack::default();
        let mut mirror: Vec<TextStyleRefinement> = Vec::new();

        let check = |stack: &TextStyleStack, mirror: &[TextStyleRefinement]| {
            let (expected, expected_align) = uncached(mirror);
            assert_eq!(stack.resolve(), expected);
            assert_eq!(stack.text_align(), expected_align);
            // Repeating the query (served from shared resolutions) gives the same answer.
            assert_eq!(*stack.resolve_shared(), expected);
        };

        check(&stack, &mirror);
        for refinement in &all {
            stack.push(refinement.clone());
            mirror.push(refinement.clone());
            check(&stack, &mirror);
        }
        // Pop two, push a different refinement, query at each step.
        for _ in 0..2 {
            stack.pop();
            mirror.pop();
            check(&stack, &mirror);
        }
        let replacement = TextStyleRefinement {
            font_style: Some(crate::FontStyle::Italic),
            text_align: Some(TextAlign::Left),
            ..Default::default()
        };
        stack.push(replacement.clone());
        mirror.push(replacement);
        check(&stack, &mirror);
        while stack.len() > 0 {
            stack.pop();
            mirror.pop();
            check(&stack, &mirror);
        }
    }

    #[test]
    fn deeper_resolutions_are_discarded_when_popping() {
        let all = refinements();
        let mut stack = TextStyleStack::default();
        for refinement in &all[..3] {
            stack.push(refinement.clone());
        }
        let _ = stack.resolve_shared();
        assert!(stack.resolved.borrow().iter().all(Option::is_some));
        stack.pop();
        assert_eq!(stack.resolved.borrow().len(), 2);
        stack.push(all[3].clone());
        assert!(stack.resolved.borrow()[2].is_none());
        assert!(stack.resolved.borrow()[1].is_some());
    }
}
