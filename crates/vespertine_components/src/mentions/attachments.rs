//! Per-scope attachment ownership and numbering (CMP-010 R13, R14).
//!
//! An [`AttachmentScope`] belongs to one editing scope (a draft or conversation key). The editor
//! calls [`AttachmentScope::reconcile`] with the attachment numbers referenced in the source after
//! every edit, undo and redo and when the scope changes; attachments whose number is no longer
//! referenced move to a stash and come back when the number is referenced again, which is how undo
//! restores an attachment.

use std::collections::BTreeSet;
use std::path::Path;

/// Identifies an attachment within its scope; never reused.
pub type AttachmentId = u64;

/// What a staged attachment is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttachmentKind {
    /// An image: labelled `Image <n>` plus the staged file's extension.
    Image,
    /// Any other file: keeps its file name as the label.
    File,
}

/// An attachment staged by the owner, waiting to be adopted by a scope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagedAttachment<T> {
    /// The staged file's name (used for its extension, or as the label of a file).
    pub name: String,
    /// Image or other file.
    pub kind: AttachmentKind,
    /// The owner's payload (path, bytes, image handle, ...).
    pub data: T,
}

/// An attachment held by a scope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attachment<T> {
    /// The attachment's id.
    pub id: AttachmentId,
    /// The number referenced from the source; `None` for attachments the source never names.
    pub number: Option<u32>,
    /// Image or other file.
    pub kind: AttachmentKind,
    /// The label shown in chips: `Image <n>` plus extension for images, the file name otherwise.
    pub label: String,
    /// The owner's payload.
    pub data: T,
    /// Whether a reconciliation has seen a reference to the number. An attachment that was adopted
    /// but never referenced yet is not stashed, so adopting before inserting the reference works.
    seen_referenced: bool,
}

/// What a [`AttachmentScope::reconcile`] call changed.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Reconciliation {
    /// Attachments moved from the active set to the stash.
    pub stashed: Vec<AttachmentId>,
    /// Attachments moved from the stash back to the active set.
    pub restored: Vec<AttachmentId>,
}

/// The result of [`AttachmentScope::remove`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Removal {
    /// No active attachment has that id.
    NotFound,
    /// A numbered attachment was moved to the stash so that undoing the chip removal restores it.
    Stashed(u32),
    /// An unnumbered attachment was dropped (nothing in the source can restore it).
    Dropped,
}

type Callback<T> = Box<dyn FnMut(&Attachment<T>)>;

/// The attachments of one scope: active set, stash and numbering.
pub struct AttachmentScope<T> {
    scope: String,
    active: Vec<Attachment<T>>,
    stash: Vec<Attachment<T>>,
    next_id: AttachmentId,
    last_issued: u32,
    referenced: BTreeSet<u32>,
    on_last_reference_removed: Option<Callback<T>>,
    on_reference_restored: Option<Callback<T>>,
}

impl<T> AttachmentScope<T> {
    /// An empty scope identified by `scope`.
    pub fn new(scope: impl Into<String>) -> Self {
        Self {
            scope: scope.into(),
            active: Vec::new(),
            stash: Vec::new(),
            next_id: 1,
            last_issued: 0,
            referenced: BTreeSet::new(),
            on_last_reference_removed: None,
            on_reference_restored: None,
        }
    }

    /// The scope identifier (written into clipboard metadata, CMP-010 R15).
    pub fn scope_id(&self) -> &str {
        &self.scope
    }

    /// Sets the callback fired exactly once when the last reference to an attachment's number
    /// disappears from the source (the attachment has just moved to the stash).
    pub fn on_last_reference_removed(&mut self, callback: impl FnMut(&Attachment<T>) + 'static) {
        self.on_last_reference_removed = Some(Box::new(callback));
    }

    /// Sets the callback fired when a reference reappears and the attachment returns from the
    /// stash.
    pub fn on_reference_restored(&mut self, callback: impl FnMut(&Attachment<T>) + 'static) {
        self.on_reference_restored = Some(Box::new(callback));
    }

    /// The active attachments, in adoption order.
    pub fn active(&self) -> &[Attachment<T>] {
        &self.active
    }

    /// The stashed attachments.
    pub fn stashed(&self) -> &[Attachment<T>] {
        &self.stash
    }

    /// The active attachment with the given id.
    pub fn get(&self, id: AttachmentId) -> Option<&Attachment<T>> {
        self.active.iter().find(|attachment| attachment.id == id)
    }

    /// The active attachment with the given number.
    pub fn by_number(&self, number: u32) -> Option<&Attachment<T>> {
        self.active
            .iter()
            .find(|attachment| attachment.number == Some(number))
    }

    /// The numbers of the active attachments: the live set the editor registers (R4).
    pub fn live_numbers(&self) -> BTreeSet<u32> {
        self.active
            .iter()
            .filter_map(|attachment| attachment.number)
            .collect()
    }

    /// The number the next adopted attachment gets (R14): the maximum of the largest number held
    /// by this scope's attachments (active or stashed), the largest number referenced in the
    /// source at the last reconciliation, and the last number issued, plus one.
    pub fn next_number(&self) -> u32 {
        let held = self
            .active
            .iter()
            .chain(&self.stash)
            .filter_map(|attachment| attachment.number)
            .max()
            .unwrap_or(0);
        let referenced = self.referenced.last().copied().unwrap_or(0);
        held.max(referenced).max(self.last_issued).saturating_add(1)
    }

    fn allocate_id(&mut self) -> AttachmentId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Adopts staged attachments, giving each the next number (images and files share one
    /// numbering), and returns the numbers in order. Images are labelled `Image <n>` plus the
    /// staged file's extension; other files keep their name.
    pub fn adopt(&mut self, staged: Vec<StagedAttachment<T>>) -> Vec<u32> {
        let mut numbers = Vec::with_capacity(staged.len());
        for staged in staged {
            let number = self.next_number();
            self.last_issued = number;
            let label = match staged.kind {
                AttachmentKind::Image => {
                    match Path::new(&staged.name).extension().and_then(|e| e.to_str()) {
                        Some(extension) => format!("Image {number}.{extension}"),
                        None => format!("Image {number}"),
                    }
                }
                AttachmentKind::File => staged.name,
            };
            let id = self.allocate_id();
            self.active.push(Attachment {
                id,
                number: Some(number),
                kind: staged.kind,
                label,
                data: staged.data,
                seen_referenced: false,
            });
            numbers.push(number);
        }
        numbers
    }

    /// Adds an attachment that carries no number; reconciliation never touches it.
    pub fn add_unnumbered(&mut self, name: String, kind: AttachmentKind, data: T) -> AttachmentId {
        let id = self.allocate_id();
        self.active.push(Attachment {
            id,
            number: None,
            kind,
            label: name,
            data,
            seen_referenced: true,
        });
        id
    }

    /// Reconciles the scope with the attachment numbers referenced in the source (R13). Call it
    /// after every edit, undo and redo and when the scope changes. An active numbered attachment
    /// whose number is no longer referenced moves to the stash and
    /// `on_last_reference_removed` fires once for it; a stashed attachment whose number is
    /// referenced again returns to the active set and `on_reference_restored` fires. Attachments
    /// without a number, and attachments never referenced so far, are left alone.
    pub fn reconcile(&mut self, referenced: &BTreeSet<u32>) -> Reconciliation {
        self.referenced = referenced.clone();
        let mut outcome = Reconciliation::default();

        let mut index = 0;
        while index < self.active.len() {
            let attachment = &mut self.active[index];
            match attachment.number {
                Some(number) if referenced.contains(&number) => {
                    attachment.seen_referenced = true;
                    index += 1;
                }
                Some(_) if attachment.seen_referenced => {
                    let attachment = self.active.remove(index);
                    outcome.stashed.push(attachment.id);
                    self.stash.push(attachment);
                    let stashed = self.stash.last().expect("just pushed");
                    fire(&mut self.on_last_reference_removed, stashed);
                }
                _ => index += 1,
            }
        }

        let mut index = 0;
        while index < self.stash.len() {
            let number = self.stash[index].number;
            if number.is_some_and(|number| referenced.contains(&number)) {
                let mut attachment = self.stash.remove(index);
                attachment.seen_referenced = true;
                outcome.restored.push(attachment.id);
                let position = self
                    .active
                    .partition_point(|existing| existing.id < attachment.id);
                self.active.insert(position, attachment);
                fire(&mut self.on_reference_restored, &self.active[position]);
            } else {
                index += 1;
            }
        }
        outcome
    }

    /// Removes an attachment through the owner's own UI. The caller then removes every chip for it
    /// with `remove_attachment_references`. A numbered attachment is stashed (so undoing the chip
    /// removal restores it) and `on_last_reference_removed` fires; the following reconciliation
    /// finds it already stashed and fires nothing more. An unnumbered attachment is dropped.
    pub fn remove(&mut self, id: AttachmentId) -> Removal {
        let Some(index) = self.active.iter().position(|a| a.id == id) else {
            return Removal::NotFound;
        };
        let attachment = self.active.remove(index);
        match attachment.number {
            Some(number) => {
                self.stash.push(attachment);
                let stashed = self.stash.last().expect("just pushed");
                fire(&mut self.on_last_reference_removed, stashed);
                Removal::Stashed(number)
            }
            None => Removal::Dropped,
        }
    }

    /// Drops every stashed attachment for good (the draft was sent or discarded); numbering
    /// continues from the last issued number.
    pub fn discard_stash(&mut self) -> Vec<Attachment<T>> {
        std::mem::take(&mut self.stash)
    }
}

fn fire<T>(callback: &mut Option<Callback<T>>, attachment: &Attachment<T>) {
    if let Some(callback) = callback {
        callback(attachment);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn image(name: &str) -> StagedAttachment<()> {
        StagedAttachment {
            name: name.to_owned(),
            kind: AttachmentKind::Image,
            data: (),
        }
    }

    fn file(name: &str) -> StagedAttachment<()> {
        StagedAttachment {
            name: name.to_owned(),
            kind: AttachmentKind::File,
            data: (),
        }
    }

    fn set(numbers: &[u32]) -> BTreeSet<u32> {
        numbers.iter().copied().collect()
    }

    type Log = Rc<RefCell<Vec<String>>>;

    fn scope_with_log() -> (AttachmentScope<()>, Log) {
        let log: Log = Rc::default();
        let mut scope = AttachmentScope::new("draft-a");
        let removed = log.clone();
        scope.on_last_reference_removed(move |a| {
            removed.borrow_mut().push(format!("removed {}", a.label))
        });
        let restored = log.clone();
        scope.on_reference_restored(move |a| {
            restored.borrow_mut().push(format!("restored {}", a.label))
        });
        (scope, log)
    }

    #[test]
    fn adopt_numbers_images_and_files_from_one_sequence() {
        let mut scope = AttachmentScope::new("s");
        let numbers = scope.adopt(vec![image("shot.png"), file("notes.pdf"), image("")]);
        assert_eq!(numbers, vec![1, 2, 3]);
        let labels: Vec<&str> = scope.active().iter().map(|a| a.label.as_str()).collect();
        assert_eq!(labels, vec!["Image 1.png", "notes.pdf", "Image 3"]);
        assert_eq!(scope.live_numbers(), set(&[1, 2, 3]));
        assert_eq!(scope.by_number(2).unwrap().label, "notes.pdf");
        assert_eq!(scope.scope_id(), "s");
    }

    #[test]
    fn pasting_two_images_numbers_them_one_and_two() {
        let mut scope = AttachmentScope::new("s");
        let numbers = scope.adopt(vec![image("a.png"), image("b.jpg")]);
        assert_eq!(numbers, vec![1, 2]);
        assert_eq!(scope.active()[0].label, "Image 1.png");
        assert_eq!(scope.active()[1].label, "Image 2.jpg");
    }

    #[test]
    fn undo_and_redo_of_a_two_image_paste_stash_and_restore_both() {
        let (mut scope, log) = scope_with_log();
        scope.adopt(vec![image("a.png"), image("b.png")]);
        // Redo/insert: the source references both numbers.
        let outcome = scope.reconcile(&set(&[1, 2]));
        assert_eq!(outcome, Reconciliation::default());
        // One undo removes both chips and both attachments.
        let outcome = scope.reconcile(&set(&[]));
        assert_eq!(outcome.stashed.len(), 2);
        assert!(scope.active().is_empty());
        assert_eq!(scope.stashed().len(), 2);
        assert_eq!(scope.live_numbers(), set(&[]));
        // One redo restores both.
        let outcome = scope.reconcile(&set(&[1, 2]));
        assert_eq!(outcome.restored.len(), 2);
        assert_eq!(scope.live_numbers(), set(&[1, 2]));
        assert!(scope.stashed().is_empty());
        assert_eq!(
            *log.borrow(),
            vec![
                "removed Image 1.png",
                "removed Image 2.png",
                "restored Image 1.png",
                "restored Image 2.png"
            ]
        );
    }

    #[test]
    fn deleting_the_last_chip_stashes_once_and_undo_restores() {
        let (mut scope, log) = scope_with_log();
        scope.adopt(vec![image("a.png"), image("b.png")]);
        scope.reconcile(&set(&[1, 2]));
        scope.reconcile(&set(&[2]));
        scope.reconcile(&set(&[2]));
        scope.reconcile(&set(&[2]));
        assert_eq!(*log.borrow(), vec!["removed Image 1.png"]);
        assert_eq!(scope.stashed()[0].number, Some(1));
        scope.reconcile(&set(&[1, 2]));
        assert_eq!(scope.live_numbers(), set(&[1, 2]));
        assert_eq!(log.borrow().len(), 2);
        assert_eq!(log.borrow()[1], "restored Image 1.png");
    }

    #[test]
    fn duplicate_references_share_one_attachment() {
        let (mut scope, log) = scope_with_log();
        scope.adopt(vec![image("a.png")]);
        // The source holds two chips for number 1: the set is {1} either way.
        scope.reconcile(&set(&[1]));
        // One of the two chips is deleted: still {1}.
        scope.reconcile(&set(&[1]));
        assert_eq!(scope.active().len(), 1);
        assert!(log.borrow().is_empty());
        // The last one goes.
        scope.reconcile(&set(&[]));
        assert_eq!(*log.borrow(), vec!["removed Image 1.png"]);
    }

    #[test]
    fn numbers_are_never_reused() {
        let mut scope = AttachmentScope::new("s");
        scope.adopt(vec![image("a.png"), image("b.png")]);
        scope.reconcile(&set(&[1, 2]));
        // Remove "Image 2" (stashed): the next number is still 3.
        scope.reconcile(&set(&[1]));
        assert_eq!(scope.next_number(), 3);
        assert_eq!(scope.adopt(vec![image("c.png")]), vec![3]);
        // Even after the stash is discarded and the attachments are gone.
        scope.reconcile(&set(&[1, 3]));
        scope.reconcile(&set(&[]));
        scope.discard_stash();
        assert!(scope.active().is_empty());
        assert_eq!(scope.next_number(), 4);
    }

    #[test]
    fn next_number_considers_numbers_referenced_in_the_source() {
        let mut scope: AttachmentScope<()> = AttachmentScope::new("s");
        scope.reconcile(&set(&[7]));
        assert_eq!(scope.next_number(), 8);
        assert_eq!(scope.adopt(vec![file("x.txt")]), vec![8]);
    }

    #[test]
    fn unnumbered_attachments_are_never_touched() {
        let (mut scope, log) = scope_with_log();
        let id = scope.add_unnumbered("loose.txt".to_owned(), AttachmentKind::File, ());
        scope.reconcile(&set(&[]));
        scope.reconcile(&set(&[3]));
        assert!(scope.get(id).is_some());
        assert!(scope.stashed().is_empty());
        assert!(log.borrow().is_empty());
        assert_eq!(scope.live_numbers(), set(&[]));
    }

    #[test]
    fn adopted_attachment_survives_reconciliation_before_its_reference_exists() {
        let (mut scope, log) = scope_with_log();
        scope.adopt(vec![image("a.png")]);
        scope.reconcile(&set(&[]));
        assert_eq!(scope.active().len(), 1);
        assert!(log.borrow().is_empty());
        scope.reconcile(&set(&[1]));
        scope.reconcile(&set(&[]));
        assert_eq!(scope.stashed().len(), 1);
    }

    #[test]
    fn removing_through_the_owner_ui_stashes_for_undo_and_fires_once() {
        let (mut scope, log) = scope_with_log();
        scope.adopt(vec![image("a.png")]);
        scope.reconcile(&set(&[1]));
        let id = scope.active()[0].id;
        assert_eq!(scope.remove(id), Removal::Stashed(1));
        assert_eq!(*log.borrow(), vec!["removed Image 1.png"]);
        // The chips are removed from the source: reconciliation fires nothing more.
        scope.reconcile(&set(&[]));
        assert_eq!(log.borrow().len(), 1);
        // Undo of the chip removal restores the attachment.
        scope.reconcile(&set(&[1]));
        assert_eq!(scope.live_numbers(), set(&[1]));
        assert_eq!(log.borrow().len(), 2);
        assert_eq!(scope.remove(999), Removal::NotFound);
    }

    #[test]
    fn removing_an_unnumbered_attachment_drops_it() {
        let mut scope: AttachmentScope<()> = AttachmentScope::new("s");
        let id = scope.add_unnumbered("x".to_owned(), AttachmentKind::File, ());
        assert_eq!(scope.remove(id), Removal::Dropped);
        assert!(scope.active().is_empty());
        assert!(scope.stashed().is_empty());
    }

    #[test]
    fn restored_attachments_keep_adoption_order() {
        let mut scope = AttachmentScope::new("s");
        scope.adopt(vec![image("a.png"), image("b.png"), image("c.png")]);
        scope.reconcile(&set(&[1, 2, 3]));
        scope.reconcile(&set(&[2]));
        scope.reconcile(&set(&[1, 2, 3]));
        let numbers: Vec<Option<u32>> = scope.active().iter().map(|a| a.number).collect();
        assert_eq!(numbers, vec![Some(1), Some(2), Some(3)]);
    }

    #[test]
    fn a_full_session_matches_the_acceptance_scenario() {
        let (mut scope, log) = scope_with_log();
        // Paste two images.
        assert_eq!(
            scope.adopt(vec![image("a.png"), image("b.png")]),
            vec![1, 2]
        );
        scope.reconcile(&set(&[1, 2]));
        // Append a second reference to attachment 1, delete one chip: kept.
        scope.reconcile(&set(&[1, 2]));
        assert!(log.borrow().is_empty());
        // Delete the last chip of attachment 1.
        scope.reconcile(&set(&[2]));
        assert_eq!(log.borrow().len(), 1);
        // Undo restores it.
        scope.reconcile(&set(&[1, 2]));
        assert_eq!(scope.live_numbers(), set(&[1, 2]));
        // Remove 2, paste another: 3.
        scope.reconcile(&set(&[1]));
        assert_eq!(scope.adopt(vec![image("c.png")]), vec![3]);
        assert_eq!(scope.by_number(3).unwrap().label, "Image 3.png");
    }
}
