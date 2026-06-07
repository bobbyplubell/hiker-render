//! Port of `dagre/test/greedy-fas-test.ts`. Test names mirror the TS so a
//! reviewer can diff against the original oracle.

use super::*;
use crate::layered::graph::{Edge, Graph, GraphOptions};

/// Test graph: edge label is the (numeric) weight, mirroring graphlib's
/// `setEdge("a", "b", 2)`.
type TestGraph = Graph<(), (), f64>;

fn directed() -> TestGraph {
    Graph::new(GraphOptions::default())
}

fn multigraph() -> TestGraph {
    Graph::new(GraphOptions {
        directed: true,
        multigraph: true,
        compound: false,
    })
}

/// `setPath` for a label-less edge graph (edge label defaults to 1.0, unused by
/// the default weight fn).
fn set_path(g: &mut TestGraph, path: &[&str]) {
    for pair in path.windows(2) {
        g.set_edge(pair[0], pair[1], 1.0, None);
    }
}

/// Tarjan SCC + cycle filter, ported from graphlib's `alg.findCycles`
/// (`tarjan.ts` + `find-cycles.ts`). Test-only oracle.
fn find_cycles<G, N, E>(graph: &Graph<G, N, E>) -> Vec<Vec<String>> {
    struct State<'a, G, N, E> {
        graph: &'a Graph<G, N, E>,
        index: usize,
        stack: Vec<String>,
        visited: std::collections::HashMap<String, (bool, usize, usize)>, // on_stack, lowlink, index
        results: Vec<Vec<String>>,
    }

    fn dfs<G, N, E>(s: &mut State<G, N, E>, v: &str) {
        let idx = s.index;
        s.visited.insert(v.to_string(), (true, idx, idx));
        s.index += 1;
        s.stack.push(v.to_string());

        for w in s.graph.successors(v).unwrap_or_default() {
            if !s.visited.contains_key(&w) {
                dfs(s, &w);
                let w_low = s.visited[&w].1;
                let entry = s.visited.get_mut(v).unwrap();
                entry.1 = entry.1.min(w_low);
            } else if s.visited[&w].0 {
                let w_index = s.visited[&w].2;
                let entry = s.visited.get_mut(v).unwrap();
                entry.1 = entry.1.min(w_index);
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
        .filter(|cmpt| {
            cmpt.len() > 1 || (cmpt.len() == 1 && graph.has_edge(&cmpt[0], &cmpt[0], None))
        })
        .collect()
}

/// `checkFAS(g, fas)` from the TS: removing the FAS makes the graph acyclic and
/// the FAS size respects the Eades–Lin–Smyth performance bound.
fn check_fas(g: &mut TestGraph, fas: &[Edge]) {
    let n = g.node_count() as i64;
    let m = g.edge_count() as i64;
    for edge in fas {
        g.remove_edge(&edge.v, &edge.w, edge.name.as_deref());
    }
    assert_eq!(find_cycles(g), Vec::<Vec<String>>::new());
    // floor(m/2) - floor(n/6); using integer division (floor for non-negatives).
    assert!((fas.len() as i64) <= (m / 2) - (n / 6));
}

/// `weightFn(g)` from the TS — reads the numeric edge label.
fn weight_fn(g: &TestGraph) -> impl Fn(&Edge) -> f64 + '_ {
    move |e: &Edge| *g.edge_by_obj(e).unwrap()
}

#[test]
fn returns_the_empty_set_for_empty_graphs() {
    let g = directed();
    assert_eq!(greedy_fas(&g, None), Vec::<Edge>::new());
}

#[test]
fn returns_the_empty_set_for_single_node_graphs() {
    let mut g = directed();
    g.ensure_node("a");
    assert_eq!(greedy_fas(&g, None), Vec::<Edge>::new());
}

#[test]
fn returns_an_empty_set_if_the_input_graph_is_acyclic() {
    let mut g = directed();
    g.set_edge("a", "b", 1.0, None);
    g.set_edge("b", "c", 1.0, None);
    g.set_edge("b", "d", 1.0, None);
    g.set_edge("a", "e", 1.0, None);
    assert_eq!(greedy_fas(&g, None), Vec::<Edge>::new());
}

#[test]
fn returns_a_single_edge_with_a_simple_cycle() {
    let mut g = directed();
    g.set_edge("a", "b", 1.0, None);
    g.set_edge("b", "a", 1.0, None);
    let fas = greedy_fas(&g, None);
    check_fas(&mut g, &fas);
}

#[test]
fn returns_a_single_edge_in_a_4_node_cycle() {
    let mut g = directed();
    g.set_edge("n1", "n2", 1.0, None);
    set_path(&mut g, &["n2", "n3", "n4", "n5", "n2"]);
    g.set_edge("n3", "n5", 1.0, None);
    g.set_edge("n4", "n2", 1.0, None);
    g.set_edge("n4", "n6", 1.0, None);
    let fas = greedy_fas(&g, None);
    check_fas(&mut g, &fas);
}

#[test]
fn returns_two_edges_for_two_4_node_cycles() {
    let mut g = directed();
    g.set_edge("n1", "n2", 1.0, None);
    set_path(&mut g, &["n2", "n3", "n4", "n5", "n2"]);
    g.set_edge("n3", "n5", 1.0, None);
    g.set_edge("n4", "n2", 1.0, None);
    g.set_edge("n4", "n6", 1.0, None);
    set_path(&mut g, &["n6", "n7", "n8", "n9", "n6"]);
    g.set_edge("n7", "n9", 1.0, None);
    g.set_edge("n8", "n6", 1.0, None);
    g.set_edge("n8", "n10", 1.0, None);
    let fas = greedy_fas(&g, None);
    check_fas(&mut g, &fas);
}

#[test]
fn works_with_arbitrarily_weighted_edges() {
    let mut g1 = directed();
    g1.set_edge("n1", "n2", 2.0, None);
    g1.set_edge("n2", "n1", 1.0, None);
    let wf = weight_fn(&g1);
    assert_eq!(greedy_fas(&g1, Some(&wf)), vec![Edge::new("n2", "n1", None)]);

    let mut g2 = directed();
    g2.set_edge("n1", "n2", 1.0, None);
    g2.set_edge("n2", "n1", 2.0, None);
    let wf = weight_fn(&g2);
    assert_eq!(greedy_fas(&g2, Some(&wf)), vec![Edge::new("n1", "n2", None)]);
}

#[test]
fn works_for_multigraphs() {
    let mut g = multigraph();
    g.set_edge("a", "b", 5.0, Some("foo"));
    g.set_edge("b", "a", 2.0, Some("bar"));
    g.set_edge("b", "a", 2.0, Some("baz"));
    let wf = weight_fn(&g);
    let mut fas = greedy_fas(&g, Some(&wf));
    fas.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(
        fas,
        vec![
            Edge::new("b", "a", Some("bar".to_string())),
            Edge::new("b", "a", Some("baz".to_string())),
        ]
    );
}
