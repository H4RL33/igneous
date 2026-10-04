//! A tab showing one note.
//!
//! Notes save themselves a moment after the last change, and when the tab or
//! window closes or loses focus. A save never overwrites changes made on disk
//! since the note was loaded: instead a banner offers to reload or keep this
//! version.
//!
//! A note left with conflict blocks by a Git merge shows a bar with Keep Mine,
//! Keep Theirs and Keep Both for the conflict at the cursor.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::{prelude::*, subclass::prelude::*};
use gtk::glib;
use igneous_core::fs::{self, Expect, FileStamp, ReadError, WriteError};
use igneous_core::{TextFile, VaultPath};
use igneous_editor::{Mode, NoteView};
use igneous_git::conflicts::{self, Keep};

use crate::vault::VaultContext;

/// How many notes Back remembers per tab.
const MAX_HISTORY: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum State {
    #[default]
    Clean,
    /// Unsaved edits; an autosave is scheduled.
    Dirty,
    /// The file changed on disk while there were unsaved edits.
    ChangedOnDisk,
    /// The file disappeared from disk.
    Deleted,
    /// The last save failed.
    SaveFailed,
    /// Not valid UTF-8: shown but never saved.
    ReadOnly,
}

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/dev/h4rl3y/igneous/note-page.ui")]
    pub struct NotePage {
        #[template_child]
        pub banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub conflict_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub conflict_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub scrolled: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub view: TemplateChild<NoteView>,
        pub buffer: OnceCell<sourceview::Buffer>,
        pub ctx: OnceCell<Rc<VaultContext>>,
        pub path: RefCell<Option<VaultPath>>,
        pub file: RefCell<TextFile>,
        pub stamp: RefCell<Option<FileStamp>>,
        pub state: Cell<State>,
        pub loading: Cell<bool>,
        pub autosave: RefCell<Option<glib::SourceId>>,
        pub save_error: RefCell<Option<String>>,
        /// A rebase is in progress, so the upper side of a conflict is the
        /// remote's version rather than ours.
        pub rebasing: Cell<bool>,
        /// Notes visited in this tab before and after the current one.
        pub back: RefCell<Vec<VaultPath>>,
        pub forward: RefCell<Vec<VaultPath>>,
        /// The editing mode to return to when leaving Reading mode.
        pub last_edit_mode: Cell<Mode>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for NotePage {
        const NAME: &'static str = "IgneousNotePage";
        type Type = super::NotePage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            NoteView::ensure_type();
            klass.bind_template();
            klass.install_action("note.keep-mine", None, |page, _, _| {
                page.resolve_conflict_block(Side::Mine)
            });
            klass.install_action("note.keep-theirs", None, |page, _, _| {
                page.resolve_conflict_block(Side::Theirs)
            });
            klass.install_action("note.keep-both", None, |page, _, _| {
                page.resolve_conflict_block(Side::Both)
            });
            klass.install_action("note.previous-conflict", None, |page, _, _| {
                page.jump_to_conflict(false)
            });
            klass.install_action("note.next-conflict", None, |page, _, _| {
                page.jump_to_conflict(true)
            });
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for NotePage {
        fn constructed(&self) {
            self.parent_constructed();
            let buffer = self.view.source_buffer();
            let page = self.obj().downgrade();
            buffer.connect_changed(move |_| {
                if let Some(page) = page.upgrade() {
                    page.on_changed();
                }
            });
            let page = self.obj().downgrade();
            buffer.connect_cursor_position_notify(move |_| {
                if let Some(page) = page.upgrade()
                    && page.has_conflicts()
                {
                    page.update_conflicts();
                }
            });
            let page = self.obj().downgrade();
            self.banner.connect_button_clicked(move |_| {
                if let Some(page) = page.upgrade() {
                    page.on_banner_button();
                }
            });
            self.buffer.set(buffer).unwrap();
        }

        fn dispose(&self) {
            if let Some(id) = self.autosave.take() {
                id.remove();
            }
        }
    }

    impl WidgetImpl for NotePage {}
    impl BinImpl for NotePage {}
}

glib::wrapper! {
    pub struct NotePage(ObjectSubclass<imp::NotePage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl NotePage {
    pub fn new(ctx: &Rc<VaultContext>) -> Self {
        let page: Self = glib::Object::new();
        page.imp().ctx.set(ctx.clone()).ok().unwrap();
        page.imp()
            .view
            .set_host(Rc::new(crate::editor_host::NoteHost {
                ctx: ctx.clone(),
                page: page.downgrade(),
            }));
        page
    }

    pub fn view(&self) -> NoteView {
        self.imp().view.get()
    }

    pub fn mode(&self) -> Mode {
        self.imp().view.mode()
    }

    pub fn set_mode(&self, mode: Mode) {
        if mode != Mode::Reading {
            self.imp().last_edit_mode.set(mode);
        }
        self.imp().view.set_mode(mode);
    }

    /// Ctrl+E: between Reading and the last editing mode.
    pub fn toggle_reading(&self) {
        let mode = if self.mode() == Mode::Reading {
            self.imp().last_edit_mode.get()
        } else {
            Mode::Reading
        };
        self.set_mode(mode);
    }

    /// Moves the cursor to a heading or block, e.g. after following a link.
    pub fn scroll_to_subpath(&self, subpath: &igneous_markdown::Subpath) {
        let text = self.text();
        let doc = igneous_markdown::parse(&text);
        if let Some(range) = igneous_markdown::subpath_range(&doc, &text, subpath) {
            self.set_cursor_byte(range.start);
        }
    }

    /// Checks link targets again (files were added, removed or renamed).
    pub fn refresh_links(&self) {
        self.imp().view.refresh_links();
    }

    fn ctx(&self) -> &Rc<VaultContext> {
        self.imp().ctx.get().unwrap()
    }

    pub fn buffer(&self) -> &sourceview::Buffer {
        self.imp().buffer.get().unwrap()
    }

    pub fn path(&self) -> Option<VaultPath> {
        self.imp().path.borrow().clone()
    }

    pub fn state(&self) -> State {
        self.imp().state.get()
    }

    pub fn text(&self) -> String {
        let (start, end) = self.buffer().bounds();
        self.buffer().text(&start, &end, true).to_string()
    }

    /// The tab title: the note's name without `.md`.
    pub fn title(&self) -> String {
        self.path()
            .map(|p| crate::files::display_name(&p, false).0)
            .unwrap_or_default()
    }

    /// Loads `path`, replacing whatever the page showed. Saves first if needed.
    pub fn load(&self, path: &VaultPath) -> Result<(), String> {
        self.flush();
        let abs = self.ctx().abs(path);
        let imp = self.imp();
        match fs::read_text(&abs) {
            Ok((file, stamp)) => {
                imp.path.replace(Some(path.clone()));
                self.set_contents(file, Some(stamp), State::Clean);
            }
            Err(ReadError::NotUtf8(_)) => {
                let bytes = std::fs::read(&abs).map_err(|e| e.to_string())?;
                imp.path.replace(Some(path.clone()));
                let text = String::from_utf8_lossy(&bytes).into_owned();
                self.set_contents(TextFile::new(text), None, State::ReadOnly);
            }
            Err(ReadError::Io(e)) => return Err(e.to_string()),
        }
        let start = self.buffer().start_iter();
        self.buffer().place_cursor(&start);
        Ok(())
    }

    /// Loads `path` in this tab, remembering the current note for Back.
    pub fn navigate(&self, path: &VaultPath) -> Result<(), String> {
        let current = self.path();
        self.load(path)?;
        if let Some(current) = current
            && &current != path
        {
            let mut back = self.imp().back.borrow_mut();
            back.push(current);
            if back.len() > MAX_HISTORY {
                back.remove(0);
            }
            self.imp().forward.borrow_mut().clear();
        }
        Ok(())
    }

    /// Goes back (or forward) a note, skipping notes that no longer exist.
    /// Returns the note now shown.
    pub fn go(&self, forward: bool) -> Option<VaultPath> {
        let imp = self.imp();
        let (from, to) = if forward {
            (&imp.forward, &imp.back)
        } else {
            (&imp.back, &imp.forward)
        };
        loop {
            let path = from.borrow_mut().pop()?;
            if !self.ctx().abs(&path).is_file() {
                continue;
            }
            let current = self.path();
            if self.load(&path).is_ok() {
                if let Some(current) = current {
                    to.borrow_mut().push(current);
                }
                return Some(path);
            }
        }
    }

    pub fn can_go(&self, forward: bool) -> bool {
        let imp = self.imp();
        let stack = if forward { &imp.forward } else { &imp.back };
        !stack.borrow().is_empty()
    }

    /// The Back and Forward lists, oldest first, as saved in the workspace.
    pub fn history(&self) -> (Vec<VaultPath>, Vec<VaultPath>) {
        let imp = self.imp();
        (imp.back.borrow().clone(), imp.forward.borrow().clone())
    }

    pub fn set_history(&self, back: Vec<VaultPath>, forward: Vec<VaultPath>) {
        let imp = self.imp();
        imp.back.replace(back);
        imp.forward.replace(forward);
    }

    fn set_contents(&self, file: TextFile, stamp: Option<FileStamp>, state: State) {
        let imp = self.imp();
        imp.loading.set(true);
        let buffer = self.buffer();
        buffer.begin_irreversible_action();
        buffer.set_text(file.text());
        buffer.end_irreversible_action();
        imp.loading.set(false);
        imp.file.replace(file);
        imp.stamp.replace(stamp);
        self.set_state(state);
        self.imp().view.set_editable(state != State::ReadOnly);
        self.update_conflicts();
    }

    fn set_state(&self, state: State) {
        let imp = self.imp();
        imp.state.set(state);
        let banner = &imp.banner;
        let (title, button) = match state {
            State::Clean | State::Dirty => {
                banner.set_revealed(false);
                return;
            }
            State::ChangedOnDisk => (
                "This note was changed by another program".to_owned(),
                Some("_Resolve…"),
            ),
            State::Deleted => (
                "This note was deleted on disk".to_owned(),
                Some("_Save Again"),
            ),
            State::SaveFailed => (
                format!(
                    "Couldn't save: {}",
                    imp.save_error
                        .borrow()
                        .as_deref()
                        .unwrap_or("unknown error")
                ),
                Some("_Retry"),
            ),
            State::ReadOnly => (
                "This file isn't valid UTF-8 text, so it's read-only".to_owned(),
                None,
            ),
        };
        banner.set_title(&glib::markup_escape_text(&title));
        banner.set_button_label(button);
        banner.set_use_markup(true);
        banner.set_revealed(true);
    }

    fn on_changed(&self) {
        let imp = self.imp();
        if imp.loading.get() {
            return;
        }
        match self.state() {
            State::Clean => self.set_state(State::Dirty),
            State::Dirty => {}
            // Keep editing, but don't save until the conflict is resolved.
            State::ChangedOnDisk | State::Deleted | State::SaveFailed | State::ReadOnly => return,
        }
        if let Some(id) = imp.autosave.take() {
            id.remove();
        }
        let delay = Duration::from_millis(u64::from(self.ctx().settings.editor.autosave_delay_ms));
        let page = self.downgrade();
        let id = glib::timeout_add_local_once(delay, move || {
            if let Some(page) = page.upgrade() {
                page.imp().autosave.take();
                page.save();
            }
        });
        imp.autosave.replace(Some(id));
    }

    /// Saves now if there are unsaved edits.
    pub fn flush(&self) {
        if let Some(id) = self.imp().autosave.take() {
            id.remove();
        }
        if self.state() == State::Dirty {
            self.save();
        }
    }

    fn save(&self) -> bool {
        let expect_stamp = self.imp().stamp.borrow().clone();
        match &expect_stamp {
            Some(stamp) => self.write(Expect::Contents(stamp)),
            None => self.write(Expect::Absent),
        }
    }

    fn write(&self, expect: Expect<'_>) -> bool {
        let Some(path) = self.path() else {
            return false;
        };
        let imp = self.imp();
        let mut file = imp.file.borrow().clone();
        file.set_text(self.text());
        match fs::write_atomic(&self.ctx().abs(&path), &file, expect) {
            Ok(stamp) => {
                self.ctx().expect_write(&path, &stamp);
                imp.file.replace(file);
                imp.stamp.replace(Some(stamp));
                self.set_state(State::Clean);
                self.update_conflicts();
                true
            }
            Err(WriteError::ChangedOnDisk { current: None }) => {
                self.set_state(State::Deleted);
                false
            }
            Err(WriteError::ChangedOnDisk { current: Some(_) }) => {
                self.set_state(State::ChangedOnDisk);
                false
            }
            Err(WriteError::Io(e)) => {
                imp.save_error.replace(Some(e.to_string()));
                self.set_state(State::SaveFailed);
                false
            }
        }
    }

    fn on_banner_button(&self) {
        match self.state() {
            State::ChangedOnDisk => self.resolve_conflict(),
            State::Deleted => {
                self.write(Expect::Absent);
            }
            State::SaveFailed => {
                self.save();
            }
            _ => {}
        }
    }

    fn resolve_conflict(&self) {
        let dialog = adw::AlertDialog::builder()
            .heading("Note Changed on Disk")
            .body(
                "Another program changed this note while it had unsaved edits here. \
                 Reloading discards the edits made here.",
            )
            .close_response("cancel")
            .default_response("keep")
            .build();
        dialog.add_responses(&[
            ("cancel", "_Cancel"),
            ("reload", "_Reload"),
            ("keep", "_Keep This Version"),
        ]);
        dialog.set_response_appearance("reload", adw::ResponseAppearance::Destructive);
        dialog.set_response_appearance("keep", adw::ResponseAppearance::Suggested);
        let page = self.downgrade();
        dialog.connect_response(None, move |_, response| {
            let Some(page) = page.upgrade() else { return };
            match response {
                "reload" => page.reload(),
                "keep" => {
                    page.write(Expect::Anything);
                }
                _ => {}
            }
        });
        dialog.present(Some(self));
    }

    /// Reloads from disk, keeping the cursor where it was.
    pub fn reload(&self) {
        let Some(path) = self.path() else { return };
        let cursor = self.cursor_offset();
        if let Err(e) = self.load(&path) {
            tracing::warn!(%e, "reload failed");
            return;
        }
        let iter = self.buffer().iter_at_offset(cursor);
        self.buffer().place_cursor(&iter);
    }

    /// The file changed on disk (and not because of this page).
    pub fn on_disk_changed(&self) {
        let Some(path) = self.path() else { return };
        let Ok(Some(now)) = fs::current_stamp(&self.ctx().abs(&path)) else {
            return;
        };
        let same = self
            .imp()
            .stamp
            .borrow()
            .as_ref()
            .is_some_and(|s| s.same_contents(&now));
        if same {
            return;
        }
        match self.state() {
            State::Clean | State::Deleted => self.reload(),
            State::Dirty | State::SaveFailed => {
                if let Some(id) = self.imp().autosave.take() {
                    id.remove();
                }
                self.set_state(State::ChangedOnDisk);
            }
            State::ChangedOnDisk | State::ReadOnly => {}
        }
    }

    /// The file was removed from disk.
    pub fn on_disk_removed(&self) {
        let Some(path) = self.path() else { return };
        if self.ctx().abs(&path).exists() {
            return;
        }
        if self.state() != State::ReadOnly {
            if let Some(id) = self.imp().autosave.take() {
                id.remove();
            }
            self.imp().stamp.replace(None);
            self.set_state(State::Deleted);
        }
    }

    /// The file was moved or renamed; follow it.
    pub fn set_path(&self, path: VaultPath) {
        self.imp().path.replace(Some(path));
    }

    /// Forgets unsaved edits (the file was deleted on purpose).
    pub fn discard(&self) {
        if let Some(id) = self.imp().autosave.take() {
            id.remove();
        }
        self.imp().state.set(State::Clean);
    }

    pub fn cursor_offset(&self) -> i32 {
        self.buffer()
            .iter_at_mark(&self.buffer().get_insert())
            .offset()
    }

    /// The cursor as a byte offset into the note (as stored in the workspace).
    pub fn cursor_byte(&self) -> usize {
        let text = self.text();
        text.char_indices()
            .nth(self.cursor_offset() as usize)
            .map_or(text.len(), |(b, _)| b)
    }

    pub fn set_cursor_byte(&self, byte: usize) {
        let text = self.text();
        let mut byte = byte.min(text.len());
        while !text.is_char_boundary(byte) {
            byte -= 1;
        }
        let offset = text[..byte].chars().count() as i32;
        let iter = self.buffer().iter_at_offset(offset);
        self.buffer().place_cursor(&iter);
        let view = self.imp().view.get();
        let mark = self.buffer().get_insert();
        glib::idle_add_local_once(glib::clone!(
            #[weak]
            view,
            move || view.scroll_to_mark(&mark, 0.0, true, 0.0, 0.3)
        ));
    }

    pub fn set_theme(&self, theme: &igneous_editor::theme::Theme, dark: bool) {
        self.imp().view.set_theme(theme, dark);
    }

    pub fn focus_editor(&self) {
        self.imp().view.grab_focus();
    }

    // --- Git conflicts -----------------------------------------------------------

    pub fn set_rebasing(&self, rebasing: bool) {
        self.imp().rebasing.set(rebasing);
    }

    /// Shows or hides the conflict bar, and says which conflict is current.
    pub fn update_conflicts(&self) {
        let imp = self.imp();
        let text = self.text();
        let found = if text.contains("<<<<<<<") {
            conflicts::find(&text)
        } else {
            Vec::new()
        };
        imp.conflict_revealer.set_reveal_child(!found.is_empty());
        if found.is_empty() {
            return;
        }
        let current = current_conflict(&found, self.cursor_byte());
        imp.conflict_label
            .set_label(&format!("Conflict {} of {}", current + 1, found.len()));
    }

    pub fn has_conflicts(&self) -> bool {
        self.imp().conflict_revealer.reveals_child()
    }

    fn resolve_conflict_block(&self, side: Side) {
        let text = self.text();
        let found = conflicts::find(&text);
        if found.is_empty() {
            return;
        }
        let conflict = &found[current_conflict(&found, self.cursor_byte())];
        // In a rebase the local commits are replayed onto the remote ones,
        // so the upper side is theirs.
        let upper_is_mine = !self.imp().rebasing.get();
        let keep = match (side, upper_is_mine) {
            (Side::Both, _) => Keep::Both,
            (Side::Mine, true) | (Side::Theirs, false) => Keep::Upper,
            (Side::Mine, false) | (Side::Theirs, true) => Keep::Lower,
        };
        let replacement = conflicts::resolve(&text, conflict, keep);
        let chars = |byte: usize| text[..byte].chars().count() as i32;
        let buffer = self.buffer();
        let mut start = buffer.iter_at_offset(chars(conflict.range.start));
        let mut end = buffer.iter_at_offset(chars(conflict.range.end));
        buffer.begin_user_action();
        buffer.delete(&mut start, &mut end);
        buffer.insert(&mut start, &replacement);
        buffer.end_user_action();
        let at = buffer.iter_at_offset(chars(conflict.range.start));
        buffer.place_cursor(&at);
        self.update_conflicts();
        self.jump_to_conflict_from(conflict.range.start, true);
    }

    fn jump_to_conflict(&self, forward: bool) {
        self.jump_to_conflict_from(self.cursor_byte(), forward);
    }

    fn jump_to_conflict_from(&self, from: usize, forward: bool) {
        let text = self.text();
        let found = conflicts::find(&text);
        let target = if forward {
            found
                .iter()
                .find(|c| c.range.start > from)
                .or(found.first())
        } else {
            found
                .iter()
                .rev()
                .find(|c| c.range.start < from && !c.range.contains(&from))
                .or(found.last())
        };
        if let Some(conflict) = target {
            self.set_cursor_byte(conflict.range.start);
            self.update_conflicts();
        }
    }
}

#[derive(Clone, Copy)]
enum Side {
    Mine,
    Theirs,
    Both,
}

/// The conflict containing `cursor`, else the next one, else the last.
fn current_conflict(found: &[conflicts::Conflict], cursor: usize) -> usize {
    found
        .iter()
        .position(|c| c.range.end > cursor)
        .unwrap_or(found.len().saturating_sub(1))
}
