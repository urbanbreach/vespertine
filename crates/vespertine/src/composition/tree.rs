//! The surface tree of a composed window (PLT-001 R3, R4).

use std::collections::HashMap;

/// The stable, opaque identity of a composition surface. Identities are never reused within
/// a window, so a stale identity is always detected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SurfaceId(u64);

impl SurfaceId {
    /// A surface identity that no tree ever hands out, for tests and placeholders.
    #[cfg(any(test, feature = "test-support"))]
    pub const fn for_test(value: u64) -> Self {
        SurfaceId(value)
    }
}

/// What produces the pixels of a composition surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SurfaceKind {
    /// Rendered by the framework: the base scene, the overlay and any additional
    /// framework-rendered surface. Only these can be the paint target of
    /// [`Window::with_composition_surface`](crate::Window::with_composition_surface).
    Framework,
    /// Presented independently by a native producer (a web view, a video surface, another
    /// toolkit's widget) through a platform attachment.
    Native,
    /// Presented independently by an external GPU producer that renders with the window's
    /// device. It is composed exactly like a native surface.
    ExternalGpu,
}

impl SurfaceKind {
    /// Whether the platform provides an attachment (handle, geometry, visibility) for
    /// surfaces of this kind.
    pub fn is_attachment(self) -> bool {
        !matches!(self, SurfaceKind::Framework)
    }
}

/// Errors of the composition API.
#[derive(Debug, thiserror::Error)]
pub enum CompositionError {
    /// Composition has not been enabled on the window.
    #[error("window composition is not enabled")]
    NotEnabled,
    /// The identity does not name a live surface.
    #[error("surface {0:?} does not exist (stale identity)")]
    Stale(SurfaceId),
    /// A surface cannot be ordered relative to itself.
    #[error("surface {0:?} cannot be ordered relative to itself")]
    SelfOrdering(SurfaceId),
    /// The new parent is the surface itself or one of its descendants.
    #[error("moving surface {0:?} under {1:?} would create a cycle")]
    Cycle(SurfaceId, SurfaceId),
    /// Relative ordering needs both surfaces under the same parent.
    #[error("surfaces {0:?} and {1:?} have different parents")]
    DifferentParents(SurfaceId, SurfaceId),
    /// The base and overlay surfaces cannot be removed or reparented.
    #[error("surface {0:?} is a default surface and cannot be removed or reparented")]
    DefaultSurface(SurfaceId),
    /// The operation needs a surface of another kind.
    #[error("surface {0:?} is a {1:?} surface, which does not support this operation")]
    WrongKind(SurfaceId, SurfaceKind),
    /// The platform cannot do what was asked.
    #[error("{0}")]
    Unsupported(String),
    /// The platform failed to apply the change; the surface tree was restored.
    #[error("platform composition failed: {0:#}")]
    Platform(anyhow::Error),
}

#[derive(Clone, Debug)]
struct Node {
    kind: SurfaceKind,
    parent: Option<SurfaceId>,
    /// Children, bottom to top.
    children: Vec<SurfaceId>,
}

/// One surface in the flattened tree handed to the platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceEntry {
    /// The surface.
    pub id: SurfaceId,
    /// What produces its pixels.
    pub kind: SurfaceKind,
    /// Its parent surface, `None` for root-level surfaces.
    pub parent: Option<SurfaceId>,
}

/// The surface tree flattened to stacking order, bottom to top. A parent precedes its
/// children, and the children of a surface stack above it in sibling order.
///
/// Root-level surfaces listed before [`CompositionLayout::base`] stack below the window's own
/// surface and root-level surfaces after it stack above it; a surface whose parent is the
/// base is a child of the window surface (PLT-001 R4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompositionLayout {
    /// The base surface (the window's own content).
    pub base: SurfaceId,
    /// The overlay surface (deferred framework content).
    pub overlay: SurfaceId,
    /// Every surface, bottom to top.
    pub entries: Vec<SurfaceEntry>,
}

impl CompositionLayout {
    /// The entry of a surface.
    pub fn entry(&self, id: SurfaceId) -> Option<&SurfaceEntry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    /// The position of a surface in stacking order, bottom first.
    pub fn position(&self, id: SurfaceId) -> Option<usize> {
        self.entries.iter().position(|entry| entry.id == id)
    }
}

/// Surfaces with explicit parents and sibling order. The base and overlay surfaces exist from
/// the start and cannot be removed.
#[derive(Clone, Debug)]
pub struct SurfaceTree {
    nodes: HashMap<SurfaceId, Node>,
    roots: Vec<SurfaceId>,
    next_id: u64,
    base: SurfaceId,
    overlay: SurfaceId,
}

impl Default for SurfaceTree {
    fn default() -> Self {
        Self::new()
    }
}

impl SurfaceTree {
    /// A tree holding only the base and overlay surfaces, in that order at the root.
    pub fn new() -> Self {
        let base = SurfaceId(0);
        let overlay = SurfaceId(1);
        let node = |kind| Node {
            kind,
            parent: None,
            children: Vec::new(),
        };
        let mut nodes = HashMap::new();
        nodes.insert(base, node(SurfaceKind::Framework));
        nodes.insert(overlay, node(SurfaceKind::Framework));
        SurfaceTree {
            nodes,
            roots: vec![base, overlay],
            next_id: 2,
            base,
            overlay,
        }
    }

    /// The base surface.
    pub fn base(&self) -> SurfaceId {
        self.base
    }

    /// The overlay surface.
    pub fn overlay(&self) -> SurfaceId {
        self.overlay
    }

    /// Whether the identity names a live surface.
    pub fn contains(&self, id: SurfaceId) -> bool {
        self.nodes.contains_key(&id)
    }

    fn node(&self, id: SurfaceId) -> Result<&Node, CompositionError> {
        self.nodes.get(&id).ok_or(CompositionError::Stale(id))
    }

    /// The kind of a surface.
    pub fn kind(&self, id: SurfaceId) -> Result<SurfaceKind, CompositionError> {
        Ok(self.node(id)?.kind)
    }

    /// The parent of a surface, `None` for root-level surfaces.
    pub fn parent(&self, id: SurfaceId) -> Result<Option<SurfaceId>, CompositionError> {
        Ok(self.node(id)?.parent)
    }

    /// The children of a surface, bottom to top.
    pub fn children(&self, id: SurfaceId) -> Result<&[SurfaceId], CompositionError> {
        Ok(&self.node(id)?.children)
    }

    /// The root-level surfaces, bottom to top.
    pub fn roots(&self) -> &[SurfaceId] {
        &self.roots
    }

    fn siblings_mut(&mut self, parent: Option<SurfaceId>) -> &mut Vec<SurfaceId> {
        match parent {
            None => &mut self.roots,
            Some(parent) => {
                &mut self
                    .nodes
                    .get_mut(&parent)
                    .expect("a live parent has a node")
                    .children
            }
        }
    }

    /// Adds a surface of `kind` on top of the root-level surfaces that stack below the
    /// overlay, that is directly below the overlay surface: the default position of native
    /// children between the base scene and the overlay.
    pub fn create_below_overlay(&mut self, kind: SurfaceKind) -> SurfaceId {
        let id = SurfaceId(self.next_id);
        self.next_id += 1;
        self.nodes.insert(
            id,
            Node {
                kind,
                parent: None,
                children: Vec::new(),
            },
        );
        let at = self
            .roots
            .iter()
            .position(|root| *root == self.overlay)
            .unwrap_or(self.roots.len());
        self.roots.insert(at, id);
        id
    }

    /// Whether `id` is `ancestor` or one of its descendants.
    fn is_self_or_descendant(&self, id: SurfaceId, ancestor: SurfaceId) -> bool {
        let mut current = Some(id);
        while let Some(node) = current {
            if node == ancestor {
                return true;
            }
            current = self.nodes.get(&node).and_then(|node| node.parent);
        }
        false
    }

    /// Places `id` directly above (`above`) or below the sibling `reference`. Both must exist,
    /// differ and share a parent.
    pub fn place_relative(
        &mut self,
        id: SurfaceId,
        reference: SurfaceId,
        above: bool,
    ) -> Result<(), CompositionError> {
        let parent = self.node(id)?.parent;
        let reference_parent = self.node(reference)?.parent;
        if id == reference {
            return Err(CompositionError::SelfOrdering(id));
        }
        if parent != reference_parent {
            return Err(CompositionError::DifferentParents(id, reference));
        }
        let siblings = self.siblings_mut(parent);
        siblings.retain(|sibling| *sibling != id);
        let at = siblings
            .iter()
            .position(|sibling| *sibling == reference)
            .expect("a live surface is listed under its parent");
        siblings.insert(if above { at + 1 } else { at }, id);
        Ok(())
    }

    /// Moves `id` under `new_parent` (`None` for root level), on top of its new siblings.
    /// The base and overlay surfaces cannot be reparented, and a surface cannot move under
    /// itself or one of its descendants.
    pub fn reparent(
        &mut self,
        id: SurfaceId,
        new_parent: Option<SurfaceId>,
    ) -> Result<(), CompositionError> {
        let old_parent = self.node(id)?.parent;
        if let Some(new_parent) = new_parent {
            self.node(new_parent)?;
            if self.is_self_or_descendant(new_parent, id) {
                return Err(CompositionError::Cycle(id, new_parent));
            }
        }
        if id == self.base || id == self.overlay {
            return Err(CompositionError::DefaultSurface(id));
        }
        self.siblings_mut(old_parent)
            .retain(|sibling| *sibling != id);
        self.siblings_mut(new_parent).push(id);
        self.nodes.get_mut(&id).expect("checked above").parent = new_parent;
        Ok(())
    }

    /// Removes a surface. Its children take its place in sibling order under its former
    /// parent. The base and overlay surfaces are not removable.
    pub fn remove(&mut self, id: SurfaceId) -> Result<(), CompositionError> {
        self.node(id)?;
        if id == self.base || id == self.overlay {
            return Err(CompositionError::DefaultSurface(id));
        }
        let node = self.nodes.remove(&id).expect("checked above");
        for child in &node.children {
            if let Some(child) = self.nodes.get_mut(child) {
                child.parent = node.parent;
            }
        }
        let siblings = self.siblings_mut(node.parent);
        let at = siblings
            .iter()
            .position(|sibling| *sibling == id)
            .expect("a live surface is listed under its parent");
        siblings.splice(at..=at, node.children);
        Ok(())
    }

    /// Whether a root-level (or ancestor-rooted) surface stacks above the overlay surface.
    /// Surfaces nested under another surface follow their top-level ancestor.
    pub fn is_above_overlay(&self, id: SurfaceId) -> bool {
        let mut top = id;
        while let Some(parent) = self.nodes.get(&top).and_then(|node| node.parent) {
            top = parent;
        }
        let position = |wanted| self.roots.iter().position(|root| *root == wanted);
        match (position(top), position(self.overlay)) {
            (Some(top), Some(overlay)) => top > overlay,
            _ => false,
        }
    }

    /// The tree flattened to stacking order, bottom to top.
    pub fn layout(&self) -> CompositionLayout {
        let mut entries = Vec::with_capacity(self.nodes.len());
        let mut stack: Vec<SurfaceId> = self.roots.iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            let node = &self.nodes[&id];
            entries.push(SurfaceEntry {
                id,
                kind: node.kind,
                parent: node.parent,
            });
            stack.extend(node.children.iter().rev().copied());
        }
        CompositionLayout {
            base: self.base,
            overlay: self.overlay,
            entries,
        }
    }

    /// The framework-rendered surfaces in stacking order, bottom to top.
    pub fn framework_surfaces(&self) -> Vec<SurfaceId> {
        self.layout()
            .entries
            .into_iter()
            .filter(|entry| entry.kind == SurfaceKind::Framework)
            .map(|entry| entry.id)
            .collect()
    }

    /// The number of surfaces, including the base and overlay.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Always false: the base and overlay surfaces exist from the start.
    pub fn is_empty(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(tree: &SurfaceTree) -> Vec<SurfaceId> {
        tree.layout().entries.iter().map(|entry| entry.id).collect()
    }

    #[test]
    fn natives_start_between_base_and_overlay() {
        let mut tree = SurfaceTree::new();
        let a = tree.create_below_overlay(SurfaceKind::Native);
        let b = tree.create_below_overlay(SurfaceKind::ExternalGpu);
        assert_eq!(order(&tree), [tree.base(), a, b, tree.overlay()]);
    }

    #[test]
    fn stale_identity_is_rejected_everywhere() {
        let mut tree = SurfaceTree::new();
        let a = tree.create_below_overlay(SurfaceKind::Native);
        let b = tree.create_below_overlay(SurfaceKind::Native);
        tree.remove(a).unwrap();
        assert!(matches!(tree.kind(a), Err(CompositionError::Stale(id)) if id == a));
        assert!(matches!(tree.parent(a), Err(CompositionError::Stale(_))));
        assert!(matches!(tree.children(a), Err(CompositionError::Stale(_))));
        assert!(matches!(tree.remove(a), Err(CompositionError::Stale(_))));
        assert!(matches!(
            tree.place_relative(a, b, true),
            Err(CompositionError::Stale(_))
        ));
        assert!(matches!(
            tree.place_relative(b, a, true),
            Err(CompositionError::Stale(_))
        ));
        assert!(matches!(
            tree.reparent(b, Some(a)),
            Err(CompositionError::Stale(_))
        ));
        // Identities are not reused, so a new surface never revives a stale one.
        let c = tree.create_below_overlay(SurfaceKind::Native);
        assert_ne!(a, c);
        assert!(!tree.contains(a));
    }

    #[test]
    fn self_ordering_cycles_and_cross_parent_ordering_are_rejected() {
        let mut tree = SurfaceTree::new();
        let a = tree.create_below_overlay(SurfaceKind::Native);
        let b = tree.create_below_overlay(SurfaceKind::Native);
        let c = tree.create_below_overlay(SurfaceKind::Native);
        assert!(matches!(
            tree.place_relative(a, a, true),
            Err(CompositionError::SelfOrdering(_))
        ));
        tree.reparent(b, Some(a)).unwrap();
        tree.reparent(c, Some(b)).unwrap();
        assert!(matches!(
            tree.reparent(a, Some(a)),
            Err(CompositionError::Cycle(..))
        ));
        assert!(matches!(
            tree.reparent(a, Some(c)),
            Err(CompositionError::Cycle(..))
        ));
        assert!(matches!(
            tree.place_relative(b, tree.overlay(), false),
            Err(CompositionError::DifferentParents(..))
        ));
        assert!(matches!(
            tree.place_relative(a, c, true),
            Err(CompositionError::DifferentParents(..))
        ));
        // Failed operations leave the tree as it was.
        assert_eq!(tree.parent(c).unwrap(), Some(b));
        assert_eq!(tree.parent(a).unwrap(), None);
    }

    #[test]
    fn ordering_above_and_below_siblings() {
        let mut tree = SurfaceTree::new();
        let a = tree.create_below_overlay(SurfaceKind::Native);
        let b = tree.create_below_overlay(SurfaceKind::Native);
        tree.place_relative(a, tree.overlay(), true).unwrap();
        assert_eq!(order(&tree), [tree.base(), b, tree.overlay(), a]);
        assert!(tree.is_above_overlay(a));
        assert!(!tree.is_above_overlay(b));
        tree.place_relative(a, b, false).unwrap();
        assert_eq!(order(&tree), [tree.base(), a, b, tree.overlay()]);
        tree.place_relative(tree.base(), b, true).unwrap();
        assert_eq!(order(&tree), [a, b, tree.base(), tree.overlay()]);
    }

    #[test]
    fn removal_puts_children_at_the_former_sibling_position() {
        let mut tree = SurfaceTree::new();
        let left = tree.create_below_overlay(SurfaceKind::Native);
        let parent = tree.create_below_overlay(SurfaceKind::Native);
        let right = tree.create_below_overlay(SurfaceKind::Native);
        let first = tree.create_below_overlay(SurfaceKind::Native);
        let second = tree.create_below_overlay(SurfaceKind::Native);
        tree.reparent(first, Some(parent)).unwrap();
        tree.reparent(second, Some(parent)).unwrap();
        assert_eq!(
            order(&tree),
            [
                tree.base(),
                left,
                parent,
                first,
                second,
                right,
                tree.overlay()
            ]
        );
        tree.remove(parent).unwrap();
        assert_eq!(
            tree.roots(),
            [tree.base(), left, first, second, right, tree.overlay()]
        );
        assert_eq!(tree.parent(first).unwrap(), None);
        assert_eq!(tree.parent(second).unwrap(), None);
    }

    #[test]
    fn removing_a_nested_surface_rehomes_children_under_its_parent() {
        let mut tree = SurfaceTree::new();
        let top = tree.create_below_overlay(SurfaceKind::Native);
        let middle = tree.create_below_overlay(SurfaceKind::Native);
        let leaf = tree.create_below_overlay(SurfaceKind::Native);
        tree.reparent(middle, Some(top)).unwrap();
        tree.reparent(leaf, Some(middle)).unwrap();
        tree.remove(middle).unwrap();
        assert_eq!(tree.parent(leaf).unwrap(), Some(top));
        assert_eq!(tree.children(top).unwrap(), [leaf]);
    }

    #[test]
    fn base_and_overlay_are_not_removable_or_reparentable() {
        let mut tree = SurfaceTree::new();
        let a = tree.create_below_overlay(SurfaceKind::Native);
        assert!(matches!(
            tree.remove(tree.base()),
            Err(CompositionError::DefaultSurface(_))
        ));
        assert!(matches!(
            tree.remove(tree.overlay()),
            Err(CompositionError::DefaultSurface(_))
        ));
        assert!(matches!(
            tree.reparent(tree.overlay(), Some(a)),
            Err(CompositionError::DefaultSurface(_))
        ));
        assert_eq!(tree.len(), 3);
    }

    #[test]
    fn children_of_the_base_are_children_of_the_window_surface() {
        let mut tree = SurfaceTree::new();
        let a = tree.create_below_overlay(SurfaceKind::Native);
        tree.reparent(a, Some(tree.base())).unwrap();
        let layout = tree.layout();
        assert_eq!(layout.entry(a).unwrap().parent, Some(tree.base()));
        assert_eq!(
            layout
                .entries
                .iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>(),
            [tree.base(), a, tree.overlay()]
        );
    }
}
