//! Rank assignment — a port of `dagre/lib/rank/index.ts`.
//!
//! Assigns a `rank` to each node respecting edge `minlen` constraints. The
//! [`rank`] dispatcher chooses a ranker based on `graph().ranker`:
//!
//! * [`Ranker::NetworkSimplex`] (and the default / unknown case) → network simplex
//! * [`Ranker::TightTree`] → longest-path then feasible-tree
//! * [`Ranker::LongestPath`] → longest-path only
//!
//! Derived from Gansner, et al., "A Technique for Drawing Directed Graphs."

pub mod feasible_tree;
pub mod network_simplex;
pub mod util;

pub use feasible_tree::{feasible_tree, TreeEdgeLabel, TreeGraph, TreeNodeLabel};
pub use network_simplex::network_simplex;
pub use util::{longest_path, slack};

use crate::layered::types::{DagreGraph, Ranker};

/// `rank(graph)` — assign a `rank` to every node (ranks may start at any index;
/// they are normalized later in the pipeline).
pub fn rank(graph: &mut DagreGraph) {
    let ranker = graph.graph().and_then(|g| g.ranker);
    match ranker {
        Some(Ranker::TightTree) => tight_tree_ranker(graph),
        Some(Ranker::LongestPath) => longest_path_ranker(graph),
        // network-simplex, plus the JS default / "unknown" fall-through.
        Some(Ranker::NetworkSimplex) | None => network_simplex_ranker(graph),
    }
}

/// A fast and simple ranker, but results are far from optimal.
fn longest_path_ranker(g: &mut DagreGraph) {
    longest_path(g);
}

fn tight_tree_ranker(g: &mut DagreGraph) {
    longest_path(g);
    feasible_tree(g);
}

fn network_simplex_ranker(g: &mut DagreGraph) {
    network_simplex(g);
}

#[cfg(test)]
mod tests;
