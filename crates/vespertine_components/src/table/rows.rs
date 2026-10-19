//! Row identity, flattening, the splice planner and the scroll anchor of the data table
//! (CMP-019 R2, R5, R6).

use std::collections::HashSet;
use std::ops::Range;
use vespertine::{ListOffset, ListState, Pixels, px};

/// The reserved id of the paging footer row.
pub const FOOTER_ID: &str = "footer:more";

/// What a row of the list is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableRowKind {
    /// A group heading.
    Group,
    /// A data row.
    Item,
    /// The paging footer.
    Footer,
}

/// A row of the list: a stable id, its kind and a content revision (rows with equal ids and
/// revisions are considered unchanged).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableRow {
    /// The stable id: `group:<key>`, `item:<id>` or [`FOOTER_ID`].
    pub id: String,
    /// The kind.
    pub kind: TableRowKind,
    /// The content revision supplied by the caller.
    pub revision: u64,
}

impl TableRow {
    /// A data row with id `item:<id>`.
    pub fn item(id: &str) -> Self {
        Self {
            id: format!("item:{id}"),
            kind: TableRowKind::Item,
            revision: 0,
        }
    }

    /// A group heading with id `group:<key>`.
    pub fn group(key: &str) -> Self {
        Self {
            id: format!("group:{key}"),
            kind: TableRowKind::Group,
            revision: 0,
        }
    }

    /// The paging footer.
    pub fn footer() -> Self {
        Self {
            id: FOOTER_ID.to_owned(),
            kind: TableRowKind::Footer,
            revision: 0,
        }
    }

    /// Sets the content revision.
    pub fn with_revision(mut self, revision: u64) -> Self {
        self.revision = revision;
        self
    }

    /// The group key of a group heading row.
    pub fn group_key(&self) -> Option<&str> {
        (self.kind == TableRowKind::Group)
            .then(|| self.id.strip_prefix("group:"))
            .flatten()
    }

    /// The item id of a data row.
    pub fn item_id(&self) -> Option<&str> {
        (self.kind == TableRowKind::Item)
            .then(|| self.id.strip_prefix("item:"))
            .flatten()
    }
}

/// A group of rows. A group without a key has no heading row and cannot be collapsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupSpec {
    /// The group key (`group:<key>` is the id of its heading).
    pub key: Option<String>,
    /// The revision of the heading row's content.
    pub revision: u64,
    /// The group's data rows.
    pub items: Vec<TableRow>,
}

impl GroupSpec {
    /// A keyed group.
    pub fn new(key: &str, items: Vec<TableRow>) -> Self {
        Self {
            key: Some(key.to_owned()),
            revision: 0,
            items,
        }
    }

    /// An unkeyed group: just data rows.
    pub fn ungrouped(items: Vec<TableRow>) -> Self {
        Self {
            key: None,
            revision: 0,
            items,
        }
    }
}

/// Flattens groups into the row sequence (R6): for each keyed group a heading row followed by
/// its rows unless the group is collapsed; then, when more data exists and the list is not
/// empty, one paging footer row.
pub fn flatten_groups(
    groups: &[GroupSpec],
    collapsed: &HashSet<String>,
    has_more: bool,
) -> Vec<TableRow> {
    let mut rows = Vec::new();
    for group in groups {
        match &group.key {
            Some(key) => {
                rows.push(TableRow::group(key).with_revision(group.revision));
                if !collapsed.contains(key) {
                    rows.extend(group.items.iter().cloned());
                }
            }
            None => rows.extend(group.items.iter().cloned()),
        }
    }
    if has_more && !rows.is_empty() {
        rows.push(TableRow::footer());
    }
    rows
}

/// The splice that turns `old` into `new` (R5): the range of `old` between the longest common
/// prefix and the longest common suffix and the number of rows replacing it; `None` when the
/// sequences are equal.
pub fn plan_splice<T: PartialEq>(old: &[T], new: &[T]) -> Option<(Range<usize>, usize)> {
    if old == new {
        return None;
    }
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let max_suffix = old.len().min(new.len()) - prefix;
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    Some((prefix..old.len() - suffix, new.len() - prefix - suffix))
}

/// The scroll anchor: the id of the first visible row and the pixel offset inside it.
#[derive(Clone, Debug, PartialEq)]
pub struct ScrollAnchor {
    /// The row id.
    pub id: String,
    /// The pixels scrolled into that row.
    pub offset: Pixels,
}

/// Captures the anchor of `list`, whose rows are `rows`; `None` for an empty list.
pub fn capture_anchor(list: &ListState, rows: &[TableRow]) -> Option<ScrollAnchor> {
    if rows.is_empty() {
        return None;
    }
    let top = list.logical_scroll_top();
    let row = &rows[top.item_ix.min(rows.len() - 1)];
    Some(ScrollAnchor {
        id: row.id.clone(),
        offset: top.offset_in_item,
    })
}

/// Where the anchor goes when `old` becomes `new`: the anchor row's new index with its original
/// offset; if the id disappeared, the nearest surviving row after it in the old order, else the
/// nearest surviving row before it, with offset 0; `None` when no row survives.
pub fn resolve_anchor(
    anchor: &ScrollAnchor,
    old: &[TableRow],
    new: &[TableRow],
) -> Option<(usize, Pixels)> {
    let position = |id: &str| new.iter().position(|row| row.id == id);
    if let Some(ix) = position(&anchor.id) {
        return Some((ix, anchor.offset));
    }
    let old_ix = old.iter().position(|row| row.id == anchor.id)?;
    let after = old[old_ix + 1..].iter();
    let before = old[..old_ix].iter().rev();
    after
        .chain(before)
        .find_map(|row| position(&row.id))
        .map(|ix| (ix, px(0.)))
}

fn restore(list: &ListState, resolved: Option<(usize, Pixels)>) {
    let (item_ix, offset_in_item) = resolved.unwrap_or((0, px(0.)));
    list.scroll_to(ListOffset {
        item_ix,
        offset_in_item,
    });
}

/// Updates `list` from `old` rows to `new` rows (R5): captures the anchor, splices only the
/// middle between the common prefix and suffix and restores the anchor explicitly. Equal row
/// sequences leave the list untouched. Returns the captured anchor.
pub fn sync_rows(list: &ListState, old: &[TableRow], new: &[TableRow]) -> Option<ScrollAnchor> {
    let (range, count) = plan_splice(old, new)?;
    let anchor = capture_anchor(list, old);
    list.splice(range, count);
    let resolved = if new.is_empty() {
        None
    } else {
        anchor
            .as_ref()
            .and_then(|anchor| resolve_anchor(anchor, old, new))
    };
    restore(list, resolved);
    anchor
}

/// An anchor kept alive across an animated transition (R5): resolved again when it settles.
#[derive(Clone, Debug, PartialEq)]
pub struct Transition {
    /// The click-time anchor.
    pub anchor: ScrollAnchor,
    /// The list position right after the anchor was restored.
    pub restored: (usize, Pixels),
}

impl Transition {
    /// Records a transition from the captured `anchor` and the list's current position.
    pub fn begin(list: &ListState, anchor: ScrollAnchor) -> Self {
        let top = list.logical_scroll_top();
        Self {
            anchor,
            restored: (top.item_ix, top.offset_in_item),
        }
    }

    /// Resolves the anchor again when the transition settles, with the scroll position at that
    /// moment: when the user scrolled since the anchor was restored the newer position wins and
    /// the anchor is captured from it; otherwise the click-time anchor is restored on the
    /// settled rows. Returns the anchor now in effect.
    pub fn settle(self, list: &ListState, rows: &[TableRow]) -> Option<ScrollAnchor> {
        let top = list.logical_scroll_top();
        let scrolled = top.item_ix != self.restored.0
            || (f32::from(top.offset_in_item) - f32::from(self.restored.1)).abs() > 0.5;
        if scrolled {
            return capture_anchor(list, rows);
        }
        if let Some((ix, offset)) = resolve_anchor(&self.anchor, rows, rows) {
            restore(list, Some((ix, offset)));
        }
        Some(self.anchor)
    }
}
