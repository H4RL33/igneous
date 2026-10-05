//! Preferences → Plugins → Git Sync: the schedule, how remote changes come
//! in, and commit messages. Saved with the vault in `.igneous/git.json`.

use std::rc::Rc;

use adw::prelude::*;
use igneous_core::settings::{GitSettings, SyncMethod};

use crate::window::Window;

pub fn group(window: &Window, dialog: &adw::PreferencesDialog) -> adw::PreferencesGroup {
    let sync = window.sync().clone();
    let group = adw::PreferencesGroup::builder().title("Git Sync").build();
    if !sync.is_available() {
        group.add(
            &adw::ActionRow::builder()
                .title("Not a Git Repository")
                .subtitle("Run “git init” and add a remote, then reopen the vault")
                .build(),
        );
        return group;
    }
    let settings = sync.settings();

    let enabled = adw::SwitchRow::builder()
        .title("Sync Automatically")
        .active(settings.enabled)
        .build();
    let sync_every = minutes_row("Commit and Sync Every (Minutes)", settings.sync_interval);
    let pull_every = minutes_row("Pull Every (Minutes)", settings.pull_interval);
    let pull_on_open = adw::SwitchRow::builder()
        .title("Pull When the Vault Opens")
        .active(settings.pull_on_open)
        .build();
    for row in [&sync_every, &pull_every] {
        enabled
            .bind_property("active", row, "sensitive")
            .sync_create()
            .build();
    }
    enabled
        .bind_property("active", &pull_on_open, "sensitive")
        .sync_create()
        .build();
    let method = adw::ComboRow::builder()
        .title("Bring In Remote Changes By")
        .model(&gtk::StringList::new(&["Merging", "Rebasing"]))
        .selected(match settings.method {
            SyncMethod::Merge => 0,
            SyncMethod::Rebase => 1,
        })
        .build();
    let push = adw::SwitchRow::builder()
        .title("Push After Committing")
        .active(settings.push)
        .build();
    let message = adw::EntryRow::builder()
        .title("Commit Message")
        .text(&settings.commit_message)
        .show_apply_button(true)
        .tooltip_text("{{date}}, {{hostname}}, {{numFiles}} and {{files}} are filled in")
        .build();
    let date_format = adw::EntryRow::builder()
        .title("Commit Message Date Format")
        .text(&settings.date_format)
        .show_apply_button(true)
        .tooltip_text("Moment.js tokens, such as YYYY-MM-DD HH:mm")
        .build();
    group.add(&enabled);
    group.add(&sync_every);
    group.add(&pull_every);
    group.add(&pull_on_open);
    group.add(&method);
    group.add(&push);
    group.add(&message);
    group.add(&date_format);

    // Every change is saved straight away.
    let save = {
        let dialog = dialog.downgrade();
        let sync = sync.clone();
        let enabled = enabled.clone();
        let sync_every = sync_every.clone();
        let pull_every = pull_every.clone();
        let pull_on_open = pull_on_open.clone();
        let method = method.clone();
        let push = push.clone();
        let message = message.clone();
        let date_format = date_format.clone();
        Rc::new(move || {
            let mut settings: GitSettings = sync.settings();
            settings.enabled = enabled.is_active();
            settings.sync_interval = sync_every.value() as u32;
            settings.pull_interval = pull_every.value() as u32;
            settings.pull_on_open = pull_on_open.is_active();
            settings.method = if method.selected() == 1 {
                SyncMethod::Rebase
            } else {
                SyncMethod::Merge
            };
            settings.push = push.is_active();
            let text = message.text();
            if !text.trim().is_empty() {
                settings.commit_message = text.to_string();
            }
            let text = date_format.text();
            if !text.trim().is_empty() {
                settings.date_format = text.to_string();
            }
            if settings == sync.settings() {
                return;
            }
            if let (Err(e), Some(dialog)) = (sync.set_settings(settings), dialog.upgrade()) {
                dialog.add_toast(adw::Toast::new(&e));
            }
        })
    };
    for row in [&enabled, &pull_on_open, &push] {
        let save = save.clone();
        row.connect_active_notify(move |_| save());
    }
    for row in [&sync_every, &pull_every] {
        let save = save.clone();
        row.connect_value_notify(move |_| save());
    }
    let on_method = save.clone();
    method.connect_selected_notify(move |_| on_method());
    for row in [&message, &date_format] {
        let save = save.clone();
        row.connect_apply(move |_| save());
    }
    group
}

fn minutes_row(title: &str, value: u32) -> adw::SpinRow {
    let row = adw::SpinRow::with_range(0.0, 1440.0, 1.0);
    row.set_title(title);
    row.set_tooltip_text(Some("0 turns it off"));
    row.set_value(f64::from(value));
    row
}
