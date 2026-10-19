//! The context menu of the tree as a pure model (CMP-016 R15): items, enablement, active-item
//! navigation that skips disabled items and wraps, and the "copy path" text.

use super::model::{Entry, EntryKind};
use crate::floating::{Edge, StepDirection, edge_available, step_available};

/// Identifier of the built-in "Copy path" item.
pub const COPY_PATH: &str = "copy-path";
/// Identifier of the built-in "Rename" item.
pub const RENAME: &str = "rename";
/// Identifier of the built-in "Delete" item.
pub const DELETE: &str = "delete";

/// One item of the menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItem {
    /// The item identifier (owner items use their own).
    pub id: String,
    /// The label.
    pub label: String,
    /// Whether the item can be activated.
    pub enabled: bool,
}

/// Whether an entry may be mutated: the owner revision token is present, it is not a symlink,
/// the owner's capability allows it and no mutation is in flight.
pub fn is_mutable(entry: &Entry, owner_capability: bool, mutation_in_flight: bool) -> bool {
    entry.revision.is_some()
        && entry.kind != EntryKind::Symlink
        && owner_capability
        && !mutation_in_flight
}

/// The items for the action paths: the owner's non-mutating items (always enabled), "Copy
/// path" (always enabled), "Rename" (enabled only for exactly one mutable path) and "Delete"
/// (labelled "Delete N items…" for several; enabled only when every path is mutable).
/// `rename_mutable` and `delete_mutable` give the mutability of each path.
pub fn menu_items(
    owner_items: &[(String, String)],
    path_count: usize,
    rename_mutable: &[bool],
    delete_mutable: &[bool],
) -> Vec<MenuItem> {
    let mut items: Vec<MenuItem> = owner_items
        .iter()
        .map(|(id, label)| MenuItem {
            id: id.clone(),
            label: label.clone(),
            enabled: true,
        })
        .collect();
    items.push(MenuItem {
        id: COPY_PATH.to_owned(),
        label: "Copy path".to_owned(),
        enabled: path_count > 0,
    });
    items.push(MenuItem {
        id: RENAME.to_owned(),
        label: "Rename".to_owned(),
        enabled: rename_mutable.len() == 1 && rename_mutable[0],
    });
    items.push(MenuItem {
        id: DELETE.to_owned(),
        label: if path_count > 1 {
            format!("Delete {path_count} items…")
        } else {
            "Delete…".to_owned()
        },
        enabled: !delete_mutable.is_empty() && delete_mutable.iter().all(|mutable| *mutable),
    });
    items
}

/// The text "Copy path" writes: each action path resolved by the owning host's rules, joined by
/// newlines in visible order.
pub fn copy_path_text(paths: &[String], resolve: impl Fn(&str) -> String) -> String {
    paths
        .iter()
        .map(|path| resolve(path))
        .collect::<Vec<_>>()
        .join("\n")
}

/// An open context menu.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextMenu {
    /// The action paths the menu applies to.
    pub paths: Vec<String>,
    /// The items.
    pub items: Vec<MenuItem>,
    active: Option<usize>,
}

impl ContextMenu {
    /// A menu over `paths`; the first enabled item is not active until a key moves it.
    pub fn new(paths: Vec<String>, items: Vec<MenuItem>) -> Self {
        Self {
            paths,
            items,
            active: None,
        }
    }

    /// The active item index.
    pub fn active(&self) -> Option<usize> {
        self.active
    }

    /// Makes `index` active when it is an enabled item.
    pub fn set_active(&mut self, index: Option<usize>) {
        self.active = index.filter(|ix| self.items.get(*ix).is_some_and(|item| item.enabled));
    }

    fn step(&mut self, direction: StepDirection) {
        let items = &self.items;
        self.active = step_available(items.len(), self.active, direction, |ix| items[ix].enabled);
    }

    /// Down: the next enabled item, wrapping.
    pub fn next(&mut self) {
        self.step(StepDirection::Forward);
    }

    /// Up: the previous enabled item, wrapping.
    pub fn previous(&mut self) {
        self.step(StepDirection::Backward);
    }

    /// Home: the first enabled item.
    pub fn first(&mut self) {
        let items = &self.items;
        self.active = edge_available(items.len(), Edge::First, |ix| items[ix].enabled);
    }

    /// End: the last enabled item.
    pub fn last(&mut self) {
        let items = &self.items;
        self.active = edge_available(items.len(), Edge::Last, |ix| items[ix].enabled);
    }

    /// Enter: the active item, when there is one.
    pub fn activate(&self) -> Option<&MenuItem> {
        self.active.and_then(|ix| self.items.get(ix))
    }
}
