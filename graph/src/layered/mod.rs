//! Layered (Sugiyama / dagre) graph layout.
//!
//! This module is a faithful pure-Rust port of dagre.js. **Step 0** is the
//! compound-multigraph data structure ([`graph::Graph`]) that every later
//! dagre module runs on — a port of graphlib's `Graph` class.
//!
//! egui-free, std-only, deterministic (no randomness, no time). Insertion
//! order is preserved everywhere graphlib guarantees it, because dagre's
//! output determinism depends on it.

pub mod acyclic;
pub mod add_border_segments;
pub mod coordinate_system;
pub mod engine;
pub mod graph;
pub mod greedy_fas;
pub mod layout;
pub mod list;
pub mod nesting_graph;
pub mod normalize;
pub mod order;
pub mod parent_dummy_chains;
pub mod position;
pub mod rank;
pub mod types;
pub mod util;

pub use add_border_segments::add_border_segments;
pub use engine::LayeredEngine;
pub use greedy_fas::greedy_fas;
pub use layout::{layout, layout_with_opts, LayoutOptions};
pub use list::{EntryId, List, ListArena};
pub use parent_dummy_chains::parent_dummy_chains;

pub use graph::{Edge, Graph, GraphOptions, NodeId};
pub use rank::{
    feasible_tree, longest_path, network_simplex, rank, slack, TreeEdgeLabel, TreeGraph,
    TreeNodeLabel,
};
pub use types::{
    Align, BorderType, DagreGraph, DummyKind, EdgeLabel, GraphLabel, LabelPos, NodeLabel,
    OrderConstraint, PartitionResult, Point, Ranker, RankDir, RankAlign, Acyclicer,
};
