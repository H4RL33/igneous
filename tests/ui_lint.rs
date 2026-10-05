//! UI tests for the linter: linting a note, on save, a whole vault, the
//! preferences page and the problems underlined in the editor. Run with
//! `build-aux/run-ui-tests.sh`.

use adw::prelude::*;
use igneous::{NoteState, Window};

mod common;
use common::*;

fn write(dir: &tempfile::TempDir, path: &str, text: &str) {
    let abs = dir.path().join(path);
    std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
    std::fs::write(abs, text).unwrap();
}

fn read(dir: &tempfile::TempDir, path: &str) -> String {
    std::fs::read_to_string(dir.path().join(path)).unwrap()
}

fn lint_json(dir: &tempfile::TempDir, json: &str) {
    write(dir, ".igneous/lint.json", json);
}

const MESSY: &str = "Hello   \n\n\n\nWorld  ";

#[gtk::test]
async fn lint_note_is_one_undo_step() {
    let dir = vault(None);
    write(&dir, "Messy.md", MESSY);
    let window = open(&dir);
    window.open_path(&p("Messy.md"), false);
    let note = window.selected_note().unwrap();
    WidgetExt::activate_action(&window, "win.lint-note", None).unwrap();
    // The default rules: trailing spaces, blank lines, final newline.
    assert_eq!(note.text(), "Hello\n\nWorld\n");
    note.buffer().undo();
    assert_eq!(note.text(), MESSY);
    note.discard();
    window.close();
}

#[gtk::test]
async fn ctrl_s_lints_when_asked_and_autosave_never_does() {
    let dir = vault(Some(100));
    lint_json(
        &dir,
        r#"{"version":1,"lintOnSave":true,"rules":{"trailing-spaces":{"enabled":true}}}"#,
    );
    write(&dir, "Note.md", "one\n");
    let window = open(&dir);
    window.open_path(&p("Note.md"), false);
    let note = window.selected_note().unwrap();
    let buffer = note.buffer();
    buffer.insert(&mut buffer.end_iter(), "two   \n");
    // Autosave keeps the spaces.
    assert!(until(2000, || note.state() == NoteState::Clean).await);
    assert_eq!(read(&dir, "Note.md"), "one\ntwo   \n");
    // Ctrl+S lints, then saves.
    WidgetExt::activate_action(&window, "win.save", None).unwrap();
    assert_eq!(read(&dir, "Note.md"), "one\ntwo\n");
    window.close();
}

#[gtk::test]
async fn linting_the_vault_skips_unsaved_notes() {
    let dir = vault(Some(60_000));
    lint_json(
        &dir,
        r#"{"version":1,"rules":{"trailing-spaces":{"enabled":true}},"ignoreFolders":["Ignored"]}"#,
    );
    write(&dir, "Closed.md", "closed  \n");
    write(&dir, "Open.md", "open  \n");
    write(&dir, "Ignored/Skip.md", "skip  \n");
    let window = open(&dir);
    window.open_path(&p("Open.md"), false);
    let note = window.selected_note().unwrap();
    let buffer = note.buffer();
    buffer.insert(&mut buffer.end_iter(), "unsaved  \n");
    assert_eq!(note.state(), NoteState::Dirty);

    window.lint_paths(vec![p("Closed.md"), p("Open.md"), p("Ignored/Skip.md")]);
    assert!(until(3000, || read(&dir, "Closed.md") == "closed\n").await);
    assert_eq!(read(&dir, "Open.md"), "open  \n");
    assert_eq!(read(&dir, "Ignored/Skip.md"), "skip  \n");
    note.discard();
    window.close();
}

/// Every widget under `widget`, depth first.
fn descendants(widget: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut out = Vec::new();
    let mut child = widget.first_child();
    while let Some(c) = child {
        out.push(c.clone());
        out.extend(descendants(&c));
        child = c.next_sibling();
    }
    out
}

fn preferences(window: &Window) -> adw::PreferencesDialog {
    descendants(window.upcast_ref())
        .into_iter()
        .find_map(|w| w.downcast::<adw::PreferencesDialog>().ok())
        .expect("the preferences dialog is open")
}

#[gtk::test]
async fn preferences_round_trip_lint_json() {
    let dir = vault(None);
    lint_json(
        &dir,
        r#"{"version":1,"rules":{"trailing-spaces":{"enabled":true}},"customKey":42}"#,
    );
    let window = open(&dir);
    WidgetExt::activate_action(&window, "win.preferences", None).unwrap();
    wait(300).await;
    let prefs = preferences(&window);
    // The rules are on a page of their own, opened from the Linter section.
    descendants(prefs.upcast_ref())
        .into_iter()
        .find_map(|w| {
            w.downcast::<adw::ActionRow>()
                .ok()
                .filter(|r| r.title() == "Rules")
        })
        .expect("a Rules row")
        .emit_by_name::<()>("activated", &[]);
    wait(300).await;
    let widgets = descendants(prefs.upcast_ref());
    let switch = |title: &str| {
        widgets
            .iter()
            .find_map(|w| {
                w.downcast_ref::<adw::SwitchRow>()
                    .filter(|r| r.title() == title)
                    .cloned()
            })
            .unwrap_or_else(|| panic!("no “{title}” switch"))
    };
    switch("Lint When Saving").set_active(true);
    // A rule without options is a switch, named after its ID.
    let rule = widgets
        .iter()
        .find(|w| w.widget_name() == "remove-hyphenated-line-breaks")
        .and_then(|w| w.downcast_ref::<adw::SwitchRow>())
        .expect("a switch for the rule")
        .clone();
    rule.set_active(true);
    // A rule with options is an expander with an enable switch.
    let expander = widgets
        .iter()
        .find(|w| w.widget_name() == "emphasis-style")
        .and_then(|w| w.downcast_ref::<adw::ExpanderRow>())
        .expect("an expander for the rule")
        .clone();
    expander.set_enable_expansion(true);

    let saved: serde_json::Value = serde_json::from_str(&read(&dir, ".igneous/lint.json")).unwrap();
    assert_eq!(saved["lintOnSave"], true);
    assert_eq!(
        saved["rules"]["remove-hyphenated-line-breaks"]["enabled"],
        true
    );
    assert_eq!(saved["rules"]["emphasis-style"]["enabled"], true);
    assert_eq!(saved["rules"]["trailing-spaces"]["enabled"], true);
    assert_eq!(saved["customKey"], 42);
    assert!(window.lint().settings().lint_on_save);
    prefs.close();
    window.close();
}

#[gtk::test]
async fn problems_are_underlined_and_fixed() {
    let dir = vault(None);
    lint_json(
        &dir,
        r#"{"version":1,"showProblems":true,"rules":{"trailing-spaces":{"enabled":true}}}"#,
    );
    write(&dir, "Spaces.md", "a  \nb\n");
    let window = open(&dir);
    window.open_path(&p("Spaces.md"), false);
    let note = window.selected_note().unwrap();
    let view = note.view();
    assert!(until(3000, || !view.diagnostics().is_empty()).await);
    let found = view.diagnostics();
    assert_eq!(found[0].rule, "trailing-spaces");
    assert_eq!(found[0].range, 1..3);
    // What the Fix button does.
    window.fix_rule(&note, "trailing-spaces");
    assert_eq!(note.text(), "a\nb\n");
    assert!(until(3000, || view.diagnostics().is_empty()).await);
    note.flush();
    window.close();
}

#[gtk::test]
async fn invalid_yaml_is_always_underlined() {
    let dir = vault(None);
    lint_json(&dir, r#"{"version":1,"rules":{}}"#);
    write(&dir, "Broken.md", "---\ntags: [a\n---\ntext  \n");
    let window = open(&dir);
    window.open_path(&p("Broken.md"), false);
    let view = window.selected_note().unwrap().view();
    assert!(until(3000, || !view.diagnostics().is_empty()).await);
    let found = view.diagnostics();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].rule, "yaml");
    window.close();
}

/// Not a test: screenshots for checking the linter's looks by eye.
/// `IGNEOUS_SCREENSHOT=/path/out.png build-aux/run-ui-tests.sh --test ui_lint -- --ignored`
#[gtk::test]
#[ignore = "visual check"]
async fn lint_screenshot() {
    let Ok(out) = std::env::var("IGNEOUS_SCREENSHOT") else {
        return;
    };
    let dir = vault(None);
    lint_json(
        &dir,
        r#"{"version":1,"showProblems":true,"rules":{"trailing-spaces":{"enabled":true},"remove-multiple-spaces":{"enabled":true},"heading-blank-lines":{"enabled":true}}}"#,
    );
    write(
        &dir,
        "Untidy.md",
        "---\ntags: [a\n---\n# Heading\nText with  double   spaces.   \nMore text.\n",
    );
    let window = open(&dir);
    window.set_default_size(1000, 640);
    window.open_path(&p("Untidy.md"), false);
    let note = window.selected_note().unwrap();
    note.view().set_mode(igneous_editor::Mode::Source);
    wait(1500).await;
    save_png(window.upcast_ref(), &out);
    WidgetExt::activate_action(&window, "win.preferences", None).unwrap();
    wait(500).await;
    let prefs = preferences(&window);
    prefs.set_visible_page_name("");
    if let Some(page) = descendants(prefs.upcast_ref()).into_iter().find_map(|w| {
        w.downcast::<adw::PreferencesPage>()
            .ok()
            .filter(|p| p.title() == "Plugins")
    }) {
        prefs.set_visible_page(&page);
    }
    wait(500).await;
    save_png(
        window.upcast_ref(),
        &out.replace(".png", "-preferences.png"),
    );
    window.lint_folder_dialog(None);
    wait(500).await;
    save_png(window.upcast_ref(), &out.replace(".png", "-dialog.png"));
    window.close();
}

/// Linting keeps the cursor where it was. With a change near the top (a
/// blank line after the frontmatter) and another further down, replacing
/// everything between them threw the cursor down to the second change, off
/// screen, so typing seemed to do nothing.
#[gtk::test]
async fn linting_keeps_the_cursor_on_its_heading() {
    let dir = vault(None);
    lint_json(
        &dir,
        r#"{"version":1,"lintOnSave":true,"rules":{
            "header-increment":{"enabled":true,"options":{"startAtH2":true}},
            "add-blank-line-after-yaml":{"enabled":true},
            "heading-blank-lines":{"enabled":true},
            "trailing-spaces":{"enabled":true},
            "remove-multiple-spaces":{"enabled":true}}}"#,
    );
    let text = "---\ntags: [a]\n---\n# Title\nSome  text.   \n## Part\nMore.\n";
    let window = open(&dir);
    // The cursor in the title, and at the start of its line.
    for (cursor, column) in [
        (text.find("Title").unwrap() + 2, 5),
        (text.find("# Title").unwrap(), 0),
    ] {
        write(&dir, "Heading.md", text);
        window.open_path(&p("Heading.md"), false);
        let note = window.selected_note().unwrap();
        note.reload();
        wait(200).await;
        note.set_cursor_byte(cursor);
        WidgetExt::activate_action(&window, "win.save", None).unwrap();
        let new = note.text();
        assert!(new.contains("\n## Title\n"), "{new:?}");
        let at = note.cursor_offset() as usize;
        let byte = new.char_indices().nth(at).map_or(new.len(), |(b, _)| b);
        let line_start = new[..byte].rfind('\n').map_or(0, |i| i + 1);
        let line_end = new[byte..].find('\n').map_or(new.len(), |i| byte + i);
        assert_eq!(&new[line_start..line_end], "## Title");
        assert_eq!(byte - line_start, column);
        note.view().check_invariants().unwrap();
        note.discard();
    }
    window.close();
}
