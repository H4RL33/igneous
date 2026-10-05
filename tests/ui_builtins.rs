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

/// The icon names a picker's grid shows.
fn grid_icons(grid: &gtk::GridView) -> Vec<String> {
    let model = grid.model().unwrap();
    (0..model.n_items())
        .filter_map(|i| model.item(i).and_downcast::<gtk::StringObject>())
        .map(|s| s.string().to_string())
        .collect()
}

#[gtk::test]
async fn note_icons_are_picked_saved_and_follow_renames() {
    let dir = vault_with(r#"{"version":1,"files":{"trash":"vaultFolder","confirmDelete":false}}"#);
    write(
        dir.path(),
        ".igneous/icons.json",
        r#"{"version":1,"fromTheFuture":true}"#,
    );
    let window = open(&dir);
    window.open_path(&p("Home.md"), false);
    assert_eq!(window.tab_icon(&p("Home.md")), None);

    // Search the picker, then choose the first match.
    WidgetExt::activate_action(&window, "win.set-icon", None).unwrap();
    let dialog = window.visible_dialog().unwrap();
    let grid = find(dialog.upcast_ref(), &|w| w.is::<gtk::GridView>())
        .and_downcast::<gtk::GridView>()
        .unwrap();
    let all = grid_icons(&grid);
    assert!(all.iter().all(|name| name.ends_with("-symbolic")));
    assert!(all.windows(2).all(|pair| pair[0] < pair[1]));
    let entry = find(dialog.upcast_ref(), &|w| w.is::<gtk::SearchEntry>())
        .and_downcast::<gtk::SearchEntry>()
        .unwrap();
    entry.set_text("folder");
    assert!(until(2000, || grid_icons(&grid).len() < all.len()).await);
    let found = grid_icons(&grid);
    assert!(!found.is_empty());
    assert!(
        found.iter().all(|name| name.contains("folder")),
        "{found:?}"
    );
    let icon = found[0].clone();
    grid.emit_by_name::<()>("activate", &[&0u32]);
    assert!(until(2000, || window.visible_dialog().is_none()).await);

    assert_eq!(window.note_icon(&p("Home.md")), Some(icon.clone()));
    let json = read(dir.path(), ".igneous/icons.json");
    assert!(json.contains(&format!("\"Home.md\": \"{icon}\"")), "{json}");
    assert!(json.contains("\"fromTheFuture\": true"), "{json}");
    assert_eq!(window.sidebar_icon(&p("Home.md")), Some(icon.clone()));
    assert_eq!(window.tab_icon(&p("Home.md")), Some(icon.clone()));

    // Renaming the note moves its icon.
    window.rename(&p("Home.md"), &p("Start.md"));
    let json = read(dir.path(), ".igneous/icons.json");
    assert!(
        json.contains(&format!("\"Start.md\": \"{icon}\"")),
        "{json}"
    );
    assert!(!json.contains("Home.md"), "{json}");
    assert_eq!(window.sidebar_icon(&p("Start.md")), Some(icon.clone()));
    assert_eq!(window.tab_icon(&p("Start.md")), Some(icon.clone()));

    // So does renaming its folder; moving the folder to the trash drops it.
    window.set_note_icon(&p("Projects/Ideas.md"), Some(&icon));
    window.rename(&p("Projects"), &p("Work"));
    assert_eq!(window.note_icon(&p("Projects/Ideas.md")), None);
    assert_eq!(window.note_icon(&p("Work/Ideas.md")), Some(icon.clone()));
    window.trash(p("Work"));
    assert_eq!(window.note_icon(&p("Work/Ideas.md")), None);
    assert!(!read(dir.path(), ".igneous/icons.json").contains("Ideas.md"));

    // Reset to Default, from the file tree's menu.
    WidgetExt::activate_action(&window, "win.file-set-icon", Some(&"Start.md".to_variant()))
        .unwrap();
    let dialog = window.visible_dialog().unwrap();
    let reset = find(dialog.upcast_ref(), &|w| {
        w.downcast_ref::<gtk::Button>()
            .is_some_and(|b| b.label().as_deref() == Some("_Reset to Default"))
    })
    .and_downcast::<gtk::Button>()
    .unwrap();
    reset.emit_clicked();
    assert_eq!(window.note_icon(&p("Start.md")), None);
    assert_eq!(
        window.sidebar_icon(&p("Start.md")).as_deref(),
        Some("text-x-generic-symbolic")
    );
    assert_eq!(window.tab_icon(&p("Start.md")), None);
    assert!(!read(dir.path(), ".igneous/icons.json").contains("Start.md"));
    window.close();
}

/// A colour is a setting of its own: a folder can have one without a custom
/// icon. It's saved, shown in the tree, follows renames and goes with the
/// folder to the trash.
#[gtk::test]
async fn folder_colours_are_saved_shown_and_follow_renames() {
    let dir = vault_with(r#"{"version":1,"files":{"trash":"vaultFolder","confirmDelete":false}}"#);
    let window = open(&dir);
    let red = gtk::gdk::RGBA::parse("#e01b24").unwrap();
    window.set_path_color(&p("Projects"), Some(&red));
    assert_eq!(
        window.sidebar_color(&p("Projects")).as_deref(),
        Some("file-color-e01b24")
    );
    assert_eq!(
        window.sidebar_icon(&p("Projects")).as_deref(),
        Some("folder-symbolic")
    );
    let saved: serde_json::Value =
        serde_json::from_str(&read(dir.path(), ".igneous/icons.json")).unwrap();
    assert_eq!(saved["colors"]["Projects"], "#e01b24");
    assert!(saved["icons"].as_object().unwrap().is_empty());

    window.rename(&p("Projects"), &p("Work"));
    wait(300).await;
    assert_eq!(window.path_color(&p("Work")), Some(red));
    assert_eq!(
        window.sidebar_color(&p("Work")).as_deref(),
        Some("file-color-e01b24")
    );

    // Remove Color, from the folder's menu.
    WidgetExt::activate_action(&window, "win.file-reset-color", Some(&"Work".to_variant()))
        .unwrap();
    assert_eq!(window.path_color(&p("Work")), None);
    assert_eq!(window.sidebar_color(&p("Work")).as_deref(), Some(""));

    window.set_path_color(&p("Work"), Some(&red));
    window.trash(p("Work"));
    wait(300).await;
    assert_eq!(window.path_color(&p("Work")), None);
    window.close();
}

/// The picker's categories are the icon theme's contexts, and icons given
/// lately show in one row above them.
#[gtk::test]
async fn the_icon_picker_has_categories_and_recents() {
    let dir = vault(None);
    let window = open(&dir);
    window.open_path(&p("Home.md"), false);
    let open_picker = || {
        WidgetExt::activate_action(&window, "win.set-icon", None).unwrap();
        window.visible_dialog().unwrap()
    };
    let dialog = open_picker();
    let grid = find(dialog.upcast_ref(), &|w| w.is::<gtk::GridView>())
        .and_downcast::<gtk::GridView>()
        .unwrap();
    let group = find(dialog.upcast_ref(), &|w| w.is::<adw::ToggleGroup>())
        .and_downcast::<adw::ToggleGroup>()
        .unwrap();
    let all = grid_icons(&grid);
    group.set_active_name(Some("Places"));
    let places = grid_icons(&grid);
    assert!(places.len() < all.len());
    assert!(places.contains(&"folder-symbolic".to_owned()));
    assert!(!places.contains(&"starred-symbolic".to_owned()));
    group.set_active_name(Some("all"));
    assert_eq!(grid_icons(&grid).len(), all.len());

    // Pick an icon; the next picker has it in the Recent row.
    let position = all.iter().position(|i| i == "starred-symbolic").unwrap() as u32;
    grid.emit_by_name::<()>("activate", &[&position]);
    assert_eq!(
        window.note_icon(&p("Home.md")).as_deref(),
        Some("starred-symbolic")
    );
    let dialog = open_picker();
    let recent: Vec<String> = descendants(dialog.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Button>().ok())
        .filter_map(|b| b.child().and_downcast::<gtk::Image>())
        .filter_map(|i| i.icon_name().map(|n| n.to_string()))
        .collect();
    assert_eq!(recent.first().map(String::as_str), Some("starred-symbolic"));
    dialog.close();
    window.close();
}

/// The picker opens at the note's icon. Scrolling the grid before it was
/// laid out left it blank, at the top.
#[gtk::test]
async fn the_icon_picker_opens_at_the_current_icon() {
    let dir = vault(None);
    let window = open(&dir);
    window.open_path(&p("Home.md"), false);
    window.set_note_icon(&p("Home.md"), Some("starred-symbolic"));
    WidgetExt::activate_action(&window, "win.set-icon", None).unwrap();
    let dialog = window.visible_dialog().unwrap();
    let grid = find(dialog.upcast_ref(), &|w| w.is::<gtk::GridView>())
        .and_downcast::<gtk::GridView>()
        .unwrap();
    let selected = grid
        .model()
        .and_downcast::<gtk::SingleSelection>()
        .unwrap()
        .selected_item()
        .and_downcast::<gtk::StringObject>()
        .map(|s| s.string().to_string());
    assert_eq!(selected.as_deref(), Some("starred-symbolic"));
    let adjustment = grid.vadjustment().unwrap();
    assert!(
        until(3000, || adjustment.value() > 0.0).await,
        "the picker didn't scroll to the icon"
    );
    dialog.close();
    window.close();
}

#[gtk::test]
async fn unreadable_icons_are_never_overwritten() {
    let dir = vault(None);
    write(dir.path(), ".igneous/icons.json", "{ nope");
    let window = open(&dir);
    window.set_note_icon(&p("Home.md"), Some("folder-symbolic"));
    assert_eq!(read(dir.path(), ".igneous/icons.json"), "{ nope");
    assert_eq!(window.note_icon(&p("Home.md")), None);
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
/// Every widget under `root`.
fn descendants(root: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut out = Vec::new();
    let mut child = root.first_child();
    while let Some(c) = child {
        out.push(c.clone());
        out.extend(descendants(&c));
        child = c.next_sibling();
    }
    out
}

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
    if let Some(dialog) = window.visible_dialog() {
        dialog.close();
    }

    // Custom icons in bookmarks and on tabs, then in the file tree with the
    // icon picker open.
    window.set_note_icon(&p("Home.md"), Some("go-home-symbolic"));
    window.set_note_icon(&p("2026-10-01.md"), Some("starred-symbolic"));
    wait(800).await;
    save_png(
        window.upcast_ref(),
        &out.replace(".png", "-icons-bookmarks.png"),
    );
    if let Some(stack) = find(&root, &|w| {
        w.downcast_ref::<adw::ViewStack>()
            .is_some_and(|s| s.child_by_name("files").is_some())
    })
    .and_downcast::<adw::ViewStack>()
    {
        stack.set_visible_child_name("files");
    }
    window.open_path(&p("Home.md"), false);
    WidgetExt::activate_action(&window, "win.set-icon", None).unwrap();
    if let Some(entry) = window
        .visible_dialog()
        .and_then(|d| find(d.upcast_ref(), &|w| w.is::<gtk::SearchEntry>()))
        .and_downcast::<gtk::SearchEntry>()
    {
        entry.set_text("go");
    }
    wait(1000).await;
    save_png(window.upcast_ref(), &out.replace(".png", "-icons.png"));
    window.close();
}
