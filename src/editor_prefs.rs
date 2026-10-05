//! Preferences → Editor, after the theme: fonts and the text column, how
//! notes open, and how typing behaves. Saved with the vault (`vault.json`,
//! and `appearance.json` for fonts and line length) and applied to open
//! notes straight away.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::pango;
use igneous_core::settings::{self as vault_settings, Appearance, EditorMode, VaultSettings};

use crate::window::{TextStyle, Window, desktop_font};

pub fn fill(page: &adw::PreferencesPage, window: &Window, dialog: &adw::PreferencesDialog) {
    let editor = window.ctx().settings.borrow().editor.clone();
    let readable = window.readable_line_length();

    let switch = |title: &str, subtitle: &str, active: bool| {
        adw::SwitchRow::builder()
            .title(title)
            .subtitle(subtitle)
            .active(active)
            .build()
    };

    page.add(&text_group(window, dialog, readable));

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
    let numbers = switch("Line Numbers in Source Mode", "", editor.show_line_numbers);
    let mentions = switch(
        "Linked Mentions at the End",
        "List the notes linking to a note below its text",
        editor.backlinks_in_document,
    );
    opening.add(&mode);
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
}

/// Changes appearance.json and applies it, or says why it couldn't.
fn save_appearance(
    window: &Window,
    dialog: &adw::PreferencesDialog,
    change: impl FnOnce(&mut Appearance),
) {
    let dir = window.ctx().vault.igneous_dir();
    // Never overwrite an appearance.json that can't be read.
    let result = vault_settings::load::<Appearance>(&dir).and_then(|mut appearance| {
        change(&mut appearance);
        vault_settings::save(&dir, &appearance).map(|_| appearance)
    });
    match result {
        Ok(appearance) => {
            window.set_readable_line_length(appearance.readable_line_length);
            window.set_text_style(TextStyle::from_appearance(&appearance));
        }
        Err(e) => dialog.add_toast(adw::Toast::new(&format!("Couldn’t save: {e}"))),
    }
}

/// A flat button that puts a setting back to the desktop's.
fn reset_button(tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name("edit-undo-symbolic")
        .tooltip_text(tooltip)
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build()
}

/// A font family picker for a row.
fn family_button(family: &str, monospace_only: bool) -> gtk::FontDialogButton {
    let dialog = gtk::FontDialog::new();
    if monospace_only {
        let filter = gtk::CustomFilter::new(|item| {
            if let Some(family) = item.downcast_ref::<pango::FontFamily>() {
                family.is_monospace()
            } else if let Some(face) = item.downcast_ref::<pango::FontFace>() {
                face.family().is_monospace()
            } else {
                true
            }
        });
        dialog.set_filter(Some(&filter));
    }
    let button = gtk::FontDialogButton::builder()
        .dialog(&dialog)
        .level(gtk::FontLevel::Family)
        .valign(gtk::Align::Center)
        .build();
    button.set_font_desc(&pango::FontDescription::from_string(family));
    button
}

/// Fonts, the base size and the text column's width.
fn text_group(
    window: &Window,
    dialog: &adw::PreferencesDialog,
    readable: bool,
) -> adw::PreferencesGroup {
    let style = window.text_style();
    let (document_family, document_size) = desktop_font(true);
    let (monospace_family, _) = desktop_font(false);
    let group = adw::PreferencesGroup::builder()
        .title("Text")
        .description("Saved with this vault")
        .build();
    // Set while a row is changed to match a reset, so it isn't saved as a
    // choice.
    let quiet = Rc::new(Cell::new(false));

    let font = adw::ActionRow::builder()
        .title("Font")
        .subtitle("Headings and other text are sized from the base size")
        .build();
    let font_button = family_button(style.family.as_deref().unwrap_or(&document_family), false);
    let font_reset = reset_button("Use the Desktop’s Document Font");
    font_reset.set_visible(style.family.is_some());
    font.add_suffix(&font_reset);
    font.add_suffix(&font_button);
    font.set_activatable_widget(Some(&font_button));

    let size = adw::SpinRow::with_range(6.0, 72.0, 0.5);
    size.set_title("Base Size");
    size.set_subtitle("Points");
    size.set_digits(1);
    size.set_value(style.size.unwrap_or(document_size));
    let size_reset = reset_button("Use the Desktop’s Document Font Size");
    size_reset.set_visible(style.size.is_some());
    size.add_suffix(&size_reset);

    let monospace = adw::ActionRow::builder()
        .title("Monospace Font")
        .subtitle("Code, math source and frontmatter")
        .build();
    let monospace_button = family_button(
        style.monospace.as_deref().unwrap_or(&monospace_family),
        true,
    );
    let monospace_reset = reset_button("Use the Desktop’s Monospace Font");
    monospace_reset.set_visible(style.monospace.is_some());
    monospace.add_suffix(&monospace_reset);
    monospace.add_suffix(&monospace_button);
    monospace.set_activatable_widget(Some(&monospace_button));

    let readable_row = adw::SwitchRow::builder()
        .title("Readable Line Length")
        .subtitle("Keep the text column narrow on wide windows")
        .active(readable)
        .build();
    let width = adw::SpinRow::with_range(320.0, 2400.0, 20.0);
    width.set_title("Text Width");
    width.set_subtitle("Pixels");
    width.set_value(f64::from(
        style
            .line_width
            .unwrap_or(igneous_editor::READABLE_WIDTH as u32),
    ));
    readable_row
        .bind_property("active", &width, "sensitive")
        .sync_create()
        .build();

    for row in [
        font.upcast_ref::<gtk::Widget>(),
        size.upcast_ref(),
        monospace.upcast_ref(),
        readable_row.upcast_ref(),
        width.upcast_ref(),
    ] {
        group.add(row);
    }

    let save = {
        let window = window.downgrade();
        let dialog = dialog.downgrade();
        let quiet = quiet.clone();
        Rc::new(move |change: &dyn Fn(&mut Appearance)| {
            if quiet.get() {
                return;
            }
            if let (Some(window), Some(dialog)) = (window.upgrade(), dialog.upgrade()) {
                save_appearance(&window, &dialog, change);
            }
        })
    };
    let family_of = |button: &gtk::FontDialogButton| {
        button
            .font_desc()
            .and_then(|d| d.family().map(|f| f.to_string()))
            .filter(|f| !f.is_empty())
    };

    let on_font = save.clone();
    let reset = font_reset.clone();
    font_button.connect_font_desc_notify(move |button| {
        let family = family_of(button);
        reset.set_visible(true);
        on_font(&|a| a.text_font = family.clone());
    });
    let on_reset = save.clone();
    let (button, family, quiet_reset) = (font_button.clone(), document_family, quiet.clone());
    font_reset.connect_clicked(move |reset| {
        on_reset(&|a| a.text_font = None);
        quiet_reset.set(true);
        button.set_font_desc(&pango::FontDescription::from_string(&family));
        quiet_reset.set(false);
        reset.set_visible(false);
    });

    let on_size = save.clone();
    let reset = size_reset.clone();
    size.connect_value_notify(move |row| {
        let value = row.value();
        reset.set_visible(true);
        on_size(&|a| a.font_size = Some(value));
    });
    let on_reset = save.clone();
    let (row, quiet_reset) = (size.clone(), quiet.clone());
    size_reset.connect_clicked(move |reset| {
        on_reset(&|a| a.font_size = None);
        quiet_reset.set(true);
        row.set_value(document_size);
        quiet_reset.set(false);
        reset.set_visible(false);
    });

    let on_monospace = save.clone();
    let reset = monospace_reset.clone();
    monospace_button.connect_font_desc_notify(move |button| {
        let family = family_of(button);
        reset.set_visible(true);
        on_monospace(&|a| a.monospace_font = family.clone());
    });
    let on_reset = save.clone();
    let (button, quiet_reset) = (monospace_button.clone(), quiet.clone());
    monospace_reset.connect_clicked(move |reset| {
        on_reset(&|a| a.monospace_font = None);
        quiet_reset.set(true);
        button.set_font_desc(&pango::FontDescription::from_string(&monospace_family));
        quiet_reset.set(false);
        reset.set_visible(false);
    });

    let on_readable = save.clone();
    readable_row.connect_active_notify(move |row| {
        let active = row.is_active();
        on_readable(&|a| a.readable_line_length = active);
    });
    let on_width = save;
    width.connect_value_notify(move |row| {
        let value = row.value() as u32;
        on_width(&|a| a.line_width = Some(value));
    });
    group
}
