//! CMP-010 editor-level tests: chips in the multiline editor, insertion and removal, attachment
//! ownership, the scoped clipboard, hover detail, image-chip activation and chip geometry.

mod common_area;

use common_area::{Fixture, fixture};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc, sync::Arc};
use vespertine::{
    ClipboardItem, EntityInputHandler as _, Modifiers, MotionClock, MouseMoveEvent, PlatformInput,
    Point, point, px,
};
use vespertine_components::{
    mentions::{
        AttachmentKind, AttachmentScope, ClosureGrammar, ReferenceDescriptor, ReferenceGrammar,
        ReferenceKind, StagedAttachment, join_links,
    },
    text::{TextAreaEvent, area::PasteRecord, editor::EditorHost},
};
use vespertine_test_host::require_backend;

const OPEN: char = '⟦';
const CLOSE: char = '⟧';

/// `⟦file:path⟧`, `⟦dir:path⟧` and `⟦img:N:label⟧` (image attachment `N`).
fn grammar() -> Arc<dyn ReferenceGrammar> {
    Arc::new(ClosureGrammar::new(|source| {
        let mut found = Vec::new();
        let mut search = 0;
        while let Some(open) = source[search..].find(OPEN) {
            let start = search + open;
            let Some(close) = source[start..].find(CLOSE) else {
                break;
            };
            let end = start + close + CLOSE.len_utf8();
            let body = &source[start + OPEN.len_utf8()..end - CLOSE.len_utf8()];
            let parts: Vec<&str> = body.splitn(3, ':').collect();
            let descriptor = |kind, name: &str, identity: &str, attachment| ReferenceDescriptor {
                range: start..end,
                kind,
                name: name.to_owned(),
                identity: identity.to_owned(),
                attachment,
            };
            match parts.as_slice() {
                ["file", path] => found.push(descriptor(
                    ReferenceKind::File,
                    path.rsplit('/').next().unwrap_or(path),
                    path,
                    None,
                )),
                ["dir", path] => found.push(descriptor(
                    ReferenceKind::Directory,
                    path.trim_end_matches('/')
                        .rsplit('/')
                        .next()
                        .unwrap_or(path),
                    path,
                    None,
                )),
                ["img", number, label] => found.push(descriptor(
                    ReferenceKind::Image,
                    label,
                    label,
                    number.parse().ok(),
                )),
                _ => {}
            }
            search = end;
        }
        found
    }))
}

fn file(path: &str) -> String {
    format!("{OPEN}file:{path}{CLOSE}")
}

fn img(number: u32, label: &str) -> String {
    format!("{OPEN}img:{number}:{label}{CLOSE}")
}

fn staged(name: &str) -> StagedAttachment<()> {
    StagedAttachment {
        name: name.to_owned(),
        kind: AttachmentKind::Image,
        data: (),
    }
}

fn editor(width: f64, height: f64) -> Fixture {
    fixture(width, height, false, |area| {
        area.with_references(vec![grammar()])
    })
}

fn chip_ranges(fx: &mut Fixture) -> Vec<std::ops::Range<usize>> {
    fx.read(|area| area.chips().iter().map(|chip| chip.source()).collect())
}

fn labels(fx: &mut Fixture) -> Vec<String> {
    fx.read(|area| {
        area.chips()
            .iter()
            .map(|chip| chip.info.label.clone())
            .collect()
    })
}

fn move_to(fx: &mut Fixture, at: Point<vespertine::Pixels>) {
    fx.raw(PlatformInput::MouseMove(MouseMoveEvent {
        position: at,
        pressed_button: None,
        modifiers: Modifiers::default(),
    }));
}

fn set_clock(fx: &mut Fixture, ms: f64) {
    fx.host.app().update(|cx| {
        MotionClock::set(cx, ms);
    });
}

fn activations(fx: &mut Fixture) -> Vec<Option<u32>> {
    fx.take_events()
        .into_iter()
        .filter_map(|event| match event {
            TextAreaEvent::ActivateReference(number) => Some(number),
            _ => None,
        })
        .collect()
}

// ---- A1: attachments stay consistent with the source ---------------------------------------------

#[test]
fn a1_chip_references_and_attachments_stay_consistent_through_insert_delete_undo_and_redo() {
    require_backend!();
    let scope = Rc::new(RefCell::new(AttachmentScope::<()>::new("draft-1")));
    let removed = Rc::new(RefCell::new(Vec::<u32>::new()));
    let restored = Rc::new(RefCell::new(Vec::<u32>::new()));
    {
        let (removed, restored) = (removed.clone(), restored.clone());
        let mut scope = scope.borrow_mut();
        scope.on_last_reference_removed(move |attachment| {
            removed.borrow_mut().push(attachment.number.unwrap())
        });
        scope.on_reference_restored(move |attachment| {
            restored.borrow_mut().push(attachment.number.unwrap())
        });
    }
    let mut fx = editor(500.0, 300.0);
    fx.focus();
    {
        let scope = scope.clone();
        fx.with(move |area, _, cx| area.set_attachment_scope(scope, cx));
    }
    fx.type_text("compare ");

    // Two pasted images are numbered, adopted and inserted as ONE edit.
    let numbers = scope
        .borrow_mut()
        .adopt(vec![staged("a.png"), staged("b.png")]);
    assert_eq!(numbers, [1, 2]);
    let labels: Vec<String> = scope
        .borrow()
        .active()
        .iter()
        .map(|attachment| attachment.label.clone())
        .collect();
    assert_eq!(labels, ["Image 1.png", "Image 2.png"]);
    let link = join_links(&[img(1, &labels[0]), img(2, &labels[1])]);
    let before = fx.read(|area| area.core().undo_len());
    assert!(fx.with(|area, window, cx| area.insert_reference(&link, window, cx)));
    let source = format!(
        "compare {} {} ",
        img(1, "Image 1.png"),
        img(2, "Image 2.png")
    );
    assert_eq!(fx.text(), source);
    assert_eq!(fx.read(|area| area.core().undo_len()), before + 1);
    assert_eq!(fx.read(|area| area.chips().len()), 2);
    assert_eq!(scope.borrow().active().len(), 2);

    // One undo removes both chips and both attachments; one redo restores them.
    fx.with(|area, window, cx| area.undo(window, cx));
    assert_eq!(fx.text(), "compare ");
    assert!(scope.borrow().active().is_empty());
    assert_eq!(scope.borrow().stashed().len(), 2);
    assert_eq!(*removed.borrow(), [1, 2], "each callback fires once");
    fx.with(|area, window, cx| area.redo(window, cx));
    assert_eq!(fx.text(), source);
    assert_eq!(scope.borrow().active().len(), 2);
    assert_eq!(*restored.borrow(), [1, 2]);
    removed.borrow_mut().clear();
    restored.borrow_mut().clear();

    // Deleting the last chip of attachment 1 stashes it once; undo restores it.
    let ranges = chip_ranges(&mut fx);
    fx.select(ranges[0].clone());
    fx.keys("backspace");
    assert_eq!(*removed.borrow(), [1]);
    assert!(scope.borrow().by_number(1).is_none());
    assert_eq!(scope.borrow().stashed().len(), 1);
    fx.with(|area, window, cx| area.undo(window, cx));
    assert_eq!(fx.text(), source);
    assert_eq!(*restored.borrow(), [1]);
    assert!(scope.borrow().by_number(1).is_some());
    removed.borrow_mut().clear();

    // A second reference to attachment 1: deleting one chip keeps the attachment.
    fx.with(|area, window, cx| {
        area.set_selected_range(source.len()..source.len(), false, window, cx);
        area.insert_reference(&img(1, "Image 1.png"), window, cx)
    });
    assert_eq!(fx.read(|area| area.chips().len()), 3);
    let ranges = chip_ranges(&mut fx);
    fx.select(ranges[0].clone());
    fx.keys("backspace");
    assert!(removed.borrow().is_empty(), "a reference remains");
    assert!(scope.borrow().by_number(1).is_some());

    // After removing 2 and adding another image the new number is 3.
    scope
        .borrow_mut()
        .add_unnumbered("notes.txt".to_owned(), AttachmentKind::File, ());
    let ranges = chip_ranges(&mut fx);
    let two = fx.read(|area| {
        area.chips()
            .iter()
            .position(|chip| chip.info.descriptor.attachment == Some(2))
            .unwrap()
    });
    fx.select(ranges[two].clone());
    fx.keys("backspace");
    assert!(removed.borrow().contains(&2));
    let next = scope.borrow_mut().adopt(vec![staged("c.png")]);
    assert_eq!(next, [3]);
    // Attachments without chips are left alone.
    assert!(
        scope
            .borrow()
            .active()
            .iter()
            .any(|attachment| attachment.number.is_none())
    );
}

#[test]
fn a1_removing_an_attachment_from_the_owner_ui_removes_every_chip_in_one_undo_step() {
    require_backend!();
    let scope = Rc::new(RefCell::new(AttachmentScope::<()>::new("draft")));
    let mut fx = editor(500.0, 300.0);
    fx.focus();
    {
        let scope = scope.clone();
        fx.with(move |area, _, cx| area.set_attachment_scope(scope, cx));
    }
    let numbers = scope
        .borrow_mut()
        .adopt(vec![staged("a.png"), staged("b.png")]);
    let link = format!(
        "see {} and {} then {}",
        img(numbers[0], "Image 1.png"),
        img(numbers[1], "Image 2.png"),
        img(numbers[0], "Image 1.png")
    );
    fx.set_value("");
    fx.with(|area, window, cx| area.insert_reference(&link, window, cx));
    let full = fx.text();
    assert_eq!(fx.read(|area| area.chips().len()), 3);
    let id = scope.borrow().by_number(1).unwrap().id;
    scope.borrow_mut().remove(id);
    let before = fx.read(|area| area.core().undo_len());
    assert!(fx.with(|area, window, cx| area.remove_attachment_references(1, window, cx)));
    assert_eq!(
        fx.text(),
        format!("see and {} then ", img(2, "Image 2.png"))
    );
    assert_eq!(fx.read(|area| area.core().undo_len()), before + 1);
    // Nothing to remove the second time.
    assert!(!fx.with(|area, window, cx| area.remove_attachment_references(1, window, cx)));
    fx.with(|area, window, cx| area.undo(window, cx));
    assert_eq!(fx.text(), full);
    assert!(
        scope.borrow().by_number(1).is_some(),
        "undo restores the attachment"
    );
    assert_eq!(fx.read(|area| area.chips().len()), 3);
}

// ---- A2 and the scoped clipboard --------------------------------------------------------------------

fn copy_item(fx: &mut Fixture) -> ClipboardItem {
    fx.with(|area, _, cx| area.copy_item(cx))
        .expect("a copy item")
}

fn paste(fx: &mut Fixture, item: &ClipboardItem) {
    let item = item.clone();
    fx.with(move |area, window, cx| area.paste_item(&item, window, cx));
}

#[test]
fn a2_the_scoped_clipboard_restores_chips_only_inside_the_scope_and_for_files() {
    require_backend!();
    let mut live = BTreeMap::new();
    live.insert(1, None);
    let source = format!("look {} and {}", img(1, "Image 1.png"), file("src/a.rs"));
    let mut fx = editor(600.0, 200.0);
    fx.focus();
    fx.with({
        let live = live.clone();
        move |area, _, cx| area.set_live_attachments("scope-a", live, cx)
    });
    fx.set_value(&source);
    fx.keys("ctrl-a");
    let item = copy_item(&mut fx);
    assert_eq!(
        item.text().as_deref(),
        Some("look Image 1.png and src/a.rs"),
        "the clipboard text is the expanded plain text"
    );
    // Within the same scope the source comes back and the chips with it.
    fx.set_value("");
    paste(&mut fx, &item);
    assert_eq!(fx.text(), source);
    assert_eq!(fx.read(|area| area.chips().len()), 2);

    // A different scope: a source naming attachments pastes as plain text.
    fx.with(|area, window, cx| {
        area.set_live_attachments("scope-b", BTreeMap::new(), cx);
        area.set_value("", window, cx);
    });
    paste(&mut fx, &item);
    assert_eq!(fx.text(), "look Image 1.png and src/a.rs");
    assert!(fx.read(|area| area.chips().is_empty()));

    // A source without attachment references pastes as chips in either scope.
    fx.set_value(&format!("see {}", file("src/a.rs")));
    fx.keys("ctrl-a");
    let files = copy_item(&mut fx);
    fx.with(|area, window, cx| {
        area.set_live_attachments("scope-a", BTreeMap::new(), cx);
        area.set_value("", window, cx);
    });
    paste(&mut fx, &files);
    assert_eq!(fx.text(), format!("see {}", file("src/a.rs")));
    assert_eq!(fx.read(|area| area.chips().len()), 1);

    // Replacing the clipboard text afterwards makes the metadata ignored.
    let replaced = ClipboardItem::new_string("other text".to_owned());
    fx.set_value("");
    paste(&mut fx, &replaced);
    assert_eq!(fx.text(), "other text");
    let stale = {
        let metadata = files
            .entries()
            .iter()
            .find_map(|entry| match entry {
                vespertine::ClipboardEntry::String(string) => string.metadata.clone(),
                _ => None,
            })
            .expect("the copy carries metadata");
        ClipboardItem::new_string_with_metadata("edited meanwhile".to_owned(), metadata)
    };
    fx.set_value("");
    paste(&mut fx, &stale);
    assert_eq!(fx.text(), "edited meanwhile");
}

#[test]
fn r16_a_paste_is_its_own_undo_step_and_the_owner_may_amend_it_while_nothing_changed() {
    require_backend!();
    let mut fx = editor(500.0, 200.0);
    fx.focus();
    fx.type_text("a ");
    let before = fx.read(|area| area.core().undo_len());
    paste(
        &mut fx,
        &ClipboardItem::new_string("pasted token".to_owned()),
    );
    assert_eq!(fx.read(|area| area.core().undo_len()), before + 1);
    let record: PasteRecord = fx
        .take_events()
        .into_iter()
        .rev()
        .find_map(|event| match event {
            TextAreaEvent::Pasted(record) => Some(record),
            _ => None,
        })
        .expect("the paste is reported");
    assert_eq!(record.range, 2..14);
    // The owner turns the token into a reference inside the paste's undo step.
    let link = file("src/a.rs");
    assert!(fx.with(|area, window, cx| area.amend_paste(&record, 9..14, &link, window, cx)));
    assert_eq!(fx.text(), format!("a pasted {link}"));
    assert_eq!(fx.read(|area| area.core().undo_len()), before + 1);
    assert_eq!(fx.read(|area| area.chips().len()), 1);
    fx.with(|area, window, cx| area.undo(window, cx));
    assert_eq!(
        fx.text(),
        "a ",
        "one undo removes the paste with its amendment"
    );
    // The record is stale after any further change.
    paste(&mut fx, &ClipboardItem::new_string("again".to_owned()));
    let stale = fx.read(|area| area.core().edit_epoch());
    fx.type_text("!");
    let _ = stale;
    assert!(!fx.with(|area, window, cx| area.amend_paste(&record, 2..4, "x", window, cx)));
    assert_eq!(fx.text(), "a again!");
}

// ---- A4: insertion separators and gates -----------------------------------------------------------------

fn inserted(before: &str, after: &str) -> (String, usize) {
    let mut fx = editor(400.0, 200.0);
    fx.focus();
    let source = format!("{before}{after}");
    fx.set_value(&source);
    fx.select(before.len()..before.len());
    assert!(fx.with(|area, window, cx| area.insert_reference("LINK", window, cx)));
    (fx.text(), fx.selection().start)
}

#[test]
fn a4_insert_reference_follows_the_separator_rules_for_every_neighbor_class() {
    require_backend!();
    // The previous character: start, "(", "*", a letter and whitespace.
    assert_eq!(inserted("", "x").0, "LINK x");
    assert_eq!(inserted("(", "x").0, "(LINK x");
    assert_eq!(inserted("*", "x").0, "*LINK x");
    assert_eq!(inserted("a", "x").0, "a LINK x");
    assert_eq!(inserted("a ", "x").0, "a LINK x");
    // The next character: line break, whitespace, closer, comma, letter.
    assert_eq!(inserted("a ", "\nz"), ("a LINK\nz".to_owned(), 6));
    assert_eq!(
        inserted("a ", " z"),
        ("a LINK z".to_owned(), 7),
        "whitespace is consumed: the caret moves past it"
    );
    assert_eq!(inserted("a ", ")z"), ("a LINK)z".to_owned(), 6));
    assert_eq!(inserted("a ", ",z"), ("a LINK,z".to_owned(), 6));
    assert_eq!(inserted("a ", "z"), ("a LINK z".to_owned(), 7));
    assert_eq!(inserted("a ", ""), ("a LINK ".to_owned(), 7));
}

#[test]
fn a4_insertion_is_rejected_while_composing_or_read_only_and_leaves_the_marked_text() {
    require_backend!();
    let mut fx = editor(400.0, 200.0);
    fx.focus();
    fx.set_value("ab");
    fx.select(1..1);
    fx.with(|area, window, cx| area.replace_and_mark_text_in_range(None, "に", None, window, cx));
    let marked = fx.read(|area| area.marked_range());
    assert!(marked.is_some());
    assert!(!fx.with(|area, window, cx| area.insert_reference(&file("x"), window, cx)));
    assert!(!fx.with(|area, window, cx| area.replace_reference(0..1, &file("x"), window, cx)));
    assert_eq!(fx.read(|area| area.marked_range()), marked);
    assert_eq!(fx.text(), "aにb");
    fx.with(|area, window, cx| area.replace_text_in_range(None, "日本", window, cx));
    fx.with(|area, _, cx| area.set_read_only(true, cx));
    assert!(!fx.with(|area, window, cx| area.insert_reference(&file("x"), window, cx)));
    assert!(!fx.with(|area, window, cx| area.remove_attachment_references(1, window, cx)));
    assert_eq!(fx.text(), "a日本b");
}

#[test]
fn r10_replacing_a_completed_token_uses_the_trailing_rule_without_a_leading_separator() {
    require_backend!();
    let mut fx = editor(400.0, 200.0);
    fx.focus();
    fx.set_value("see @src now");
    let before = fx.read(|area| area.core().undo_len());
    assert!(fx.with(|area, window, cx| area.replace_reference(
        4..8,
        &file("src/a.rs"),
        window,
        cx
    )));
    assert_eq!(fx.text(), format!("see {} now", file("src/a.rs")));
    assert_eq!(fx.read(|area| area.core().undo_len()), before + 1);
    let end = 4 + file("src/a.rs").len() + 1;
    assert_eq!(fx.selection(), end..end);
}

// ---- A5: atomic behavior ------------------------------------------------------------------------------

fn boundary_at(source: &str, start: usize, end: usize, percent: usize) -> usize {
    let target = start + (end - start) * percent / 100;
    source
        .char_indices()
        .map(|(index, _)| index)
        .filter(|index| *index > start && *index < end)
        .min_by_key(|index| index.abs_diff(target))
        .unwrap()
}

#[test]
fn a5_chips_are_atomic_for_carets_keys_selection_and_input_method_ranges() {
    require_backend!();
    let source = format!("see {} end", file("src/a.rs"));
    let mut fx = editor(500.0, 200.0);
    fx.focus();
    fx.set_value(&source);
    let chip = chip_ranges(&mut fx)[0].clone();
    // A caret at 40% / 60% inside a chip snaps to its start / end.
    let low = boundary_at(&source, chip.start, chip.end, 40);
    let high = boundary_at(&source, chip.start, chip.end, 60);
    fx.select(low..low);
    assert_eq!(fx.selection(), chip.start..chip.start);
    fx.select(high..high);
    assert_eq!(fx.selection(), chip.end..chip.end);
    // Right crosses the chip in one press.
    fx.select(chip.start..chip.start);
    fx.keys("right");
    assert_eq!(fx.selection(), chip.end..chip.end);
    // Backspace after a chip deletes it whole, in one undo step.
    let before = fx.read(|area| area.core().undo_len());
    fx.keys("backspace");
    assert_eq!(fx.text(), "see  end");
    assert_eq!(fx.read(|area| area.core().undo_len()), before + 1);
    fx.with(|area, window, cx| area.undo(window, cx));
    assert_eq!(fx.text(), source);
    // Double-click selects the whole chip.
    let inside = fx.row_point(chip.start, 40.0);
    fx.press_at(inside, 2, false);
    fx.release_at(inside);
    assert_eq!(fx.selection(), chip);
    // A drag selection touching a chip is widened to include it.
    fx.select(0..0);
    let start = fx.row_point(0, 1.0);
    fx.press_at(start, 1, false);
    fx.drag_at(inside);
    fx.release_at(inside);
    assert_eq!(fx.selection(), 0..chip.end);
    // Input-method replace ranges are normalized.
    fx.select(0..0);
    let range16 = fx.read(|area| area.core().range_to_utf16(&(low..high)));
    fx.with(move |area, window, cx| area.replace_text_in_range(Some(range16), "X", window, cx));
    assert_eq!(fx.text(), "see X end");
}

// ---- A6: labels ------------------------------------------------------------------------------------------

#[test]
fn a6_labels_are_disambiguated_compacted_and_dangling_numbers_are_plain_text() {
    require_backend!();
    let mut fx = editor(900.0, 200.0);
    fx.set_value(&format!("{} {}", file("a/x.rs"), file("b/x.rs")));
    assert_eq!(labels(&mut fx), ["a/x.rs", "b/x.rs"]);
    fx.set_value(&format!("{} {}", file("a/x.rs"), file("b/y.rs")));
    assert_eq!(labels(&mut fx), ["x.rs", "y.rs"]);
    // A 40-grapheme label becomes 18 + "…" + 12.
    let long: String = "abcdefghij".repeat(4);
    fx.set_value(&file(&format!("d/{long}")));
    assert_eq!(
        labels(&mut fx),
        [format!("{}…{}", &long[..18], &long[28..])]
    );
    // Labels equal after compaction but different in full are lengthened until distinct.
    let one = format!("{}{}{}", "a".repeat(18), "1".repeat(10), "b".repeat(12));
    let two = format!("{}{}{}", "a".repeat(18), "2".repeat(10), "b".repeat(12));
    fx.set_value(&format!("{} {}", file(&one), file(&two)));
    assert_eq!(labels(&mut fx), [one, two]);
    // An unregistered attachment number displays as plain text.
    let mut live = BTreeMap::new();
    live.insert(1, None);
    fx.with(|area, _, cx| area.set_live_attachments("s", live, cx));
    fx.set_value(&format!(
        "{} {}",
        img(1, "Image 1.png"),
        img(2, "Image 2.png")
    ));
    assert_eq!(fx.read(|area| area.chips().len()), 1);
    let display = fx.read(|area| area.display_text().to_owned());
    assert!(display.ends_with("Image 2.png"), "{display:?}");
    assert_eq!(
        fx.text(),
        format!("{} {}", img(1, "Image 1.png"), img(2, "Image 2.png"))
    );
}

// ---- A7, A8: wrapping and geometry ----------------------------------------------------------------------------

#[test]
fn a7_a_chip_never_breaks_when_it_fits_on_the_next_row_and_an_oversized_one_wraps() {
    require_backend!();
    let mut fx = editor(260.0, 400.0);
    let chip = file("src/lib.rs");
    let mut widths = Vec::new();
    for words in 0..40 {
        fx.set_value(&format!("{}{chip} tail", "word ".repeat(words)));
        let pills = fx.with(|area, window, _| area.chip_pills(window));
        let chips = pills.iter().filter(|pill| pill.chip == 0).count();
        assert_eq!(chips, 1, "the chip stays on one row after {words} words");
        widths.push(pills[0].geometry.bounds.width);
    }
    assert!(widths.iter().all(|width| *width == widths[0]));
    assert!(
        widths[0] < 200.0,
        "the chip fits a row of the editor: {}",
        widths[0]
    );

    // An oversized chip wraps: one pill segment per row, only the first with the icon.
    let mut narrow = editor(120.0, 400.0);
    narrow.set_value(&file("a/some-quite-long-file-name.rs"));
    let pills = narrow.with(|area, window, _| area.chip_pills(window));
    assert!(pills.len() >= 2, "{pills:?}");
    assert!(pills[0].geometry.well.is_some());
    assert!(pills[1..].iter().all(|pill| pill.geometry.well.is_none()));
    let tops: Vec<f32> = pills.iter().map(|pill| pill.geometry.bounds.y).collect();
    assert!(tops.windows(2).all(|pair| pair[1] > pair[0]), "{tops:?}");
}

#[test]
fn a8_the_pill_is_inset_and_centered_within_the_row_and_the_offset_is_clamped() {
    require_backend!();
    let mut fx = editor(400.0, 200.0);
    fx.set_value(&format!("a {}", file("src/lib.rs")));
    let pills = fx.with(|area, window, _| area.chip_pills(window));
    assert_eq!(pills.len(), 1);
    let row_top = fx.point_of(2).y;
    let pill = pills[0].geometry;
    let offset = pill.bounds.y - (f32::from(row_top) + 1.5);
    assert!(offset.abs() <= 1.5 + f32::EPSILON, "offset {offset}");
    assert_eq!(pill.radius, 5.0);
    assert_eq!(pill.bounds.height, 20.0 - 3.0);
    let well = pill.well.expect("the first segment has an icon well");
    assert_eq!(well.bounds.width, pill.bounds.height - 2.0);
    assert_eq!(well.radius, 4.0);
    assert_eq!(well.icon.width, 14.0f32.min(well.bounds.width - 4.0));
}

// ---- A3: hover detail and image-chip activation ---------------------------------------------------------------

fn chips_and_images() -> (String, BTreeMap<u32, Option<vespertine::Image>>) {
    let mut live = BTreeMap::new();
    live.insert(1, None);
    (
        format!(
            "\n\n\n{} {} {}",
            file("src/a.rs"),
            img(1, "Image 1.png"),
            file("b/c.rs")
        ),
        live,
    )
}

fn point_in_chip(fx: &mut Fixture, index: usize, dx: f32) -> Point<vespertine::Pixels> {
    let start = chip_ranges(fx)[index].start;
    fx.row_point(start, dx)
}

#[test]
fn a3_an_image_chip_activates_on_a_click_that_does_not_drag() {
    require_backend!();
    let (source, live) = chips_and_images();
    let mut fx = editor(700.0, 300.0);
    fx.with(move |area, _, cx| area.set_live_attachments("s", live, cx));
    fx.set_value(&source);
    fx.focus();
    let image = point_in_chip(&mut fx, 1, 40.0);
    let other = point_in_chip(&mut fx, 0, 40.0);
    let moved = |dx: f32| point(image.x + px(dx), image.y);
    for (distance, activates) in [(0.0, true), (3.0, true), (4.0, true), (5.0, false)] {
        fx.take_events();
        fx.press_at(image, 1, false);
        fx.release_at(moved(distance));
        assert_eq!(
            activations(&mut fx),
            if activates { vec![Some(1)] } else { vec![] },
            "{distance} px of movement"
        );
    }
    // A release over a different chip, a Shift press and a double click do not activate.
    fx.press_at(image, 1, false);
    fx.release_at(other);
    assert!(activations(&mut fx).is_empty());
    fx.press_at(image, 1, true);
    fx.release_at(image);
    assert!(activations(&mut fx).is_empty());
    fx.press_at(image, 2, false);
    fx.release_at(image);
    assert!(activations(&mut fx).is_empty());
    // The pointing hand appears over the image chip only.
    assert!(fx.read(|area| area.chip_at(image).is_some()));
}

#[test]
fn a3_the_hover_detail_shows_after_420_ms_for_non_image_chips_only() {
    require_backend!();
    let (source, live) = chips_and_images();
    let mut fx = editor(700.0, 300.0);
    fx.with(move |area, _, cx| area.set_live_attachments("s", live, cx));
    fx.set_value(&source);
    fx.focus();
    set_clock(&mut fx, 1000.0);
    let file_chip = point_in_chip(&mut fx, 0, 40.0);
    let same_chip = point_in_chip(&mut fx, 0, 60.0);
    let image = point_in_chip(&mut fx, 1, 40.0);
    let second_file = point_in_chip(&mut fx, 2, 40.0);

    move_to(&mut fx, file_chip);
    let generation = fx.read(|area| area.hover_generation());
    assert!(
        !fx.with(|area, _, cx| {
            area.hover_timer_fired(generation, 1419, cx);
            area.hover_detail_visible()
        }),
        "not after 419 ms"
    );
    // Moving inside the chip keeps the wait.
    set_clock(&mut fx, 1200.0);
    move_to(&mut fx, same_chip);
    assert_eq!(fx.read(|area| area.hover_generation()), generation);
    assert!(
        fx.with(|area, _, cx| {
            area.hover_timer_fired(generation, 1420, cx);
            area.hover_detail_visible()
        }),
        "after 420 ms"
    );
    // The popup is 24 px tall and, with room above, sits above the chip with a 1 px gap.
    let popup = fx
        .with(|area, window, _| area.hover_popup(window))
        .expect("a popup");
    assert_eq!(popup.text, "src/a.rs");
    assert_eq!(popup.bounds.size.height, px(24.0));
    let hit = fx.read(|area| area.chip_at(same_chip)).unwrap();
    assert_eq!(popup.bounds.bottom() + px(1.0), hit.segment.top());
    // Moving inside the chip keeps a visible popup.
    move_to(&mut fx, file_chip);
    assert!(fx.read(|area| area.hover_detail_visible()));
    // Image chips never show a popup and hide the one of the chip left.
    move_to(&mut fx, image);
    assert!(!fx.read(|area| area.hover_detail_visible()));
    // Moving to another chip restarts the wait.
    set_clock(&mut fx, 2000.0);
    move_to(&mut fx, file_chip);
    let first = fx.read(|area| area.hover_generation());
    move_to(&mut fx, second_file);
    let second = fx.read(|area| area.hover_generation());
    assert_ne!(first, second);
    assert!(
        !fx.with(|area, _, cx| {
            area.hover_timer_fired(first, 2500, cx);
            area.hover_detail_visible()
        }),
        "the stale generation does nothing"
    );
    assert!(fx.with(|area, _, cx| {
        area.hover_timer_fired(second, 2420, cx);
        area.hover_detail_visible()
    }));
    // An edit hides it.
    fx.type_text("x");
    assert!(!fx.read(|area| area.hover_detail_visible()));
}

#[test]
fn r8_the_popup_goes_above_a_chip_that_has_room_and_directories_end_in_a_slash() {
    require_backend!();
    let mut fx = editor(700.0, 300.0);
    fx.set_value(&format!("\n\n\n\n\n\n{OPEN}dir:src/widgets{CLOSE}"));
    fx.focus();
    set_clock(&mut fx, 100.0);
    let at = point_in_chip(&mut fx, 0, 40.0);
    move_to(&mut fx, at);
    let generation = fx.read(|area| area.hover_generation());
    fx.with(|area, _, cx| area.hover_timer_fired(generation, 520, cx));
    let popup = fx
        .with(|area, window, _| area.hover_popup(window))
        .expect("a popup");
    assert_eq!(popup.text, "src/widgets/");
    let hit = fx.read(|area| area.chip_at(at)).unwrap();
    assert_eq!(
        popup.bounds.bottom() + px(1.0),
        hit.segment.top(),
        "1 px gap above"
    );
    // A wheel scroll hides it.
    fx.raw(PlatformInput::ScrollWheel(vespertine::ScrollWheelEvent {
        position: at,
        delta: vespertine::ScrollDelta::Pixels(point(px(0.0), px(-5.0))),
        modifiers: Modifiers::default(),
        touch_phase: vespertine::TouchPhase::Moved,
    }));
    assert!(fx.read(|area| !area.hover_detail_visible()));
}
