//! The force simulation.
//!
//! This follows d3-force closely, so its behaviour (and Obsidian's, which
//! imitates it) carries over: velocity Verlet integration with friction, an
//! `alpha` "temperature" that scales every force and cools towards zero, and
//! three forces applied in this order each tick:
//!
//! 1. **Links** pull linked nodes towards a rest length. As in d3, a link's
//!    strength is `1 / min(degree)` of its two ends, so hubs aren't pulled
//!    apart, and the correction is split between the ends by degree.
//! 2. **Repulsion** between every pair of nodes, approximated with a
//!    Barnes–Hut quadtree.
//! 3. **Centring** pulls each node towards the origin, so separate clusters
//!    and unlinked notes stay in view.

use std::collections::HashMap;
use std::f32::consts::PI;

use crate::graph::Graph;
use crate::quadtree::QuadTree;
use crate::rng::Rng;
use crate::vec2::{Rect, Vec2};

/// How strongly each force acts. The fields match the sliders in the graph's
/// "Forces" popover and the keys in `.igneous/graph.json`, and their defaults
/// match Obsidian's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Forces {
    /// Pull towards the origin, 0 to 1. Becomes d3's `forceX`/`forceY`
    /// strength `center / 10`, so 1 is d3's default pull.
    pub center: f32,
    /// Repulsion between nodes, 0 to 20. Becomes d3's many-body strength
    /// `-30 × repel`, so 1 is d3's default.
    pub repel: f32,
    /// Spring strength of links, 0 to 1. Multiplies d3's default link strength
    /// of `1 / min(degree)`.
    pub link_strength: f32,
    /// Rest length of links, in layout units (pixels at zoom 1), 30 to 500.
    pub link_distance: f32,
}

impl Default for Forces {
    fn default() -> Self {
        Self {
            center: 0.5,
            repel: 10.0,
            link_strength: 1.0,
            link_distance: 250.0,
        }
    }
}

/// d3's defaults: `alpha` decays from 1 to `ALPHA_MIN` in 300 ticks.
const ALPHA_MIN: f32 = 0.001;
const VELOCITY_DECAY: f32 = 0.4;
const THETA: f32 = 0.9;
/// How hot [`Simulation::update`] makes a settled simulation, so that new
/// nodes find their place without shaking up the rest.
const UPDATE_ALPHA: f32 = 0.3;

#[derive(Debug, Clone, Copy)]
struct Link {
    source: u32,
    target: u32,
    strength: f32,
    /// The share of the correction applied to the target.
    bias: f32,
}

/// Lays out a [`Graph`]. Call [`tick`](Self::tick) once per frame until it
/// returns `false`, reading [`positions`](Self::positions) after each tick.
#[derive(Debug)]
pub struct Simulation {
    forces: Forces,
    ids: Vec<String>,
    edges: Vec<(u32, u32)>,
    links: Vec<Link>,
    positions: Vec<Vec2>,
    velocities: Vec<Vec2>,
    pinned: Vec<Option<Vec2>>,
    alpha: f32,
    alpha_target: f32,
    alpha_decay: f32,
    rng: Rng,
    tree: QuadTree,
}

impl Simulation {
    /// Places the nodes in a seeded spiral and starts the simulation hot. The
    /// same graph, forces and seed always give the same layout.
    pub fn new(graph: &Graph, forces: Forces, seed: u64) -> Self {
        let mut sim = Self {
            forces,
            ids: Vec::new(),
            edges: Vec::new(),
            links: Vec::new(),
            positions: Vec::new(),
            velocities: Vec::new(),
            pinned: Vec::new(),
            alpha: 1.0,
            alpha_target: 0.0,
            alpha_decay: 1.0 - ALPHA_MIN.powf(1.0 / 300.0),
            rng: Rng::new(seed),
            tree: QuadTree::default(),
        };
        sim.ids = graph.nodes.iter().map(|node| node.id.clone()).collect();
        sim.positions = (0..graph.len()).map(|i| sim.spiral(i)).collect();
        sim.velocities = vec![Vec2::ZERO; graph.len()];
        sim.pinned = vec![None; graph.len()];
        sim.edges = graph.undirected_edges();
        sim.update_links();
        sim
    }

    /// Initial position `i`: d3's phyllotaxis spiral, scaled to the link
    /// distance, plus a little seeded jitter.
    fn spiral(&mut self, i: usize) -> Vec2 {
        let scale = (self.forces.link_distance / 30.0).max(1.0);
        let radius = 10.0 * scale * (0.5 + i as f32).sqrt();
        let angle = i as f32 * PI * (3.0 - 5f32.sqrt());
        let jitter = Vec2::new(self.rng.next_f32() - 0.5, self.rng.next_f32() - 0.5) * scale;
        Vec2::new(radius * angle.cos(), radius * angle.sin()) + jitter
    }

    fn update_links(&mut self) {
        let mut degree = vec![0u32; self.positions.len()];
        for &(a, b) in &self.edges {
            degree[a as usize] += 1;
            degree[b as usize] += 1;
        }
        self.links = self
            .edges
            .iter()
            .map(|&(source, target)| {
                let (s, t) = (degree[source as usize], degree[target as usize]);
                Link {
                    source,
                    target,
                    strength: self.forces.link_strength / s.min(t) as f32,
                    bias: s as f32 / (s + t) as f32,
                }
            })
            .collect();
    }

    pub fn len(&self) -> usize {
        self.positions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    /// Each node's position, indexed like the graph's nodes.
    pub fn positions(&self) -> &[Vec2] {
        &self.positions
    }

    pub fn forces(&self) -> Forces {
        self.forces
    }

    /// The current temperature: 1 when hot, below 0.001 once settled.
    pub fn alpha(&self) -> f32 {
        self.alpha
    }

    /// True once the simulation has cooled down and [`tick`](Self::tick) does
    /// nothing.
    pub fn is_settled(&self) -> bool {
        self.alpha < ALPHA_MIN && self.alpha_target < ALPHA_MIN
    }

    /// Advances the layout by one step. Returns whether nodes are still
    /// moving; once it returns `false`, further ticks do nothing until the
    /// simulation is reheated.
    pub fn tick(&mut self) -> bool {
        if self.is_settled() || self.positions.is_empty() {
            return false;
        }
        self.alpha += (self.alpha_target - self.alpha) * self.alpha_decay;
        let alpha = self.alpha;

        // Links: d3's forceLink, one iteration.
        for link in &self.links {
            let (s, t) = (link.source as usize, link.target as usize);
            let mut d =
                (self.positions[t] + self.velocities[t]) - (self.positions[s] + self.velocities[s]);
            if d.x == 0.0 {
                d.x = self.rng.jiggle();
            }
            if d.y == 0.0 {
                d.y = self.rng.jiggle();
            }
            let l = d.length();
            let d = d * ((l - self.forces.link_distance) / l * alpha * link.strength);
            self.velocities[t] -= d * link.bias;
            self.velocities[s] += d * (1.0 - link.bias);
        }

        // Repulsion: d3's forceManyBody.
        self.tree.build(&self.positions, THETA * THETA);
        self.tree.apply_many_body(
            &self.positions,
            &mut self.velocities,
            -30.0 * self.forces.repel * alpha,
            &mut self.rng,
        );

        // Centring: d3's forceX and forceY towards the origin.
        let pull = self.forces.center / 10.0 * alpha;
        for (p, v) in self.positions.iter().zip(&mut self.velocities) {
            *v -= *p * pull;
        }

        for ((p, v), pin) in self
            .positions
            .iter_mut()
            .zip(&mut self.velocities)
            .zip(&self.pinned)
        {
            match pin {
                Some(pin) => {
                    *p = *pin;
                    *v = Vec2::ZERO;
                }
                None => {
                    *v = *v * (1.0 - VELOCITY_DECAY);
                    *p += *v;
                }
            }
        }
        !self.is_settled()
    }

    /// Holds node `index` at `position`, for example while it's dragged. The
    /// node moves there immediately.
    pub fn pin(&mut self, index: usize, position: Vec2) {
        self.pinned[index] = Some(position);
        self.positions[index] = position;
        self.velocities[index] = Vec2::ZERO;
    }

    /// Lets a pinned node move again.
    pub fn unpin(&mut self, index: usize) {
        self.pinned[index] = None;
    }

    pub fn is_pinned(&self, index: usize) -> bool {
        self.pinned[index].is_some()
    }

    /// Restarts a settled simulation at full heat, for example after the forces
    /// change.
    pub fn reheat(&mut self) {
        self.alpha = 1.0;
    }

    /// Keeps the simulation at least this hot until it's set back to 0. While
    /// dragging a node, d3 uses 0.3 so the rest of the graph follows.
    pub fn set_alpha_target(&mut self, target: f32) {
        self.alpha_target = target.clamp(0.0, 1.0);
    }

    /// Changes the forces and reheats the simulation.
    pub fn set_forces(&mut self, forces: Forces) {
        if forces != self.forces {
            self.forces = forces;
            self.update_links();
            self.reheat();
        }
    }

    /// Switches to a changed graph, such as after a note is added or a link
    /// removed. Nodes whose ID is still present keep their position, speed and
    /// pin. New nodes start next to a neighbour that was already placed, or on
    /// the spiral if they have none. If anything changed, the simulation is
    /// warmed up enough to make room for the changes.
    pub fn update(&mut self, graph: &Graph) {
        let old: HashMap<&str, usize> = self
            .ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();
        let survivors: Vec<Option<usize>> = graph
            .nodes
            .iter()
            .map(|node| old.get(node.id.as_str()).copied())
            .collect();
        let edges = graph.undirected_edges();
        let unchanged = survivors.len() == self.ids.len()
            && survivors.iter().enumerate().all(|(i, &s)| s == Some(i))
            && edges == self.edges;
        if unchanged {
            return;
        }

        let mut positions = Vec::with_capacity(graph.len());
        let mut velocities = Vec::with_capacity(graph.len());
        let mut pinned = Vec::with_capacity(graph.len());
        for &survivor in &survivors {
            match survivor {
                Some(i) => {
                    positions.push(self.positions[i]);
                    velocities.push(self.velocities[i]);
                    pinned.push(self.pinned[i]);
                }
                None => {
                    positions.push(Vec2::ZERO);
                    velocities.push(Vec2::ZERO);
                    pinned.push(None);
                }
            }
        }

        // Place each new node a short, random way from a neighbour that was
        // already placed, so new nodes don't land on top of each other.
        let adjacency = graph.adjacency();
        for (i, survivor) in survivors.iter().enumerate() {
            if survivor.is_some() {
                continue;
            }
            let neighbour = adjacency[i]
                .iter()
                .map(|&j| j as usize)
                .find(|&j| survivors[j].is_some());
            positions[i] = match neighbour {
                Some(j) => {
                    let angle = self.rng.next_f32() * 2.0 * PI;
                    let reach = self.forces.link_distance * (0.1 + 0.2 * self.rng.next_f32());
                    positions[j] + Vec2::new(angle.cos(), angle.sin()) * reach
                }
                None => self.spiral(i),
            };
        }

        self.ids = graph.nodes.iter().map(|node| node.id.clone()).collect();
        self.positions = positions;
        self.velocities = velocities;
        self.pinned = pinned;
        self.edges = edges;
        self.update_links();
        self.alpha = self.alpha.max(UPDATE_ALPHA);
    }

    /// The node closest to `point`, if one is within `max_distance` of it.
    pub fn nearest(&self, point: Vec2, max_distance: f32) -> Option<usize> {
        let limit = max_distance * max_distance;
        self.positions
            .iter()
            .enumerate()
            .map(|(i, p)| (i, (*p - point).length_squared()))
            .filter(|&(_, d)| d <= limit)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// The smallest rectangle holding every node, or `None` for an empty graph.
    pub fn bounds(&self) -> Option<Rect> {
        let first = *self.positions.first()?;
        let mut rect = Rect {
            min: first,
            max: first,
        };
        for p in &self.positions[1..] {
            rect.min = Vec2::new(rect.min.x.min(p.x), rect.min.y.min(p.y));
            rect.max = Vec2::new(rect.max.x.max(p.x), rect.max.y.max(p.y));
        }
        Some(rect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Node, NodeKind};

    fn pair() -> Graph {
        Graph {
            nodes: vec![
                Node::new("a", "A", NodeKind::Note),
                Node::new("b", "B", NodeKind::Note),
            ],
            edges: vec![(0, 1)],
        }
    }

    #[test]
    fn a_linked_pair_settles_near_the_link_distance() {
        let forces = Forces {
            repel: 0.0,
            center: 0.0,
            ..Forces::default()
        };
        let mut sim = Simulation::new(&pair(), forces, 1);
        while sim.tick() {}
        let d = sim.positions()[0].distance(sim.positions()[1]);
        assert!((d - forces.link_distance).abs() < 5.0, "distance {d}");
    }

    #[test]
    fn link_bias_follows_degree() {
        let graph = Graph {
            nodes: (0..4)
                .map(|i| Node::new(i.to_string(), "", NodeKind::Note))
                .collect(),
            edges: vec![(0, 1), (0, 2), (0, 3)],
        };
        let sim = Simulation::new(&graph, Forces::default(), 0);
        // The hub has degree 3 and each leaf degree 1: strength 1/min = 1, and
        // the leaf (the target) takes 3/4 of each correction.
        assert!(
            sim.links
                .iter()
                .all(|l| l.strength == 1.0 && l.bias == 0.75)
        );
    }

    #[test]
    fn empty_graph() {
        let mut sim = Simulation::new(&Graph::default(), Forces::default(), 0);
        assert!(!sim.tick());
        assert_eq!(sim.bounds(), None);
        assert_eq!(sim.nearest(Vec2::ZERO, 100.0), None);
    }
}
