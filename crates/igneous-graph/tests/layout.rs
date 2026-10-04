//! Behaviour of the layout as a whole: determinism, convergence, pinning,
//! updates and local graphs.

use std::time::{Duration, Instant};

use igneous_graph::{Forces, Graph, Node, NodeKind, Simulation, Vec2, local};

/// A small deterministic generator, so test graphs don't need a dependency.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) % n as u64) as usize
    }
}

fn nodes(n: usize) -> Vec<Node> {
    (0..n)
        .map(|i| Node::new(format!("note-{i}.md"), format!("Note {i}"), NodeKind::Note))
        .collect()
}

/// `n` nodes and `m` random edges between them.
fn random_graph(n: usize, m: usize, seed: u64) -> Graph {
    let mut rng = Lcg(seed);
    let edges = (0..m)
        .map(|_| (rng.below(n) as u32, rng.below(n) as u32))
        .collect();
    Graph {
        nodes: nodes(n),
        edges,
    }
}

fn chain(n: usize) -> Graph {
    Graph {
        nodes: nodes(n),
        edges: (1..n as u32).map(|i| (i - 1, i)).collect(),
    }
}

fn settle(sim: &mut Simulation) -> usize {
    let mut ticks = 0;
    while sim.tick() {
        ticks += 1;
        assert!(ticks < 1000, "the simulation never settled");
    }
    ticks
}

#[test]
fn same_seed_same_layout() {
    let graph = random_graph(300, 600, 1);
    let mut a = Simulation::new(&graph, Forces::default(), 42);
    let mut b = Simulation::new(&graph, Forces::default(), 42);
    for _ in 0..100 {
        a.tick();
        b.tick();
    }
    assert_eq!(a.positions(), b.positions());

    let mut c = Simulation::new(&graph, Forces::default(), 43);
    for _ in 0..100 {
        c.tick();
    }
    assert_ne!(a.positions(), c.positions());
}

#[test]
fn settles_and_then_stops() {
    let graph = random_graph(200, 300, 2);
    let mut sim = Simulation::new(&graph, Forces::default(), 0);
    let ticks = settle(&mut sim);
    // d3's cooling schedule takes 300 ticks from alpha 1 to 0.001.
    assert!((290..=310).contains(&ticks), "{ticks} ticks");
    assert!(sim.is_settled());

    let before = sim.positions().to_vec();
    assert!(!sim.tick());
    assert_eq!(sim.positions(), before);
    assert!(
        sim.positions()
            .iter()
            .all(|p| p.x.is_finite() && p.y.is_finite())
    );

    sim.reheat();
    assert!(sim.tick());
}

#[test]
fn linked_nodes_end_up_closer_than_unlinked_ones() {
    let graph = random_graph(150, 200, 3);
    let mut sim = Simulation::new(&graph, Forces::default(), 0);
    settle(&mut sim);
    let p = sim.positions();
    let edges = graph.undirected_edges();
    let linked: f32 = edges
        .iter()
        .map(|&(a, b)| p[a as usize].distance(p[b as usize]))
        .sum::<f32>()
        / edges.len() as f32;
    let mut all = 0.0;
    let mut pairs = 0;
    for i in 0..p.len() {
        for j in i + 1..p.len() {
            all += p[i].distance(p[j]);
            pairs += 1;
        }
    }
    let all = all / pairs as f32;
    assert!(linked < all * 0.75, "linked {linked}, all {all}");
}

#[test]
fn pinned_nodes_stay_put() {
    let graph = chain(20);
    let mut sim = Simulation::new(&graph, Forces::default(), 0);
    let spot = Vec2::new(1000.0, -500.0);
    sim.pin(5, spot);
    assert_eq!(sim.positions()[5], spot);
    settle(&mut sim);
    assert_eq!(sim.positions()[5], spot);
    assert!(sim.is_pinned(5));

    // While dragging, the simulation stays warm.
    sim.set_alpha_target(0.3);
    for _ in 0..500 {
        assert!(sim.tick());
    }
    sim.set_alpha_target(0.0);
    sim.unpin(5);
    settle(&mut sim);
    assert_ne!(sim.positions()[5], spot);
}

#[test]
fn update_keeps_survivors_in_place() {
    let graph = chain(10);
    let mut sim = Simulation::new(&graph, Forces::default(), 0);
    settle(&mut sim);
    sim.pin(2, Vec2::new(50.0, 50.0));
    let before = sim.positions().to_vec();

    // Remove note-0, add a new note linked to note-9 and an unlinked one.
    let mut changed = Graph {
        nodes: graph.nodes[1..].to_vec(),
        edges: (1..9u32).map(|i| (i - 1, i)).collect(),
    };
    changed
        .nodes
        .push(Node::new("new.md", "New", NodeKind::Note));
    changed.nodes.push(Node::new("#tag", "#tag", NodeKind::Tag));
    changed.edges.push((8, 9));
    sim.update(&changed);

    assert_eq!(sim.len(), 11);
    for i in 0..9 {
        assert_eq!(sim.positions()[i], before[i + 1], "node {i} moved");
    }
    assert!(sim.is_pinned(1));
    let near = sim.positions()[9].distance(sim.positions()[8]);
    assert!(
        near <= Forces::default().link_distance,
        "new node {near} away"
    );
    assert!(sim.alpha() >= 0.3);
    settle(&mut sim);

    // An identical graph changes nothing and doesn't reheat.
    let settled = sim.positions().to_vec();
    sim.update(&changed);
    assert!(sim.is_settled());
    assert_eq!(sim.positions(), settled);
}

#[test]
fn changing_forces_reheats() {
    let graph = chain(5);
    let mut sim = Simulation::new(&graph, Forces::default(), 0);
    settle(&mut sim);
    sim.set_forces(Forces::default());
    assert!(sim.is_settled());
    sim.set_forces(Forces {
        link_distance: 100.0,
        ..Forces::default()
    });
    assert!(!sim.is_settled());
    settle(&mut sim);
    let d = sim.positions()[0].distance(sim.positions()[1]);
    assert!(d < 250.0, "{d}");
}

#[test]
fn hit_testing_and_bounds() {
    let graph = chain(3);
    let mut sim = Simulation::new(&graph, Forces::default(), 0);
    sim.pin(0, Vec2::new(0.0, 0.0));
    sim.pin(1, Vec2::new(100.0, 0.0));
    sim.pin(2, Vec2::new(0.0, 300.0));
    assert_eq!(sim.nearest(Vec2::new(90.0, 5.0), 20.0), Some(1));
    assert_eq!(sim.nearest(Vec2::new(50.0, 150.0), 20.0), None);
    let bounds = sim.bounds().unwrap();
    assert_eq!(bounds.min, Vec2::new(0.0, 0.0));
    assert_eq!(bounds.max, Vec2::new(100.0, 300.0));
    assert!(bounds.contains(Vec2::new(10.0, 10.0)));
}

#[test]
fn local_graph_depths() {
    // 0 - 1 - 2 - 3 - 4, plus 5 linked into 1 and an isolated 6.
    let mut graph = chain(5);
    graph.nodes.extend(nodes(7).drain(5..));
    graph.edges.push((5, 1));

    let (g, map) = local(&graph, 2, 1);
    assert_eq!(map, [2, 1, 3]);
    assert_eq!(g.nodes[0].id, "note-2.md");
    assert_eq!(g.undirected_edges(), [(0, 1), (0, 2)]);

    let (g, map) = local(&graph, 2, 2);
    assert_eq!(map, [2, 1, 3, 0, 5, 4]);
    assert_eq!(g.undirected_edges().len(), 5);

    let (_, map) = local(&graph, 2, 3);
    assert_eq!(map.len(), 6);

    let (g, map) = local(&graph, 6, 3);
    assert_eq!(map, [6]);
    assert!(g.edges.is_empty());

    let (g, map) = local(&graph, 99, 1);
    assert!(g.is_empty() && map.is_empty());
}

#[test]
fn local_graph_keeps_edges_between_neighbours() {
    // A triangle with a tail: 0-1, 1-2, 2-0, 2-3.
    let graph = Graph {
        nodes: nodes(4),
        edges: vec![(0, 1), (1, 2), (2, 0), (2, 3)],
    };
    let (g, map) = local(&graph, 0, 1);
    assert_eq!(map, [0, 1, 2]);
    // The 1-2 edge joins two depth-1 nodes and is kept; 2-3 leaves the set.
    assert_eq!(g.undirected_edges(), [(0, 1), (0, 2), (1, 2)]);
    assert_eq!(g.edges.len(), 3);
}

#[test]
#[ignore = "timing; run with `cargo test -p igneous-graph --release -- --ignored`"]
fn ticks_fast_on_a_large_graph() {
    let graph = random_graph(5_000, 20_000, 7);
    let mut sim = Simulation::new(&graph, Forces::default(), 0);
    for _ in 0..5 {
        sim.tick();
    }
    let mut times: Vec<Duration> = (0..60)
        .map(|_| {
            let start = Instant::now();
            sim.tick();
            start.elapsed()
        })
        .collect();
    times.sort();
    let median = times[times.len() / 2];
    eprintln!(
        "5,000 nodes, 20,000 edges: median tick {median:?}, max {:?}",
        times.last().unwrap()
    );
    assert!(median < Duration::from_millis(4), "median tick {median:?}");
}
