//! The graph being laid out.

use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeKind {
    Note,
    Attachment,
    Tag,
    /// A link target that doesn't exist yet.
    Unresolved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// Stable identity, such as the vault path or `#tag`. [`Simulation::update`]
    /// uses it to keep a node in place when the graph changes.
    ///
    /// [`Simulation::update`]: crate::Simulation::update
    pub id: String,
    pub label: String,
    pub kind: NodeKind,
    /// The colour group the node belongs to, if any.
    pub group: Option<u16>,
}

impl Node {
    pub fn new(id: impl Into<String>, label: impl Into<String>, kind: NodeKind) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind,
            group: None,
        }
    }
}

/// Nodes and the links between them. Edges are index pairs into `nodes`.
///
/// Links have a direction (an edge `(a, b)` is a link from `a` to `b`), but the
/// layout and the neighbourhood functions treat them as undirected. Self-links,
/// duplicate links and edges pointing past the end of `nodes` are ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<(u32, u32)>,
}

impl Graph {
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The index of the node with this ID.
    pub fn find(&self, id: &str) -> Option<usize> {
        self.nodes.iter().position(|node| node.id == id)
    }

    /// Each distinct, valid edge once, as `(lower, higher)` index pairs in
    /// ascending order.
    pub fn undirected_edges(&self) -> Vec<(u32, u32)> {
        let n = self.nodes.len();
        let mut edges: Vec<(u32, u32)> = self
            .edges
            .iter()
            .filter(|&&(a, b)| a != b && (a as usize) < n && (b as usize) < n)
            .map(|&(a, b)| (a.min(b), a.max(b)))
            .collect();
        edges.sort_unstable();
        edges.dedup();
        edges
    }

    /// Each node's neighbours, sorted, ignoring direction.
    pub fn adjacency(&self) -> Vec<Vec<u32>> {
        let mut adjacency = vec![Vec::new(); self.nodes.len()];
        for (a, b) in self.undirected_edges() {
            adjacency[a as usize].push(b);
            adjacency[b as usize].push(a);
        }
        for neighbours in &mut adjacency {
            neighbours.sort_unstable();
        }
        adjacency
    }

    /// How many distinct nodes each node is linked with, in either direction.
    pub fn degrees(&self) -> Vec<u32> {
        let mut degrees = vec![0; self.nodes.len()];
        for (a, b) in self.undirected_edges() {
            degrees[a as usize] += 1;
            degrees[b as usize] += 1;
        }
        degrees
    }

    /// The nodes linked with `index` in either direction, sorted.
    pub fn neighbours(&self, index: usize) -> Vec<usize> {
        let mut neighbours: Vec<usize> = self
            .undirected_edges()
            .into_iter()
            .filter_map(|(a, b)| {
                if a as usize == index {
                    Some(b as usize)
                } else if b as usize == index {
                    Some(a as usize)
                } else {
                    None
                }
            })
            .collect();
        neighbours.sort_unstable();
        neighbours
    }
}

/// The neighbourhood of `root`: every node within `depth` links of it, in
/// either direction, plus all edges between those nodes.
///
/// Returns the subgraph and, for each of its nodes, the index of the same node
/// in `graph`. The root comes first, followed by the other nodes in order of
/// distance. The inspector uses depths 1 to 3; depth 0 gives the root alone. A
/// root outside the graph gives an empty graph.
pub fn local(graph: &Graph, root: usize, depth: u8) -> (Graph, Vec<usize>) {
    if root >= graph.nodes.len() {
        return (Graph::default(), Vec::new());
    }
    let adjacency = graph.adjacency();
    let mut local_index = vec![u32::MAX; graph.nodes.len()];
    let mut order = vec![root];
    local_index[root] = 0;
    let mut queue = VecDeque::from([(root, 0u8)]);
    while let Some((node, distance)) = queue.pop_front() {
        if distance == depth {
            continue;
        }
        for &next in &adjacency[node] {
            let next = next as usize;
            if local_index[next] == u32::MAX {
                local_index[next] = order.len() as u32;
                order.push(next);
                queue.push_back((next, distance + 1));
            }
        }
    }

    let nodes = order.iter().map(|&i| graph.nodes[i].clone()).collect();
    let edges = graph
        .edges
        .iter()
        .filter(|&&(a, b)| (a as usize) < graph.nodes.len() && (b as usize) < graph.nodes.len())
        .filter_map(|&(a, b)| {
            let (a, b) = (local_index[a as usize], local_index[b as usize]);
            (a != u32::MAX && b != u32::MAX).then_some((a, b))
        })
        .collect();
    (Graph { nodes, edges }, order)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(n: usize, edges: &[(u32, u32)]) -> Graph {
        Graph {
            nodes: (0..n)
                .map(|i| Node::new(format!("n{i}"), format!("Node {i}"), NodeKind::Note))
                .collect(),
            edges: edges.to_vec(),
        }
    }

    #[test]
    fn degrees_ignore_direction_duplicates_and_bad_edges() {
        let g = graph(4, &[(0, 1), (1, 0), (0, 1), (1, 2), (2, 2), (3, 9)]);
        assert_eq!(g.undirected_edges(), [(0, 1), (1, 2)]);
        assert_eq!(g.degrees(), [1, 2, 1, 0]);
        assert_eq!(g.neighbours(1), [0, 2]);
        assert_eq!(g.adjacency()[1], [0, 2]);
        assert_eq!(g.find("n2"), Some(2));
        assert_eq!(g.find("missing"), None);
    }
}
