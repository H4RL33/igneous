//! Bookmarks: notes, folders, headings and searches pinned in the sidebar,
//! saved in `.igneous/bookmarks.json` and kept pointing at notes that move.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use gtk::subclass::prelude::*;
use igneous_core::VaultPath;
use igneous_core::settings::{self as vault_settings, Bookmark, Bookmarks};

use crate::window::Window;

pub struct BookmarksPane {
    pub widget: gtk::Stack,
    list: gtk::ListBox,
    window: glib::WeakRef<Window>,
    store: RefCell<Bookmarks>,
    /// Set when bookmarks.json couldn't be read: it's then never written.
    error: RefCell<Option<String>>,
}

fn path_of(bookmark: &Bookmark) -> Option<&VaultPath> {
    match bookmark {
        Bookmark::File { path, .. }
        | Bookmark::Folder { path, .. }
        | Bookmark::Heading { path, .. } => Some(path),
        Bookmark::Search { .. } => None,
    }
}

/// What a bookmark shows: a title, a subtitle and an icon.
fn describe(bookmark: &Bookmark) -> (String, String, &'static str) {
    match bookmark {
        Bookmark::File { path, title } => (
            title
                .clone()
                .unwrap_or_else(|| crate::files::display_name(path, false).0),
            path.parent().map(|p| p.to_string()).unwrap_or_default(),
            "text-x-generic-symbolic",
        ),
        Bookmark::Folder { path, title } => (
            title.clone().unwrap_or_else(|| path.file_name().to_owned()),
            path.parent().map(|p| p.to_string()).unwrap_or_default(),
            "folder-symbolic",
        ),
        Bookmark::Heading {
            path,
            heading,
            title,
        } => (
            title.clone().unwrap_or_else(|| heading.clone()),
            crate::files::display_name(path, false).0,
            "view-list-bullet-symbolic",
        ),
        Bookmark::Search { query, title } => (
            title.clone().unwrap_or_else(|| query.clone()),
            "Search".to_owned(),
            "edit-find-symbolic",
        ),
    }
}

/// `path` after `from` moved to `to` (a file, or a folder and its contents).
fn moved(path: &VaultPath, from: &VaultPath, to: &VaultPath) -> Option<VaultPath> {
    if path == from {
        return Some(to.clone());
    }
    let rest = path
        .as_str()
        .strip_prefix(from.as_str())?
        .strip_prefix('/')?;
    to.join(rest).ok()
}

/// Bookmarks after `from` moved to `to`; `None` if none changed.
pub fn follow_rename(
    items: &[Bookmark],
    from: &VaultPath,
    to: &VaultPath,
) -> Option<Vec<Bookmark>> {
    let mut changed = false;
    let items = items
        .iter()
        .map(|b| {
            let mut b = b.clone();
            if let Bookmark::File { path, .. }
            | Bookmark::Folder { path, .. }
            | Bookmark::Heading { path, .. } = &mut b
                && let Some(new) = moved(path, from, to)
            {
                *path = new;
                changed = true;
            }
            b
        })
        .collect();
    changed.then_some(items)
}

impl BookmarksPane {
    pub fn new(window: &Window) -> Rc<Self> {
        let list = gtk::ListBox::builder()
            .css_classes(["navigation-sidebar"])
            .selection_mode(gtk::SelectionMode::None)
            .build();
        list.connect_row_activated(|_, row| row.emit_activate());
        let empty = adw::StatusPage::builder()
            .icon_name("user-bookmarks-symbolic")
            .title("No Bookmarks")
            .description(
                "Bookmark notes, headings and searches from the main menu or the command palette",
            )
            .css_classes(["compact"])
            .build();
        let widget = gtk::Stack::new();
        widget.add_named(
            &gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .vexpand(true)
                .child(&list)
                .build(),
            Some("list"),
        );
        widget.add_named(&empty, Some("empty"));
        let (store, error) =
            match vault_settings::load::<Bookmarks>(&window.ctx().vault.igneous_dir()) {
                Ok(store) => (store, None),
                Err(e) => (Bookmarks::default(), Some(e.to_string())),
            };
        if let Some(error) = &error {
            window.toast(&format!("Couldn’t read the bookmarks: {error}"));
        }
        let pane = Rc::new(Self {
            widget,
            list,
            window: window.downgrade(),
            store: RefCell::new(store),
            error: RefCell::new(error),
        });
        pane.rebuild();
        pane
    }

    pub fn items(&self) -> Vec<Bookmark> {
        self.store.borrow().items.clone()
    }

    /// Replaces the bookmarks and saves them.
    pub fn set_items(self: &Rc<Self>, items: Vec<Bookmark>) {
        let window = self.window.upgrade();
        if let Some(error) = self.error.borrow().as_ref() {
            if let Some(window) = &window {
                window.toast(&format!(
                    "bookmarks.json can’t be read, so it wasn’t changed: {error}"
                ));
            }
            return;
        }
        self.store.borrow_mut().items = items;
        if let Some(window) = &window
            && let Err(e) =
                vault_settings::save(&window.ctx().vault.igneous_dir(), &*self.store.borrow())
        {
            window.toast(&format!("Couldn’t save the bookmarks: {e}"));
        }
        self.rebuild();
    }

    pub(crate) fn rebuild(self: &Rc<Self>) {
        self.list.remove_all();
        let items = self.items();
        self.widget
            .set_visible_child_name(if items.is_empty() { "empty" } else { "list" });
        let count = items.len();
        for (i, bookmark) in items.into_iter().enumerate() {
            let (title, subtitle, icon) = describe(&bookmark);
            // Bookmarked notes show their custom icon.
            let custom = match (&bookmark, self.window.upgrade()) {
                (Bookmark::File { path, .. }, Some(window)) => window.ctx().icons.shown(path),
                _ => None,
            };
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&title))
                .subtitle(glib::markup_escape_text(&subtitle))
                .activatable(true)
                .build();
            let image = gtk::Image::from_icon_name(custom.as_deref().unwrap_or(icon));
            // And their colour, as in the file tree.
            if let (Bookmark::File { path, .. } | Bookmark::Folder { path, .. }, Some(window)) =
                (&bookmark, self.window.upgrade())
                && let Some(color) = window.path_color(path)
            {
                image.add_css_class(&crate::icons::color_class(&color));
            }
            row.add_prefix(&image);
            row.add_suffix(&self.row_menu(i, count));
            let window = self.window.clone();
            row.connect_activated(move |_| {
                if let Some(window) = window.upgrade() {
                    window.open_bookmark(&bookmark);
                }
            });
            self.list.append(&row);
        }
    }

    fn row_menu(self: &Rc<Self>, index: usize, count: usize) -> gtk::MenuButton {
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .build();
        let popover = gtk::Popover::builder().child(&content).build();
        let entries: [(&str, bool, isize); 3] = [
            ("Move Up", index > 0, -1),
            ("Move Down", index + 1 < count, 1),
            ("Remove", true, 0),
        ];
        for (label, sensitive, step) in entries {
            let button = gtk::Button::builder()
                .child(&gtk::Label::builder().label(label).xalign(0.0).build())
                .css_classes(if step == 0 {
                    vec!["flat", "error"]
                } else {
                    vec!["flat"]
                })
                .sensitive(sensitive)
                .build();
            let pane = Rc::downgrade(self);
            let popover_weak = popover.downgrade();
            button.connect_clicked(move |_| {
                if let Some(popover) = popover_weak.upgrade() {
                    popover.popdown();
                }
                let Some(pane) = pane.upgrade() else { return };
                let mut items = pane.items();
                if step == 0 {
                    items.remove(index);
                } else {
                    let other = (index as isize + step) as usize;
                    items.swap(index, other);
                }
                pane.set_items(items);
            });
            content.append(&button);
        }
        gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text("Bookmark Options")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .popover(&popover)
            .build()
    }
}

impl Window {
    fn bookmarks_pane(&self) -> Option<Rc<BookmarksPane>> {
        self.imp().bookmarks.get().cloned()
    }

    /// The vault's bookmarks, in order.
    pub fn bookmarks(&self) -> Vec<Bookmark> {
        self.bookmarks_pane().map(|p| p.items()).unwrap_or_default()
    }

    /// Bookmarks the open note, or removes its bookmark.
    pub fn toggle_bookmark(&self) {
        let (Some(pane), Some(path)) = (self.bookmarks_pane(), self.selected_path()) else {
            return;
        };
        let mut items = pane.items();
        let before = items.len();
        items.retain(|b| !matches!(b, Bookmark::File { path: p, .. } if *p == path));
        let added = items.len() == before;
        if added {
            items.push(Bookmark::File { path, title: None });
        }
        pane.set_items(items);
        self.toast(if added {
            "Bookmarked"
        } else {
            "Bookmark removed"
        });
    }

    /// Bookmarks the heading the cursor is under.
    pub fn bookmark_heading(&self) {
        let (Some(pane), Some(note)) = (self.bookmarks_pane(), self.selected_note()) else {
            return;
        };
        let Some(path) = note.path() else { return };
        let text = note.text();
        let cursor = note.cursor_byte();
        let doc = igneous_markdown::parse(&text);
        let Some(heading) = doc.headings.iter().rev().find(|h| h.range.start <= cursor) else {
            self.toast("Put the cursor under a heading to bookmark it");
            return;
        };
        let mut items = pane.items();
        let bookmark = Bookmark::Heading {
            path,
            heading: heading.text.clone(),
            title: None,
        };
        if !items.contains(&bookmark) {
            items.push(bookmark);
            pane.set_items(items);
        }
        self.toast(&format!("Bookmarked “{}”", heading.text));
    }

    /// Bookmarks the Search pane's query.
    pub fn bookmark_search(&self) {
        let Some(pane) = self.bookmarks_pane() else {
            return;
        };
        let query = self
            .imp()
            .search
            .get()
            .map(|s| s.entry.text().trim().to_owned())
            .unwrap_or_default();
        if query.is_empty() {
            self.toast("Search for something to bookmark it");
            return;
        }
        let mut items = pane.items();
        let bookmark = Bookmark::Search { query, title: None };
        if !items.contains(&bookmark) {
            items.push(bookmark);
            pane.set_items(items);
        }
        self.toast("Bookmarked the search");
    }

    pub fn open_bookmark(&self, bookmark: &Bookmark) {
        match bookmark {
            Bookmark::File { path, .. } => self.open_path(path, false),
            Bookmark::Folder { path, .. } => {
                self.imp().sidebar_stack.set_visible_child_name("files");
                self.toast(&format!("“{path}” is a folder"));
            }
            Bookmark::Heading { path, heading, .. } => {
                self.open_path(path, false);
                if let Some(note) = self.selected_note()
                    && note.path().as_ref() == Some(path)
                {
                    note.scroll_to_subpath(&igneous_markdown::Subpath::Heading(vec![
                        heading.clone(),
                    ]));
                    note.focus_editor();
                }
            }
            Bookmark::Search { query, .. } => self.search_vault(query),
        }
    }

    /// Keeps bookmarks on notes and folders that were renamed or moved.
    pub(crate) fn bookmarks_follow_rename(&self, from: &VaultPath, to: &VaultPath) {
        if let Some(pane) = self.bookmarks_pane()
            && let Some(items) = follow_rename(&pane.items(), from, to)
        {
            pane.set_items(items);
        }
    }

    /// Whether a bookmark points at `path`.
    pub fn is_bookmarked(&self, path: &VaultPath) -> bool {
        self.bookmarks().iter().any(|b| path_of(b) == Some(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> VaultPath {
        VaultPath::new(s).unwrap()
    }

    #[test]
    fn bookmarks_follow_moves() {
        let items = vec![
            Bookmark::File {
                path: p("Projects/Plan.md"),
                title: None,
            },
            Bookmark::Heading {
                path: p("Home.md"),
                heading: "Next".into(),
                title: None,
            },
            Bookmark::Search {
                query: "tag:#x".into(),
                title: None,
            },
        ];
        let moved = follow_rename(&items, &p("Projects"), &p("Work")).unwrap();
        assert_eq!(path_of(&moved[0]), Some(&p("Work/Plan.md")));
        assert_eq!(path_of(&moved[1]), Some(&p("Home.md")));
        assert!(follow_rename(&items, &p("Other.md"), &p("Else.md")).is_none());
        let moved = follow_rename(&items, &p("Home.md"), &p("Start.md")).unwrap();
        assert_eq!(path_of(&moved[1]), Some(&p("Start.md")));
    }
}
