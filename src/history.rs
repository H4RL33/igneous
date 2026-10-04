//! A file's Git history: its commits, newest first. Choosing one opens that
//! version in a read-only tab.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use igneous_core::VaultPath;

use crate::sync::SyncService;
use crate::sync_button::relative_time;
use crate::window::Window;

const LIMIT: usize = 200;

pub fn show(window: &Window, service: &Rc<SyncService>, path: &VaultPath) {
    let Some(git) = service.git() else {
        window.toast("This vault isn’t a Git repository");
        return;
    };
    let name = crate::files::display_name(path, false).0;
    let list = gtk::ListBox::builder()
        .css_classes(["boxed-list"])
        .selection_mode(gtk::SelectionMode::None)
        .valign(gtk::Align::Start)
        .build();
    let stack = gtk::Stack::new();
    stack.add_named(&adw::Spinner::new(), Some("loading"));
    let clamp = adw::Clamp::builder()
        .child(&list)
        .margin_start(12)
        .margin_end(12)
        .margin_top(6)
        .margin_bottom(12)
        .build();
    stack.add_named(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&clamp)
            .build(),
        Some("list"),
    );
    stack.add_named(
        &adw::StatusPage::builder()
            .icon_name("document-open-recent-symbolic")
            .title("No History")
            .description("This file hasn’t been committed yet")
            .build(),
        Some("empty"),
    );
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&stack));
    let dialog = adw::Dialog::builder()
        .title(format!("History of {name}"))
        .content_width(460)
        .content_height(560)
        .child(&toolbar)
        .build();
    dialog.present(Some(window));

    let repo_path = git.to_repo(path.as_str());
    let service = service.clone();
    let window = window.downgrade();
    let vault_path = path.clone();
    glib::spawn_future_local(async move {
        let result = service.call(move |git| git.log(&repo_path, LIMIT)).await;
        let commits = match result {
            Some(Ok(commits)) => commits,
            Some(Err(e)) => {
                if let Some(window) = window.upgrade() {
                    window.toast(&format!("Couldn’t read the history: {e}"));
                }
                dialog.close();
                return;
            }
            None => return,
        };
        if commits.is_empty() {
            stack.set_visible_child_name("empty");
            return;
        }
        for commit in commits {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&commit.subject))
                .subtitle(glib::markup_escape_text(&format!(
                    "{} · {} · {}",
                    commit.author,
                    relative_time(commit.time),
                    commit.short
                )))
                .activatable(true)
                .build();
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            let window = window.clone();
            let service = service.clone();
            let vault_path = vault_path.clone();
            let dialog = dialog.downgrade();
            row.connect_activated(move |_| {
                let Some(window) = window.upgrade() else {
                    return;
                };
                window.open_version(&service, &vault_path, commit.clone());
                if let Some(dialog) = dialog.upgrade() {
                    dialog.close();
                }
            });
            list.append(&row);
        }
        stack.set_visible_child_name("list");
    });
}
