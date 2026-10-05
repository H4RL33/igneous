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
