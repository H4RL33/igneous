//! The Preferences dialog: Editor (the theme, fonts and typing), Files &
//! Links, and Plugins (daily notes, templates, Git sync and the linter).
//! Typing searches every setting.

use std::cell::{Cell, RefCell};

use adw::{prelude::*, subclass::prelude::*};
use gtk::{gio, glib};
use igneous_editor::theme::{Origin, Theme};

use crate::window::Window;

/// Themes shown before "Show All Themes" (two rows of three).
const FEW_THEMES: i32 = 6;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/dev/h4rl3y/igneous/preferences.ui")]
    pub struct Preferences {
        #[template_child]
        pub editor_page: TemplateChild<adw::PreferencesPage>,
        #[template_child]
        pub themes_box: TemplateChild<gtk::FlowBox>,
        #[template_child]
        pub show_all_themes: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub show_all_content: TemplateChild<adw::ButtonContent>,
        #[template_child]
        pub problems_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub problems_list: TemplateChild<gtk::ListBox>,
        pub window: glib::WeakRef<Window>,
        pub dark_handler: RefCell<Option<glib::SignalHandlerId>>,
        /// The theme cards' colours (see `fill_themes`).
        pub card_css: RefCell<Option<gtk::CssProvider>>,
        /// The id of the theme in use.
        pub current: RefCell<String>,
        pub theme_count: Cell<i32>,
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
            if let Some(css) = self.card_css.take() {
                gtk::style_context_remove_provider_for_display(
                    &WidgetExt::display(&*self.obj()),
                    &css,
                );
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
        let imp = prefs.imp();
        imp.window.set(Some(window));
        let dialog: &adw::PreferencesDialog = prefs.upcast_ref();

        // Editor: the theme first, then text and typing.
        window.reload_themes();
        prefs.set_up_theme_cards();
        prefs.fill_themes();
        let weak = prefs.downgrade();
        let handler = adw::StyleManager::default().connect_dark_notify(move |_| {
            if let Some(prefs) = weak.upgrade() {
                prefs.fill_themes();
            }
        });
        imp.dark_handler.replace(Some(handler));
        crate::editor_prefs::fill(&imp.editor_page, window, dialog);

        // The notes settings are split between Files & Links and Plugins.
        let notes = crate::notes_prefs::groups(window, dialog);
        prefs.add(&crate::files_prefs::page(
            window,
            dialog,
            &[&notes.opening, &notes.recovery],
        ));

        let plugins = adw::PreferencesPage::builder()
            .name("plugins")
            .title("Plugins")
            .icon_name("application-x-addon-symbolic")
            .build();
        plugins.add(&notes.daily);
        plugins.add(&notes.templates);
        plugins.add(&crate::sync_prefs::group(window, dialog));
        plugins.add(&crate::lint_prefs::group(window, dialog));
        prefs.add(&plugins);
        prefs
    }

    // --- Theme cards ---------------------------------------------------------

    fn set_up_theme_cards(&self) {
        let imp = self.imp();
        imp.themes_box.connect_child_activated(glib::clone!(
            #[weak(rename_to = prefs)]
            self,
            move |_, child| {
                let Some(window) = prefs.imp().window.upgrade() else {
                    return;
                };
                let id = child.widget_name();
                window.set_editor_theme(&id);
                prefs.mark_selected(&id);
            }
        ));
        imp.themes_box.set_filter_func(glib::clone!(
            #[weak(rename_to = prefs)]
            self,
            #[upgrade_or]
            true,
            move |child| {
                let imp = prefs.imp();
                imp.show_all_themes.is_active()
                    || child.index() < FEW_THEMES
                    || child.widget_name() == imp.current.borrow().as_str()
            }
        ));
        imp.show_all_themes.connect_active_notify(glib::clone!(
            #[weak(rename_to = prefs)]
            self,
            move |button| {
                let imp = prefs.imp();
                let all = button.is_active();
                imp.show_all_content.set_label(if all {
                    "Show Fewer Themes"
                } else {
                    "Show All Themes"
                });
                imp.show_all_content.set_icon_name(if all {
                    "pan-up-symbolic"
                } else {
                    "pan-down-symbolic"
                });
                imp.themes_box.invalidate_filter();
            }
        ));
    }

    /// One card per theme, in the variant for the current light or dark
    /// style, as Ptyxis shows its palettes: the theme's name and a sample
    /// in its colours, with a row of its accents.
    fn fill_themes(&self) {
        let imp = self.imp();
        let Some(window) = imp.window.upgrade() else {
            return;
        };
        imp.themes_box.remove_all();
        let catalog = window.themes();
        imp.current.replace(window.editor_theme_id());
        let dark = adw::StyleManager::default().is_dark();
        let accent = igneous_editor::system_accent();
        let family = window
            .text_style()
            .family
            .unwrap_or_else(|| crate::window::desktop_font(true).0);
        let mut css = String::new();
        for (i, theme) in catalog.themes().iter().enumerate() {
            css.push_str(&card_css(i, theme, dark, accent));
            imp.themes_box
                .append(&self.theme_card(i, theme, dark, &family));
        }
        // The cards' colours come from the themes, so they're CSS of their own.
        let provider = gtk::CssProvider::new();
        provider.load_from_string(&css);
        let display = WidgetExt::display(self);
        if let Some(old) = imp.card_css.replace(Some(provider.clone())) {
            gtk::style_context_remove_provider_for_display(&display, &old);
        }
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        let count = catalog.themes().len() as i32;
        imp.theme_count.set(count);
        imp.show_all_themes.set_visible(count > FEW_THEMES);
        let current = imp.current.borrow().clone();
        self.mark_selected(&current);

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

    fn theme_card(
        &self,
        index: usize,
        theme: &Theme,
        dark: bool,
        family: &str,
    ) -> gtk::FlowBoxChild {
        let name = theme.display_name(dark);
        let title = gtk::Label::builder()
            .label(&name)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(14)
            .css_classes(["theme-card-title"])
            .build();
        let sample = gtk::Label::builder()
            .label("The quick brown fox jumps over the lazy dog")
            .xalign(0.0)
            .yalign(0.0)
            .wrap(true)
            // FlowBox fits as many cards on a line as their natural widths
            // allow: keep them narrow enough for three.
            .width_chars(10)
            .max_width_chars(14)
            .lines(3)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["theme-card-sample"])
            .build();
        let attributes = gtk::pango::AttrList::new();
        attributes.insert(gtk::pango::AttrString::new_family(family));
        sample.set_attributes(Some(&attributes));
        let swatches = gtk::Box::builder()
            .spacing(4)
            .css_classes(["theme-card-swatches"])
            .build();
        for _ in SWATCHES {
            swatches.append(&gtk::Box::builder().css_classes(["theme-swatch"]).build());
        }
        let card = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .css_classes(["theme-card", &format!("theme-card-{index}")])
            .build();
        card.append(&title);
        card.append(&sample);
        card.append(&swatches);

        let check = gtk::Image::builder()
            .icon_name("object-select-symbolic")
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .margin_top(10)
            .margin_end(10)
            .css_classes(["theme-card-check"])
            .visible(false)
            .build();
        let overlay = gtk::Overlay::builder().child(&card).build();
        overlay.add_overlay(&check);

        let origin = match theme.origin {
            Origin::BuiltIn => "Built in",
            Origin::User => "From your themes folder",
            Origin::Vault => "From this vault",
        };
        let child = gtk::FlowBoxChild::builder()
            .child(&overlay)
            .name(&theme.id)
            .tooltip_text(format!("{name} · {origin}"))
            .build();
        child.update_property(&[gtk::accessible::Property::Label(&name)]);
        child
    }

    fn mark_selected(&self, id: &str) {
        let imp = self.imp();
        imp.current.replace(id.to_owned());
        let mut child = imp.themes_box.first_child();
        while let Some(widget) = child {
            if let Some(flow_child) = widget.downcast_ref::<gtk::FlowBoxChild>() {
                let selected = flow_child.widget_name() == id;
                if let Some(overlay) = flow_child.child().and_downcast::<gtk::Overlay>() {
                    if let Some(card) = overlay.child() {
                        if selected {
                            card.add_css_class("selected");
                        } else {
                            card.remove_css_class("selected");
                        }
                    }
                    if let Some(check) = overlay.last_child().filter(|c| c.is::<gtk::Image>()) {
                        check.set_visible(selected);
                    }
                }
                flow_child.update_state(&[gtk::accessible::State::Checked(if selected {
                    gtk::AccessibleTristate::True
                } else {
                    gtk::AccessibleTristate::False
                })]);
            }
            child = widget.next_sibling();
        }
        imp.themes_box.invalidate_filter();
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

/// The accents shown under each sample, in Ptyxis's order.
const SWATCHES: [&str; 6] = ["red", "green", "yellow", "blue", "purple", "cyan"];

/// A card's colours: the theme's background and text, and its accents.
fn card_css(
    index: usize,
    theme: &Theme,
    dark: bool,
    accent: igneous_editor::theme::Color,
) -> String {
    let variant = theme.variant(dark);
    let role = |name: &str| variant.role(name, accent).to_string();
    let mut css = format!(
        ".theme-card-{index} {{ background-color: {}; color: {}; }}\n",
        role("background"),
        role("text"),
    );
    for (n, name) in SWATCHES.iter().enumerate() {
        let colour = variant
            .palette(name)
            .map_or_else(|| role("text"), |c| c.to_string());
        css.push_str(&format!(
            ".theme-card-{index} .theme-swatch:nth-child({}) {{ background-color: {colour}; }}\n",
            n + 1
        ));
    }
    css
}
