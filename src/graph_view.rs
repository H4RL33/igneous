//! The graph widget: notes as circles, links as lines, drawn with GskPath.
//!
//! The layout (igneous-graph's force simulation) runs on a thread of its own
//! and publishes positions after each step; a tick callback picks them up
//! and redraws. Once the layout settles the thread blocks and the tick
//! callback removes itself, so an idle graph uses no CPU.
//!
//! - Hovering a node highlights it and its neighbours in the accent colour.
//! - Clicking a node opens its note; Ctrl+click or middle-click opens it in a
//!   new tab.
//! - Dragging a node pins it while dragged; dragging the background pans.
//! - Ctrl+scroll and pinch zoom; scrolling pans.
//!
//! Screen readers get a summary label; the inspector's link lists are the
//! accessible way to follow the same links.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::{gdk, glib, graphene, gsk, pango, subclass::prelude::*};
use igneous_core::VaultPath;
use igneous_graph::{Forces, Graph, NodeKind, Simulation, Vec2};

use crate::graph_data::GraphModel;

/// How long the layout thread waits between steps, so the layout animates
/// instead of jumping to its end.
const STEP: Duration = Duration::from_millis(8);
/// Pointer travel (pixels) below which a press and release is a click.
const CLICK_SLOP: f64 = 4.0;
/// Labels drawn at most per frame, biggest nodes first.
const MAX_LABELS: usize = 400;

/// How the graph is drawn (the "Display" part of the graph settings).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Display {
    pub arrows: bool,
    /// Obsidian's "text fade threshold", -3 to 3: higher shows labels
    /// sooner when zoomed out.
    pub text_fade: f32,
    pub node_size: f32,
    pub link_thickness: f32,
}

impl Default for Display {
    fn default() -> Self {
        Self {
            arrows: false,
            text_fade: 0.0,
            node_size: 1.0,
            link_thickness: 1.0,
        }
    }
}

// --- the layout thread --------------------------------------------------------------

enum Command {
    Update(Graph),
    Forces(Forces),
    Pin(usize, Vec2),
    Unpin(usize),
    AlphaTarget(f32),
}

#[derive(Default)]
struct Published {
    positions: Vec<Vec2>,
    generation: u64,
    settled: bool,
}

struct Layout {
    commands: mpsc::Sender<Command>,
    published: Arc<Mutex<Published>>,
}

impl Layout {
    fn start(graph: Graph, forces: Forces) -> Self {
        let (commands, receiver) = mpsc::channel::<Command>();
        let published = Arc::new(Mutex::new(Published::default()));
        let shared = published.clone();
        std::thread::Builder::new()
            .name("igneous-graph".into())
            .spawn(move || run_layout(graph, forces, &receiver, &shared))
            .expect("can start the graph layout thread");
        Self {
            commands,
            published,
        }
    }

    fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

fn run_layout(
    graph: Graph,
    forces: Forces,
    receiver: &mpsc::Receiver<Command>,
    published: &Mutex<Published>,
) {
    let mut sim = Simulation::new(&graph, forces, 0x1915);
    let publish = |sim: &Simulation| {
        let mut p = published.lock().unwrap();
        p.positions.clear();
        p.positions.extend_from_slice(sim.positions());
        p.generation += 1;
        p.settled = sim.is_settled();
    };
    publish(&sim);
    loop {
        // Settled: sleep until told to do something. Otherwise take whatever
        // has arrived and carry on.
        let first = if sim.is_settled() {
            match receiver.recv() {
                Ok(command) => Some(command),
                Err(_) => return,
            }
        } else {
            match receiver.try_recv() {
                Ok(command) => Some(command),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        };
        let mut command = first;
        while let Some(c) = command {
            match c {
                Command::Update(graph) => sim.update(&graph),
                Command::Forces(forces) => sim.set_forces(forces),
                Command::Pin(i, at) if i < sim.len() => sim.pin(i, at),
                Command::Unpin(i) if i < sim.len() => sim.unpin(i),
                Command::AlphaTarget(t) => sim.set_alpha_target(t),
                Command::Pin(..) | Command::Unpin(_) => {}
            }
            command = receiver.try_recv().ok();
        }
        let started = Instant::now();
        sim.tick();
        publish(&sim);
        if let Some(rest) = STEP.checked_sub(started.elapsed()) {
            std::thread::sleep(rest);
        }
    }
}

// --- the widget ----------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum Drag {
    Node { index: usize, was_pinned: bool },
    Pan { from: (f32, f32) },
}

type ActivateFn = dyn Fn(&VaultPath, bool);

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct GraphView {
        pub(super) model: RefCell<GraphModel>,
        pub(super) degrees: RefCell<Vec<u32>>,
        pub(super) adjacency: RefCell<Vec<Vec<u32>>>,
        pub(super) positions: RefCell<Vec<Vec2>>,
        pub(super) generation: Cell<u64>,
        pub(super) settled: Cell<bool>,
        pub(super) layout: RefCell<Option<Layout>>,
        pub(super) ticking: Cell<bool>,
        /// Screen pixels per layout unit.
        pub(super) scale: Cell<f32>,
        /// Where the layout's origin is, relative to the widget's centre.
        pub(super) pan: Cell<(f32, f32)>,
        /// Set once the user zooms or pans; until then the view follows the
        /// layout as it spreads out.
        pub(super) user_moved: Cell<bool>,
        pub(super) hovered: Cell<Option<usize>>,
        /// The node to emphasise, such as the local graph's note.
        pub(super) focus: Cell<Option<usize>>,
        pub(super) drag: Cell<Option<Drag>>,
        pub(super) pinned: RefCell<HashSet<usize>>,
        pub(super) pointer: Cell<(f64, f64)>,
        pub(super) zoom_start: Cell<f32>,
        pub(super) display: Cell<Display>,
        pub(super) forces: Cell<Option<Forces>>,
        pub(super) group_colors: RefCell<Vec<Option<gdk::RGBA>>>,
        pub(super) labels: RefCell<Vec<Option<pango::Layout>>>,
        pub(super) on_activate: RefCell<Option<Rc<ActivateFn>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GraphView {
        const NAME: &'static str = "IgneousGraphView";
        type Type = super::GraphView;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("graphview");
            klass.set_accessible_role(gtk::AccessibleRole::Img);
        }
    }

    impl ObjectImpl for GraphView {
        fn constructed(&self) {
            self.parent_constructed();
            self.scale.set(1.0);
            let view = self.obj();
            view.set_hexpand(true);
            view.set_vexpand(true);
            view.set_focusable(true);
            view.connect_controllers();
            let weak = view.downgrade();
            adw::StyleManager::default().connect_accent_color_notify(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.queue_draw();
                }
            });
        }
    }

    impl WidgetImpl for GraphView {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let _ = orientation;
            (100, 300, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            self.obj().draw(snapshot);
        }
    }
}

glib::wrapper! {
    pub struct GraphView(ObjectSubclass<imp::GraphView>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for GraphView {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphView {
    pub fn new() -> Self {
        glib::Object::new()
    }

    /// Shows `model`. Nodes already shown keep their place.
    pub fn set_model(&self, model: GraphModel, forces: Forces) {
        let imp = self.imp();
        let graph = model.graph.clone();
        imp.degrees.replace(graph.degrees());
        imp.adjacency.replace(graph.adjacency());
        imp.labels.replace(vec![None; graph.len()]);
        imp.pinned.borrow_mut().clear();
        imp.hovered.set(None);
        let notes = model
            .graph
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Note)
            .count();
        imp.model.replace(model);
        self.update_property(&[gtk::accessible::Property::Label(&format!(
            "Graph of {notes} notes and {} links",
            graph.undirected_edges().len()
        ))]);
        let running = imp.layout.borrow().is_some();
        if running {
            if let Some(layout) = imp.layout.borrow().as_ref() {
                if imp.forces.get() != Some(forces) {
                    layout.send(Command::Forces(forces));
                }
                layout.send(Command::Update(graph));
            }
        } else {
            imp.positions.replace(Vec::new());
            imp.layout.replace(Some(Layout::start(graph, forces)));
        }
        imp.forces.set(Some(forces));
        imp.settled.set(false);
        self.start_ticking();
        self.queue_draw();
    }

    pub fn model(&self) -> GraphModel {
        self.imp().model.borrow().clone()
    }

    pub fn set_forces(&self, forces: Forces) {
        let imp = self.imp();
        if imp.forces.get() == Some(forces) {
            return;
        }
        imp.forces.set(Some(forces));
        if let Some(layout) = imp.layout.borrow().as_ref() {
            layout.send(Command::Forces(forces));
        }
        imp.settled.set(false);
        self.start_ticking();
    }

    pub fn set_display(&self, display: Display) {
        self.imp().display.set(display);
        self.queue_draw();
    }

    /// Colours for the colour groups, in order; `None` picks one derived from
    /// the accent colour.
    pub fn set_group_colors(&self, colors: Vec<Option<gdk::RGBA>>) {
        self.imp().group_colors.replace(colors);
        self.queue_draw();
    }

    /// Emphasises the node for `path` (the local graph's note).
    pub fn set_focus_path(&self, path: Option<&VaultPath>) {
        let focus = path.and_then(|p| self.imp().model.borrow().find(p));
        self.imp().focus.set(focus);
        self.queue_draw();
    }

    /// Called with a node's file when it's clicked, and whether to open it in
    /// a new tab.
    pub fn connect_activate(&self, f: impl Fn(&VaultPath, bool) + 'static) {
        self.imp().on_activate.replace(Some(Rc::new(f)));
    }

    /// Opens the note behind node `index`, as a click would.
    pub fn activate_node(&self, index: usize, new_tab: bool) {
        let path = self
            .imp()
            .model
            .borrow()
            .paths
            .get(index)
            .cloned()
            .flatten();
        let callback = self.imp().on_activate.borrow().clone();
        if let (Some(path), Some(callback)) = (path, callback) {
            callback(&path, new_tab);
        }
    }

    /// Whether the layout has come to rest.
    pub fn is_settled(&self) -> bool {
        self.imp().settled.get()
    }

    /// Whether the widget is still doing per-frame work.
    pub fn is_ticking(&self) -> bool {
        self.imp().ticking.get()
    }

    /// Fits the whole graph in view.
    pub fn fit(&self) {
        let imp = self.imp();
        imp.user_moved.set(false);
        self.fit_now();
        self.queue_draw();
    }

    fn fit_now(&self) {
        let imp = self.imp();
        let positions = imp.positions.borrow();
        if positions.is_empty() || self.width() <= 0 {
            return;
        }
        let (mut min, mut max) = (positions[0], positions[0]);
        for p in positions.iter() {
            min.x = min.x.min(p.x);
            min.y = min.y.min(p.y);
            max.x = max.x.max(p.x);
            max.y = max.y.max(p.y);
        }
        let margin = 40.0;
        let (w, h) = (
            self.width() as f32 - 2.0 * margin,
            self.height() as f32 - 2.0 * margin,
        );
        let span = ((max.x - min.x) / w.max(1.0)).max((max.y - min.y) / h.max(1.0));
        let scale = if span > 0.0 {
            (1.0 / span).clamp(0.05, 2.0)
        } else {
            1.0
        };
        let centre = Vec2::new((min.x + max.x) / 2.0, (min.y + max.y) / 2.0);
        imp.scale.set(scale);
        imp.pan.set((-centre.x * scale, -centre.y * scale));
    }

    // --- coordinates -------------------------------------------------------------

    fn to_screen(&self, p: Vec2) -> (f32, f32) {
        let imp = self.imp();
        let (px, py) = imp.pan.get();
        let s = imp.scale.get();
        (
            self.width() as f32 / 2.0 + px + p.x * s,
            self.height() as f32 / 2.0 + py + p.y * s,
        )
    }

    fn to_layout(&self, x: f64, y: f64) -> Vec2 {
        let imp = self.imp();
        let (px, py) = imp.pan.get();
        let s = imp.scale.get();
        Vec2::new(
            (x as f32 - self.width() as f32 / 2.0 - px) / s,
            (y as f32 - self.height() as f32 / 2.0 - py) / s,
        )
    }

    fn radius(&self, index: usize) -> f32 {
        let imp = self.imp();
        let degree = imp.degrees.borrow().get(index).copied().unwrap_or(0) as f32;
        let size = imp.display.get().node_size;
        ((4.0 + degree.sqrt() * 1.8) * size * imp.scale.get().sqrt().clamp(0.6, 2.5)).max(3.0)
    }

    /// The node under widget coordinates `(x, y)`.
    pub fn node_at(&self, x: f64, y: f64) -> Option<usize> {
        let positions = self.imp().positions.borrow();
        let mut best: Option<(usize, f32)> = None;
        for (i, p) in positions.iter().enumerate() {
            let (sx, sy) = self.to_screen(*p);
            let d = ((sx - x as f32).powi(2) + (sy - y as f32).powi(2)).sqrt();
            let reach = self.radius(i).max(6.0) + 2.0;
            if d <= reach && best.is_none_or(|(_, b)| d < b) {
                best = Some((i, d));
            }
        }
        best.map(|(i, _)| i)
    }

    /// Where node `index` is drawn, in widget coordinates.
    pub fn node_position(&self, index: usize) -> Option<(f32, f32)> {
        let positions = self.imp().positions.borrow();
        positions.get(index).map(|p| self.to_screen(*p))
    }

    // --- per-frame work -------------------------------------------------------------

    fn start_ticking(&self) {
        let imp = self.imp();
        if imp.ticking.replace(true) {
            return;
        }
        self.add_tick_callback(|view, _| {
            let view = view.downcast_ref::<GraphView>().unwrap();
            if view.poll_layout() {
                glib::ControlFlow::Continue
            } else {
                view.imp().ticking.set(false);
                glib::ControlFlow::Break
            }
        });
    }

    /// Takes the latest positions. Returns whether to keep polling.
    fn poll_layout(&self) -> bool {
        let imp = self.imp();
        let Some(published) = imp.layout.borrow().as_ref().map(|l| l.published.clone()) else {
            return false;
        };
        let (changed, settled) = {
            let p = published.lock().unwrap();
            let changed = p.generation != imp.generation.get();
            if changed {
                imp.generation.set(p.generation);
                let mut positions = imp.positions.borrow_mut();
                positions.clear();
                positions.extend_from_slice(&p.positions);
            }
            (changed, p.settled)
        };
        if changed {
            if !imp.user_moved.get() {
                self.fit_now();
            }
            self.queue_draw();
        }
        imp.settled.set(settled && !changed);
        let dragging = matches!(imp.drag.get(), Some(Drag::Node { .. }));
        !(settled && !changed) || dragging
    }

    // --- drawing ------------------------------------------------------------------------

    /// The accent colour with its hue turned `by` (0 to 1): colours that
    /// suit the desktop's accent without being hard-coded.
    fn turned_accent(by: f32) -> gdk::RGBA {
        let accent = adw::StyleManager::default().accent_color_rgba();
        let (h, s, v) = gtk::rgb_to_hsv(accent.red(), accent.green(), accent.blue());
        let (r, g, b) = gtk::hsv_to_rgb((h + by).rem_euclid(1.0), s.max(0.45), v.max(0.6));
        gdk::RGBA::new(r, g, b, 1.0)
    }

    /// The colour of colour group `index`: its own, or one derived from the
    /// accent colour.
    pub fn group_colour(&self, index: usize) -> gdk::RGBA {
        self.imp()
            .group_colors
            .borrow()
            .get(index)
            .copied()
            .flatten()
            .unwrap_or_else(|| Self::turned_accent(0.13 + index as f32 * 0.17))
    }

    fn colours(&self) -> Palette {
        let fg = self.color();
        let accent = adw::StyleManager::default().accent_color_rgba();
        let turned = Self::turned_accent;
        let groups = (0..self.imp().group_colors.borrow().len())
            .map(|i| self.group_colour(i))
            .collect();
        Palette {
            note: with_alpha(&fg, 0.62),
            attachment: turned(0.5),
            tag: turned(0.33),
            unresolved: with_alpha(&fg, 0.28),
            edge: with_alpha(&fg, 0.22),
            label: fg,
            accent,
            groups,
        }
    }

    fn label_alpha(&self) -> f32 {
        let imp = self.imp();
        let threshold = 2f32.powf(-imp.display.get().text_fade);
        let (start, end) = (0.55 * threshold, 0.95 * threshold);
        ((imp.scale.get() - start) / (end - start)).clamp(0.0, 1.0)
    }

    /// Draws one frame into `snapshot`. For timing tests.
    #[doc(hidden)]
    pub fn draw_into(&self, snapshot: &gtk::Snapshot) {
        self.draw(snapshot);
    }

    fn draw(&self, snapshot: &gtk::Snapshot) {
        let imp = self.imp();
        let positions = imp.positions.borrow();
        let model = imp.model.borrow();
        let n = positions.len().min(model.graph.len());
        if n == 0 {
            return;
        }
        let palette = self.colours();
        let display = imp.display.get();
        let (w, h) = (self.width() as f32, self.height() as f32);
        let screen: Vec<(f32, f32)> = positions[..n].iter().map(|p| self.to_screen(*p)).collect();
        let visible =
            |&(x, y): &(f32, f32), r: f32| x + r >= 0.0 && x - r <= w && y + r >= 0.0 && y - r <= h;
        let hovered = imp.hovered.get().filter(|&i| i < n);
        let adjacency = imp.adjacency.borrow();
        let near: HashSet<usize> = hovered
            .map(|i| {
                adjacency[i]
                    .iter()
                    .map(|&j| j as usize)
                    .chain(std::iter::once(i))
                    .collect()
            })
            .unwrap_or_default();
        let dim = |c: &gdk::RGBA, i: usize| {
            if hovered.is_some() && !near.contains(&i) {
                with_alpha(c, c.alpha() * 0.35)
            } else {
                *c
            }
        };

        // Links. Each is a thin rotated rectangle rather than one big stroked
        // path: GSK fills those cheaply, while a stroke spanning the whole
        // view costs a mask the size of the view (over a second for 20,000
        // links when rendered in software).
        let width = display.link_thickness * imp.scale.get().sqrt().clamp(0.5, 2.0);
        let mut plain = Vec::new();
        let mut lit = Vec::new();
        let arrows = gsk::PathBuilder::new();
        for &(a, b) in &model.graph.edges {
            let (a, b) = (a as usize, b as usize);
            if a >= n || b >= n || a == b {
                continue;
            }
            let (pa, pb) = (screen[a], screen[b]);
            // Both ends off the same side: the line can't cross the view.
            if (pa.0 < 0.0 && pb.0 < 0.0)
                || (pa.0 > w && pb.0 > w)
                || (pa.1 < 0.0 && pb.1 < 0.0)
                || (pa.1 > h && pb.1 > h)
            {
                continue;
            }
            if hovered.is_some_and(|i| i == a || i == b) {
                lit.push((pa, pb));
            } else {
                plain.push((pa, pb));
            }
            if display.arrows {
                let (dx, dy) = (pb.0 - pa.0, pb.1 - pa.1);
                let len = (dx * dx + dy * dy).sqrt();
                if len > 1.0 {
                    let (ux, uy) = (dx / len, dy / len);
                    let tip = (pb.0 - ux * self.radius(b), pb.1 - uy * self.radius(b));
                    let size = 4.0 + width * 2.0;
                    arrows.move_to(tip.0, tip.1);
                    arrows.line_to(
                        tip.0 - ux * size - uy * size * 0.5,
                        tip.1 - uy * size + ux * size * 0.5,
                    );
                    arrows.line_to(
                        tip.0 - ux * size + uy * size * 0.5,
                        tip.1 - uy * size - ux * size * 0.5,
                    );
                    arrows.close();
                }
            }
        }
        let edge_colour = if hovered.is_some() {
            with_alpha(&palette.edge, palette.edge.alpha() * 0.5)
        } else {
            palette.edge
        };
        draw_lines(snapshot, &plain, width, &edge_colour);
        draw_lines(snapshot, &lit, width * 1.6, &palette.accent);
        if display.arrows {
            snapshot.append_fill(&arrows.to_path(), gsk::FillRule::Winding, &edge_colour);
        }

        // Nodes, batched by colour.
        let mut batches: Vec<(gdk::RGBA, gsk::PathBuilder)> = Vec::new();
        let focus = imp.focus.get();
        for (i, node) in model.graph.nodes.iter().take(n).enumerate() {
            let r = self.radius(i);
            if !visible(&screen[i], r) {
                continue;
            }
            let base = if hovered == Some(i) || focus == Some(i) {
                palette.accent
            } else if let Some(g) = node.group.and_then(|g| palette.groups.get(g as usize)) {
                *g
            } else {
                match node.kind {
                    NodeKind::Note => palette.note,
                    NodeKind::Attachment => palette.attachment,
                    NodeKind::Tag => palette.tag,
                    NodeKind::Unresolved => palette.unresolved,
                }
            };
            let colour = dim(&base, i);
            let builder = match batches.iter().position(|(c, _)| *c == colour) {
                Some(found) => &batches[found].1,
                None => {
                    batches.push((colour, gsk::PathBuilder::new()));
                    &batches.last().unwrap().1
                }
            };
            builder.add_circle(&graphene::Point::new(screen[i].0, screen[i].1), r);
        }
        for (colour, builder) in batches {
            snapshot.append_fill(&builder.to_path(), gsk::FillRule::Winding, &colour);
        }

        // Labels: faded in with zoom, always shown around the hovered node.
        let fade = self.label_alpha();
        let degrees = imp.degrees.borrow();
        let mut labelled: Vec<usize> = (0..n)
            .filter(|&i| {
                visible(&screen[i], 0.0) && (fade > 0.01 || near.contains(&i) || focus == Some(i))
            })
            .collect();
        labelled.sort_by_key(|&i| std::cmp::Reverse(degrees.get(i).copied().unwrap_or(0)));
        labelled.truncate(MAX_LABELS);
        let mut layouts = imp.labels.borrow_mut();
        for i in labelled {
            let alpha = if near.contains(&i) || focus == Some(i) {
                1.0
            } else if hovered.is_some() {
                fade * 0.35
            } else {
                fade
            };
            if alpha <= 0.01 {
                continue;
            }
            let layout = layouts.get_mut(i).map(|slot| {
                slot.get_or_insert_with(|| {
                    let layout = self.create_pango_layout(Some(&model.graph.nodes[i].label));
                    layout.set_alignment(pango::Alignment::Center);
                    layout
                })
                .clone()
            });
            let Some(layout) = layout else { continue };
            let (lw, _) = layout.pixel_size();
            let (x, y) = screen[i];
            snapshot.save();
            snapshot.translate(&graphene::Point::new(
                x - lw as f32 / 2.0,
                y + self.radius(i) + 3.0,
            ));
            snapshot.append_layout(&layout, &with_alpha(&palette.label, alpha));
            snapshot.restore();
        }
    }

    // --- input -----------------------------------------------------------------------

    fn connect_controllers(&self) {
        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_, x, y| {
                view.imp().pointer.set((x, y));
                if view.imp().drag.get().is_some() {
                    return;
                }
                let node = view.node_at(x, y);
                if node != view.imp().hovered.get() {
                    view.imp().hovered.set(node);
                    view.set_cursor_from_name(node.map(|_| "pointer"));
                    view.queue_draw();
                }
            }
        ));
        motion.connect_leave(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_| {
                if view.imp().hovered.take().is_some() {
                    view.queue_draw();
                }
            }
        ));
        self.add_controller(motion);

        let drag = gtk::GestureDrag::new();
        drag.connect_drag_begin(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_, x, y| view.drag_begin(x, y)
        ));
        drag.connect_drag_update(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |gesture, dx, dy| {
                if let Some((x, y)) = gesture.start_point() {
                    view.drag_update(x, y, dx, dy);
                }
            }
        ));
        drag.connect_drag_end(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |gesture, dx, dy| {
                let ctrl = gesture
                    .current_event_state()
                    .contains(gdk::ModifierType::CONTROL_MASK);
                view.drag_end(dx, dy, ctrl);
            }
        ));
        self.add_controller(drag);

        let middle = gtk::GestureClick::builder().button(2).build();
        middle.connect_released(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_, _, x, y| {
                if let Some(i) = view.node_at(x, y) {
                    view.activate_node(i, true);
                }
            }
        ));
        self.add_controller(middle);

        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        scroll.connect_scroll(glib::clone!(
            #[weak(rename_to = view)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |controller, dx, dy| {
                let ctrl = controller
                    .current_event_state()
                    .contains(gdk::ModifierType::CONTROL_MASK);
                let unit = if controller.unit() == gdk::ScrollUnit::Surface {
                    1.0
                } else {
                    32.0
                };
                if ctrl {
                    let (x, y) = view.imp().pointer.get();
                    view.zoom_at(x, y, (-dy * unit / 200.0).exp() as f32);
                } else {
                    let (px, py) = view.imp().pan.get();
                    view.imp()
                        .pan
                        .set((px - (dx * unit) as f32, py - (dy * unit) as f32));
                    view.imp().user_moved.set(true);
                    view.queue_draw();
                }
                glib::Propagation::Stop
            }
        ));
        self.add_controller(scroll);

        let zoom = gtk::GestureZoom::new();
        zoom.connect_begin(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_, _| view.imp().zoom_start.set(view.imp().scale.get())
        ));
        zoom.connect_scale_changed(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |gesture, scale| {
                let (x, y) = gesture
                    .bounding_box_center()
                    .unwrap_or((view.width() as f64 / 2.0, view.height() as f64 / 2.0));
                let target = view.imp().zoom_start.get() * scale as f32;
                let factor = target / view.imp().scale.get();
                view.zoom_at(x, y, factor);
            }
        ));
        self.add_controller(zoom);
    }

    /// Zooms by `factor`, keeping the point under `(x, y)` still.
    pub fn zoom_at(&self, x: f64, y: f64, factor: f32) {
        let imp = self.imp();
        let before = self.to_layout(x, y);
        let scale = (imp.scale.get() * factor).clamp(0.02, 8.0);
        imp.scale.set(scale);
        let (sx, sy) = self.to_screen(before);
        let (px, py) = imp.pan.get();
        imp.pan.set((px + x as f32 - sx, py + y as f32 - sy));
        imp.user_moved.set(true);
        self.queue_draw();
    }

    fn drag_begin(&self, x: f64, y: f64) {
        let imp = self.imp();
        self.grab_focus();
        match self.node_at(x, y) {
            Some(index) => {
                let was_pinned = imp.pinned.borrow().contains(&index);
                imp.drag.set(Some(Drag::Node { index, was_pinned }));
                if let Some(layout) = imp.layout.borrow().as_ref() {
                    layout.send(Command::AlphaTarget(0.3));
                    layout.send(Command::Pin(index, self.to_layout(x, y)));
                }
                imp.settled.set(false);
                self.start_ticking();
            }
            None => imp.drag.set(Some(Drag::Pan {
                from: imp.pan.get(),
            })),
        }
    }

    fn drag_update(&self, x: f64, y: f64, dx: f64, dy: f64) {
        let imp = self.imp();
        if dx.hypot(dy) < CLICK_SLOP {
            return;
        }
        match imp.drag.get() {
            Some(Drag::Node { index, .. }) => {
                imp.pinned.borrow_mut().insert(index);
                if let Some(layout) = imp.layout.borrow().as_ref() {
                    layout.send(Command::Pin(index, self.to_layout(x + dx, y + dy)));
                }
            }
            Some(Drag::Pan { from }) => {
                imp.pan.set((from.0 + dx as f32, from.1 + dy as f32));
                imp.user_moved.set(true);
                self.queue_draw();
            }
            None => {}
        }
    }

    fn drag_end(&self, dx: f64, dy: f64, ctrl: bool) {
        let imp = self.imp();
        let clicked = dx.hypot(dy) < CLICK_SLOP;
        match imp.drag.take() {
            Some(Drag::Node { index, was_pinned }) => {
                if let Some(layout) = imp.layout.borrow().as_ref() {
                    if !was_pinned {
                        layout.send(Command::Unpin(index));
                    }
                    layout.send(Command::AlphaTarget(0.0));
                }
                if !was_pinned {
                    imp.pinned.borrow_mut().remove(&index);
                }
                if clicked {
                    self.activate_node(index, ctrl);
                }
            }
            Some(Drag::Pan { .. }) | None => {}
        }
    }
}

struct Palette {
    note: gdk::RGBA,
    attachment: gdk::RGBA,
    tag: gdk::RGBA,
    unresolved: gdk::RGBA,
    edge: gdk::RGBA,
    label: gdk::RGBA,
    accent: gdk::RGBA,
    groups: Vec<gdk::RGBA>,
}

/// A line between two points on screen.
type Line = ((f32, f32), (f32, f32));

/// Lines from `.0` to `.1`, `width` thick.
fn draw_lines(snapshot: &gtk::Snapshot, lines: &[Line], width: f32, colour: &gdk::RGBA) {
    for &(a, b) in lines {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = (dx * dx + dy * dy).sqrt();
        snapshot.save();
        snapshot.translate(&graphene::Point::new(a.0, a.1));
        snapshot.rotate(dy.atan2(dx).to_degrees());
        snapshot.append_color(colour, &graphene::Rect::new(0.0, -width / 2.0, len, width));
        snapshot.restore();
    }
}

fn with_alpha(c: &gdk::RGBA, alpha: f32) -> gdk::RGBA {
    gdk::RGBA::new(c.red(), c.green(), c.blue(), alpha)
}
