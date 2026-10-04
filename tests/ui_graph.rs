//! The graph tab and the inspector's local graph, on a copy of the fixture
//! vault. Run with `build-aux/run-ui-tests.sh`.

use adw::prelude::*;
use igneous::{GraphModel, GraphPage, GraphView, Window};
use igneous_core::settings::{self as vault_settings, GraphGroup, GraphSettings};

mod common;
use common::*;

fn labels(model: &GraphModel) -> Vec<String> {
    let mut labels: Vec<String> = model.graph.nodes.iter().map(|n| n.label.clone()).collect();
    labels.sort();
    labels
}

/// Opens the graph tab once the index is ready, and waits for its data.
async fn graph(window: &Window) -> GraphPage {
    assert!(until(5000, || window.index().is_ready()).await);
    WidgetExt::activate_action(window, "win.graph", None).unwrap();
    let page = window.graph_page().expect("a graph tab");
    assert!(until(5000, || !page.view().model().graph.is_empty()).await);
    page
}

fn index_of(view: &GraphView, label: &str) -> usize {
    view.model()
        .graph
        .nodes
        .iter()
        .position(|n| n.label == label)
        .unwrap_or_else(|| panic!("no node {label}"))
}

#[gtk::test]
async fn graph_tab_shows_notes_and_links() {
    let dir = vault(None);
    let window = open(&dir);
    let page = graph(&window).await;
    let model = page.view().model();
    assert_eq!(
        labels(&model),
        ["2026-10-04", "Home", "Ideas", "Roadmap", "Windows"]
    );
    // Home links to Roadmap and the daily note; Roadmap links back.
    assert_eq!(model.graph.edges.len(), 3);
    let view = page.view();
    let home = index_of(view, "Home");
    assert_eq!(model.graph.neighbours(home).len(), 2);
    assert_eq!(window.selected_path(), None);
    window.close();
}

#[gtk::test]
async fn clicking_a_node_opens_its_note() {
    let dir = vault(None);
    let window = open(&dir);
    let page = graph(&window).await;
    let view = page.view().clone();
    assert!(until(10_000, || view.is_settled()).await);
    let roadmap = index_of(&view, "Roadmap");
    let (x, y) = view.node_position(roadmap).unwrap();
    assert_eq!(view.node_at(f64::from(x), f64::from(y)), Some(roadmap));
    view.activate_node(roadmap, false);
    assert_eq!(
        window.selected_path(),
        Some(p("Projects/Igneous/Roadmap.md"))
    );
    // Ctrl+click opens another tab rather than replacing the note.
    view.activate_node(index_of(&view, "Home"), true);
    assert_eq!(window.tab_paths().len(), 2);
    window.close();
}

#[gtk::test]
async fn the_layout_settles_and_stops() {
    let dir = vault(None);
    let window = open(&dir);
    let page = graph(&window).await;
    let view = page.view().clone();
    assert!(until(10_000, || view.is_settled() && !view.is_ticking()).await);
    // Idle: no per-frame work until something changes.
    wait(300).await;
    assert!(!view.is_ticking());
    window.close();
}

#[gtk::test]
async fn filters_groups_and_kinds() {
    let dir = vault(None);
    let window = open(&dir);
    let page = graph(&window).await;
    page.set_settings(GraphSettings {
        filter: "path:Projects".into(),
        ..page.settings()
    });
    assert_eq!(
        labels(&page.view().model()),
        ["Ideas", "Roadmap", "Windows"]
    );

    page.set_settings(GraphSettings {
        filter: String::new(),
        show_attachments: true,
        show_orphans: false,
        groups: vec![GraphGroup {
            query: "path:Daily".into(),
            color: Some("rgb(200,50,50)".into()),
        }],
        ..page.settings()
    });
    let model = page.view().model();
    assert_eq!(
        labels(&model),
        ["2026-10-04", "Home", "Roadmap", "diagram.png"]
    );
    let daily = index_of(page.view(), "2026-10-04");
    assert_eq!(model.graph.nodes[daily].group, Some(0));
    assert_eq!(model.graph.nodes[index_of(page.view(), "Home")].group, None);
    window.close();
}

#[gtk::test]
async fn graph_settings_and_tab_persist() {
    let dir = vault(None);
    let window = open(&dir);
    let page = graph(&window).await;
    let changed = GraphSettings {
        show_tags: true,
        arrows: true,
        repel_force: 4.5,
        groups: vec![GraphGroup {
            query: "tag:#home".into(),
            color: None,
        }],
        ..page.settings()
    };
    page.set_settings(changed.clone());
    page.save_now();
    let saved: GraphSettings = vault_settings::load(&dir.path().join(".igneous")).unwrap();
    assert_eq!(saved, changed);
    window.save_workspace();
    window.close();

    // The graph tab and its settings come back.
    let window = open(&dir);
    let page = window.graph_page().expect("the graph tab is restored");
    assert_eq!(page.settings(), changed);
    window.close();
}

#[gtk::test]
async fn local_graph_follows_the_note() {
    let dir = vault(None);
    let window = open(&dir);
    assert!(until(5000, || window.index().is_ready()).await);
    window.show_inspector("graph");
    window.open_path(&p("Projects/Igneous/Roadmap.md"), false);
    assert!(until(5000, || window.local_graph_labels().len() == 2).await);
    assert_eq!(window.local_graph_labels(), ["Roadmap", "Home"]);
    window.open_path(&p("Projects/Ideas.md"), false);
    assert!(until(5000, || window.local_graph_labels() == ["Ideas"]).await);
    window.save_workspace();
    let saved = std::fs::read_to_string(dir.path().join(".igneous/workspace.json")).unwrap();
    assert!(saved.contains("\"view\": \"localGraph\""), "{saved}");
    window.close();
}

/// A synthetic 5,000-node, 20,000-edge graph: how long building a frame and
/// rendering it take. Headless mutter renders in software, so these numbers
/// are an upper bound for a real GPU.
#[gtk::test]
#[ignore = "timing"]
async fn large_graph_frame_time() {
    use igneous_graph::{Graph, Node, NodeKind};
    use std::time::Instant;

    let n = 5000u32;
    let mut graph = Graph::default();
    for i in 0..n {
        graph.nodes.push(Node::new(
            format!("n{i}"),
            format!("Note {i}"),
            NodeKind::Note,
        ));
    }
    let mut seed = 7u64;
    let mut next = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        (seed >> 33) as u32 % n
    };
    while graph.edges.len() < 20_000 {
        let (a, b) = (next(), next());
        if a != b {
            graph.edges.push((a, b));
        }
    }
    let model = GraphModel {
        paths: vec![None; n as usize],
        graph,
        errors: Vec::new(),
    };
    let view = GraphView::new();
    let window = gtk::Window::builder()
        .default_width(1280)
        .default_height(800)
        .child(&view)
        .build();
    window.present();
    view.set_model(model, igneous_graph::Forces::default());
    wait(500).await;

    let renderer = window.renderer().unwrap();
    let median = |mut v: Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    let ms = |started: Instant| started.elapsed().as_secs_f64() * 1000.0;
    // Building a frame (our drawing code) and rendering it.
    let (mut build, mut render) = (Vec::new(), Vec::new());
    for _ in 0..20 {
        wait(16).await;
        let started = Instant::now();
        let snapshot = gtk::Snapshot::new();
        view.draw_into(&snapshot);
        let node = snapshot.to_node().unwrap();
        build.push(ms(started));
        let started = Instant::now();
        let _ = renderer.render_texture(&node, None);
        render.push(ms(started));
    }
    let settle = Instant::now();
    assert!(until(60_000, || view.is_settled()).await);
    eprintln!(
        "5,000 nodes, 20,000 edges ({}): frame build {:.2} ms, render {:.1} ms (medians of 20); \
         settled {:.1} s after timing",
        renderer.type_().name(),
        median(build),
        median(render),
        settle.elapsed().as_secs_f64()
    );
    window.close();
}

fn descendants(widget: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut out = Vec::new();
    let mut child = widget.first_child();
    while let Some(c) = child {
        out.push(c.clone());
        out.extend(descendants(&c));
        child = c.next_sibling();
    }
    out
}

/// Saves PNGs of the graph tab and the local graph, for checking by eye.
/// `IGNEOUS_SCREENSHOT=/path/out.png build-aux/run-ui-tests.sh --test ui_graph -- --ignored screenshot`
#[gtk::test]
#[ignore = "visual check"]
async fn screenshot() {
    let Ok(out) = std::env::var("IGNEOUS_SCREENSHOT") else {
        return;
    };
    let dir = vault(None);
    let window = open(&dir);
    window.set_default_size(1100, 720);
    if std::env::var("IGNEOUS_SCREENSHOT_DARK").is_ok() {
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
    }
    let page = graph(&window).await;
    page.set_settings(GraphSettings {
        show_attachments: true,
        show_tags: true,
        arrows: true,
        groups: vec![GraphGroup {
            query: "path:Projects".into(),
            color: None,
        }],
        ..page.settings()
    });
    let view = page.view().clone();
    assert!(until(10_000, || view.is_settled()).await);
    wait(300).await;
    save_png(window.upcast_ref(), &out);

    // The settings popover, drawn on its own.
    let menu = descendants(page.upcast_ref())
        .into_iter()
        .find_map(|w| w.downcast::<gtk::MenuButton>().ok())
        .unwrap();
    menu.popup();
    wait(800).await;
    let popover = menu.popover().unwrap();
    let paintable = gtk::WidgetPaintable::new(Some(&popover));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, popover.width() as f64, popover.height() as f64);
    if let Some(node) = snapshot.to_node() {
        let texture = window.renderer().unwrap().render_texture(node, None);
        texture
            .save_to_png(out.replace(".png", "-settings.png"))
            .unwrap();
    }
    menu.popdown();

    // Wide enough for the note, the inspector and both sidebars.
    window.set_default_size(1500, 720);
    window.show_inspector("graph");
    window.open_path(&p("Home.md"), false);
    wait(1500).await;
    save_png(window.upcast_ref(), &out.replace(".png", "-local.png"));
    window.close();
}
