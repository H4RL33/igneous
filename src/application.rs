//! The application: one window per open vault.

use std::cell::{Cell, OnceCell};
use std::path::{Path, PathBuf};

use adw::{prelude::*, subclass::prelude::*};
use gtk::{gio, glib};
use igneous_core::VaultPath;

use crate::vault_picker::VaultPicker;
use crate::window::Window;
use crate::{config, gsettings};

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Application {
        pub settings: OnceCell<gio::Settings>,
        /// Set while quitting, so closing windows keeps them in `open-vaults`.
        pub quitting: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Application {
        const NAME: &'static str = "IgneousApplication";
        type Type = super::Application;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for Application {}

    impl ApplicationImpl for Application {
        fn startup(&self) {
            self.parent_startup();
            crate::init();
            gtk::Window::set_default_icon_name(config::APP_ID);
            self.obj().setup_actions();
        }

        fn activate(&self) {
            self.parent_activate();
            self.obj().on_activate();
        }

        fn open(&self, files: &[gio::File], _hint: &str) {
            for file in files {
                if let Some(path) = file.path() {
                    self.obj().open_location(&path);
                }
            }
        }
    }

    impl GtkApplicationImpl for Application {}
    impl AdwApplicationImpl for Application {}
}

glib::wrapper! {
    pub struct Application(ObjectSubclass<imp::Application>)
        @extends adw::Application, gtk::Application, gio::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl Default for Application {
    fn default() -> Self {
        Self::new()
    }
}

impl Application {
    pub fn new() -> Self {
        // libadwaita looks for the shortcuts dialog and stylesheet in our
        // resources during startup, so they must be registered first.
        crate::init();
        glib::Object::builder()
            .property("application-id", config::APP_ID)
            .property("flags", gio::ApplicationFlags::HANDLES_OPEN)
            .property("resource-base-path", config::RESOURCE_BASE)
            .build()
    }

    pub fn settings(&self) -> &gio::Settings {
        self.imp().settings.get_or_init(gsettings::settings)
    }

    fn setup_actions(&self) {
        let quit = gio::ActionEntry::builder("quit")
            .activate(|app: &Self, _, _| app.quit_gracefully())
            .build();
        let about = gio::ActionEntry::builder("about")
            .activate(|app: &Self, _, _| app.show_about())
            .build();
        let new_window = gio::ActionEntry::builder("new-window")
            .activate(|app: &Self, _, _| app.show_picker())
            .build();
        self.add_action_entries([quit, about, new_window]);

        for (action, accels) in [
            ("app.quit", &["<Control>q"][..]),
            ("app.new-window", &["<Control><Shift>n"]),
            ("win.new-note", &["<Control>n"]),
            ("win.quick-switcher", &["<Control>o"]),
            ("win.close-tab", &["<Control>w"]),
            ("win.reopen-tab", &["<Control><Shift>t"]),
            ("win.toggle-sidebar", &["F9"]),
            ("win.save", &["<Control>s"]),
            ("win.rename-note", &["F2"]),
            ("win.preferences", &["<Control>comma"]),
            ("win.sync-now", &["<Control><Alt>s"]),
            ("win.command-palette", &["<Control>p"]),
            ("win.go-back", &["<Alt>Left"]),
            ("win.toggle-reading", &["<Control>e"]),
            ("win.search", &["<Control><Shift>f"]),
            ("win.go-forward", &["<Alt>Right"]),
        ] {
            self.set_accels_for_action(action, accels);
        }
    }

    fn on_activate(&self) {
        if let Some(window) = self.active_window() {
            window.present();
            return;
        }
        let mut opened = false;
        for root in gsettings::open_vaults(self.settings()) {
            if Path::new(&root).is_dir() && self.open_vault(Path::new(&root)).is_ok() {
                opened = true;
            }
        }
        if !opened {
            self.show_picker();
        }
    }

    pub fn vault_windows(&self) -> Vec<Window> {
        self.windows()
            .into_iter()
            .filter_map(|w| w.downcast::<Window>().ok())
            .collect()
    }

    /// Opens (or focuses) the window for the vault at `root`.
    pub fn open_vault(&self, root: &Path) -> Result<Window, String> {
        let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
        if let Some(window) = self.vault_windows().into_iter().find(|w| w.root() == root) {
            window.present();
            return Ok(window);
        }
        let window = Window::new(self, &root)?;
        window.present();
        gsettings::add_recent_vault(self.settings(), &root.to_string_lossy());
        self.save_open_vaults(None);
        Ok(window)
    }

    pub fn show_picker(&self) {
        let existing = self
            .windows()
            .into_iter()
            .find_map(|w| w.downcast::<VaultPicker>().ok());
        existing.unwrap_or_else(|| VaultPicker::new(self)).present();
    }

    /// Opens a folder as a vault, or a file in the vault containing it.
    fn open_location(&self, path: &Path) {
        if path.is_dir() {
            if let Err(e) = self.open_vault(path) {
                tracing::warn!(%e, "can't open vault");
                self.show_picker();
            }
            return;
        }
        let root = vault_root_for(path);
        match self.open_vault(&root) {
            Ok(window) => {
                let rel = std::fs::canonicalize(path)
                    .ok()
                    .and_then(|abs| VaultPath::from_fs(&window.root(), &abs).ok());
                if let Some(rel) = rel {
                    window.open_path(&rel, true);
                }
            }
            Err(e) => {
                tracing::warn!(%e, "can't open vault");
                self.show_picker();
            }
        }
    }

    /// Records which vaults are open, so they reopen next time. The closing
    /// window is left out unless it's the last one or the app is quitting.
    pub fn save_open_vaults(&self, closing: Option<&Window>) {
        let windows = self.vault_windows();
        let keep_closing = self.imp().quitting.get() || windows.len() <= 1;
        let roots: Vec<String> = windows
            .iter()
            .filter(|w| keep_closing || Some(*w) != closing)
            .map(|w| w.root().to_string_lossy().into_owned())
            .collect();
        gsettings::set_open_vaults(self.settings(), &roots);
    }

    fn quit_gracefully(&self) {
        self.imp().quitting.set(true);
        for window in self.windows() {
            window.close();
        }
        self.quit();
    }

    fn show_about(&self) {
        let about = adw::AboutDialog::builder()
            .application_name("Igneous")
            .application_icon(config::APP_ID)
            .developer_name("Harley Welsh")
            .version(config::VERSION)
            .license_type(gtk::License::Bsd3)
            .copyright("© 2026 Harley Welsh")
            .build();
        about.present(self.active_window().as_ref());
    }
}

/// The vault a loose file belongs to: the nearest folder above it containing
/// `.igneous` or `.obsidian`, else the file's own folder.
fn vault_root_for(file: &Path) -> PathBuf {
    let parent = file.parent().unwrap_or(file).to_path_buf();
    parent
        .ancestors()
        .find(|dir| dir.join(".igneous").is_dir() || dir.join(".obsidian").is_dir())
        .map(Path::to_path_buf)
        .unwrap_or(parent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_vault_roots() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("vault/.obsidian")).unwrap();
        std::fs::create_dir_all(root.join("vault/a/b")).unwrap();
        std::fs::create_dir_all(root.join("loose")).unwrap();
        assert_eq!(
            vault_root_for(&root.join("vault/a/b/n.md")),
            root.join("vault")
        );
        assert_eq!(vault_root_for(&root.join("loose/n.md")), root.join("loose"));
    }
}
