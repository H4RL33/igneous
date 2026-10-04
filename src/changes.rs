//! The sidebar's Changes pane: conflicts, staged and unstaged changes with
//! stage, unstage and discard buttons, and a box to commit.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::glib;
use igneous_core::VaultPath;
use igneous_git::{Entry, InProgress, RepoStatus, template};

use crate::sync::SyncService;
use crate::window::Window;

pub struct ChangesPane {
    pub widget: gtk::Box,
    message: gtk::Entry,
    commit: gtk::Button,
    stack: gtk::Stack,
    sections: gtk::Box,
    service: Weak<SyncService>,
    window: glib::WeakRef<Window>,
    shown: RefCell<Option<RepoStatus>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Conflicts,
    Staged,
    Unstaged,
}

impl ChangesPane {
    pub fn new(service: &Rc<SyncService>, window: &Window) -> Rc<Self> {
        let message = gtk::Entry::builder()
            .placeholder_text("Commit message")
            .tooltip_text("Leave empty to use the vault's commit message template")
            .build();
        let commit = gtk::Button::builder()
            .label("_Commit")
            .use_underline(true)
            .css_classes(["suggested-action"])
            .build();
        let top = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_start(12)
            .margin_end(12)
            .margin_top(6)
            .margin_bottom(6)
            .build();
        top.append(&message);
        top.append(&commit);

        let sections = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&sections)
            .build();
        let empty = adw::StatusPage::builder()
            .icon_name("object-select-symbolic")
            .title("No Changes")
            .description("Everything in this vault is committed")
            .css_classes(["compact"])
            .build();
        let stack = gtk::Stack::new();
        stack.add_named(&scrolled, Some("list"));
        stack.add_named(&empty, Some("empty"));
        stack.set_vexpand(true);

        let widget = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        widget.append(&top);
        widget.append(&stack);

        let pane = Rc::new(Self {
            widget,
            message,
            commit,
            stack,
            sections,
            service: Rc::downgrade(service),
            window: window.downgrade(),
            shown: RefCell::default(),
        });
        let weak = Rc::downgrade(&pane);
        pane.commit.connect_clicked(move |_| {
            if let Some(pane) = weak.upgrade() {
                pane.commit();
            }
        });
        let weak = Rc::downgrade(&pane);
        pane.message.connect_activate(move |_| {
            if let Some(pane) = weak.upgrade() {
                pane.commit();
            }
        });
        let weak = Rc::downgrade(&pane);
        service.connect_changed(move || {
            if let Some(pane) = weak.upgrade() {
                pane.update();
            }
        });
        pane.update();
        pane
    }

    pub fn focus_message(&self) {
        self.message.grab_focus();
    }

    fn update(self: &Rc<Self>) {
        let Some(service) = self.service.upgrade() else {
            return;
        };
        let status = service.status();
        let merging = status
            .as_ref()
            .is_some_and(|s| s.in_progress.is_some() && !s.has_conflicts());
        let conflicts = status.as_ref().is_some_and(RepoStatus::has_conflicts);
        let dirty = status.as_ref().is_some_and(RepoStatus::is_dirty);
        self.commit.set_label(if merging {
            match status.as_ref().and_then(|s| s.in_progress) {
                Some(InProgress::Rebase) => "_Continue Rebase",
                _ => "_Commit Merge",
            }
        } else {
            "_Commit"
        });
        self.commit.set_sensitive(!conflicts && (dirty || merging));
        self.message.set_sensitive(!merging);

        if *self.shown.borrow() == status {
            return;
        }
        self.shown.replace(status.clone());
        while let Some(child) = self.sections.first_child() {
            self.sections.remove(&child);
        }
        let Some(status) = status else {
            self.stack.set_visible_child_name("empty");
            return;
        };
        let groups = [
            (Section::Conflicts, "Conflicts"),
            (Section::Staged, "Staged"),
            (Section::Unstaged, "Changes"),
        ];
        let mut any = false;
        for (section, title) in groups {
            let entries: Vec<&Entry> = status
                .entries
                .iter()
                .filter(|e| match section {
                    Section::Conflicts => e.conflicted,
                    Section::Staged => e.is_staged(),
                    Section::Unstaged => e.is_unstaged(),
                })
                .collect();
            if entries.is_empty() {
                continue;
            }
            any = true;
            self.sections
                .append(&self.header(section, title, entries.len()));
            let list = gtk::ListBox::builder()
                .css_classes(["navigation-sidebar"])
                .selection_mode(gtk::SelectionMode::None)
                .build();
            for entry in entries {
                list.append(&self.row(section, entry));
            }
            self.sections.append(&list);
        }
        self.stack
            .set_visible_child_name(if any { "list" } else { "empty" });
    }

    fn header(self: &Rc<Self>, section: Section, title: &str, count: usize) -> gtk::Box {
        let label = gtk::Label::builder()
            .label(format!("{title} ({count})"))
            .xalign(0.0)
            .hexpand(true)
            .css_classes(["heading"])
            .build();
        let header = gtk::Box::builder()
            .spacing(6)
            .margin_start(12)
            .margin_end(6)
            .margin_top(12)
            .build();
        header.append(&label);
        let (icon, tooltip) = match section {
            Section::Conflicts => return header,
            Section::Staged => ("list-remove-symbolic", "Unstage All"),
            Section::Unstaged => ("list-add-symbolic", "Stage All"),
        };
        let button = gtk::Button::builder()
            .icon_name(icon)
            .tooltip_text(tooltip)
            .css_classes(["flat", "circular"])
            .valign(gtk::Align::Center)
            .build();
        let weak = Rc::downgrade(self);
        button.connect_clicked(move |_| {
            let Some(pane) = weak.upgrade() else { return };
            let paths: Vec<String> = pane.paths_in(section);
            pane.act(move |git| {
                let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
                match section {
                    Section::Staged => git.unstage(&refs),
                    _ => git.stage_all(),
                }
            });
        });
        header.append(&button);
        header
    }

    fn paths_in(&self, section: Section) -> Vec<String> {
        self.shown
            .borrow()
            .iter()
            .flat_map(|s| s.entries.iter())
            .filter(|e| match section {
                Section::Conflicts => e.conflicted,
                Section::Staged => e.is_staged(),
                Section::Unstaged => e.is_unstaged(),
            })
            .map(|e| e.path.clone())
            .collect()
    }

    fn row(self: &Rc<Self>, section: Section, entry: &Entry) -> gtk::ListBoxRow {
        let git = self.service.upgrade().and_then(|s| s.git());
        let vault_path = git
            .as_ref()
            .and_then(|g| g.to_vault(&entry.path))
            .and_then(|p| VaultPath::new(p).ok());
        let (name, folder) = match &vault_path {
            Some(path) => (
                crate::files::display_name(path, false).0,
                path.parent().map(|p| p.to_string()),
            ),
            None => (entry.path.clone(), None),
        };
        let letter = match section {
            Section::Staged if entry.index != '.' => entry.index,
            _ => entry.letter(),
        };
        let badge = gtk::Label::builder()
            .label(letter.to_string())
            .css_classes([
                "change-badge",
                &format!("change-{}", letter.to_ascii_lowercase()),
            ])
            .valign(gtk::Align::Center)
            .tooltip_text(describe_letter(letter))
            .build();
        let title = gtk::Label::builder()
            .label(&name)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .build();
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        text.append(&title);
        if let Some(folder) = folder {
            text.append(
                &gtk::Label::builder()
                    .label(folder)
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::Start)
                    .css_classes(["caption", "dim-label"])
                    .build(),
            );
        }
        let content = gtk::Box::builder().spacing(8).build();
        content.append(&badge);
        content.append(&text);

        let path = entry.path.clone();
        let untracked = entry.is_untracked();
        let buttons: &[(&str, &str, Action)] = match section {
            Section::Conflicts => &[("object-select-symbolic", "Mark as Resolved", Action::Stage)],
            Section::Staged => &[("list-remove-symbolic", "Unstage", Action::Unstage)],
            Section::Unstaged => &[
                ("edit-undo-symbolic", "Discard Changes", Action::Discard),
                ("list-add-symbolic", "Stage", Action::Stage),
            ],
        };
        for (icon, tooltip, action) in buttons {
            let button = gtk::Button::builder()
                .icon_name(*icon)
                .tooltip_text(*tooltip)
                .css_classes(["flat", "circular"])
                .valign(gtk::Align::Center)
                .build();
            let weak = Rc::downgrade(self);
            let path = path.clone();
            let vault_path = vault_path.clone();
            let action = *action;
            button.connect_clicked(move |_| {
                if let Some(pane) = weak.upgrade() {
                    pane.on_row_action(action, &path, vault_path.clone(), untracked);
                }
            });
            content.append(&button);
        }
        let row = gtk::ListBoxRow::builder()
            .child(&content)
            .activatable(true)
            .tooltip_text(&entry.path)
            .build();
        let window = self.window.clone();
        let staged = section == Section::Staged;
        row.connect_activate(move |_| {
            let (Some(window), Some(path)) = (window.upgrade(), vault_path.clone()) else {
                return;
            };
            if section == Section::Conflicts {
                window.open_path(&path, false);
            } else {
                window.open_changes(&path, staged, untracked);
            }
        });
        row
    }

    fn on_row_action(
        self: &Rc<Self>,
        action: Action,
        path: &str,
        vault_path: Option<VaultPath>,
        untracked: bool,
    ) {
        let path = path.to_owned();
        match action {
            Action::Stage => self.act(move |git| git.stage(&[&path])),
            Action::Unstage => self.act(move |git| git.unstage(&[&path])),
            Action::Discard if untracked => {
                if let (Some(window), Some(vault_path)) = (self.window.upgrade(), vault_path) {
                    window.trash(vault_path);
                }
            }
            Action::Discard => self.confirm_discard(path, vault_path),
        }
    }

    fn confirm_discard(self: &Rc<Self>, path: String, vault_path: Option<VaultPath>) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let name = vault_path
            .as_ref()
            .map_or_else(|| path.clone(), |p| crate::files::display_name(p, false).0);
        let dialog = adw::AlertDialog::builder()
            .heading(format!("Discard Changes to “{name}”?"))
            .body("Changes that aren’t staged will be lost. This can’t be undone.")
            .close_response("cancel")
            .default_response("cancel")
            .build();
        dialog.add_responses(&[("cancel", "_Cancel"), ("discard", "_Discard")]);
        dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
        let weak = Rc::downgrade(self);
        dialog.connect_response(Some("discard"), move |_, _| {
            if let Some(pane) = weak.upgrade() {
                let path = path.clone();
                pane.act(move |git| git.discard(&[&path]));
            }
        });
        dialog.present(Some(&window));
    }

    /// Runs a Git command on the worker, then refreshes; errors become toasts.
    fn act(
        self: &Rc<Self>,
        f: impl FnOnce(&igneous_git::Git) -> Result<(), igneous_git::GitError> + Send + 'static,
    ) {
        let (Some(service), window) = (self.service.upgrade(), self.window.clone()) else {
            return;
        };
        glib::spawn_future_local(async move {
            let result = service.call(f).await;
            service.refresh_now();
            if let (Some(Err(e)), Some(window)) = (result, window.upgrade()) {
                window.toast(&e.to_string());
            }
        });
    }

    fn commit(self: &Rc<Self>) {
        let (Some(service), Some(window)) = (self.service.upgrade(), self.window.upgrade()) else {
            return;
        };
        if !self.commit.is_sensitive() {
            return;
        }
        window.flush_all();
        let typed = self.message.text().trim().to_owned();
        let settings = service.settings();
        let merging = self
            .shown
            .borrow()
            .as_ref()
            .is_some_and(|s| s.in_progress.is_some());
        let hostname = glib::host_name().to_string();
        let message = self.message.clone();
        glib::spawn_future_local(async move {
            let result = service
                .call(move |git| -> Result<usize, igneous_git::GitError> {
                    if merging {
                        git.conclude()?;
                        return Ok(0);
                    }
                    let mut status = git.status()?;
                    if status.staged().next().is_none() {
                        git.stage_all()?;
                        status = git.status()?;
                    }
                    let staged: Vec<Entry> = status.staged().cloned().collect();
                    let text = if typed.is_empty() {
                        template::render(
                            &settings.commit_message,
                            &template::Context {
                                now: jiff::Zoned::now(),
                                date_format: &settings.date_format,
                                hostname: &hostname,
                                files: &staged,
                            },
                        )
                    } else {
                        typed
                    };
                    git.commit(&text)?;
                    Ok(staged.len())
                })
                .await;
            service.refresh_now();
            match result {
                Some(Ok(n)) => {
                    message.set_text("");
                    window.toast(&match n {
                        0 => "Committed".to_owned(),
                        1 => "Committed 1 file".to_owned(),
                        n => format!("Committed {n} files"),
                    });
                }
                Some(Err(e)) => window.toast(&format!("Couldn’t commit: {e}")),
                None => {}
            }
        });
    }
}

#[derive(Clone, Copy)]
enum Action {
    Stage,
    Unstage,
    Discard,
}

fn describe_letter(letter: char) -> &'static str {
    match letter {
        'M' => "Modified",
        'A' => "Added",
        'D' => "Deleted",
        'R' => "Renamed",
        'C' => "Copied",
        'T' => "Type changed",
        'U' => "Conflicted",
        _ => "Changed",
    }
}
