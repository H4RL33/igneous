//! The window shown when no vault is open: open a folder or a recent vault.

use std::path::{Path, PathBuf};

use adw::{prelude::*, subclass::prelude::*};
use gtk::{gio, glib};

use crate::application::Application;
use crate::{config, gsettings};

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/dev/h4rl3y/igneous/vault-picker.ui")]
    pub struct VaultPicker {
        #[template_child]
        pub toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub status_page: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub recent_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub recent_list: TemplateChild<gtk::ListBox>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for VaultPicker {
        const NAME: &'static str = "IgneousVaultPicker";
        type Type = super::VaultPicker;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.install_action("picker.open-folder", None, |picker, _, _| {
                picker.choose_folder();
            });
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for VaultPicker {
        fn constructed(&self) {
            self.parent_constructed();
            self.status_page.set_icon_name(Some(config::APP_ID));
            if config::PROFILE == "Devel" {
                self.obj().add_css_class("devel");
            }
        }
    }

    impl WidgetImpl for VaultPicker {}
    impl WindowImpl for VaultPicker {}
    impl ApplicationWindowImpl for VaultPicker {}
    impl AdwApplicationWindowImpl for VaultPicker {}
}

glib::wrapper! {
    pub struct VaultPicker(ObjectSubclass<imp::VaultPicker>)
        @extends adw::ApplicationWindow, gtk::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable,
                    gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl VaultPicker {
    pub fn new(app: &Application) -> Self {
        let picker: Self = glib::Object::builder().property("application", app).build();
        picker.fill_recent();
        picker
    }

    fn app(&self) -> Application {
        self.application().and_downcast::<Application>().unwrap()
    }

    pub fn toast(&self, message: &str) {
        self.imp().toast_overlay.add_toast(adw::Toast::new(message));
    }

    fn fill_recent(&self) {
        let imp = self.imp();
        imp.recent_list.remove_all();
        let recent = gsettings::recent_vaults(self.app().settings());
        imp.recent_group.set_visible(!recent.is_empty());
        for root in recent {
            let path = PathBuf::from(&root);
            let exists = path.is_dir();
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(
                    &path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or(root.clone()),
                ))
                .subtitle(glib::markup_escape_text(&if exists {
                    home_relative(&path)
                } else {
                    format!("Folder not found: {}", home_relative(&path))
                }))
                .activatable(exists)
                .build();
            row.set_sensitive(true);
            let remove = gtk::Button::builder()
                .icon_name("window-close-symbolic")
                .tooltip_text("Remove From List")
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            let root_for_remove = root.clone();
            remove.connect_clicked(glib::clone!(
                #[weak(rename_to = picker)]
                self,
                move |_| {
                    gsettings::remove_recent_vault(picker.app().settings(), &root_for_remove);
                    picker.fill_recent();
                }
            ));
            row.add_suffix(&remove);
            if exists {
                row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            }
            row.connect_activated(glib::clone!(
                #[weak(rename_to = picker)]
                self,
                move |_| picker.open(&path)
            ));
            imp.recent_list.append(&row);
        }
    }

    fn choose_folder(&self) {
        let dialog = gtk::FileDialog::builder()
            .title("Open a Vault")
            .accept_label("_Open")
            .modal(true)
            .build();
        dialog.select_folder(
            Some(self),
            gio::Cancellable::NONE,
            glib::clone!(
                #[weak(rename_to = picker)]
                self,
                move |result| {
                    if let Ok(folder) = result
                        && let Some(path) = folder.path()
                    {
                        picker.open(&path);
                    }
                }
            ),
        );
    }

    fn open(&self, path: &Path) {
        match self.app().open_vault(path) {
            Ok(_) => self.close(),
            Err(e) => self.toast(&format!("Couldn't open the vault: {e}")),
        }
    }
}

/// Shows paths under the home folder as `~/…`.
pub fn home_relative(path: &Path) -> String {
    let home = glib::home_dir();
    match path.strip_prefix(&home) {
        Ok(rest) if !rest.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}
