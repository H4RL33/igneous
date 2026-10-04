//! Force-directed layout for Igneous's graph view.
//!
//! A [`Graph`] holds the nodes (notes, attachments, tags and unresolved links)
//! and the links between them. A [`Simulation`] lays it out with the same
//! forces as d3-force, which Obsidian's graph also imitates: many-body
//! repulsion (approximated with a Barnes–Hut quadtree), springs along links,
//! and a pull towards the centre. The simulation cools down on its own and
//! [`Simulation::tick`] returns `false` once it has settled, so an idle graph
//! costs nothing.
//!
//! [`local`] extracts the neighbourhood of one node for the inspector's local
//! graph.
//!
//! Nothing in this crate depends on GTK; the widget that draws the graph lives
//! in the app.

#![forbid(unsafe_code)]

mod graph;
mod quadtree;
mod rng;
mod sim;
mod vec2;

pub use graph::{Graph, Node, NodeKind, local};
pub use sim::{Forces, Simulation};
pub use vec2::{Rect, Vec2};
