//! Daily notes and templates (Preferences → Plugins), and the startup note
//! and file recovery (Preferences → Files & Links). Saved with the vault in
//! `vault.json`.

use std::rc::Rc;

use adw::prelude::*;
use igneous_core::VaultPath;
use igneous_core::settings::{self as vault_settings, VaultSettings};

use crate::window::Window;

/// Today's daily note name in `format`, for the format row's subtitle.
fn preview(format: &str) -> String {
    let settings = igneous_core::settings::DailyNoteSettings {
        format: format.to_owned(),
        ..Default::default()
    };
    match crate::daily_notes::path_for(&settings, crate::daily_notes::today()) {
        Some(path) => format!("Today: {}", path.as_str().trim_end_matches(".md")),
        None => "Not a valid file name".to_owned(),
    }
}

/// An optional vault path from an entry: empty means none.
fn optional_path(text: &str) -> Result<Option<VaultPath>, ()> {
    let text = text.trim().trim_matches('/');
    if text.is_empty() {
        return Ok(None);
    }
    let with_ext = if text.contains('.') {
        text.to_owned()
    } else {
        format!("{text}.md")
    };
    VaultPath::new(&with_ext).map(Some).map_err(|_| ())
}

/// The groups, for the pages they belong on.
pub struct Groups {
    pub daily: adw::PreferencesGroup,
    pub templates: adw::PreferencesGroup,
    pub opening: adw::PreferencesGroup,
    pub recovery: adw::PreferencesGroup,
}

pub fn groups(window: &Window, dialog: &adw::PreferencesDialog) -> Groups {
    let settings = window.ctx().settings.borrow().clone();

    let entry = |title: &str, text: &str| {
        adw::EntryRow::builder()
            .title(title)
            .text(text)
            .show_apply_button(true)
            .build()
    };

    let daily = adw::PreferencesGroup::builder()
        .title("Daily Notes")
        .description("Saved with this vault")
        .build();
    let daily_folder = entry("Folder", &settings.daily_notes.folder);
    let daily_format = entry("Name Format", &settings.daily_notes.format);
    let format_preview = gtk::Label::builder()
        .label(preview(&settings.daily_notes.format))
        .css_classes(["dim-label", "caption"])
        .valign(gtk::Align::Center)
        .build();
    daily_format.add_suffix(&format_preview);
    daily_format.set_tooltip_text(Some(
        "Moment.js tokens, such as YYYY-MM-DD; slashes make folders",
    ));
    let daily_template = entry(
        "Template",
        settings
            .daily_notes
            .template
            .as_ref()
            .map_or("", |p| p.as_str()),
    );
    daily.add(&daily_folder);
    daily.add(&daily_format);
    daily.add(&daily_template);

    let templates = adw::PreferencesGroup::builder()
        .title("Templates")
        .description("{{title}}, {{date}} and {{time}} are filled in; {{date:FORMAT}} and {{time:FORMAT}} take Moment.js formats")
        .build();
    let template_folder = entry(
        "Templates Folder",
        settings.templates.folder.as_deref().unwrap_or(""),
    );
    let date_format = entry("Date Format", &settings.templates.date_format);
    let time_format = entry("Time Format", &settings.templates.time_format);
    templates.add(&template_folder);
    templates.add(&date_format);
    templates.add(&time_format);

    let opening = adw::PreferencesGroup::builder()
        .title("Opening the Vault")
        .build();
    let startup = entry(
        "Startup Note",
        settings.startup_note.as_ref().map_or("", |p| p.as_str()),
    );
    startup.set_tooltip_text(Some(
        "Opened whenever the vault opens; leave empty for none",
    ));
    opening.add(&startup);

    let recovery = adw::PreferencesGroup::builder()
        .title("File Recovery")
        .description("Snapshots of notes as they’re edited, kept outside the vault; see Note Snapshots in the main menu")
        .build();
    let spin = |title: &str, subtitle: &str, max: f64, value: u32| {
        let row = adw::SpinRow::with_range(0.0, max, 1.0);
        row.set_title(title);
        row.set_subtitle(subtitle);
        row.set_value(f64::from(value));
        row
    };
    let interval = spin(
        "Snapshot Every",
        "Minutes between snapshots of a note; 0 for every save",
        120.0,
        settings.recovery.interval_minutes,
    );
    let keep = spin("Keep For", "Days", 365.0, settings.recovery.keep_days);
    let size = spin(
        "Use Up To",
        "Megabytes for this vault",
        10_000.0,
        settings.recovery.max_megabytes,
    );
    recovery.add(&interval);
    recovery.add(&keep);
    recovery.add(&size);

    let save = {
        let window = window.downgrade();
        let dialog = dialog.downgrade();
        let rows = (
            daily_folder.clone(),
            daily_format.clone(),
            daily_template.clone(),
            template_folder.clone(),
            date_format.clone(),
            time_format.clone(),
            startup.clone(),
            interval.clone(),
            keep.clone(),
            size.clone(),
        );
        Rc::new(move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            let toast = |message: &str| {
                if let Some(dialog) = dialog.upgrade() {
                    dialog.add_toast(adw::Toast::new(message));
                }
            };
            let (
                daily_folder,
                daily_format,
                daily_template,
                template_folder,
                date_format,
                time_format,
                startup,
                interval,
                keep,
                size,
            ) = &rows;
            let (Ok(template), Ok(startup)) = (
                optional_path(&daily_template.text()),
                optional_path(&startup.text()),
            ) else {
                toast("That isn’t a valid path in the vault");
                return;
            };
            let dir = window.ctx().vault.igneous_dir();
            // Never overwrite a vault.json that can't be read.
            let result = vault_settings::load::<VaultSettings>(&dir).and_then(|mut s| {
                s.daily_notes.folder = daily_folder.text().trim().trim_matches('/').to_owned();
                let format = daily_format.text().trim().to_owned();
                if !format.is_empty() {
                    s.daily_notes.format = format;
                }
                s.daily_notes.template = template;
                let folder = template_folder.text().trim().trim_matches('/').to_owned();
                s.templates.folder = (!folder.is_empty()).then_some(folder);
                for (row, value) in [
                    (date_format, &mut s.templates.date_format),
                    (time_format, &mut s.templates.time_format),
                ] {
                    let text = row.text().trim().to_owned();
                    if !text.is_empty() {
                        *value = text;
                    }
                }
                s.startup_note = startup;
                s.recovery.interval_minutes = interval.value() as u32;
                s.recovery.keep_days = keep.value() as u32;
                s.recovery.max_megabytes = size.value() as u32;
                vault_settings::save(&dir, &s).map(|_| s)
            });
            match result {
                Ok(saved) => {
                    let mut settings = window.ctx().settings.borrow_mut();
                    settings.daily_notes = saved.daily_notes;
                    settings.templates = saved.templates;
                    settings.startup_note = saved.startup_note;
                    settings.recovery = saved.recovery;
                }
                Err(e) => toast(&format!("Couldn’t save: {e}")),
            }
        })
    };
    for row in [
        &daily_folder,
        &daily_format,
        &daily_template,
        &template_folder,
        &date_format,
        &time_format,
        &startup,
    ] {
        let save = save.clone();
        row.connect_apply(move |_| save());
    }
    daily_format.connect_changed(move |row| format_preview.set_label(&preview(&row.text())));
    for row in [&interval, &keep, &size] {
        let save = save.clone();
        row.connect_value_notify(move |_| save());
    }
    Groups {
        daily,
        templates,
        opening,
        recovery,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_from_entries() {
        assert_eq!(optional_path("  "), Ok(None));
        assert_eq!(
            optional_path("Templates/Daily"),
            Ok(Some(VaultPath::new("Templates/Daily.md").unwrap()))
        );
        assert_eq!(
            optional_path("/Home.md"),
            Ok(Some(VaultPath::new("Home.md").unwrap()))
        );
    }
}
