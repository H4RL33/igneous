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

impl LocalGraph {
    pub fn new(on_activate: impl Fn(&VaultPath, bool) + 'static) -> Rc<Self> {
        let view = GraphView::new();
        view.set_size_request(-1, 280);
        view.connect_activate(on_activate);
        let depth = gtk::SpinButton::with_range(1.0, 3.0, 1.0);
        depth.set_value(1.0);
        depth.set_valign(gtk::Align::Center);
        depth.update_property(&[gtk::accessible::Property::Label("Depth")]);
        let header = adw::ActionRow::builder()
            .title("Depth")
            .tooltip_text("How many links away from the note to show")
            .build();
        header.add_suffix(&depth);
        let list = gtk::ListBox::builder()
            .css_classes(["boxed-list"])
            .selection_mode(gtk::SelectionMode::None)
            .margin_start(12)
            .margin_end(12)
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
        let weak = Rc::downgrade(&this);
        depth.connect_value_changed(move |spin| {
            if let Some(this) = weak.upgrade() {
                this.depth.set(spin.value() as u8);
                this.refresh();
            }
        });
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
