//! Bases: `.base` files in their own tab, view state written back, and bases
//! embedded in notes. Run with `build-aux/run-ui-tests.sh`.

use std::path::Path;

use adw::prelude::*;
use igneous::{BasePage, Window};

mod common;
use common::*;

const TASKS_BASE: &str = r#"filters:
  and:
    - file.inFolder("Tasks")
    - file.ext == "md"
properties:
  note.priority:
    displayName: Priority
views:
  - type: table
    name: By priority
    order:
      - file.name
      - priority
      - status
    sort:
      - property: priority
        direction: DESC
    columnSize:
      file.name: 220
  - type: cards
    name: Cards
    order:
      - file.name
      - status
  - type: kanban
    name: Board
"#;

/// The fixture vault plus a folder of tasks with properties and a base over
/// them.
fn tasks_vault() -> tempfile::TempDir {
    let dir = vault(Some(100));
    let tasks = dir.path().join("Tasks");
    std::fs::create_dir_all(&tasks).unwrap();
    for (name, priority, status) in [
        ("Write tests", 2, "doing"),
        ("Ship it", 3, "todo"),
        ("Plan", 1, "done"),
    ] {
        std::fs::write(
            tasks.join(format!("{name}.md")),
            format!("---\npriority: {priority}\nstatus: {status}\n---\n# {name}\n"),
        )
        .unwrap();
    }
    std::fs::write(tasks.join("Tasks.base"), TASKS_BASE).unwrap();
    dir
}

async fn open_base(window: &Window, path: &str) -> BasePage {
    assert!(until(5000, || window.index().is_ready()).await);
    window.open_path(&p(path), false);
    let base = window.selected_base().expect("a Bases tab");
    assert!(
        until(5000, || base.data().is_some()).await,
        "the view never ran"
    );
    base
}

fn names(base: &BasePage) -> Vec<String> {
    base.data()
        .unwrap()
        .rows
        .iter()
        .map(|r| r.file.stem().to_owned())
        .collect()
}

#[gtk::test]
async fn base_files_open_in_a_bases_tab() {
    let dir = tasks_vault();
    let window = open(&dir);
    let base = open_base(&window, "Tasks/Tasks.base").await;
    assert_eq!(base.showing(), "table");
    assert_eq!(base.view_name().as_deref(), Some("By priority"));
    let data = base.data().unwrap();
    let columns: Vec<(&str, &str)> = data
        .columns
        .iter()
        .map(|c| (c.id.as_str(), c.name.as_str()))
        .collect();
    assert_eq!(columns[0].0, "file.name");
    assert_eq!(columns[1], ("note.priority", "Priority"));
    assert_eq!(columns[2].0, "note.status");
    // Sorted by priority, highest first.
    assert_eq!(names(&base), ["Ship it", "Write tests", "Plan"]);
    assert_eq!(data.columns[0].width, Some(220));

    // The table shows the same columns, widths and sort.
    let table = base.column_view().unwrap();
    let column = |i: u32| {
        table
            .columns()
            .item(i)
            .and_downcast::<gtk::ColumnViewColumn>()
            .unwrap()
    };
    assert_eq!(table.columns().n_items(), 3);
    assert_eq!(column(1).title().as_deref(), Some("Priority"));
    assert_eq!(column(0).fixed_width(), 220);

    // Other views.
    assert!(base.select_view("Cards"));
    assert!(until(3000, || base.showing() == "cards").await);

    // The tab is saved like any other.
    window.save_workspace();
    let saved = std::fs::read_to_string(dir.path().join(".igneous/workspace.json")).unwrap();
    assert!(saved.contains(r#""kind": "base""#), "{saved}");
    window.close();
}

#[gtk::test]
async fn rearranging_columns_writes_minimal_edits() {
    let dir = tasks_vault();
    let file = dir.path().join("Tasks/Tasks.base");
    let window = open(&dir);
    let base = open_base(&window, "Tasks/Tasks.base").await;
    let table = base.column_view().unwrap();
    let column = |i: u32| {
        table
            .columns()
            .item(i)
            .and_downcast::<gtk::ColumnViewColumn>()
            .unwrap()
    };

    // Resizing adds just that column's width.
    column(1).set_fixed_width(150);
    assert!(
        until(3000, || std::fs::read_to_string(&file).unwrap()
            != TASKS_BASE)
        .await
    );
    let resized = std::fs::read_to_string(&file).unwrap();
    assert_eq!(
        resized,
        TASKS_BASE.replace(
            "      file.name: 220\n",
            "      file.name: 220\n      priority: 150\n"
        )
    );

    // Sorting by another column replaces the sort, and nothing else.
    let table = base.column_view().unwrap();
    let status = table
        .columns()
        .item(2)
        .and_downcast::<gtk::ColumnViewColumn>()
        .unwrap();
    table.sort_by_column(Some(&status), gtk::SortType::Ascending);
    assert!(until(3000, || std::fs::read_to_string(&file).unwrap() != resized).await);
    let sorted = std::fs::read_to_string(&file).unwrap();
    assert_eq!(
        sorted,
        resized.replace(
            "      - property: priority\n        direction: DESC\n",
            "      - property: status\n        direction: ASC\n"
        )
    );
    assert!(until(3000, || names(&base) == ["Write tests", "Plan", "Ship it"]).await);

    // Moving a column rewrites the order. (Dragging a header moves it in
    // place; taking a column out, as here, would also drop its sort, so the
    // column moved isn't the sorted one.)
    let table = base.column_view().unwrap();
    let priority = table
        .columns()
        .item(1)
        .and_downcast::<gtk::ColumnViewColumn>()
        .unwrap();
    table.remove_column(&priority);
    table.insert_column(0, &priority);
    assert!(until(3000, || std::fs::read_to_string(&file).unwrap() != sorted).await);
    let moved = std::fs::read_to_string(&file).unwrap();
    assert_eq!(
        moved,
        sorted.replace(
            "      - file.name\n      - priority\n      - status\n",
            "      - priority\n      - file.name\n      - status\n"
        )
    );
    window.close();
}

#[gtk::test]
async fn unsupported_views_leave_the_file_alone() {
    let dir = tasks_vault();
    let file = dir.path().join("Tasks/Tasks.base");
    let window = open(&dir);
    let base = open_base(&window, "Tasks/Tasks.base").await;
    assert!(base.select_view("Board"));
    assert!(until(3000, || base.showing() == "message").await);
    wait(800).await;
    base.flush();
    window.close();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), TASKS_BASE);
}

#[gtk::test]
async fn the_source_can_be_edited() {
    let dir = tasks_vault();
    let file = dir.path().join("Tasks/Tasks.base");
    let window = open(&dir);
    let base = open_base(&window, "Tasks/Tasks.base").await;
    base.set_source_shown(true);
    assert_eq!(base.showing(), "source");
    let note = base.source_note().unwrap();
    assert_eq!(note.text(), TASKS_BASE);
    let buffer = note.buffer();
    let edited = TASKS_BASE.replace("name: By priority", "name: Priorities");
    buffer.set_text(&edited);
    base.set_source_shown(false);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), edited);
    assert!(until(3000, || base.view_name().as_deref() == Some("Priorities")).await);
    window.close();
}

/// Widgets under `widget` with the CSS class `class`.
fn find_class(widget: &gtk::Widget, class: &str, found: &mut Vec<gtk::Widget>) {
    if widget.has_css_class(class) {
        found.push(widget.clone());
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        find_class(&c, class, found);
        child = c.next_sibling();
    }
}

#[gtk::test]
async fn bases_embedded_in_notes_render() {
    let dir = tasks_vault();
    std::fs::write(
        dir.path().join("Overview.md"),
        "# Overview\n\n![[Tasks.base#Cards]]\n\n```base\nfilters:\n  and:\n    - file.inFolder(\"Tasks\")\nviews:\n  - type: table\n    name: All\n```\n\nThe end.\n",
    )
    .unwrap();
    let window = open(&dir);
    assert!(until(5000, || window.index().is_ready()).await);
    window.open_path(&p("Overview.md"), false);
    let note = window.selected_note().unwrap();
    note.set_cursor_byte(0);
    let kinds = note.view().overlay_kinds();
    assert!(kinds.contains(&"base".to_owned()), "{kinds:?}");
    assert!(kinds.contains(&"embed:Tasks.base".to_owned()), "{kinds:?}");
    // Both fill in with their results once shown.
    let view = note.view();
    assert!(
        until(5000, || {
            let mut tables = Vec::new();
            find_class(view.upcast_ref(), "base-embed-table", &mut tables);
            tables.len() == 2
        })
        .await
    );
    // Nothing was written.
    window.close();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("Tasks/Tasks.base")).unwrap(),
        TASKS_BASE
    );
}

#[gtk::test]
async fn the_fixture_base_lists_the_projects_folder() {
    let dir = vault(None);
    let window = open(&dir);
    let base = open_base(&window, "Projects/Projects.base").await;
    let mut rows = names(&base);
    rows.sort();
    assert_eq!(rows, ["Ideas", "Projects", "Roadmap", "Windows"]);
    window.close();
}

/// Screenshots of the Bases tab and embeds, for checking by eye:
/// `IGNEOUS_SCREENSHOT=/tmp/bases.png build-aux/run-ui-tests.sh --test ui_bases -- --ignored`.
/// `IGNEOUS_SCREENSHOT_VAULT` shows each base of another vault instead. It
/// must be a throwaway copy: opening a vault writes `.igneous/` into it.
/// Only counts are printed.
#[gtk::test]
#[ignore = "visual check"]
async fn screenshot_bases() {
    let Ok(out) = std::env::var("IGNEOUS_SCREENSHOT") else {
        return;
    };
    if let Ok(copy) = std::env::var("IGNEOUS_SCREENSHOT_VAULT") {
        let copy = Path::new(&copy);
        if let Some(settings) = gtk::Settings::default() {
            settings.set_gtk_error_bell(false);
        }
        let window = Window::for_vault(copy).unwrap();
        window.present();
        window.set_default_size(1280, 800);
        assert!(until(10_000, || window.index().is_ready()).await);
        let mut bases = Vec::new();
        for entry in walkdir(copy) {
            if entry.extension().is_some_and(|e| e == "base") {
                bases.push(entry);
            }
        }
        for (i, path) in bases.iter().enumerate() {
            let rel = path
                .strip_prefix(copy)
                .unwrap()
                .to_string_lossy()
                .to_string();
            let base = open_base(&window, &rel).await;
            wait(800).await;
            let data = base.data().unwrap();
            println!(
                "base {i}: {} rows, {} columns, view kind {:?}",
                data.rows.len(),
                data.columns.len(),
                data.kind
            );
            save_png(
                window.upcast_ref(),
                &out.replace(".png", &format!("-real-{i}.png")),
            );
        }
        window.close();
        return;
    }
    let dir = tasks_vault();
    std::fs::write(
        dir.path().join("Overview.md"),
        "# Overview\n\nTasks as cards:\n\n![[Tasks.base#Cards]]\n\nAnd a table:\n\n```base\nfilters:\n  and:\n    - file.inFolder(\"Tasks\")\n    - file.ext == \"md\"\nviews:\n  - type: table\n    name: All\n    order:\n      - file.name\n      - status\n```\n\nThe end.\n",
    )
    .unwrap();
    let window = open(&dir);
    window.set_default_size(1100, 720);
    let base = open_base(&window, "Tasks/Tasks.base").await;
    wait(800).await;
    save_png(window.upcast_ref(), &out.replace(".png", "-table.png"));
    base.select_view("Cards");
    wait(800).await;
    save_png(window.upcast_ref(), &out.replace(".png", "-cards.png"));
    base.select_view("Board");
    wait(500).await;
    save_png(
        window.upcast_ref(),
        &out.replace(".png", "-unsupported.png"),
    );
    window.open_path(&p("Overview.md"), true);
    wait(1500).await;
    save_png(window.upcast_ref(), &out.replace(".png", "-embeds.png"));
    window.close();
}

fn walkdir(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            out.extend(walkdir(&path));
        } else {
            out.push(path);
        }
    }
    out
}
