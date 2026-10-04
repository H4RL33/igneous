//! Igneous: read and edit Markdown vaults on GNOME.

mod application;
mod changes;
mod command_palette;
mod config;
mod editor_host;
mod files;
mod gsettings;
mod history;
mod image_page;
mod index;
mod inspector;
mod lint;
mod lint_prefs;
mod note_page;
mod preferences;
mod query_data;
mod quick_switcher;
mod sync;
mod sync_button;
mod text_page;
mod vault;
mod vault_picker;
mod window;
mod worker;

pub use application::Application;
pub use index::IndexService;
pub use note_page::{NotePage, State as NoteState};
pub use sync::{State as SyncState, SyncService};
pub use vault_picker::VaultPicker;
pub use window::Window;

use gtk::{gio, glib, prelude::*};

/// Registers the app's resources. Safe to call more than once.
pub fn init() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        gio::resources_register_include!("igneous.gresource").expect("app resources are valid");
        igneous_editor::init();
        // The application adds this itself on startup; windows created before
        // then (and in tests) need it too.
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::IconTheme::for_display(&display)
                .add_resource_path(&format!("{}/icons", config::RESOURCE_BASE));
            // Loaded here rather than as libadwaita's automatic style.css, so
            // windows get it even without a running application (tests).
            let css = gtk::CssProvider::new();
            css.load_from_resource(&format!("{}/igneous.css", config::RESOURCE_BASE));
            gtk::style_context_add_provider_for_display(
                &display,
                &css,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
}

pub fn run() -> glib::ExitCode {
    let filter = tracing_subscriber::EnvFilter::try_from_env("IGNEOUS_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
    glib::set_application_name("Igneous");
    gtk::init().expect("GTK can start");
    init();
    Application::new().run()
}
