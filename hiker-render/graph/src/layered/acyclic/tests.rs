//! Port of `dagre/test/acyclic-test.ts`. Test names mirror the TS so a reviewer
//! can diff against the original oracle. Covers the dfs and greedy acyclicers,
//! self-loops, and run→undo round-trips.

use super::*;
use crate::layered::graph::{Edge, GraphOptions};
use crate::layered::types::{Acyclicer, EdgeLabel};

fn new_graph() -> DagreGraph {
    let mut g: DagreGraph = Graph::new(GraphOptions {
        directed: true,
        multigraph: true,
        compound: false,
    });
    // setDefaultEdgeLabel(() => ({minlen: 1, weight: 1}))
    g.set_default_edge_label(EdgeLabel {
        minlen: Some(1),
        weight: Some(1.0),
        ..Default::default()
    });
    g
}

/// Set the graph's acyclicer from the test's string id. "dfs" and
/// "unknown-should-still-work" both fall through to the DFS acyclicer (graph
/// label has no `acyclicer` set).
fn set_acyclicer(g: &mut DagreGraph, name: &str) {
    let acyclicer = match name {
        "greedy" => Some(Acyclicer::Greedy),
        _ => None,
    };
    g.set_graph(GraphLabel {
        acyclicer,
        ..Default::default()
    });
}

/// `setPath` with the default edge label.
fn set_path(g: &mut DagreGraph, path: &[&str]) {
    for pair in path.windows(2) {
        g.ensure_edge(pair[0], pair[1], None);
    }
}

/// Tarjan-based `alg.findCycles`, test-only oracle (see greedy-fas tests).
fn find_cycles(graph: &DagreGraph) -> Vec<Vec<String>> {
    struct State<'a> {
        graph: &'a DagreGraph,
        index: usize,
        stack: Vec<String>,
        visited: std::collections::HashMap<String, (bool, usize, usize)>,
        results: Vec<Vec<String>>,
    }

    fn dfs(s: &mut State, v: &str) {
        let idx = s.index;
        s.visited.insert(v.to_string(), (true, idx, idx));
        s.index += 1;
        s.stack.push(v.to_string());

        for w in s.graph.successors(v).unwrap_or_default() {
            if !s.visited.contains_key(&w) {
                dfs(s, &w);
                let w_low = s.visited[&w].1;
                let e = s.visited.get_mut(v).unwrap();
                e.1 = e.1.min(w_low);
            } else if s.visited[&w].0 {
                let w_index = s.visited[&w].2;
                let e = s.visited.get_mut(v).unwrap();
                e.1 = e.1.min(w_index);
            }
        }

        let (_, lowlink, index) = s.visited[v];
        if lowlink == index {
            let mut cmpt = Vec::new();
            loop {
                let w = s.stack.pop().unwrap();
                s.visited.get_mut(&w).unwrap().0 = false;
                cmpt.push(w.clone());
                if w == v {
                    break;
                }
            }
            s.results.push(cmpt);
        }
    }

    let mut s = State {
        graph,
        index: 0,
        stack: Vec::new(),
        visited: std::collections::HashMap::new(),
        results: Vec::new(),
    };
    for v in graph.nodes() {
        if !s.visited.contains_key(&v) {
            dfs(&mut s, &v);
        }
    }
    s.results
        .into_iter()
        .filter(|c| c.len() > 1 || (c.len() == 1 && graph.has_edge(&c[0], &c[0], None)))
        .collect()
}

/// `(v, w)` pair, the result of `stripLabel`.
fn strip_label(edges: Vec<Edge>) -> Vec<(String, String)> {
    edges.into_iter().map(|e| (e.v, e.w)).collect()
}

const ACYCLICERS: &[&str] = &["greedy", "dfs", "unknown-should-still-work"];

#[test]
fn run_does_not_change_an_already_acyclic_graph() {
    for acyclicer in ACYCLICERS {
        let mut g = new_graph();
        set_acyclicer(&mut g, acyclicer);
        set_path(&mut g, &["a", "b", "d"]);
        set_path(&mut g, &["a", "c", "d"]);
        run(&mut g);
        let mut results = strip_label(g.edges());
        results.sort();
        assert_eq!(
            results,
            vec![
                ("a".to_string(), "b".to_string()),
                ("a".to_string(), "c".to_string()),
                ("b".to_string(), "d".to_string()),
                ("c".to_string(), "d".to_string()),
            ],
            "acyclicer = {acyclicer}"
        );
    }
}

#[test]
fn run_breaks_cycles_in_the_input_graph() {
    for acyclicer in ACYCLICERS {
        let mut g = new_graph();
        set_acyclicer(&mut g, acyclicer);
        set_path(&mut g, &["a", "b", "c", "d", "a"]);
        run(&mut g);
        assert_eq!(find_cycles(&g), Vec::<Vec<String>>::new(), "acyclicer = {acyclicer}");
    }
}

#[test]
fn run_creates_a_multi_edge_where_necessary() {
    for acyclicer in ACYCLICERS {
        let mut g = new_graph();
        set_acyclicer(&mut g, acyclicer);
        set_path(&mut g, &["a", "b", "a"]);
        run(&mut g);
        assert_eq!(find_cycles(&g), Vec::<Vec<String>>::new(), "acyclicer = {acyclicer}");
        if g.has_edge("a", "b", None) {
            assert_eq!(g.out_edges("a", Some("b")).unwrap().len(), 2, "acyclicer = {acyclicer}");
        } else {
            assert_eq!(g.out_edges("b", Some("a")).unwrap().len(), 2, "acyclicer = {acyclicer}");
        }
        assert_eq!(g.edge_count(), 2, "acyclicer = {acyclicer}");
    }
}

#[test]
fn undo_does_not_change_edges_where_the_original_graph_was_acyclic() {
    for acyclicer in ACYCLICERS {
        let mut g = new_graph();
        set_acyclicer(&mut g, acyclicer);
        g.set_edge(
            "a",
            "b",
            EdgeLabel {
                minlen: Some(2),
                weight: Some(3.0),
                ..Default::default()
            },
            None,
        );
        run(&mut g);
        undo(&mut g);
        assert_eq!(
            g.edge("a", "b", None),
            Some(&EdgeLabel {
                minlen: Some(2),
                weight: Some(3.0),
                ..Default::default()
            }),
            "acyclicer = {acyclicer}"
        );
        assert_eq!(g.edges().len(), 1, "acyclicer = {acyclicer}");
    }
}

#[test]
fn undo_can_restore_previously_reversed_edges() {
    for acyclicer in ACYCLICERS {
        let mut g = new_graph();
        set_acyclicer(&mut g, acyclicer);
        g.set_edge(
            "a",
            "b",
            EdgeLabel {
                minlen: Some(2),
                weight: Some(3.0),
                ..Default::default()
            },
            None,
        );
        g.set_edge(
            "b",
            "a",
            EdgeLabel {
                minlen: Some(3),
                weight: Some(4.0),
                ..Default::default()
            },
            None,
        );
        run(&mut g);
        undo(&mut g);
        assert_eq!(
            g.edge("a", "b", None),
            Some(&EdgeLabel {
                minlen: Some(2),
                weight: Some(3.0),
                ..Default::default()
            }),
            "acyclicer = {acyclicer}"
        );
        assert_eq!(
            g.edge("b", "a", None),
            Some(&EdgeLabel {
                minlen: Some(3),
                weight: Some(4.0),
                ..Default::default()
            }),
            "acyclicer = {acyclicer}"
        );
        assert_eq!(g.edges().len(), 2, "acyclicer = {acyclicer}");
    }
}

#[test]
fn greedy_prefers_to_break_cycles_at_low_weight_edges() {
    let mut g = new_graph();
    set_acyclicer(&mut g, "greedy");
    g.set_default_edge_label(EdgeLabel {
        minlen: Some(1),
        weight: Some(2.0),
        ..Default::default()
    });
    set_path(&mut g, &["a", "b", "c", "d", "a"]);
    g.set_edge(
        "c",
        "d",
        EdgeLabel {
            weight: Some(1.0),
            ..Default::default()
        },
        None,
    );
    run(&mut g);
    assert_eq!(find_cycles(&g), Vec::<Vec<String>>::new());
    assert_eq!(g.has_edge("c", "d", None), false);
}
