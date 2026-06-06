//! Stylo's element/DOM trait surface implemented over our arena [`Node`].
//!
//! This is the selector-matching layer of the Stylo integration: it implements
//! the trait surface Stylo's style system needs (`selectors::Element`,
//! `TDocument`/`TNode`/`TShadowRoot`/`NodeInfo`/`AttributeProvider`/`TElement`)
//! over a `&Node` borrow, so Stylo can walk our tree, match selectors, read
//! attributes, and synthesize presentational hints from legacy HTML attributes.

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
use style::context::{QuirksMode, SharedStyleContext};
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

use crate::dom::{Node, NodeData};

// ---------------------------------------------------------------------------
// Node handle helpers (mirror the spike's ToyNode accessors).
// ---------------------------------------------------------------------------

/// The styling node handle (a borrow), exactly as blitz does it.
pub(super) type NodeRef<'a> = &'a Node;

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
// Legacy presentational-attribute value parsers (used by the hint synthesis
// in `TElement::synthesize_presentational_hints_for_legacy_attributes`).
// ---------------------------------------------------------------------------

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
