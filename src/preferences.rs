//! The Preferences dialog: Appearance (the editor theme) and Sync (Git).

use std::cell::RefCell;

use adw::{prelude::*, subclass::prelude::*};
use gtk::{gio, glib};
use igneous_core::settings::{GitSettings, SyncMethod};
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
        prefs.add_sync_page(window);
        prefs.add(&crate::lint_prefs::page(window, prefs.upcast_ref()));
        prefs
    }

    // --- Sync ----------------------------------------------------------------

    fn add_sync_page(&self, window: &Window) {
        let page = adw::PreferencesPage::builder()
            .title("Sync")
            .icon_name("view-refresh-symbolic")
            .build();
        self.add(&page);
        let sync = window.sync().clone();
        if !sync.is_available() {
            let group = adw::PreferencesGroup::builder()
                .title("Git Sync")
                .description(
                    "This vault isn’t in a Git repository. To sync it, make it one \
                     (for example with “git init” and “git remote add”), then reopen the vault.",
                )
                .build();
            page.add(&group);
            return;
        }
        let settings = sync.settings();

        let schedule = adw::PreferencesGroup::builder()
            .title("Automatic Sync")
            .description("Saved with this vault, in .igneous/git.json")
            .build();
        let enabled = adw::SwitchRow::builder()
            .title("Sync Automatically")
            .subtitle("Commit, pull and push on a schedule")
            .active(settings.enabled)
            .build();
        let sync_every = minutes_row(
            "Commit and Sync Every",
            "Minutes; 0 turns it off",
            settings.sync_interval,
        );
        let pull_every = minutes_row(
            "Pull Every",
            "Minutes; 0 turns it off",
            settings.pull_interval,
        );
        let pull_on_open = adw::SwitchRow::builder()
            .title("Pull When the Vault Opens")
            .active(settings.pull_on_open)
            .build();
        for row in [&sync_every, &pull_every] {
            enabled
                .bind_property("active", row, "sensitive")
                .sync_create()
                .build();
        }
        enabled
            .bind_property("active", &pull_on_open, "sensitive")
            .sync_create()
            .build();
        schedule.add(&enabled);
        schedule.add(&sync_every);
        schedule.add(&pull_every);
        schedule.add(&pull_on_open);

        let how = adw::PreferencesGroup::builder().title("Syncing").build();
        let method = adw::ComboRow::builder()
            .title("Bring In Remote Changes By")
            .model(&gtk::StringList::new(&["Merging", "Rebasing"]))
            .selected(match settings.method {
                SyncMethod::Merge => 0,
                SyncMethod::Rebase => 1,
            })
            .build();
        let push = adw::SwitchRow::builder()
            .title("Push After Committing")
            .active(settings.push)
            .build();
        how.add(&method);
        how.add(&push);

        let messages = adw::PreferencesGroup::builder()
            .title("Commit Messages")
            .description(
                "{{date}}, {{hostname}}, {{numFiles}} and {{files}} are filled in. \
                 The date format uses Moment.js tokens, such as YYYY-MM-DD HH:mm.",
            )
            .build();
        let message = adw::EntryRow::builder()
            .title("Message")
            .text(&settings.commit_message)
            .show_apply_button(true)
            .build();
        let date_format = adw::EntryRow::builder()
            .title("Date Format")
            .text(&settings.date_format)
            .show_apply_button(true)
            .build();
        messages.add(&message);
        messages.add(&date_format);

        for group in [&schedule, &how, &messages] {
            page.add(group);
        }

        // Every change is saved straight away.
        let save = {
            let prefs = self.downgrade();
            let sync = sync.clone();
            let enabled = enabled.clone();
            let sync_every = sync_every.clone();
            let pull_every = pull_every.clone();
            let pull_on_open = pull_on_open.clone();
            let method = method.clone();
            let push = push.clone();
            let message = message.clone();
            let date_format = date_format.clone();
            std::rc::Rc::new(move || {
                let mut settings: GitSettings = sync.settings();
                settings.enabled = enabled.is_active();
                settings.sync_interval = sync_every.value() as u32;
                settings.pull_interval = pull_every.value() as u32;
                settings.pull_on_open = pull_on_open.is_active();
                settings.method = if method.selected() == 1 {
                    SyncMethod::Rebase
                } else {
                    SyncMethod::Merge
                };
                settings.push = push.is_active();
                let text = message.text();
                if !text.trim().is_empty() {
                    settings.commit_message = text.to_string();
                }
                let text = date_format.text();
                if !text.trim().is_empty() {
                    settings.date_format = text.to_string();
                }
                if settings == sync.settings() {
                    return;
                }
                if let (Err(e), Some(prefs)) = (sync.set_settings(settings), prefs.upgrade()) {
                    prefs.add_toast(adw::Toast::new(&e));
                }
            })
        };
        for row in [&enabled, &pull_on_open, &push] {
            let save = save.clone();
            row.connect_active_notify(move |_| save());
        }
        for row in [&sync_every, &pull_every] {
            let save = save.clone();
            row.connect_value_notify(move |_| save());
        }
        let on_method = save.clone();
        method.connect_selected_notify(move |_| on_method());
        for row in [&message, &date_format] {
            let save = save.clone();
            row.connect_apply(move |_| save());
        }
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

fn minutes_row(title: &str, subtitle: &str, value: u32) -> adw::SpinRow {
    let row = adw::SpinRow::with_range(0.0, 1440.0, 1.0);
    row.set_title(title);
    row.set_subtitle(subtitle);
    row.set_value(f64::from(value));
    row
}
