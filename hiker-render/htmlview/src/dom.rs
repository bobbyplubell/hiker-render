//! Arena DOM.
//!
//! Nodes live in a flat `Vec<Node>` indexed by [`NodeId`]; parent/child links
//! are stored as indices (no `Rc`/`RefCell`). The html5ever `TreeSink` impl that
//! populates this arena is owned by a later agent — [`parse_html`] is a stub.

use crate::css::stylo::data::StyloData;

use std::cell::Cell;
use std::sync::atomic::AtomicBool;

use markup5ever::QualName as MQualName;
use selectors::matching::ElementSelectorFlags;
use style::properties::PropertyDeclarationBlock;
use style::servo_arc::Arc as ServoArc;
use style::shared_lock::{Locked, SharedRwLock};
use style::Atom;
use stylo_dom::ElementState;

/// Index into [`Document::nodes`].
pub type NodeId = usize;

/// A parsed document: the node arena plus the root (the `Document` node).
///
/// `Default`/`Debug` are derived for `Document` even though `Node` is not
/// `Clone` (the Stylo per-element state is interior-mutable and non-clonable).
#[derive(Default)]
pub struct Document {
    pub nodes: Vec<Node>,
    pub root: NodeId,
    /// The `SharedRwLock` guarding every `Locked<…>` produced by the Stylo
    /// cascade (inline-style decl blocks, stylesheets). Owned by the document so
    /// the `TDocument::shared_lock()` impl can hand out a `&SharedRwLock` without
    /// any thread-local/leak hack. Lazily created on the first Stylo style pass.
    pub stylo_lock: Option<SharedRwLock>,
}

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Document")
            .field("nodes", &self.nodes)
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl Document {
    /// An empty document containing only the root `Document` node.
    pub fn new() -> Self {
        Document {
            nodes: vec![Node::new(0, NodeData::Document)],
            root: 0,
            stylo_lock: None,
        }
    }

    /// Push a new node (with no parent/children wired yet) and return its id.
    pub fn push(&mut self, data: NodeData) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(Node::new(id, data));
        id
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id]
    }

    /// Point every node's `tree`/`stylo_lock` back-pointers at this document's
    /// arena and shared lock.
    ///
    /// Must be called once the arena is fully built and will NOT grow again
    /// (Stylo's traversal borrows nodes through these raw pointers, so any
    /// reallocation of `nodes` afterwards would dangle them). The Stylo style
    /// pass ensures `stylo_lock` is `Some`, calls this just before traversing,
    /// and keeps the arena frozen for the duration. Idempotent.
    pub fn set_tree_pointers(&mut self) {
        let arena: *const Vec<Node> = &self.nodes;
        let lock: *const SharedRwLock = self
            .stylo_lock
            .as_ref()
            .map_or(std::ptr::null(), |l| l as *const _);
        for node in &mut self.nodes {
            node.tree = arena;
            node.stylo_lock = lock;
        }
    }
}

/// A single DOM node.
///
/// Beyond the arena/tree structure, each
/// node carries the per-element state Stylo's traversal needs (see
/// `src/css/stylo/`). These default to empty/null and are inert unless the Stylo
/// style pass runs, so the existing pipeline is unaffected.
///
/// `Node` is intentionally NOT `Clone`/`Copy`: `stylo_element_data` is an
/// interior-mutable cell that must not be duplicated. `Debug` is implemented
/// manually (the raw `tree` pointer and Stylo cells don't derive cleanly).
pub struct Node {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub data: NodeData,
    /// Intrinsic replaced-element size in UNZOOMED CSS px, stamped by the
    /// `<math>` pre-render pass (which sizes a `<math>` like a replaced box).
    /// `None` for everything else. Layout's replaced path reads this in lieu of
    /// a Stylo width/height (Stylo doesn't know the rendered math size).
    pub replaced_size: Option<(f32, f32)>,
    /// Forces the projected `display` for this node, bypassing Stylo's computed
    /// value. Set by the `<math>` pre-render pass to un-hide block math that lives
    /// only inside Wikipedia's hidden MathML a11y wrapper (no `<img>` fallback
    /// exists for it). `None` for everything else.
    pub display_override: Option<crate::css::values::Display>,
    /// Forces this node visible and in-flow, overriding the projected
    /// `opacity`/`position`/`width`/`height`. Set by the `<math>` pre-render pass
    /// on the chain that wraps no-fallback block math: Wikipedia hides the MathML
    /// a11y span with `opacity:0; position:absolute; width:1px; height:1px`, which
    /// this neutralizes so the math renders in place. `false` for everything else.
    pub force_visible: bool,

    // --- Stylo integration (parallel to `data`; only meaningful for elements) ---
    /// The element's qualified name as a markup5ever atom (same type Stylo's
    /// `LocalName`/`Namespace` wrap). `None` for non-elements. Set by the parser.
    pub qual_name: Option<MQualName>,
    /// Interned `id=""` attribute, `None` if absent. Set by the parser.
    pub id_atom: Option<Atom>,
    /// Parsed inline `style="…"` declaration block, built just before a Stylo
    /// style pass (see `style_document_stylo`). `None` when the element has no
    /// inline style. Stored so `TElement::style_attribute()` can hand Stylo an
    /// `ArcBorrow` into it.
    pub inline_style: Option<ServoArc<Locked<PropertyDeclarationBlock>>>,
    /// Stylo's per-element cascade result (`Arc<ComputedValues>` + flags).
    pub stylo_element_data: StyloData,
    /// Selector-matching flags Stylo accumulates during traversal.
    pub selector_flags: Cell<ElementSelectorFlags>,
    /// CSS pseudo-class element state (hover/focus/…); static for our use.
    pub element_state: ElementState,
    /// Snapshot bookkeeping for incremental restyle (unused; static documents).
    pub has_snapshot: bool,
    pub snapshot_handled: AtomicBool,
    /// Stylo dirty-descendants bit, toggled during traversal.
    pub dirty_descendants: Cell<bool>,
    /// Arena back-pointer, set after the arena `Vec` is fully built and frozen
    /// (just before a Stylo style pass). Null otherwise. See `set_tree_pointers`.
    pub tree: *const Vec<Node>,
    /// Pointer to the owning [`Document::stylo_lock`], so the `TDocument`
    /// `shared_lock()` impl can return a `&SharedRwLock` from a `&Node` without
    /// any thread-local. Set alongside `tree` for the duration of a style pass.
    pub stylo_lock: *const SharedRwLock,
}

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node")
            .field("id", &self.id)
            .field("parent", &self.parent)
            .field("children", &self.children)
            .field("data", &self.data)
            .finish_non_exhaustive()
    }
}

// Stylo's selector/traversal code requires `Element` handles (`&Node`) to be
// `Eq`/`Hash`. Identity is the node's `id` within its arena (matching the
// spike's `ToyNode` impls), so equal/hash by (id, arena pointer).
impl std::hash::Hash for Node {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write_usize(self.id);
    }
}

impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && std::ptr::eq(self.tree, other.tree)
    }
}
impl Eq for Node {}

impl Node {
    /// Construct a fresh node with all Stylo state at its inert default.
    pub fn new(id: NodeId, data: NodeData) -> Node {
        Node {
            id,
            parent: None,
            children: Vec::new(),
            data,
            replaced_size: None,
            display_override: None,
            force_visible: false,
            qual_name: None,
            id_atom: None,
            inline_style: None,
            stylo_element_data: StyloData::default(),
            selector_flags: Cell::new(ElementSelectorFlags::empty()),
            element_state: ElementState::empty(),
            has_snapshot: false,
            snapshot_handled: AtomicBool::new(false),
            dirty_descendants: Cell::new(false),
            tree: std::ptr::null(),
            stylo_lock: std::ptr::null(),
        }
    }
}

/// Node payload. Element names are lowercased; namespaces are ignored.
#[derive(Debug, Clone)]
pub enum NodeData {
    Document,
    Element {
        name: String,
        attrs: Vec<(String, String)>,
    },
    Text(String),
    Comment(String),
    Doctype,
}

impl Node {
    /// Value of attribute `name` (case-sensitive lookup; names are expected
    /// already-lowercased by the parser). Returns `None` for non-elements.
    pub fn attr(&self, name: &str) -> Option<&str> {
        match &self.data {
            NodeData::Element { attrs, .. } => attrs
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str()),
            _ => None,
        }
    }

    /// The element's tag name, or `None` for non-elements.
    pub fn tag(&self) -> Option<&str> {
        match &self.data {
            NodeData::Element { name, .. } => Some(name.as_str()),
            _ => None,
        }
    }

    /// Iterator over the whitespace-separated tokens of the `class` attribute.
    pub fn classes(&self) -> impl Iterator<Item = &str> {
        self.attr("class")
            .unwrap_or("")
            .split_whitespace()
    }

    /// The element's `id` attribute, if any.
    pub fn id_attr(&self) -> Option<&str> {
        self.attr("id")
    }

    /// True if this is an element node.
    pub fn is_element(&self) -> bool {
        matches!(self.data, NodeData::Element { .. })
    }

    /// The text content if this is a text node.
    pub fn text(&self) -> Option<&str> {
        match &self.data {
            NodeData::Text(s) => Some(s.as_str()),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// html5ever TreeSink -> arena Document
// ---------------------------------------------------------------------------

use std::borrow::Cow;
use std::cell::{Ref, RefCell};
use std::collections::HashMap;

use html5ever::interface::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::{parse_document, Attribute, ParseOpts, QualName};

/// A `TreeSink` that builds our arena [`Document`] directly.
///
/// html5ever invokes every `TreeSink` method through `&self`, so the document
/// under construction lives behind a `RefCell`. Node handles are arena
/// [`NodeId`]s.
struct ArenaSink {
    doc: RefCell<Document>,
    quirks: RefCell<QuirksMode>,
    /// `QualName` of each element node, kept so `elem_name` can hand html5ever
    /// back something implementing its `ElemName` trait. Stored separately from
    /// the arena so `elem_name` can return a `Ref` without entangling the main
    /// document borrow.
    names: RefCell<HashMap<NodeId, QualName>>,
}

impl ArenaSink {
    fn new() -> Self {
        ArenaSink {
            doc: RefCell::new(Document::new()),
            quirks: RefCell::new(QuirksMode::NoQuirks),
            names: RefCell::new(HashMap::new()),
        }
    }

    /// Append `child` as the last child of `parent`, wiring both links. If
    /// `child` already had a parent it is detached first (reparent safety).
    fn append_node(&self, parent: NodeId, child: NodeId) {
        let mut doc = self.doc.borrow_mut();
        if let Some(old) = doc.node(child).parent {
            doc.node_mut(old).children.retain(|&c| c != child);
        }
        doc.node_mut(child).parent = Some(parent);
        doc.node_mut(parent).children.push(child);
    }

    /// Append raw text to `parent`, merging into a trailing text node if present
    /// (per the `TreeSink::append` contract; whitespace is preserved verbatim).
    fn append_text(&self, parent: NodeId, text: &str) {
        let mut doc = self.doc.borrow_mut();
        if let Some(&last) = doc.node(parent).children.last() {
            if let NodeData::Text(s) = &mut doc.node_mut(last).data {
                s.push_str(text);
                return;
            }
        }
        let id = doc.push(NodeData::Text(text.to_string()));
        doc.node_mut(id).parent = Some(parent);
        doc.node_mut(parent).children.push(id);
    }

    /// Insert `new_node` immediately before `sibling` among its parent's
    /// children. `new_node` is detached from any old parent first.
    fn insert_before(&self, sibling: NodeId, new_node: NodeId) {
        let mut doc = self.doc.borrow_mut();
        let parent = doc
            .node(sibling)
            .parent
            .expect("append_before_sibling: sibling has no parent");
        if let Some(old) = doc.node(new_node).parent {
            doc.node_mut(old).children.retain(|&c| c != new_node);
        }
        let pos = doc
            .node(parent)
            .children
            .iter()
            .position(|&c| c == sibling)
            .expect("sibling not found among parent's children");
        doc.node_mut(new_node).parent = Some(parent);
        doc.node_mut(parent).children.insert(pos, new_node);
    }

    /// Insert text immediately before `sibling`, merging into the preceding
    /// text node if one exists (per the `append_before_sibling` contract).
    fn insert_text_before(&self, sibling: NodeId, text: &str) {
        let mut doc = self.doc.borrow_mut();
        let parent = doc
            .node(sibling)
            .parent
            .expect("append_before_sibling: sibling has no parent");
        let pos = doc
            .node(parent)
            .children
            .iter()
            .position(|&c| c == sibling)
            .expect("sibling not found among parent's children");
        if pos > 0 {
            let prev = doc.node(parent).children[pos - 1];
            if let NodeData::Text(s) = &mut doc.node_mut(prev).data {
                s.push_str(text);
                return;
            }
        }
        let id = doc.push(NodeData::Text(text.to_string()));
        doc.node_mut(id).parent = Some(parent);
        doc.node_mut(parent).children.insert(pos, id);
    }
}

impl TreeSink for ArenaSink {
    type Handle = NodeId;
    type Output = Document;
    // `&QualName: ElemName` (impl in markup5ever), so we lend a reference into
    // the `names` map, guarded by a `Ref`.
    type ElemName<'a> = Ref<'a, QualName>;

    fn finish(self) -> Document {
        self.doc.into_inner()
    }

    fn parse_error(&self, _msg: Cow<'static, str>) {}

    fn get_document(&self) -> NodeId {
        self.doc.borrow().root
    }

    fn elem_name<'a>(&'a self, target: &'a NodeId) -> Ref<'a, QualName> {
        // `Ref::map` keeps the borrow alive while narrowing it to the stored
        // QualName. html5ever only calls this on element handles we created.
        Ref::map(self.names.borrow(), |m| {
            m.get(target).expect("elem_name called on non-element node")
        })
    }

    fn create_element(
        &self,
        name: QualName,
        attrs: Vec<Attribute>,
        _flags: ElementFlags,
    ) -> NodeId {
        // Namespace is dropped; we keep the lowercased local name only.
        let tag = name.local.to_string().to_ascii_lowercase();
        let attrs: Vec<(String, String)> = attrs
            .into_iter()
            .map(|a| {
                (
                    a.name.local.to_string().to_ascii_lowercase(),
                    a.value.to_string(),
                )
            })
            .collect();
        // Interned id="" attribute for Stylo (case-sensitive, as the atom store).
        let id_atom = attrs
            .iter()
            .find(|(k, _)| k == "id")
            .map(|(_, v)| Atom::from(v.as_str()));
        let mut doc = self.doc.borrow_mut();
        let id = doc.push(NodeData::Element { name: tag, attrs });
        // Store the full QualName (the markup5ever atom type Stylo wraps) and the
        // interned id on the node, so the Stylo bridge can read them directly.
        doc.node_mut(id).qual_name = Some(name.clone());
        doc.node_mut(id).id_atom = id_atom;
        drop(doc);
        self.names.borrow_mut().insert(id, name);
        id
    }

    fn create_comment(&self, text: StrTendril) -> NodeId {
        self.doc.borrow_mut().push(NodeData::Comment(text.to_string()))
    }

    fn create_pi(&self, target: StrTendril, data: StrTendril) -> NodeId {
        // No PI variant in our DOM; represent it as a comment so it survives.
        let text = format!("{target} {data}");
        self.doc.borrow_mut().push(NodeData::Comment(text))
    }

    fn append(&self, parent: &NodeId, child: NodeOrText<NodeId>) {
        match child {
            NodeOrText::AppendNode(id) => self.append_node(*parent, id),
            NodeOrText::AppendText(text) => self.append_text(*parent, &text),
        }
    }

    fn append_based_on_parent_node(
        &self,
        element: &NodeId,
        prev_element: &NodeId,
        child: NodeOrText<NodeId>,
    ) {
        let has_parent = self.doc.borrow().node(*element).parent.is_some();
        if has_parent {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_before_sibling(&self, sibling: &NodeId, new_node: NodeOrText<NodeId>) {
        match new_node {
            NodeOrText::AppendNode(id) => self.insert_before(*sibling, id),
            NodeOrText::AppendText(text) => self.insert_text_before(*sibling, &text),
        }
    }

    fn append_doctype_to_document(
        &self,
        _name: StrTendril,
        _public_id: StrTendril,
        _system_id: StrTendril,
    ) {
        let mut doc = self.doc.borrow_mut();
        let root = doc.root;
        let id = doc.push(NodeData::Doctype);
        doc.node_mut(id).parent = Some(root);
        doc.node_mut(root).children.push(id);
    }

    fn get_template_contents(&self, target: &NodeId) -> NodeId {
        // Simplest correct handling: the template element is its own contents
        // container, so children append directly under it.
        *target
    }

    fn mark_script_already_started(&self, _node: &NodeId) {}

    fn same_node(&self, x: &NodeId, y: &NodeId) -> bool {
        x == y
    }

    fn set_quirks_mode(&self, mode: QuirksMode) {
        *self.quirks.borrow_mut() = mode;
    }

    fn add_attrs_if_missing(&self, target: &NodeId, attrs: Vec<Attribute>) {
        let mut doc = self.doc.borrow_mut();
        if let NodeData::Element { attrs: existing, .. } = &mut doc.node_mut(*target).data {
            for a in attrs {
                let name = a.name.local.to_string().to_ascii_lowercase();
                if !existing.iter().any(|(n, _)| *n == name) {
                    existing.push((name, a.value.to_string()));
                }
            }
        }
    }

    fn remove_from_parent(&self, target: &NodeId) {
        let mut doc = self.doc.borrow_mut();
        if let Some(parent) = doc.node(*target).parent {
            doc.node_mut(parent).children.retain(|&c| c != *target);
            doc.node_mut(*target).parent = None;
        }
    }

    fn reparent_children(&self, node: &NodeId, new_parent: &NodeId) {
        let mut doc = self.doc.borrow_mut();
        let moved: Vec<NodeId> = std::mem::take(&mut doc.node_mut(*node).children);
        for &c in &moved {
            doc.node_mut(c).parent = Some(*new_parent);
        }
        doc.node_mut(*new_parent).children.extend(moved);
    }
}

/// Parse an HTML string into an arena [`Document`] using html5ever's
/// spec-compliant tree builder driving our custom [`ArenaSink`].
///
/// Text is preserved verbatim (no whitespace collapsing — that is layout's job)
/// and `<head>`/`<script>`/`<style>` elements are kept in the tree.
pub fn parse_html(html: &str) -> Document {
    let sink = ArenaSink::new();
    parse_document(sink, ParseOpts::default()).one(html)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// First element with the given tag name, in arena order.
    fn find_tag(doc: &Document, tag: &str) -> Option<NodeId> {
        doc.nodes.iter().find_map(|n| match &n.data {
            NodeData::Element { name, .. } if name == tag => Some(n.id),
            _ => None,
        })
    }

    #[test]
    fn parses_inline_structure() {
        let doc = parse_html(r#"<p class="a">hi<b>x</b></p>"#);

        let p = find_tag(&doc, "p").expect("expected a <p> element");
        match &doc.node(p).data {
            NodeData::Element { name, attrs } => {
                assert_eq!(name, "p");
                assert_eq!(attrs, &vec![("class".to_string(), "a".to_string())]);
            }
            other => panic!("expected element, got {other:?}"),
        }

        // <p> has two children: text "hi" then element <b>.
        let children = doc.node(p).children.clone();
        assert_eq!(children.len(), 2, "p children: {children:?}");

        let text = doc.node(children[0]);
        assert!(matches!(&text.data, NodeData::Text(t) if t == "hi"));
        assert_eq!(text.parent, Some(p));

        let b = doc.node(children[1]);
        assert!(matches!(&b.data, NodeData::Element { name, .. } if name == "b"));
        assert_eq!(b.parent, Some(p));

        // <b> contains text "x".
        assert_eq!(b.children.len(), 1);
        let bx = doc.node(b.children[0]);
        assert!(matches!(&bx.data, NodeData::Text(t) if t == "x"));
        assert_eq!(bx.parent, Some(b.id));

        // html5ever wraps content in html/head/body.
        assert!(find_tag(&doc, "html").is_some());
        assert!(find_tag(&doc, "body").is_some());

        // <p> ultimately descends from the document root.
        let mut cur = p;
        while let Some(parent) = doc.node(cur).parent {
            cur = parent;
        }
        assert_eq!(cur, doc.root);
    }

    #[test]
    fn whitespace_is_preserved() {
        let doc = parse_html("<pre>a   b\n c</pre>");
        let pre = find_tag(&doc, "pre").expect("expected <pre>");
        let text = doc.node(doc.node(pre).children[0]);
        assert!(matches!(&text.data, NodeData::Text(t) if t == "a   b\n c"));
    }

    #[test]
    fn uppercase_tags_and_attrs_lowercased() {
        let doc = parse_html(r#"<DIV CLASS="X">y</DIV>"#);
        let div = find_tag(&doc, "div").expect("expected <div>");
        match &doc.node(div).data {
            NodeData::Element { name, attrs } => {
                assert_eq!(name, "div");
                assert_eq!(attrs, &vec![("class".to_string(), "X".to_string())]);
            }
            other => panic!("expected element, got {other:?}"),
        }
    }

    #[test]
    fn parses_wiki_article() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/wiki-sample/article.html");
        let html = std::fs::read_to_string(path).expect("read article.html");
        let doc = parse_html(&html);

        eprintln!("article.html node count = {}", doc.nodes.len());

        assert!(
            doc.nodes.len() > 1000,
            "expected > 1000 nodes, got {}",
            doc.nodes.len()
        );
        assert!(find_tag(&doc, "html").is_some(), "missing <html>");
        assert!(find_tag(&doc, "head").is_some(), "missing <head>");
        assert!(find_tag(&doc, "body").is_some(), "missing <body>");
    }
}
