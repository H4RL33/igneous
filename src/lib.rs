//! Igneous: read and edit Markdown vaults on GNOME.

mod application;
mod base_page;
mod base_view;
mod changes;
mod command_palette;
mod config;
mod editor_host;
mod editor_prefs;
mod files;
mod graph_data;
mod graph_page;
mod graph_view;
mod gsettings;
mod history;
mod image_page;
mod index;
mod inspector;
mod lint;
mod lint_prefs;
mod local_graph;
mod note_page;
mod preferences;
mod query_data;
mod quick_switcher;
mod search_pane;
mod sync;
mod sync_button;
mod tags_pane;
mod text_page;
mod vault;
mod vault_picker;
mod window;
mod worker;

pub use application::Application;
pub use base_page::BasePage;
pub use graph_data::GraphModel;
pub use graph_page::GraphPage;
pub use graph_view::GraphView;
pub use index::IndexService;
pub use note_page::{NotePage, State as NoteState};
pub use sync::{State as SyncState, SyncService};
pub use vault::VaultContext;
pub use vault_picker::VaultPicker;
pub use window::Window;

use gtk::{gio, glib, prelude::*};

/// Registers the app's resources. Safe to call more than once.
pub fn init() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        // libadwaita's styles and icons; an AdwApplication does this itself,
        // but windows made without one (tests) need it too.
        let _ = adw::init();
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
    // Sets the locale from the environment, too.
    gtk::init().expect("GTK can start");
    // Translations (po/README.md), before any UI is built: GtkBuilder looks up
    // the Blueprint files' `_()` strings in the default domain set here.
    if let Err(e) = gettextrs::bindtextdomain(config::GETTEXT_PACKAGE, config::LOCALEDIR)
        .and_then(|_| gettextrs::bind_textdomain_codeset(config::GETTEXT_PACKAGE, "UTF-8"))
        .and_then(|_| gettextrs::textdomain(config::GETTEXT_PACKAGE))
    {
        tracing::warn!(%e, "translations unavailable");
    }
    init();
    Application::new().run()
}
