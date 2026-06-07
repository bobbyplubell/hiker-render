//! ER-diagram data model: entities (tables with attribute rows) and
//! relationships (with crow's-foot cardinality at each end). Produced by
//! [`super::parse`] and consumed by [`super::render`].

use crate::model::ElemStyle;

/// One cardinality end of a relationship.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Cardinality {
    /// `||` exactly one.
    ExactlyOne,
    /// `|{` / `}|` one or more.
    OneOrMore,
    /// `o{` / `}o` zero or more.
    ZeroOrMore,
    /// `o|` / `|o` zero or one.
    ZeroOrOne,
}

impl Cardinality {
    /// True when this end's outer mark is an open circle (the `o…` forms,
    /// i.e. the "zero" cardinalities).
    pub(super) fn has_circle(self) -> bool {
        matches!(self, Cardinality::ZeroOrMore | Cardinality::ZeroOrOne)
    }

    /// True when this end fans out into a crow's foot (the "many" forms).
    pub(super) fn has_foot(self) -> bool {
        matches!(self, Cardinality::OneOrMore | Cardinality::ZeroOrMore)
    }
}

/// An attribute row inside an entity box: `<type> <name> [<keys>] ["<comment>"]`.
/// `keys` holds the recognized `PK`/`FK`/`UK` markers in source order; `comment`
/// is the optional quoted trailing text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Attribute {
    pub(super) ty: String,
    pub(super) name: String,
    /// Recognized key markers (`PK`/`FK`/`UK`), in source order.
    pub(super) keys: Vec<String>,
    /// Optional quoted comment.
    pub(super) comment: Option<String>,
}

impl Attribute {
    /// Whether this attribute is (part of) a primary key — rendered emphasized.
    pub(super) fn is_pk(&self) -> bool {
        self.keys.iter().any(|k| k == "PK")
    }

    /// The keys joined for display, e.g. `PK,FK`. Empty when no keys.
    pub(super) fn keys_text(&self) -> String {
        self.keys.join(",")
    }
}

/// An entity (table). `attrs` empty → name-only box.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Entity {
    pub(super) name: String,
    pub(super) attrs: Vec<Attribute>,
    /// Per-entity style overrides (from `classDef`/`class`/`style`/`:::`).
    pub(super) style: ElemStyle,
    /// `click` interaction data: open URL (`link`), host callback name
    /// (`callback`), and hover `tooltip`. `None` unless a `click` directive
    /// targeted this entity.
    pub(super) link: Option<String>,
    pub(super) callback: Option<String>,
    pub(super) tooltip: Option<String>,
}

/// A relationship between two entities.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Relationship {
    pub(super) left: String,
    pub(super) right: String,
    pub(super) left_card: Cardinality,
    pub(super) right_card: Cardinality,
    /// Non-identifying (`..`) → dashed line.
    pub(super) dashed: bool,
    pub(super) label: Option<String>,
}

/// Parsed ER diagram.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct ErDiagram {
    /// Entities in first-seen order.
    pub(super) entities: Vec<Entity>,
    pub(super) relationships: Vec<Relationship>,
}
