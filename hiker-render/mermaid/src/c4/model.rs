//! C4-diagram data model: elements (person/system/container/component, internal
//! or external), directed relationships, and nesting boundaries. Produced by
//! [`super::parse`] and drawn by [`super::render`].

/// The broad category of a C4 element, which drives the type line and fill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ElemKind {
    Person,
    System,
    Container,
    Component,
}

impl ElemKind {
    /// The bracketed type label, e.g. `[Person]` / `[Container: tech]`.
    pub(super) fn type_label(self, external: bool, tech: &str) -> String {
        let base = match (self, external) {
            (ElemKind::Person, false) => "Person",
            (ElemKind::Person, true) => "External Person",
            (ElemKind::System, false) => "Software System",
            (ElemKind::System, true) => "External System",
            (ElemKind::Container, _) => "Container",
            (ElemKind::Component, _) => "Component",
        };
        if tech.is_empty() {
            format!("[{base}]")
        } else {
            format!("[{base}: {tech}]")
        }
    }
}

/// A parsed C4 element (becomes one layout node and one drawn box).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Element {
    /// The id (first arg) used to reference this element in relationships.
    pub(super) id: String,
    /// The display name (bold first line).
    pub(super) label: String,
    /// The technology string (container/component only); empty otherwise.
    pub(super) tech: String,
    /// The wrapped description; empty if absent.
    pub(super) descr: String,
    pub(super) kind: ElemKind,
    pub(super) external: bool,
}

/// A directed relationship `from → to`, with an optional technology suffix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Relationship {
    pub(super) from: String,
    pub(super) to: String,
    pub(super) label: String,
    pub(super) tech: String,
}

/// The category of a boundary, which drives its `«…»` type label and the
/// default when the optional `type` arg is omitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BoundaryKind {
    System,
    Enterprise,
    Container,
    /// Bare `Boundary(...)` — generic.
    Generic,
}

impl BoundaryKind {
    /// The `«…»` type label shown at the boundary's top-left.
    pub(super) fn type_label(self) -> &'static str {
        match self {
            BoundaryKind::System => "«System»",
            BoundaryKind::Enterprise => "«Enterprise»",
            BoundaryKind::Container => "«Container»",
            BoundaryKind::Generic => "«Boundary»",
        }
    }
}

/// A parsed boundary block (becomes one dagre container node and one drawn
/// dashed rectangle).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Boundary {
    /// The id (first arg).
    pub(super) id: String,
    /// The display name (defaults to the id when absent).
    pub(super) label: String,
    pub(super) kind: BoundaryKind,
    /// Index into `C4Diagram::boundaries` of the enclosing boundary, if nested.
    pub(super) parent: Option<usize>,
    /// Ids of elements declared directly inside this boundary's `{ … }`.
    pub(super) member_elems: Vec<String>,
}

/// A parsed C4 diagram.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct C4Diagram {
    pub(super) elements: Vec<Element>,
    pub(super) relationships: Vec<Relationship>,
    pub(super) boundaries: Vec<Boundary>,
}

impl Element {
    pub(super) fn type_label(&self) -> String {
        self.kind.type_label(self.external, &self.tech)
    }
}
