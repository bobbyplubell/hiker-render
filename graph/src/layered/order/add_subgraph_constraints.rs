//! Sibling-subgraph ordering constraints — a port of
//! `dagre/lib/order/add-subgraph-constraints.ts`.
//!
//! Walks the resolved order `vs` and, for each node, climbs its ancestor chain;
//! when two adjacent `vs` entries diverge at some ancestor level it adds a
//! constraint edge between the previous and current child at that level so the
//! subgraphs stay contiguous in subsequent sweeps.

use super::graph::Graph;

/// `addSubgraphConstraints(graph, constraintGraph, vs)`.
pub fn add_subgraph_constraints<G, N, E, CgG, CgN, CgE>(
    graph: &Graph<G, N, E>,
    constraint_graph: &mut Graph<CgG, CgN, CgE>,
    vs: &[String],
) where
    CgE: Default,
{
    use std::collections::HashMap;
    let mut prev: HashMap<String, String> = HashMap::new();
    let mut root_prev: Option<String> = None;

    for v in vs {
        let mut child: Option<String> = graph.parent(v);
        while let Some(c) = child.clone() {
            let parent = graph.parent(&c);
            let prev_child: Option<String>;
            match &parent {
                Some(p) => {
                    prev_child = prev.get(p).cloned();
                    prev.insert(p.clone(), c.clone());
                }
                None => {
                    prev_child = root_prev.clone();
                    root_prev = Some(c.clone());
                }
            }
            if let Some(pc) = prev_child {
                if pc != c {
                    constraint_graph.set_edge(pc, c.clone(), CgE::default(), None);
                    break;
                }
            }
            child = parent;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{new_compound_graph, new_constraint_graph};
    use super::*;

    fn ids(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn does_not_change_cg_for_flat_set() {
        let mut g = new_compound_graph();
        let vs = ids(&["a", "b", "c", "d"]);
        for v in &vs {
            g.ensure_node(v.clone());
        }
        let mut cg = new_constraint_graph();
        add_subgraph_constraints(&g, &mut cg, &vs);
        assert_eq!(cg.node_count(), 0);
        assert_eq!(cg.edge_count(), 0);
    }

    #[test]
    fn no_constraint_for_contiguous_subgraph_nodes() {
        let mut g = new_compound_graph();
        let vs = ids(&["a", "b", "c"]);
        for v in &vs {
            g.set_parent(v.clone(), "sg");
        }
        let mut cg = new_constraint_graph();
        add_subgraph_constraints(&g, &mut cg, &vs);
        assert_eq!(cg.node_count(), 0);
        assert_eq!(cg.edge_count(), 0);
    }

    #[test]
    fn adds_constraint_when_parents_differ() {
        let mut g = new_compound_graph();
        let vs = ids(&["a", "b"]);
        g.set_parent("a", "sg1");
        g.set_parent("b", "sg2");
        let mut cg = new_constraint_graph();
        add_subgraph_constraints(&g, &mut cg, &vs);
        let edges = cg.edges();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].v, "sg1");
        assert_eq!(edges[0].w, "sg2");
    }

    #[test]
    fn works_for_multiple_levels() {
        let mut g = new_compound_graph();
        let vs = ids(&["a", "b", "c", "d", "e", "f", "g", "h"]);
        for v in &vs {
            g.ensure_node(v.clone());
        }
        g.set_parent("b", "sg2");
        g.set_parent("sg2", "sg1");
        g.set_parent("c", "sg1");
        g.set_parent("d", "sg3");
        g.set_parent("sg3", "sg1");
        g.set_parent("f", "sg4");
        g.set_parent("g", "sg5");
        g.set_parent("sg5", "sg4");
        let mut cg = new_constraint_graph();
        add_subgraph_constraints(&g, &mut cg, &vs);
        let mut edges = cg.edges();
        edges.sort_by(|a, b| a.v.cmp(&b.v));
        assert_eq!(edges.len(), 2);
        assert_eq!((edges[0].v.as_str(), edges[0].w.as_str()), ("sg1", "sg4"));
        assert_eq!((edges[1].v.as_str(), edges[1].w.as_str()), ("sg2", "sg3"));
    }
}
