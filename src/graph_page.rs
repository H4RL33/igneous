//! The graph tab: the whole vault as a graph, with a popover for filters,
//! colour groups, display and forces. Its settings live in
//! `.igneous/graph.json`.

use std::cell::{Cell, OnceCell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use adw::{prelude::*, subclass::prelude::*};
use gtk::{gdk, glib};
use igneous_core::settings::{self as vault_settings, GraphGroup, GraphSettings};

use crate::graph_data::{GraphSource, build, forces};
use crate::graph_view::{Display, GraphView};

type SettingsFn = dyn Fn(&GraphSettings);
/// A switch in the popover: title, value, and how to store it.
type Toggle = (&'static str, bool, fn(&mut GraphSettings, bool));
/// A slider in the popover: title, range, value, and how to store it.
type Slider = (&'static str, f64, f64, f64, fn(&mut GraphSettings, f64));

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct GraphPage {
        pub view: OnceCell<GraphView>,
        pub igneous_dir: OnceCell<PathBuf>,
        pub settings: RefCell<GraphSettings>,
        /// graph.json couldn't be read: defaults are used and it's never
        /// overwritten.
        pub settings_error: RefCell<Option<String>>,
        pub source: RefCell<Option<Rc<GraphSource>>>,
        pub save_timer: RefCell<Option<glib::SourceId>>,
        pub filter_timer: RefCell<Option<glib::SourceId>>,
        pub problems: OnceCell<gtk::Label>,
        pub groups_box: RefCell<Option<gtk::Box>>,
        /// Set while the popover is being filled from the settings.
        pub loading: Cell<bool>,
        pub on_settings_changed: RefCell<Vec<Box<SettingsFn>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GraphPage {
        const NAME: &'static str = "IgneousGraphPage";
        type Type = super::GraphPage;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for GraphPage {
        fn dispose(&self) {
            if let Some(id) = self.filter_timer.take() {
                id.remove();
            }
            // Don't lose a change made just before the tab closed.
            self.obj().save_now();
        }
    }

    impl WidgetImpl for GraphPage {}
    impl BinImpl for GraphPage {}
}

glib::wrapper! {
    pub struct GraphPage(ObjectSubclass<imp::GraphPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl GraphPage {
    pub fn new(igneous_dir: PathBuf) -> Self {
        let page: Self = glib::Object::new();
        let imp = page.imp();
        let (settings, error) = match vault_settings::load::<GraphSettings>(&igneous_dir) {
            Ok(settings) => (settings, None),
            Err(e) => (GraphSettings::default(), Some(e.to_string())),
        };
        imp.settings.replace(settings);
        imp.settings_error.replace(error);
        imp.igneous_dir.set(igneous_dir).unwrap();
        page.build_ui();
        page
    }

    pub fn view(&self) -> &GraphView {
        self.imp().view.get().unwrap()
    }

    pub fn settings(&self) -> GraphSettings {
        self.imp().settings.borrow().clone()
    }

    /// Changes the settings, redraws, and saves them to graph.json soon.
    pub fn set_settings(&self, settings: GraphSettings) {
        let rebuild = {
            let old = self.imp().settings.borrow();
            old.filter != settings.filter
                || old.show_tags != settings.show_tags
                || old.show_attachments != settings.show_attachments
                || old.show_orphans != settings.show_orphans
                || old.show_unresolved != settings.show_unresolved
                || old
                    .groups
                    .iter()
                    .map(|g| &g.query)
                    .ne(settings.groups.iter().map(|g| &g.query))
        };
        self.imp().settings.replace(settings.clone());
        for callback in self.imp().on_settings_changed.borrow().iter() {
            callback(&settings);
        }
        if rebuild {
            self.rebuild();
        } else {
            self.apply_display();
        }
        self.schedule_save();
    }

    /// Called after the settings change (the local graph follows them).
    pub fn connect_settings_changed(&self, f: impl Fn(&GraphSettings) + 'static) {
        self.imp()
            .on_settings_changed
            .borrow_mut()
            .push(Box::new(f));
    }

    /// Writes graph.json now, if a change is waiting.
    pub fn save_now(&self) {
        if let Some(id) = self.imp().save_timer.take() {
            id.remove();
            self.save();
        }
    }

    /// The data to draw; called whenever the index changes.
    pub fn set_source(&self, source: Rc<GraphSource>) {
        self.imp().source.replace(Some(source));
        self.rebuild();
    }

    fn rebuild(&self) {
        let imp = self.imp();
        let Some(source) = imp.source.borrow().clone() else {
            return;
        };
        let settings = self.settings();
        let model = build(&source.notes, &source.edges, &settings);
        if let Some(problems) = imp.problems.get() {
            problems.set_label(&model.errors.join("\n"));
            problems.set_visible(!model.errors.is_empty());
        }
        self.view().set_model(model, forces(&settings));
        self.apply_display();
    }

    fn apply_display(&self) {
        let settings = self.settings();
        let view = self.view();
        view.set_forces(forces(&settings));
        view.set_display(Display {
            arrows: settings.arrows,
            text_fade: settings.text_fade as f32,
            node_size: settings.node_size as f32,
            link_thickness: settings.link_thickness as f32,
        });
        view.set_group_colors(
            settings
                .groups
                .iter()
                .map(|g| g.color.as_deref().and_then(|c| gdk::RGBA::parse(c).ok()))
                .collect(),
        );
    }

    fn schedule_save(&self) {
        let imp = self.imp();
        if let Some(id) = imp.save_timer.take() {
            id.remove();
        }
        let weak = self.downgrade();
        let id = glib::timeout_add_local_once(Duration::from_millis(500), move || {
            if let Some(page) = weak.upgrade() {
                page.imp().save_timer.take();
                page.save();
            }
        });
        imp.save_timer.replace(Some(id));
    }

    fn save(&self) {
        let imp = self.imp();
        if let Some(error) = imp.settings_error.borrow().as_ref() {
            tracing::warn!(%error, "graph.json can't be read, so it isn't overwritten");
            return;
        }
        let dir = imp.igneous_dir.get().unwrap();
        if let Err(e) = vault_settings::save(dir, &*imp.settings.borrow()) {
            tracing::warn!(%e, "couldn't save graph.json");
        }
    }

    /// Edits the settings with `f` (from the popover's controls).
    fn change(&self, f: impl FnOnce(&mut GraphSettings)) {
        if self.imp().loading.get() {
            return;
        }
        let mut settings = self.settings();
        f(&mut settings);
        self.set_settings(settings);
    }

    // --- the interface ------------------------------------------------------------------

    fn build_ui(&self) {
        let imp = self.imp();
        let view = GraphView::new();
        imp.view.set(view.clone()).unwrap();

        let fit = gtk::Button::builder()
            .icon_name("zoom-fit-best-symbolic")
            .tooltip_text("Fit to View")
            .css_classes(["osd", "circular"])
            .build();
        fit.connect_clicked(glib::clone!(
            #[weak]
            view,
            move |_| view.fit()
        ));
        let menu = gtk::MenuButton::builder()
            .icon_name("emblem-system-symbolic")
            .tooltip_text("Graph Settings")
            .css_classes(["osd", "circular"])
            .build();
        // Built as it opens, so it always shows the current settings.
        menu.set_create_popup_func(glib::clone!(
            #[weak(rename_to = page)]
            self,
            move |menu| menu.set_popover(Some(&page.settings_popover()))
        ));
        let buttons = gtk::Box::builder()
            .spacing(6)
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .margin_top(12)
            .margin_end(12)
            .build();
        buttons.append(&fit);
        buttons.append(&menu);

        let problems = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .valign(gtk::Align::End)
            .margin_start(12)
            .margin_bottom(12)
            .wrap(true)
            .visible(false)
            .css_classes(["error", "caption"])
            .build();
        imp.problems.set(problems.clone()).unwrap();

        let overlay = gtk::Overlay::builder().child(&view).build();
        overlay.add_overlay(&buttons);
        overlay.add_overlay(&problems);
        self.set_child(Some(&overlay));
        self.apply_display();
    }

    fn settings_popover(&self) -> gtk::Popover {
        let imp = self.imp();
        imp.loading.set(true);
        let settings = self.settings();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .width_request(320)
            .build();

        // Filters.
        let filter_group = adw::PreferencesGroup::builder()
            .title("Filter")
            .description("Only notes matching this search")
            .build();
        let filter = gtk::SearchEntry::builder()
            .placeholder_text("Search, e.g. tag:#project")
            .text(&settings.filter)
            .hexpand(true)
            .build();
        filter_group.add(&filter);
        content.append(&filter_group);
        let filters = adw::PreferencesGroup::builder().title("Show").build();
        filter.connect_search_changed(glib::clone!(
            #[weak(rename_to = page)]
            self,
            move |entry| {
                let imp = page.imp();
                if let Some(id) = imp.filter_timer.take() {
                    id.remove();
                }
                let text = entry.text().to_string();
                let weak = page.downgrade();
                let id = glib::timeout_add_local_once(Duration::from_millis(400), move || {
                    if let Some(page) = weak.upgrade() {
                        page.imp().filter_timer.take();
                        page.change(|s| s.filter = text);
                    }
                });
                imp.filter_timer.replace(Some(id));
            }
        ));
        let toggles: [Toggle; 4] = [
            ("Tags", settings.show_tags, |s, v| s.show_tags = v),
            ("Attachments", settings.show_attachments, |s, v| {
                s.show_attachments = v
            }),
            ("Unresolved Links", settings.show_unresolved, |s, v| {
                s.show_unresolved = v
            }),
            ("Orphans", settings.show_orphans, |s, v| s.show_orphans = v),
        ];
        for (title, active, set) in toggles {
            let row = adw::SwitchRow::builder()
                .title(title)
                .active(active)
                .build();
            row.connect_active_notify(glib::clone!(
                #[weak(rename_to = page)]
                self,
                move |row| {
                    let value = row.is_active();
                    page.change(|s| set(s, value));
                }
            ));
            filters.add(&row);
        }
        content.append(&filters);

        // Colour groups.
        let groups = adw::PreferencesGroup::builder()
            .title("Groups")
            .description("Notes matching a search take its colour")
            .build();
        let add = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("New Group")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        groups.set_header_suffix(Some(&add));
        let groups_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        groups.add(&groups_box);
        imp.groups_box.replace(Some(groups_box));
        add.connect_clicked(glib::clone!(
            #[weak(rename_to = page)]
            self,
            move |_| {
                page.change(|s| {
                    s.groups.push(GraphGroup {
                        query: String::new(),
                        color: None,
                    })
                });
                page.fill_groups();
            }
        ));
        content.append(&groups);
        self.fill_groups();

        // Display.
        let display = adw::PreferencesGroup::builder().title("Display").build();
        let arrows = adw::SwitchRow::builder()
            .title("Arrows")
            .active(settings.arrows)
            .build();
        arrows.connect_active_notify(glib::clone!(
            #[weak(rename_to = page)]
            self,
            move |row| {
                let value = row.is_active();
                page.change(|s| s.arrows = value);
            }
        ));
        display.add(&arrows);
        let sliders: [Slider; 3] = [
            ("Text Fade", -3.0, 3.0, settings.text_fade, |s, v| {
                s.text_fade = v
            }),
            ("Node Size", 0.1, 5.0, settings.node_size, |s, v| {
                s.node_size = v
            }),
            (
                "Link Thickness",
                0.1,
                5.0,
                settings.link_thickness,
                |s, v| s.link_thickness = v,
            ),
        ];
        for (title, min, max, value, set) in sliders {
            display.add(&self.slider(title, min, max, value, set));
        }
        content.append(&display);

        // Forces.
        let forces = adw::PreferencesGroup::builder().title("Forces").build();
        let sliders: [Slider; 4] = [
            ("Centre", 0.0, 1.0, settings.center_force, |s, v| {
                s.center_force = v
            }),
            ("Repel", 0.0, 20.0, settings.repel_force, |s, v| {
                s.repel_force = v
            }),
            ("Link Force", 0.0, 1.0, settings.link_force, |s, v| {
                s.link_force = v
            }),
            (
                "Link Distance",
                30.0,
                500.0,
                settings.link_distance,
                |s, v| s.link_distance = v,
            ),
        ];
        for (title, min, max, value, set) in sliders {
            forces.add(&self.slider(title, min, max, value, set));
        }
        content.append(&forces);

        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(560)
            .child(&content)
            .build();
        imp.loading.set(false);
        gtk::Popover::builder().child(&scrolled).build()
    }

    fn slider(
        &self,
        title: &str,
        min: f64,
        max: f64,
        value: f64,
        set: fn(&mut GraphSettings, f64),
    ) -> adw::ActionRow {
        let scale =
            gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, (max - min) / 100.0);
        scale.set_value(value);
        scale.set_width_request(150);
        scale.set_valign(gtk::Align::Center);
        scale.update_property(&[gtk::accessible::Property::Label(title)]);
        scale.connect_value_changed(glib::clone!(
            #[weak(rename_to = page)]
            self,
            move |scale| {
                let value = scale.value();
                page.change(|s| set(s, value));
            }
        ));
        let row = adw::ActionRow::builder().title(title).build();
        row.add_suffix(&scale);
        row
    }

    /// One row per colour group: its search, its colour, and a button to
    /// remove it.
    fn fill_groups(&self) {
        let imp = self.imp();
        let Some(container) = imp.groups_box.borrow().clone() else {
            return;
        };
        while let Some(child) = container.first_child() {
            container.remove(&child);
        }
        let settings = self.settings();
        // Filling the rows isn't a change to save.
        imp.loading.set(true);
        for (i, group) in settings.groups.iter().enumerate() {
            let entry = gtk::Entry::builder()
                .text(&group.query)
                .placeholder_text("Search")
                .hexpand(true)
                .build();
            entry.connect_changed(glib::clone!(
                #[weak(rename_to = page)]
                self,
                move |entry| {
                    let text = entry.text().to_string();
                    page.change(|s| {
                        if let Some(g) = s.groups.get_mut(i) {
                            g.query = text;
                        }
                    });
                }
            ));
            let colour = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::new()));
            colour.set_tooltip_text(Some("Colour"));
            colour.set_rgba(
                &group
                    .color
                    .as_deref()
                    .and_then(|c| gdk::RGBA::parse(c).ok())
                    .unwrap_or_else(|| self.view().group_colour(i)),
            );
            colour.connect_rgba_notify(glib::clone!(
                #[weak(rename_to = page)]
                self,
                move |button| {
                    let css = button.rgba().to_string();
                    page.change(|s| {
                        if let Some(g) = s.groups.get_mut(i) {
                            g.color = Some(css);
                        }
                    });
                }
            ));
            let remove = gtk::Button::builder()
                .icon_name("list-remove-symbolic")
                .tooltip_text("Remove Group")
                .css_classes(["flat"])
                .build();
            remove.connect_clicked(glib::clone!(
                #[weak(rename_to = page)]
                self,
                move |_| {
                    page.change(|s| {
                        if i < s.groups.len() {
                            s.groups.remove(i);
                        }
                    });
                    page.fill_groups();
                }
            ));
            let row = gtk::Box::builder().spacing(6).build();
            row.append(&entry);
            row.append(&colour);
            row.append(&remove);
            container.append(&row);
        }
        imp.loading.set(false);
    }
}
