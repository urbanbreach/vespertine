//! Rich clipboard: scoped structured copy and paste, and owner paste policies (PLT-009).
//!
//! An editor with chips copies text with scoped metadata. The item is pasted into an editor of
//! a compatible scope (chips restored), an editor of an incompatible scope and a plain field
//! (plain expanded text). Then an item holding an image plus text is pasted into an owner that
//! prefers media (the image is used) and one that prefers text.
//!
//! Run with `cargo run -p vespertine --example rich_clipboard`. No window is needed: the
//! clipboard item produced by the copy is handed to each paste, exactly what
//! `App::read_from_clipboard` returns on a platform.

#[path = "example_support/chip_editor.rs"]
mod chip_editor;

use chip_editor::run_scenario;

fn main() {
    let scenario = run_scenario();

    println!("copied text:       {:?}", scenario.copied.text().unwrap());
    println!(
        "copied metadata:   {}",
        scenario.copied.metadata().map_or("none", String::as_str)
    );
    println!();
    println!("compatible scope:  {}", scenario.compatible.display());
    println!("incompatible:      {}", scenario.incompatible.display());
    println!("plain field:       {}", scenario.plain.display());
    println!();
    println!(
        "image + text -> media first: text {:?}, attachments {:?}",
        scenario.media_first.expanded_text(),
        scenario.media_first.attachments
    );
    println!(
        "image + text -> text first:  text {:?}, attachments {:?}",
        scenario.text_first.expanded_text(),
        scenario.text_first.attachments
    );

    assert_eq!(scenario.compatible.chips().len(), 2);
    assert!(scenario.incompatible.chips().is_empty());
    assert!(scenario.plain.chips().is_empty());
    assert_eq!(
        scenario.incompatible.expanded_text(),
        scenario.copied.text().unwrap()
    );
    assert_eq!(scenario.media_first.attachments.len(), 1);
    assert_eq!(scenario.media_first.expanded_text(), "");
    assert_eq!(scenario.text_first.expanded_text(), "a photo");
}
