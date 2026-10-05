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
    self as vault_settings, Appearance, InspectorView, Location, SidebarPane, TabKind, TabState,
    TrashMode, Workspace, WorkspaceStore,
};
use igneous_core::watch::VaultEvent;
use igneous_core::{TextFile, VaultPath};
use igneous_editor::Mode;
use igneous_editor::theme::{Catalog, Theme};
use igneous_git::{Commit, SyncKind};
use igneous_markdown::LinkRef;

use crate::application::Application;
use crate::base_page::{self, BasePage};
use crate::changes::ChangesPane;
use crate::files::FileTree;
use crate::graph_data::GraphSource;
use crate::graph_page::GraphPage;
use crate::image_page::{self, ImagePage};
use crate::index::IndexService;
use crate::inspector::{Inspector, Links};
use crate::note_page::NotePage;
use crate::quick_switcher::{Choice, QuickSwitcher};
use crate::search_pane::SearchPane;
use crate::sync::{State as SyncState, SyncService};
use crate::sync_button::SyncButton;
use crate::tags_pane::TagsPane;
use crate::text_page::{Contents, TextPage};
use crate::vault::VaultContext;
use crate::{config, gsettings};
use igneous_index::refactor::RefactorPlan;

const MAX_RECENT_FILES: usize = 50;
const MAX_CLOSED_TABS: usize = 20;

pub(crate) mod imp {
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
        pub sidebar_switcher: TemplateChild<adw::InlineViewSwitcher>,
        #[template_child]
        pub sidebar_stack: TemplateChild<adw::ViewStack>,
        #[template_child]
        pub files_view: TemplateChild<gtk::ListView>,
        #[template_child]
        pub changes_page: TemplateChild<adw::ViewStackPage>,
        #[template_child]
        pub changes_bin: TemplateChild<adw::Bin>,
        #[template_child]
        pub search_bin: TemplateChild<adw::Bin>,
        #[template_child]
        pub tags_bin: TemplateChild<adw::Bin>,
        #[template_child]
        pub bookmarks_bin: TemplateChild<adw::Bin>,
        #[template_child]
        pub calendar_button: TemplateChild<gtk::MenuButton>,
        #[template_child]
        pub sync_slot: TemplateChild<adw::Bin>,
        #[template_child]
        pub sync_banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub mode_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub tab_button: TemplateChild<adw::TabButton>,
        #[template_child]
        pub tab_overview: TemplateChild<adw::TabOverview>,
        #[template_child]
        pub inspector_split: TemplateChild<adw::OverlaySplitView>,
        #[template_child]
        pub inspector_bin: TemplateChild<adw::Bin>,
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
        pub themes: RefCell<Catalog>,
        pub theme_id: RefCell<String>,
        pub style_handlers: RefCell<Vec<glib::SignalHandlerId>>,
        /// appearance.json's fonts and text width.
        pub text_style: RefCell<TextStyle>,
        /// The fonts as CSS for this window's notes (see `apply_text_style`).
        pub text_css: OnceCell<gtk::CssProvider>,
        pub sync: OnceCell<Rc<SyncService>>,
        pub sync_button: OnceCell<Rc<SyncButton>>,
        pub changes: OnceCell<Rc<ChangesPane>>,
        /// The sidebar pane to show once it exists (Changes appears only
        /// after Git has been found).
        pub wanted_pane: Cell<Option<SidebarPane>>,
        pub index: OnceCell<Rc<IndexService>>,
        /// Set once the template is built; property actions are queried
        /// before that.
        pub constructed: Cell<bool>,
        pub inspector: OnceCell<Rc<Inspector>>,
        pub inspector_timer: RefCell<Option<glib::SourceId>>,
        pub search: OnceCell<Rc<SearchPane>>,
        /// appearance.json's readable line length.
        pub readable: Cell<bool>,
        pub tags: OnceCell<Rc<TagsPane>>,
        pub lint: OnceCell<Rc<crate::lint::LintConfig>>,
        pub graph_timer: RefCell<Option<glib::SourceId>>,
        pub bookmarks: OnceCell<Rc<crate::bookmarks::BookmarksPane>>,
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
                    vec![
                        glib::ParamSpecBoolean::builder("menu-page-pinned").build(),
                        glib::ParamSpecString::builder("note-mode")
                            .default_value(Some(""))
                            .build(),
                    ]
                });
            PROPERTIES.as_ref()
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            match pspec.name() {
                "menu-page-pinned" => self.obj().set_menu_page_pinned(value.get().unwrap()),
                "note-mode" => self
                    .obj()
                    .set_note_mode(value.get::<Option<String>>().unwrap().as_deref()),
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "menu-page-pinned" => self.obj().menu_page_pinned().to_value(),
                // Never NULL: the win.mode action holds it as a GVariant.
                "note-mode" => self.obj().note_mode().unwrap_or_default().to_value(),
                _ => unimplemented!(),
            }
        }

        fn dispose(&self) {
            let style = adw::StyleManager::default();
            for handler in self.style_handlers.take() {
                style.disconnect(handler);
            }
            if let Some(css) = self.text_css.get() {
                gtk::style_context_remove_provider_for_display(
                    &WidgetExt::display(&*self.obj()),
                    css,
                );
            }
        }

        fn constructed(&self) {
            self.parent_constructed();
            self.constructed.set(true);
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
    klass.install_action("win.preferences", None, |w, _, _| {
        crate::preferences::Preferences::new(w).present(Some(w));
    });
    klass.install_action("win.close-tab", None, |w, _, _| {
        let view = w.imp().tab_view.get();
        if let Some(page) = view.selected_page() {
            view.close_page(&page);
        }
    });
    klass.install_action("win.reopen-tab", None, |w, _, _| w.reopen_tab());
    klass.install_action("win.go-back", None, |w, _, _| w.go(false));
    klass.install_action("win.search", None, |w, _, _| w.show_search());
    klass.install_action("win.cycle-mode", None, |w, _, _| {
        if let Some(note) = w.selected_note() {
            note.set_mode(next_mode(note.mode()));
            note.focus_editor();
            w.sync_mode_toggle();
            w.schedule_workspace_save();
        }
    });
    klass.install_action("win.toggle-reading", None, |w, _, _| {
        if let Some(note) = w.selected_note() {
            note.toggle_reading();
            w.sync_mode_toggle();
            w.schedule_workspace_save();
        }
    });
    klass.install_action("win.go-forward", None, |w, _, _| w.go(true));
    klass.install_action("win.graph", None, |w, _, _| {
        w.open_graph();
    });
    klass.install_action("win.command-palette", None, |w, _, _| {
        crate::command_palette::show(w)
    });
    klass.install_action("win.toggle-sidebar", None, |w, _, _| {
        let split = &w.imp().split_view;
        split.set_show_sidebar(!split.shows_sidebar());
    });
    klass.install_action("win.save", None, |w, _, _| w.save_selected_note());
    klass.install_action("win.lint-note", None, |w, _, _| w.lint_selected_note());
    klass.install_action("win.lint-vault", None, |w, _, _| w.lint_folder_dialog(None));
    klass.install_action("win.lint-folder", string, |w, _, p| {
        w.lint_folder_dialog(path_param(p))
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
    klass.install_property_action("win.mode", "note-mode");

    // Git.
    klass.install_action("win.sync-now", None, |w, _, _| w.sync_now(SyncKind::Full));
    klass.install_action("win.pull", None, |w, _, _| w.sync_now(SyncKind::Pull));
    klass.install_action("win.show-changes", None, |w, _, _| w.show_changes());
    klass.install_action("win.publish-branch", None, |w, _, _| w.publish_branch());
    klass.install_action("win.note-history", None, |w, _, _| {
        if let Some(path) = w.selected_path() {
            w.show_history(&path);
        }
    });
    klass.install_action("win.file-history", string, |w, _, p| {
        if let Some(path) = path_param(p) {
            w.show_history(&path);
        }
    });

    // Daily notes, templates, bookmarks and file recovery.
    crate::builtins::install_actions(klass);
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

    pub(crate) fn ctx(&self) -> &Rc<VaultContext> {
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
        imp.sidebar_stack
            .connect_visible_child_name_notify(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_| window.schedule_workspace_save()
            ));

        // Git sync.
        self.set_git_actions_enabled(false);
        let sync = SyncService::new(ctx.root(), ctx.vault.igneous_dir());
        let weak = self.downgrade();
        sync.set_before_commit(move || {
            if let Some(window) = weak.upgrade() {
                window.flush_all();
            }
        });
        let weak = self.downgrade();
        sync.connect_changed(move || {
            if let Some(window) = weak.upgrade() {
                window.on_sync_changed();
            }
        });
        let button = SyncButton::new(&sync);
        imp.sync_slot.set_child(Some(&button.button));
        imp.sync_button.set(button).ok().unwrap();
        let changes = ChangesPane::new(&sync, self);
        imp.changes_bin.set_child(Some(&changes.widget));
        imp.changes.set(changes).ok().unwrap();
        imp.sync_banner.connect_button_clicked(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.show_changes()
        ));
        imp.sync.set(sync).ok().unwrap();

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

        // The index, and the inspector that shows what it knows.
        let index = IndexService::new(&ctx.vault);
        index.set_overrides(ctx.settings.borrow().properties.types.clone());
        let weak = self.downgrade();
        index.connect_changed(move || {
            if let Some(window) = weak.upgrade() {
                window.update_inspector();
                let imp = window.imp();
                if let Some(tags) = imp.tags.get() {
                    tags.set_tags(&window.index().tags());
                }
                if let Some(search) = imp.search.get() {
                    search.refresh();
                }
                for base in window.bases() {
                    base.refresh();
                }
                window.schedule_graph_refresh();
            }
        });
        let search = SearchPane::new(self);
        imp.search_bin.set_child(Some(&search.widget));
        imp.search.set(search).ok().unwrap();
        let tags = TagsPane::new(self);
        imp.tags_bin.set_child(Some(&tags.widget));
        tags.set_tags(&[]);
        imp.tags.set(tags).ok().unwrap();
        imp.index.set(index).ok().unwrap();
        imp.lint
            .set(crate::lint::LintConfig::load(&ctx.vault.igneous_dir()))
            .ok()
            .unwrap();
        let inspector = Inspector::new(self);
        imp.inspector_bin.set_child(Some(&inspector.widget));
        imp.inspector.set(inspector).ok().unwrap();
        imp.inspector_split
            .connect_show_sidebar_notify(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_| {
                    window.update_inspector();
                    window.schedule_workspace_save();
                }
            ));

        imp.tab_button.connect_clicked(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.imp().tab_overview.set_open(true)
        ));

        // Save when the window loses focus; check Git when it comes back.
        self.connect_is_active_notify(|window| {
            if window.is_active() {
                window.sync().refresh();
            } else {
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
        let weak_window = self.downgrade();
        glib::spawn_future_local(async move {
            if let Ok(files) = gio::spawn_blocking(move || VaultContext::scan(&vault)).await {
                scan_ctx.set_files(files);
                // Links can be checked now that every file is known.
                if let Some(window) = weak_window.upgrade() {
                    for note in window.notes() {
                        note.refresh_links();
                    }
                }
            }
        });

        // Editor theme: follow the desktop's light/dark style and accent colour.
        self.load_themes();
        let style = adw::StyleManager::default();
        let weak = self.downgrade();
        let on_dark = style.connect_dark_notify(move |_| {
            if let Some(window) = weak.upgrade() {
                window.apply_editor_theme();
            }
        });
        let weak = self.downgrade();
        let on_accent = style.connect_accent_color_notify(move |_| {
            if let Some(window) = weak.upgrade() {
                window.apply_editor_theme();
            }
        });
        // Fonts: the vault's, or the desktop's document and monospace fonts.
        let css = gtk::CssProvider::new();
        gtk::style_context_add_provider_for_display(
            &WidgetExt::display(self),
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        self.imp().text_css.set(css).ok();
        self.apply_text_style();
        let mut handlers = vec![on_dark, on_accent];
        for property in ["document-font-name", "monospace-font-name"] {
            let weak = self.downgrade();
            handlers.push(style.connect_notify_local(Some(property), move |_, _| {
                if let Some(window) = weak.upgrade() {
                    window.apply_text_style();
                }
            }));
        }
        self.imp().style_handlers.replace(handlers);

        self.restore_workspace();
        self.set_up_builtins();
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
        } else if let Some(base) = child.downcast_ref::<BasePage>() {
            base.path()
        } else {
            None
        }
    }

    fn bases(&self) -> Vec<BasePage> {
        self.pages()
            .iter()
            .filter_map(|p| p.child().downcast::<BasePage>().ok())
            .collect()
    }

    /// Gives a note the vault's theme, fonts and text width.
    pub fn style_note(&self, note: &NotePage) {
        note.set_theme(&self.editor_theme(), adw::StyleManager::default().is_dark());
        self.style_note_text(note);
    }

    fn style_note_text(&self, note: &NotePage) {
        let style = self.imp().text_style.borrow().clone();
        let view = note.view();
        view.add_css_class(&self.text_class());
        view.set_readable_line_length(self.readable_line_length());
        view.set_readable_width(
            style
                .line_width
                .map_or(igneous_editor::READABLE_WIDTH, |w| w as i32),
        );
        let monospace = style
            .monospace
            .clone()
            .unwrap_or_else(|| desktop_font(false).0);
        view.set_monospace_family(Some(&monospace));
    }

    /// The CSS class this window's notes get their fonts by.
    fn text_class(&self) -> String {
        format!("igneous-text-{:x}", self.as_ptr() as usize)
    }

    /// The vault's fonts and text width, from appearance.json.
    pub fn text_style(&self) -> TextStyle {
        self.imp().text_style.borrow().clone()
    }

    /// Uses `style` for every note (the caller saves it).
    pub fn set_text_style(&self, style: TextStyle) {
        self.imp().text_style.replace(style);
        self.apply_text_style();
    }

    /// Writes the fonts into this window's CSS and restyles every note.
    fn apply_text_style(&self) {
        let Some(css) = self.imp().text_css.get() else {
            return;
        };
        let style = self.text_style();
        let (desktop_family, desktop_size) = desktop_font(true);
        let family = style.family.clone().unwrap_or(desktop_family);
        let size = style.size.unwrap_or(desktop_size);
        css.load_from_string(&format!(
            "textview.igneous-note.{} {{ font-family: {}; font-size: {size}pt; }}",
            self.text_class(),
            css_string(&family),
        ));
        for page in self.pages() {
            let child = page.child();
            let note = child.downcast_ref::<NotePage>().cloned().or_else(|| {
                child
                    .downcast_ref::<BasePage>()
                    .and_then(BasePage::source_note)
            });
            if let Some(note) = note {
                self.style_note_text(&note);
                // Formulas are rendered at the text's size.
                note.view().refresh_widgets();
            }
        }
    }

    pub(crate) fn notes(&self) -> Vec<NotePage> {
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

    pub fn selected_base(&self) -> Option<BasePage> {
        self.imp()
            .tab_view
            .selected_page()
            .and_then(|p| p.child().downcast::<BasePage>().ok())
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
        if base_page::is_base(path) {
            self.open_base(path, new_tab);
            return;
        }
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
            match note.navigate(path) {
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
            note.buffer().connect_changed(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_| window.schedule_inspector_update()
            ));
            note.buffer().connect_changed(glib::clone!(
                #[weak(rename_to = window)]
                self,
                #[weak]
                note,
                move |_| window.schedule_problems(&note)
            ));
            self.style_note(&note);
            note.set_mode(mode_from(self.ctx().settings.borrow().editor.default_mode));
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

    /// Opens a `.base` file in a Bases tab.
    fn open_base(&self, path: &VaultPath, new_tab: bool) {
        let imp = self.imp();
        let page = BasePage::new(self.ctx(), self.index());
        if let Err(e) = page.load(path) {
            self.toast(&format!("Couldn't open “{path}”: {e}"));
            return;
        }
        let current = imp.tab_view.selected_page();
        // Like notes, a base replaces an unpinned note tab unless asked not to.
        let replace = current
            .as_ref()
            .filter(|p| !new_tab && !p.is_pinned() && p.child().is::<NotePage>());
        let tab = imp.tab_view.add_page(&page, current.as_ref());
        if let Some(old) = replace {
            imp.tab_view.close_page(old);
        }
        Self::update_tab(&tab, path);
        imp.tab_view.set_selected_page(&tab);
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
            None if self.selected_graph().is_some() => {
                imp.note_title.set_title("Graph");
                imp.note_title.set_subtitle(&self.ctx().name());
            }
            None => {
                let text_page = imp
                    .tab_view
                    .selected_page()
                    .and_then(|p| p.child().downcast::<TextPage>().ok());
                match text_page {
                    Some(page) => {
                        imp.note_title.set_title(&page.title());
                        imp.note_title
                            .set_subtitle(page.path().as_ref().map_or("", |p| p.as_str()));
                    }
                    None => {
                        imp.note_title.set_title("Igneous");
                        imp.note_title.set_subtitle("");
                    }
                }
            }
        }
        self.tree().select(path.as_ref());
        self.sync_mode_toggle();
        self.update_inspector();
        self.focus_selected_note();
        self.schedule_workspace_save();
    }

    /// Puts the focus in the selected note's text once the tab view has
    /// switched pages (it focuses a page's first focusable widget, which in
    /// a note is the properties header). Focus elsewhere, such as in the
    /// sidebar, is left alone.
    fn focus_selected_note(&self) {
        glib::idle_add_local_once(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move || {
                let Some(note) = window.selected_note() else {
                    return;
                };
                let focus = gtk::prelude::GtkWindowExt::focus(&window);
                let view = note.view();
                let in_tabs = focus.as_ref().is_none_or(|f| {
                    f.is_ancestor(&*window.imp().tab_view) && f != view.upcast_ref::<gtk::Widget>()
                });
                if in_tabs {
                    note.focus_editor();
                }
            }
        ));
    }

    fn note_mode(&self) -> Option<String> {
        if !self.imp().constructed.get() {
            return None;
        }
        self.selected_note().map(|note| {
            match note.mode() {
                Mode::Live => "live",
                Mode::Source => "source",
                Mode::Reading => "reading",
            }
            .to_owned()
        })
    }

    /// The main menu's mode items (shown on narrow windows).
    fn set_note_mode(&self, mode: Option<&str>) {
        let Some(note) = self.selected_note() else {
            return;
        };
        if mode.is_none_or(str::is_empty) {
            return;
        }
        note.set_mode(match mode {
            Some("source") => Mode::Source,
            Some("reading") => Mode::Reading,
            _ => Mode::Live,
        });
        self.sync_mode_toggle();
        self.schedule_workspace_save();
    }

    /// Points the header bar's mode button at the next mode, as Nautilus's
    /// view button does: its icon and tooltip say what a click switches to.
    fn sync_mode_toggle(&self) {
        let imp = self.imp();
        let note = self.selected_note();
        self.action_set_enabled("win.mode", note.is_some());
        self.notify("note-mode");
        imp.mode_button.set_visible(note.is_some());
        if let Some(note) = note {
            let (icon, tooltip) = match next_mode(note.mode()) {
                Mode::Live => ("format-text-rich-symbolic", "Switch to Live Preview"),
                Mode::Source => ("text-x-generic-symbolic", "Switch to Source"),
                Mode::Reading => ("view-reveal-symbolic", "Switch to Reading"),
            };
            imp.mode_button.set_icon_name(icon);
            imp.mode_button.set_tooltip_text(Some(tooltip));
        }
    }

    /// Follows a link from `from`: opens the file (scrolling to a heading or
    /// block), opens a web link in the browser, or creates a missing note.
    pub fn follow_link(&self, link: &LinkRef, from: Option<&VaultPath>, new_tab: bool) {
        let target = link.target.trim();
        if target.contains("://") || target.starts_with("mailto:") {
            gtk::UriLauncher::new(target).launch(Some(self), gio::Cancellable::NONE, |_| {});
            return;
        }
        let path = match self.ctx().resolve(link, from) {
            Some(path) => path,
            None => {
                // Obsidian creates the note when an unresolved link is followed.
                let name = target.trim_end_matches(".md");
                if name.is_empty() {
                    return;
                }
                self.create_named_note_in(name, new_tab);
                return;
            }
        };
        if Some(&path) != from || new_tab {
            self.open_path(&path, new_tab);
        }
        if let (Some(subpath), Some(note)) = (&link.subpath, self.selected_note()) {
            note.scroll_to_subpath(subpath);
        }
    }

    fn remember_recent(&self, path: &VaultPath) {
        let mut recent = self.imp().recent_files.borrow_mut();
        recent.retain(|p| p != path);
        recent.insert(0, path.clone());
        recent.truncate(MAX_RECENT_FILES);
    }

    fn tab_state(page: &adw::TabPage) -> Option<TabState> {
        let child = page.child();
        if child.is::<GraphPage>() {
            let mut state = TabState::new(TabKind::Graph, None);
            state.pinned = page.is_pinned();
            return Some(state);
        }
        let (kind, cursor) = if let Some(note) = child.downcast_ref::<NotePage>() {
            (TabKind::Note, note.cursor_byte())
        } else if child.is::<ImagePage>() {
            (TabKind::Image, 0)
        } else if child.is::<BasePage>() {
            (TabKind::Base, 0)
        } else {
            return None;
        };
        let mut state = TabState::new(kind, Some(Self::page_path(page)?));
        state.cursor = cursor;
        state.pinned = page.is_pinned();
        if let Some(note) = child.downcast_ref::<NotePage>() {
            (state.back, state.forward) = note.history();
            state.mode = Some(mode_to(note.mode()));
        }
        Some(state)
    }

    fn remember_closed(&self, page: &adw::TabPage) {
        if let Ok(note) = page.child().downcast::<NotePage>() {
            note.flush();
        }
        if let Ok(base) = page.child().downcast::<BasePage>() {
            base.flush();
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

    /// Back or forward in the selected tab's history.
    fn go(&self, forward: bool) {
        let imp = self.imp();
        let (Some(page), Some(note)) = (imp.tab_view.selected_page(), self.selected_note()) else {
            return;
        };
        if let Some(path) = note.go(forward) {
            Self::update_tab(&page, &path);
            self.on_selected_page();
            note.focus_editor();
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

    pub fn flush_all(&self) {
        for note in self.notes() {
            note.flush();
        }
        for base in self.bases() {
            base.flush();
            if let Some(note) = base.source_note() {
                note.flush();
            }
        }
    }

    // --- changes on disk -------------------------------------------------------

    pub fn on_vault_events(&self, events: Vec<VaultEvent>) {
        self.sync().refresh();
        self.index().apply(events.clone());
        let ctx = self.ctx();
        ctx.apply_events(&events);
        // Files appearing or disappearing change which links resolve.
        if events.iter().any(|e| !matches!(e, VaultEvent::Modified(_))) {
            for note in self.notes() {
                note.refresh_links();
            }
        }
        self.tree().refresh_for_events(&events);
        for event in &events {
            match event {
                VaultEvent::Modified(path) | VaultEvent::Created(path) => {
                    let child = self.find_page(path).map(|p| p.child());
                    if let Some(note) = child.as_ref().and_then(|c| c.downcast_ref::<NotePage>()) {
                        note.on_disk_changed();
                    } else if let Some(base) =
                        child.as_ref().and_then(|c| c.downcast_ref::<BasePage>())
                    {
                        base.on_disk_changed();
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
            } else if let Some(base) = child.downcast_ref::<BasePage>() {
                base.set_path(new.clone());
            }
            Self::update_tab(&page, &new);
        }
        for path in self.imp().recent_files.borrow_mut().iter_mut() {
            if let Some(new) = moved(path) {
                *path = new;
            }
        }
        self.bookmarks_follow_rename(from, to);
        self.on_selected_page();
    }

    // --- file operations -----------------------------------------------------

    /// Where new notes go: `folder` if given (`Some(None)` is the root),
    /// otherwise the vault's new-note location.
    fn new_note_folder(&self, folder: Option<Option<VaultPath>>) -> Option<VaultPath> {
        if let Some(folder) = folder {
            return folder;
        }
        match &self.ctx().settings.borrow().files.new_note_location {
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
        // Plan link updates against the index as it is before the move.
        let plan =
            (ctx.settings.borrow().links.update_on_rename && self.index().is_ready()).then(|| {
                let (from, to) = (from.clone(), to.clone());
                self.index()
                    .submit(move |index| igneous_index::refactor::plan_rename(index, &from, &to))
            });
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
            Ok(()) => {
                self.on_vault_events(vec![VaultEvent::Renamed {
                    from: from.clone(),
                    to: to.clone(),
                }]);
                if let Some(plan) = plan {
                    self.update_links_after_rename(plan, from.clone(), to.clone());
                }
            }
            Err(e) => self.toast(&format!("Couldn't rename “{from}”: {e}")),
        }
    }

    /// Rewrites links to a renamed file or folder once it has moved, then
    /// offers to undo the whole rename.
    fn update_links_after_rename(
        &self,
        plan: async_channel::Receiver<Option<Result<RefactorPlan, igneous_index::IndexError>>>,
        from: VaultPath,
        to: VaultPath,
    ) {
        let window = self.downgrade();
        glib::spawn_future_local(async move {
            let Ok(Some(Ok(plan))) = plan.recv().await else {
                return;
            };
            let Some(window) = window.upgrade() else {
                return;
            };
            let mut undo: Vec<(VaultPath, String)> = Vec::new();
            let mut skipped = 0;
            for file in &plan.files {
                if file.edits.is_empty() {
                    continue;
                }
                let new_text = file.apply();
                let result = window.edit_note(&file.path_after, |current| {
                    (current == file.text).then(|| new_text.clone())
                });
                match result {
                    Ok(()) => undo.push((file.path_after.clone(), file.text.clone())),
                    Err(e) => {
                        tracing::warn!(path = %file.path_after, %e, "skipped a link update");
                        skipped += 1;
                    }
                }
            }
            if undo.is_empty() && skipped == 0 {
                return;
            }
            let notes = |n: usize| {
                if n == 1 {
                    "1 note".to_owned()
                } else {
                    format!("{n} notes")
                }
            };
            let mut title = format!("Updated links in {}", notes(undo.len()));
            if skipped > 0 {
                title.push_str(&format!("; {} changed and were skipped", notes(skipped)));
            }
            let toast = adw::Toast::builder()
                .title(title)
                .button_label("_Undo")
                .timeout(8)
                .build();
            let weak = window.downgrade();
            toast.connect_button_clicked(move |_| {
                let Some(window) = weak.upgrade() else { return };
                window.undo_rename(&from, &to, &undo);
            });
            window.imp().toast_overlay.add_toast(toast);
        });
    }

    /// Moves a renamed file back and restores the notes whose links were
    /// rewritten, unless they've changed since.
    fn undo_rename(&self, from: &VaultPath, to: &VaultPath, texts: &[(VaultPath, String)]) {
        let ctx = self.ctx();
        if ctx.abs(from).exists() {
            self.toast(&format!("Can’t undo: “{from}” exists again"));
            return;
        }
        // Back to the old text first, at the paths the notes have now.
        for (path, old) in texts {
            let _ = self.edit_note(path, |_| Some(old.clone()));
        }
        match corefs::rename(&ctx.abs(to), &ctx.abs(from)) {
            Ok(()) => self.on_vault_events(vec![VaultEvent::Renamed {
                from: to.clone(),
                to: from.clone(),
            }]),
            Err(e) => self.toast(&format!("Couldn't move “{to}” back: {e}")),
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
        if !self.ctx().settings.borrow().files.confirm_delete {
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
        match ctx.settings.borrow().files.trash {
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
        let aliases = self
            .index()
            .notes()
            .iter()
            .filter(|n| !n.aliases.is_empty())
            .map(|n| (n.path.clone(), n.aliases.clone()))
            .collect();
        let ctx = self.ctx().clone();
        let headings = move |path: &VaultPath| -> Vec<(String, u8, usize)> {
            let Ok((file, _)) = corefs::read_text(&ctx.abs(path)) else {
                return Vec::new();
            };
            igneous_markdown::parse(file.text())
                .headings
                .into_iter()
                .map(|h| (h.text, h.level, h.range.start))
                .collect()
        };
        let weak = self.downgrade();
        let switcher = QuickSwitcher::new(files, recent, aliases, headings, move |choice| {
            let Some(window) = weak.upgrade() else { return };
            match choice {
                Choice::Open { path, new_tab, at } => {
                    window.open_path(&path, new_tab);
                    if let (Some(at), Some(note)) = (at, window.selected_note()) {
                        note.set_cursor_byte(at);
                    }
                }
                Choice::Create { name } => window.create_named_note(&name),
            }
        });
        switcher.present(Some(self));
    }

    /// Creates a note from a typed name, which may include folders.
    fn create_named_note(&self, name: &str) {
        self.create_named_note_in(name, false);
    }

    fn create_named_note_in(&self, name: &str, new_tab: bool) {
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
        self.open_path(&path, new_tab);
    }

    // --- editor theme ----------------------------------------------------------

    /// Where the user's own themes go.
    pub fn user_themes_dir() -> PathBuf {
        glib::user_data_dir().join("igneous").join("themes")
    }

    /// Loads every theme and the vault's choice (or the app-wide default).
    fn load_themes(&self) {
        let ctx = self.ctx();
        let catalog = Catalog::load(
            Some(&Self::user_themes_dir()),
            Some(&ctx.vault.igneous_dir().join("themes")),
        );
        for (path, error) in &catalog.errors {
            tracing::warn!(path = %path.display(), %error, "skipping a theme");
        }
        let appearance = vault_settings::load::<Appearance>(&ctx.vault.igneous_dir()).ok();
        self.imp()
            .readable
            .set(appearance.as_ref().is_none_or(|a| a.readable_line_length));
        self.imp().text_style.replace(
            appearance
                .as_ref()
                .map(TextStyle::from_appearance)
                .unwrap_or_default(),
        );
        let id = appearance
            .and_then(|a| a.editor_theme)
            .unwrap_or_else(|| gsettings::settings().string("editor-theme").to_string());
        let imp = self.imp();
        imp.themes.replace(catalog);
        imp.theme_id.replace(id);
    }

    /// The vault context. For tests.
    #[doc(hidden)]
    pub fn ctx_for_test(&self) -> Rc<VaultContext> {
        self.ctx().clone()
    }

    pub fn readable_line_length(&self) -> bool {
        self.imp().readable.get()
    }

    pub fn set_readable_line_length(&self, readable: bool) {
        self.imp().readable.set(readable);
        self.apply_text_style();
    }

    /// Applies changed editor settings to every open note.
    pub fn apply_editor_settings(&self) {
        let editor = self.ctx().settings.borrow().editor.clone();
        for note in self.notes() {
            note.apply_editor_settings(&editor);
        }
    }

    /// Re-reads theme files, e.g. after the user added one.
    pub fn reload_themes(&self) {
        self.load_themes();
        self.apply_editor_theme();
    }

    pub fn themes(&self) -> Catalog {
        self.imp().themes.borrow().clone()
    }

    pub fn editor_theme_id(&self) -> String {
        self.editor_theme().id
    }

    fn editor_theme(&self) -> Theme {
        let imp = self.imp();
        imp.themes
            .borrow()
            .get_or_default(&imp.theme_id.borrow())
            .clone()
    }

    fn editor_scheme(&self) -> Option<sourceview::StyleScheme> {
        igneous_editor::style_scheme(&self.editor_theme(), adw::StyleManager::default().is_dark())
    }

    fn apply_editor_theme(&self) {
        let scheme = self.editor_scheme();
        let theme = self.editor_theme();
        let dark = adw::StyleManager::default().is_dark();
        for page in self.pages() {
            let child = page.child();
            if let Some(note) = child.downcast_ref::<NotePage>() {
                note.set_theme(&theme, dark);
            } else if let Some(text) = child.downcast_ref::<TextPage>() {
                text.set_style_scheme(scheme.as_ref());
            } else if let Some(note) = child
                .downcast_ref::<BasePage>()
                .and_then(BasePage::source_note)
            {
                note.set_theme(&theme, dark);
            }
        }
    }

    /// Uses `id` in this vault, and makes it the default for vaults that
    /// haven't chosen a theme.
    pub fn set_editor_theme(&self, id: &str) {
        self.imp().theme_id.replace(id.to_owned());
        let dir = self.ctx().vault.igneous_dir();
        match vault_settings::load::<Appearance>(&dir) {
            Ok(mut appearance) => {
                appearance.editor_theme = Some(id.to_owned());
                if let Err(e) = vault_settings::save(&dir, &appearance) {
                    self.toast(&format!("Couldn't save the theme for this vault: {e}"));
                }
            }
            Err(e) => self.toast(&format!(
                "Couldn't save the theme for this vault, because appearance.json can't be read: {e}"
            )),
        }
        let _ = gsettings::settings().set_string("editor-theme", id);
        self.apply_editor_theme();
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
        workspace.sidebar.pane = match imp.sidebar_stack.visible_child_name().as_deref() {
            Some("changes") => SidebarPane::Changes,
            Some("search") => SidebarPane::Search,
            Some("tags") => SidebarPane::Tags,
            Some("bookmarks") => SidebarPane::Bookmarks,
            _ => SidebarPane::Files,
        };
        workspace.sidebar.expanded = self.tree().expanded_folders();
        workspace.inspector.visible = imp.inspector_split.shows_sidebar();
        if let Some(inspector) = imp.inspector.get() {
            workspace.inspector.view = match inspector.stack.visible_child_name().as_deref() {
                Some("outline") => InspectorView::Outline,
                Some("graph") => InspectorView::LocalGraph,
                _ => InspectorView::Backlinks,
            };
        }
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
        imp.inspector_split
            .set_show_sidebar(workspace.inspector.visible);
        if let Some(inspector) = imp.inspector.get() {
            match workspace.inspector.view {
                InspectorView::Outline => inspector.stack.set_visible_child_name("outline"),
                InspectorView::LocalGraph => inspector.stack.set_visible_child_name("graph"),
                InspectorView::Backlinks => {}
            }
        }
        match workspace.sidebar.pane {
            SidebarPane::Search => imp.sidebar_stack.set_visible_child_name("search"),
            SidebarPane::Tags => imp.sidebar_stack.set_visible_child_name("tags"),
            SidebarPane::Bookmarks => imp.sidebar_stack.set_visible_child_name("bookmarks"),
            pane => imp.wanted_pane.set(Some(pane)),
        }
        imp.recently_closed
            .replace(workspace.recently_closed.clone());
        imp.recent_files.replace(workspace.recent_files.clone());
        let mut active = None;
        for (i, tab) in workspace.tabs.iter().enumerate() {
            if tab.kind == TabKind::Graph {
                let page = self.open_graph();
                imp.tab_view.set_page_pinned(&page, tab.pinned);
                if workspace.active_tab == Some(i) {
                    active = Some(page);
                }
                continue;
            }
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
                note.set_history(tab.back.clone(), tab.forward.clone());
                if let Some(mode) = tab.mode {
                    note.set_mode(mode_from(mode));
                }
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

    /// The text of the selected tab, if it shows text.
    pub fn selected_tab_text(&self) -> Option<String> {
        let child = self.imp().tab_view.selected_page()?.child();
        if let Some(note) = child.downcast_ref::<NotePage>() {
            Some(note.text())
        } else {
            child.downcast_ref::<TextPage>().map(TextPage::text)
        }
    }

    /// Paths of the open tabs, in order.
    pub fn tab_paths(&self) -> Vec<VaultPath> {
        self.pages().iter().filter_map(Self::page_path).collect()
    }

    // --- search and tags ---------------------------------------------------------

    fn show_search(&self) {
        let imp = self.imp();
        imp.split_view.set_show_sidebar(true);
        imp.sidebar_stack.set_visible_child_name("search");
        if let Some(search) = imp.search.get() {
            search.entry.grab_focus();
            search.entry.select_region(0, -1);
        }
    }

    /// Shows the Search pane with `query`'s results.
    pub fn search_vault(&self, query: &str) {
        self.show_search();
        if let Some(search) = self.imp().search.get() {
            search.search(query);
        }
    }

    pub fn rename_tag_dialog(&self, tag: &str) {
        let entry = gtk::Entry::builder()
            .text(format!("#{tag}"))
            .activates_default(true)
            .build();
        let dialog = name_dialog("Rename Tag", "_Rename", &entry);
        dialog.set_body(&format!(
            "#{tag} and the tags nested in it are renamed in every note, including in tags properties"
        ));
        let old = tag.to_owned();
        let validate_old = old.clone();
        entry.connect_changed(glib::clone!(
            #[weak]
            dialog,
            move |entry| {
                let new = entry.text().trim().trim_start_matches('#').to_owned();
                let valid = !new.is_empty()
                    && new != validate_old
                    && !new.contains(char::is_whitespace)
                    && new.chars().any(|c| !c.is_ascii_digit() && c != '/');
                dialog.set_response_enabled("ok", valid);
            }
        ));
        dialog.set_response_enabled("ok", false);
        dialog.connect_response(
            Some("ok"),
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                #[weak]
                entry,
                move |_, _| {
                    let new = entry.text().trim().trim_start_matches('#').to_owned();
                    window.rename_tag(&old, &new);
                }
            ),
        );
        dialog.present(Some(self));
        entry.grab_focus();
        entry.select_region(1, -1);
    }

    /// The notes in the Search pane's results. For tests.
    #[doc(hidden)]
    pub fn search_results(&self) -> Vec<VaultPath> {
        self.imp()
            .search
            .get()
            .map(|s| s.found.borrow().clone())
            .unwrap_or_default()
    }

    pub fn rename_tag(&self, old: &str, new: &str) {
        self.flush_all();
        let window = self.downgrade();
        let index = self.index().clone();
        let (old, new) = (old.to_owned(), new.to_owned());
        glib::spawn_future_local(async move {
            let (o, n) = (old.clone(), new.clone());
            let plan = index
                .query(move |index| igneous_index::refactor::plan_tag_rename(index, &o, &n))
                .await;
            let Some(window) = window.upgrade() else {
                return;
            };
            let plan = match plan {
                Some(Ok(plan)) => plan,
                Some(Err(e)) => return window.toast(&format!("Couldn’t rename the tag: {e}")),
                None => return,
            };
            let mut undo: Vec<(VaultPath, String)> = Vec::new();
            for file in &plan.files {
                if file.edits.is_empty() {
                    continue;
                }
                let text = file.apply();
                if window
                    .edit_note(&file.path, |current| {
                        (current == file.text).then(|| text.clone())
                    })
                    .is_ok()
                {
                    undo.push((file.path.clone(), file.text.clone()));
                }
            }
            let n = undo.len();
            let toast = adw::Toast::builder()
                .title(format!(
                    "Renamed #{old} to #{new} in {}",
                    if n == 1 {
                        "1 note".to_owned()
                    } else {
                        format!("{n} notes")
                    }
                ))
                .button_label("_Undo")
                .timeout(8)
                .build();
            let weak = window.downgrade();
            toast.connect_button_clicked(move |_| {
                if let Some(window) = weak.upgrade() {
                    for (path, text) in &undo {
                        let _ = window.edit_note(path, |_| Some(text.clone()));
                    }
                }
            });
            window.imp().toast_overlay.add_toast(toast);
        });
    }

    // --- index and inspector ------------------------------------------------------

    pub fn index(&self) -> &Rc<IndexService> {
        self.imp().index.get().unwrap()
    }

    pub fn lint(&self) -> &Rc<crate::lint::LintConfig> {
        self.imp().lint.get().unwrap()
    }

    fn schedule_inspector_update(&self) {
        let imp = self.imp();
        if !imp.inspector_split.shows_sidebar() {
            return;
        }
        if let Some(id) = imp.inspector_timer.take() {
            id.remove();
        }
        let weak = self.downgrade();
        let id = glib::timeout_add_local_once(Duration::from_millis(500), move || {
            if let Some(window) = weak.upgrade() {
                window.imp().inspector_timer.take();
                window.update_outline();
            }
        });
        imp.inspector_timer.replace(Some(id));
    }

    fn update_outline(&self) {
        let Some(inspector) = self.imp().inspector.get() else {
            return;
        };
        let note = self.selected_note();
        let headings = note
            .as_ref()
            .map(|n| igneous_markdown::parse(&n.text()).headings)
            .unwrap_or_default();
        inspector.show_outline(note.and_then(|n| n.path()).as_ref(), &headings);
    }

    /// Fills the selected note's Linked Mentions section, when the vault
    /// shows one (Preferences → Editor).
    pub fn update_linked_mentions(&self) {
        let Some(note) = self.selected_note() else {
            return;
        };
        if !self.ctx().settings.borrow().editor.backlinks_in_document {
            note.view().set_linked_mentions(None);
            return;
        }
        let Some(path) = note.path() else { return };
        let index = self.index().clone();
        let weak = note.downgrade();
        glib::spawn_future_local(async move {
            let query = path.clone();
            let hits = index
                .query(move |index| index.backlinks(&query))
                .await
                .and_then(Result::ok)
                .unwrap_or_default();
            let Some(note) = weak.upgrade() else { return };
            if note.path().as_ref() != Some(&path) {
                return;
            }
            let mentions = hits
                .into_iter()
                .map(|hit| igneous_editor::Mention {
                    title: crate::files::display_name(&hit.source, false).0,
                    path: hit.source.to_string(),
                    line: hit.line_text,
                    at: hit.range.start,
                })
                .collect();
            note.view().set_linked_mentions(Some(mentions));
        });
    }

    /// Refreshes the inspector for the selected note, if it's showing.
    pub fn update_inspector(&self) {
        self.update_linked_mentions();
        let imp = self.imp();
        if !imp.inspector_split.shows_sidebar() {
            return;
        }
        let Some(inspector) = imp.inspector.get().cloned() else {
            return;
        };
        self.update_outline();
        let selected = self.selected_note().and_then(|n| n.path());
        inspector.local_graph.show(selected.clone());
        if !inspector.local_graph.has_source() {
            self.schedule_graph_refresh();
        }
        let Some(path) = selected else {
            inspector.show_links(None);
            return;
        };
        let index = self.index().clone();
        let mut names = index.aliases(&path);
        names.insert(0, path.stem().to_owned());
        let window = self.downgrade();
        glib::spawn_future_local(async move {
            let query = path.clone();
            let result = index
                .query(move |index| {
                    (
                        index.backlinks(&query),
                        index.unlinked_mentions(&query),
                        index.outgoing(&query),
                    )
                })
                .await;
            let Some(window) = window.upgrade() else {
                return;
            };
            // The selection may have moved on while the index was busy.
            if window.selected_note().and_then(|n| n.path()).as_ref() != Some(&path) {
                return;
            }
            let Some((backlinks, mentions, outgoing)) = result else {
                return;
            };
            inspector.show_links(Some(Links {
                path,
                backlinks: backlinks.unwrap_or_default(),
                mentions: mentions.unwrap_or_default(),
                outgoing: outgoing.unwrap_or_default(),
                names,
            }));
        });
    }

    /// Opens a note with the cursor at byte `at`.
    pub fn open_at(&self, path: &VaultPath, at: usize) {
        self.open_path(path, false);
        if let Some(note) = self.selected_note()
            && note.path().as_ref() == Some(path)
        {
            note.set_cursor_byte(at);
            note.focus_editor();
        }
    }

    /// Turns an unlinked mention of `target` into a link.
    pub fn link_mention(&self, mention: &igneous_index::Mention, target: &VaultPath) {
        let link_text = self.ctx().link_text(target);
        let edit = move |text: &str| -> Option<String> {
            let found = text.get(mention_range(mention))?;
            let replacement = if found == link_text {
                format!("[[{found}]]")
            } else {
                format!("[[{link_text}|{found}]]")
            };
            let mut out = text.to_owned();
            out.replace_range(mention_range(mention), &replacement);
            Some(out)
        };
        if let Err(e) = self.edit_note(&mention.source, edit) {
            self.toast(&e);
        }
        self.update_inspector();
    }

    /// Changes a note's text with `edit`, through its open tab if there is
    /// one (as an undoable edit), otherwise on disk with the usual guard.
    pub fn edit_note(
        &self,
        path: &VaultPath,
        edit: impl FnOnce(&str) -> Option<String>,
    ) -> Result<(), String> {
        if let Some(note) = self
            .find_page(path)
            .and_then(|p| p.child().downcast::<NotePage>().ok())
        {
            let old = note.text();
            let new = edit(&old).ok_or("The note changed; try again")?;
            note.replace_text(&old, &new);
            note.flush();
            return Ok(());
        }
        let abs = self.ctx().abs(path);
        let (mut file, stamp) = corefs::read_text(&abs).map_err(|e| e.to_string())?;
        let new = edit(file.text()).ok_or("The note changed; try again")?;
        file.set_text(new);
        let stamp = corefs::write_atomic(&abs, &file, Expect::Contents(&stamp))
            .map_err(|e| format!("Couldn’t update “{path}”: {e}"))?;
        self.ctx().expect_write(path, &stamp);
        self.index().apply(vec![VaultEvent::Modified(path.clone())]);
        Ok(())
    }

    // --- graph -------------------------------------------------------------------

    fn selected_graph(&self) -> Option<GraphPage> {
        self.imp()
            .tab_view
            .selected_page()
            .and_then(|p| p.child().downcast::<GraphPage>().ok())
    }

    /// The graph tab, if one is open.
    pub fn graph_page(&self) -> Option<GraphPage> {
        self.pages()
            .iter()
            .find_map(|p| p.child().downcast::<GraphPage>().ok())
    }

    /// Opens the graph tab, or switches to it.
    pub fn open_graph(&self) -> adw::TabPage {
        let imp = self.imp();
        if let Some(page) = self
            .pages()
            .into_iter()
            .find(|p| p.child().is::<GraphPage>())
        {
            imp.tab_view.set_selected_page(&page);
            return page;
        }
        let graph = GraphPage::new(self.ctx().vault.igneous_dir());
        let weak = self.downgrade();
        graph.view().connect_activate(move |path, new_tab| {
            if let Some(window) = weak.upgrade() {
                window.open_path(path, new_tab);
            }
        });
        let weak = self.downgrade();
        graph.connect_settings_changed(move |settings| {
            if let Some(inspector) = weak
                .upgrade()
                .and_then(|w| w.imp().inspector.get().cloned())
            {
                inspector.local_graph.set_settings(settings);
            }
        });
        let page = imp
            .tab_view
            .add_page(&graph, imp.tab_view.selected_page().as_ref());
        page.set_title("Graph");
        page.set_icon(Some(&gio::ThemedIcon::new("network-workgroup-symbolic")));
        imp.tab_view.set_selected_page(&page);
        self.refresh_graphs();
        page
    }

    /// Rebuilds the graphs soon, coalescing bursts of index changes.
    fn schedule_graph_refresh(&self) {
        let imp = self.imp();
        if let Some(id) = imp.graph_timer.take() {
            id.remove();
        }
        let weak = self.downgrade();
        let id = glib::timeout_add_local_once(Duration::from_millis(300), move || {
            if let Some(window) = weak.upgrade() {
                window.imp().graph_timer.take();
                window.refresh_graphs();
            }
        });
        imp.graph_timer.replace(Some(id));
    }

    /// Gives the graph tab and the local graph fresh data from the index,
    /// if either is showing.
    pub fn refresh_graphs(&self) {
        let imp = self.imp();
        let graph = self.graph_page();
        let local = imp
            .inspector
            .get()
            .filter(|_| imp.inspector_split.shows_sidebar())
            .map(|i| i.local_graph.clone());
        if local.is_none()
            && let Some(inspector) = imp.inspector.get()
        {
            inspector.local_graph.invalidate();
        }
        if graph.is_none() && local.is_none() {
            return;
        }
        let index = self.index().clone();
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let notes = index.note_data().await;
            let edges = index
                .query(|index| index.graph_edges())
                .await
                .and_then(Result::ok)
                .unwrap_or_default();
            let Some(window) = weak.upgrade() else { return };
            let source = Rc::new(GraphSource { notes, edges });
            let settings = window.graph_settings();
            if let Some(graph) = window.graph_page() {
                graph.set_source(source.clone());
            }
            if let Some(inspector) = window.imp().inspector.get() {
                inspector.local_graph.set_source(source, &settings);
            }
        });
    }

    /// The labels of the inspector's local graph, root first. For tests.
    #[doc(hidden)]
    pub fn local_graph_labels(&self) -> Vec<String> {
        self.imp()
            .inspector
            .get()
            .map(|i| {
                i.local_graph
                    .view
                    .model()
                    .graph
                    .nodes
                    .iter()
                    .map(|n| n.label.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Shows an inspector page ("backlinks", "outgoing", "outline" or
    /// "graph").
    pub fn show_inspector(&self, page: &str) {
        let imp = self.imp();
        imp.inspector_split.set_show_sidebar(true);
        if let Some(inspector) = imp.inspector.get() {
            inspector.stack.set_visible_child_name(page);
        }
    }

    /// The vault's graph settings (from the graph tab if it's open).
    fn graph_settings(&self) -> igneous_core::settings::GraphSettings {
        match self.graph_page() {
            Some(page) => page.settings(),
            None => vault_settings::load(&self.ctx().vault.igneous_dir()).unwrap_or_default(),
        }
    }

    // --- Git -------------------------------------------------------------------

    pub fn sync(&self) -> &Rc<SyncService> {
        self.imp().sync.get().unwrap()
    }

    fn set_git_actions_enabled(&self, enabled: bool) {
        for action in [
            "win.sync-now",
            "win.pull",
            "win.show-changes",
            "win.publish-branch",
            "win.note-history",
            "win.file-history",
        ] {
            self.action_set_enabled(action, enabled);
        }
    }

    fn on_sync_changed(&self) {
        let imp = self.imp();
        let sync = self.sync();
        let available = sync.is_available();
        self.set_git_actions_enabled(available);
        imp.changes_page.set_visible(available);
        if available && let Some(pane) = imp.wanted_pane.take() {
            imp.restoring.set(true);
            if pane == SidebarPane::Changes {
                imp.sidebar_stack.set_visible_child_name("changes");
            }
            imp.restoring.set(false);
        }
        let status = sync.status();
        let count = status.as_ref().map_or(0, |s| s.entries.len());
        imp.changes_page.set_badge_number(count as u32);
        let paused = sync.state() == SyncState::Paused;
        if paused {
            let n = status.as_ref().map_or(0, |s| s.conflicts().count());
            imp.sync_banner.set_title(&match n {
                0 => "Sync paused: a merge is waiting to be committed".to_owned(),
                1 => "Sync paused: 1 file has conflicts".to_owned(),
                n => format!("Sync paused: {n} files have conflicts"),
            });
        }
        imp.sync_banner.set_revealed(paused);
        let rebasing = status
            .as_ref()
            .is_some_and(|s| s.in_progress == Some(igneous_git::InProgress::Rebase));
        for note in self.notes() {
            note.set_rebasing(rebasing);
        }
    }

    fn sync_now(&self, kind: SyncKind) {
        let sync = self.sync().clone();
        let window = self.downgrade();
        glib::spawn_future_local(async move {
            let Some(result) = sync.run(kind, true).await else {
                return;
            };
            let Some(window) = window.upgrade() else {
                return;
            };
            match result {
                Ok(report) => window.toast(&crate::sync::describe(&report)),
                Err(e) => window.toast(&e.to_string()),
            }
        });
    }

    fn publish_branch(&self) {
        let sync = self.sync().clone();
        let window = self.downgrade();
        glib::spawn_future_local(async move {
            let result = sync.publish().await;
            if let (Some(result), Some(window)) = (result, window.upgrade()) {
                match result {
                    Ok(()) => window.toast("Branch published"),
                    Err(e) => window.toast(&e.to_string()),
                }
            }
        });
    }

    pub fn show_changes(&self) {
        let imp = self.imp();
        if !self.sync().is_available() {
            return;
        }
        imp.split_view.set_show_sidebar(true);
        imp.sidebar_stack.set_visible_child_name("changes");
        if let Some(changes) = imp.changes.get() {
            changes.focus_message();
        }
    }

    fn show_history(&self, path: &VaultPath) {
        crate::history::show(self, self.sync(), path);
    }

    /// Opens (or refreshes) a tab with a file's uncommitted changes.
    pub fn open_changes(&self, path: &VaultPath, staged: bool, untracked: bool) {
        let sync = self.sync().clone();
        let Some(git) = sync.git() else { return };
        let repo_path = git.to_repo(path.as_str());
        let window = self.downgrade();
        let path = path.clone();
        glib::spawn_future_local(async move {
            let result = sync
                .call(move |git| git.diff(&repo_path, staged, untracked))
                .await;
            let Some(window) = window.upgrade() else {
                return;
            };
            match result {
                Some(Ok(diff)) => {
                    let text = if diff.is_empty() {
                        "No changes.\n".to_owned()
                    } else {
                        diff
                    };
                    window.open_text_page(&path, Contents::Diff { staged }, &text);
                }
                Some(Err(e)) => window.toast(&format!("Couldn’t show the changes: {e}")),
                None => {}
            }
        });
    }

    /// Opens a file as it was in `commit`, read-only.
    pub fn open_version(&self, sync: &Rc<SyncService>, path: &VaultPath, commit: Commit) {
        let window = self.downgrade();
        let path = path.clone();
        let hash = commit.hash.clone();
        let old_path = commit.path.clone();
        let sync = sync.clone();
        glib::spawn_future_local(async move {
            let result = sync.call(move |git| git.show(&hash, &old_path)).await;
            let Some(window) = window.upgrade() else {
                return;
            };
            match result {
                Some(Ok(bytes)) => {
                    let text = match TextFile::from_bytes(&bytes) {
                        Ok(file) => file.text().to_owned(),
                        Err(_) => String::from_utf8_lossy(&bytes).into_owned(),
                    };
                    window.open_text_page(
                        &path,
                        Contents::Version {
                            hash: commit.hash,
                            short: commit.short,
                        },
                        &text,
                    );
                }
                Some(Err(e)) => window.toast(&format!("Couldn’t open that version: {e}")),
                None => {}
            }
        });
    }

    pub(crate) fn open_text_page(&self, path: &VaultPath, contents: Contents, text: &str) {
        let imp = self.imp();
        let page = TextPage::new(path, contents.clone(), text, self.editor_scheme().as_ref());
        let existing = self.pages().into_iter().find(|p| {
            p.child().downcast_ref::<TextPage>().is_some_and(|t| {
                t.path().as_ref() == Some(path) && t.contents() == Some(contents.clone())
            })
        });
        let position = existing.as_ref().map(|p| imp.tab_view.page_position(p));
        let tab = match position {
            Some(position) => imp.tab_view.insert(&page, position),
            None => imp
                .tab_view
                .add_page(&page, imp.tab_view.selected_page().as_ref()),
        };
        if let Some(old) = existing {
            imp.tab_view.close_page(&old);
        }
        tab.set_title(&page.title());
        tab.set_tooltip(&glib::markup_escape_text(path.as_str()));
        tab.set_icon(Some(&gio::ThemedIcon::new(match page.contents() {
            Some(Contents::Version { .. }) => "document-open-recent-symbolic",
            Some(Contents::Snapshot { .. }) => "document-revert-symbolic",
            _ => "document-edit-symbolic",
        })));
        imp.tab_view.set_selected_page(&tab);
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
        Some("md" | "markdown" | "txt" | "canvas" | "css" | "json" | "yaml" | "yml")
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

/// The fonts and text width from a vault's appearance.json.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextStyle {
    /// The text's family; None for the desktop's document font.
    pub family: Option<String>,
    /// The base size in points; None for the document font's size.
    pub size: Option<f64>,
    /// The family for code and tables; None for the desktop's monospace font.
    pub monospace: Option<String>,
    /// The text column's width with readable line length on, in pixels.
    pub line_width: Option<u32>,
}

impl TextStyle {
    pub fn from_appearance(appearance: &Appearance) -> Self {
        Self {
            family: appearance.text_font.clone().filter(|f| !f.is_empty()),
            size: appearance.font_size.filter(|s| *s > 0.0),
            monospace: appearance.monospace_font.clone().filter(|f| !f.is_empty()),
            line_width: appearance.line_width.filter(|w| *w > 0),
        }
    }
}

/// The desktop's document (or monospace) font: its family and size in
/// points.
pub fn desktop_font(document: bool) -> (String, f64) {
    let style = adw::StyleManager::default();
    let name = if document {
        style.document_font_name()
    } else {
        style.monospace_font_name()
    };
    let description = gtk::pango::FontDescription::from_string(&name);
    let family = description
        .family()
        .map(|f| f.to_string())
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| if document { "sans-serif" } else { "monospace" }.to_owned());
    let size = f64::from(description.size()) / f64::from(gtk::pango::SCALE);
    (family, if size > 0.0 { size } else { 11.0 })
}

/// `text` as a quoted CSS string.
fn css_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The mode the header bar's button switches a note to.
fn next_mode(mode: Mode) -> Mode {
    match mode {
        Mode::Live => Mode::Source,
        Mode::Source => Mode::Reading,
        Mode::Reading => Mode::Live,
    }
}

fn mode_from(mode: igneous_core::settings::EditorMode) -> Mode {
    match mode {
        igneous_core::settings::EditorMode::Live => Mode::Live,
        igneous_core::settings::EditorMode::Source => Mode::Source,
        igneous_core::settings::EditorMode::Reading => Mode::Reading,
    }
}

fn mode_to(mode: Mode) -> igneous_core::settings::EditorMode {
    match mode {
        Mode::Live => igneous_core::settings::EditorMode::Live,
        Mode::Source => igneous_core::settings::EditorMode::Source,
        Mode::Reading => igneous_core::settings::EditorMode::Reading,
    }
}

fn mention_range(mention: &igneous_index::Mention) -> std::ops::Range<usize> {
    mention.range.clone()
}
