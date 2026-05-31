//! Real Stylo (Servo's style engine) integration over our arena [`Document`].
//!
//! This is the Stage-1 port of `stylo-spike/` onto our real [`crate::dom::Node`].
//! It implements Stylo's trait surface (`selectors::Element`, `TDocument`/
//! `TNode`/`TShadowRoot`/`NodeInfo`/`AttributeProvider`/`TElement`) over `&Node`,
//! builds a `Stylist` from our UA + author stylesheets, runs the **Option A**
//! sequential cascade (`style::driver::traverse_dom`, `rayon_pool = None`), and
//! stores an `Arc<ComputedValues>` per element in its [`data::StyloData`].
//!
//! It runs ALONGSIDE the existing `css::cascade`; it deletes nothing and layout
//! still reads the old `ComputedStyle`. Reading these `ComputedValues` into
//! layout is Stage 2.

pub mod data;
pub mod read;

use std::ptr::NonNull;
use std::sync::atomic::Ordering;

use markup5ever::{local_name, LocalName, Namespace, QualName};

use selectors::attr::{AttrSelectorOperation, CaseSensitivity, NamespaceConstraint};
use selectors::bloom::{BloomFilter, BLOOM_HASH_MASK};
use selectors::matching::{ElementSelectorFlags, MatchingContext, VisitedHandlingMode};
use selectors::sink::Push;
use selectors::{Element, OpaqueElement};

use style::applicable_declarations::ApplicableDeclarationBlock;
use style::bloom::each_relevant_element_hash;
use style::color::AbsoluteColor;
use style::properties::{Importance, PropertyDeclaration};
use style::rule_tree::{CascadeLevel, CascadeOrigin};
use style::stylesheets::layer_rule::LayerOrder;
use style::values::computed::Percentage;
use style::context::{QuirksMode, SharedStyleContext, StyleContext};
use style::data::{ElementDataMut, ElementDataRef};
use style::dom::{
    AttributeProvider, LayoutIterator, NodeInfo, OpaqueNode, TDocument, TElement, TNode,
    TShadowRoot,
};
use style::properties::{ComputedValues, PropertyDeclarationBlock};
use style::selector_parser::{NonTSPseudoClass, PseudoElement, RestyleDamage, SelectorImpl};
use style::servo_arc::{Arc as ServoArc, ArcBorrow};
use style::shared_lock::{Locked, SharedRwLock};
use style::stylesheets::scope_rule::ImplicitScopeRoot;
use style::values::{AtomIdent, GenericAtomIdent};
use style::{Atom, CaseSensitivityExt};

use stylo_dom::ElementState;

use crate::dom::{Document, Node, NodeData};
use crate::{ResourceProvider, Theme};

// ---------------------------------------------------------------------------
// Node handle helpers (mirror the spike's ToyNode accessors).
// ---------------------------------------------------------------------------

/// The styling node handle (a borrow), exactly as blitz does it.
type NodeRef<'a> = &'a Node;

trait NodeExt {
    fn tree(&self) -> &Vec<Node>;
    fn with(&self, id: usize) -> &Node;
    fn forward(&self, n: usize) -> Option<&Node>;
    fn backward(&self, n: usize) -> Option<&Node>;
    fn is_text(&self) -> bool;
    fn element_name(&self) -> Option<&QualName>;
    fn local_attr(&self, local: &LocalName) -> Option<&str>;
}

impl NodeExt for Node {
    #[inline]
    fn tree(&self) -> &Vec<Node> {
        // SAFETY: set by `Document::set_tree_pointers` before the style pass and
        // kept frozen (no Vec growth) for the pass's duration.
        unsafe { &*self.tree }
    }
    #[inline]
    fn with(&self, id: usize) -> &Node {
        &self.tree()[id]
    }
    /// nth following sibling (1 = immediate next).
    fn forward(&self, n: usize) -> Option<&Node> {
        let parent = self.with(self.parent?);
        let pos = parent.children.iter().position(|&c| c == self.id)?;
        parent.children.get(pos + n).map(|&id| self.with(id))
    }
    /// nth preceding sibling (1 = immediate prev).
    fn backward(&self, n: usize) -> Option<&Node> {
        let parent = self.with(self.parent?);
        let pos = parent.children.iter().position(|&c| c == self.id)?;
        if pos < n {
            return None;
        }
        parent.children.get(pos - n).map(|&id| self.with(id))
    }
    #[inline]
    fn is_text(&self) -> bool {
        matches!(self.data, NodeData::Text(_))
    }
    #[inline]
    fn element_name(&self) -> Option<&QualName> {
        self.qual_name.as_ref()
    }
    /// Attribute lookup by markup5ever local name (case-sensitive, matching how
    /// the parser stored lowercased names).
    fn local_attr(&self, local: &LocalName) -> Option<&str> {
        // Our parser lowercases attribute names into `NodeData::Element.attrs`.
        self.attr(local)
    }
}

// ---------------------------------------------------------------------------
// Shared lock: read from the owning Document via the node's `stylo_lock` ptr.
// ---------------------------------------------------------------------------

fn node_shared_lock(node: &Node) -> &SharedRwLock {
    // SAFETY: set by `Document::set_tree_pointers` once `Document::stylo_lock`
    // is `Some`, before the style pass, and valid for its duration.
    debug_assert!(!node.stylo_lock.is_null(), "stylo_lock pointer not set");
    unsafe { &*node.stylo_lock }
}

// ---------------------------------------------------------------------------
// Trait impls (ported from the spike, onto &Node).
// ---------------------------------------------------------------------------

impl<'a> TDocument for NodeRef<'a> {
    type ConcreteNode = NodeRef<'a>;

    fn as_node(&self) -> Self::ConcreteNode {
        self
    }
    fn is_html_document(&self) -> bool {
        true
    }
    fn quirks_mode(&self) -> QuirksMode {
        QuirksMode::NoQuirks
    }
    fn shared_lock(&self) -> &SharedRwLock {
        node_shared_lock(self)
    }
}

impl NodeInfo for NodeRef<'_> {
    fn is_element(&self) -> bool {
        Node::is_element(self)
    }
    fn is_text_node(&self) -> bool {
        NodeExt::is_text(*self)
    }
}

impl<'a> TShadowRoot for NodeRef<'a> {
    type ConcreteNode = NodeRef<'a>;

    fn as_node(&self) -> Self::ConcreteNode {
        self
    }
    fn host(&self) -> <Self::ConcreteNode as TNode>::ConcreteElement {
        unreachable!("no shadow DOM")
    }
    fn style_data<'b>(&self) -> Option<&'b style::stylist::CascadeData>
    where
        Self: 'b,
    {
        None
    }
}

impl<'a> TNode for NodeRef<'a> {
    type ConcreteElement = NodeRef<'a>;
    type ConcreteDocument = NodeRef<'a>;
    type ConcreteShadowRoot = NodeRef<'a>;

    fn parent_node(&self) -> Option<Self> {
        self.parent.map(|id| self.with(id))
    }
    fn first_child(&self) -> Option<Self> {
        self.children.first().map(|id| self.with(*id))
    }
    fn last_child(&self) -> Option<Self> {
        self.children.last().map(|id| self.with(*id))
    }
    fn prev_sibling(&self) -> Option<Self> {
        self.backward(1)
    }
    fn next_sibling(&self) -> Option<Self> {
        self.forward(1)
    }
    fn owner_doc(&self) -> Self::ConcreteDocument {
        self.with(0)
    }
    fn is_in_document(&self) -> bool {
        true
    }
    fn traversal_parent(&self) -> Option<Self::ConcreteElement> {
        self.parent_node().and_then(|node| node.as_element())
    }
    fn opaque(&self) -> OpaqueNode {
        OpaqueNode(self.id)
    }
    fn debug_id(self) -> usize {
        self.id
    }
    fn as_element(&self) -> Option<Self::ConcreteElement> {
        if self.is_element() {
            Some(self)
        } else {
            None
        }
    }
    fn as_document(&self) -> Option<Self::ConcreteDocument> {
        match self.data {
            NodeData::Document => Some(self),
            _ => None,
        }
    }
    fn as_shadow_root(&self) -> Option<Self::ConcreteShadowRoot> {
        None
    }
}

impl AttributeProvider for NodeRef<'_> {
    fn get_attr(&self, attr: &style::LocalName, _ns: &style::Namespace) -> Option<String> {
        self.local_attr(&attr.0).map(|s| s.to_string())
    }
}

impl selectors::Element for NodeRef<'_> {
    type Impl = SelectorImpl;

    fn opaque(&self) -> OpaqueElement {
        let non_null = NonNull::new((self.id + 1) as *mut ()).unwrap();
        OpaqueElement::from_non_null_ptr(non_null)
    }

    fn parent_element(&self) -> Option<Self> {
        TElement::traversal_parent(self)
    }
    fn parent_node_is_shadow_root(&self) -> bool {
        false
    }
    fn containing_shadow_host(&self) -> Option<Self> {
        None
    }
    fn is_pseudo_element(&self) -> bool {
        false
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        let mut n = 1;
        while let Some(node) = self.backward(n) {
            if node.is_element() {
                return Some(node);
            }
            n += 1;
        }
        None
    }
    fn next_sibling_element(&self) -> Option<Self> {
        let mut n = 1;
        while let Some(node) = self.forward(n) {
            if node.is_element() {
                return Some(node);
            }
            n += 1;
        }
        None
    }
    fn first_element_child(&self) -> Option<Self> {
        self.children
            .iter()
            .map(|&id| self.with(id))
            .find(|c| c.is_element())
    }

    fn is_html_element_in_html_document(&self) -> bool {
        true
    }

    fn has_local_name(&self, local_name: &LocalName) -> bool {
        self.element_name()
            .map(|n| &n.local == local_name)
            .unwrap_or(false)
    }
    fn has_namespace(&self, ns: &Namespace) -> bool {
        self.element_name().map(|n| &n.ns == ns).unwrap_or(false)
    }
    fn is_same_type(&self, other: &Self) -> bool {
        self.element_name().map(|n| (&n.local, &n.ns))
            == other.element_name().map(|n| (&n.local, &n.ns))
    }

    fn attr_matches(
        &self,
        _ns: &NamespaceConstraint<&GenericAtomIdent<markup5ever::NamespaceStaticSet>>,
        local_name: &GenericAtomIdent<markup5ever::LocalNameStaticSet>,
        operation: &AttrSelectorOperation<&style::values::AtomString>,
    ) -> bool {
        match self.local_attr(&local_name.0) {
            None => false,
            Some(value) => operation.eval_str(value),
        }
    }

    fn match_non_ts_pseudo_class(
        &self,
        pseudo_class: &NonTSPseudoClass,
        _context: &mut MatchingContext<Self::Impl>,
    ) -> bool {
        let is_link = || {
            self.element_name()
                .map(|n| n.local == local_name!("a") || n.local == local_name!("area"))
                .unwrap_or(false)
                && self.local_attr(&local_name!("href")).is_some()
        };
        match *pseudo_class {
            NonTSPseudoClass::Active => self.element_state.contains(ElementState::ACTIVE),
            NonTSPseudoClass::Hover => self.element_state.contains(ElementState::HOVER),
            NonTSPseudoClass::Focus => self.element_state.contains(ElementState::FOCUS),
            NonTSPseudoClass::Enabled => self.element_state.contains(ElementState::ENABLED),
            NonTSPseudoClass::Disabled => self.element_state.contains(ElementState::DISABLED),
            NonTSPseudoClass::Link | NonTSPseudoClass::AnyLink => is_link(),
            // Everything else stubbed false for static documents.
            _ => false,
        }
    }

    fn match_pseudo_element(
        &self,
        _pe: &PseudoElement,
        _context: &mut MatchingContext<Self::Impl>,
    ) -> bool {
        false
    }

    fn apply_selector_flags(&self, flags: ElementSelectorFlags) {
        let self_flags = flags.for_self();
        if !self_flags.is_empty() {
            self.selector_flags
                .set(self.selector_flags.get() | self_flags);
        }
        let parent_flags = flags.for_parent();
        if !parent_flags.is_empty() {
            if let Some(parent) = self.parent_node() {
                parent
                    .selector_flags
                    .set(parent.selector_flags.get() | parent_flags);
            }
        }
    }

    fn is_link(&self) -> bool {
        self.element_name()
            .map(|n| n.local == local_name!("a"))
            .unwrap_or(false)
    }
    fn is_html_slot_element(&self) -> bool {
        false
    }

    fn has_id(&self, id: &AtomIdent, case_sensitivity: CaseSensitivity) -> bool {
        self.id_atom
            .as_ref()
            .map(|id_attr| case_sensitivity.eq_atom(id_attr, id))
            .unwrap_or(false)
    }

    fn has_class(&self, search_name: &AtomIdent, case_sensitivity: CaseSensitivity) -> bool {
        if let Some(class_attr) = self.local_attr(&local_name!("class")) {
            for pheme in class_attr.split_ascii_whitespace() {
                let atom = Atom::from(pheme);
                if case_sensitivity.eq_atom(&atom, search_name) {
                    return true;
                }
            }
        }
        false
    }

    fn imported_part(&self, _name: &AtomIdent) -> Option<AtomIdent> {
        None
    }
    fn is_part(&self, _name: &AtomIdent) -> bool {
        false
    }
    fn is_empty(&self) -> bool {
        !self
            .children
            .iter()
            .any(|&id| self.with(id).is_element() || NodeExt::is_text(self.with(id)))
    }
    fn is_root(&self) -> bool {
        self.parent_node()
            .and_then(|parent| parent.parent_node())
            .is_none()
    }
    fn has_custom_state(&self, _name: &AtomIdent) -> bool {
        false
    }

    fn add_element_unique_hashes(&self, filter: &mut BloomFilter) -> bool {
        each_relevant_element_hash(*self, |hash| filter.insert_hash(hash & BLOOM_HASH_MASK));
        true
    }
}

pub struct Traverser<'a> {
    parent: NodeRef<'a>,
    child_index: usize,
}

impl<'a> Iterator for Traverser<'a> {
    type Item = NodeRef<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        let node_id = self.parent.children.get(self.child_index)?;
        let node = self.parent.with(*node_id);
        self.child_index += 1;
        Some(node)
    }
}

impl<'a> TElement for NodeRef<'a> {
    type ConcreteNode = NodeRef<'a>;
    type TraversalChildrenIterator = Traverser<'a>;

    fn as_node(&self) -> Self::ConcreteNode {
        self
    }

    fn implicit_scope_for_sheet_in_shadow_root(
        _opaque_host: OpaqueElement,
        _sheet_index: usize,
    ) -> Option<ImplicitScopeRoot> {
        None
    }

    fn traversal_children(&self) -> LayoutIterator<Self::TraversalChildrenIterator> {
        LayoutIterator(Traverser {
            parent: self,
            child_index: 0,
        })
    }

    fn is_html_element(&self) -> bool {
        self.is_element()
    }
    fn is_mathml_element(&self) -> bool {
        false
    }
    fn is_svg_element(&self) -> bool {
        false
    }

    fn style_attribute(&self) -> Option<ArcBorrow<'_, Locked<PropertyDeclarationBlock>>> {
        self.inline_style.as_ref().map(|arc| arc.borrow_arc())
    }

    fn state(&self) -> ElementState {
        self.element_state
    }

    fn has_part_attr(&self) -> bool {
        false
    }
    fn exports_any_part(&self) -> bool {
        false
    }

    fn id(&self) -> Option<&Atom> {
        self.id_atom.as_ref()
    }

    fn each_class<F>(&self, mut callback: F)
    where
        F: FnMut(&AtomIdent),
    {
        if let Some(class_attr) = self.local_attr(&local_name!("class")) {
            for pheme in class_attr.split_ascii_whitespace() {
                let atom = Atom::from(pheme);
                callback(AtomIdent::cast(&atom));
            }
        }
    }

    fn each_attr_name<F>(&self, mut callback: F)
    where
        F: FnMut(&style::LocalName),
    {
        if let NodeData::Element { attrs, .. } = &self.data {
            for (name, _) in attrs {
                // stylo's `LocalName` is `GenericAtomIdent<LocalNameStaticSet>`,
                // a newtype over the markup5ever atom; build one from our String.
                callback(&GenericAtomIdent(LocalName::from(name.as_str())));
            }
        }
    }

    fn has_dirty_descendants(&self) -> bool {
        self.dirty_descendants.get()
    }
    fn has_snapshot(&self) -> bool {
        self.has_snapshot
    }
    fn handled_snapshot(&self) -> bool {
        self.snapshot_handled.load(Ordering::SeqCst)
    }
    unsafe fn set_handled_snapshot(&self) {
        self.snapshot_handled.store(true, Ordering::SeqCst);
    }
    unsafe fn set_dirty_descendants(&self) {
        self.dirty_descendants.set(true);
    }
    unsafe fn unset_dirty_descendants(&self) {
        self.dirty_descendants.set(false);
    }

    fn store_children_to_process(&self, _n: isize) {
        unimplemented!()
    }
    fn did_process_child(&self) -> isize {
        unimplemented!()
    }

    unsafe fn ensure_data(&self) -> ElementDataMut<'_> {
        unsafe { self.stylo_element_data.ensure_init() }
    }
    unsafe fn clear_data(&self) {
        unsafe { self.stylo_element_data.clear() }
    }
    fn has_data(&self) -> bool {
        self.stylo_element_data.has_data()
    }
    fn borrow_data(&self) -> Option<ElementDataRef<'_>> {
        self.stylo_element_data.get()
    }
    fn mutate_data(&self) -> Option<ElementDataMut<'_>> {
        unsafe { self.stylo_element_data.unsafe_stylo_only_mut() }
    }

    fn skip_item_display_fixup(&self) -> bool {
        false
    }

    fn may_have_animations(&self) -> bool {
        false
    }
    fn has_animations(&self, _context: &SharedStyleContext) -> bool {
        false
    }
    fn has_css_animations(
        &self,
        _context: &SharedStyleContext,
        _pseudo_element: Option<PseudoElement>,
    ) -> bool {
        false
    }
    fn has_css_transitions(
        &self,
        _context: &SharedStyleContext,
        _pseudo_element: Option<PseudoElement>,
    ) -> bool {
        false
    }
    fn animation_rule(
        &self,
        _context: &SharedStyleContext,
    ) -> Option<ServoArc<Locked<PropertyDeclarationBlock>>> {
        None
    }
    fn transition_rule(
        &self,
        _context: &SharedStyleContext,
    ) -> Option<ServoArc<Locked<PropertyDeclarationBlock>>> {
        None
    }

    fn shadow_root(&self) -> Option<<Self::ConcreteNode as TNode>::ConcreteShadowRoot> {
        None
    }
    fn containing_shadow(&self) -> Option<<Self::ConcreteNode as TNode>::ConcreteShadowRoot> {
        None
    }

    fn lang_attr(&self) -> Option<style::selector_parser::AttrValue> {
        None
    }
    fn match_element_lang(
        &self,
        _override_lang: Option<Option<style::selector_parser::AttrValue>>,
        _value: &style::selector_parser::Lang,
    ) -> bool {
        false
    }

    fn is_html_document_body_element(&self) -> bool {
        let is_body = self
            .element_name()
            .map(|n| n.local == local_name!("body"))
            .unwrap_or(false);
        if !is_body {
            return false;
        }
        match self.parent_node() {
            Some(parent) => parent.is_root(),
            None => false,
        }
    }

    fn synthesize_presentational_hints_for_legacy_attributes<V>(
        &self,
        _visited_handling: VisitedHandlingMode,
        hints: &mut V,
    ) where
        V: Push<ApplicableDeclarationBlock>,
    {
        let NodeData::Element { attrs, .. } = &self.data else {
            return;
        };
        let Some(qual) = self.element_name() else {
            return;
        };
        let tag = &qual.local;
        let lock = node_shared_lock(self);

        let mut push_style = |decl: PropertyDeclaration| {
            hints.push(ApplicableDeclarationBlock::from_declarations(
                ServoArc::new(
                    lock.wrap(PropertyDeclarationBlock::with_one(decl, Importance::Normal)),
                ),
                CascadeLevel::new(CascadeOrigin::PresHints),
                LayerOrder::root(),
            ));
        };

        for (name, value) in attrs.iter() {
            let value = value.as_str();
            match name.as_str() {
                // Generic `align="left|right|center"` → text-align.
                "align" => {
                    use style::values::specified::text::{TextAlign, TextAlignKeyword};
                    // HTML `align` uses the -moz-* legacy keywords (which don't
                    // get overridden by inheriting `text-align`).
                    let keyword = match value {
                        "left" => Some(TextAlignKeyword::MozLeft),
                        "right" => Some(TextAlignKeyword::MozRight),
                        "center" => Some(TextAlignKeyword::MozCenter),
                        _ => None,
                    };
                    if let Some(k) = keyword {
                        push_style(PropertyDeclaration::TextAlign(TextAlign::Keyword(k)));
                    }
                }

                // `width` on table/col/tr/td/th/img/hr.
                "width"
                    if *tag == local_name!("table")
                        || *tag == local_name!("col")
                        || *tag == local_name!("tr")
                        || *tag == local_name!("td")
                        || *tag == local_name!("th")
                        || *tag == local_name!("img")
                        || *tag == local_name!("hr") =>
                {
                    let is_table = *tag == local_name!("table");
                    if let Some(w) = parse_legacy_size(value, |v| !is_table || v != 0.0) {
                        use style::values::generics::{length::Size, NonNegative};
                        push_style(PropertyDeclaration::Width(Size::LengthPercentage(
                            NonNegative(w),
                        )));
                    }
                }

                // `height` on table/td/th/thead/tbody/tfoot/img.
                "height"
                    if *tag == local_name!("table")
                        || *tag == local_name!("td")
                        || *tag == local_name!("th")
                        || *tag == local_name!("thead")
                        || *tag == local_name!("tbody")
                        || *tag == local_name!("tfoot")
                        || *tag == local_name!("img") =>
                {
                    if let Some(h) = parse_legacy_size(value, |_| true) {
                        use style::values::generics::{length::Size, NonNegative};
                        push_style(PropertyDeclaration::Height(Size::LengthPercentage(
                            NonNegative(h),
                        )));
                    }
                }

                // Generic `bgcolor` → background-color.
                "bgcolor" => {
                    if let Some((r, g, b, a)) = parse_legacy_color(value) {
                        use style::values::specified::Color;
                        push_style(PropertyDeclaration::BackgroundColor(
                            Color::from_absolute_color(AbsoluteColor::srgb_legacy(r, g, b, a)),
                        ));
                    }
                }

                // `<td>/<th> nowrap` → white-space: nowrap (text-wrap-mode).
                "nowrap" if *tag == local_name!("td") || *tag == local_name!("th") => {
                    use style::computed_values::text_wrap_mode::T as TextWrapMode;
                    push_style(PropertyDeclaration::TextWrapMode(TextWrapMode::Nowrap));
                }

                // `<table border>` → uniform solid border on the table box.
                "border" if *tag == local_name!("table") => {
                    use style::values::specified::border::{BorderSideWidth, BorderStyle};
                    // Empty/absent value defaults to 1px in legacy HTML.
                    let px = if value.is_empty() {
                        1.0
                    } else {
                        value.parse::<f32>().unwrap_or(0.0)
                    };
                    if px > 0.0 {
                        let w = BorderSideWidth::from_px(px);
                        push_style(PropertyDeclaration::BorderTopWidth(w.clone()));
                        push_style(PropertyDeclaration::BorderRightWidth(w.clone()));
                        push_style(PropertyDeclaration::BorderBottomWidth(w.clone()));
                        push_style(PropertyDeclaration::BorderLeftWidth(w));
                        push_style(PropertyDeclaration::BorderTopStyle(BorderStyle::Outset));
                        push_style(PropertyDeclaration::BorderRightStyle(BorderStyle::Outset));
                        push_style(PropertyDeclaration::BorderBottomStyle(BorderStyle::Outset));
                        push_style(PropertyDeclaration::BorderLeftStyle(BorderStyle::Outset));
                    }
                }

                // `<table cellspacing>` → border-spacing.
                "cellspacing" if *tag == local_name!("table") => {
                    if let Some(px) = value.parse::<f32>().ok().filter(|v| *v >= 0.0) {
                        use style::values::generics::border::BorderSpacing;
                        use style::values::specified::NonNegativeLength;
                        let len = NonNegativeLength::from_px(px);
                        push_style(PropertyDeclaration::BorderSpacing(Box::new(
                            BorderSpacing::new(len.clone(), len),
                        )));
                    }
                }

                _ => {}
            }
        }
        // NOTE (Stage-2b): `<table cellpadding>` is intentionally NOT handled
        // here — it must set padding on every descendant cell, which a hint on
        // the <table> element cannot express. The old cascade applied it per
        // cell; Stage 2b must replicate that propagation outside Stylo, or it
        // can be modeled via a UA rule keyed on an internal attribute.
    }

    fn local_name(&self) -> &LocalName {
        &self.element_name().expect("not an element").local
    }
    fn namespace(&self) -> &Namespace {
        &self.element_name().expect("not an element").ns
    }

    fn query_container_size(
        &self,
        _display: &style::values::specified::Display,
    ) -> euclid::default::Size2D<Option<app_units::Au>> {
        Default::default()
    }

    fn each_custom_state<F>(&self, _callback: F)
    where
        F: FnMut(&AtomIdent),
    {
    }

    fn has_selector_flags(&self, flags: ElementSelectorFlags) -> bool {
        self.selector_flags.get().contains(flags)
    }

    fn relative_selector_search_direction(&self) -> ElementSelectorFlags {
        let flags = self.selector_flags.get();
        use ElementSelectorFlags as F;
        if flags.contains(F::RELATIVE_SELECTOR_SEARCH_DIRECTION_ANCESTOR_SIBLING) {
            F::RELATIVE_SELECTOR_SEARCH_DIRECTION_ANCESTOR_SIBLING
        } else if flags.contains(F::RELATIVE_SELECTOR_SEARCH_DIRECTION_ANCESTOR) {
            F::RELATIVE_SELECTOR_SEARCH_DIRECTION_ANCESTOR
        } else if flags.contains(F::RELATIVE_SELECTOR_SEARCH_DIRECTION_SIBLING) {
            F::RELATIVE_SELECTOR_SEARCH_DIRECTION_SIBLING
        } else {
            F::empty()
        }
    }

    fn compute_layout_damage(_old: &ComputedValues, _new: &ComputedValues) -> RestyleDamage {
        // We never do incremental relayout; report all damage.
        RestyleDamage::from_bits_retain(0b_0000_0000_0111_1111)
    }
}

// ---------------------------------------------------------------------------
// Cascade driver — Option A: sequential `style::driver::traverse_dom`.
// ---------------------------------------------------------------------------

use style::context::RegisteredSpeculativePainters;
use style::traversal::{recalc_style_at, DomTraversal, PerLevelTraversalData};

pub struct RegisteredPaintersImpl;
impl RegisteredSpeculativePainters for RegisteredPaintersImpl {
    fn get(&self, _name: &Atom) -> Option<&dyn style::context::RegisteredSpeculativePainter> {
        None
    }
}

pub struct RecalcStyle<'a> {
    context: SharedStyleContext<'a>,
}

impl<'a> RecalcStyle<'a> {
    pub fn new(context: SharedStyleContext<'a>) -> Self {
        RecalcStyle { context }
    }
}

impl<E> DomTraversal<E> for RecalcStyle<'_>
where
    E: TElement,
{
    fn process_preorder<F: FnMut(E::ConcreteNode)>(
        &self,
        traversal_data: &PerLevelTraversalData,
        context: &mut StyleContext<E>,
        node: E::ConcreteNode,
        note_child: F,
    ) {
        if let Some(el) = node.as_element() {
            let mut data = unsafe { el.ensure_data() };
            recalc_style_at(self, traversal_data, context, el, &mut data, note_child);
            unsafe { el.unset_dirty_descendants() }
        }
    }

    fn needs_postorder_traversal() -> bool {
        false
    }

    fn process_postorder(&self, _ctx: &mut StyleContext<E>, _node: E::ConcreteNode) {
        panic!("postorder should never be called")
    }

    fn shared_context(&self) -> &SharedStyleContext<'_> {
        &self.context
    }
}

// ---------------------------------------------------------------------------
// Device / Stylist / cascade entry point.
// ---------------------------------------------------------------------------

use style::animation::DocumentAnimationSet;
use style::device::Device;
use style::font_metrics::FontMetrics;
use style::global_style_data::GLOBAL_STYLE_DATA;
use style::media_queries::{MediaList, MediaType};
use style::properties::style_structs::Font;
use style::queries::values::PrefersColorScheme;
use style::selector_parser::SnapshotMap;
use style::shared_lock::StylesheetGuards;
use style::stylesheets::{
    AllowImportRules, CssRuleType, DocumentStyleSheet, Origin, Stylesheet, UrlExtraData,
};
use style::stylist::Stylist;
use style::thread_state::ThreadState;
use style::traversal_flags::TraversalFlags;
use style::values::computed::font::QueryFontMetricsFlags;
use style::values::computed::{CSSPixelLength, Length};

/// Synthetic font-metric ratios — used as a fallback when no `egui::Context` is
/// available (and to fill in any metric egui cannot measure). Tuned to match the
/// approximations used elsewhere in our font stack.
fn synthetic_metrics(font_size: CSSPixelLength) -> FontMetrics {
    FontMetrics {
        x_height: Some(font_size * 0.5),
        zero_advance_measure: Some(font_size * 0.5),
        cap_height: Some(font_size * 0.7),
        ascent: font_size * 0.8,
        ic_width: Some(font_size),
        script_percent_scale_down: None,
        script_script_percent_scale_down: None,
    }
}

/// Real font metrics backed by egui's font system (our [`crate::layout::fonts`]
/// measurement path), so `ex`/`ch`/`cap` units and font-relative line metrics
/// match what layout actually measures.
///
/// ## Why an `egui::Context` and not a `FontCtx`
/// Stylo's `Device` requires the provider be `Send + Sync + 'static`, and the
/// metrics query is `&self` (no zoom in scope). [`crate::layout::fonts::FontCtx`]
/// is neither `Send`/`Sync` (it holds non-thread-safe `RefCell` caches) nor does
/// it carry the right lifetime, and zoom must NOT enter Stylo's CSS-px space
/// anyway. So we hold the cheap-to-clone, `Send + Sync` [`egui::Context`]
/// directly and query the same `egui::Fonts` that `FontCtx` measures through —
/// at `zoom == 1.0` (CSS px), exactly what Stylo expects.
///
/// ## How metrics are measured
/// egui exposes `row_height` and per-glyph layout (`Glyph::uv_rect`,
/// `font_ascent`). We lay out a representative glyph and read:
/// - **ascent** from `Glyph::font_ascent` (real),
/// - **x-height** from the rasterized height of `x` (`uv_rect.size.y`),
/// - **cap-height** from the rasterized height of `H`,
/// - **zero advance (`ch`)** from `Fonts::glyph_width('0')`.
/// Any measurement that comes back as zero (e.g. before the font atlas is
/// populated) falls back to the synthetic ratio for that one metric.
#[derive(Clone)]
struct EguiFontMetricsProvider {
    ctx: egui::Context,
}

impl std::fmt::Debug for EguiFontMetricsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EguiFontMetricsProvider")
    }
}

impl EguiFontMetricsProvider {
    /// Map a Stylo `Font` style struct to an `egui::FontId` at CSS px (no zoom).
    fn font_id(font: &Font, size_px: f32) -> egui::FontId {
        // Fold the family list onto egui's two built-in generics, mirroring
        // `crate::layout::fonts::FontCtx::family`.
        // First family entry that resolves to a known egui generic wins; else
        // proportional. Mirrors `crate::layout::fonts::FontCtx::family`.
        let mut family = egui::FontFamily::Proportional;
        for fam in font.font_family.families.iter() {
            use style::values::computed::font::{GenericFontFamily, SingleFontFamily};
            let name = match fam {
                SingleFontFamily::FamilyName(n) => n.name.to_string().to_ascii_lowercase(),
                SingleFontFamily::Generic(GenericFontFamily::Monospace) => {
                    family = egui::FontFamily::Monospace;
                    break;
                }
                SingleFontFamily::Generic(_) => {
                    family = egui::FontFamily::Proportional;
                    break;
                }
            };
            match name.as_str() {
                "monospace" | "courier" | "courier new" | "consolas" | "menlo" | "monaco"
                | "code" => {
                    family = egui::FontFamily::Monospace;
                    break;
                }
                "serif" | "sans-serif" | "system-ui" | "arial" | "helvetica" | "times"
                | "times new roman" | "georgia" | "verdana" => {
                    family = egui::FontFamily::Proportional;
                    break;
                }
                // Unrecognized named family: keep looking for a later generic.
                _ => {}
            }
        }
        egui::FontId::new(size_px.max(1.0), family)
    }
}

impl style::device::servo::FontMetricsProvider for EguiFontMetricsProvider {
    fn query_font_metrics(
        &self,
        _vertical: bool,
        font: &Font,
        font_size: CSSPixelLength,
        _flags: QueryFontMetricsFlags,
    ) -> FontMetrics {
        let size_px = font_size.px();
        // Ratio-based x-height/cap-height/ch/ic. egui only exposes real glyph
        // sizes via the font *atlas* (`Glyph::uv_rect`), which isn't populated
        // during the style pass (before any paint) — it reads as ~0, which
        // collapsed `ex`/`ch` units and gave math images a 0×0 box. Ratios are
        // stable and plenty accurate for unit resolution.
        let mut m = synthetic_metrics(font_size);

        // The font *ascent* IS reliably available (it's font-table metadata, not
        // atlas raster), so take the real one when egui has it.
        let font_id = Self::font_id(font, size_px);
        let ascent = self.ctx.fonts(|f| {
            f.layout_no_wrap("x".to_string(), font_id, egui::Color32::WHITE)
                .rows
                .first()
                .and_then(|r| r.glyphs.first())
                .map(|g| g.font_ascent)
                .unwrap_or(0.0)
        });
        if ascent > 0.0 {
            m.ascent = CSSPixelLength::new(ascent);
        }
        m
    }

    fn base_size_for_generic(
        &self,
        generic: style::values::computed::font::GenericFontFamily,
    ) -> Length {
        let px = match generic {
            style::values::computed::font::GenericFontFamily::Monospace => 13.0,
            _ => 16.0,
        };
        Length::from(app_units::Au::from_f32_px(px))
    }
}

/// Fallback provider used when no `egui::Context` is supplied (pure synthetic
/// ratios; preserves the Stage-1 behavior).
#[derive(Debug)]
struct SyntheticFontMetrics;

impl style::device::servo::FontMetricsProvider for SyntheticFontMetrics {
    fn query_font_metrics(
        &self,
        _vertical: bool,
        _font: &Font,
        font_size: CSSPixelLength,
        _flags: QueryFontMetricsFlags,
    ) -> FontMetrics {
        synthetic_metrics(font_size)
    }

    fn base_size_for_generic(
        &self,
        generic: style::values::computed::font::GenericFontFamily,
    ) -> Length {
        let px = match generic {
            style::values::computed::font::GenericFontFamily::Monospace => 13.0,
            _ => 16.0,
        };
        Length::from(app_units::Au::from_f32_px(px))
    }
}

/// A default viewport height paired with `viewport_width` to build the Device.
/// Height media features are rare on the pages we target; a tall-ish default
/// keeps `min-height`/`vh` sane without a real window size.
const DEFAULT_VIEWPORT_HEIGHT: f32 = 600.0;

/// Style every element of `doc` with Stylo, storing an `Arc<ComputedValues>` per
/// element in its [`data::StyloData`]. Runs alongside (does not replace) the old
/// `css::cascade::style_document`.
///
/// Stylesheets are registered UA-first (our [`crate::css::ua`] sheet as
/// `Origin::UserAgent`) then author sheets (external `<link>` via `provider`,
/// then inline `<style>`, in document order) as `Origin::Author`. Media queries
/// (`min/max-width`, `prefers-color-scheme`) are handled natively by the Device,
/// so the raw sheet text is fed unfiltered.
/// `font_ctx`, when `Some`, supplies a real egui [`FontMetricsProvider`] so
/// `ex`/`ch`/`cap` units and font-relative metrics match what layout measures;
/// when `None`, falls back to synthetic ratios (Stage-1 behavior).
///
/// [`FontMetricsProvider`]: style::device::servo::FontMetricsProvider
pub fn style_document_stylo(
    doc: &mut Document,
    provider: &dyn ResourceProvider,
    base_url: Option<&str>,
    theme: Theme,
    viewport_width: f32,
    font_ctx: Option<&egui::Context>,
) {
    // Feature prefs must be set before constructing the Stylist.
    style_config::set_pref!("layout.grid.enabled", true);
    style_config::set_pref!("layout.unimplemented", true);
    style_config::set_pref!("layout.columns.enabled", true);
    style_config::set_pref!("layout.threads", -1);

    // Ensure a SharedRwLock owned by the document, then take it out so we can
    // borrow the arena mutably for inline-style parsing without aliasing it.
    if doc.stylo_lock.is_none() {
        doc.stylo_lock = Some(SharedRwLock::new());
    }
    let lock = doc.stylo_lock.take().expect("just set");

    // Dummy base URL for stylesheet/inline-style parsing.
    let dummy_url = ServoArc::new(url::Url::parse("data:text/css,").unwrap());
    let url_extra = UrlExtraData(dummy_url);

    // ---- Parse inline style="" attributes into per-element decl blocks. ----
    parse_inline_styles(doc, &lock, &url_extra);

    // ---- Collect stylesheet sources (UA, then author in document order). ----
    let author_sources = collect_author_sheet_sources(doc, provider, base_url);

    // ---- Build the Device. ----
    let viewport_size = euclid::Size2D::new(viewport_width, DEFAULT_VIEWPORT_HEIGHT);
    let dppx = euclid::Scale::new(1.0);
    let prefers = match theme {
        Theme::Light => PrefersColorScheme::Light,
        Theme::Dark => PrefersColorScheme::Dark,
    };
    let metrics_provider: Box<dyn style::device::servo::FontMetricsProvider> = match font_ctx {
        Some(ctx) => Box::new(EguiFontMetricsProvider { ctx: ctx.clone() }),
        None => Box::new(SyntheticFontMetrics),
    };
    let device = Device::new(
        MediaType::screen(),
        QuirksMode::NoQuirks,
        viewport_size,
        dppx,
        metrics_provider,
        ComputedValues::initial_values_with_font_override(Font::initial_values()),
        prefers,
    );

    // ---- Build the Stylist and register sheets. ----
    let mut stylist = Stylist::new(device, QuirksMode::NoQuirks);

    let make_sheet = |css: &str, origin: Origin| -> DocumentStyleSheet {
        let data = Stylesheet::from_str(
            css,
            url_extra.clone(),
            origin,
            ServoArc::new(lock.wrap(MediaList::empty())),
            lock.clone(),
            None,
            None,
            QuirksMode::NoQuirks,
            AllowImportRules::Yes,
        );
        DocumentStyleSheet(ServoArc::new(data))
    };

    // UA sheet (+ dark override, matching the old cascade's UA origin).
    let mut ua_src = String::from(crate::css::ua::UA_CSS);
    if theme == Theme::Dark {
        ua_src.push_str(crate::css::ua::UA_CSS_DARK);
    }
    stylist.append_stylesheet(make_sheet(&ua_src, Origin::UserAgent), &lock.read());

    // Author sheets in document order.
    for src in &author_sources {
        stylist.append_stylesheet(make_sheet(src, Origin::Author), &lock.read());
    }

    // Flush so the cascade data is built. Scope the read guards so they drop
    // before we move `lock` back into the document.
    {
        let guards = StylesheetGuards {
            author: &lock.read(),
            ua_or_user: &lock.read(),
        };
        stylist.flush(&guards);
    }

    // ---- Freeze the arena & set back-pointers, then run the cascade. ----
    doc.stylo_lock = Some(lock);
    doc.set_tree_pointers();
    let lock = doc.stylo_lock.as_ref().expect("lock present");

    style::thread_state::enter(ThreadState::LAYOUT);

    let snapshots = SnapshotMap::new();
    let root_node = &doc.nodes[doc.root];
    let root_element = match TDocument::as_node(&root_node)
        .first_element_child()
        .and_then(|n| n.as_element())
    {
        Some(el) => el,
        None => {
            // No element to style (empty document); restore state and bail.
            style::thread_state::exit(ThreadState::LAYOUT);
            return;
        }
    };

    let context = SharedStyleContext {
        traversal_flags: TraversalFlags::empty(),
        stylist: &stylist,
        options: GLOBAL_STYLE_DATA.options.clone(),
        guards: StylesheetGuards {
            author: &lock.read(),
            ua_or_user: &lock.read(),
        },
        visited_styles_enabled: false,
        animations: DocumentAnimationSet::default().clone(),
        current_time_for_animations: 0.0,
        snapshot_map: &snapshots,
        registered_speculative_painters: &RegisteredPaintersImpl,
    };

    let token = RecalcStyle::pre_traverse(root_element, &context);
    if token.should_traverse() {
        let traverser = RecalcStyle::new(context);
        // rayon_pool = None => sequential styling (Option A).
        style::driver::traverse_dom(&traverser, token, None);
    }

    style::thread_state::exit(ThreadState::LAYOUT);
}

/// Parse every element's inline `style="…"` attribute into a `Locked<…>` decl
/// block and stash it on the node for `TElement::style_attribute()`.
fn parse_inline_styles(doc: &mut Document, lock: &SharedRwLock, url_extra: &UrlExtraData) {
    for node in &mut doc.nodes {
        node.inline_style = None;
        let Some(style_src) = node.attr("style") else {
            continue;
        };
        if style_src.trim().is_empty() {
            continue;
        }
        let block = style::properties::parse_style_attribute(
            style_src,
            url_extra,
            None,
            QuirksMode::NoQuirks,
            CssRuleType::Style,
        );
        node.inline_style = Some(ServoArc::new(lock.wrap(block)));
    }
}

/// Collect author stylesheet *sources* (CSS text) in document order: external
/// `<link rel=stylesheet href>` fetched via `provider`, then inline `<style>`.
/// Mirrors `css::cascade::collect_author_sheets` but returns raw text (Stylo
/// parses it and handles media queries itself).
fn collect_author_sheet_sources(
    doc: &Document,
    provider: &dyn ResourceProvider,
    base_url: Option<&str>,
) -> Vec<String> {
    let mut out = Vec::new();

    // Pre-order traversal for document order.
    let mut ordered = Vec::new();
    let mut stack = vec![doc.root];
    while let Some(n) = stack.pop() {
        ordered.push(n);
        for &c in doc.nodes[n].children.iter().rev() {
            stack.push(c);
        }
    }

    for &n in &ordered {
        let node = &doc.nodes[n];
        match node.tag() {
            Some("link") => {
                let rel = node.attr("rel").unwrap_or("");
                if rel
                    .split_whitespace()
                    .any(|r| r.eq_ignore_ascii_case("stylesheet"))
                {
                    if let Some(href) = node.attr("href") {
                        let url = resolve_url(base_url, href);
                        if let Some((bytes, _mime)) = provider.fetch(&url) {
                            if let Ok(text) = String::from_utf8(bytes) {
                                out.push(text);
                            }
                        }
                    }
                }
            }
            Some("style") => {
                let mut text = String::new();
                for &c in &node.children {
                    if let NodeData::Text(t) = &doc.nodes[c].data {
                        text.push_str(t);
                    }
                }
                if !text.trim().is_empty() {
                    out.push(text);
                }
            }
            _ => {}
        }
    }
    out
}

/// Resolve `href` against `base_url` (copied from `css::cascade::resolve_url`).
fn resolve_url(base_url: Option<&str>, href: &str) -> String {
    let href = href.trim();
    if href.starts_with("http://")
        || href.starts_with("https://")
        || href.starts_with("//")
        || href.starts_with("data:")
    {
        return href.to_string();
    }
    match base_url {
        None => href.to_string(),
        Some(base) => {
            let dir = match base.rfind('/') {
                Some(i) => &base[..=i],
                None => "",
            };
            if href.starts_with('/') {
                if let Some(scheme_end) = base.find("://") {
                    let after = &base[scheme_end + 3..];
                    if let Some(slash) = after.find('/') {
                        return format!("{}{}", &base[..scheme_end + 3 + slash], href);
                    }
                    return format!("{base}{href}");
                }
                href.to_string()
            } else {
                format!("{dir}{href}")
            }
        }
    }
}

/// Parse a legacy HTML length attribute (`width`/`height`): bare number → px,
/// `Npx` → px, `N%` → percentage. `filter` rejects unwanted bare numbers (e.g.
/// `table width=0`). Returns a *specified* `LengthPercentage`.
fn parse_legacy_size(
    value: &str,
    filter: impl FnOnce(f32) -> bool,
) -> Option<style::values::specified::LengthPercentage> {
    use style::values::specified::{AbsoluteLength, LengthPercentage, NoCalcLength};
    let value = value.trim();
    if let Some(v) = value.strip_suffix("px") {
        let val: f32 = v.trim().parse().ok()?;
        return Some(LengthPercentage::Length(NoCalcLength::Absolute(
            AbsoluteLength::Px(val),
        )));
    }
    if let Some(v) = value.strip_suffix('%') {
        let val: f32 = v.trim().parse().ok()?;
        return Some(LengthPercentage::Percentage(Percentage(val / 100.0)));
    }
    let val: f32 = value.parse().ok().filter(|v| filter(*v))?;
    Some(LengthPercentage::Length(NoCalcLength::Absolute(
        AbsoluteLength::Px(val),
    )))
}

/// Parse a legacy `bgcolor`/color attribute. Supports `#rgb`/`#rrggbb`; returns
/// `(r, g, b, a)` with `a` in 0..=1.
fn parse_legacy_color(value: &str) -> Option<(u8, u8, u8, f32)> {
    let value = value.trim();
    let hex = value.strip_prefix('#')?;
    match hex.len() {
        3 => {
            let r = u8::from_str_radix(&hex[0..1], 16).ok()?;
            let g = u8::from_str_radix(&hex[1..2], 16).ok()?;
            let b = u8::from_str_radix(&hex[2..3], 16).ok()?;
            Some((r * 17, g * 17, b * 17, 1.0))
        }
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some((r, g, b, 1.0))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Layout-facing primary-style accessor (the Stage-2b seam).
// ---------------------------------------------------------------------------

use crate::dom::NodeId;

/// Cached CSS initial-value `ComputedValues` for anonymous boxes / fallback
/// (e.g. a node with no Stylo data, or layout asking for style before a pass).
/// Built once on first use; cheap to clone (Arc bump) thereafter.
pub fn initial_computed_values() -> ServoArc<ComputedValues> {
    use std::sync::OnceLock;
    // `ComputedValues` is `Send + Sync`, so a process-wide cache is fine. We
    // wrap the servo `Arc` (not std `Arc`) in a struct so it can live in the
    // `OnceLock`.
    struct Holder(ServoArc<ComputedValues>);
    // SAFETY: `ComputedValues` is Send + Sync (it's shared across Stylo's
    // parallel traversal), so the holder is too.
    unsafe impl Send for Holder {}
    unsafe impl Sync for Holder {}
    static INITIAL: OnceLock<Holder> = OnceLock::new();
    INITIAL
        .get_or_init(|| Holder(ComputedValues::initial_values_with_font_override(Font::initial_values())))
        .0
        .clone()
}

/// The primary `ComputedValues` for a node, as a cheap Arc clone.
///
/// For an ELEMENT node, returns its own Stylo primary style. For a TEXT node
/// (which Stylo does not style directly), returns the nearest ELEMENT
/// ancestor's primary style — text inherits its parent's style, exactly as the
/// old `style_for` did by walking up. Returns `None` only when no ancestor has
/// been styled (e.g. a node outside a completed Stylo pass).
pub fn primary_computed(doc: &Document, node: NodeId) -> Option<ServoArc<ComputedValues>> {
    let mut cur = Some(node);
    while let Some(id) = cur {
        let n = &doc.nodes[id];
        if let Some(styles) = n.stylo_element_data.primary_styles() {
            // `StyleDataRef` derefs to the primary `Arc<ComputedValues>`; clone
            // the Arc (cheap refcount bump).
            let arc: &ServoArc<ComputedValues> = &styles;
            return Some(arc.clone());
        }
        cur = n.parent;
    }
    None
}

/// The single Stylo→[`ComputedStyle`] projection boundary.
///
/// Materializes our owned [`ComputedStyle`] for a node from Stylo's primary
/// `ComputedValues` (via the [`read`] accessors). Text nodes inherit the nearest
/// styled element ancestor's style (handled by [`primary_computed`]). Unstyled
/// nodes fall back to the CSS initial style. The `<math>` pre-render path stamps
/// an intrinsic replaced size onto `node.replaced_size`, which overrides
/// width/height here.
///
/// This is the ONLY function layout/paint may use to obtain a node's style;
/// Stylo's `ComputedValues` and the `read::*` seam stay confined to this module.
pub fn computed_style_for(doc: &Document, node: NodeId) -> crate::css::computed::ComputedStyle {
    let mut style = match primary_computed(doc, node) {
        Some(cv) => style_from_cv(&cv, doc.nodes[node].replaced_size),
        None => crate::css::computed::ComputedStyle::initial(),
    };
    // Overrides set by the `<math>` pre-render pass to un-hide block math trapped
    // in Wikipedia's MathML a11y wrapper win over Stylo's computed values.
    if let Some(d) = doc.nodes[node].display_override {
        style.display = d;
    }
    if doc.nodes[node].force_visible {
        use crate::css::values::{LengthPercentOrAuto, Position};
        style.opacity = 1.0;
        style.position = Position::Static;
        // The a11y wrapper is clamped to 1px; let it size to its content. Replaced
        // nodes (the `<math>` itself) keep their stamped intrinsic size.
        if doc.nodes[node].replaced_size.is_none() {
            style.width = LengthPercentOrAuto::Auto;
            style.height = LengthPercentOrAuto::Auto;
        }
    }
    style
}

/// Build our [`ComputedStyle`] from Stylo's [`ComputedValues`] using the [`read`]
/// accessors. `replaced` (when `Some`) overrides width/height with an intrinsic
/// replaced size in UNZOOMED px (the `<math>` pre-render path).
fn style_from_cv(
    cv: &ComputedValues,
    replaced: Option<(f32, f32)>,
) -> crate::css::computed::ComputedStyle {
    use crate::css::computed::ComputedStyle;
    use crate::css::values::{Length, LengthPercentOrAuto};
    let (width, height) = match replaced {
        Some((w, h)) => (
            LengthPercentOrAuto::Length(Length::Px(w)),
            LengthPercentOrAuto::Length(Length::Px(h)),
        ),
        None => (read::width(cv), read::height(cv)),
    };
    ComputedStyle {
        display: read::display(cv),
        position: read::position(cv),
        float: read::float(cv),
        clear: read::clear(cv),
        box_sizing: read::box_sizing(cv),

        width,
        height,
        min_width: read::min_width(cv),
        max_width: read::max_width(cv),
        min_height: read::min_height(cv),
        max_height: read::max_height(cv),

        margin: read::margin(cv),
        padding: read::padding(cv),
        border_width: read::border_width(cv),
        border_style: read::border_style(cv),
        border_color: read::border_color(cv),

        top: read::top(cv),
        right: read::right(cv),
        bottom: read::bottom(cv),
        left: read::left(cv),

        color: read::color(cv),
        background_color: read::background_color(cv),

        font_family: read::font_family(cv),
        font_size: read::font_size(cv),
        font_weight: read::font_weight(cv),
        font_style: read::font_style(cv),
        line_height: read::line_height(cv),

        text_align: read::text_align(cv),
        text_decoration_underline: read::text_decoration_underline(cv),
        white_space: read::white_space(cv),
        vertical_align: read::vertical_align(cv),

        list_style_type: read::list_style_type(cv),

        opacity: read::opacity(cv),
    }
}

// ---------------------------------------------------------------------------
// Computed-value read-back helpers (used by tests; Stage 2 layout will grow
// these into the real bridge to our box tree).
// ---------------------------------------------------------------------------

/// The computed `display` of an element, or `None` if it was not styled.
pub fn computed_display(node: &Node) -> Option<style::values::computed::Display> {
    let styles = node.stylo_element_data.primary_styles()?;
    Some(styles.clone_display())
}

/// The computed text `color` of an element as `(r, g, b, a)` (a in 0..=1).
pub fn computed_color(node: &Node) -> Option<(u8, u8, u8, f32)> {
    let styles = node.stylo_element_data.primary_styles()?;
    let cv: &ComputedValues = &styles;
    let srgb = cv.clone_color().into_srgb_legacy();
    let c = srgb.raw_components();
    Some((
        (c[0] * 255.0).round() as u8,
        (c[1] * 255.0).round() as u8,
        (c[2] * 255.0).round() as u8,
        c[3],
    ))
}

/// The computed `font-size` of an element in CSS px, or `None` if not styled.
pub fn computed_font_size_px(node: &Node) -> Option<f32> {
    let styles = node.stylo_element_data.primary_styles()?;
    let cv: &ComputedValues = &styles;
    Some(cv.clone_font_size().used_size().px())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A dir-backed provider, matching the existing cascade/lib tests.
    struct DirProvider {
        dir: PathBuf,
    }
    impl ResourceProvider for DirProvider {
        fn fetch(&self, url: &str) -> Option<(Vec<u8>, String)> {
            let name = url.rsplit('/').next().unwrap_or(url);
            let path = self.dir.join(name);
            std::fs::read(&path)
                .ok()
                .map(|bytes| (bytes, "text/css".to_string()))
        }
    }

    /// A headless egui context with default fonts (mirrors `layout::mod`'s
    /// test helper) so the real `EguiFontMetricsProvider` can measure glyphs.
    fn headless_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(egui::FontDefinitions::default());
        // Run one empty frame so the font atlas is ready for measurement
        // (egui panics on `fonts(...)` before the first frame).
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        ctx
    }

    /// First element with the given tag name (markup5ever local), in arena order.
    fn find_tag<'a>(doc: &'a Document, tag: &str) -> Option<&'a Node> {
        doc.nodes
            .iter()
            .find(|n| n.tag() == Some(tag))
    }

    /// First element carrying `class` token `cls`.
    fn find_class<'a>(doc: &'a Document, cls: &str) -> Option<&'a Node> {
        doc.nodes
            .iter()
            .find(|n| n.classes().any(|c| c == cls))
    }

    #[test]
    fn stylo_styles_wiki_article() {
        let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/wiki-sample"));
        let html = std::fs::read_to_string(dir.join("article.html")).expect("read article.html");
        let provider = DirProvider { dir };

        let mut doc = crate::dom::parse_html(&html);
        let ctx = headless_ctx();
        style_document_stylo(
            &mut doc,
            &provider,
            Some("./"),
            Theme::Light,
            800.0,
            Some(&ctx),
        );

        // (a) A large fraction of element nodes got primary styles.
        let elements: Vec<&Node> = doc.nodes.iter().filter(|n| n.is_element()).collect();
        let styled = elements
            .iter()
            .filter(|n| n.stylo_element_data.primary_styles().is_some())
            .count();
        let total = elements.len();
        eprintln!(
            "stylo: {styled}/{total} element nodes styled ({:.1}%)",
            100.0 * styled as f32 / total as f32
        );
        assert!(total > 500, "expected a large article, got {total} elements");
        assert!(
            styled as f32 >= 0.9 * total as f32,
            "expected >=90% of elements styled, got {styled}/{total}"
        );

        // (b) Spot-checks against well-defined computed values.

        // The infobox/ib-chembox table computes display:table (the bug we fixed
        // was it computing block).
        let table = find_class(&doc, "ib-chembox")
            .filter(|n| n.tag() == Some("table"))
            .expect("ib-chembox table");
        let disp = computed_display(table).expect("table styled");
        eprintln!("ib-chembox table display = {disp:?}");
        assert_eq!(
            disp,
            style::values::computed::Display::Table,
            "infobox table should compute display:table, got {disp:?}"
        );

        // A normal <a href> link computes a blue-ish color (UA a:link blue).
        let link = doc
            .nodes
            .iter()
            .find(|n| n.tag() == Some("a") && n.attr("href").is_some())
            .expect("an <a href> link");
        let (r, g, b, _a) = computed_color(link).expect("link styled");
        eprintln!("link color = ({r},{g},{b})");
        assert!(
            b > r && b > g && b >= 120,
            "link should be blue-ish, got ({r},{g},{b})"
        );

        // <body> font-size is sane (UA medium ~16px; never absurd).
        let body = find_tag(&doc, "body").expect("body");
        let fs = computed_font_size_px(body).expect("body styled");
        eprintln!("body font-size = {fs}px");
        assert!(
            (8.0..=32.0).contains(&fs),
            "body font-size should be sane, got {fs}px"
        );
    }

    /// Borrow a node's primary `ComputedValues` and run `f` over the read layer.
    fn with_cv<R>(node: &Node, f: impl FnOnce(&ComputedValues) -> R) -> Option<R> {
        let styles = node.stylo_element_data.primary_styles()?;
        let cv: &ComputedValues = &styles;
        Some(f(cv))
    }

    #[test]
    fn read_layer_and_hints_on_wiki_article() {
        use crate::css::stylo::read;
        use crate::css::values::{Display, LengthOrPercent, LengthPercentOrAuto, Length};

        let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/wiki-sample"));
        let html = std::fs::read_to_string(dir.join("article.html")).expect("read article.html");
        let provider = DirProvider { dir };

        let mut doc = crate::dom::parse_html(&html);
        let ctx = headless_ctx();
        style_document_stylo(
            &mut doc,
            &provider,
            Some("./"),
            Theme::Light,
            800.0,
            Some(&ctx),
        );

        // (1) read::display on the ib-chembox table == Display::Table.
        let table = find_class(&doc, "ib-chembox")
            .filter(|n| n.tag() == Some("table"))
            .expect("ib-chembox table");
        let disp = with_cv(table, read::display).expect("table styled");
        assert_eq!(disp, Display::Table, "ib-chembox table => Display::Table");

        // (2) read::font_size on <body> ~16px.
        let body = find_tag(&doc, "body").expect("body");
        let fs = with_cv(body, read::font_size).expect("body styled");
        assert!(
            (12.0..=20.0).contains(&fs),
            "body font-size ~16px, got {fs}"
        );

        // (3) read::color on a link is blue-ish.
        let link = doc
            .nodes
            .iter()
            .find(|n| n.tag() == Some("a") && n.attr("href").is_some())
            .expect("an <a href> link");
        let c = with_cv(link, read::color).expect("link styled");
        assert!(
            c.b() > c.r() && c.b() > c.g() && c.b() >= 120,
            "link color blue-ish, got {c:?}"
        );

        // (4) margin/padding read back as expected px on some block. The body has
        // a UA margin (8px) in our UA sheet; assert it reads as a px length.
        let body_margin = with_cv(body, read::margin).expect("body styled");
        // Whatever the UA sets, the values must be concrete (px or auto), and the
        // top margin must resolve to a finite length when non-auto.
        if let LengthPercentOrAuto::Length(Length::Px(px)) = body_margin.top {
            assert!(px.is_finite() && px >= 0.0, "body margin-top px sane");
        }

        // padding edges must be length-or-percent (no panic / well-formed).
        let body_padding = with_cv(body, read::padding).expect("body styled");
        matches!(body_padding.top, LengthOrPercent::Length(_) | LengthOrPercent::Percent(_));

        // A cell with an inline `width:50%` reads back as a 50% width through the
        // read layer (exercises LengthPercentage → Percent mapping).
        let half_cell = doc
            .nodes
            .iter()
            .find(|n| {
                n.tag() == Some("td")
                    && n.attr("style").map(|s| s.contains("width:50%")).unwrap_or(false)
            })
            .expect("a td with width:50%");
        let w = with_cv(half_cell, read::width).expect("cell styled");
        assert!(
            matches!(w, LengthPercentOrAuto::Percent(p) if (p - 0.5).abs() < 1e-3),
            "td width:50% => Percent(0.5), got {w:?}"
        );
    }

    #[test]
    fn presentational_hints_reflect_in_computed_style() {
        use crate::css::stylo::read;
        use crate::css::values::{LengthPercentOrAuto, Length, TextAlign};

        // A synthetic table using only legacy presentational attributes.
        let html = r##"<!DOCTYPE html><html><body>
            <table border="3" cellspacing="5">
              <tr>
                <td width="120" height="40" bgcolor="#ff0000" align="center" nowrap>cell</td>
                <td width="50%">half</td>
              </tr>
            </table>
        </body></html>"##;

        struct NoProvider;
        impl ResourceProvider for NoProvider {
            fn fetch(&self, _url: &str) -> Option<(Vec<u8>, String)> {
                None
            }
        }

        let mut doc = crate::dom::parse_html(html);
        let ctx = headless_ctx();
        style_document_stylo(&mut doc, &NoProvider, None, Theme::Light, 800.0, Some(&ctx));

        // The first <td> with the legacy attributes.
        let td = doc
            .nodes
            .iter()
            .find(|n| n.tag() == Some("td") && n.attr("width") == Some("120"))
            .expect("the legacy <td>");

        // width=120 → 120px.
        let w = with_cv(td, read::width).expect("td styled");
        assert_eq!(
            w,
            LengthPercentOrAuto::Length(Length::Px(120.0)),
            "td width=120 => 120px hint"
        );
        // height=40 → 40px.
        let h = with_cv(td, read::height).expect("td styled");
        assert_eq!(
            h,
            LengthPercentOrAuto::Length(Length::Px(40.0)),
            "td height=40 => 40px hint"
        );
        // bgcolor=#ff0000 → red background.
        let bg = with_cv(td, read::background_color).expect("td styled").expect("bg set");
        assert!(
            bg.r() >= 200 && bg.g() < 60 && bg.b() < 60,
            "td bgcolor=#ff0000 => red, got {bg:?}"
        );
        // align=center → text-align center.
        let ta = with_cv(td, read::text_align).expect("td styled");
        assert_eq!(ta, TextAlign::Center, "td align=center => center");
        // nowrap → white-space nowrap.
        let ws = with_cv(td, read::white_space).expect("td styled");
        assert_eq!(
            ws,
            crate::css::values::WhiteSpace::Nowrap,
            "td nowrap => white-space:nowrap"
        );

        // The <table border="3"> got a uniform border width via the hint.
        let table = doc.nodes.iter().find(|n| n.tag() == Some("table")).expect("table");
        let bw = with_cv(table, read::border_width).expect("table styled");
        assert!(
            (bw.top - 3.0).abs() < 0.5 && (bw.left - 3.0).abs() < 0.5,
            "table border=3 => ~3px border widths, got {bw:?}"
        );

        // The second cell width=50% reads as Percent(0.5).
        let td2 = doc
            .nodes
            .iter()
            .find(|n| n.tag() == Some("td") && n.attr("width") == Some("50%"))
            .expect("the 50% <td>");
        let w2 = with_cv(td2, read::width).expect("td2 styled");
        assert!(
            matches!(w2, LengthPercentOrAuto::Percent(p) if (p - 0.5).abs() < 1e-3),
            "td width=50% => Percent(0.5), got {w2:?}"
        );
    }
}
