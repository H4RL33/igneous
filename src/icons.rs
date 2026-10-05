//! Custom icons for notes (any symbolic icon from the icon theme) and
//! custom colours for notes and folders (from GTK's colour chooser), saved
//! in `.igneous/icons.json`. Icons show in the file tree, on tabs and in
//! bookmarks; colours tint the file tree's and bookmarks' icons. Both
//! follow files that Igneous renames, moves or deletes.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib, subclass::prelude::*};
use igneous_core::VaultPath;
use igneous_core::settings::{self as vault_settings, Icons, Removed, SettingsError};

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

    /// The colour set for `path`, if it's a valid one.
    pub fn color(&self, path: &VaultPath) -> Option<gdk::RGBA> {
        let icons = self.icons.borrow();
        gdk::RGBA::parse(icons.color(path)?).ok()
    }

    /// Every colour in use, each once.
    pub fn colors(&self) -> Vec<gdk::RGBA> {
        let mut colors: Vec<String> = self
            .icons
            .borrow()
            .colors
            .values()
            .filter_map(|c| gdk::RGBA::parse(c).ok())
            .map(|c| hex(&c))
            .collect();
        colors.sort_unstable();
        colors.dedup();
        colors
            .iter()
            .filter_map(|c| gdk::RGBA::parse(c).ok())
            .collect()
    }

    /// Sets the colour for `path`, or removes it with `None`. Returns
    /// whether that changed anything.
    pub fn set_color(&self, path: &VaultPath, color: Option<&gdk::RGBA>) -> Result<bool, String> {
        let color = color.map(hex);
        self.update(|icons| icons.set_color(path, color.as_deref()))
    }

    /// Moves the icons and colours of a renamed file, or of everything in a
    /// renamed folder.
    pub fn follow_rename(&self, from: &VaultPath, to: &VaultPath) -> Result<bool, String> {
        if !self.icons.borrow().has_under(from) {
            return Ok(false);
        }
        self.update(|icons| icons.follow_rename(from, to))
    }

    /// Removes the icons and colours of a deleted file or folder, returning
    /// them.
    pub fn forget(&self, path: &VaultPath) -> Result<Removed, String> {
        if !self.icons.borrow().has_under(path) {
            return Ok(Removed::default());
        }
        self.update(|icons| icons.remove_under(path))
    }

    /// Puts back what [`IconStore::forget`] removed.
    pub fn restore(&self, removed: &Removed) -> Result<bool, String> {
        if removed.is_empty() {
            return Ok(false);
        }
        self.update(|icons| icons.restore(removed))
    }
}

/// A colour as `#rrggbb`, the form icons.json keeps.
pub fn hex(color: &gdk::RGBA) -> String {
    let channel = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(color.red()),
        channel(color.green()),
        channel(color.blue())
    )
}

/// The CSS class that tints an icon in `color`.
pub fn color_class(color: &gdk::RGBA) -> String {
    format!("file-color-{}", hex(color).trim_start_matches('#'))
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

/// The icon theme contexts the picker shows as categories, in order: the
/// context's name in `index.theme`, a label, and an icon for its button.
const CATEGORIES: &[(&str, &str, &str)] = &[
    ("Actions", "Actions", "document-edit-symbolic"),
    ("Applications", "Apps", "application-x-executable-symbolic"),
    ("Categories", "Categories", "applications-system-symbolic"),
    ("Devices", "Devices", "computer-symbolic"),
    ("Emblems", "Emblems", "starred-symbolic"),
    ("Emotes", "Emotes", "face-smile-symbolic"),
    ("MimeTypes", "File Types", "text-x-generic-symbolic"),
    ("Places", "Places", "folder-symbolic"),
    ("Status", "Status", "dialog-information-symbolic"),
    ("UI", "Interface", "view-grid-symbolic"),
];

/// Icons in none of [`CATEGORIES`]: other contexts, or none at all.
const OTHER: (&str, &str, &str) = ("Other", "Other", "view-more-symbolic");

/// Each icon's context (such as "Places" or "Status"), from the
/// `index.theme` of `theme` and of the themes it inherits from (hicolor
/// last), looked for in each of `search`. The first theme with an icon
/// decides its context.
fn icon_contexts(search: &[PathBuf], theme: &str) -> HashMap<String, String> {
    let mut contexts = HashMap::new();
    let mut queue = VecDeque::from([theme.to_owned()]);
    let mut seen = HashSet::new();
    while let Some(name) = queue.pop_front() {
        if !seen.insert(name.clone()) {
            continue;
        }
        for base in search {
            let dir = base.join(&name);
            let index = glib::KeyFile::new();
            if index
                .load_from_file(dir.join("index.theme"), glib::KeyFileFlags::NONE)
                .is_err()
            {
                continue;
            }
            if let Ok(inherits) = index.string("Icon Theme", "Inherits") {
                queue.extend(
                    inherits
                        .split(',')
                        .map(str::trim)
                        .filter(|t| !t.is_empty())
                        .map(str::to_owned),
                );
            }
            for group in index.groups().iter() {
                let Ok(context) = index.string(group, "Context") else {
                    continue;
                };
                for icon in icons_in(&dir.join(group.as_str())) {
                    contexts.entry(icon).or_insert_with(|| context.to_string());
                }
            }
        }
        if queue.is_empty() && !seen.contains("hicolor") {
            queue.push_back("hicolor".to_owned());
        }
    }
    contexts
}

/// The icons in a theme directory: `foo-symbolic.svg` and
/// `foo.symbolic.png` hold `foo-symbolic`.
fn icons_in(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let file = entry.file_name().to_string_lossy().into_owned();
            let stem = file
                .strip_suffix(".svg")
                .or_else(|| file.strip_suffix(".png"))?;
            Some(match stem.strip_suffix(".symbolic") {
                Some(base) => format!("{base}-symbolic"),
                None => stem.to_owned(),
            })
        })
        .collect()
}

/// The picker's category for an icon in `context`.
fn category_of(context: Option<&String>) -> &'static str {
    context
        .and_then(|c| CATEGORIES.iter().find(|(name, _, _)| name == c))
        .map_or(OTHER.0, |(name, _, _)| name)
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
        "win.file-set-color",
        Some(glib::VariantTy::STRING),
        |w, _, p| {
            if let Some(path) = p
                .and_then(|p| p.get::<String>())
                .and_then(|p| VaultPath::new(&p).ok())
            {
                w.color_dialog(path);
            }
        },
    );
    klass.install_action(
        "win.file-reset-color",
        Some(glib::VariantTy::STRING),
        |w, _, p| {
            if let Some(path) = p
                .and_then(|p| p.get::<String>())
                .and_then(|p| VaultPath::new(&p).ok())
            {
                w.set_path_color(&path, None);
            }
        },
    );
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

    /// The colour set for a file or folder, if any.
    pub fn path_color(&self, path: &VaultPath) -> Option<gdk::RGBA> {
        self.ctx().icons.color(path)
    }

    /// Gives a file or folder's icon a colour, or its usual one with `None`.
    pub fn set_path_color(&self, path: &VaultPath, color: Option<&gdk::RGBA>) {
        match self.ctx().icons.set_color(path, color) {
            Ok(true) => self.show_icons(),
            Ok(false) => {}
            Err(e) => self.toast(&e),
        }
    }

    /// Picks a colour for a file or folder with GTK's colour chooser.
    pub fn color_dialog(&self, path: VaultPath) {
        let folder = self.ctx().abs(&path).is_dir();
        let dialog = gtk::ColorDialog::builder()
            .title(if folder { "Folder Color" } else { "Note Color" })
            .modal(true)
            .with_alpha(false)
            .build();
        let current = self.path_color(&path);
        let window = self.downgrade();
        glib::spawn_future_local(async move {
            let Some(parent) = window.upgrade() else {
                return;
            };
            // Cancelling is an error too; it changes nothing.
            if let Ok(color) = dialog
                .choose_rgba_future(Some(&parent), current.as_ref())
                .await
            {
                parent.set_path_color(&path, Some(&color));
            }
        });
    }

    /// Moves icons along with a renamed file or folder.
    pub(crate) fn icons_follow_rename(&self, from: &VaultPath, to: &VaultPath) {
        match self.ctx().icons.follow_rename(from, to) {
            Ok(true) => self.show_icons(),
            Ok(false) => {}
            Err(e) => self.toast(&e),
        }
    }

    /// Drops the icons and colours of a deleted file or folder, returning
    /// them so an undo can put them back.
    pub(crate) fn forget_icons(&self, path: &VaultPath) -> Removed {
        match self.ctx().icons.forget(path) {
            Ok(gone) => {
                if !gone.is_empty() {
                    self.show_icons();
                }
                gone
            }
            Err(e) => {
                self.toast(&e);
                Removed::default()
            }
        }
    }

    pub(crate) fn restore_icons(&self, removed: &Removed) {
        match self.ctx().icons.restore(removed) {
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

    /// The colour class on the file tree row for `path` (empty for none),
    /// if the row is showing.
    pub fn sidebar_color(&self, path: &VaultPath) -> Option<String> {
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
            .map(|item| item.color())
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
        let display = WidgetExt::display(self);
        let theme = gtk::IconTheme::for_display(&display);
        let names = symbolic_icons(&display);
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let icons = gtk::StringList::new(&names);

        // Categories: the theme's contexts.
        let contexts = icon_contexts(&theme.search_path(), &theme.theme_name());
        let categories: Rc<HashMap<String, &'static str>> = Rc::new(
            names
                .iter()
                .map(|n| (n.to_string(), category_of(contexts.get(*n))))
                .collect(),
        );
        let category: Rc<RefCell<String>> = Rc::new(RefCell::new("all".to_owned()));
        let words: Rc<RefCell<Vec<String>>> = Rc::default();
        let filter = gtk::CustomFilter::new({
            let words = words.clone();
            let category = category.clone();
            let categories = categories.clone();
            move |obj| {
                obj.downcast_ref::<gtk::StringObject>().is_some_and(|name| {
                    let name = name.string();
                    let wanted = category.borrow();
                    (*wanted == "all"
                        || categories.get(name.as_str()).copied() == Some(wanted.as_str()))
                        && matches(&words.borrow(), &name)
                })
            }
        });
        let group = adw::ToggleGroup::builder()
            .homogeneous(true)
            .margin_start(6)
            .margin_end(6)
            .margin_bottom(6)
            .build();
        group
            .upcast_ref::<gtk::Widget>()
            .update_property(&[gtk::accessible::Property::Label("Categories")]);
        let toggle = |name: &str, label: &str, icon: &str| {
            let toggle = adw::Toggle::builder().name(name).tooltip(label).build();
            if theme.has_icon(icon) {
                toggle.set_icon_name(Some(icon));
            } else {
                toggle.set_label(Some(label));
            }
            toggle
        };
        group.add(toggle("all", "All", "view-app-grid-symbolic"));
        for (name, label, icon) in CATEGORIES.iter().chain([&OTHER]) {
            if categories.values().any(|c| c == name) {
                group.add(toggle(name, label, icon));
            }
        }
        group.set_active_name(Some("all"));
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
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&grid)
            .build();
        stack.add_named(&scrolled, Some("icons"));
        stack.add_named(&empty, Some("empty"));

        // The icons given most recently, in one row above the rest.
        let recent: Vec<String> = crate::gsettings::recent_icons(&crate::gsettings::settings())
            .into_iter()
            .filter(|i| theme.has_icon(i))
            .collect();
        let recent_row = gtk::Box::builder().margin_start(6).margin_end(6).build();
        let recent_section = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .margin_top(6)
            .visible(!recent.is_empty())
            .build();
        recent_section.append(
            &gtk::Label::builder()
                .label("Recent")
                .xalign(0.0)
                .margin_start(12)
                .css_classes(["caption-heading", "dim-label"])
                .build(),
        );
        recent_section.append(&recent_row);
        recent_section.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        content.append(&recent_section);
        content.append(&stack);

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
        toolbar.add_top_bar(&group);
        toolbar.add_bottom_bar(&bottom);
        toolbar.set_content(Some(&content));
        let (name, _) = crate::files::display_name(&path, false);
        let dialog = adw::Dialog::builder()
            .title(format!("Icon for “{name}”"))
            .content_width(560)
            .content_height(540)
            .child(&toolbar)
            .build();

        let choose: Rc<dyn Fn(Option<String>)> = {
            let window = self.downgrade();
            let dialog = dialog.downgrade();
            Rc::new(move |icon| {
                if let Some(icon) = &icon {
                    crate::gsettings::add_recent_icon(&crate::gsettings::settings(), icon);
                }
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
        for icon in recent {
            let button = gtk::Button::builder()
                .child(
                    &gtk::Image::builder()
                        .icon_name(&icon)
                        .pixel_size(32)
                        .build(),
                )
                .tooltip_text(&icon)
                .css_classes(["flat"])
                .build();
            button.update_property(&[gtk::accessible::Property::Label(&icon)]);
            let choose = choose.clone();
            button.connect_clicked(move |_| choose(Some(icon.clone())));
            recent_row.append(&button);
        }
        // Recent shows over all icons, until a search or a category narrows
        // them.
        let refilter = {
            let filter = filter.clone();
            let filtered = filtered.clone();
            let stack = stack.clone();
            let recent_section = recent_section.clone();
            let has_recent = recent_row.first_child().is_some();
            let words = words.clone();
            let category = category.clone();
            Rc::new(move || {
                filter.changed(gtk::FilterChange::Different);
                let found = filtered.n_items() > 0;
                stack.set_visible_child_name(if found { "icons" } else { "empty" });
                recent_section.set_visible(
                    has_recent && words.borrow().is_empty() && *category.borrow() == "all",
                );
            })
        };
        {
            let refilter = refilter.clone();
            group.connect_active_name_notify(move |group| {
                category.replace(group.active_name().map_or("all".into(), |n| n.to_string()));
                refilter();
            });
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
            refilter();
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

        // Start at the icon the note has now. Scrolling a grid before it has
        // a size, or while it's being laid out, leaves it blank: scroll once
        // it has one, between frames.
        if let Some(current) = &current
            && let Some(position) = names.iter().position(|n| *n == current.as_str())
        {
            let position = position as u32;
            selection.set_selected(position);
            let adjustment = scrolled.vadjustment();
            let handler: Rc<RefCell<Option<glib::SignalHandlerId>>> = Rc::default();
            let grid = grid.downgrade();
            let once = handler.clone();
            let id = adjustment.connect_changed(move |adjustment| {
                if adjustment.page_size() <= 0.0 {
                    return;
                }
                if let Some(id) = once.take() {
                    adjustment.disconnect(id);
                }
                let grid = grid.clone();
                glib::idle_add_local_once(move || {
                    if let Some(grid) = grid.upgrade() {
                        grid.scroll_to(position, gtk::ListScrollFlags::NONE, None);
                    }
                });
            });
            handler.replace(Some(id));
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

    /// Writes an icon theme: its index.theme and an icon file per entry.
    fn theme(root: &Path, name: &str, inherits: &str, icons: &[(&str, &str, &str)]) {
        let mut index = format!("[Icon Theme]\nName={name}\nInherits={inherits}\n");
        for (dir, context, file) in icons {
            index.push_str(&format!("\n[{dir}]\nContext={context}\nType=Scalable\n"));
            std::fs::create_dir_all(root.join(name).join(dir)).unwrap();
            std::fs::write(root.join(name).join(dir).join(file), "").unwrap();
        }
        std::fs::write(root.join(name).join("index.theme"), index).unwrap();
    }

    #[test]
    fn categories_come_from_the_theme_and_its_parents() {
        let dir = tempfile::tempdir().unwrap();
        theme(
            dir.path(),
            "Mine",
            "Base",
            &[("symbolic/places", "Places", "folder-symbolic.svg")],
        );
        theme(
            dir.path(),
            "Base",
            "",
            &[
                // The child theme's context wins.
                ("symbolic/status", "Status", "folder-symbolic.svg"),
                ("symbolic/status", "Status", "starred-symbolic.svg"),
                ("16x16/devices", "Devices", "phone.symbolic.png"),
                ("symbolic/stock", "Stock", "odd-symbolic.svg"),
            ],
        );
        theme(
            dir.path(),
            "hicolor",
            "",
            &[("symbolic/apps", "Applications", "app-symbolic.svg")],
        );
        let contexts = icon_contexts(&[dir.path().to_owned()], "Mine");
        let category = |icon: &str| category_of(contexts.get(icon));
        assert_eq!(category("folder-symbolic"), "Places");
        assert_eq!(category("starred-symbolic"), "Status");
        assert_eq!(category("phone-symbolic"), "Devices");
        assert_eq!(category("app-symbolic"), "Applications");
        // A context the picker doesn't list, and no context at all.
        assert_eq!(category("odd-symbolic"), "Other");
        assert_eq!(category("unknown-symbolic"), "Other");
    }
}
