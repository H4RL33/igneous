//! The smaller built-ins: daily notes, templates, the
//! startup note, bookmarks and file recovery. This wires their actions and
//! sidebar pieces into the window; each lives in its own module.

use adw::{prelude::*, subclass::prelude::*};
use gtk::glib;

use crate::bookmarks::BookmarksPane;
use crate::window::Window;

pub fn install_actions(
    klass: &mut <crate::window::imp::Window as glib::subclass::types::ObjectSubclass>::Class,
) {
    klass.install_action("win.daily-note", None, |w, _, _| {
        w.open_daily_note(crate::daily_notes::today())
    });
    klass.install_action("win.daily-note-previous", None, |w, _, _| {
        w.step_daily_note(false)
    });
    klass.install_action("win.daily-note-next", None, |w, _, _| {
        w.step_daily_note(true)
    });
    klass.install_action("win.insert-template", None, |w, _, _| {
        w.show_template_picker()
    });
    klass.install_action("win.bookmark", None, |w, _, _| w.toggle_bookmark());
    klass.install_action("win.bookmark-heading", None, |w, _, _| w.bookmark_heading());
    klass.install_action("win.bookmark-search", None, |w, _, _| w.bookmark_search());
    klass.install_action("win.show-bookmarks", None, |w, _, _| {
        let imp = w.imp();
        imp.split_view.set_show_sidebar(true);
        imp.sidebar_stack.set_visible_child_name("bookmarks");
    });
    klass.install_action("win.note-snapshots", None, |w, _, _| {
        match w.selected_path() {
            Some(path) => w.show_snapshots(&path),
            None => w.toast("Open a note to see its snapshots"),
        }
    });
    klass.install_action("win.restore-snapshot", None, |w, _, _| {
        w.restore_selected_snapshot()
    });
}

impl Window {
    /// Sets up bookmarks and the calendar, then opens the startup note.
    pub(crate) fn set_up_builtins(&self) {
        let imp = self.imp();
        let bookmarks = BookmarksPane::new(self);
        imp.bookmarks_bin.set_child(Some(&bookmarks.widget));
        imp.bookmarks.set(bookmarks).ok();
        self.set_up_calendar();
        self.open_startup_note();
    }

    /// Opens the vault's startup note, if it has one, after the restored tabs.
    fn open_startup_note(&self) {
        let Some(path) = self.ctx().settings.borrow().startup_note.clone() else {
            return;
        };
        if !self.ctx().abs(&path).is_file() {
            self.toast(&format!("The startup note “{path}” doesn’t exist"));
            return;
        }
        let has_tabs = self.imp().tab_view.n_pages() > 0;
        self.open_path(&path, has_tabs);
        if let Some(note) = self.selected_note() {
            note.focus_editor();
        }
    }
}
