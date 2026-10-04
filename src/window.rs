//! A window onto one vault: the file tree, tabs, and everything that keeps them
//! in step with the disk and with `.igneous/workspace.json`.

use std::cell::{Cell, OnceCell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use adw::{prelude::*, subclass::prelude::*};
use gtk::{gio, glib};
use igneous_core::fs::{self as corefs, Expect};
use igneous_core::settings::{
    Location, SidebarPane, TabKind, TabState, TrashMode, Workspace, WorkspaceStore,
};
use igneous_core::watch::VaultEvent;
use igneous_core::{TextFile, VaultPath};

use crate::application::Application;
use crate::files::FileTree;
use crate::image_page::{self, ImagePage};
use crate::note_page::NotePage;
use crate::quick_switcher::{Choice, QuickSwitcher};
use crate::vault::VaultContext;
use crate::{config, gsettings};

const MAX_RECENT_FILES: usize = 50;
const MAX_CLOSED_TABS: usize = 20;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/dev/h4rl3y/igneous/window.ui")]
    pub struct Window {
        #[template_child]
        pub toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub split_view: TemplateChild<adw::OverlaySplitView>,
        #[template_child]
        pub vault_title: TemplateChild<adw::WindowTitle>,
        #[template_child]
        pub files_view: TemplateChild<gtk::ListView>,
        #[template_child]
        pub note_title: TemplateChild<adw::WindowTitle>,
        #[template_child]
        pub tab_view: TemplateChild<adw::TabView>,
        #[template_child]
        pub content_stack: TemplateChild<gtk::Stack>,
        pub ctx: OnceCell<Rc<VaultContext>>,
        pub tree: OnceCell<Rc<FileTree>>,
        pub workspace: RefCell<Option<WorkspaceStore>>,
        pub workspace_timer: RefCell<Option<glib::SourceId>>,
        pub recently_closed: RefCell<Vec<TabState>>,
        pub recent_files: RefCell<Vec<VaultPath>>,
        pub restoring: Cell<bool>,
        pub menu_page: RefCell<Option<adw::TabPage>>,
        /// The workspace as loaded, so fields Igneous doesn't manage survive.
        pub base_workspace: RefCell<Workspace>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Window {
        const NAME: &'static str = "IgneousWindow";
        type Type = super::Window;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            super::install_actions(klass);
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for Window {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: std::sync::LazyLock<Vec<glib::ParamSpec>> =
                std::sync::LazyLock::new(|| {
                    vec![glib::ParamSpecBoolean::builder("menu-page-pinned").build()]
                });
            PROPERTIES.as_ref()
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            match pspec.name() {
                "menu-page-pinned" => self.obj().set_menu_page_pinned(value.get().unwrap()),
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "menu-page-pinned" => self.obj().menu_page_pinned().to_value(),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            if config::PROFILE == "Devel" {
                obj.add_css_class("devel");
            }
            let settings = gsettings::settings();
            obj.set_default_size(settings.int("window-width"), settings.int("window-height"));
            if settings.boolean("window-maximized") {
                obj.maximize();
            }
        }
    }

    impl WidgetImpl for Window {}

    impl WindowImpl for Window {
        fn close_request(&self) -> glib::Propagation {
            self.obj().on_close();
            self.parent_close_request()
        }
    }

    impl ApplicationWindowImpl for Window {}
    impl AdwApplicationWindowImpl for Window {}
}

glib::wrapper! {
    pub struct Window(ObjectSubclass<imp::Window>)
        @extends adw::ApplicationWindow, gtk::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable,
                    gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

fn path_param(param: Option<&glib::Variant>) -> Option<VaultPath> {
    let s = param?.get::<String>()?;
    if s.is_empty() {
        None
    } else {
        VaultPath::new(&s).ok()
    }
}

fn install_actions(klass: &mut <imp::Window as ObjectSubclass>::Class) {
    let string = Some(glib::VariantTy::STRING);
    klass.install_action("win.new-note", None, |w, _, _| w.new_note(None));
    klass.install_action("win.new-folder", None, |w, _, _| w.new_folder_dialog(None));
    klass.install_action("win.quick-switcher", None, |w, _, _| {
        w.show_quick_switcher()
    });
    klass.install_action("win.close-tab", None, |w, _, _| {
        let view = w.imp().tab_view.get();
        if let Some(page) = view.selected_page() {
            view.close_page(&page);
        }
    });
    klass.install_action("win.reopen-tab", None, |w, _, _| w.reopen_tab());
    klass.install_action("win.toggle-sidebar", None, |w, _, _| {
        let split = &w.imp().split_view;
        split.set_show_sidebar(!split.shows_sidebar());
    });
    klass.install_action("win.save", None, |w, _, _| {
        if let Some(note) = w.selected_note() {
            note.flush();
        }
    });
    klass.install_action("win.rename-note", None, |w, _, _| {
        if let Some(path) = w.selected_path() {
            w.rename_dialog(path);
        }
    });
    klass.install_action("win.file-open-new-tab", string, |w, _, p| {
        if let Some(path) = path_param(p) {
            w.open_path(&path, true);
        }
    });
    klass.install_action("win.file-new-note", string, |w, _, p| {
        w.new_note(Some(path_param(p)))
    });
    klass.install_action("win.file-new-folder", string, |w, _, p| {
        w.new_folder_dialog(path_param(p))
    });
    klass.install_action("win.file-rename", string, |w, _, p| {
        if let Some(path) = path_param(p) {
            w.rename_dialog(path);
        }
    });
    klass.install_action("win.file-trash", string, |w, _, p| {
        if let Some(path) = path_param(p) {
            w.trash(path);
        }
    });
    klass.install_action("win.file-show", string, |w, _, p| {
        if let Some(path) = path_param(p) {
            w.show_in_files(&path);
        }
    });
    klass.install_action("win.tab-close", None, |w, _, _| {
        if let Some(page) = w.imp().menu_page.borrow().clone() {
            w.imp().tab_view.close_page(&page);
        }
    });
    klass.install_action("win.tab-close-others", None, |w, _, _| {
        if let Some(page) = w.imp().menu_page.borrow().clone() {
            w.imp().tab_view.close_other_pages(&page);
        }
    });
    klass.install_property_action("win.tab-pinned", "menu-page-pinned");
}

impl Window {
    pub fn new(app: &Application, root: &Path) -> Result<Self, String> {
        let ctx = Rc::new(VaultContext::open(root)?);
        let window: Self = glib::Object::builder().property("application", app).build();
        window.set_vault(ctx);
        Ok(window)
    }

    /// A window that isn't attached to an application (used by tests).
    pub fn for_vault(root: &Path) -> Result<Self, String> {
        crate::init();
        let ctx = Rc::new(VaultContext::open(root)?);
        let window: Self = glib::Object::new();
        window.set_vault(ctx);
        Ok(window)
    }

    fn ctx(&self) -> &Rc<VaultContext> {
        self.imp().ctx.get().unwrap()
    }

    fn tree(&self) -> &Rc<FileTree> {
        self.imp().tree.get().unwrap()
    }

    pub fn root(&self) -> PathBuf {
        self.ctx().root().to_path_buf()
    }

    pub fn toast(&self, message: &str) {
        self.imp().toast_overlay.add_toast(adw::Toast::new(message));
    }

    fn set_vault(&self, ctx: Rc<VaultContext>) {
        let imp = self.imp();
        let name = ctx.name();
        self.set_title(Some(&name));
        imp.vault_title.set_title(&name);
        imp.ctx.set(ctx.clone()).ok().unwrap();
        if let Some(error) = &ctx.settings_error {
            self.toast(&format!(
                "Couldn't read vault settings, using defaults: {error}"
            ));
        }

        // File tree.
        let tree = FileTree::new(ctx.clone());
        imp.files_view.set_model(Some(&tree.selection));
        imp.files_view.set_factory(Some(&tree.factory()));
        let weak = self.downgrade();
        tree.connect_open(move |path, new_tab| {
            if let Some(w) = weak.upgrade() {
                w.open_path(&path, new_tab);
            }
        });
        let weak = self.downgrade();
        tree.connect_move(move |from, into| {
            if let Some(w) = weak.upgrade() {
                w.move_into(from, into);
            }
        });
        imp.files_view.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, position| {
                window.tree().activate(position);
                window.schedule_workspace_save();
            }
        ));
        tree.model.connect_items_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _, _, _| window.schedule_workspace_save()
        ));
        imp.tree.set(tree).ok().unwrap();

        // Tabs.
        let view = imp.tab_view.get();
        view.connect_selected_page_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.on_selected_page()
        ));
        view.connect_n_pages_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |view| {
                let name = if view.n_pages() > 0 { "tabs" } else { "empty" };
                window.imp().content_stack.set_visible_child_name(name);
                window.schedule_workspace_save();
            }
        ));
        view.connect_page_reordered(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _, _| window.schedule_workspace_save()
        ));
        view.connect_close_page(glib::clone!(
            #[weak(rename_to = window)]
            self,
            #[upgrade_or]
            glib::Propagation::Stop,
            move |view, page| {
                window.remember_closed(page);
                view.close_page_finish(page, !page.is_pinned());
                glib::Propagation::Stop
            }
        ));
        let tab_menu = gio::Menu::new();
        tab_menu.append(Some("_Pinned"), Some("win.tab-pinned"));
        let close = gio::Menu::new();
        close.append(Some("_Close"), Some("win.tab-close"));
        close.append(Some("Close _Other Tabs"), Some("win.tab-close-others"));
        tab_menu.append_section(None, &close);
        view.set_menu_model(Some(&tab_menu));
        view.connect_setup_menu(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, page| {
                window.imp().menu_page.replace(page.cloned());
                window.notify("menu-page-pinned");
            }
        ));

        imp.split_view.connect_show_sidebar_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.schedule_workspace_save()
        ));

        // Save when the window loses focus.
        self.connect_is_active_notify(|window| {
            if !window.is_active() {
                window.flush_all();
            }
        });

        // Watch the disk and list every file for the quick switcher.
        let weak = self.downgrade();
        ctx.watch(move |events| {
            if let Some(w) = weak.upgrade() {
                w.on_vault_events(events);
            }
        });
        let vault = ctx.vault.clone();
        let scan_ctx = ctx.clone();
        glib::spawn_future_local(async move {
            if let Ok(files) = gio::spawn_blocking(move || VaultContext::scan(&vault)).await {
                scan_ctx.set_files(files);
            }
        });

        self.restore_workspace();
    }

    // --- tabs ----------------------------------------------------------------

    fn pages(&self) -> Vec<adw::TabPage> {
        let view = &self.imp().tab_view;
        (0..view.n_pages()).map(|i| view.nth_page(i)).collect()
    }

    fn page_path(page: &adw::TabPage) -> Option<VaultPath> {
        let child = page.child();
        if let Some(note) = child.downcast_ref::<NotePage>() {
            note.path()
        } else if let Some(image) = child.downcast_ref::<ImagePage>() {
            image.path()
        } else {
            None
        }
    }

    fn notes(&self) -> Vec<NotePage> {
        self.pages()
            .iter()
            .filter_map(|p| p.child().downcast::<NotePage>().ok())
            .collect()
    }

    pub fn selected_note(&self) -> Option<NotePage> {
        self.imp()
            .tab_view
            .selected_page()
            .and_then(|p| p.child().downcast::<NotePage>().ok())
    }

    pub fn selected_path(&self) -> Option<VaultPath> {
        self.imp()
            .tab_view
            .selected_page()
            .as_ref()
            .and_then(Self::page_path)
    }

    fn find_page(&self, path: &VaultPath) -> Option<adw::TabPage> {
        self.pages()
            .into_iter()
            .find(|p| Self::page_path(p).as_ref() == Some(path))
    }

    fn update_tab(page: &adw::TabPage, path: &VaultPath) {
        page.set_title(&crate::files::display_name(path, false).0);
        page.set_tooltip(&glib::markup_escape_text(path.as_str()));
    }

    /// Opens a file: notes and images in a tab, anything else in its default
    /// app. Notes replace the current tab unless it's pinned or `new_tab`.
    pub fn open_path(&self, path: &VaultPath, new_tab: bool) {
        let imp = self.imp();
        let ctx = self.ctx().clone();
        let abs = ctx.abs(path);
        if !abs.is_file() {
            self.toast(&format!("“{path}” no longer exists"));
            return;
        }
        if let Some(page) = self.find_page(path) {
            imp.tab_view.set_selected_page(&page);
            return;
        }
        self.remember_recent(path);
        let is_note = is_text(path);
        if !is_note && !image_page::is_image(path) {
            gtk::FileLauncher::new(Some(&gio::File::for_path(&abs))).launch(
                Some(self),
                gio::Cancellable::NONE,
                |_| {},
            );
            return;
        }
        let current = imp.tab_view.selected_page();
        if is_note
            && !new_tab
            && let Some(page) = &current
            && !page.is_pinned()
            && let Ok(note) = page.child().downcast::<NotePage>()
        {
            match note.load(path) {
                Ok(()) => {
                    Self::update_tab(page, path);
                    self.on_selected_page();
                    note.focus_editor();
                }
                Err(e) => self.toast(&format!("Couldn't open “{path}”: {e}")),
            }
            return;
        }
        let child: gtk::Widget = if is_note {
            let note = NotePage::new(&ctx);
            if let Err(e) = note.load(path) {
                self.toast(&format!("Couldn't open “{path}”: {e}"));
                return;
            }
            note.upcast()
        } else {
            ImagePage::new(path, &abs).upcast()
        };
        let page = imp.tab_view.add_page(&child, current.as_ref());
        Self::update_tab(&page, path);
        imp.tab_view.set_selected_page(&page);
        if let Ok(note) = child.downcast::<NotePage>() {
            note.focus_editor();
        }
    }

    fn on_selected_page(&self) {
        let imp = self.imp();
        let path = self.selected_path();
        match &path {
            Some(path) => {
                imp.note_title
                    .set_title(&crate::files::display_name(path, false).0);
                let folder = path.parent().map(|p| p.to_string());
                imp.note_title
                    .set_subtitle(&folder.unwrap_or_else(|| self.ctx().name()));
                self.remember_recent(path);
            }
            None => {
                imp.note_title.set_title("Igneous");
                imp.note_title.set_subtitle("");
            }
        }
        self.tree().select(path.as_ref());
        self.schedule_workspace_save();
    }

    fn remember_recent(&self, path: &VaultPath) {
        let mut recent = self.imp().recent_files.borrow_mut();
        recent.retain(|p| p != path);
        recent.insert(0, path.clone());
        recent.truncate(MAX_RECENT_FILES);
    }

    fn tab_state(page: &adw::TabPage) -> Option<TabState> {
        let child = page.child();
        let (kind, cursor) = if let Some(note) = child.downcast_ref::<NotePage>() {
            (TabKind::Note, note.cursor_byte())
        } else if child.is::<ImagePage>() {
            (TabKind::Image, 0)
        } else {
            return None;
        };
        let mut state = TabState::new(kind, Some(Self::page_path(page)?));
        state.cursor = cursor;
        state.pinned = page.is_pinned();
        Some(state)
    }

    fn remember_closed(&self, page: &adw::TabPage) {
        if let Ok(note) = page.child().downcast::<NotePage>() {
            note.flush();
        }
        if page.is_pinned() {
            return;
        }
        if let Some(state) = Self::tab_state(page) {
            let mut closed = self.imp().recently_closed.borrow_mut();
            closed.push(state);
            if closed.len() > MAX_CLOSED_TABS {
                closed.remove(0);
            }
        }
    }

    fn reopen_tab(&self) {
        let state = self.imp().recently_closed.borrow_mut().pop();
        if let Some(state) = state
            && let Some(path) = &state.path
        {
            self.open_path(path, true);
            if let Some(note) = self.selected_note() {
                note.set_cursor_byte(state.cursor);
            }
        }
    }

    fn flush_all(&self) {
        for note in self.notes() {
            note.flush();
        }
    }

    // --- changes on disk -------------------------------------------------------

    pub fn on_vault_events(&self, events: Vec<VaultEvent>) {
        let ctx = self.ctx();
        ctx.apply_events(&events);
        self.tree().refresh_for_events(&events);
        for event in &events {
            match event {
                VaultEvent::Modified(path) | VaultEvent::Created(path) => {
                    if let Some(note) = self
                        .find_page(path)
                        .and_then(|p| p.child().downcast::<NotePage>().ok())
                    {
                        note.on_disk_changed();
                    }
                }
                VaultEvent::Removed(path) => {
                    for page in self.pages() {
                        if Self::page_path(&page).is_some_and(|p| p.starts_with(path))
                            && let Ok(note) = page.child().downcast::<NotePage>()
                        {
                            note.on_disk_removed();
                        }
                    }
                }
                VaultEvent::Renamed { from, to } => self.follow_rename(from, to),
            }
        }
    }

    /// Points tabs and history at the new location of a renamed file or folder.
    fn follow_rename(&self, from: &VaultPath, to: &VaultPath) {
        let moved = |path: &VaultPath| -> Option<VaultPath> {
            if path == from {
                return Some(to.clone());
            }
            let rest = path
                .as_str()
                .strip_prefix(from.as_str())?
                .strip_prefix('/')?;
            to.join(rest).ok()
        };
        for page in self.pages() {
            let Some(new) = Self::page_path(&page).and_then(|p| moved(&p)) else {
                continue;
            };
            let child = page.child();
            if let Some(note) = child.downcast_ref::<NotePage>() {
                note.set_path(new.clone());
            } else if let Some(image) = child.downcast_ref::<ImagePage>() {
                image.set_path(new.clone());
            }
            Self::update_tab(&page, &new);
        }
        for path in self.imp().recent_files.borrow_mut().iter_mut() {
            if let Some(new) = moved(path) {
                *path = new;
            }
        }
        self.on_selected_page();
    }

    // --- file operations -----------------------------------------------------

    /// Where new notes go: `folder` if given (`Some(None)` is the root),
    /// otherwise the vault's new-note location.
    fn new_note_folder(&self, folder: Option<Option<VaultPath>>) -> Option<VaultPath> {
        if let Some(folder) = folder {
            return folder;
        }
        match &self.ctx().settings.files.new_note_location {
            Location::VaultRoot => None,
            Location::SameFolder => self.selected_path().and_then(|p| p.parent()),
            Location::Folder(f) => VaultPath::new(f).ok(),
            Location::Subfolder(sub) => {
                let base = self.selected_path().and_then(|p| p.parent());
                match base {
                    Some(base) => base.join(sub).ok(),
                    None => VaultPath::new(sub).ok(),
                }
            }
        }
    }

    pub fn new_note(&self, folder: Option<Option<VaultPath>>) {
        let ctx = self.ctx().clone();
        let folder = self.new_note_folder(folder);
        if let Some(folder) = &folder
            && let Err(e) = std::fs::create_dir_all(ctx.abs(folder))
        {
            self.toast(&format!("Couldn't create the folder: {e}"));
            return;
        }
        let path = ctx.vault.unused_name(folder.as_ref(), "Untitled", "md");
        match corefs::write_atomic(&ctx.abs(&path), &TextFile::new(""), Expect::Absent) {
            Ok(_) => {
                self.on_vault_events(vec![VaultEvent::Created(path.clone())]);
                self.open_path(&path, true);
                self.rename_dialog(path);
            }
            Err(e) => self.toast(&format!("Couldn't create a note: {e}")),
        }
    }

    pub fn new_folder_dialog(&self, parent: Option<VaultPath>) {
        let entry = gtk::Entry::builder()
            .text("Untitled")
            .activates_default(true)
            .build();
        let dialog = name_dialog("New Folder", "_Create", &entry);
        let ctx = self.ctx().clone();
        let validate_parent = parent.clone();
        entry.connect_changed(glib::clone!(
            #[weak]
            dialog,
            move |entry| {
                let ok = valid_name(&entry.text())
                    && child_path(validate_parent.as_ref(), &entry.text())
                        .is_some_and(|p| !ctx.abs(&p).exists());
                dialog.set_response_enabled("ok", ok);
            }
        ));
        dialog.connect_response(
            Some("ok"),
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                #[weak]
                entry,
                move |_, _| {
                    let Some(path) = child_path(parent.as_ref(), &entry.text()) else {
                        return;
                    };
                    match std::fs::create_dir(window.ctx().abs(&path)) {
                        Ok(()) => window.on_vault_events(vec![VaultEvent::Created(path)]),
                        Err(e) => window.toast(&format!("Couldn't create the folder: {e}")),
                    }
                }
            ),
        );
        dialog.present(Some(self));
        entry.grab_focus();
    }

    pub fn rename_dialog(&self, path: VaultPath) {
        let ctx = self.ctx().clone();
        let folder = ctx.abs(&path).is_dir();
        let is_note = !folder && path.extension() == Some("md");
        let current = if is_note {
            path.stem()
        } else {
            path.file_name()
        };
        let entry = gtk::Entry::builder()
            .text(current)
            .activates_default(true)
            .build();
        let heading = if folder {
            "Rename Folder"
        } else if is_note {
            "Rename Note"
        } else {
            "Rename File"
        };
        let dialog = name_dialog(heading, "_Rename", &entry);
        let parent = path.parent();
        let target = move |name: &str| -> Option<VaultPath> {
            let name = name.trim();
            if !valid_name(name) {
                return None;
            }
            let name = if is_note {
                format!("{name}.md")
            } else {
                name.to_owned()
            };
            child_path(parent.as_ref(), &name)
        };
        let target_for_validation = target.clone();
        let original = path.clone();
        let ctx_for_validation = ctx.clone();
        entry.connect_changed(glib::clone!(
            #[weak]
            dialog,
            move |entry| {
                let ok = target_for_validation(&entry.text()).is_some_and(|t| {
                    t != original && (!ctx_for_validation.abs(&t).exists() || t.eq_loose(&original))
                });
                dialog.set_response_enabled("ok", ok);
            }
        ));
        dialog.set_response_enabled("ok", false);
        let from = path.clone();
        dialog.connect_response(
            Some("ok"),
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                #[weak]
                entry,
                move |_, _| {
                    if let Some(to) = target(&entry.text()) {
                        window.rename(&from, &to);
                    }
                }
            ),
        );
        dialog.present(Some(self));
        entry.grab_focus();
        entry.select_region(0, -1);
    }

    pub fn rename(&self, from: &VaultPath, to: &VaultPath) {
        let ctx = self.ctx();
        if let Some(note) = self
            .find_page(from)
            .and_then(|p| p.child().downcast::<NotePage>().ok())
        {
            note.flush();
        }
        // Renaming that only changes case needs a hop on case-insensitive disks.
        let result = if from.eq_loose(to) && from != to {
            let hop = ctx
                .vault
                .unused_name(from.parent().as_ref(), ".igneous-rename", "");
            corefs::rename(&ctx.abs(from), &ctx.abs(&hop))
                .and_then(|()| corefs::rename(&ctx.abs(&hop), &ctx.abs(to)))
        } else {
            corefs::rename(&ctx.abs(from), &ctx.abs(to))
        };
        match result {
            Ok(()) => self.on_vault_events(vec![VaultEvent::Renamed {
                from: from.clone(),
                to: to.clone(),
            }]),
            Err(e) => self.toast(&format!("Couldn't rename “{from}”: {e}")),
        }
    }

    /// Moves a dragged file or folder into `into` (`None` is the root).
    fn move_into(&self, from: VaultPath, into: Option<VaultPath>) {
        if from.parent() == into || into.as_ref().is_some_and(|i| i.starts_with(&from)) {
            return;
        }
        let Some(to) = child_path(into.as_ref(), from.file_name()) else {
            return;
        };
        if self.ctx().abs(&to).exists() {
            self.toast(&format!("“{}” already exists there", from.file_name()));
            return;
        }
        self.rename(&from, &to);
    }

    pub fn trash(&self, path: VaultPath) {
        if !self.ctx().settings.files.confirm_delete {
            self.trash_now(&path);
            return;
        }
        let (name, _) = crate::files::display_name(&path, self.ctx().abs(&path).is_dir());
        let dialog = adw::AlertDialog::builder()
            .heading(format!("Move “{name}” to the Trash?"))
            .close_response("cancel")
            .default_response("cancel")
            .build();
        dialog.add_responses(&[("cancel", "_Cancel"), ("trash", "_Move to Trash")]);
        dialog.set_response_appearance("trash", adw::ResponseAppearance::Destructive);
        dialog.connect_response(
            Some("trash"),
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_, _| window.trash_now(&path)
            ),
        );
        dialog.present(Some(self));
    }

    fn trash_now(&self, path: &VaultPath) {
        let ctx = self.ctx().clone();
        let abs = ctx.abs(path);
        // Close tabs for the file (or anything inside the folder) without saving.
        for page in self.pages() {
            if Self::page_path(&page).is_some_and(|p| p.starts_with(path)) {
                if let Ok(note) = page.child().downcast::<NotePage>() {
                    note.discard();
                }
                self.imp().tab_view.close_page(&page);
            }
        }
        let (name, _) = crate::files::display_name(path, abs.is_dir());
        match ctx.settings.files.trash {
            TrashMode::System => match gio::File::for_path(&abs).trash(gio::Cancellable::NONE) {
                Ok(()) => {
                    self.on_vault_events(vec![VaultEvent::Removed(path.clone())]);
                    self.toast(&format!("“{name}” moved to the Trash"));
                }
                Err(e) => self.toast(&format!("Couldn't move “{name}” to the Trash: {e}")),
            },
            TrashMode::VaultFolder => match corefs::move_to_vault_trash(ctx.root(), &abs) {
                Ok(dest) => {
                    self.on_vault_events(vec![VaultEvent::Removed(path.clone())]);
                    let toast = adw::Toast::builder()
                        .title(format!("“{name}” moved to .trash"))
                        .button_label("_Undo")
                        .build();
                    let original = path.clone();
                    toast.connect_button_clicked(glib::clone!(
                        #[weak(rename_to = window)]
                        self,
                        move |_| {
                            let abs = window.ctx().abs(&original);
                            match corefs::rename(&dest, &abs) {
                                Ok(()) => window
                                    .on_vault_events(vec![VaultEvent::Created(original.clone())]),
                                Err(e) => window.toast(&format!("Couldn't restore: {e}")),
                            }
                        }
                    ));
                    self.imp().toast_overlay.add_toast(toast);
                }
                Err(e) => self.toast(&format!("Couldn't move “{name}” to .trash: {e}")),
            },
        }
    }

    fn show_in_files(&self, path: &VaultPath) {
        let file = gio::File::for_path(self.ctx().abs(path));
        gtk::FileLauncher::new(Some(&file)).open_containing_folder(
            Some(self),
            gio::Cancellable::NONE,
            |_| {},
        );
    }

    fn show_quick_switcher(&self) {
        let files = self.ctx().files();
        let recent = self.imp().recent_files.borrow().clone();
        let weak = self.downgrade();
        let switcher = QuickSwitcher::new(files, recent, move |choice| {
            let Some(window) = weak.upgrade() else { return };
            match choice {
                Choice::Open { path, new_tab } => window.open_path(&path, new_tab),
                Choice::Create { name } => window.create_named_note(&name),
            }
        });
        switcher.present(Some(self));
    }

    /// Creates a note from a typed name, which may include folders.
    fn create_named_note(&self, name: &str) {
        let name = name.trim().trim_end_matches(".md");
        let Ok(path) = VaultPath::new(&format!("{name}.md")) else {
            self.toast("That isn't a valid note name");
            return;
        };
        let ctx = self.ctx().clone();
        let path = match path.parent() {
            Some(_) => path,
            None => match self.new_note_folder(None) {
                Some(folder) => folder.join(path.as_str()).unwrap_or(path),
                None => path,
            },
        };
        let abs = ctx.abs(&path);
        if !abs.exists() {
            if let Some(parent) = abs.parent()
                && let Err(e) = std::fs::create_dir_all(parent)
            {
                self.toast(&format!("Couldn't create the folder: {e}"));
                return;
            }
            if let Err(e) = corefs::write_atomic(&abs, &TextFile::new(""), Expect::Absent) {
                self.toast(&format!("Couldn't create “{name}”: {e}"));
                return;
            }
            self.on_vault_events(vec![VaultEvent::Created(path.clone())]);
        }
        self.open_path(&path, false);
    }

    // --- workspace -------------------------------------------------------------

    fn schedule_workspace_save(&self) {
        let imp = self.imp();
        if imp.restoring.get() {
            return;
        }
        if let Some(id) = imp.workspace_timer.take() {
            id.remove();
        }
        let weak = self.downgrade();
        let id = glib::timeout_add_local_once(Duration::from_secs(1), move || {
            if let Some(window) = weak.upgrade() {
                window.imp().workspace_timer.take();
                window.save_workspace();
            }
        });
        imp.workspace_timer.replace(Some(id));
    }

    pub fn workspace(&self) -> Workspace {
        let imp = self.imp();
        let mut workspace = imp.base_workspace.borrow().clone();
        workspace.tabs = self.pages().iter().filter_map(Self::tab_state).collect();
        workspace.active_tab = imp
            .tab_view
            .selected_page()
            .map(|p| imp.tab_view.page_position(&p) as usize);
        workspace.sidebar.visible = imp.split_view.shows_sidebar();
        workspace.sidebar.pane = SidebarPane::Files;
        workspace.sidebar.expanded = self.tree().expanded_folders();
        workspace.recently_closed = imp.recently_closed.borrow().clone();
        workspace.recent_files = imp.recent_files.borrow().clone();
        workspace
    }

    pub fn save_workspace(&self) {
        let workspace = self.workspace();
        let mut store = self.imp().workspace.borrow_mut();
        let store =
            store.get_or_insert_with(|| WorkspaceStore::new(self.ctx().vault.igneous_dir()));
        if let Err(e) = store.save_if_changed(&workspace) {
            tracing::warn!(%e, "couldn't save the workspace");
        }
    }

    fn restore_workspace(&self) {
        let imp = self.imp();
        let mut store = WorkspaceStore::new(self.ctx().vault.igneous_dir());
        let (workspace, error) = store.load();
        imp.workspace.replace(Some(store));
        imp.base_workspace.replace(workspace.clone());
        if let Some(error) = error {
            self.toast(&format!("Couldn't read the saved tabs: {error}"));
        }
        imp.restoring.set(true);
        self.tree().expand(&workspace.sidebar.expanded);
        imp.split_view.set_show_sidebar(workspace.sidebar.visible);
        imp.recently_closed
            .replace(workspace.recently_closed.clone());
        imp.recent_files.replace(workspace.recent_files.clone());
        let mut active = None;
        for (i, tab) in workspace.tabs.iter().enumerate() {
            let Some(path) = &tab.path else { continue };
            if !self.ctx().abs(path).is_file() {
                continue;
            }
            self.open_path(path, true);
            let Some(page) = self.find_page(path) else {
                continue;
            };
            imp.tab_view.set_page_pinned(&page, tab.pinned);
            if let Ok(note) = page.child().downcast::<NotePage>() {
                note.set_cursor_byte(tab.cursor);
            }
            if workspace.active_tab == Some(i) {
                active = Some(page);
            }
        }
        if let Some(page) = active {
            imp.tab_view.set_selected_page(&page);
        }
        // Opening tabs reorders the recent list; put it back as saved.
        imp.recent_files.replace(workspace.recent_files);
        imp.restoring.set(false);
        self.on_selected_page_quietly();
    }

    /// Updates titles without scheduling a workspace save.
    fn on_selected_page_quietly(&self) {
        let imp = self.imp();
        imp.restoring.set(true);
        self.on_selected_page();
        imp.restoring.set(false);
    }

    fn on_close(&self) {
        self.flush_all();
        if let Some(id) = self.imp().workspace_timer.take() {
            id.remove();
        }
        self.save_workspace();
        let settings = gsettings::settings();
        let (width, height) = self.default_size();
        let _ = settings.set_int("window-width", width);
        let _ = settings.set_int("window-height", height);
        let _ = settings.set_boolean("window-maximized", self.is_maximized());
        if let Some(app) = self.application().and_downcast::<Application>() {
            app.save_open_vaults(Some(self));
        }
    }

    /// Paths of the rows currently shown in the file tree, top to bottom.
    pub fn sidebar_paths(&self) -> Vec<VaultPath> {
        let model = &self.tree().model;
        (0..model.n_items())
            .filter_map(|i| {
                model
                    .item(i)
                    .and_downcast::<gtk::TreeListRow>()?
                    .item()
                    .and_downcast::<crate::files::FileItem>()
            })
            .map(|item| item.path())
            .collect()
    }

    /// Paths of the open tabs, in order.
    pub fn tab_paths(&self) -> Vec<VaultPath> {
        self.pages().iter().filter_map(Self::page_path).collect()
    }

    // --- tab menu property -----------------------------------------------------

    fn menu_page_pinned(&self) -> bool {
        self.imp()
            .menu_page
            .borrow()
            .as_ref()
            .is_some_and(adw::TabPage::is_pinned)
    }

    fn set_menu_page_pinned(&self, pinned: bool) {
        if let Some(page) = self.imp().menu_page.borrow().clone() {
            self.imp().tab_view.set_page_pinned(&page, pinned);
            self.schedule_workspace_save();
        }
    }
}

/// Text formats that open in the editor.
fn is_text(path: &VaultPath) -> bool {
    matches!(
        path.extension().map(str::to_lowercase).as_deref(),
        Some("md" | "markdown" | "txt" | "base" | "canvas" | "css" | "json" | "yaml" | "yml")
    )
}

fn valid_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty() && !name.contains('/') && !name.starts_with('.') && name != ".."
}

fn child_path(parent: Option<&VaultPath>, name: &str) -> Option<VaultPath> {
    let name = name.trim();
    if !valid_name(name) {
        return None;
    }
    match parent {
        Some(parent) => parent.join(name).ok(),
        None => VaultPath::new(name).ok(),
    }
}

fn name_dialog(heading: &str, accept: &str, entry: &gtk::Entry) -> adw::AlertDialog {
    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .extra_child(entry)
        .close_response("cancel")
        .default_response("ok")
        .build();
    dialog.add_responses(&[("cancel", "_Cancel"), ("ok", accept)]);
    dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    dialog
}
