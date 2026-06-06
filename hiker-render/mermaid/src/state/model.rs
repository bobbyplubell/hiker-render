//! State-diagram data model: states (real, pseudo `[*]`, and the special
//! `<<fork>>`/`<<join>>`/`<<choice>>` markers, with composite nesting),
//! transitions, and notes. Produced by [`super::parse`], drawn by
//! [`super::render`].

use crate::model::ElemStyle;

/// A state node. `pseudo` is `None` for a real state, or `Start`/`End` for the
/// two synthetic pseudo-states.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct State {
    pub(super) id: String,
    pub(super) label: String,
    pub(super) pseudo: Option<Pseudo>,
    /// Special marker shape, if any (`<<fork>>`/`<<join>>`/`<<choice>>`).
    pub(super) kind: StateKind,
    /// Index (into `states`) of the composite that directly contains this state,
    /// or `None` for a top-level state.
    pub(super) parent: Option<usize>,
    /// True if this state is itself a composite (has a `{ … }` body).
    pub(super) composite: bool,
    /// Per-state style overrides (from `classDef`/`class`/`style`/`:::`).
    pub(super) style: ElemStyle,
    /// `click` interaction data: open URL (`link`), host callback name
    /// (`callback`), and hover `tooltip`. `None` unless a `click` directive
    /// targeted this state.
    pub(super) link: Option<String>,
    pub(super) callback: Option<String>,
    pub(super) tooltip: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pseudo {
    Start,
    End,
}

/// Special marker shape for `<<fork>>` / `<<join>>` / `<<choice>>` states.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum StateKind {
    #[default]
    Normal,
    Fork,
    Join,
    Choice,
}

/// Where a note is anchored relative to its target state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NotePos {
    Left,
    Right,
    Over,
}

/// A note attached to a state. Not part of the dagre graph: placed beside the
/// target's final position after layout.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Note {
    pub(super) target: String,
    pub(super) pos: NotePos,
    pub(super) text: String,
}

/// A transition `from --> to` with an optional label.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Transition {
    pub(super) from: String,
    pub(super) to: String,
    pub(super) label: Option<String>,
}

/// Parsed state diagram.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct StateDiagram {
    /// States in first-seen order.
    pub(super) states: Vec<State>,
    pub(super) transitions: Vec<Transition>,
    pub(super) notes: Vec<Note>,
}
