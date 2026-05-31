//! Port of graphlib's `graph-test.ts`. Test names/structure mirror the TS so a
//! reviewer can diff against the original.
//!
//! The TS uses `any` labels; here we pick concrete label types per test
//! (`String` for the many string-label tests, and small structs/ints where the
//! TS uses object/number labels). `g.node("a") === undefined` becomes
//! `g.node("a") == None`.

use super::*;

type SG = Graph<String, String, String>;

fn s(x: &str) -> String {
    x.to_string()
}

/// `sortEdges` from the TS: named edges first (by name), then by v, then by w.
fn sort_edges(a: &Edge, b: &Edge) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (&a.name, &b.name) {
        (Some(_), None) => return Ordering::Less,
        (None, Some(_)) => return Ordering::Greater,
        (Some(an), Some(bn)) => {
            let o = an.cmp(bn);
            if o != Ordering::Equal {
                return o;
            }
        }
        (None, None) => {}
    }
    let o = a.v.cmp(&b.v);
    if o != Ordering::Equal {
        return o;
    }
    a.w.cmp(&b.w)
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

fn sorted_edges(mut v: Vec<Edge>) -> Vec<Edge> {
    v.sort_by(sort_edges);
    v
}

fn edge(v: &str, w: &str) -> Edge {
    Edge::new(v, w, None)
}
fn nedge(v: &str, w: &str, name: &str) -> Edge {
    Edge::new(v, w, Some(name.to_string()))
}

// ── initial state ─────────────────────────────────────────────────────────

#[test]
fn initial_has_no_nodes() {
    let g = SG::directed();
    assert_eq!(g.node_count(), 0);
}

#[test]
fn initial_has_no_edges() {
    let g = SG::directed();
    assert_eq!(g.edge_count(), 0);
}

#[test]
fn initial_has_no_attributes() {
    let g = SG::directed();
    assert!(g.graph().is_none());
}

#[test]
fn defaults_to_simple_directed_graph() {
    let g = SG::directed();
    assert!(g.is_directed());
    assert!(!g.is_compound());
    assert!(!g.is_multigraph());
}

#[test]
fn can_be_set_to_undirected() {
    let g = SG::new(GraphOptions {
        directed: false,
        ..Default::default()
    });
    assert!(!g.is_directed());
    assert!(!g.is_compound());
    assert!(!g.is_multigraph());
}

#[test]
fn can_be_set_to_compound() {
    let g = SG::new(GraphOptions {
        compound: true,
        ..Default::default()
    });
    assert!(g.is_directed());
    assert!(g.is_compound());
    assert!(!g.is_multigraph());
}

#[test]
fn can_be_set_to_multigraph() {
    let g = SG::new(GraphOptions {
        multigraph: true,
        ..Default::default()
    });
    assert!(g.is_directed());
    assert!(!g.is_compound());
    assert!(g.is_multigraph());
}

// ── setGraph ────────────────────────────────────────────────────────────────

#[test]
fn set_graph_get_and_set() {
    let mut g = SG::directed();
    g.set_graph(s("foo"));
    assert_eq!(g.graph(), Some(&s("foo")));
}

// ── nodes ───────────────────────────────────────────────────────────────────

#[test]
fn nodes_empty_initially() {
    let g = SG::directed();
    assert_eq!(g.nodes(), Vec::<String>::new());
}

#[test]
fn nodes_returns_ids() {
    let mut g = SG::directed();
    g.ensure_node("a");
    g.ensure_node("b");
    assert_eq!(sorted(g.nodes()), vec![s("a"), s("b")]);
}

// ── sources / sinks ─────────────────────────────────────────────────────────

#[test]
fn sources_returns_nodes_without_in_edges() {
    let mut g = SG::directed();
    g.ensure_path(&["a", "b", "c"]);
    g.ensure_node("d");
    assert_eq!(sorted(g.sources()), vec![s("a"), s("d")]);
}

#[test]
fn sinks_returns_nodes_without_out_edges() {
    let mut g = SG::directed();
    g.ensure_path(&["a", "b", "c"]);
    g.ensure_node("d");
    assert_eq!(sorted(g.sinks()), vec![s("c"), s("d")]);
}

// ── filterNodes ─────────────────────────────────────────────────────────────

#[test]
fn filter_nodes_identity() {
    let mut g = SG::directed();
    g.set_graph(s("graph label"));
    g.set_node("a", s("123"));
    g.ensure_path(&["a", "b", "c"]);
    g.set_edge("a", "c", s("456"), None);
    let g2 = g.filter_nodes(|_| true);
    assert_eq!(sorted(g2.nodes()), vec![s("a"), s("b"), s("c")]);
    assert_eq!(sorted(g2.successors("a").unwrap()), vec![s("b"), s("c")]);
    assert_eq!(sorted(g2.successors("b").unwrap()), vec![s("c")]);
    assert_eq!(g2.node("a"), Some(&s("123")));
    assert_eq!(g2.edge("a", "c", None), Some(&s("456")));
    assert_eq!(g2.graph(), Some(&s("graph label")));
}

#[test]
fn filter_nodes_empty() {
    let mut g = SG::directed();
    g.ensure_path(&["a", "b", "c"]);
    let g2 = g.filter_nodes(|_| false);
    assert_eq!(g2.nodes(), Vec::<String>::new());
    assert_eq!(g2.edges(), Vec::<Edge>::new());
}

#[test]
fn filter_nodes_only_true() {
    let mut g = SG::directed();
    g.ensure_nodes(&["a", "b"]);
    let g2 = g.filter_nodes(|v| v == "a");
    assert_eq!(g2.nodes(), vec![s("a")]);
}

#[test]
fn filter_nodes_removes_connected_edges() {
    let mut g = SG::directed();
    g.ensure_edge("a", "b", None);
    let g2 = g.filter_nodes(|v| v == "a");
    assert_eq!(sorted(g2.nodes()), vec![s("a")]);
    assert_eq!(g2.edges(), Vec::<Edge>::new());
}

#[test]
fn filter_nodes_preserves_directed() {
    let g = SG::new(GraphOptions {
        directed: true,
        ..Default::default()
    });
    assert!(g.filter_nodes(|_| true).is_directed());
    let g = SG::new(GraphOptions {
        directed: false,
        ..Default::default()
    });
    assert!(!g.filter_nodes(|_| true).is_directed());
}

#[test]
fn filter_nodes_preserves_multigraph() {
    let g = SG::new(GraphOptions {
        multigraph: true,
        ..Default::default()
    });
    assert!(g.filter_nodes(|_| true).is_multigraph());
    let g = SG::new(GraphOptions {
        multigraph: false,
        ..Default::default()
    });
    assert!(!g.filter_nodes(|_| true).is_multigraph());
}

#[test]
fn filter_nodes_preserves_compound() {
    let g = SG::new(GraphOptions {
        compound: true,
        ..Default::default()
    });
    assert!(g.filter_nodes(|_| true).is_compound());
    let g = SG::new(GraphOptions {
        compound: false,
        ..Default::default()
    });
    assert!(!g.filter_nodes(|_| true).is_compound());
}

#[test]
fn filter_nodes_includes_subgraphs() {
    let mut g = SG::new(GraphOptions {
        compound: true,
        ..Default::default()
    });
    g.set_parent("a", "parent");
    let g2 = g.filter_nodes(|_| true);
    assert_eq!(g2.parent("a"), Some(s("parent")));
}

#[test]
fn filter_nodes_includes_multi_level_subgraphs() {
    let mut g = SG::new(GraphOptions {
        compound: true,
        ..Default::default()
    });
    g.set_parent("a", "parent");
    g.set_parent("parent", "root");
    let g2 = g.filter_nodes(|_| true);
    assert_eq!(g2.parent("a"), Some(s("parent")));
    assert_eq!(g2.parent("parent"), Some(s("root")));
}

#[test]
fn filter_nodes_promotes_when_parent_excluded() {
    let mut g = SG::new(GraphOptions {
        compound: true,
        ..Default::default()
    });
    g.set_parent("a", "parent");
    g.set_parent("parent", "root");
    let g2 = g.filter_nodes(|v| v != "parent");
    assert_eq!(g2.parent("a"), Some(s("root")));
}

// ── setNodes ────────────────────────────────────────────────────────────────

#[test]
fn set_nodes_creates_multiple() {
    let mut g = SG::directed();
    g.ensure_nodes(&["a", "b", "c"]);
    assert!(g.has_node("a"));
    assert!(g.has_node("b"));
    assert!(g.has_node("c"));
}

#[test]
fn set_nodes_can_set_value_for_all() {
    let mut g = SG::directed();
    g.set_nodes(&["a", "b", "c"], s("foo"));
    assert_eq!(g.node("a"), Some(&s("foo")));
    assert_eq!(g.node("b"), Some(&s("foo")));
    assert_eq!(g.node("c"), Some(&s("foo")));
}

// ── setNode ─────────────────────────────────────────────────────────────────

#[test]
fn set_node_creates_if_absent() {
    let mut g = SG::directed();
    g.ensure_node("a");
    assert!(g.has_node("a"));
    assert_eq!(g.node("a"), None);
    assert_eq!(g.node_count(), 1);
}

#[test]
fn set_node_can_set_value() {
    let mut g = SG::directed();
    g.set_node("a", s("foo"));
    assert_eq!(g.node("a"), Some(&s("foo")));
}

#[test]
fn set_node_one_arg_does_not_change_value() {
    let mut g = SG::directed();
    g.set_node("a", s("foo"));
    g.ensure_node("a");
    assert_eq!(g.node("a"), Some(&s("foo")));
}

#[test]
fn set_node_can_remove_value_with_none() {
    let mut g = SG::directed();
    g.set_node_none("a");
    assert_eq!(g.node("a"), None);
}

#[test]
fn set_node_idempotent() {
    let mut g = SG::directed();
    g.set_node("a", s("foo"));
    g.set_node("a", s("foo"));
    assert_eq!(g.node("a"), Some(&s("foo")));
    assert_eq!(g.node_count(), 1);
}

#[test]
fn set_node_stringified_id() {
    let mut g = SG::directed();
    g.ensure_node(format!("{}", 1));
    assert!(g.has_node("1"));
    assert_eq!(g.nodes(), vec![s("1")]);
}

// ── setNodeDefaults ─────────────────────────────────────────────────────────

#[test]
fn default_node_label_set() {
    let mut g = SG::directed();
    g.set_default_node_label(s("foo"));
    g.ensure_node("a");
    assert_eq!(g.node("a"), Some(&s("foo")));
}

#[test]
fn default_node_label_does_not_change_existing() {
    let mut g = SG::directed();
    g.ensure_node("a");
    g.set_default_node_label(s("foo"));
    assert_eq!(g.node("a"), None);
}

#[test]
fn default_node_label_not_used_with_explicit() {
    let mut g = SG::directed();
    g.set_default_node_label(s("foo"));
    g.set_node("a", s("bar"));
    assert_eq!(g.node("a"), Some(&s("bar")));
}

#[test]
fn default_node_label_fn() {
    let mut g = SG::directed();
    g.set_default_node_label_fn(|_| s("foo"));
    g.ensure_node("a");
    assert_eq!(g.node("a"), Some(&s("foo")));
}

#[test]
fn default_node_label_fn_takes_name() {
    let mut g = SG::directed();
    g.set_default_node_label_fn(|v| format!("{v}-foo"));
    g.ensure_node("a");
    assert_eq!(g.node("a"), Some(&s("a-foo")));
}

// ── node ────────────────────────────────────────────────────────────────────

#[test]
fn node_undefined_if_absent() {
    let g = SG::directed();
    assert_eq!(g.node("a"), None);
}

#[test]
fn node_returns_value_if_present() {
    let mut g = SG::directed();
    g.set_node("a", s("foo"));
    assert_eq!(g.node("a"), Some(&s("foo")));
}

// ── removeNode ──────────────────────────────────────────────────────────────

#[test]
fn remove_node_noop_if_absent() {
    let mut g = SG::directed();
    assert_eq!(g.node_count(), 0);
    g.remove_node("a");
    assert!(!g.has_node("a"));
    assert_eq!(g.node_count(), 0);
}

#[test]
fn remove_node_removes_if_present() {
    let mut g = SG::directed();
    g.ensure_node("a");
    g.remove_node("a");
    assert!(!g.has_node("a"));
    assert_eq!(g.node_count(), 0);
}

#[test]
fn remove_node_idempotent() {
    let mut g = SG::directed();
    g.ensure_node("a");
    g.remove_node("a");
    g.remove_node("a");
    assert!(!g.has_node("a"));
    assert_eq!(g.node_count(), 0);
}

#[test]
fn remove_node_removes_incident_edges() {
    let mut g = SG::directed();
    g.ensure_edge("a", "b", None);
    g.ensure_edge("b", "c", None);
    g.remove_node("b");
    assert_eq!(g.edge_count(), 0);
}

#[test]
fn remove_node_removes_parent_child_relationships() {
    let mut g = SG::new(GraphOptions {
        compound: true,
        ..Default::default()
    });
    g.set_parent("c", "b");
    g.set_parent("b", "a");
    g.remove_node("b");
    assert_eq!(g.parent("b"), None);
    assert_eq!(g.children("b"), Vec::<String>::new());
    assert!(!g.children("a").contains(&s("b")));
    assert_eq!(g.parent("c"), None);
}

// ── setParent ───────────────────────────────────────────────────────────────

fn compound() -> SG {
    SG::new(GraphOptions {
        compound: true,
        ..Default::default()
    })
}

#[test]
#[should_panic]
fn set_parent_throws_if_not_compound() {
    let mut g = SG::directed();
    g.set_parent("a", "parent");
}

#[test]
fn set_parent_creates_parent_if_absent() {
    let mut g = compound();
    g.ensure_node("a");
    g.set_parent("a", "parent");
    assert!(g.has_node("parent"));
    assert_eq!(g.parent("a"), Some(s("parent")));
}

#[test]
fn set_parent_creates_child_if_absent() {
    let mut g = compound();
    g.ensure_node("parent");
    g.set_parent("a", "parent");
    assert!(g.has_node("a"));
    assert_eq!(g.parent("a"), Some(s("parent")));
}

#[test]
fn set_parent_undefined_if_never_invoked() {
    let mut g = compound();
    g.ensure_node("a");
    assert_eq!(g.parent("a"), None);
}

#[test]
fn set_parent_moves_from_previous() {
    let mut g = compound();
    g.set_parent("a", "parent");
    g.set_parent("a", "parent2");
    assert_eq!(g.parent("a"), Some(s("parent2")));
    assert_eq!(g.children("parent"), Vec::<String>::new());
    assert_eq!(g.children("parent2"), vec![s("a")]);
}

#[test]
fn set_parent_removes_parent_if_root() {
    let mut g = compound();
    g.set_parent("a", "parent");
    g.set_parent_root("a");
    assert_eq!(g.parent("a"), None);
    assert_eq!(sorted(g.children_root()), vec![s("a"), s("parent")]);
}

#[test]
fn set_parent_idempotent_remove() {
    let mut g = compound();
    g.set_parent("a", "parent");
    g.set_parent_root("a");
    g.set_parent_root("a");
    assert_eq!(g.parent("a"), None);
    assert_eq!(sorted(g.children_root()), vec![s("a"), s("parent")]);
}

#[test]
fn set_parent_stringified_id() {
    let mut g = compound();
    g.set_parent(format!("{}", 2), format!("{}", 1));
    g.set_parent(format!("{}", 3), format!("{}", 2));
    assert_eq!(g.parent("2"), Some(s("1")));
    assert_eq!(g.parent("3"), Some(s("2")));
}

#[test]
#[should_panic]
fn set_parent_preserves_tree_invariant() {
    let mut g = compound();
    g.set_parent("c", "b");
    g.set_parent("b", "a");
    g.set_parent("a", "c");
}

// ── parent ──────────────────────────────────────────────────────────────────

#[test]
fn parent_undefined_if_not_compound() {
    let g = SG::new(GraphOptions {
        compound: false,
        ..Default::default()
    });
    assert_eq!(g.parent("a"), None);
}

#[test]
fn parent_undefined_if_absent() {
    let g = compound();
    assert_eq!(g.parent("a"), None);
}

#[test]
fn parent_defaults_undefined_for_new() {
    let mut g = compound();
    g.ensure_node("a");
    assert_eq!(g.parent("a"), None);
}

#[test]
fn parent_returns_current() {
    let mut g = compound();
    g.ensure_node("a");
    g.ensure_node("parent");
    g.set_parent("a", "parent");
    assert_eq!(g.parent("a"), Some(s("parent")));
}

// ── children ────────────────────────────────────────────────────────────────

#[test]
fn children_empty_if_absent() {
    let g = compound();
    assert_eq!(g.children("a"), Vec::<String>::new());
}

#[test]
fn children_empty_for_new() {
    let mut g = compound();
    g.ensure_node("a");
    assert_eq!(g.children("a"), Vec::<String>::new());
}

#[test]
fn children_empty_noncompound_without_node() {
    let g = SG::directed();
    assert_eq!(g.children("a"), Vec::<String>::new());
}

#[test]
fn children_empty_noncompound_with_node() {
    let mut g = SG::directed();
    g.ensure_node("a");
    assert_eq!(g.children("a"), Vec::<String>::new());
}

#[test]
fn children_root_returns_all_noncompound() {
    let mut g = SG::directed();
    g.ensure_node("a");
    g.ensure_node("b");
    assert_eq!(sorted(g.children_root()), vec![s("a"), s("b")]);
}

#[test]
fn children_returns_children() {
    let mut g = compound();
    g.set_parent("a", "parent");
    g.set_parent("b", "parent");
    assert_eq!(sorted(g.children("parent")), vec![s("a"), s("b")]);
}

#[test]
fn children_root_returns_unparented() {
    let mut g = compound();
    g.ensure_node("a");
    g.ensure_node("b");
    g.ensure_node("c");
    g.ensure_node("parent");
    g.set_parent("a", "parent");
    assert_eq!(sorted(g.children_root()), vec![s("b"), s("c"), s("parent")]);
}

// ── predecessors / successors / neighbors ───────────────────────────────────

#[test]
fn predecessors_undefined_if_absent() {
    let g = SG::directed();
    assert!(g.predecessors("a").is_none());
}

#[test]
fn predecessors_returns() {
    let mut g = SG::directed();
    g.ensure_edge("a", "b", None);
    g.ensure_edge("b", "c", None);
    g.ensure_edge("a", "a", None);
    assert_eq!(sorted(g.predecessors("a").unwrap()), vec![s("a")]);
    assert_eq!(sorted(g.predecessors("b").unwrap()), vec![s("a")]);
    assert_eq!(sorted(g.predecessors("c").unwrap()), vec![s("b")]);
}

#[test]
fn successors_undefined_if_absent() {
    let g = SG::directed();
    assert!(g.successors("a").is_none());
}

#[test]
fn successors_returns() {
    let mut g = SG::directed();
    g.ensure_edge("a", "b", None);
    g.ensure_edge("b", "c", None);
    g.ensure_edge("a", "a", None);
    assert_eq!(sorted(g.successors("a").unwrap()), vec![s("a"), s("b")]);
    assert_eq!(sorted(g.successors("b").unwrap()), vec![s("c")]);
    assert_eq!(g.successors("c").unwrap(), Vec::<String>::new());
}

#[test]
fn neighbors_undefined_if_absent() {
    let g = SG::directed();
    assert!(g.neighbors("a").is_none());
}

#[test]
fn neighbors_returns() {
    let mut g = SG::directed();
    g.ensure_edge("a", "b", None);
    g.ensure_edge("b", "c", None);
    g.ensure_edge("a", "a", None);
    assert_eq!(sorted(g.neighbors("a").unwrap()), vec![s("a"), s("b")]);
    assert_eq!(sorted(g.neighbors("b").unwrap()), vec![s("a"), s("c")]);
    assert_eq!(sorted(g.neighbors("c").unwrap()), vec![s("b")]);
}

// ── isLeaf ──────────────────────────────────────────────────────────────────

#[test]
fn is_leaf_connected_undirected_false() {
    let mut g = SG::new(GraphOptions {
        directed: false,
        ..Default::default()
    });
    g.ensure_node("a");
    g.ensure_node("b");
    g.ensure_edge("a", "b", None);
    assert!(!g.is_leaf("b"));
}

#[test]
fn is_leaf_unconnected_undirected_true() {
    let mut g = SG::new(GraphOptions {
        directed: false,
        ..Default::default()
    });
    g.ensure_node("a");
    assert!(g.is_leaf("a"));
}

#[test]
fn is_leaf_unconnected_directed_true() {
    let mut g = SG::directed();
    g.ensure_node("a");
    assert!(g.is_leaf("a"));
}

#[test]
fn is_leaf_predecessor_directed_false() {
    let mut g = SG::directed();
    g.ensure_edge("a", "b", None);
    assert!(!g.is_leaf("a"));
}

#[test]
fn is_leaf_successor_directed_true() {
    let mut g = SG::directed();
    g.ensure_edge("a", "b", None);
    assert!(g.is_leaf("b"));
}

// ── edges ───────────────────────────────────────────────────────────────────

#[test]
fn edges_empty_initially() {
    let g = SG::directed();
    assert_eq!(g.edges(), Vec::<Edge>::new());
}

#[test]
fn edges_returns_keys() {
    let mut g = SG::directed();
    g.ensure_edge("a", "b", None);
    g.ensure_edge("b", "c", None);
    assert_eq!(
        sorted_edges(g.edges()),
        vec![edge("a", "b"), edge("b", "c")]
    );
}

// ── setPath ─────────────────────────────────────────────────────────────────

#[test]
fn set_path_creates_edges() {
    let mut g = SG::directed();
    g.ensure_path(&["a", "b", "c"]);
    assert!(g.has_edge("a", "b", None));
    assert!(g.has_edge("b", "c", None));
}

#[test]
fn set_path_sets_value_for_all() {
    let mut g = SG::directed();
    g.set_path(&["a", "b", "c"], s("foo"));
    assert_eq!(g.edge("a", "b", None), Some(&s("foo")));
    assert_eq!(g.edge("b", "c", None), Some(&s("foo")));
}

// ── setEdge ─────────────────────────────────────────────────────────────────

#[test]
fn set_edge_creates_if_absent() {
    let mut g = SG::directed();
    g.ensure_node("a");
    g.ensure_node("b");
    g.ensure_edge("a", "b", None);
    assert_eq!(g.edge("a", "b", None), None);
    assert!(g.has_edge("a", "b", None));
    assert!(g.has_edge_obj(&edge("a", "b")));
    assert_eq!(g.edge_count(), 1);
}

#[test]
fn set_edge_creates_nodes() {
    let mut g = SG::directed();
    g.ensure_edge("a", "b", None);
    assert!(g.has_node("a"));
    assert!(g.has_node("b"));
    assert_eq!(g.node_count(), 2);
}

#[test]
fn set_edge_creates_multi_edge() {
    let mut g = SG::new(GraphOptions {
        multigraph: true,
        ..Default::default()
    });
    g.ensure_edge("a", "b", Some("name"));
    assert!(!g.has_edge("a", "b", None));
    assert!(g.has_edge("a", "b", Some("name")));
}

#[test]
#[should_panic]
fn set_edge_named_in_non_multigraph_throws() {
    let mut g = SG::directed();
    g.ensure_edge("a", "b", Some("name"));
}

#[test]
fn set_edge_changes_value() {
    let mut g = SG::directed();
    g.set_edge("a", "b", s("foo"), None);
    g.set_edge("a", "b", s("bar"), None);
    assert_eq!(g.edge("a", "b", None), Some(&s("bar")));
}

#[test]
fn set_edge_deletes_value_with_none() {
    let mut g = SG::directed();
    g.set_edge("a", "b", s("foo"), None);
    g.set_edge_none("a", "b", None);
    assert_eq!(g.edge("a", "b", None), None);
    assert!(g.has_edge("a", "b", None));
}

#[test]
fn set_edge_changes_multi_edge_value() {
    let mut g = SG::new(GraphOptions {
        multigraph: true,
        ..Default::default()
    });
    g.set_edge("a", "b", s("value"), Some("name"));
    g.set_edge_none("a", "b", Some("name"));
    assert_eq!(g.edge("a", "b", Some("name")), None);
    assert!(g.has_edge("a", "b", Some("name")));
}

#[test]
fn set_edge_object_first_param() {
    let mut g = SG::directed();
    g.set_edge_obj(&edge("a", "b"), s("value"));
    assert_eq!(g.edge("a", "b", None), Some(&s("value")));
}

#[test]
fn set_edge_multi_edge_object_first_param() {
    let mut g = SG::new(GraphOptions {
        multigraph: true,
        ..Default::default()
    });
    g.set_edge_obj(&nedge("a", "b", "name"), s("value"));
    assert_eq!(g.edge("a", "b", Some("name")), Some(&s("value")));
}

#[test]
fn set_edge_stringified_id_1() {
    let mut g = SG::directed();
    g.set_edge(format!("{}", 1), format!("{}", 2), s("foo"), None);
    assert_eq!(g.edges(), vec![edge("1", "2")]);
    assert_eq!(g.edge("1", "2", None), Some(&s("foo")));
}

#[test]
fn set_edge_stringified_id_with_name() {
    let mut g = SG::new(GraphOptions {
        multigraph: true,
        ..Default::default()
    });
    g.set_edge(format!("{}", 1), format!("{}", 2), s("foo"), Some("3"));
    assert_eq!(g.edge("1", "2", Some("3")), Some(&s("foo")));
    assert_eq!(g.edges(), vec![nedge("1", "2", "3")]);
}

#[test]
fn set_edge_opposite_directions_distinct() {
    let mut g = SG::directed();
    g.ensure_edge("a", "b", None);
    assert!(g.has_edge("a", "b", None));
    assert!(!g.has_edge("b", "a", None));
}

#[test]
fn set_edge_undirected() {
    let mut g = SG::new(GraphOptions {
        directed: false,
        ..Default::default()
    });
    g.set_edge("a", "b", s("foo"), None);
    assert_eq!(g.edge("a", "b", None), Some(&s("foo")));
    assert_eq!(g.edge("b", "a", None), Some(&s("foo")));
}

#[test]
fn set_edge_undirected_different_order() {
    let mut g = SG::new(GraphOptions {
        directed: false,
        ..Default::default()
    });
    g.set_edge(format!("{}", 9), format!("{}", 10), s("foo"), None);
    assert!(g.has_edge("9", "10", None));
    assert!(g.has_edge("10", "9", None));
    assert_eq!(g.edge("9", "10", None), Some(&s("foo")));
}

// ── setDefaultEdgeLabel ─────────────────────────────────────────────────────

#[test]
fn default_edge_label_set() {
    let mut g = SG::directed();
    g.set_default_edge_label(s("foo"));
    g.ensure_edge("a", "b", None);
    assert_eq!(g.edge("a", "b", None), Some(&s("foo")));
}

#[test]
fn default_edge_label_does_not_change_existing() {
    let mut g = SG::directed();
    g.ensure_edge("a", "b", None);
    g.set_default_edge_label(s("foo"));
    assert_eq!(g.edge("a", "b", None), None);
}

#[test]
fn default_edge_label_not_used_with_explicit() {
    let mut g = SG::directed();
    g.set_default_edge_label(s("foo"));
    g.set_edge("a", "b", s("bar"), None);
    assert_eq!(g.edge("a", "b", None), Some(&s("bar")));
}

#[test]
fn default_edge_label_fn_takes_endpoints_and_name() {
    let mut g = SG::new(GraphOptions {
        multigraph: true,
        ..Default::default()
    });
    g.set_default_edge_label_fn(|v, w, name| {
        format!("{}-{}-{}-foo", v, w, name.unwrap_or("None"))
    });
    g.ensure_edge_obj(&nedge("a", "b", "name"));
    assert_eq!(g.edge("a", "b", Some("name")), Some(&s("a-b-name-foo")));
}

#[test]
fn default_edge_label_not_set_for_existing_multi_edge() {
    let mut g = SG::new(GraphOptions {
        multigraph: true,
        ..Default::default()
    });
    g.set_edge("a", "b", s("old"), Some("name"));
    g.set_default_edge_label(s("should not set this"));
    g.ensure_edge_obj(&nedge("a", "b", "name"));
    assert_eq!(g.edge("a", "b", Some("name")), Some(&s("old")));
}

// ── edge ────────────────────────────────────────────────────────────────────

#[test]
fn edge_undefined_if_absent() {
    let g = SG::directed();
    assert_eq!(g.edge("a", "b", None), None);
    assert_eq!(g.edge_by_obj(&edge("a", "b")), None);
    assert_eq!(g.edge("a", "b", Some("foo")), None);
}

#[test]
fn edge_returns_value_if_present() {
    let mut g = SG::directed();
    g.set_edge("a", "b", s("bar"), None);
    assert_eq!(g.edge("a", "b", None), Some(&s("bar")));
    assert_eq!(g.edge_by_obj(&edge("a", "b")), Some(&s("bar")));
    assert_eq!(g.edge("b", "a", None), None);
}

#[test]
fn edge_returns_multi_edge_value() {
    let mut g = SG::new(GraphOptions {
        multigraph: true,
        ..Default::default()
    });
    g.set_edge("a", "b", s("baz"), Some("foo"));
    assert_eq!(g.edge("a", "b", Some("foo")), Some(&s("baz")));
    assert_eq!(g.edge("a", "b", None), None);
}

#[test]
fn edge_either_direction_undirected() {
    let mut g = SG::new(GraphOptions {
        directed: false,
        ..Default::default()
    });
    g.set_edge("a", "b", s("bar"), None);
    assert_eq!(g.edge("a", "b", None), Some(&s("bar")));
    assert_eq!(g.edge("b", "a", None), Some(&s("bar")));
}

// ── removeEdge ──────────────────────────────────────────────────────────────

#[test]
fn remove_edge_noop_if_absent() {
    let mut g = SG::directed();
    g.remove_edge("a", "b", None);
    assert!(!g.has_edge("a", "b", None));
    assert_eq!(g.edge_count(), 0);
}

#[test]
fn remove_edge_by_obj() {
    let mut g = SG::new(GraphOptions {
        multigraph: true,
        ..Default::default()
    });
    g.ensure_edge_obj(&nedge("a", "b", "foo"));
    g.remove_edge_obj(&nedge("a", "b", "foo"));
    assert!(!g.has_edge("a", "b", Some("foo")));
    assert_eq!(g.edge_count(), 0);
}

#[test]
fn remove_edge_by_separate_ids() {
    let mut g = SG::new(GraphOptions {
        multigraph: true,
        ..Default::default()
    });
    g.ensure_edge_obj(&nedge("a", "b", "foo"));
    g.remove_edge("a", "b", Some("foo"));
    assert!(!g.has_edge("a", "b", Some("foo")));
    assert_eq!(g.edge_count(), 0);
}

#[test]
fn remove_edge_removes_neighbors() {
    let mut g = SG::directed();
    g.ensure_edge("a", "b", None);
    g.remove_edge("a", "b", None);
    assert_eq!(g.successors("a").unwrap(), Vec::<String>::new());
    assert_eq!(g.neighbors("a").unwrap(), Vec::<String>::new());
    assert_eq!(g.predecessors("b").unwrap(), Vec::<String>::new());
    assert_eq!(g.neighbors("b").unwrap(), Vec::<String>::new());
}

#[test]
fn remove_edge_decrements_neighbor_counts() {
    let mut g = SG::new(GraphOptions {
        multigraph: true,
        ..Default::default()
    });
    g.ensure_edge("a", "b", None);
    g.ensure_edge_obj(&nedge("a", "b", "foo"));
    g.remove_edge("a", "b", None);
    assert!(g.has_edge("a", "b", Some("foo")));
    assert_eq!(g.successors("a").unwrap(), vec![s("b")]);
    assert_eq!(g.neighbors("a").unwrap(), vec![s("b")]);
    assert_eq!(g.predecessors("b").unwrap(), vec![s("a")]);
    assert_eq!(g.neighbors("b").unwrap(), vec![s("a")]);
}

#[test]
fn remove_edge_undirected() {
    let mut g = SG::new(GraphOptions {
        directed: false,
        ..Default::default()
    });
    g.ensure_edge("h", "g", None);
    g.remove_edge("g", "h", None);
    assert_eq!(g.neighbors("g").unwrap(), Vec::<String>::new());
    assert_eq!(g.neighbors("h").unwrap(), Vec::<String>::new());
}

// ── inEdges ─────────────────────────────────────────────────────────────────

#[test]
fn in_edges_undefined_if_absent() {
    let g = SG::directed();
    assert!(g.in_edges("a", None).is_none());
}

#[test]
fn in_edges_returns_edges_pointing_at() {
    let mut g = SG::directed();
    g.ensure_edge("a", "b", None);
    g.ensure_edge("b", "c", None);
    assert_eq!(g.in_edges("a", None).unwrap(), Vec::<Edge>::new());
    assert_eq!(g.in_edges("b", None).unwrap(), vec![edge("a", "b")]);
    assert_eq!(g.in_edges("c", None).unwrap(), vec![edge("b", "c")]);
}

#[test]
fn in_edges_multigraph() {
    let mut g = SG::new(GraphOptions {
        multigraph: true,
        ..Default::default()
    });
    g.ensure_edge("a", "b", None);
    g.ensure_edge("a", "b", Some("bar"));
    g.ensure_edge("a", "b", Some("foo"));
    assert_eq!(g.in_edges("a", None).unwrap(), Vec::<Edge>::new());
    assert_eq!(
        sorted_edges(g.in_edges("b", None).unwrap()),
        vec![nedge("a", "b", "bar"), nedge("a", "b", "foo"), edge("a", "b")]
    );
}

#[test]
fn in_edges_filtered_by_node() {
    let mut g = SG::new(GraphOptions {
        multigraph: true,
        ..Default::default()
    });
    g.ensure_edge("a", "b", None);
    g.ensure_edge("a", "b", Some("foo"));
    g.ensure_edge("a", "c", None);
    g.ensure_edge("b", "c", None);
    g.ensure_edge("z", "a", None);
    g.ensure_edge("z", "b", None);
    assert_eq!(g.in_edges("a", Some("b")).unwrap(), Vec::<Edge>::new());
    assert_eq!(
        sorted_edges(g.in_edges("b", Some("a")).unwrap()),
        vec![nedge("a", "b", "foo"), edge("a", "b")]
    );
}

// ── outEdges ────────────────────────────────────────────────────────────────

#[test]
fn out_edges_undefined_if_absent() {
    let g = SG::directed();
    assert!(g.out_edges("a", None).is_none());
}

#[test]
fn out_edges_returns_edges_pointed_at() {
    let mut g = SG::directed();
    g.ensure_edge("a", "b", None);
    g.ensure_edge("b", "c", None);
    assert_eq!(g.out_edges("a", None).unwrap(), vec![edge("a", "b")]);
    assert_eq!(g.out_edges("b", None).unwrap(), vec![edge("b", "c")]);
    assert_eq!(g.out_edges("c", None).unwrap(), Vec::<Edge>::new());
}

#[test]
fn out_edges_multigraph() {
    let mut g = SG::new(GraphOptions {
        multigraph: true,
        ..Default::default()
    });
    g.ensure_edge("a", "b", None);
    g.ensure_edge("a", "b", Some("bar"));
    g.ensure_edge("a", "b", Some("foo"));
    assert_eq!(
        sorted_edges(g.out_edges("a", None).unwrap()),
        vec![nedge("a", "b", "bar"), nedge("a", "b", "foo"), edge("a", "b")]
    );
    assert_eq!(g.out_edges("b", None).unwrap(), Vec::<Edge>::new());
}

#[test]
fn out_edges_filtered_by_node() {
    let mut g = SG::new(GraphOptions {
        multigraph: true,
        ..Default::default()
    });
    g.ensure_edge("a", "b", None);
    g.ensure_edge("a", "b", Some("foo"));
    g.ensure_edge("a", "c", None);
    g.ensure_edge("b", "c", None);
    g.ensure_edge("z", "a", None);
    g.ensure_edge("z", "b", None);
    assert_eq!(
        sorted_edges(g.out_edges("a", Some("b")).unwrap()),
        vec![nedge("a", "b", "foo"), edge("a", "b")]
    );
    assert_eq!(g.out_edges("b", Some("a")).unwrap(), Vec::<Edge>::new());
}

// ── nodeEdges ───────────────────────────────────────────────────────────────

#[test]
fn node_edges_undefined_if_absent() {
    let g = SG::directed();
    assert!(g.node_edges("a", None).is_none());
}

#[test]
fn node_edges_returns_incident() {
    let mut g = SG::directed();
    g.ensure_edge("a", "b", None);
    g.ensure_edge("b", "c", None);
    assert_eq!(g.node_edges("a", None).unwrap(), vec![edge("a", "b")]);
    assert_eq!(
        sorted_edges(g.node_edges("b", None).unwrap()),
        vec![edge("a", "b"), edge("b", "c")]
    );
    assert_eq!(g.node_edges("c", None).unwrap(), vec![edge("b", "c")]);
}

#[test]
fn node_edges_multigraph() {
    let mut g = SG::new(GraphOptions {
        multigraph: true,
        ..Default::default()
    });
    g.ensure_edge("a", "b", None);
    g.ensure_edge_obj(&nedge("a", "b", "bar"));
    g.ensure_edge_obj(&nedge("a", "b", "foo"));
    assert_eq!(
        sorted_edges(g.node_edges("a", None).unwrap()),
        vec![nedge("a", "b", "bar"), nedge("a", "b", "foo"), edge("a", "b")]
    );
    assert_eq!(
        sorted_edges(g.node_edges("b", None).unwrap()),
        vec![nedge("a", "b", "bar"), nedge("a", "b", "foo"), edge("a", "b")]
    );
}

#[test]
fn node_edges_filtered_between_nodes() {
    let mut g = SG::new(GraphOptions {
        multigraph: true,
        ..Default::default()
    });
    g.ensure_edge("a", "b", None);
    g.ensure_edge_obj(&nedge("a", "b", "foo"));
    g.ensure_edge("a", "c", None);
    g.ensure_edge("b", "c", None);
    g.ensure_edge("z", "a", None);
    g.ensure_edge("z", "b", None);
    assert_eq!(
        sorted_edges(g.node_edges("a", Some("b")).unwrap()),
        vec![nedge("a", "b", "foo"), edge("a", "b")]
    );
    assert_eq!(
        sorted_edges(g.node_edges("b", Some("a")).unwrap()),
        vec![nedge("a", "b", "foo"), edge("a", "b")]
    );
}

// ── object-label edge test (TS uses {foo:"bar"}) ────────────────────────────

#[test]
fn edge_with_struct_label() {
    #[derive(Clone, Debug, PartialEq)]
    struct L {
        foo: String,
    }
    let mut g: Graph<(), (), L> = Graph::directed();
    g.set_edge("a", "b", L { foo: s("bar") }, None);
    assert_eq!(g.edge("a", "b", None), Some(&L { foo: s("bar") }));
    assert_eq!(g.edge("b", "a", None), None);
}
