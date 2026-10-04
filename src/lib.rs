//! Igneous: read and edit Markdown vaults on GNOME.

mod application;
mod config;
mod files;
mod gsettings;
mod image_page;
mod note_page;
mod quick_switcher;
mod vault;
mod vault_picker;
mod window;

pub use application::Application;
pub use note_page::{NotePage, State as NoteState};
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
