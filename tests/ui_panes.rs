//! The sidebar's Search and Tags panes.

use adw::prelude::*;

mod common;
use common::*;

#[gtk::test]
async fn search_finds_notes() {
    let dir = vault(None);
    let window = open(&dir);
    assert!(until(5000, || window.index().is_ready()).await);
    window.search_vault("fixture vault");
    assert!(until(3000, || window.search_results() == [p("Home.md")]).await);
    window.search_vault("path:Projects -file:Ideas");
    assert!(
        until(3000, || {
            let found = window.search_results();
            found.contains(&p("Projects/Igneous/Roadmap.md"))
                && !found.contains(&p("Projects/Ideas.md"))
        })
        .await,
        "{:?}",
        window.search_results()
    );
    window.search_vault("tag:#home");
    assert!(until(3000, || window.search_results() == [p("Home.md")]).await);
    window.close();
}

#[gtk::test]
async fn tags_rename_across_the_vault() {
    let dir = vault(None);
    let window = open(&dir);
    assert!(until(5000, || window.index().is_ready()).await);
    assert!(window.index().tags().iter().any(|(t, _)| t == "home"));
    window.rename_tag("home", "start");
    let home = || std::fs::read_to_string(dir.path().join("Home.md")).unwrap();
    assert!(
        until(3000, || home().contains("- start")).await,
        "{}",
        home()
    );
    assert!(!home().contains("- home"));
    let _ = WidgetExt::activate_action(&window, "win.search", None);
    window.close();
}

#[gtk::test]
async fn dropped_files_become_attachments() {
    let dir = vault(None);
    let window = open(&dir);
    window.open_path(&p("Home.md"), false);
    let outside = tempfile::tempdir().unwrap();
    let file = outside.path().join("Scan.pdf");
    std::fs::write(&file, b"%PDF-1.4").unwrap();
    let note = window.selected_note().unwrap();
    let link = note.attach_for_test(&file);
    assert_eq!(link.as_deref(), Some("![[Scan.pdf]]"));
    assert!(dir.path().join("Scan.pdf").is_file());
    // A second copy gets a free name.
    let link = note.attach_for_test(&file);
    assert_eq!(link.as_deref(), Some("![[Scan 1.pdf]]"));
    window.close();
}

#[gtk::test]
async fn editor_settings_apply_to_open_notes() {
    let dir = vault(None);
    let window = open(&dir);
    window.open_path(&p("Home.md"), false);
    let note = window.selected_note().unwrap();
    assert!(note.view().vim_context().is_none());
    {
        let ctx = window.ctx_for_test();
        let mut settings = ctx.settings.borrow_mut();
        settings.editor.vim_mode = true;
        settings.editor.spellcheck = true;
    }
    window.apply_editor_settings();
    assert!(note.view().vim_context().is_some());
    wait(200).await;
    window.close();
}

#[gtk::test]
async fn linked_mentions_at_the_end_of_notes() {
    let dir = vault(None);
    let window = open(&dir);
    assert!(until(5000, || window.index().is_ready()).await);
    window
        .ctx_for_test()
        .settings
        .borrow_mut()
        .editor
        .backlinks_in_document = true;
    window.open_path(&p("Projects/Igneous/Roadmap.md"), false);
    let note = window.selected_note().unwrap();
    assert!(
        until(3000, || note
            .view()
            .overlay_kinds()
            .contains(&"mentions".to_owned()))
        .await
    );
    note.view().check_invariants().unwrap();
    window.close();
}

#[gtk::test]
async fn opening_a_note_focuses_its_text() {
    let dir = vault(None);
    // As when the vault opens with this tab restored.
    std::fs::create_dir_all(dir.path().join(".igneous")).unwrap();
    std::fs::write(
        dir.path().join(".igneous/workspace.json"),
        r#"{"version":1,"tabs":[{"kind":"note","path":"Projects/Ideas.md"},{"kind":"note","path":"Home.md"}],"activeTab":1,"inspector":{"visible":true}}"#,
    )
    .unwrap();
    let window = open(&dir);
    wait(600).await;
    let focus = gtk::prelude::GtkWindowExt::focus(&window).map(|w| {
        let mut chain = Vec::new();
        let mut at = Some(w);
        while let Some(widget) = at {
            chain.push(widget.type_().name().to_string());
            at = widget.parent();
        }
        chain.join(" < ")
    });
    let note = window.selected_note().unwrap();
    let view = note.view();
    let focused = gtk::prelude::GtkWindowExt::focus(&window);
    assert!(
        focused.as_ref() == Some(view.upcast_ref::<gtk::Widget>()),
        "focus is on {focus:?}"
    );
    window.close();
}

/// Every row of a plain list in the panes, under `root`.
fn list_rows(root: &gtk::Widget) -> Vec<(gtk::ListBox, gtk::ListBoxRow)> {
    let mut out = Vec::new();
    if let Some(list) = root.downcast_ref::<gtk::ListBox>() {
        let mut i = 0;
        while let Some(row) = list.row_at_index(i) {
            out.push((list.clone(), row));
            i += 1;
        }
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        out.extend(list_rows(&c));
        child = c.next_sibling();
    }
    out
}

/// Clicking a row (the list's row-activated) or pressing Enter on it (the
/// row's activate) opens what it shows. Re-emitting one from the other
/// recursed until the stack overflowed.
#[gtk::test]
async fn rows_in_the_panes_open_what_they_show() {
    let dir = vault(Some(100));
    let window = open(&dir);
    let roadmap = p("Projects/Igneous/Roadmap.md");

    // A backlink in the inspector, clicked and then with Enter.
    for click in [true, false] {
        window.open_path(&p("Home.md"), false);
        window.show_inspector("backlinks");
        assert!(
            until(3000, || list_rows(window.upcast_ref()).iter().any(
                |(_, r)| r.tooltip_text().as_deref() == Some(roadmap.as_str())
            ))
            .await
        );
        let (list, row) = list_rows(window.upcast_ref())
            .into_iter()
            .find(|(_, r)| r.tooltip_text().as_deref() == Some(roadmap.as_str()))
            .unwrap();
        if click {
            list.emit_by_name::<()>("row-activated", &[&row]);
        } else {
            row.emit_activate();
        }
        assert_eq!(window.selected_path(), Some(roadmap.clone()));
    }

    // A heading in the outline moves the cursor to it.
    window.open_path(&p("Home.md"), false);
    window.show_inspector("outline");
    wait(300).await;
    let note = window.selected_note().unwrap();
    note.set_cursor_byte(note.text().len());
    let heading = note.text().find("# Home").unwrap();
    let (list, row) = list_rows(window.upcast_ref())
        .into_iter()
        .find(|(_, r)| {
            r.child()
                .and_downcast::<gtk::Label>()
                .is_some_and(|l| l.label() == "Home")
        })
        .expect("the outline lists Home");
    list.emit_by_name::<()>("row-activated", &[&row]);
    let at = note.cursor_offset() as usize;
    assert_eq!(
        note.text().char_indices().nth(at).map(|(b, _)| b),
        Some(heading)
    );

    // A tag searches for it.
    let (list, row) = list_rows(window.upcast_ref())
        .into_iter()
        .find(|(_, r)| r.tooltip_text().as_deref() == Some("#home"))
        .expect("the tags pane lists #home");
    list.emit_by_name::<()>("row-activated", &[&row]);
    window.close();
}

/// The sidebar's resize handle: the widget with the resize cursor nearest
/// the start (`end`: the end) of the window.
fn resize_handle(root: &gtk::Widget, label: &str) -> gtk::Widget {
    fn all(w: &gtk::Widget, out: &mut Vec<gtk::Widget>) {
        out.push(w.clone());
        let mut c = w.first_child();
        while let Some(x) = c {
            all(&x, out);
            c = x.next_sibling();
        }
    }
    let mut widgets = Vec::new();
    all(root, &mut widgets);
    widgets
        .into_iter()
        .filter(|w| w.cursor().and_then(|c| c.name()).as_deref() == Some("col-resize"))
        .find(|w| {
            w.observe_controllers()
                .into_iter()
                .any(|c| c.is_ok_and(|c| c.is::<gtk::GestureDrag>()))
                && w.halign()
                    == if label == "end" {
                        gtk::Align::End
                    } else {
                        gtk::Align::Start
                    }
        })
        .expect("a resize handle")
}

/// Both sidebars keep the width they're dragged to, within limits, and it's
/// saved with the vault's workspace.
#[gtk::test]
async fn sidebars_resize_and_remember_their_width() {
    let dir = vault(None);
    std::fs::create_dir_all(dir.path().join(".igneous")).unwrap();
    std::fs::write(
        dir.path().join(".igneous/workspace.json"),
        r#"{"version":1,"sidebar":{"visible":true,"width":360},"inspector":{"visible":true,"width":50}}"#,
    )
    .unwrap();
    let window = open(&dir);
    window.set_default_size(1400, 900);
    wait(500).await;
    let workspace = window.workspace();
    assert_eq!(workspace.sidebar.width, 360);
    // Too narrow is held at the inspector's narrowest.
    assert_eq!(workspace.inspector.width, 240);

    // Dragging the sidebar's handle 100 pixels outwards widens it.
    let handle = resize_handle(window.upcast_ref(), "start");
    let drag = handle
        .observe_controllers()
        .into_iter()
        .find_map(|c| c.ok().and_downcast::<gtk::GestureDrag>())
        .unwrap();
    drag.emit_by_name::<()>("drag-begin", &[&0.0f64, &0.0f64]);
    drag.emit_by_name::<()>("drag-update", &[&100.0f64, &0.0f64]);
    drag.emit_by_name::<()>("drag-end", &[&100.0f64, &0.0f64]);
    assert_eq!(window.workspace().sidebar.width, 460);
    // And the inspector's, dragged left, widens it too.
    let handle = resize_handle(window.upcast_ref(), "end");
    let drag = handle
        .observe_controllers()
        .into_iter()
        .find_map(|c| c.ok().and_downcast::<gtk::GestureDrag>())
        .unwrap();
    drag.emit_by_name::<()>("drag-begin", &[&0.0f64, &0.0f64]);
    drag.emit_by_name::<()>("drag-update", &[&-60.0f64, &0.0f64]);
    drag.emit_by_name::<()>("drag-end", &[&-60.0f64, &0.0f64]);
    assert_eq!(window.workspace().inspector.width, 300);
    // The sidebar is laid out at that width.
    assert!(until(2000, || handle.margin_end() == 300 - 3).await);

    window.save_workspace();
    let saved: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".igneous/workspace.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["sidebar"]["width"], 460);
    assert_eq!(saved["inspector"]["width"], 300);
    window.close();
}
