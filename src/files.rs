//! The file tree in the sidebar.
//!
//! Folders load their children when first expanded. Each loaded folder's list
//! is kept in step with the disk by diffing, so expanded folders stay expanded
//! when files change.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use gtk::{gdk, gio, glib, pango, prelude::*, subclass::prelude::*};
use igneous_core::VaultPath;
use igneous_core::vault::EntryKind;
use igneous_core::watch::VaultEvent;

use crate::vault::VaultContext;

mod imp {
    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::FileItem)]
    pub struct FileItem {
        pub path: RefCell<String>,
        pub folder: Cell<bool>,
        /// The row's icon: a custom one, or the usual one for its kind.
        #[property(get, set)]
        pub icon_name: RefCell<String>,
        /// The CSS class colouring the icon (see `icons::color_class`), or
        /// empty.
        #[property(get, set)]
        pub color: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FileItem {
        const NAME: &'static str = "IgneousFileItem";
        type Type = super::FileItem;
    }

    #[glib::derived_properties]
    impl ObjectImpl for FileItem {}
}

glib::wrapper! {
    pub struct FileItem(ObjectSubclass<imp::FileItem>);
}

impl FileItem {
    pub fn new(path: &VaultPath, folder: bool, icon: &str, color: &str) -> Self {
        let item: Self = glib::Object::builder()
            .property("icon-name", icon)
            .property("color", color)
            .build();
        item.imp().path.replace(path.to_string());
        item.imp().folder.set(folder);
        item
    }

    pub fn path(&self) -> VaultPath {
        VaultPath::new(&self.imp().path.borrow()).expect("file items hold valid paths")
    }

    pub fn is_folder(&self) -> bool {
        self.imp().folder.get()
    }
}

/// How a file is shown: notes without their `.md`, everything else with its
/// extension as a badge.
pub fn display_name(path: &VaultPath, folder: bool) -> (String, Option<String>) {
    match path.extension() {
        _ if folder => (path.file_name().to_owned(), None),
        Some("md") => (path.stem().to_owned(), None),
        Some(ext) => (path.stem().to_owned(), Some(ext.to_lowercase())),
        None => (path.file_name().to_owned(), None),
    }
}

type OpenFn = dyn Fn(VaultPath, bool);
type MoveFn = dyn Fn(VaultPath, Option<VaultPath>);

pub struct FileTree {
    ctx: Rc<VaultContext>,
    pub model: gtk::TreeListModel,
    pub selection: gtk::SingleSelection,
    /// Loaded folder lists by folder path ("" for the root).
    stores: RefCell<HashMap<String, glib::WeakRef<gio::ListStore>>>,
    on_open: RefCell<Option<Rc<OpenFn>>>,
    on_move: RefCell<Option<Rc<MoveFn>>>,
    /// A class per custom colour in use, for the tree's and bookmarks'
    /// icons.
    colors: gtk::CssProvider,
}

impl Drop for FileTree {
    fn drop(&mut self) {
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_remove_provider_for_display(&display, &self.colors);
        }
    }
}

impl FileTree {
    pub fn new(ctx: Rc<VaultContext>) -> Rc<Self> {
        let root = gio::ListStore::new::<FileItem>();
        let tree = Rc::new_cyclic(|weak: &Weak<FileTree>| {
            let weak = weak.clone();
            let model = gtk::TreeListModel::new(root.clone(), false, false, move |obj| {
                let item = obj.downcast_ref::<FileItem>()?;
                if !item.is_folder() {
                    return None;
                }
                let tree = weak.upgrade()?;
                Some(tree.load(Some(&item.path())).upcast())
            });
            let selection = gtk::SingleSelection::builder()
                .model(&model)
                .autoselect(false)
                .can_unselect(true)
                .build();
            let colors = gtk::CssProvider::new();
            if let Some(display) = gdk::Display::default() {
                gtk::style_context_add_provider_for_display(
                    &display,
                    &colors,
                    gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                );
            }
            FileTree {
                ctx,
                model,
                selection,
                stores: RefCell::default(),
                on_open: RefCell::default(),
                on_move: RefCell::default(),
                colors,
            }
        });
        tree.write_colors();
        tree.fill(&root, None);
        tree.stores
            .borrow_mut()
            .insert(String::new(), root.downgrade());
        tree
    }

    pub fn connect_open(&self, f: impl Fn(VaultPath, bool) + 'static) {
        self.on_open.replace(Some(Rc::new(f)));
    }

    pub fn connect_move(&self, f: impl Fn(VaultPath, Option<VaultPath>) + 'static) {
        self.on_move.replace(Some(Rc::new(f)));
    }

    fn open(&self, path: VaultPath, new_tab: bool) {
        let f = self.on_open.borrow().clone();
        if let Some(f) = f {
            f(path, new_tab);
        }
    }

    fn request_move(&self, from: VaultPath, into: Option<VaultPath>) {
        let f = self.on_move.borrow().clone();
        if let Some(f) = f {
            f(from, into);
        }
    }

    fn entries(&self, folder: Option<&VaultPath>) -> Vec<(VaultPath, bool)> {
        match self.ctx.vault.list(folder) {
            Ok(entries) => entries
                .into_iter()
                .map(|e| (e.path, e.kind == EntryKind::Folder))
                .collect(),
            Err(e) => {
                tracing::warn!(%e, ?folder, "can't list folder");
                Vec::new()
            }
        }
    }

    fn item(&self, path: &VaultPath, folder: bool) -> FileItem {
        FileItem::new(
            path,
            folder,
            &self.icon_of(path, folder),
            &self.color_of(path),
        )
    }

    /// The class colouring the icon of `path`, or an empty string.
    fn color_of(&self, path: &VaultPath) -> String {
        self.ctx
            .icons
            .color(path)
            .map(|c| crate::icons::color_class(&c))
            .unwrap_or_default()
    }

    /// One CSS class for each custom colour in use.
    fn write_colors(&self) {
        let css: String = self
            .ctx
            .icons
            .colors()
            .iter()
            .map(|c| {
                format!(
                    ".{} {{ color: {}; }}\n",
                    crate::icons::color_class(c),
                    crate::icons::hex(c)
                )
            })
            .collect();
        self.colors.load_from_string(&css);
    }

    /// A note's custom icon, or the usual one.
    fn icon_of(&self, path: &VaultPath, folder: bool) -> String {
        let custom = (!folder).then(|| self.ctx.icons.shown(path)).flatten();
        custom.unwrap_or_else(|| icon_for(folder).to_owned())
    }

    /// Shows changed custom icons and colours on every loaded row.
    pub fn refresh_icons(&self) {
        self.write_colors();
        let stores: Vec<gio::ListStore> = self
            .stores
            .borrow()
            .values()
            .filter_map(|w| w.upgrade())
            .collect();
        for item in stores.iter().flat_map(|s| s.iter::<FileItem>().flatten()) {
            let icon = self.icon_of(&item.path(), item.is_folder());
            if item.icon_name() != icon {
                item.set_icon_name(icon);
            }
            let color = self.color_of(&item.path());
            if item.color() != color {
                item.set_color(color);
            }
        }
    }

    fn fill(&self, store: &gio::ListStore, folder: Option<&VaultPath>) {
        let items: Vec<FileItem> = self
            .entries(folder)
            .iter()
            .map(|(path, is_folder)| self.item(path, *is_folder))
            .collect();
        store.splice(0, store.n_items(), &items);
    }

    fn load(&self, folder: Option<&VaultPath>) -> gio::ListStore {
        let store = gio::ListStore::new::<FileItem>();
        self.fill(&store, folder);
        let key = folder.map(|f| f.to_string()).unwrap_or_default();
        self.stores.borrow_mut().insert(key, store.downgrade());
        store
    }

    /// Re-reads a loaded folder from disk, changing only what differs.
    pub fn refresh(&self, folder: Option<&VaultPath>) {
        let key = folder.map(|f| f.to_string()).unwrap_or_default();
        let store = self.stores.borrow().get(&key).and_then(|w| w.upgrade());
        let Some(store) = store else {
            self.stores.borrow_mut().remove(&key);
            return;
        };
        let wanted = self.entries(folder);
        let mut i = store.n_items();
        while i > 0 {
            i -= 1;
            let item = store.item(i).and_downcast::<FileItem>().unwrap();
            let keep = wanted
                .iter()
                .any(|(p, f)| *p == item.path() && *f == item.is_folder());
            if !keep {
                store.remove(i);
            }
        }
        // What's left is in the same order as `wanted`; insert the gaps.
        for (j, (path, is_folder)) in wanted.iter().enumerate() {
            let present = store
                .item(j as u32)
                .and_downcast::<FileItem>()
                .is_some_and(|item| item.path() == *path);
            if !present {
                store.insert(j as u32, &self.item(path, *is_folder));
            }
        }
    }

    pub fn refresh_for_events(&self, events: &[VaultEvent]) {
        let mut folders: Vec<Option<VaultPath>> = Vec::new();
        for event in events {
            let paths: Vec<&VaultPath> = match event {
                VaultEvent::Created(p) | VaultEvent::Removed(p) => vec![p],
                VaultEvent::Renamed { from, to } => vec![from, to],
                VaultEvent::Modified(_) => vec![],
            };
            for path in paths {
                let parent = path.parent();
                if !folders.contains(&parent) {
                    folders.push(parent);
                }
            }
        }
        for folder in folders {
            self.refresh(folder.as_ref());
        }
    }

    fn rows(&self) -> impl Iterator<Item = (u32, gtk::TreeListRow, FileItem)> + '_ {
        (0..self.model.n_items()).filter_map(|i| {
            let row = self.model.item(i).and_downcast::<gtk::TreeListRow>()?;
            let item = row.item().and_downcast::<FileItem>()?;
            Some((i, row, item))
        })
    }

    pub fn expanded_folders(&self) -> Vec<VaultPath> {
        self.rows()
            .filter(|(_, row, item)| item.is_folder() && row.is_expanded())
            .map(|(_, _, item)| item.path())
            .collect()
    }

    /// Expands the given folders, parents first.
    pub fn expand(&self, folders: &[VaultPath]) {
        let mut folders = folders.to_vec();
        folders.sort_by_key(|f| f.components().count());
        for folder in folders {
            if let Some((_, row, _)) = self.rows().find(|(_, _, item)| item.path() == folder) {
                row.set_expanded(true);
            }
        }
    }

    /// Highlights `path` if its row is visible.
    pub fn select(&self, path: Option<&VaultPath>) {
        let position = path.and_then(|path| {
            self.rows()
                .find(|(_, _, item)| item.path() == *path)
                .map(|(i, _, _)| i)
        });
        self.selection
            .set_selected(position.unwrap_or(gtk::INVALID_LIST_POSITION));
    }

    /// Opens or expands the row at `position` (the list view's activation).
    pub fn activate(&self, position: u32) {
        let Some(row) = self.model.item(position).and_downcast::<gtk::TreeListRow>() else {
            return;
        };
        let Some(item) = row.item().and_downcast::<FileItem>() else {
            return;
        };
        if item.is_folder() {
            row.set_expanded(!row.is_expanded());
        } else {
            self.open(item.path(), false);
        }
    }

    pub fn factory(self: &Rc<Self>) -> gtk::SignalListItemFactory {
        let factory = gtk::SignalListItemFactory::new();
        let weak = Rc::downgrade(self);
        factory.connect_setup(move |_, obj| {
            let list_item = obj.downcast_ref::<gtk::ListItem>().unwrap();
            let icon = gtk::Image::new();
            // Follows the item, so a changed custom icon shows straight away.
            list_item
                .property_expression_weak("item")
                .chain_property::<gtk::TreeListRow>("item")
                .chain_property::<FileItem>("icon-name")
                .bind(&icon, "icon-name", gtk::Widget::NONE);
            list_item
                .property_expression_weak("item")
                .chain_property::<gtk::TreeListRow>("item")
                .chain_property::<FileItem>("color")
                .chain_closure::<Vec<String>>(glib::closure!(
                    |_: Option<glib::Object>, color: String| {
                        if color.is_empty() {
                            vec![]
                        } else {
                            vec![color]
                        }
                    }
                ))
                .bind(&icon, "css-classes", gtk::Widget::NONE);
            let label = gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(pango::EllipsizeMode::End)
                .hexpand(true)
                .build();
            let extension = gtk::Label::builder()
                .css_classes(["file-badge"])
                .valign(gtk::Align::Center)
                .visible(false)
                .build();
            let row = gtk::Box::builder()
                .spacing(6)
                .css_classes(["file-row"])
                .build();
            row.append(&icon);
            row.append(&label);
            row.append(&extension);
            let expander = gtk::TreeExpander::new();
            expander.set_child(Some(&row));
            list_item.set_child(Some(&expander));
            attach_controllers(&weak, list_item, &expander);
        });
        factory.connect_bind(|_, obj| {
            let list_item = obj.downcast_ref::<gtk::ListItem>().unwrap();
            let Some(row) = list_item.item().and_downcast::<gtk::TreeListRow>() else {
                return;
            };
            let expander = list_item
                .child()
                .and_downcast::<gtk::TreeExpander>()
                .unwrap();
            expander.set_list_row(Some(&row));
            let item = row.item().and_downcast::<FileItem>().unwrap();
            let path = item.path();
            let content = expander.child().and_downcast::<gtk::Box>().unwrap();
            let icon = content.first_child().unwrap();
            let label = icon.next_sibling().and_downcast::<gtk::Label>().unwrap();
            let extension = label.next_sibling().and_downcast::<gtk::Label>().unwrap();
            let (name, ext) = display_name(&path, item.is_folder());
            label.set_label(&name);
            extension.set_visible(ext.is_some());
            extension.set_label(&ext.as_deref().unwrap_or("").to_uppercase());
            content.set_tooltip_text(Some(path.as_str()));
        });
        factory.connect_unbind(|_, obj| {
            let list_item = obj.downcast_ref::<gtk::ListItem>().unwrap();
            if let Some(expander) = list_item.child().and_downcast::<gtk::TreeExpander>() {
                expander.set_list_row(None);
            }
        });
        factory
    }
}

/// Folders get a folder icon; every file gets the same file icon, with its
/// type shown as a pill when it isn't a note, unless it has a custom icon.
fn icon_for(folder: bool) -> &'static str {
    if folder {
        "folder-symbolic"
    } else {
        "text-x-generic-symbolic"
    }
}

fn item_of(list_item: &glib::WeakRef<gtk::ListItem>) -> Option<FileItem> {
    list_item
        .upgrade()?
        .item()
        .and_downcast::<gtk::TreeListRow>()?
        .item()
        .and_downcast::<FileItem>()
}

/// Context menu, middle-click, drag source and drop target for one row.
fn attach_controllers(
    tree: &Weak<FileTree>,
    list_item: &gtk::ListItem,
    widget: &gtk::TreeExpander,
) {
    let list_item = list_item.downgrade();

    let click = gtk::GestureClick::builder().button(0).build();
    let (weak, li) = (tree.clone(), list_item.clone());
    click.connect_pressed(move |gesture, _, x, y| {
        let (Some(tree), Some(item)) = (weak.upgrade(), item_of(&li)) else {
            return;
        };
        let ctrl = gesture
            .current_event_state()
            .contains(gdk::ModifierType::CONTROL_MASK);
        match gesture.current_button() {
            gdk::BUTTON_SECONDARY => {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                let widget = gesture.widget().unwrap();
                show_context_menu(&widget, Some(&item), x, y);
            }
            gdk::BUTTON_MIDDLE if !item.is_folder() => {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                tree.open(item.path(), true);
            }
            gdk::BUTTON_PRIMARY if ctrl && !item.is_folder() => {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                tree.open(item.path(), true);
            }
            _ => {}
        }
    });
    widget.add_controller(click);

    let drag = gtk::DragSource::new();
    drag.set_actions(gdk::DragAction::MOVE);
    let li = list_item.clone();
    drag.connect_prepare(move |_, _, _| {
        let item = item_of(&li)?;
        Some(gdk::ContentProvider::for_value(
            &item.path().to_string().to_value(),
        ))
    });
    widget.add_controller(drag);

    let drop = gtk::DropTarget::new(String::static_type(), gdk::DragAction::MOVE);
    let (weak, li) = (tree.clone(), list_item);
    drop.connect_drop(move |_, value, _, _| {
        let (Some(tree), Some(item), Ok(from)) =
            (weak.upgrade(), item_of(&li), value.get::<String>())
        else {
            return false;
        };
        let Ok(from) = VaultPath::new(&from) else {
            return false;
        };
        let into = if item.is_folder() {
            Some(item.path())
        } else {
            item.path().parent()
        };
        tree.request_move(from, into);
        true
    });
    widget.add_controller(drop);
}

/// Shows the file menu for `item`, or for the vault root when `None`.
pub fn show_context_menu(widget: &gtk::Widget, item: Option<&FileItem>, x: f64, y: f64) {
    let path = item.map(FileItem::path);
    let folder = item.is_none_or(FileItem::is_folder);
    // New notes and folders go inside a folder, or next to a file.
    let base = match (&path, folder) {
        (Some(p), true) => p.to_string(),
        (Some(p), false) => p.parent().map(|p| p.to_string()).unwrap_or_default(),
        (None, _) => String::new(),
    };
    let entry = |label: &str, action: &str, target: &str| {
        let item = gio::MenuItem::new(Some(label), None);
        item.set_action_and_target_value(Some(action), Some(&target.to_variant()));
        item
    };
    let menu = gio::Menu::new();
    if let (Some(p), false) = (&path, folder) {
        let open = gio::Menu::new();
        open.append_item(&entry(
            "Open in New _Tab",
            "win.file-open-new-tab",
            p.as_str(),
        ));
        menu.append_section(None, &open);
    }
    let create = gio::Menu::new();
    create.append_item(&entry("_New Note", "win.file-new-note", &base));
    create.append_item(&entry("New _Folder", "win.file-new-folder", &base));
    menu.append_section(None, &create);
    if let Some(p) = &path {
        let edit = gio::Menu::new();
        edit.append_item(&entry("_Rename…", "win.file-rename", p.as_str()));
        if !folder {
            edit.append_item(&entry("Set _Icon…", "win.file-set-icon", p.as_str()));
        }
        edit.append_item(&entry("Set _Color…", "win.file-set-color", p.as_str()));
        if item.is_some_and(|i| !i.color().is_empty()) {
            edit.append_item(&entry("Remove C_olor", "win.file-reset-color", p.as_str()));
        }
        edit.append_item(&entry("Move to _Trash", "win.file-trash", p.as_str()));
        menu.append_section(None, &edit);
        let show = gio::Menu::new();
        if !folder {
            let history = entry("_History", "win.file-history", p.as_str());
            history.set_attribute_value("hidden-when", Some(&"action-disabled".to_variant()));
            show.append_item(&history);
        }
        show.append_item(&entry("_Show in Files", "win.file-show", p.as_str()));
        menu.append_section(None, &show);
    }
    // Linting a folder, or the whole vault from the empty area.
    if folder || path.is_none() {
        let lint = gio::Menu::new();
        let label = if path.is_some() {
            "_Lint Folder…"
        } else {
            "_Lint Vault…"
        };
        lint.append_item(&entry(label, "win.lint-folder", &base));
        menu.append_section(None, &lint);
    }

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.set_parent(widget);
    popover.set_has_arrow(false);
    popover.set_halign(gtk::Align::Start);
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    popover.connect_closed(|popover| {
        let popover = popover.clone();
        glib::idle_add_local_once(move || popover.unparent());
    });
    popover.popup();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names() {
        let p = |s| VaultPath::new(s).unwrap();
        assert_eq!(display_name(&p("a/Note.md"), false), ("Note".into(), None));
        assert_eq!(
            display_name(&p("img/Photo.PNG"), false),
            ("Photo".into(), Some("png".into()))
        );
        assert_eq!(
            display_name(&p("Folder.d"), true),
            ("Folder.d".into(), None)
        );
        assert_eq!(display_name(&p("LICENSE"), false), ("LICENSE".into(), None));
    }
}
