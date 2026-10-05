//! Preferences → Editor: how notes open and how typing behaves. Saved with
//! the vault (`vault.json`, and `appearance.json` for line length) and
//! applied to open notes straight away.

use std::rc::Rc;

use adw::prelude::*;
use igneous_core::settings::{self as vault_settings, Appearance, EditorMode, VaultSettings};

use crate::window::Window;

pub fn page(window: &Window, dialog: &adw::PreferencesDialog) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Editor")
        .icon_name("document-edit-symbolic")
        .build();
    let editor = window.ctx().settings.borrow().editor.clone();
    let readable = window.readable_line_length();

    let switch = |title: &str, subtitle: &str, active: bool| {
        adw::SwitchRow::builder()
            .title(title)
            .subtitle(subtitle)
            .active(active)
            .build()
    };

    let opening = adw::PreferencesGroup::builder()
        .title("Notes")
        .description("Saved with this vault")
        .build();
    let mode = adw::ComboRow::builder()
        .title("Open Notes In")
        .model(&gtk::StringList::new(&[
            "Live Preview",
            "Source",
            "Reading",
        ]))
        .selected(match editor.default_mode {
            EditorMode::Live => 0,
            EditorMode::Source => 1,
            EditorMode::Reading => 2,
        })
        .build();
    let readable_row = switch(
        "Readable Line Length",
        "Keep the text column narrow on wide windows",
        readable,
    );
    let numbers = switch("Line Numbers in Source Mode", "", editor.show_line_numbers);
    let mentions = switch(
        "Linked Mentions at the End",
        "List the notes linking to a note below its text",
        editor.backlinks_in_document,
    );
    opening.add(&mode);
    opening.add(&readable_row);
    opening.add(&numbers);
    opening.add(&mentions);

    let typing = adw::PreferencesGroup::builder().title("Typing").build();
    let vim = switch(
        "Vim Mode",
        "Vim keybindings, with :w to save",
        editor.vim_mode,
    );
    let spellcheck = switch(
        "Check Spelling",
        "In the desktop’s language; right-click a word for corrections",
        editor.spellcheck,
    );
    let lists = switch(
        "Smart Lists",
        "Enter continues lists and quotes; Tab and Shift+Tab indent list items",
        editor.smart_lists,
    );
    let brackets = switch("Pair Brackets", "", editor.auto_pair_brackets);
    let markers = switch(
        "Wrap Selections in Markdown",
        "Typing * _ ` = or ~ with text selected wraps it",
        editor.auto_pair_markdown,
    );
    let tabs = switch(
        "Indent With Tabs",
        "As Obsidian does",
        editor.indent_with_tabs,
    );
    let tab_width = adw::SpinRow::with_range(1.0, 8.0, 1.0);
    tab_width.set_title("Tab Width");
    tab_width.set_value(f64::from(editor.tab_size));
    for row in [&vim, &spellcheck, &lists, &brackets, &markers, &tabs] {
        typing.add(row);
    }
    typing.add(&tab_width);

    let saving = adw::PreferencesGroup::builder()
        .title("Saving")
        .description(
            "Notes save themselves shortly after you stop typing, and when you switch away",
        )
        .build();
    let delay = adw::SpinRow::with_range(0.5, 30.0, 0.5);
    delay.set_title("Save After (Seconds)");
    delay.set_digits(1);
    delay.set_value(f64::from(editor.autosave_delay_ms) / 1000.0);
    saving.add(&delay);

    page.add(&opening);
    page.add(&typing);
    page.add(&saving);

    let save = {
        let window = window.downgrade();
        let dialog = dialog.downgrade();
        let mentions = mentions.clone();
        let (mode, numbers, vim, spellcheck, lists, brackets, markers, tabs, tab_width, delay) = (
            mode.clone(),
            numbers.clone(),
            vim.clone(),
            spellcheck.clone(),
            lists.clone(),
            brackets.clone(),
            markers.clone(),
            tabs.clone(),
            tab_width.clone(),
            delay.clone(),
        );
        Rc::new(move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            let dir = window.ctx().vault.igneous_dir();
            let result = vault_settings::load::<VaultSettings>(&dir).and_then(|mut settings| {
                let e = &mut settings.editor;
                e.default_mode = match mode.selected() {
                    1 => EditorMode::Source,
                    2 => EditorMode::Reading,
                    _ => EditorMode::Live,
                };
                e.show_line_numbers = numbers.is_active();
                e.backlinks_in_document = mentions.is_active();
                e.vim_mode = vim.is_active();
                e.spellcheck = spellcheck.is_active();
                e.smart_lists = lists.is_active();
                e.auto_pair_brackets = brackets.is_active();
                e.auto_pair_markdown = markers.is_active();
                e.indent_with_tabs = tabs.is_active();
                e.tab_size = tab_width.value() as u32;
                e.autosave_delay_ms = (delay.value() * 1000.0) as u32;
                vault_settings::save(&dir, &settings).map(|_| settings)
            });
            match result {
                Ok(settings) => {
                    window.ctx().settings.borrow_mut().editor = settings.editor;
                    window.apply_editor_settings();
                    window.update_linked_mentions();
                }
                Err(e) => {
                    if let Some(dialog) = dialog.upgrade() {
                        dialog.add_toast(adw::Toast::new(&format!("Couldn’t save: {e}")));
                    }
                }
            }
        })
    };
    for row in [
        &numbers,
        &mentions,
        &vim,
        &spellcheck,
        &lists,
        &brackets,
        &markers,
        &tabs,
    ] {
        let save = save.clone();
        row.connect_active_notify(move |_| save());
    }
    for row in [&tab_width, &delay] {
        let save = save.clone();
        row.connect_value_notify(move |_| save());
    }
    let on_mode = save.clone();
    mode.connect_selected_notify(move |_| on_mode());

    let window_weak = window.downgrade();
    let dialog_weak = dialog.downgrade();
    readable_row.connect_active_notify(move |row| {
        let Some(window) = window_weak.upgrade() else {
            return;
        };
        let dir = window.ctx().vault.igneous_dir();
        let result = vault_settings::load::<Appearance>(&dir).and_then(|mut appearance| {
            appearance.readable_line_length = row.is_active();
            vault_settings::save(&dir, &appearance)
        });
        match result {
            Ok(_) => window.set_readable_line_length(row.is_active()),
            Err(e) => {
                if let Some(dialog) = dialog_weak.upgrade() {
                    dialog.add_toast(adw::Toast::new(&format!("Couldn’t save: {e}")));
                }
            }
        }
    });
    page
}
