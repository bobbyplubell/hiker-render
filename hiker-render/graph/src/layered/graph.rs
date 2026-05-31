//! Compound multigraph data structure — a faithful Rust port of graphlib's
//! `Graph` class (`graphlib/lib/graph.ts`), the substrate dagre runs on.
//!
//! This is **Step 0** of the dagre port (see `DAGRE_PORT.md`). It ports the
//! subset of graphlib's `Graph` that dagre actually relies on, plus enough of
//! the surrounding surface to make graphlib's `graph-test.ts` pass.
//!
//! # Fidelity notes
//!
//! * **Node ids are `String`** ([`NodeId`]). dagre generates string dummy-node
//!   ids (`"_d3"`, border nodes, …) throughout, so integer ids are not an
//!   option.
//! * **Edges are keyed exactly as graphlib does**: a stringified key
//!   `v\u{1}w\u{1}name` ([`EDGE_KEY_DELIM`]), with the unnamed-edge sentinel
//!   `\u{0}` ([`DEFAULT_EDGE_NAME`]) and direction normalization (`v`/`w`
//!   swapped when `!directed && v > w`) so iteration order and dedup behave
//!   identically to the original.
//! * **Insertion order is preserved** wherever graphlib guarantees it (JS
//!   object key order). We use an [`OrderMap`] (a `Vec<String>` order list +
//!   `HashMap`) so `nodes()`, `edges()`, `successors()`, `inEdges()`, … iterate
//!   in insertion order. dagre's determinism depends on this.
//! * **Incremental bookkeeping** mirrors graphlib: in/out edge maps,
//!   predecessor/successor *count* maps, and (for compound graphs) parent /
//!   children maps are maintained indexes, not recomputed scans.
//!
//! # Generics
//!
//! `Graph<G, N, E>` mirrors graphlib's `Graph<GraphLabel, NodeLabel,
//! EdgeLabel>`. Later dagre modules instantiate the graph with different label
//! types (e.g. the network-simplex tree graph), so generic labels are kept
//! rather than a single monolithic label struct.
//!
//! Where graphlib returns `undefined` for a missing-or-unset label, this port
//! returns `Option<&T>`: node/edge labels are stored as `Option<N>` /
//! `Option<E>` so the *key* (membership) is independent of the *label* (which
//! may be `None`), exactly matching graphlib where the key exists with value
//! `undefined`.

use std::collections::HashMap;

/// Node identifier. dagre uses string ids everywhere (including generated
/// dummy/border node ids), so this is a `String`.
pub type NodeId = String;

const DEFAULT_EDGE_NAME: &str = "\u{0}";
const GRAPH_NODE: &str = "\u{0}";
const EDGE_KEY_DELIM: &str = "\u{1}";

/// Construction options, mirroring graphlib's `GraphOptions`.
///
/// Defaults (matching graphlib): `directed = true`, `multigraph = false`,
/// `compound = false`.
#[derive(Clone, Copy, Debug)]
pub struct GraphOptions {
    pub directed: bool,
    pub multigraph: bool,
    pub compound: bool,
}

impl Default for GraphOptions {
    fn default() -> Self {
        Self {
            directed: true,
            multigraph: false,
            compound: false,
        }
    }
}

/// An edge descriptor — graphlib's `Edge` (`{ v, w, name? }`).
///
/// `name` distinguishes parallel edges in a multigraph. Two edges are equal
/// iff all three fields match.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Edge {
    pub v: NodeId,
    pub w: NodeId,
    pub name: Option<String>,
}

impl Edge {
    pub fn new(v: impl Into<String>, w: impl Into<String>, name: Option<String>) -> Self {
        Self {
            v: v.into(),
            w: w.into(),
            name,
        }
    }
}

/// An insertion-ordered string-keyed map.
///
/// JS objects iterate keys in insertion order, and `delete` preserves the
/// order of the remaining keys. dagre depends on this determinism, so we
/// replicate it: a `Vec<String>` order list plus a `HashMap` for O(1) lookup.
#[derive(Clone, Debug)]
struct OrderMap<V> {
    order: Vec<String>,
    map: HashMap<String, V>,
}

impl<V> Default for OrderMap<V> {
    fn default() -> Self {
        Self {
            order: Vec::new(),
            map: HashMap::new(),
        }
    }
}

impl<V> OrderMap<V> {
    fn new() -> Self {
        Self::default()
    }

    fn len(&self) -> usize {
        self.order.len()
    }

    fn contains_key(&self, k: &str) -> bool {
        self.map.contains_key(k)
    }

    fn get(&self, k: &str) -> Option<&V> {
        self.map.get(k)
    }

    fn get_mut(&mut self, k: &str) -> Option<&mut V> {
        self.map.get_mut(k)
    }

    /// Insert, preserving the position of an existing key (JS object semantics:
    /// assigning to an existing key keeps its original insertion position).
    fn insert(&mut self, k: String, v: V) {
        if !self.map.contains_key(&k) {
            self.order.push(k.clone());
        }
        self.map.insert(k, v);
    }

    /// Delete, preserving the order of the remaining keys (JS `delete`).
    fn remove(&mut self, k: &str) -> Option<V> {
        let removed = self.map.remove(k);
        if removed.is_some() {
            if let Some(pos) = self.order.iter().position(|e| e == k) {
                self.order.remove(pos);
            }
        }
        removed
    }

    /// Keys in insertion order.
    fn keys(&self) -> impl Iterator<Item = &String> {
        self.order.iter()
    }

    /// Values in insertion order.
    fn values(&self) -> impl Iterator<Item = &V> {
        self.order.iter().map(move |k| &self.map[k])
    }
}

/// A compound multigraph — port of graphlib's `Graph`.
///
/// `G` / `N` / `E` are the graph-label / node-label / edge-label types,
/// matching graphlib's `Graph<GraphLabel, NodeLabel, EdgeLabel>`.
pub struct Graph<G = (), N = (), E = ()> {
    is_directed: bool,
    is_multigraph: bool,
    is_compound: bool,

    /// Label for the graph itself. `None` until [`set_graph`](Graph::set_graph).
    label: Option<G>,

    /// v -> node label. `None` models graphlib's `undefined` label (the key is
    /// still present, so the node exists).
    nodes: OrderMap<Option<N>>,
    /// w -> (edge-id -> Edge): edges pointing *at* w.
    in_edges: HashMap<NodeId, OrderMap<Edge>>,
    /// w -> (v -> count): predecessor multiplicities.
    preds: HashMap<NodeId, OrderMap<usize>>,
    /// v -> (edge-id -> Edge): edges pointing *out of* v.
    out_edges: HashMap<NodeId, OrderMap<Edge>>,
    /// v -> (w -> count): successor multiplicities.
    sucs: HashMap<NodeId, OrderMap<usize>>,
    /// edge-id -> Edge.
    edge_objs: OrderMap<Edge>,
    /// edge-id -> edge label. `None` models graphlib's `undefined` label (the
    /// key is still present, so the edge exists).
    edge_labels: OrderMap<Option<E>>,

    node_count: usize,
    edge_count: usize,

    /// v -> parent (compound only). The root sentinel is [`GRAPH_NODE`].
    parent: HashMap<NodeId, NodeId>,
    /// v -> (child -> ()) (compound only).
    children: HashMap<NodeId, OrderMap<()>>,

    default_node_label: Box<dyn Fn(&str) -> Option<N>>,
    default_edge_label: Box<dyn Fn(&str, &str, Option<&str>) -> Option<E>>,
}

// ── id helpers (port of edgeArgsToId / edgeArgsToObj / edgeObjToId) ────────

fn edge_args_to_id(is_directed: bool, v_: &str, w_: &str, name: Option<&str>) -> String {
    let (v, w) = if !is_directed && v_ > w_ {
        (w_, v_)
    } else {
        (v_, w_)
    };
    format!(
        "{}{}{}{}{}",
        v,
        EDGE_KEY_DELIM,
        w,
        EDGE_KEY_DELIM,
        name.unwrap_or(DEFAULT_EDGE_NAME)
    )
}

fn edge_args_to_obj(is_directed: bool, v_: &str, w_: &str, name: Option<&str>) -> Edge {
    let (v, w) = if !is_directed && v_ > w_ {
        (w_, v_)
    } else {
        (v_, w_)
    };
    // graphlib only stores `name` on the obj when it is truthy (non-empty).
    let name = match name {
        Some(n) if !n.is_empty() => Some(n.to_string()),
        _ => None,
    };
    Edge {
        v: v.to_string(),
        w: w.to_string(),
        name,
    }
}

fn edge_obj_to_id(is_directed: bool, e: &Edge) -> String {
    edge_args_to_id(is_directed, &e.v, &e.w, e.name.as_deref())
}

fn increment_or_init(map: &mut OrderMap<usize>, k: &str) {
    match map.get_mut(k) {
        Some(c) => *c += 1,
        None => map.insert(k.to_string(), 1),
    }
}

fn decrement_or_remove(map: &mut OrderMap<usize>, k: &str) {
    if let Some(c) = map.get_mut(k) {
        *c -= 1;
        if *c == 0 {
            map.remove(k);
        }
    }
}

impl<G, N, E> Graph<G, N, E> {
    /// Construct a graph with the given options.
    pub fn new(opts: GraphOptions) -> Self {
        let mut children: HashMap<NodeId, OrderMap<()>> = HashMap::new();
        if opts.compound {
            children.insert(GRAPH_NODE.to_string(), OrderMap::new());
        }
        Self {
            is_directed: opts.directed,
            is_multigraph: opts.multigraph,
            is_compound: opts.compound,
            label: None,
            nodes: OrderMap::new(),
            in_edges: HashMap::new(),
            preds: HashMap::new(),
            out_edges: HashMap::new(),
            sucs: HashMap::new(),
            edge_objs: OrderMap::new(),
            edge_labels: OrderMap::new(),
            node_count: 0,
            edge_count: 0,
            parent: HashMap::new(),
            children,
            default_node_label: Box::new(|_| None),
            default_edge_label: Box::new(|_, _, _| None),
        }
    }

    /// A simple directed graph (graphlib's `new Graph()` default).
    pub fn directed() -> Self {
        Self::new(GraphOptions::default())
    }

    // ── flags ─────────────────────────────────────────────────────────────

    pub fn is_directed(&self) -> bool {
        self.is_directed
    }
    pub fn is_multigraph(&self) -> bool {
        self.is_multigraph
    }
    pub fn is_compound(&self) -> bool {
        self.is_compound
    }

    // ── graph label ───────────────────────────────────────────────────────

    /// Set the graph's own label. Chainable.
    pub fn set_graph(&mut self, label: G) -> &mut Self {
        self.label = Some(label);
        self
    }

    /// The graph label, or `None` if never set.
    pub fn graph(&self) -> Option<&G> {
        self.label.as_ref()
    }
    pub fn graph_mut(&mut self) -> Option<&mut G> {
        self.label.as_mut()
    }

    // ── default-label factories ───────────────────────────────────────────

    /// Set a constant default node label. Chainable.
    pub fn set_default_node_label(&mut self, label: N) -> &mut Self
    where
        N: Clone + 'static,
    {
        self.default_node_label = Box::new(move |_| Some(label.clone()));
        self
    }

    /// Set a default node-label factory (receives the node id). Chainable.
    pub fn set_default_node_label_fn(&mut self, f: impl Fn(&str) -> N + 'static) -> &mut Self {
        self.default_node_label = Box::new(move |v| Some(f(v)));
        self
    }

    /// Set a constant default edge label. Chainable.
    pub fn set_default_edge_label(&mut self, label: E) -> &mut Self
    where
        E: Clone + 'static,
    {
        self.default_edge_label = Box::new(move |_, _, _| Some(label.clone()));
        self
    }

    /// Set a default edge-label factory (receives `v`, `w`, `name`). Chainable.
    pub fn set_default_edge_label_fn(
        &mut self,
        f: impl Fn(&str, &str, Option<&str>) -> E + 'static,
    ) -> &mut Self {
        self.default_edge_label = Box::new(move |v, w, name| Some(f(v, w, name)));
        self
    }

    // ── node queries ──────────────────────────────────────────────────────

    pub fn node_count(&self) -> usize {
        self.node_count
    }

    /// All node ids, in insertion order.
    pub fn nodes(&self) -> Vec<NodeId> {
        self.nodes.keys().cloned().collect()
    }

    /// Nodes with no in-edges.
    pub fn sources(&self) -> Vec<NodeId> {
        self.nodes
            .keys()
            .filter(|v| self.in_edges[*v].len() == 0)
            .cloned()
            .collect()
    }

    /// Nodes with no out-edges.
    pub fn sinks(&self) -> Vec<NodeId> {
        self.nodes
            .keys()
            .filter(|v| self.out_edges[*v].len() == 0)
            .cloned()
            .collect()
    }

    pub fn has_node(&self, name: &str) -> bool {
        self.nodes.contains_key(name)
    }

    /// The label of node `name`, or `None` if the node is absent (or its label
    /// is `None`).
    pub fn node(&self, name: &str) -> Option<&N> {
        self.nodes.get(name).and_then(|o| o.as_ref())
    }
    pub fn node_mut(&mut self, name: &str) -> Option<&mut N> {
        self.nodes.get_mut(name).and_then(|o| o.as_mut())
    }

    /// Create or update node `name`, assigning `label`. If the node already
    /// exists its label is overwritten. Chainable.
    pub fn set_node(&mut self, name: impl Into<String>, label: N) -> &mut Self {
        let name = name.into();
        if self.nodes.contains_key(&name) {
            self.nodes.insert(name, Some(label));
            return self;
        }
        self.create_node(name, Some(label));
        self
    }

    /// Set node `name`'s label to `None` (graphlib `setNode(v, undefined)`).
    /// Creates the node if absent. Chainable.
    pub fn set_node_none(&mut self, name: impl Into<String>) -> &mut Self {
        let name = name.into();
        if self.nodes.contains_key(&name) {
            self.nodes.insert(name, None);
            return self;
        }
        self.create_node(name, None);
        self
    }

    /// Create node `name` if absent, using the default node-label factory; if
    /// it exists, leave it untouched (graphlib's 1-arg `setNode`). Chainable.
    pub fn ensure_node(&mut self, name: impl Into<String>) -> &mut Self {
        let name = name.into();
        if self.nodes.contains_key(&name) {
            return self;
        }
        let label = (self.default_node_label)(&name);
        self.create_node(name, label);
        self
    }

    fn create_node(&mut self, name: String, label: Option<N>) {
        self.nodes.insert(name.clone(), label);
        self.init_node_bookkeeping(name);
    }

    fn init_node_bookkeeping(&mut self, name: String) {
        if self.is_compound {
            self.parent.insert(name.clone(), GRAPH_NODE.to_string());
            self.children.insert(name.clone(), OrderMap::new());
            self.children
                .get_mut(GRAPH_NODE)
                .unwrap()
                .insert(name.clone(), ());
        }
        self.in_edges.insert(name.clone(), OrderMap::new());
        self.preds.insert(name.clone(), OrderMap::new());
        self.out_edges.insert(name.clone(), OrderMap::new());
        self.sucs.insert(name.clone(), OrderMap::new());
        self.node_count += 1;
    }

    /// `setNodes(names, label)` — set each node to a clone of `label`.
    pub fn set_nodes(&mut self, names: &[&str], label: N) -> &mut Self
    where
        N: Clone,
    {
        for v in names {
            self.set_node(*v, label.clone());
        }
        self
    }

    /// `setNodes(names)` — create each node with the default label.
    pub fn ensure_nodes(&mut self, names: &[&str]) -> &mut Self {
        for v in names {
            self.ensure_node(*v);
        }
        self
    }

    /// Remove node `name` and all incident edges; no-op if absent. Chainable.
    pub fn remove_node(&mut self, name: &str) -> &mut Self {
        if !self.has_node(name) {
            return self;
        }
        let mut incident: Vec<String> = Vec::new();
        if let Some(m) = self.in_edges.get(name) {
            incident.extend(m.keys().cloned());
        }
        if let Some(m) = self.out_edges.get(name) {
            incident.extend(m.keys().cloned());
        }
        for e in incident {
            if let Some(edge) = self.edge_objs.get(&e).cloned() {
                self.remove_edge_obj(&edge);
            }
        }

        self.nodes.remove(name);
        if self.is_compound {
            self.remove_from_parents_child_list(name);
            self.parent.remove(name);
            let kids = self.children(name);
            for child in kids {
                self.set_parent_root(&child);
            }
            self.children.remove(name);
        }
        self.in_edges.remove(name);
        self.preds.remove(name);
        self.out_edges.remove(name);
        self.sucs.remove(name);
        self.node_count -= 1;
        self
    }

    // ── compound: parent / children ───────────────────────────────────────

    /// Set the parent of `v` to `parent`. Creates both nodes if absent.
    ///
    /// # Panics
    /// Panics if the graph is not compound, or if the assignment would create a
    /// cycle (matching graphlib's thrown errors).
    pub fn set_parent(&mut self, v: impl Into<String>, parent: impl Into<String>) -> &mut Self {
        let v = v.into();
        let parent = parent.into();
        if !self.is_compound {
            panic!("Cannot set parent in a non-compound graph");
        }
        // cycle check: walk parent's ancestor chain; if we hit v, it's a cycle.
        let mut ancestor: Option<String> = Some(parent.clone());
        while let Some(a) = ancestor {
            if a == v {
                panic!("Setting {parent} as parent of {v} would create a cycle");
            }
            ancestor = self.parent_internal(&a);
        }
        self.ensure_node(parent.clone());
        self.ensure_node(v.clone());
        self.remove_from_parents_child_list(&v);
        self.parent.insert(v.clone(), parent.clone());
        self.children.get_mut(&parent).unwrap().insert(v, ());
        self
    }

    /// Remove `v`'s parent (re-parent to the graph root). Creates `v` if absent.
    /// Equivalent to graphlib's `setParent(v)` / `setParent(v, undefined)`.
    pub fn set_parent_root(&mut self, v: &str) -> &mut Self {
        if !self.is_compound {
            panic!("Cannot set parent in a non-compound graph");
        }
        self.ensure_node(v.to_string());
        self.remove_from_parents_child_list(v);
        self.parent.insert(v.to_string(), GRAPH_NODE.to_string());
        self.children
            .get_mut(GRAPH_NODE)
            .unwrap()
            .insert(v.to_string(), ());
        self
    }

    /// Internal parent lookup that returns the raw stored parent (which may be
    /// the [`GRAPH_NODE`] root sentinel).
    fn parent_internal(&self, v: &str) -> Option<String> {
        self.parent.get(v).cloned()
    }

    /// The parent of `v`, or `None` if `v` is at the root / absent / the graph
    /// is non-compound.
    pub fn parent(&self, v: &str) -> Option<NodeId> {
        if self.is_compound {
            if let Some(p) = self.parent.get(v) {
                if p != GRAPH_NODE {
                    return Some(p.clone());
                }
            }
        }
        None
    }

    /// Direct children of `v`.
    ///
    /// For a non-compound graph: the root sentinel returns all nodes; a present
    /// or absent node returns `[]`. Use [`children_root`](Graph::children_root)
    /// for the root.
    pub fn children(&self, v: &str) -> Vec<NodeId> {
        if self.is_compound {
            if let Some(c) = self.children.get(v) {
                return c.keys().cloned().collect();
            }
            return Vec::new();
        }
        if v == GRAPH_NODE {
            return self.nodes();
        }
        Vec::new()
    }

    /// Children of the graph root (graphlib `children()` / `children(undefined)`).
    pub fn children_root(&self) -> Vec<NodeId> {
        self.children(GRAPH_NODE)
    }

    fn remove_from_parents_child_list(&mut self, v: &str) {
        if let Some(p) = self.parent.get(v).cloned() {
            if let Some(siblings) = self.children.get_mut(&p) {
                siblings.remove(v);
            }
        }
    }

    // ── adjacency ─────────────────────────────────────────────────────────

    /// Predecessors of `v` (insertion order), or `None` if `v` is absent.
    pub fn predecessors(&self, v: &str) -> Option<Vec<NodeId>> {
        self.preds.get(v).map(|m| m.keys().cloned().collect())
    }

    /// Successors of `v` (insertion order), or `None` if `v` is absent.
    pub fn successors(&self, v: &str) -> Option<Vec<NodeId>> {
        self.sucs.get(v).map(|m| m.keys().cloned().collect())
    }

    /// Predecessors ∪ successors of `v` (predecessors first, dedup'd preserving
    /// first-seen order), or `None` if `v` is absent.
    pub fn neighbors(&self, v: &str) -> Option<Vec<NodeId>> {
        let preds = self.predecessors(v)?;
        let mut out = preds.clone();
        let succs = self.successors(v).unwrap_or_default();
        for s in succs {
            if !out.contains(&s) {
                out.push(s);
            }
        }
        Some(out)
    }

    /// Whether `v` is a leaf (no successors for directed graphs, no neighbors
    /// for undirected).
    pub fn is_leaf(&self, v: &str) -> bool {
        let neighbors = if self.is_directed {
            self.successors(v)
        } else {
            self.neighbors(v)
        };
        neighbors.map(|n| n.is_empty()).unwrap_or(true)
    }

    // ── edge queries ──────────────────────────────────────────────────────

    pub fn edge_count(&self) -> usize {
        self.edge_count
    }

    /// All edges, in insertion order.
    pub fn edges(&self) -> Vec<Edge> {
        self.edge_objs.values().cloned().collect()
    }

    fn edge_id(&self, v: &str, w: &str, name: Option<&str>) -> String {
        edge_args_to_id(self.is_directed, v, w, name)
    }

    pub fn has_edge(&self, v: &str, w: &str, name: Option<&str>) -> bool {
        self.edge_labels.contains_key(&self.edge_id(v, w, name))
    }

    pub fn has_edge_obj(&self, e: &Edge) -> bool {
        self.edge_labels
            .contains_key(&edge_obj_to_id(self.is_directed, e))
    }

    /// The label of edge `(v, w, name)`, or `None` if absent (or its label is
    /// `None`).
    pub fn edge(&self, v: &str, w: &str, name: Option<&str>) -> Option<&E> {
        self.edge_labels
            .get(&self.edge_id(v, w, name))
            .and_then(|o| o.as_ref())
    }

    pub fn edge_mut(&mut self, v: &str, w: &str, name: Option<&str>) -> Option<&mut E> {
        let id = self.edge_id(v, w, name);
        self.edge_labels.get_mut(&id).and_then(|o| o.as_mut())
    }

    pub fn edge_by_obj(&self, e: &Edge) -> Option<&E> {
        self.edge_labels
            .get(&edge_obj_to_id(self.is_directed, e))
            .and_then(|o| o.as_ref())
    }

    /// Create or update edge `(v, w, name)`, assigning `label`. Creates the
    /// endpoint nodes if absent. Chainable.
    ///
    /// # Panics
    /// Panics if `name` is `Some` but the graph is not a multigraph (matching
    /// graphlib).
    pub fn set_edge(
        &mut self,
        v: impl Into<String>,
        w: impl Into<String>,
        label: E,
        name: Option<&str>,
    ) -> &mut Self {
        let v = v.into();
        let w = w.into();
        let id = edge_args_to_id(self.is_directed, &v, &w, name);
        if self.edge_labels.contains_key(&id) {
            self.edge_labels.insert(id, Some(label));
            return self;
        }
        if name.is_some() && !self.is_multigraph {
            panic!("Cannot set a named edge when isMultigraph = false");
        }
        self.create_edge(id, v, w, name, Some(label));
        self
    }

    /// Set edge `(v, w, name)`'s label to `None` (graphlib `setEdge(v, w,
    /// undefined)`). Creates the edge / its endpoints if absent. Chainable.
    pub fn set_edge_none(
        &mut self,
        v: impl Into<String>,
        w: impl Into<String>,
        name: Option<&str>,
    ) -> &mut Self {
        let v = v.into();
        let w = w.into();
        let id = edge_args_to_id(self.is_directed, &v, &w, name);
        if self.edge_labels.contains_key(&id) {
            self.edge_labels.insert(id, None);
            return self;
        }
        if name.is_some() && !self.is_multigraph {
            panic!("Cannot set a named edge when isMultigraph = false");
        }
        self.create_edge(id, v, w, name, None);
        self
    }

    /// Create edge `(v, w, name)` if absent, using the default edge-label
    /// factory; if it exists, leave its label untouched. Chainable.
    pub fn ensure_edge(
        &mut self,
        v: impl Into<String>,
        w: impl Into<String>,
        name: Option<&str>,
    ) -> &mut Self {
        let v = v.into();
        let w = w.into();
        let id = edge_args_to_id(self.is_directed, &v, &w, name);
        if self.edge_labels.contains_key(&id) {
            return self;
        }
        if name.is_some() && !self.is_multigraph {
            panic!("Cannot set a named edge when isMultigraph = false");
        }
        let label = (self.default_edge_label)(&v, &w, name);
        self.create_edge(id, v, w, name, label);
        self
    }

    /// `setEdge(edgeObj, label)`.
    pub fn set_edge_obj(&mut self, e: &Edge, label: E) -> &mut Self {
        self.set_edge(e.v.clone(), e.w.clone(), label, e.name.as_deref())
    }

    /// `setEdge(edgeObj)` with the default label.
    pub fn ensure_edge_obj(&mut self, e: &Edge) -> &mut Self {
        self.ensure_edge(e.v.clone(), e.w.clone(), e.name.as_deref())
    }

    fn create_edge(
        &mut self,
        id: String,
        v: String,
        w: String,
        name: Option<&str>,
        label: Option<E>,
    ) {
        self.ensure_node(v.clone());
        self.ensure_node(w.clone());
        self.edge_labels.insert(id.clone(), label);
        self.register_edge(id, v, w, name);
    }

    fn register_edge(&mut self, id: String, v: String, w: String, name: Option<&str>) {
        let edge_obj = edge_args_to_obj(self.is_directed, &v, &w, name);
        let nv = edge_obj.v.clone();
        let nw = edge_obj.w.clone();
        self.edge_objs.insert(id.clone(), edge_obj.clone());
        increment_or_init(self.preds.get_mut(&nw).unwrap(), &nv);
        increment_or_init(self.sucs.get_mut(&nv).unwrap(), &nw);
        self.in_edges
            .get_mut(&nw)
            .unwrap()
            .insert(id.clone(), edge_obj.clone());
        self.out_edges.get_mut(&nv).unwrap().insert(id, edge_obj);
        self.edge_count += 1;
    }

    /// `setPath([n0, n1, ...], label)` — connect consecutive nodes, cloning
    /// `label` for each edge. Chainable.
    pub fn set_path(&mut self, nodes: &[&str], label: E) -> &mut Self
    where
        E: Clone,
    {
        for pair in nodes.windows(2) {
            self.set_edge(pair[0], pair[1], label.clone(), None);
        }
        self
    }

    /// `setPath([n0, n1, ...])` — connect consecutive nodes with the default
    /// edge label. Chainable.
    pub fn ensure_path(&mut self, nodes: &[&str]) -> &mut Self {
        for pair in nodes.windows(2) {
            self.ensure_edge(pair[0], pair[1], None);
        }
        self
    }

    pub fn remove_edge(&mut self, v: &str, w: &str, name: Option<&str>) -> &mut Self {
        let id = self.edge_id(v, w, name);
        self.remove_edge_by_id(&id);
        self
    }

    pub fn remove_edge_obj(&mut self, e: &Edge) -> &mut Self {
        let id = edge_obj_to_id(self.is_directed, e);
        self.remove_edge_by_id(&id);
        self
    }

    fn remove_edge_by_id(&mut self, id: &str) {
        let edge = match self.edge_objs.get(id).cloned() {
            Some(e) => e,
            None => return,
        };
        let v = edge.v.clone();
        let w = edge.w.clone();
        self.edge_labels.remove(id);
        self.edge_objs.remove(id);
        if let Some(p) = self.preds.get_mut(&w) {
            decrement_or_remove(p, &v);
        }
        if let Some(sm) = self.sucs.get_mut(&v) {
            decrement_or_remove(sm, &w);
        }
        if let Some(m) = self.in_edges.get_mut(&w) {
            m.remove(id);
        }
        if let Some(m) = self.out_edges.get_mut(&v) {
            m.remove(id);
        }
        self.edge_count -= 1;
    }

    /// In-edges of `v`, optionally filtered to those from `u`. `None` if `v` is
    /// absent. For undirected graphs, defers to [`node_edges`](Graph::node_edges).
    pub fn in_edges(&self, v: &str, u: Option<&str>) -> Option<Vec<Edge>> {
        if self.is_directed {
            let m = self.in_edges.get(v)?;
            let edges: Vec<Edge> = m.values().cloned().collect();
            Some(Self::filter_edges(edges, v, u))
        } else {
            self.node_edges(v, u)
        }
    }

    /// Out-edges of `v`, optionally filtered to those to `w`. `None` if `v` is
    /// absent. For undirected graphs, defers to [`node_edges`](Graph::node_edges).
    pub fn out_edges(&self, v: &str, w: Option<&str>) -> Option<Vec<Edge>> {
        if self.is_directed {
            let m = self.out_edges.get(v)?;
            let edges: Vec<Edge> = m.values().cloned().collect();
            Some(Self::filter_edges(edges, v, w))
        } else {
            self.node_edges(v, w)
        }
    }

    /// All edges incident to `v` (in then out, in insertion order), optionally
    /// filtered to those between `v` and `w` regardless of direction. `None` if
    /// `v` is absent.
    pub fn node_edges(&self, v: &str, w: Option<&str>) -> Option<Vec<Edge>> {
        if !self.has_node(v) {
            return None;
        }
        // graphlib does {...in, ...out}: in-edges first, then out-edges; a
        // self-loop edge id appears in both and stays at its in-edge position.
        let mut seen: Vec<String> = Vec::new();
        let mut combined: Vec<Edge> = Vec::new();
        if let Some(m) = self.in_edges.get(v) {
            for k in m.keys() {
                seen.push(k.clone());
                combined.push(m.get(k).unwrap().clone());
            }
        }
        if let Some(m) = self.out_edges.get(v) {
            for k in m.keys() {
                if !seen.contains(k) {
                    seen.push(k.clone());
                    combined.push(m.get(k).unwrap().clone());
                }
            }
        }
        Some(Self::filter_edges(combined, v, w))
    }

    fn filter_edges(edges: Vec<Edge>, local: &str, remote: Option<&str>) -> Vec<Edge> {
        match remote {
            None => edges,
            Some(r) => edges
                .into_iter()
                .filter(|e| (e.v == local && e.w == r) || (e.v == r && e.w == local))
                .collect(),
        }
    }
}

// ── filterNodes ───────────────────────────────────────────────────────────

impl<G, N, E> Graph<G, N, E>
where
    G: Clone,
    N: Clone,
    E: Clone,
{
    /// Create a new graph with nodes filtered by `filter`. Edges incident to
    /// rejected nodes are dropped. In a compound graph, a node whose parent is
    /// rejected is promoted to the nearest surviving ancestor.
    pub fn filter_nodes(&self, filter: impl Fn(&str) -> bool) -> Graph<G, N, E> {
        let mut copy = Graph::new(GraphOptions {
            directed: self.is_directed,
            multigraph: self.is_multigraph,
            compound: self.is_compound,
        });

        if let Some(l) = self.graph() {
            copy.set_graph(l.clone());
        }

        for v in self.nodes.keys() {
            if filter(v) {
                match self.nodes.get(v).and_then(|o| o.as_ref()) {
                    Some(label) => {
                        copy.set_node(v.clone(), label.clone());
                    }
                    None => {
                        copy.set_node_none(v.clone());
                    }
                }
            }
        }

        for e in self.edge_objs.values() {
            if copy.has_node(&e.v) && copy.has_node(&e.w) {
                match self.edge_by_obj(e) {
                    Some(label) => {
                        copy.set_edge_obj(e, label.clone());
                    }
                    None => {
                        copy.set_edge_none(e.v.clone(), e.w.clone(), e.name.as_deref());
                    }
                }
            }
        }

        if self.is_compound {
            let mut parents: HashMap<String, Option<String>> = HashMap::new();
            let copy_nodes = copy.nodes();
            for v in &copy_nodes {
                let p = self.find_surviving_parent(v, &copy, &mut parents);
                match p {
                    Some(parent) => {
                        copy.set_parent(v.clone(), parent);
                    }
                    None => {
                        copy.set_parent_root(v);
                    }
                }
            }
        }

        copy
    }

    fn find_surviving_parent(
        &self,
        v: &str,
        copy: &Graph<G, N, E>,
        parents: &mut HashMap<String, Option<String>>,
    ) -> Option<String> {
        let parent = self.parent(v);
        match parent {
            None => {
                parents.insert(v.to_string(), None);
                None
            }
            Some(p) => {
                if copy.has_node(&p) {
                    parents.insert(v.to_string(), Some(p.clone()));
                    Some(p)
                } else if let Some(cached) = parents.get(&p) {
                    cached.clone()
                } else {
                    self.find_surviving_parent(&p, copy, parents)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
