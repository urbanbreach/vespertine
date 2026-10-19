//! CMP-003 A3: an editable modal above selected document text.
//!
//! A read-only "document" field holds a selection ("quick"). Opening the dialog (the Rename
//! button) puts a modal with its own editable field above it. While the modal is open:
//!
//! * Ctrl-A selects the modal's text and Ctrl-C copies it: copy and select-all act on the modal's
//!   editor (through the focus-scoped action context), never on the document's selection, which
//!   stays "quick";
//! * a press on the document is a press on the backdrop: the modal closes, the press never
//!   reaches the document and its selection is intact;
//! * closing (Escape, Cancel or the backdrop) returns focus to the document element that had it.
//!
//! Enter in the modal's single-line field is the dialog's default action (the primary Rename
//! action). Without arguments the example runs offscreen on the production test host with real
//! input and exits 0 when every check holds (1 otherwise); with `--interactive` it opens a window.

mod controls_common;

use std::{cell::RefCell, rc::Rc};

use controls_common::{Build, Checks, run_window, scripted_host};
use vespertine::{
    AppContext as _, Entity, Focusable as _, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Role, StatefulInteractiveElement, Styled, div, px, rgb,
};
use vespertine_components::dialog::{ActionKind, Dialog, DialogAction, DialogModel};
use vespertine_components::floating::FocusTarget;
use vespertine_components::text::TextField;
use vespertine_test_host::TestHost;

const DOCUMENT_TEXT: &str = "The quick brown fox jumps over the lazy dog";
const NEW_NAME: &str = "Fox story";

struct Parts {
    document: Entity<TextField>,
    editor: Entity<TextField>,
    dialog: Entity<Dialog>,
}

fn parts(
    slot: &Rc<RefCell<Option<Parts>>>,
    window: &mut vespertine::Window,
    cx: &mut vespertine::App,
) -> (Entity<TextField>, Entity<Dialog>) {
    let mut slot = slot.borrow_mut();
    let parts = slot.get_or_insert_with(|| {
        let document = cx.new(|cx| {
            let mut field = TextField::new(window, cx)
                .value(DOCUMENT_TEXT)
                .read_only(true);
            field.set_selected_range(4..9, false, window, cx);
            field
        });
        let editor = cx.new(|cx| TextField::new(window, cx).value(NEW_NAME));
        let editor_focus = editor.read(cx).focus_handle(cx);
        let dialog = cx.new(|cx| {
            Dialog::new("rename", "Rename document", cx)
                .body("Pick a new name for the document.")
                .child(editor.clone())
                // Enter in the single-line field is the dialog's default action (R12).
                .default_action_field(&editor, window, cx)
                .initial_focus(FocusTarget::Handle(editor_focus))
                .model(DialogModel::new(vec![
                    DialogAction::new("cancel", "Cancel", ActionKind::Secondary),
                    DialogAction::new("rename", "Rename", ActionKind::Primary),
                ]))
        });
        Parts {
            document,
            editor,
            dialog,
        }
    });
    (parts.document.clone(), parts.dialog.clone())
}

fn build(slot: &Rc<RefCell<Option<Parts>>>) -> Build {
    let slot = slot.clone();
    Rc::new(move |window, cx| {
        let (document, dialog) = parts(&slot, window, cx);
        let opener = dialog.clone();
        div()
            .size_full()
            .relative()
            .child(
                div()
                    .absolute()
                    .left(px(20.))
                    .top(px(20.))
                    .w(px(400.))
                    .child(document),
            )
            .child(
                div()
                    .id("rename-button")
                    .role(Role::Button)
                    .aria_label("Rename")
                    .absolute()
                    .left(px(20.))
                    .top(px(70.))
                    .w(px(100.))
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .bg(rgb(0x2B3038))
                    .child("Rename\u{2026}")
                    // A toolbar button does not take focus from the document.
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .on_click(move |_, window, cx| {
                        opener.update(cx, |dialog, cx| dialog.open(window, cx))
                    }),
            )
            .child(dialog)
            .into_any_element()
    })
}

fn settle(host: &mut TestHost) {
    host.flush();
    for _ in 0..3 {
        let window = host.window();
        host.app()
            .update_window(window, |_, window, cx| window.simulate_next_frame(cx))
            .unwrap();
        host.flush();
    }
    host.advance_executor_time(300.).unwrap();
    host.flush();
}

fn main() {
    let slot = Rc::new(RefCell::new(None));
    if std::env::args().any(|argument| argument == "--interactive") {
        run_window(460.0, 320.0, build(&slot));
        return;
    }
    let mut host = match scripted_host(460.0, 320.0, build(&slot)) {
        Ok(host) => host,
        Err(reason) => {
            println!(
                "dialog_modal_editor: no offscreen host here ({reason}); run with --interactive"
            );
            return;
        }
    };
    let mut checks = Checks::default();
    host.flush();
    let (document, dialog, editor) = {
        let parts = slot.borrow();
        let parts = parts
            .as_ref()
            .expect("the parts exist after the first frame");
        (
            parts.document.clone(),
            parts.dialog.clone(),
            parts.editor.clone(),
        )
    };

    // The document holds focus and a selection.
    let window = host.window();
    let doc_focus = host.app().update(|cx| document.read(cx).focus_handle(cx));
    host.app()
        .update_window(window, |_, window, cx| doc_focus.focus(window, cx))
        .unwrap();
    settle(&mut host);
    let doc_selection =
        |host: &mut TestHost| host.app().update(|cx| document.read(cx).selected_range());
    checks.check(
        doc_selection(&mut host) == (4..9),
        "the document selects \"quick\"",
    );

    // Open the modal.
    host.click(70., 85., 0, "").unwrap();
    settle(&mut host);
    let is_open = |host: &mut TestHost| host.app().update(|cx| dialog.read(cx).is_open());
    checks.check(is_open(&mut host), "the Rename button opens the modal");
    let editor_focus = host.app().update(|cx| editor.read(cx).focus_handle(cx));
    let focused = {
        let window = host.window();
        host.app()
            .update_window(window, |_, window, _| editor_focus.is_focused(window))
            .unwrap()
    };
    checks.check(focused, "focus moved into the modal's editor");

    // Select-all and copy act on the modal's editor.
    host.simulate_keystrokes("ctrl-a", None).unwrap();
    settle(&mut host);
    let editor_selection = host.app().update(|cx| editor.read(cx).selected_range());
    checks.check(
        editor_selection == (0..NEW_NAME.len()),
        "Ctrl-A selects the modal editor's text",
    );
    checks.check(
        doc_selection(&mut host) == (4..9),
        "the background selection did not change",
    );
    host.simulate_keystrokes("ctrl-c", None).unwrap();
    settle(&mut host);
    let copied = host
        .app()
        .update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    checks.check(
        copied.as_deref() == Some(NEW_NAME),
        format!("Ctrl-C copies the modal editor's text, not the document's ({copied:?})"),
    );

    // A press on the document is a backdrop press: it closes the modal and never reaches the
    // document, whose selection stays.
    host.click(150., 36., 0, "").unwrap();
    settle(&mut host);
    checks.check(
        !is_open(&mut host),
        "a press on the backdrop closes the modal",
    );
    checks.check(
        doc_selection(&mut host) == (4..9),
        "the press did not change the background selection",
    );
    let restored = {
        let window = host.window();
        host.app()
            .update_window(window, |_, window, _| doc_focus.is_focused(window))
            .unwrap()
    };
    checks.check(
        restored,
        "closing restored focus to the previously focused document element",
    );

    // Escape closes too, and Enter in the field is the default action.
    host.click(70., 85., 0, "").unwrap();
    settle(&mut host);
    host.simulate_keystrokes("escape", None).unwrap();
    settle(&mut host);
    checks.check(!is_open(&mut host), "Escape closes the modal");
    host.click(70., 85., 0, "").unwrap();
    settle(&mut host);
    host.simulate_keystrokes("enter", None).unwrap();
    settle(&mut host);
    checks.check(
        !is_open(&mut host),
        "Enter in the single-line field runs the primary action and closes the dialog",
    );
    checks.finish("dialog_modal_editor");
}
