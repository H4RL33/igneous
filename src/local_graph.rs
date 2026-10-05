//! The inspector's local graph: the selected note and the notes within one
//! to three links of it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::gdk;
use igneous_core::VaultPath;
use igneous_core::settings::GraphSettings;
use igneous_graph::Forces;

use crate::graph_data::{GraphModel, GraphSource, build};
use crate::graph_view::{Display, GraphView};

pub struct LocalGraph {
    pub widget: gtk::Box,
    pub view: GraphView,
    depth: Cell<u8>,
    source: RefCell<Option<Rc<GraphSource>>>,
    settings: RefCell<GraphSettings>,
    /// The whole vault's graph, from which neighbourhoods are cut.
    full: RefCell<Option<GraphModel>>,
    path: RefCell<Option<VaultPath>>,
    empty: gtk::Label,
}

/// Small graphs read better with shorter links than the global graph's.
fn local_forces() -> Forces {
    Forces {
        link_distance: 110.0,
        ..Forces::default()
    }
}

/// How many links away from the note the graph can reach.
const MIN_DEPTH: i8 = 1;
const MAX_DEPTH: i8 = 3;

impl LocalGraph {
    pub fn new(on_activate: impl Fn(&VaultPath, bool) + 'static) -> Rc<Self> {
        let view = GraphView::new();
        view.set_size_request(-1, 280);
        view.connect_activate(on_activate);
        // − 1 +: the buttons either side of the number.
        let fewer = gtk::Button::builder()
            .icon_name("list-remove-symbolic")
            .tooltip_text("Fewer Links Away")
            .valign(gtk::Align::Center)
            .sensitive(false)
            .css_classes(["flat", "circular"])
            .build();
        let level = gtk::Label::builder()
            .label("1")
            .width_chars(2)
            .css_classes(["numeric"])
            .build();
        let more = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("More Links Away")
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular"])
            .build();
        let stepper = gtk::Box::builder()
            .spacing(4)
            .valign(gtk::Align::Center)
            .build();
        stepper.append(&fewer);
        stepper.append(&level);
        stepper.append(&more);
        let header = adw::ActionRow::builder()
            .title("Depth")
            .tooltip_text("How many links away from the note to show")
            .build();
        header.add_suffix(&stepper);
        let list = gtk::ListBox::builder()
            .css_classes(["boxed-list"])
            .selection_mode(gtk::SelectionMode::None)
            // Level with the page switcher above.
            .margin_start(6)
            .margin_end(6)
            .margin_top(6)
            .build();
        list.append(&header);
        let empty = gtk::Label::builder()
            .label("Open a note to see its neighbourhood.")
            .wrap(true)
            .margin_start(12)
            .margin_end(12)
            .css_classes(["dim-label"])
            .build();
        let widget = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        widget.append(&list);
        widget.append(&empty);
        widget.append(&view);
        let this = Rc::new(Self {
            widget,
            view,
            depth: Cell::new(1),
            source: RefCell::default(),
            settings: RefCell::default(),
            full: RefCell::default(),
            path: RefCell::default(),
            empty,
        });
        let step = {
            let weak = Rc::downgrade(&this);
            let (fewer, more) = (fewer.clone(), more.clone());
            move |by: i8| {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                let depth = (this.depth.get() as i8 + by).clamp(MIN_DEPTH, MAX_DEPTH);
                this.depth.set(depth as u8);
                level.set_label(&depth.to_string());
                fewer.set_sensitive(depth > MIN_DEPTH);
                more.set_sensitive(depth < MAX_DEPTH);
                this.refresh();
            }
        };
        let step = Rc::new(step);
        let down = step.clone();
        fewer.connect_clicked(move |_| down(-1));
        more.connect_clicked(move |_| step(1));
        this
    }

    /// New data from the index.
    pub fn set_source(&self, source: Rc<GraphSource>, settings: &GraphSettings) {
        self.source.replace(Some(source));
        self.set_settings(settings);
    }

    /// The vault's graph settings changed. Colour groups and what's shown
    /// follow them; the filter doesn't, so a note's neighbours always show.
    pub fn set_settings(&self, settings: &GraphSettings) {
        let settings = GraphSettings {
            filter: String::new(),
            show_orphans: true,
            show_unresolved: true,
            ..settings.clone()
        };
        let full = self
            .source
            .borrow()
            .as_ref()
            .map(|source| build(&source.notes, &source.edges, &settings));
        self.view.set_display(Display {
            arrows: settings.arrows,
            text_fade: settings.text_fade as f32,
            node_size: settings.node_size as f32,
            link_thickness: settings.link_thickness as f32,
        });
        self.view.set_group_colors(
            settings
                .groups
                .iter()
                .map(|g| g.color.as_deref().and_then(|c| gdk::RGBA::parse(c).ok()))
                .collect(),
        );
        self.settings.replace(settings);
        self.full.replace(full);
        self.refresh();
    }

    pub fn has_source(&self) -> bool {
        self.full.borrow().is_some()
    }

    /// Forgets its data, which changed while it was hidden, so it's
    /// fetched again when next shown.
    pub fn invalidate(&self) {
        self.source.replace(None);
        self.full.replace(None);
    }

    /// Centres the graph on `path`.
    pub fn show(&self, path: Option<VaultPath>) {
        if *self.path.borrow() == path {
            return;
        }
        self.path.replace(path);
        self.refresh();
    }

    fn refresh(&self) {
        let path = self.path.borrow().clone();
        let local = path.as_ref().and_then(|p| {
            self.full
                .borrow()
                .as_ref()
                .and_then(|full| full.local(p, self.depth.get()))
        });
        self.empty.set_visible(local.is_none());
        self.view.set_visible(local.is_some());
        self.view
            .set_model(local.unwrap_or_default(), local_forces());
        self.view.set_focus_path(path.as_ref());
    }
}
