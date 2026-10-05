//! The property names used across the vault, for Add Property's menu. The
//! menu is made once and shared by every note, and names are appended to
//! it as notes gain properties. They're kept in `.igneous/properties.json`,
//! so the menu is complete as soon as a vault opens; the file is written
//! when a property is made in Igneous, as opening a vault writes nothing.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;

use gtk::gio;
use igneous_core::settings::{self as vault_settings, PropertyNames as NamesFile};

pub struct PropertyNames {
    igneous_dir: PathBuf,
    /// Every name, in the menu's order.
    names: RefCell<Vec<String>>,
    known: RefCell<HashSet<String>>,
    menu: gio::Menu,
}

impl PropertyNames {
    /// Loads properties.json; one that can't be read starts the list empty
    /// and is left alone.
    pub fn load(igneous_dir: PathBuf) -> Self {
        let file = vault_settings::load::<NamesFile>(&igneous_dir).unwrap_or_else(|e| {
            tracing::warn!(%e, "can't read the property names");
            NamesFile::default()
        });
        let this = Self {
            igneous_dir,
            names: RefCell::default(),
            known: RefCell::default(),
            menu: gio::Menu::new(),
        };
        this.learn(file.names.iter().map(String::as_str));
        this
    }

    /// Every name, as the menu Add Property shows.
    pub fn menu(&self) -> gio::MenuModel {
        self.menu.clone().into()
    }

    /// Appends names the notes use (from the index) to the menu. They're
    /// saved along with the next new property.
    pub fn learn<'a>(&self, names: impl IntoIterator<Item = &'a str>) {
        let mut known = self.known.borrow_mut();
        for name in names.into_iter().map(str::trim) {
            if !name.is_empty() && known.insert(name.to_owned()) {
                self.names.borrow_mut().push(name.to_owned());
                self.menu
                    .append_item(&igneous_editor::property_name_item(name));
            }
        }
    }

    /// Makes the list just `names` (what the notes use now, after a rescan)
    /// and saves it, returning how many there are.
    pub fn replace<'a>(&self, names: impl IntoIterator<Item = &'a str>) -> Result<usize, String> {
        self.names.borrow_mut().clear();
        self.known.borrow_mut().clear();
        self.menu.remove_all();
        self.learn(names);
        let names = self.names.borrow().clone();
        let count = names.len();
        // Keep anything else in the file, and never overwrite one that
        // can't be read.
        let mut file = vault_settings::load::<NamesFile>(&self.igneous_dir)
            .map_err(|e| format!("Couldn’t read the property names: {e}"))?;
        file.names = names;
        vault_settings::save(&self.igneous_dir, &file)
            .map_err(|e| format!("Couldn’t save the property names: {e}"))?;
        Ok(count)
    }

    /// Notes a property just made in Igneous, saving every name known so
    /// far to properties.json.
    pub fn add(&self, name: &str) {
        self.learn([name]);
        // Re-read first, so names added elsewhere (a Git pull, say) stay.
        // A file that can't be read isn't overwritten.
        match vault_settings::load::<NamesFile>(&self.igneous_dir) {
            Ok(mut file) => {
                let added = file.add(self.names.borrow().iter().map(String::as_str));
                if !added.is_empty()
                    && let Err(e) = vault_settings::save(&self.igneous_dir, &file)
                {
                    tracing::warn!(%e, "can't save the property names");
                }
            }
            Err(e) => tracing::warn!(%e, "can't read the property names"),
        }
    }
}
