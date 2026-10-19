//! PLT-009 behavior tests against the reference chip editor model: scoped paste, owner
//! policies and the standard edit actions.

#[path = "../examples/example_support/chip_editor.rs"]
mod chip_editor;

use chip_editor::{ChipEditor, ChipRef, run_scenario};
use vespertine::{
    ClipboardItem, EditTarget as _,
    clipboard::edit::{EditAction, EditOutcome, availability, perform},
};

fn file(path: &str) -> ChipRef {
    ChipRef::File { path: path.into() }
}

fn editor_with_chips() -> ChipEditor {
    let mut editor = ChipEditor::new("docs", &["docs"]);
    editor.type_text("see ");
    editor.insert_chip(file("main.rs"));
    editor.type_text(" ok");
    editor.select_all();
    editor
}

fn copy(editor: &mut ChipEditor) -> ClipboardItem {
    match perform(editor, EditAction::Copy, None) {
        EditOutcome::WriteToClipboard(item) => item,
        other => panic!("copy produced {other:?}"),
    }
}

#[test]
fn scenario_restores_chips_only_in_a_compatible_scope() {
    let scenario = run_scenario();
    let expanded = scenario.copied.text().unwrap();
    assert_eq!(expanded, "see @main.rs and @alice");
    assert!(scenario.copied.metadata().is_some());

    assert_eq!(
        scenario.compatible.chips(),
        [
            &file("main.rs"),
            &ChipRef::Mention {
                name: "alice".into()
            }
        ]
    );
    assert_eq!(scenario.compatible.expanded_text(), expanded);
    assert_eq!(scenario.compatible.display(), "see [@main.rs] and [@alice]");

    for plain_result in [&scenario.incompatible, &scenario.plain] {
        assert!(plain_result.chips().is_empty());
        assert_eq!(plain_result.expanded_text(), expanded);
    }
}

#[test]
fn scenario_media_first_uses_the_image_and_text_first_the_text() {
    let scenario = run_scenario();
    assert_eq!(scenario.media_first.attachments.len(), 1);
    assert!(scenario.media_first.attachments[0].starts_with("image png"));
    assert_eq!(scenario.media_first.expanded_text(), "");
    assert!(scenario.text_first.attachments.is_empty());
    assert_eq!(scenario.text_first.expanded_text(), "a photo");
    assert_eq!(scenario.image_with_text.entries().len(), 2);
}

#[test]
fn copy_without_projections_writes_plain_text_without_metadata() {
    let mut editor = ChipEditor::new("docs", &["docs"]);
    editor.type_text("plain words");
    editor.select(0..5);
    let item = copy(&mut editor);
    assert_eq!(item, ClipboardItem::new_string("plain".into()));
    assert_eq!(item.metadata(), None);
}

#[test]
fn metadata_whose_text_no_longer_matches_pastes_plain_text() {
    let mut source = editor_with_chips();
    let copied = copy(&mut source);
    // Another program replaced the clipboard text but the old metadata lingers.
    let stale = ClipboardItem::new_string_with_metadata(
        "something else".into(),
        copied.metadata().unwrap().clone(),
    );
    let mut target = ChipEditor::new("docs", &["docs"]);
    assert_eq!(
        perform(&mut target, EditAction::Paste, Some(&stale)),
        EditOutcome::Changed
    );
    assert!(target.chips().is_empty());
    assert_eq!(target.expanded_text(), "something else");
}

#[test]
fn malformed_or_unknown_version_metadata_pastes_plain_text_without_error() {
    let valid = {
        let mut source = editor_with_chips();
        copy(&mut source)
    };
    let text = valid.text().unwrap();
    let unknown_version = valid
        .metadata()
        .unwrap()
        .replacen("\"version\":1", "\"version\":2", 1);
    assert_ne!(Some(&unknown_version), valid.metadata());
    for metadata in [
        unknown_version.as_str(),
        "not json at all",
        "{\"version\":1}",
        "{\"version\":1,\"scope\":\"docs\",\"text\":\"x\",\"projections\":[{\"reference\":{\"Nope\":1},\"range\":{\"start\":0,\"end\":1}}]}",
    ] {
        let item = ClipboardItem::new_string_with_metadata(text.clone(), metadata.into());
        let mut target = ChipEditor::new("docs", &["docs"]);
        assert_eq!(
            perform(&mut target, EditAction::Paste, Some(&item)),
            EditOutcome::Changed,
            "{metadata}"
        );
        assert!(target.chips().is_empty(), "{metadata}");
        assert_eq!(target.expanded_text(), text, "{metadata}");
    }
}

#[test]
fn text_round_trips_exactly_through_a_copy_and_paste() {
    let text = "astral 𝄞 😀, combining e\u{301}, 日本語";
    let mut source = ChipEditor::new("docs", &["docs"]);
    source.type_text(text);
    source.select_all();
    let item = copy(&mut source);
    assert_eq!(item.text().as_deref(), Some(text));
    let mut target = ChipEditor::plain_field();
    perform(&mut target, EditAction::Paste, Some(&item));
    assert_eq!(target.expanded_text(), text);
}

#[test]
fn pasting_an_empty_clipboard_is_a_no_op() {
    let mut editor = editor_with_chips();
    let before = editor.display();
    assert_eq!(
        perform(&mut editor, EditAction::Paste, None),
        EditOutcome::Unavailable
    );
    for empty in [
        ClipboardItem::from_entries([]),
        ClipboardItem::new_string(String::new()),
    ] {
        assert_eq!(
            perform(&mut editor, EditAction::Paste, Some(&empty)),
            EditOutcome::Unavailable
        );
    }
    assert_eq!(editor.display(), before);
    assert!(!availability(&editor, None).paste);
}

#[test]
fn read_only_editor_copies_but_never_cuts_or_pastes() {
    let mut editor = editor_with_chips().read_only();
    let before = editor.display();
    let availability = availability(&editor, Some(&ClipboardItem::new_string("x".into())));
    assert!(availability.copy);
    assert!(!availability.cut);
    assert!(!availability.paste);

    assert_eq!(
        perform(&mut editor, EditAction::Cut, None),
        EditOutcome::Unavailable
    );
    assert_eq!(editor.display(), before);
    assert_eq!(
        perform(
            &mut editor,
            EditAction::Paste,
            Some(&ClipboardItem::new_string("x".into()))
        ),
        EditOutcome::Unavailable
    );
    assert_eq!(editor.display(), before);
    assert!(matches!(
        perform(&mut editor, EditAction::Copy, None),
        EditOutcome::WriteToClipboard(_)
    ));
}

#[test]
fn copy_and_cut_need_a_selection_and_select_all_needs_content() {
    let mut empty = ChipEditor::new("docs", &["docs"]);
    let state = availability(&empty, None);
    assert!(!state.copy && !state.cut && !state.select_all && !state.paste);
    for action in [EditAction::Copy, EditAction::Cut, EditAction::SelectAll] {
        assert_eq!(perform(&mut empty, action, None), EditOutcome::Unavailable);
    }

    // Content but only a caret: select-all is available, copy and cut are not.
    let mut caret = ChipEditor::new("docs", &["docs"]);
    caret.type_text("abc");
    let state = availability(&caret, None);
    assert!(state.select_all && !state.copy && !state.cut);
    assert_eq!(
        perform(&mut caret, EditAction::Copy, None),
        EditOutcome::Unavailable
    );
    assert_eq!(
        perform(&mut caret, EditAction::SelectAll, None),
        EditOutcome::Changed
    );
    let state = availability(&caret, None);
    assert!(state.copy && state.cut);
}

#[test]
fn cut_removes_the_selection_and_returns_the_item_to_write() {
    let mut editor = editor_with_chips();
    let EditOutcome::WriteToClipboard(item) = perform(&mut editor, EditAction::Cut, None) else {
        panic!("cut writes to the clipboard");
    };
    assert_eq!(item.text().as_deref(), Some("see @main.rs ok"));
    assert!(editor.units().is_empty());
}

#[test]
fn paste_is_available_for_text_or_an_accepted_representation() {
    let mut editor = ChipEditor::new("docs", &["docs"]);
    let image = vespertine::Image::from_bytes(vespertine::ImageFormat::Png, vec![1]);
    let image_only = ClipboardItem::new_image(&image);
    // A text editor does not accept a bare image.
    assert!(!availability(&editor, Some(&image_only)).paste);
    assert!(availability(&editor, Some(&ClipboardItem::new_string("t".into()))).paste);
    // The owner that prefers media does.
    editor = editor.with_policy(vespertine::PastePolicy::media_first());
    assert!(availability(&editor, Some(&image_only)).paste);
    assert_eq!(
        perform(&mut editor, EditAction::Paste, Some(&image_only)),
        EditOutcome::Changed
    );
    assert_eq!(editor.attachments.len(), 1);
}

#[test]
fn media_first_falls_back_to_text_when_the_image_format_is_not_accepted() {
    let image = vespertine::Image::from_bytes(vespertine::ImageFormat::Tiff, vec![1]);
    let item = ClipboardItem::from_entries([
        vespertine::ClipboardEntry::String(vespertine::ClipboardString::new("caption".into())),
        vespertine::ClipboardEntry::Image(image),
    ]);
    let mut editor = ChipEditor::plain_field().with_policy(
        vespertine::PastePolicy::media_first().with_image_formats(&[vespertine::ImageFormat::Png]),
    );
    perform(&mut editor, EditAction::Paste, Some(&item));
    assert!(editor.attachments.is_empty());
    assert_eq!(editor.expanded_text(), "caption");
}
