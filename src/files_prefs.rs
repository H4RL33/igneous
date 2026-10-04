//! Preferences → Files & Links: where new notes and attachments go, how
//! deleting works, link updates on rename, and excluded files. Saved with
//! the vault in `vault.json`.

use std::rc::Rc;

use adw::prelude::*;
use igneous_core::settings::{self as vault_settings, Location, TrashMode, VaultSettings};

use crate::window::Window;

/// A combo row and folder entry for a [`Location`].
struct LocationRows {
    combo: adw::ComboRow,
    folder: adw::EntryRow,
}

const PLACES: [&str; 4] = [
    "Vault Folder",
    "Same Folder as the Note",
    "Folder…",
    "Subfolder of the Note’s Folder…",
];

impl LocationRows {
    fn new(title: &str, location: &Location) -> Self {
        let (selected, folder) = match location {
            Location::VaultRoot => (0, String::new()),
            Location::SameFolder => (1, String::new()),
            Location::Folder(f) => (2, f.clone()),
            Location::Subfolder(f) => (3, f.clone()),
        };
        let combo = adw::ComboRow::builder()
            .title(title)
            .model(&gtk::StringList::new(&PLACES))
            .selected(selected)
            .build();
        let folder = adw::EntryRow::builder()
            .title("Folder")
            .text(folder)
            .show_apply_button(true)
            .visible(selected >= 2)
            .build();
        combo
            .bind_property("selected", &folder, "visible")
            .transform_to(|_, selected: u32| Some(selected >= 2))
            .build();
        Self { combo, folder }
    }

    fn location(&self) -> Location {
        let folder = self.folder.text().trim().trim_matches('/').to_owned();
        match self.combo.selected() {
            1 => Location::SameFolder,
            2 if !folder.is_empty() => Location::Folder(folder),
            3 if !folder.is_empty() => Location::Subfolder(folder),
            _ if self.combo.selected() >= 2 => Location::SameFolder,
            _ => Location::VaultRoot,
        }
    }
}

pub fn page(window: &Window, dialog: &adw::PreferencesDialog) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Files & Links")
        .icon_name("folder-symbolic")
        .build();
    let settings = window.ctx().settings.borrow().clone();

    let creating = adw::PreferencesGroup::builder()
        .title("New Files")
        .description("Saved with this vault")
        .build();
    let notes = LocationRows::new("New Notes Go In", &settings.files.new_note_location);
    let attachments = LocationRows::new(
        "Pasted and Dropped Files Go In",
        &settings.files.attachment_location,
    );
    creating.add(&notes.combo);
    creating.add(&notes.folder);
    creating.add(&attachments.combo);
    creating.add(&attachments.folder);

    let deleting = adw::PreferencesGroup::builder().title("Deleting").build();
    let trash = adw::ComboRow::builder()
        .title("Deleted Files Go To")
        .model(&gtk::StringList::new(&[
            "The Trash",
            "The Vault’s .trash Folder",
        ]))
        .selected(match settings.files.trash {
            TrashMode::System => 0,
            TrashMode::VaultFolder => 1,
        })
        .build();
    let confirm = adw::SwitchRow::builder()
        .title("Ask Before Deleting")
        .active(settings.files.confirm_delete)
        .build();
    deleting.add(&trash);
    deleting.add(&confirm);

    let links = adw::PreferencesGroup::builder().title("Links").build();
    let update = adw::SwitchRow::builder()
        .title("Update Links When Renaming")
        .subtitle("Rewrite links to a note or folder when it’s renamed or moved")
        .active(settings.links.update_on_rename)
        .build();
    links.add(&update);

    let hiding = adw::PreferencesGroup::builder()
        .title("Excluded Files")
        .description(
            "Paths or patterns (such as Archive/ or *.tmp), separated by commas, hidden from \
             the file tree, search, the graph and the index. Dot-folders such as .obsidian are \
             always hidden. Takes effect when the vault is opened again.",
        )
        .build();
    let excluded = adw::EntryRow::builder()
        .title("Excluded")
        .text(settings.files.excluded.join(", "))
        .show_apply_button(true)
        .build();
    hiding.add(&excluded);

    for group in [&creating, &deleting, &links, &hiding] {
        page.add(group);
    }

    let notes = Rc::new(notes);
    let attachments = Rc::new(attachments);
    let save = {
        let window = window.downgrade();
        let dialog = dialog.downgrade();
        let (notes, attachments) = (notes.clone(), attachments.clone());
        let (trash, confirm, update, excluded) = (
            trash.clone(),
            confirm.clone(),
            update.clone(),
            excluded.clone(),
        );
        Rc::new(move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            let dir = window.ctx().vault.igneous_dir();
            // Never overwrite a vault.json that can't be read.
            let result = vault_settings::load::<VaultSettings>(&dir).and_then(|mut settings| {
                settings.files.new_note_location = notes.location();
                settings.files.attachment_location = attachments.location();
                settings.files.trash = if trash.selected() == 1 {
                    TrashMode::VaultFolder
                } else {
                    TrashMode::System
                };
                settings.files.confirm_delete = confirm.is_active();
                settings.links.update_on_rename = update.is_active();
                settings.files.excluded = excluded
                    .text()
                    .split(',')
                    .map(|p| p.trim().to_owned())
                    .filter(|p| !p.is_empty())
                    .collect();
                vault_settings::save(&dir, &settings).map(|_| settings)
            });
            match result {
                Ok(saved) => {
                    let mut current = window.ctx().settings.borrow_mut();
                    current.files = saved.files;
                    current.links = saved.links;
                }
                Err(e) => {
                    if let Some(dialog) = dialog.upgrade() {
                        dialog.add_toast(adw::Toast::new(&format!("Couldn’t save: {e}")));
                    }
                }
            }
        })
    };
    for combo in [&notes.combo, &attachments.combo, &trash] {
        let save = save.clone();
        combo.connect_selected_notify(move |_| save());
    }
    for entry in [&notes.folder, &attachments.folder, &excluded] {
        let save = save.clone();
        entry.connect_apply(move |_| save());
    }
    for switch in [&confirm, &update] {
        let save = save.clone();
        switch.connect_active_notify(move |_| save());
    }
    page
}
