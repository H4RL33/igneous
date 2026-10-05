//! The command palette (Ctrl+P): every command, with its shortcut, found by
//! typing part of its name.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

use crate::window::Window;

/// When a command makes sense.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Needs {
    Nothing,
    /// A note is open in the selected tab.
    Note,
    /// The vault is in a Git repository.
    Git,
}

pub struct Command {
    pub action: &'static str,
    pub label: &'static str,
    pub needs: Needs,
}

const fn command(action: &'static str, label: &'static str, needs: Needs) -> Command {
    Command {
        action,
        label,
        needs,
    }
}

/// Every command the palette offers, in the order shown for an empty query.
pub const COMMANDS: &[Command] = &[
    command("win.quick-switcher", "Find note", Needs::Nothing),
    command("win.search", "Search vault", Needs::Nothing),
    command("win.toggle-reading", "Toggle Reading view", Needs::Note),
    command("win.new-note", "New note", Needs::Nothing),
    command("win.new-folder", "New folder", Needs::Nothing),
    command("win.rename-note", "Rename note", Needs::Note),
    command("win.save", "Save now", Needs::Note),
    command("win.go-back", "Go back", Needs::Note),
    command("win.go-forward", "Go forward", Needs::Note),
    command("win.close-tab", "Close tab", Needs::Nothing),
    command("win.reopen-tab", "Reopen closed tab", Needs::Nothing),
    command("win.toggle-sidebar", "Toggle sidebar", Needs::Nothing),
    command("win.graph", "Open graph view", Needs::Nothing),
    command("win.sync-now", "Git: Sync now", Needs::Git),
    command("win.pull", "Git: Pull", Needs::Git),
    command("win.show-changes", "Git: Show changes", Needs::Git),
    command("win.note-history", "Git: Show note history", Needs::Git),
    command("win.publish-branch", "Git: Publish branch", Needs::Git),
    command("win.lint-note", "Lint: Lint note", Needs::Note),
    command(
        "win.lint-vault",
        "Lint: Lint every note in the vault",
        Needs::Nothing,
    ),
    command(
        "win.daily-note",
        "Daily notes: Open today’s note",
        Needs::Nothing,
    ),
    command(
        "win.daily-note-previous",
        "Daily notes: Previous daily note",
        Needs::Nothing,
    ),
    command(
        "win.daily-note-next",
        "Daily notes: Next daily note",
        Needs::Nothing,
    ),
    command("win.insert-template", "Insert template", Needs::Note),
    command("win.bookmark", "Bookmark note (or remove it)", Needs::Note),
    command("win.bookmark-heading", "Bookmark the heading", Needs::Note),
    command("win.bookmark-search", "Bookmark the search", Needs::Nothing),
    command("win.set-icon", "Set note icon", Needs::Note),
    command("win.show-bookmarks", "Show bookmarks", Needs::Nothing),
    command("win.note-snapshots", "Show note snapshots", Needs::Note),
    command("win.preferences", "Open preferences", Needs::Nothing),
    command("app.shortcuts", "Show keyboard shortcuts", Needs::Nothing),
    command("app.new-window", "Open another vault", Needs::Nothing),
    command("app.about", "About Igneous", Needs::Nothing),
    command("app.quit", "Quit", Needs::Nothing),
];

/// The commands matching `query`, best first.
pub fn rank<'a>(query: &str, commands: &[&'a Command]) -> Vec<&'a Command> {
    let query = query.trim();
    if query.is_empty() {
        return commands.to_vec();
    }
    let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buf = Vec::new();
    let mut scored: Vec<(u32, usize, &Command)> = commands
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let score = pattern.score(Utf32Str::new(c.label, &mut buf), &mut matcher)?;
            Some((score, i, *c))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, c)| c).collect()
}

pub fn show(window: &Window) {
    let available: Vec<&'static Command> = COMMANDS
        .iter()
        .filter(|c| match c.needs {
            Needs::Nothing => true,
            Needs::Note => window.selected_note().is_some(),
            Needs::Git => window.sync().is_available(),
        })
        .collect();
    let app = window.application();
    let accel = move |action: &str| -> Option<String> {
        app.as_ref()?
            .accels_for_action(action)
            .first()
            .map(|a| a.to_string())
    };

    let entry = gtk::SearchEntry::builder()
        .placeholder_text("Run a command")
        .hexpand(true)
        .build();
    let list = gtk::ListBox::builder()
        .css_classes(["navigation-sidebar"])
        .selection_mode(gtk::SelectionMode::Browse)
        .build();
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&list)
        .build();
    let header = adw::HeaderBar::builder()
        .show_start_title_buttons(false)
        .show_end_title_buttons(false)
        .title_widget(&entry)
        .build();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&scrolled));
    let dialog = adw::Dialog::builder()
        .title("Command Palette")
        .content_width(560)
        .content_height(480)
        .child(&toolbar)
        .build();

    let run: Rc<dyn Fn(&'static str)> = {
        let window = window.downgrade();
        let dialog = dialog.downgrade();
        Rc::new(move |action| {
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
            if let Some(window) = window.upgrade() {
                // Run once the dialog has gone, so commands that open dialogs
                // of their own get the window as their parent.
                glib::idle_add_local_once(move || {
                    let _ = WidgetExt::activate_action(&window, action, None);
                });
            }
        })
    };

    let fill = {
        let list = list.clone();
        let run = run.clone();
        move |query: &str| {
            list.remove_all();
            for command in rank(query, &available) {
                let label = gtk::Label::builder()
                    .label(command.label)
                    .xalign(0.0)
                    .hexpand(true)
                    .build();
                let row_box = gtk::Box::builder().spacing(12).build();
                row_box.append(&label);
                if let Some(accel) = accel(command.action) {
                    row_box.append(&adw::ShortcutLabel::new(&accel));
                }
                let row = gtk::ListBoxRow::builder().child(&row_box).build();
                let action = command.action;
                let run = run.clone();
                crate::rows::on_activate(&row, move || run(action));
                list.append(&row);
            }
            if let Some(first) = list.row_at_index(0) {
                list.select_row(Some(&first));
            }
        }
    };
    fill("");
    let fill = Rc::new(fill);
    {
        let fill = fill.clone();
        entry.connect_search_changed(move |entry| fill(&entry.text()));
    }
    {
        let list = list.clone();
        entry.connect_activate(move |_| {
            if let Some(row) = list.selected_row() {
                row.emit_activate();
            }
        });
    }
    // Up and down move through the list while typing.
    let keys = gtk::EventControllerKey::new();
    {
        let list = list.clone();
        let scrolled = scrolled.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            let step = match key {
                gtk::gdk::Key::Down => 1,
                gtk::gdk::Key::Up => -1,
                _ => return glib::Propagation::Proceed,
            };
            let current = list.selected_row().map_or(-1, |r| r.index());
            if let Some(row) = list.row_at_index((current + step).max(0)) {
                list.select_row(Some(&row));
                scroll_to(&scrolled, &list, &row);
            }
            glib::Propagation::Stop
        });
    }
    entry.add_controller(keys);
    dialog.present(Some(window));
    entry.grab_focus();
}

/// Scrolls just enough to show `row`.
fn scroll_to(scrolled: &gtk::ScrolledWindow, list: &gtk::ListBox, row: &gtk::ListBoxRow) {
    let Some(bounds) = row.compute_bounds(list) else {
        return;
    };
    let adjustment = scrolled.vadjustment();
    let (top, bottom) = (
        f64::from(bounds.y()),
        f64::from(bounds.y() + bounds.height()),
    );
    if top < adjustment.value() {
        adjustment.set_value(top);
    } else if bottom > adjustment.value() + adjustment.page_size() {
        adjustment.set_value(bottom - adjustment.page_size());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_by_label() {
        let all: Vec<&Command> = COMMANDS.iter().collect();
        assert_eq!(rank("", &all).len(), COMMANDS.len());
        assert_eq!(rank("sync now", &all)[0].action, "win.sync-now");
        assert_eq!(rank("prefs", &all)[0].action, "win.preferences");
        assert_eq!(rank("new note", &all)[0].action, "win.new-note");
        assert!(rank("zzzz", &all).is_empty());
    }

    #[test]
    fn labels_are_unique() {
        let mut labels: Vec<&str> = COMMANDS.iter().map(|c| c.label).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), COMMANDS.len());
    }
}
