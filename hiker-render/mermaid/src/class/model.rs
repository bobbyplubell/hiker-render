//! Class-diagram data model: the parsed `classDiagram` types (classes with
//! attribute/method compartments, relationships with UML markers, and notes).
//! Produced by [`super::parse`] and consumed by [`super::layout`]/[`super::render`].

use crate::model::ElemStyle;

/// One compartment row: an attribute (`+int age`) or a method (`+eat()`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    /// Full displayed text, including any visibility sigil (`+ - # ~`).
    pub text: String,
    /// `true` if this is a method (had `(...)`), `false` for an attribute.
    pub is_method: bool,
}

/// A parsed class: a name plus its attribute and method compartments.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Class {
    /// The bare id used for relationship matching (generic suffix stripped).
    pub name: String,
    /// The displayed name in the name compartment. Differs from `name` when the
    /// class carries a generic suffix (e.g. id `List`, display `List<int>`).
    pub display_name: String,
    /// Stereotype/annotation, without the `<<` `>>` (e.g. `interface`). Rendered
    /// «interface» in italics above the class name. Last one wins.
    pub annotation: Option<String>,
    pub attributes: Vec<Member>,
    pub methods: Vec<Member>,
    /// Per-class style overrides (from `classDef`/`class`/`cssClass`/`style`/`:::`).
    pub style: ElemStyle,
    /// `click` interaction data: open URL (`link`), host callback name
    /// (`callback`), and hover `tooltip`. All `None` unless a `click`/`link`/
    /// `callback` directive targeted this class.
    pub link: Option<String>,
    pub callback: Option<String>,
    pub tooltip: Option<String>,
}

/// A note rectangle: either attached to a class (`note for Class "text"`) or
/// floating (`note "text"`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    /// Note body text.
    pub text: String,
    /// The class id this note is attached to, if any (floating note → `None`).
    pub for_class: Option<String>,
}

/// Which UML marker a relationship carries and at which end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelMarker {
    /// `<|` hollow triangle (inheritance / realization).
    Triangle,
    /// `o` hollow diamond (aggregation).
    DiamondHollow,
    /// `*` filled diamond (composition).
    DiamondFilled,
    /// `>`/`<` open arrow (association / dependency).
    Arrow,
    /// Plain link, no marker.
    None,
}

/// A relationship between two classes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relation {
    /// Index/name of the left class (source as written).
    pub from: String,
    /// Index/name of the right class (target as written).
    pub to: String,
    /// The marker and the end it sits at.
    pub marker: RelMarker,
    /// `true` if the marker is at the `to` end, `false` if at the `from` end.
    pub marker_at_to: bool,
    /// Dashed line (`..`), e.g. dependency / realization.
    pub dashed: bool,
    /// Optional `: label`.
    pub label: Option<String>,
}

/// A parsed class diagram. `classes` is in first-seen order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClassDiagram {
    pub classes: Vec<Class>,
    pub relations: Vec<Relation>,
    pub notes: Vec<Note>,
}
