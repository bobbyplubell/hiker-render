//! Sequence-diagram data model: participants, messages (with arrow styles and
//! activation flags), notes, block frames (`loop`/`alt`/`par`/…), `rect`
//! background highlights, and the ordered item tree. Produced by
//! [`super::parse`] and drawn by [`super::render`].

/// A participant column: its id (used for matching in messages) and the label
/// drawn in its box (defaults to the id when no `as` alias is given).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Participant {
    pub(super) id: String,
    pub(super) label: String,
}

/// The visual style of an arrow's line + head, decoded from the arrow token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ArrowStyle {
    /// `->>` / `-->>` — solid filled triangle head.
    Filled,
    /// `->` / `-->` — open V (line, no fill).
    Open,
    /// `-)` / `--)` — async, open V (same draw as `Open` here).
    Async,
    /// `-x` / `--x` — a small cross at the end.
    Cross,
}

/// One message between two participants.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Message {
    pub(super) from: String,
    pub(super) to: String,
    pub(super) text: String,
    pub(super) style: ArrowStyle,
    /// Dashed line (the `--` arrow variants).
    pub(super) dashed: bool,
    /// `+` suffix on the arrow ⇒ activate `to` on arrival.
    pub(super) activate_to: bool,
    /// `-` suffix on the arrow ⇒ deactivate `from` (its current activation) on
    /// send.
    pub(super) deactivate_from: bool,
    /// Sequence number from `autonumber` (1-based), or `None` when autonumber is
    /// off. Rendered as a small badge before the message text.
    pub(super) number: Option<u32>,
}

/// Where a note sits relative to its participant(s).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NotePlacement {
    LeftOf,
    RightOf,
    /// Spans over one (`over A`) or two (`over A,B`) participants.
    Over,
}

/// A `Note …` line.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Note {
    pub(super) placement: NotePlacement,
    /// One participant for left/right/over-single; two for `over A,B`.
    pub(super) targets: Vec<String>,
    pub(super) text: String,
}

/// The keyword that opened a block frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BlockKind {
    Loop,
    Opt,
    Alt,
    Par,
    Break,
    Critical,
}

impl BlockKind {
    pub(super) fn keyword(self) -> &'static str {
        match self {
            BlockKind::Loop => "loop",
            BlockKind::Opt => "opt",
            BlockKind::Alt => "alt",
            BlockKind::Par => "par",
            BlockKind::Break => "break",
            BlockKind::Critical => "critical",
        }
    }
}

/// A parsed block frame: keyword + opening label, then a flat list of child
/// items, with section markers recording (child-index, section-label) for each
/// `else`/`and`/`option` divider.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Block {
    pub(super) kind: BlockKind,
    pub(super) label: String,
    pub(super) items: Vec<Item>,
    /// (index into `items` where the section starts, section label). The first
    /// section (the opening one) is implicit and uses `label`.
    pub(super) sections: Vec<(usize, String)>,
}

/// An RGBA color parsed from a `rect` background block's color argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Rgba {
    pub(super) r: u8,
    pub(super) g: u8,
    pub(super) b: u8,
    /// Alpha 0..=255. `rgb(...)`/`#hex` default to a translucent highlight.
    pub(super) a: u8,
}

/// A `rect <color> … end` background block: a translucent filled rectangle drawn
/// behind the contained rows, spanning the involved participants. Unlike a
/// [`Block`] frame it has no label tab — it only tints the background.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct RectBlock {
    pub(super) color: Rgba,
    pub(super) items: Vec<Item>,
}

/// An ordered diagram item: a leaf (message/note/activation event) or a nested
/// block.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Item {
    Message(Message),
    Note(Note),
    /// `activate A`.
    Activate(String),
    /// `deactivate A`.
    Deactivate(String),
    Block(Block),
    /// `rect <color> … end` translucent background highlight.
    Rect(RectBlock),
}

/// A fully parsed sequence diagram (participants in column order + a tree of
/// ordered items).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SequenceDiagram {
    pub(super) participants: Vec<Participant>,
    pub(super) items: Vec<Item>,
}
