//! App-wide preferences in GSettings: recent vaults and window state.

use gtk::{gio, prelude::*};

use crate::config;

/// The app's settings, from the installed schema or, for runs from the build
/// tree, the schema build.rs compiled.
pub fn settings() -> gio::Settings {
    let default = gio::SettingsSchemaSource::default();
    let schema = default
        .as_ref()
        .and_then(|source| source.lookup(config::APP_ID, true))
        .or_else(|| {
            gio::SettingsSchemaSource::from_directory(config::SCHEMA_DIR, default.as_ref(), false)
                .ok()?
                .lookup(config::APP_ID, false)
        })
        .expect("the GSettings schema is installed or built");
    gio::Settings::new_full(&schema, None::<&gio::SettingsBackend>, None)
}

const MAX_RECENT: usize = 10;

/// How many icons the icon picker's Recent row holds.
pub const MAX_RECENT_ICONS: usize = 8;

/// Icons recently given to notes, most recent first.
pub fn recent_icons(settings: &gio::Settings) -> Vec<String> {
    settings
        .strv("recent-icons")
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Moves `icon` to the front of the recent icons.
pub fn add_recent_icon(settings: &gio::Settings, icon: &str) {
    let mut list = recent_icons(settings);
    list.retain(|i| i != icon);
    list.insert(0, icon.to_owned());
    list.truncate(MAX_RECENT_ICONS);
    let _ = settings.set_strv("recent-icons", list);
}

pub fn recent_vaults(settings: &gio::Settings) -> Vec<String> {
    settings
        .strv("recent-vaults")
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Moves `root` to the front of the recent vaults list.
pub fn add_recent_vault(settings: &gio::Settings, root: &str) {
    let mut list = recent_vaults(settings);
    list.retain(|r| r != root);
    list.insert(0, root.to_owned());
    list.truncate(MAX_RECENT);
    let _ = settings.set_strv("recent-vaults", list);
}

pub fn remove_recent_vault(settings: &gio::Settings, root: &str) {
    let mut list = recent_vaults(settings);
    list.retain(|r| r != root);
    let _ = settings.set_strv("recent-vaults", list);
}

pub fn set_open_vaults(settings: &gio::Settings, roots: &[String]) {
    let _ = settings.set_strv("open-vaults", roots);
}

pub fn open_vaults(settings: &gio::Settings) -> Vec<String> {
    settings
        .strv("open-vaults")
        .iter()
        .map(|s| s.to_string())
        .collect()
}
