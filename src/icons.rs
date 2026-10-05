//! Custom icons for notes: any symbolic icon from the icon theme, saved in
//! `.igneous/icons.json` and shown in the file tree, on tabs and in
//! bookmarks. They follow notes that Igneous renames, moves or deletes.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib, subclass::prelude::*};
use igneous_core::VaultPath;
use igneous_core::settings::{self as vault_settings, Icons, SettingsError};

use crate::window::Window;

/// The vault's custom icons.
pub struct IconStore {
    igneous_dir: PathBuf,
    /// icons.json as last read or written.
    icons: RefCell<Icons>,
}

impl IconStore {
    /// Loads icons.json; if it can't be read, no icons show and the reason
    /// is returned.
    pub fn load(igneous_dir: PathBuf) -> (Self, Option<String>) {
        let (icons, error) = match vault_settings::load::<Icons>(&igneous_dir) {
            Ok(icons) => (icons, None),
            Err(e) => (Icons::default(), Some(e.to_string())),
        };
        let store = Self {
            igneous_dir,
            icons: RefCell::new(icons),
        };
        (store, error)
    }

    /// The custom icon set for `path`, if any.
    pub fn get(&self, path: &VaultPath) -> Option<String> {
        self.icons.borrow().get(path).map(str::to_owned)
    }

    /// The custom icon to show for `path`: none if the icon theme lacks it,
    /// as it may on another computer.
    pub fn shown(&self, path: &VaultPath) -> Option<String> {
        let icon = self.get(path)?;
        let Some(display) = gdk::Display::default() else {
            return Some(icon);
        };
        gtk::IconTheme::for_display(&display)
            .has_icon(&icon)
            .then_some(icon)
    }

    /// Re-reads icons.json, applies `change` and saves the result if it
    /// differs, so edits made elsewhere (a Git pull, say) aren't lost. A
    /// file that can't be read is never overwritten.
    fn update<T>(&self, change: impl FnOnce(&mut Icons) -> T) -> Result<T, String> {
        let mut icons = vault_settings::load::<Icons>(&self.igneous_dir).map_err(|e| match e {
            SettingsError::Io(e) => format!("Couldn’t read the icons: {e}"),
            e => format!("icons.json can’t be read, so it wasn’t changed: {e}"),
        })?;
        let before = icons.clone();
        let result = change(&mut icons);
        let saved = if icons == before {
            Ok(false)
        } else {
            vault_settings::save(&self.igneous_dir, &icons)
        };
        self.icons.replace(icons);
        saved.map_err(|e| format!("Couldn’t save the icons: {e}"))?;
        Ok(result)
    }

    /// Sets the icon for `path`, or removes it with `None`. Returns whether
    /// that changed anything.
    pub fn set(&self, path: &VaultPath, icon: Option<&str>) -> Result<bool, String> {
        self.update(|icons| icons.set(path, icon))
    }

    /// Moves the icons of a renamed file, or of everything in a renamed
    /// folder.
    pub fn follow_rename(&self, from: &VaultPath, to: &VaultPath) -> Result<bool, String> {
        if !self.has_under(from) {
            return Ok(false);
        }
        self.update(|icons| icons.follow_rename(from, to))
    }

    /// Removes the icons of a deleted file or folder, returning them.
    pub fn forget(&self, path: &VaultPath) -> Result<Vec<(VaultPath, String)>, String> {
        if !self.has_under(path) {
            return Ok(Vec::new());
        }
        self.update(|icons| icons.remove_under(path))
    }

    /// Puts back icons that [`IconStore::forget`] removed.
    pub fn restore(&self, entries: &[(VaultPath, String)]) -> Result<bool, String> {
        if entries.is_empty() {
            return Ok(false);
        }
        self.update(|icons| {
            entries.iter().fold(false, |changed, (path, icon)| {
                icons.set(path, Some(icon)) || changed
            })
        })
    }

    fn has_under(&self, path: &VaultPath) -> bool {
        self.icons
            .borrow()
            .icons
            .keys()
            .any(|p| p.starts_with(path))
    }
}

/// Every symbolic icon in the display's icon theme, sorted, each once.
fn symbolic_icons(display: &gdk::Display) -> Vec<String> {
    let mut names: Vec<String> = gtk::IconTheme::for_display(display)
        .icon_names()
        .into_iter()
        .filter(|name| name.ends_with("-symbolic"))
        .map(String::from)
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// How names and queries are compared: lower case, without `-symbolic`,
/// with dashes and underscores as spaces.
fn search_key(text: &str) -> String {
    let text = text.trim().to_lowercase();
    text.strip_suffix("-symbolic")
        .unwrap_or(&text)
        .replace(['-', '_'], " ")
}

/// Whether the icon `name` contains every word of the query.
fn matches(words: &[String], name: &str) -> bool {
    let key = search_key(name);
    words.iter().all(|word| key.contains(word.as_str()))
}

fn query_words(query: &str) -> Vec<String> {
    search_key(query)
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

pub fn install_actions(
    klass: &mut <crate::window::imp::Window as glib::subclass::types::ObjectSubclass>::Class,
) {
    klass.install_action("win.set-icon", None, |w, _, _| match w.selected_path() {
        Some(path) => w.icon_dialog(path),
        None => w.toast("Open a note to set its icon"),
    });
    klass.install_action(
        "win.file-set-icon",
        Some(glib::VariantTy::STRING),
        |w, _, p| {
            if let Some(path) = p
                .and_then(|p| p.get::<String>())
                .and_then(|p| VaultPath::new(&p).ok())
            {
                w.icon_dialog(path);
            }
        },
    );
}

impl Window {
    /// The custom icon set for `path`, if any.
    pub fn note_icon(&self, path: &VaultPath) -> Option<String> {
        self.ctx().icons.get(path)
    }

    /// Gives `path` a custom icon, or its usual one again with `None`.
    pub fn set_note_icon(&self, path: &VaultPath, icon: Option<&str>) {
        match self.ctx().icons.set(path, icon) {
            Ok(true) => self.show_icons(),
            Ok(false) => {}
            Err(e) => self.toast(&e),
        }
    }

    /// Moves icons along with a renamed file or folder.
    pub(crate) fn icons_follow_rename(&self, from: &VaultPath, to: &VaultPath) {
        match self.ctx().icons.follow_rename(from, to) {
            Ok(true) => self.show_icons(),
            Ok(false) => {}
            Err(e) => self.toast(&e),
        }
    }

    /// Drops the icons of a deleted file or folder, returning them so an
    /// undo can put them back.
    pub(crate) fn forget_icons(&self, path: &VaultPath) -> Vec<(VaultPath, String)> {
        match self.ctx().icons.forget(path) {
            Ok(gone) => {
                if !gone.is_empty() {
                    self.show_icons();
                }
                gone
            }
            Err(e) => {
                self.toast(&e);
                Vec::new()
            }
        }
    }

    pub(crate) fn restore_icons(&self, entries: &[(VaultPath, String)]) {
        match self.ctx().icons.restore(entries) {
            Ok(true) => self.show_icons(),
            Ok(false) => {}
            Err(e) => self.toast(&e),
        }
    }

    /// Shows changed icons in the file tree, on tabs and in bookmarks.
    fn show_icons(&self) {
        let imp = self.imp();
        if let Some(tree) = imp.tree.get() {
            tree.refresh_icons();
        }
        let view = &imp.tab_view;
        for page in (0..view.n_pages()).map(|i| view.nth_page(i)) {
            if let Some(path) = Self::page_path(&page) {
                self.update_tab(&page, &path);
            }
        }
        if let Some(bookmarks) = imp.bookmarks.get() {
            bookmarks.rebuild();
        }
    }

    /// The icon on the file tree row for `path`, if the row is showing.
    pub fn sidebar_icon(&self, path: &VaultPath) -> Option<String> {
        let tree = self.imp().tree.get()?;
        let model = &tree.model;
        (0..model.n_items())
            .filter_map(|i| {
                model
                    .item(i)
                    .and_downcast::<gtk::TreeListRow>()?
                    .item()
                    .and_downcast::<crate::files::FileItem>()
            })
            .find(|item| item.path() == *path)
            .map(|item| item.icon_name())
    }

    /// The icon on the tab showing `path`, if it has one.
    pub fn tab_icon(&self, path: &VaultPath) -> Option<String> {
        let view = &self.imp().tab_view;
        let page = (0..view.n_pages())
            .map(|i| view.nth_page(i))
            .find(|p| Self::page_path(p).as_ref() == Some(path))?;
        let icon = page.icon()?.downcast::<gio::ThemedIcon>().ok()?;
        icon.names().first().map(|n| n.to_string())
    }

    /// Shows the icon picker for `path`.
    pub fn icon_dialog(&self, path: VaultPath) {
        let current = self.note_icon(&path);
        let names = symbolic_icons(&WidgetExt::display(self));
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let icons = gtk::StringList::new(&names);

        let words: Rc<RefCell<Vec<String>>> = Rc::default();
        let filter = gtk::CustomFilter::new({
            let words = words.clone();
            move |obj| {
                obj.downcast_ref::<gtk::StringObject>()
                    .is_some_and(|name| matches(&words.borrow(), &name.string()))
            }
        });
        let filtered = gtk::FilterListModel::new(Some(icons), Some(filter.clone()));
        let selection = gtk::SingleSelection::builder()
            .model(&filtered)
            .autoselect(false)
            .can_unselect(true)
            .selected(gtk::INVALID_LIST_POSITION)
            .build();
        let grid = gtk::GridView::builder()
            .model(&selection)
            .factory(&icon_factory())
            .single_click_activate(true)
            .min_columns(4)
            .max_columns(12)
            .build();
        grid.update_property(&[gtk::accessible::Property::Label("Icons")]);

        let entry = gtk::SearchEntry::builder()
            .placeholder_text("Search icons")
            .hexpand(true)
            .build();
        let empty = adw::StatusPage::builder()
            .icon_name("edit-find-symbolic")
            .title("No Matching Icons")
            .css_classes(["compact"])
            .build();
        let stack = gtk::Stack::new();
        stack.add_named(
            &gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .vexpand(true)
                .child(&grid)
                .build(),
            Some("icons"),
        );
        stack.add_named(&empty, Some("empty"));

        let reset = gtk::Button::builder()
            .label("_Reset to Default")
            .use_underline(true)
            .sensitive(current.is_some())
            .build();
        let bottom = gtk::ActionBar::new();
        bottom.set_center_widget(Some(&reset));

        let header = adw::HeaderBar::builder().title_widget(&entry).build();
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.add_bottom_bar(&bottom);
        toolbar.set_content(Some(&stack));
        let (name, _) = crate::files::display_name(&path, false);
        let dialog = adw::Dialog::builder()
            .title(format!("Icon for “{name}”"))
            .content_width(560)
            .content_height(480)
            .child(&toolbar)
            .build();

        let choose: Rc<dyn Fn(Option<String>)> = {
            let window = self.downgrade();
            let dialog = dialog.downgrade();
            Rc::new(move |icon| {
                if let Some(window) = window.upgrade() {
                    window.set_note_icon(&path, icon.as_deref());
                }
                if let Some(dialog) = dialog.upgrade() {
                    dialog.close();
                }
            })
        };
        let icon_at = {
            let selection = selection.clone();
            move |position: u32| {
                selection
                    .item(position)
                    .and_downcast::<gtk::StringObject>()
                    .map(|s| s.string().to_string())
            }
        };
        {
            let choose = choose.clone();
            let icon_at = icon_at.clone();
            grid.connect_activate(move |_, position| {
                if let Some(icon) = icon_at(position) {
                    choose(Some(icon));
                }
            });
        }
        {
            let choose = choose.clone();
            reset.connect_clicked(move |_| choose(None));
        }
        {
            let dialog = dialog.downgrade();
            entry.connect_stop_search(move |_| {
                if let Some(dialog) = dialog.upgrade() {
                    dialog.close();
                }
            });
        }
        // Enter picks the highlighted icon, or the first match.
        {
            let selection = selection.clone();
            entry.connect_activate(move |_| {
                let position = match selection.selected() {
                    gtk::INVALID_LIST_POSITION => 0,
                    position => position,
                };
                if let Some(icon) = icon_at(position) {
                    choose(Some(icon));
                }
            });
        }
        entry.connect_search_changed(move |entry| {
            words.replace(query_words(&entry.text()));
            filter.changed(gtk::FilterChange::Different);
            let found = filtered.n_items() > 0;
            stack.set_visible_child_name(if found { "icons" } else { "empty" });
        });
        // Down moves from the search into the icons.
        let keys = gtk::EventControllerKey::new();
        {
            let grid = grid.downgrade();
            keys.connect_key_pressed(move |_, key, _, _| {
                if key != gdk::Key::Down {
                    return glib::Propagation::Proceed;
                }
                if let Some(grid) = grid.upgrade() {
                    grid.child_focus(gtk::DirectionType::TabForward);
                }
                glib::Propagation::Stop
            });
        }
        entry.add_controller(keys);

        // Start at the icon the note has now.
        if let Some(current) = &current
            && let Some(position) = names.iter().position(|n| *n == current.as_str())
        {
            let position = position as u32;
            selection.set_selected(position);
            grid.scroll_to(position, gtk::ListScrollFlags::NONE, None);
        }
        dialog.present(Some(self));
        entry.grab_focus();
    }
}

fn icon_factory() -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, obj| {
        let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
        let image = gtk::Image::builder()
            .pixel_size(32)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        item.set_child(Some(&image));
    });
    factory.connect_bind(|_, obj| {
        let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
        let Some(name) = item
            .item()
            .and_downcast::<gtk::StringObject>()
            .map(|s| s.string())
        else {
            return;
        };
        let image = item.child().and_downcast::<gtk::Image>().unwrap();
        image.set_icon_name(Some(&name));
        image.set_tooltip_text(Some(&name));
        item.set_accessible_label(&name);
    });
    factory
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(query: &str, name: &str) -> bool {
        matches(&query_words(query), name)
    }

    #[test]
    fn search_ignores_symbolic_and_dashes() {
        assert!(found("", "go-home-symbolic"));
        assert!(found("home", "go-home-symbolic"));
        assert!(found("go home", "go-home-symbolic"));
        assert!(found("go-home", "go-home-symbolic"));
        assert!(found("Go-Home-Symbolic", "go-home-symbolic"));
        assert!(found("home go", "go-home-symbolic"));
        assert!(found("star", "non-starred-symbolic"));
        assert!(!found("symbolic", "go-home-symbolic"));
        assert!(!found("gohome", "go-home-symbolic"));
        assert!(!found("home office", "go-home-symbolic"));
    }
}
