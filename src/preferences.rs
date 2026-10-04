//! The Preferences dialog. For now it has one page: Appearance, with the
//! editor theme.

use std::cell::RefCell;

use adw::{prelude::*, subclass::prelude::*};
use gtk::{gio, glib};
use igneous_editor::theme::Origin;

use crate::window::Window;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/dev/h4rl3y/igneous/preferences.ui")]
    pub struct Preferences {
        #[template_child]
        pub themes_box: TemplateChild<gtk::FlowBox>,
        #[template_child]
        pub problems_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub problems_list: TemplateChild<gtk::ListBox>,
        pub window: glib::WeakRef<Window>,
        pub dark_handler: RefCell<Option<glib::SignalHandlerId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Preferences {
        const NAME: &'static str = "IgneousPreferences";
        type Type = super::Preferences;
        type ParentType = adw::PreferencesDialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.install_action("prefs.open-themes-folder", None, |prefs, _, _| {
                prefs.open_themes_folder();
            });
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for Preferences {
        fn dispose(&self) {
            if let Some(handler) = self.dark_handler.take() {
                adw::StyleManager::default().disconnect(handler);
            }
        }
    }

    impl WidgetImpl for Preferences {}
    impl AdwDialogImpl for Preferences {}
    impl PreferencesDialogImpl for Preferences {}
}

glib::wrapper! {
    pub struct Preferences(ObjectSubclass<imp::Preferences>)
        @extends adw::PreferencesDialog, adw::Dialog, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::ShortcutManager;
}

impl Preferences {
    pub fn new(window: &Window) -> Self {
        let prefs: Self = glib::Object::new();
        prefs.imp().window.set(Some(window));
        // Pick up theme files added since the window opened.
        window.reload_themes();
        prefs.fill_themes();
        let weak = prefs.downgrade();
        let handler = adw::StyleManager::default().connect_dark_notify(move |_| {
            if let Some(prefs) = weak.upgrade() {
                prefs.fill_themes();
            }
        });
        prefs.imp().dark_handler.replace(Some(handler));
        prefs
    }

    /// One preview per theme, in the variant for the current light/dark style.
    fn fill_themes(&self) {
        let imp = self.imp();
        let Some(window) = imp.window.upgrade() else {
            return;
        };
        imp.themes_box.remove_all();
        let catalog = window.themes();
        let current = window.editor_theme_id();
        let dark = adw::StyleManager::default().is_dark();
        for theme in catalog.themes() {
            let Some(scheme) = igneous_editor::style_scheme(theme, dark) else {
                continue;
            };
            let preview = sourceview::StyleSchemePreview::new(&scheme);
            preview.set_widget_name(&theme.id);
            preview.set_selected(theme.id == current);
            let origin = match theme.origin {
                Origin::BuiltIn => "Built in",
                Origin::User => "From your themes folder",
                Origin::Vault => "From this vault",
            };
            preview.set_tooltip_text(Some(&format!("{} · {origin}", theme.display_name(dark))));
            let id = theme.id.clone();
            preview.connect_activate(glib::clone!(
                #[weak(rename_to = prefs)]
                self,
                #[weak]
                window,
                move |_| {
                    window.set_editor_theme(&id);
                    prefs.mark_selected(&id);
                }
            ));
            let label = gtk::Label::builder()
                .label(theme.display_name(dark))
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["caption"])
                .build();
            let tile = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(6)
                .build();
            tile.append(&preview);
            tile.append(&label);
            imp.themes_box.append(&tile);
        }

        imp.problems_list.remove_all();
        imp.problems_group.set_visible(!catalog.errors.is_empty());
        for (path, error) in &catalog.errors {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(
                    &path.file_name().unwrap_or_default().to_string_lossy(),
                ))
                .subtitle(glib::markup_escape_text(error))
                .build();
            imp.problems_list.append(&row);
        }
    }

    fn mark_selected(&self, id: &str) {
        let mut child = self.imp().themes_box.first_child();
        while let Some(flow_child) = child {
            if let Some(preview) = flow_child
                .downcast_ref::<gtk::FlowBoxChild>()
                .and_then(|c| c.child())
                .and_then(|tile| tile.first_child())
                .and_downcast::<sourceview::StyleSchemePreview>()
            {
                preview.set_selected(preview.widget_name() == id);
            }
            child = flow_child.next_sibling();
        }
    }

    fn open_themes_folder(&self) {
        let dir = Window::user_themes_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!(%e, "can't create the themes folder");
            return;
        }
        let root = self.root().and_downcast::<gtk::Window>();
        gtk::FileLauncher::new(Some(&gio::File::for_path(&dir))).launch(
            root.as_ref(),
            gio::Cancellable::NONE,
            |_| {},
        );
    }
}
