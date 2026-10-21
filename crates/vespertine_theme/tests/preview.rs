//! THM-004 R8: the preview / cancel / commit transaction.

mod support;

use std::sync::{Mutex, MutexGuard};
use support::*;
use vespertine::{AppContext as _, TestAppContext};
use vespertine_theme::{
    appearance::{self, style_generation, theme_of},
    import::Compilation,
    library::{self, InstallMode, Library},
    preview::{self, PreviewInfo},
    selection::{AccentSelection, AppearanceMode, SurfacePreference, ThemeSelection},
    theme::DEFAULT_DARK_ID,
};

static GENERATION_LOCK: Mutex<()> = Mutex::new(());

fn serialize() -> MutexGuard<'static, ()> {
    GENERATION_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

fn init(cx: &mut TestAppContext) {
    cx.update(|cx| {
        appearance::init(
            cx,
            AppearanceMode::Dark,
            ThemeSelection::default(),
            AccentSelection::ThemeDefault,
            SurfacePreference::ThemeDefault,
        )
    });
}

fn candidate(dir: &std::path::Path, family: &str, background: &str) -> Compilation {
    let mut colors = good_dark_colors();
    colors["editor.background"] = serde_json::json!(background);
    let path = dir.join(format!("{family}.json"));
    write_json(&path, &theme_json(colors));
    compile(&path, family).unwrap()
}

struct Owner;

#[vespertine::test]
fn preview_and_cancel_swap_the_active_theme_without_touching_state(cx: &mut TestAppContext) {
    let _guard = serialize();
    init(cx);
    let source = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let library = Library::load(data.path());
    let compilation = candidate(source.path(), "cand", "#223344");
    let committed = cx.update(|cx| theme_of(cx).variant.clone());
    let generation = style_generation();

    let info = cx
        .update(|cx| preview::preview(cx, &compilation.family, None))
        .unwrap();
    assert_eq!(
        info,
        PreviewInfo {
            family_id: "cand".into(),
            variant_id: "cand".into()
        }
    );
    cx.update(|cx| {
        assert!(preview::is_previewing(cx));
        assert_eq!(preview::preview_info(cx), Some(info.clone()));
        assert!(theme_of(cx).is_preview);
        assert_eq!(theme_of(cx).id, "cand");
        assert_eq!(theme_of(cx).colors.background, hex("#223344"));
        // Registry, persisted selection and library are untouched.
        assert!(appearance::registry(cx).variant("cand").is_none());
        assert_eq!(appearance::selection(cx), &ThemeSelection::default());
    });
    assert!(library.entries().is_empty());
    assert!(!data.path().join("theme-library.json").exists());
    assert_eq!(style_generation(), generation.wrapping_add(1));

    assert!(cx.update(|cx| preview::cancel(cx)));
    cx.update(|cx| {
        assert!(!preview::is_previewing(cx));
        assert!(!theme_of(cx).is_preview);
        assert_eq!(theme_of(cx).variant, committed);
    });
    assert_eq!(style_generation(), generation.wrapping_add(2));
    assert!(!cx.update(|cx| preview::cancel(cx)), "cancel is idempotent");

    // Previewing a specific variant, then previewing again.
    let variant_id = compilation.family.variants[0].id.clone();
    cx.update(|cx| preview::preview(cx, &compilation.family, Some(&variant_id)))
        .unwrap();
    assert!(
        cx.update(|cx| preview::preview(cx, &compilation.family, Some("missing")))
            .is_err()
    );
    assert!(
        cx.update(|cx| preview::is_previewing(cx)),
        "a rejected preview leaves the current one"
    );
}

#[vespertine::test]
fn an_invalid_candidate_is_rejected_and_the_active_theme_is_untouched(cx: &mut TestAppContext) {
    let _guard = serialize();
    init(cx);
    let source = tempfile::tempdir().unwrap();
    let mut compilation = candidate(source.path(), "bad", "#223344");
    compilation.family.variants[0].source.revision.clear();
    let before = cx.update(|cx| theme_of(cx).variant.clone());
    let generation = style_generation();
    let error = cx
        .update(|cx| preview::preview(cx, &compilation.family, None))
        .unwrap_err();
    assert!(error.0.contains("cannot be previewed"), "{error}");
    assert_eq!(cx.update(|cx| theme_of(cx).variant.clone()), before);
    assert_eq!(style_generation(), generation);
    assert!(!cx.update(|cx| preview::is_previewing(cx)));
}

#[vespertine::test]
fn commit_installs_selects_and_ends_the_preview_in_one_resolution(cx: &mut TestAppContext) {
    let _guard = serialize();
    init(cx);
    let source = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut library = Library::load(data.path());
    let compilation = candidate(source.path(), "cand", "#223344");
    cx.update(|cx| preview::preview(cx, &compilation.family, None))
        .unwrap();
    let generation = style_generation();

    let id = cx
        .update(|cx| {
            preview::commit(
                cx,
                &mut library,
                &compilation,
                &[],
                InstallMode::Link,
                Some("cand"),
            )
        })
        .unwrap();
    assert_eq!(id, "cand");
    // One resolution: a single generation step covers ending the preview, the registry
    // install and the selection.
    assert_eq!(style_generation(), generation.wrapping_add(1));
    cx.update(|cx| {
        assert!(!preview::is_previewing(cx));
        assert!(!theme_of(cx).is_preview);
        assert_eq!(theme_of(cx).id, "cand");
        assert!(appearance::registry(cx).variant("cand").is_some());
        assert_eq!(appearance::selection(cx).dark, "cand");
        assert_eq!(appearance::selection(cx).light, "default-light");
    });
    assert_eq!(library.entries().len(), 1);
    assert!(data.path().join("theme-library.json").exists());
}

#[vespertine::test]
fn commit_failures_leave_the_preview_and_library_untouched(cx: &mut TestAppContext) {
    let _guard = serialize();
    init(cx);
    let source = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut library = Library::load(data.path());
    let mut compilation = candidate(source.path(), "cand", "#223344");
    cx.update(|cx| preview::preview(cx, &compilation.family, None))
        .unwrap();
    let generation = style_generation();
    compilation.family.variants[0].source.license.clear();
    let error = cx
        .update(|cx| {
            preview::commit(
                cx,
                &mut library,
                &compilation,
                &[],
                InstallMode::Snapshot,
                None,
            )
        })
        .unwrap_err();
    assert!(error.0.contains("validation failed"), "{error}");
    assert!(library.entries().is_empty());
    cx.update(|cx| {
        assert!(
            preview::is_previewing(cx),
            "the preview survives a failed commit"
        );
        assert_eq!(theme_of(cx).id, "cand");
    });
    assert_eq!(style_generation(), generation);
}

#[vespertine::test]
fn commit_with_a_suffixed_identifier_selects_the_rekeyed_variant(cx: &mut TestAppContext) {
    let _guard = serialize();
    init(cx);
    let source = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut library = Library::load(data.path());
    let first = candidate(source.path(), "cand", "#223344");
    cx.update(|cx| preview::commit(cx, &mut library, &first, &[], InstallMode::Snapshot, None))
        .unwrap();
    let second = candidate(source.path(), "cand", "#334455");
    let id = cx
        .update(|cx| {
            preview::commit(
                cx,
                &mut library,
                &second,
                &[],
                InstallMode::Snapshot,
                Some("cand"),
            )
        })
        .unwrap();
    assert_eq!(id, "cand-2");
    cx.update(|cx| {
        assert_eq!(appearance::selection(cx).dark, "cand-2");
        assert_eq!(theme_of(cx).colors.background, hex("#334455"));
    });
    assert_eq!(
        library::rekeyed_variant_id("cand", "cand-2", "cand-cand"),
        "cand-2-cand"
    );
    assert_eq!(library::rekeyed_variant_id("x", "y", "other"), "y-other");
}

#[vespertine::test]
fn closing_the_owner_cancels_the_preview(cx: &mut TestAppContext) {
    let _guard = serialize();
    init(cx);
    let source = tempfile::tempdir().unwrap();
    let compilation = candidate(source.path(), "cand", "#223344");
    let owner = cx.new(|_| Owner);
    let subscription = cx.update(|cx| preview::cancel_on_release(cx, &owner));
    cx.update(|cx| preview::preview(cx, &compilation.family, None))
        .unwrap();
    assert!(cx.update(|cx| preview::is_previewing(cx)));
    drop(owner);
    cx.update(|_| {});
    assert_eq!(cx.update(|cx| theme_of(cx).id.clone()), DEFAULT_DARK_ID);
    assert!(!cx.update(|cx| preview::is_previewing(cx)));
    drop(subscription);
}
