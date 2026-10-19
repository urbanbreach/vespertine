//! The root view of the offscreen window.

use crate::tree::RetainedHost;
use vespertine::{
    AnyElement, AnyView, App, Context, IntoElement, ParentElement, Render, Styled, Window, div,
};

/// What the host window shows.
pub(crate) enum Content {
    /// A retained host tree, rebuilt into elements for every frame.
    Tree(Box<dyn RetainedHost>),
    /// A native view entity.
    View(AnyView),
}

/// The window's root view. It owns the hosted content, so releasing it (as the host does on
/// teardown) releases the retained tree and the view-owned entities.
pub(crate) struct HostRoot {
    pub(crate) content: Option<Content>,
}

impl Render for HostRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let root = div().size_full();
        match self.content.as_mut() {
            None => root,
            Some(Content::View(view)) => root.child(view.clone()),
            Some(Content::Tree(tree)) => root.child(tree.render(window, cx)),
        }
    }
}

/// A view that builds its element from a closure on every render.
pub(crate) struct ElementView(pub(crate) Box<dyn Fn(&mut Window, &mut App) -> AnyElement>);

impl Render for ElementView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        (self.0)(window, cx)
    }
}
