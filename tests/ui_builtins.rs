//! Daily notes, templates, the startup note, bookmarks and file recovery.

use std::path::Path;

use adw::prelude::*;
use igneous_core::settings::Bookmark;
use jiff::civil::Date;

mod common;
use common::*;

/// A fresh fixture vault with `settings` as its vault.json.
fn vault_with(settings: &str) -> tempfile::TempDir {
    let dir = vault(None);
    std::fs::create_dir_all(dir.path().join(".igneous")).unwrap();
    std::fs::write(dir.path().join(".igneous/vault.json"), settings).unwrap();
    dir
}

fn write(dir: &Path, path: &str, text: &str) {
    let path = dir.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn read(dir: &Path, path: &str) -> String {
    std::fs::read_to_string(dir.join(path)).unwrap()
}

#[gtk::test]
async fn daily_notes_follow_the_folder_format_and_template() {
    let dir = vault_with(
        r#"{"version":1,
            "dailyNotes":{"folder":"Journal","format":"YYYY/YYYY-MM-DD","template":"Templates/Daily.md"},
            "templates":{"folder":"Templates"}}"#,
    );
    write(
        dir.path(),
        "Templates/Daily.md",
        "# {{title}}\n\n{{date:dddd}}\n",
    );
    let window = open(&dir);

    window.open_daily_note(Date::new(2026, 9, 1).unwrap());
    assert_eq!(
        read(dir.path(), "Journal/2026/2026-09-01.md"),
        "# 2026-09-01\n\nTuesday\n"
    );
    assert_eq!(
        window.selected_path(),
        Some(p("Journal/2026/2026-09-01.md"))
    );

    // Today's note, then back to the earlier one and forward again.
    WidgetExt::activate_action(&window, "win.daily-note", None).unwrap();
    let today = window.selected_path().unwrap();
    assert!(dir.path().join(today.as_str()).is_file());
    assert_ne!(today, p("Journal/2026/2026-09-01.md"));
    WidgetExt::activate_action(&window, "win.daily-note-previous", None).unwrap();
    assert_eq!(
        window.selected_path(),
        Some(p("Journal/2026/2026-09-01.md"))
    );
    WidgetExt::activate_action(&window, "win.daily-note-next", None).unwrap();
    assert_eq!(window.selected_path(), Some(today));
    window.close();
}

#[gtk::test]
async fn templates_insert_at_the_cursor_and_merge_properties() {
    let dir = vault_with(r#"{"version":1,"templates":{"folder":"Templates"}}"#);
    write(
        dir.path(),
        "Templates/Meeting.md",
        "---\ntags:\n  - meeting\nstatus: open\n---\n## {{title}} notes\n",
    );
    write(
        dir.path(),
        "Projects/Ideas.md",
        "---\ntags:\n  - idea\n---\n# Ideas\n\n",
    );
    let window = open(&dir);
    assert!(until(3000, || window.templates() == [p("Templates/Meeting.md")]).await);
    window.open_path(&p("Projects/Ideas.md"), false);
    let note = window.selected_note().unwrap();
    let before = note.text();
    let buffer = note.buffer();
    buffer.place_cursor(&buffer.end_iter());

    window.insert_template(&p("Templates/Meeting.md"));
    assert_eq!(
        note.text(),
        "---\ntags:\n  - idea\n  - meeting\nstatus: open\n---\n# Ideas\n\n## Ideas notes\n"
    );
    // One undo step takes it all back.
    buffer.undo();
    assert_eq!(note.text(), before);
    window.close();
}

#[gtk::test]
async fn the_startup_note_opens_with_the_vault() {
    let dir = vault_with(r#"{"version":1,"startupNote":"Projects/Ideas.md"}"#);
    let window = open(&dir);
    assert_eq!(window.selected_path(), Some(p("Projects/Ideas.md")));
    window.close();
}

#[gtk::test]
async fn bookmarks_toggle_persist_and_follow_renames() {
    let dir = vault(None);
    let window = open(&dir);
    window.open_path(&p("Home.md"), false);
    WidgetExt::activate_action(&window, "win.bookmark", None).unwrap();
    assert_eq!(
        window.bookmarks(),
        [Bookmark::File {
            path: p("Home.md"),
            title: None
        }]
    );
    assert!(read(dir.path(), ".igneous/bookmarks.json").contains("\"path\": \"Home.md\""));

    window.rename(&p("Home.md"), &p("Start.md"));
    assert!(window.is_bookmarked(&p("Start.md")));
    assert!(read(dir.path(), ".igneous/bookmarks.json").contains("\"path\": \"Start.md\""));

    // The bookmark survives reopening the vault.
    window.close();
    let window = open(&dir);
    assert!(window.is_bookmarked(&p("Start.md")));
    window.open_path(&p("Start.md"), false);
    WidgetExt::activate_action(&window, "win.bookmark", None).unwrap();
    assert!(window.bookmarks().is_empty());
    window.close();
}

#[gtk::test]
async fn snapshots_are_taken_listed_and_restored() {
    let dir = vault_with(r#"{"version":1,"recovery":{"intervalMinutes":0}}"#);
    let window = open(&dir);
    window.open_path(&p("Projects/Ideas.md"), false);
    let note = window.selected_note().unwrap();
    let buffer = note.buffer();
    buffer.insert(&mut buffer.end_iter(), "First.\n");
    note.flush();
    let first = note.text();
    wait(5).await;
    buffer.insert(&mut buffer.end_iter(), "Second.\n");
    note.flush();
    let snapshots = window.snapshots(&p("Projects/Ideas.md"));
    assert_eq!(snapshots.len(), 2);

    // Open the older one and put it back.
    window.open_snapshot(&p("Projects/Ideas.md"), &snapshots[1]);
    assert_eq!(window.selected_tab_text().as_deref(), Some(first.as_str()));
    WidgetExt::activate_action(&window, "win.restore-snapshot", None).unwrap();
    let note = window.selected_note().unwrap();
    assert_eq!(note.text(), first);
    assert_eq!(read(dir.path(), "Projects/Ideas.md"), first);
    note.buffer().undo();
    assert!(note.text().ends_with("Second.\n"));
    window.close();
}

/// Finds the first widget under `root` (depth first) that `pred` accepts.
fn find(root: &gtk::Widget, pred: &dyn Fn(&gtk::Widget) -> bool) -> Option<gtk::Widget> {
    if pred(root) {
        return Some(root.clone());
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        if let Some(found) = find(&c, pred) {
            return Some(found);
        }
        child = c.next_sibling();
    }
    None
}

/// Paints one widget (a popover's content, say) with the window's renderer.
fn save_widget(widget: &gtk::Widget, window: &gtk::Window, out: &str) {
    let paintable = gtk::WidgetPaintable::new(Some(widget));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, widget.width() as f64, widget.height() as f64);
    if let Some(node) = snapshot.to_node() {
        let texture = window.renderer().unwrap().render_texture(node, None);
        texture.save_to_png(out).unwrap();
    }
}

/// Screenshots for checking by eye: `IGNEOUS_SCREENSHOT=/tmp/x.png
/// build-aux/run-ui-tests.sh --test ui_builtins -- --ignored screenshot`.
#[gtk::test]
#[ignore = "visual check"]
async fn screenshot() {
    let Ok(out) = std::env::var("IGNEOUS_SCREENSHOT") else {
        return;
    };
    if std::env::var("IGNEOUS_SCREENSHOT_DARK").is_ok() {
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
    }
    let dir = vault_with(
        r#"{"version":1,"recovery":{"intervalMinutes":0},"templates":{"folder":"Templates"}}"#,
    );
    write(dir.path(), "Templates/Meeting.md", "## {{title}}\n");
    let window = open(&dir);
    window.set_default_size(1100, 720);
    window.open_daily_note(Date::new(2026, 10, 1).unwrap());
    window.open_path(&p("Home.md"), false);
    WidgetExt::activate_action(&window, "win.bookmark", None).unwrap();
    let note = window.selected_note().unwrap();
    note.set_cursor_byte(note.text().find("Welcome").unwrap());
    WidgetExt::activate_action(&window, "win.bookmark-heading", None).unwrap();
    window.search_vault("tag:#home");
    WidgetExt::activate_action(&window, "win.bookmark-search", None).unwrap();
    WidgetExt::activate_action(&window, "win.show-bookmarks", None).unwrap();
    let buffer = note.buffer();
    for line in ["One more line.\n", "And another.\n"] {
        buffer.insert(&mut buffer.end_iter(), line);
        note.flush();
        wait(5).await;
    }
    wait(1000).await;
    save_png(window.upcast_ref(), &out.replace(".png", "-bookmarks.png"));

    // The calendar popover in the sidebar header.
    let root: gtk::Widget = window.clone().upcast();
    let button = find(&root, &|w| {
        w.downcast_ref::<gtk::MenuButton>()
            .is_some_and(|b| b.tooltip_text().as_deref() == Some("Daily Notes"))
    })
    .and_downcast::<gtk::MenuButton>()
    .unwrap();
    button.popup();
    wait(800).await;
    if let Some(content) = button.popover().and_then(|p| p.child()) {
        save_widget(
            &content,
            window.upcast_ref(),
            &out.replace(".png", "-calendar.png"),
        );
    }
    button.popdown();

    window.show_snapshots(&p("Home.md"));
    wait(800).await;
    save_png(window.upcast_ref(), &out.replace(".png", "-snapshots.png"));
    if let Some(dialog) = window.visible_dialog() {
        dialog.close();
    }
    let snapshots = window.snapshots(&p("Home.md"));
    window.open_snapshot(&p("Home.md"), snapshots.last().unwrap());
    wait(800).await;
    save_png(window.upcast_ref(), &out.replace(".png", "-snapshot.png"));

    WidgetExt::activate_action(&window, "win.preferences", None).unwrap();
    wait(500).await;
    if let Some(prefs) = window
        .visible_dialog()
        .and_downcast::<adw::PreferencesDialog>()
    {
        prefs.set_visible_page_name("plugins");
    }
    wait(800).await;
    save_png(window.upcast_ref(), &out.replace(".png", "-prefs.png"));
    window.close();
}
