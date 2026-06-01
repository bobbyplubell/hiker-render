//! Sequence diagram (self-contained: parse + layout + draw, no dagre).
//!
//! Mermaid sequence syntax (subset supported here):
//! ```text
//! sequenceDiagram
//!     participant A
//!     participant B as Bob
//!     actor C
//!     A->>B: Hello Bob
//!     B-->>A: Hi Alice
//!     A-)B: async
//!     A->>A: think
//! ```
//! Self-layout: participants become **columns** (x positions) with vertical
//! dashed lifelines; messages become **horizontal arrows** at increasing y.
//!
//! ## Supported
//! - Header `sequenceDiagram`.
//! - `participant <id>` / `participant <id> as <Label>` / `actor <id>`.
//! - Auto-created participants (first appearance order) for ids used in a
//!   message but never declared.
//! - Messages with arrow tokens between two ids and an optional `: text`:
//!   `->>`/`-->>` (filled arrowhead, solid/**dashed**), `->`/`-->` (open V),
//!   `-)`/`--)` (async open V), `-x`/`--x` (cross end). The `--` variants are
//!   dashed.
//! - Self-messages (`A->>A: text`) draw a small loop to the right of A's
//!   lifeline.
//!
//! ## Advanced features (v2)
//! - **Notes** — `Note left of A: t`, `Note right of A: t`, `Note over A: t`,
//!   `Note over A,B: t`. Drawn as a themed rectangle on a vertical row.
//! - **Activations** — `activate A` / `deactivate A`, and the `+`/`-` suffixes
//!   on message targets/sources (`A->>+B`, `B-->>-A`). Drawn as narrow vertical
//!   bars on the lifeline; nested activations offset horizontally.
//! - **Block frames** — `loop`/`opt`/`alt`/`par`/`break`/`critical` … `end`,
//!   with `else`/`and`/`option` section dividers. Drawn as labeled frames.
//! - **autonumber** — `autonumber` (optionally `autonumber <start>` /
//!   `autonumber <start> <step>`) prefixes each subsequent message with a small
//!   numbered badge.
//! - **rect background blocks** — `rect <color> … end` (color as `rgb(...)`,
//!   `rgba(...)`, or `#hex`) draws a translucent highlight behind the contained
//!   rows; nests freely with block frames.
//!
//! ## Skipped (intentionally)
//! Participant `links`/`box` grouping, and `break`-specific styling beyond a
//! plain frame.

use std::fmt::Write as _;

use crate::{MermaidError, MermaidOptions, MermaidRender};

// ----------------------------------------------------------------------------
// Model
// ----------------------------------------------------------------------------

/// A participant column: its id (used for matching in messages) and the label
/// drawn in its box (defaults to the id when no `as` alias is given).
#[derive(Clone, Debug, PartialEq)]
struct Participant {
    id: String,
    label: String,
}

/// The visual style of an arrow's line + head, decoded from the arrow token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArrowStyle {
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
struct Message {
    from: String,
    to: String,
    text: String,
    style: ArrowStyle,
    /// Dashed line (the `--` arrow variants).
    dashed: bool,
    /// `+` suffix on the arrow ⇒ activate `to` on arrival.
    activate_to: bool,
    /// `-` suffix on the arrow ⇒ deactivate `from` (its current activation) on
    /// send.
    deactivate_from: bool,
    /// Sequence number from `autonumber` (1-based), or `None` when autonumber is
    /// off. Rendered as a small badge before the message text.
    number: Option<u32>,
}

/// Where a note sits relative to its participant(s).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NotePlacement {
    LeftOf,
    RightOf,
    /// Spans over one (`over A`) or two (`over A,B`) participants.
    Over,
}

/// A `Note …` line.
#[derive(Clone, Debug, PartialEq)]
struct Note {
    placement: NotePlacement,
    /// One participant for left/right/over-single; two for `over A,B`.
    targets: Vec<String>,
    text: String,
}

/// The keyword that opened a block frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BlockKind {
    Loop,
    Opt,
    Alt,
    Par,
    Break,
    Critical,
}

impl BlockKind {
    fn keyword(self) -> &'static str {
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
struct Block {
    kind: BlockKind,
    label: String,
    items: Vec<Item>,
    /// (index into `items` where the section starts, section label). The first
    /// section (the opening one) is implicit and uses `label`.
    sections: Vec<(usize, String)>,
}

/// An RGBA color parsed from a `rect` background block's color argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rgba {
    r: u8,
    g: u8,
    b: u8,
    /// Alpha 0..=255. `rgb(...)`/`#hex` default to a translucent highlight.
    a: u8,
}

/// A `rect <color> … end` background block: a translucent filled rectangle drawn
/// behind the contained rows, spanning the involved participants. Unlike a
/// [`Block`] frame it has no label tab — it only tints the background.
#[derive(Clone, Debug, PartialEq)]
struct RectBlock {
    color: Rgba,
    items: Vec<Item>,
}

/// An ordered diagram item: a leaf (message/note/activation event) or a nested
/// block.
#[derive(Clone, Debug, PartialEq)]
enum Item {
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
struct SequenceDiagram {
    participants: Vec<Participant>,
    items: Vec<Item>,
}

// ----------------------------------------------------------------------------
// Parse
// ----------------------------------------------------------------------------

/// While parsing a block we are inside, this records the kind, opening label,
/// items collected so far, and section markers.
struct BlockFrame {
    /// `None` for a `rect <color>` background block (it has no keyword/sections,
    /// only a `color`); `Some(kind)` for a labeled block frame.
    kind: Option<BlockKind>,
    label: String,
    /// Background color for `rect` frames; ignored for labeled blocks.
    color: Option<Rgba>,
    items: Vec<Item>,
    sections: Vec<(usize, String)>,
}

/// Decode a `loop`/`opt`/`alt`/`par`/`break`/`critical` opener into its kind.
fn block_opener(lower: &str) -> Option<BlockKind> {
    let kw = lower.split_whitespace().next()?;
    match kw {
        "loop" => Some(BlockKind::Loop),
        "opt" => Some(BlockKind::Opt),
        "alt" => Some(BlockKind::Alt),
        "par" => Some(BlockKind::Par),
        "break" => Some(BlockKind::Break),
        "critical" => Some(BlockKind::Critical),
        _ => None,
    }
}

/// Parse `sequenceDiagram` source into a [`SequenceDiagram`].
///
/// Participants are collected in declaration order; any id referenced by a
/// message/note but not declared is appended in first-appearance order. Block
/// frames nest; `else`/`and`/`option` add section markers to the open block.
/// Returns the raw parse error string on malformed input.
fn parse_sequence(src: &str) -> Result<SequenceDiagram, String> {
    let mut participants: Vec<Participant> = Vec::new();
    let mut seen_header = false;

    // Track explicitly-declared ids so auto-created ones don't duplicate.
    let mut have: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Items at the top level, plus the stack of currently-open block frames.
    let mut top: Vec<Item> = Vec::new();
    let mut stack: Vec<BlockFrame> = Vec::new();

    // Autonumber state: when `on`, each message gets the `next` number then the
    // counter advances by `step`.
    let mut auto_on = false;
    let mut auto_next: u32 = 1;
    let mut auto_step: u32 = 1;

    for raw in src.lines() {
        // Strip `%%` comments, trailing `;`, and surrounding whitespace.
        let line = raw.split("%%").next().unwrap_or("").trim().trim_end_matches(';').trim();
        if line.is_empty() {
            continue;
        }

        let lower = line.to_ascii_lowercase();

        if !seen_header {
            if lower == "sequencediagram" || lower.starts_with("sequencediagram ") {
                seen_header = true;
                continue;
            }
            return Err(format!("expected 'sequenceDiagram' header, found {line:?}"));
        }

        // `autonumber` turns on sequential numbering of subsequent messages.
        // `autonumber <start>` sets the next number; `autonumber <start> <step>`
        // also sets the step. Bare `autonumber` resets to start 1 step 1.
        if lower == "autonumber" || lower.starts_with("autonumber ") {
            auto_on = true;
            let args: Vec<&str> = line.split_whitespace().skip(1).collect();
            auto_next = args.first().and_then(|s| s.parse().ok()).unwrap_or(1);
            auto_step = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1).max(1);
            continue;
        }

        // `rect <color> … end` background block: push a frame carrying the parsed
        // color; its matching `end` turns it into an `Item::Rect`.
        if lower == "rect" || lower.starts_with("rect ") {
            let arg = line["rect".len()..].trim();
            let color = parse_color(arg)
                .ok_or_else(|| format!("rect: unrecognized color {arg:?} in {line:?}"))?;
            stack.push(BlockFrame {
                kind: None,
                label: String::new(),
                color: Some(color),
                items: Vec::new(),
                sections: Vec::new(),
            });
            continue;
        }

        // `end` closes the innermost open block (frame or rect background).
        if lower == "end" {
            let frame = stack.pop().ok_or_else(|| "unexpected 'end' (no open block)".to_string())?;
            let item = match frame.kind {
                None => {
                    // `rect` background block.
                    Item::Rect(RectBlock {
                        color: frame.color.unwrap_or(Rgba { r: 200, g: 200, b: 255, a: 80 }),
                        items: frame.items,
                    })
                }
                Some(kind) => Item::Block(Block {
                    kind,
                    label: frame.label,
                    items: frame.items,
                    sections: frame.sections,
                }),
            };
            let dest = stack.last_mut().map(|f| &mut f.items).unwrap_or(&mut top);
            dest.push(item);
            continue;
        }

        // Section dividers inside the innermost block: `else [label]`,
        // `and [label]`, `option [label]`.
        if let Some(rest) = section_divider(&lower, line) {
            let frame = stack.last_mut().ok_or_else(|| {
                format!("section divider outside a block: {line:?}")
            })?;
            let idx = frame.items.len();
            frame.sections.push((idx, rest));
            continue;
        }

        // Block openers: `loop`/`opt`/`alt`/`par`/`break`/`critical [label]`.
        if let Some(kind) = block_opener(&lower) {
            let label = line[kind.keyword().len()..].trim().to_string();
            stack.push(BlockFrame {
                kind: Some(kind),
                label,
                color: None,
                items: Vec::new(),
                sections: Vec::new(),
            });
            continue;
        }

        // Notes.
        if lower.starts_with("note ") {
            let note = parse_note(line)?;
            for t in &note.targets {
                if !have.contains(t) {
                    declare(&mut participants, &mut have, t, None);
                }
            }
            push_item(&mut stack, &mut top, Item::Note(note));
            continue;
        }

        // Activations.
        if let Some(rest) = strip_keyword(line, "activate") {
            let id = rest.trim().to_string();
            if id.is_empty() {
                return Err(format!("activate missing participant: {line:?}"));
            }
            if !have.contains(&id) {
                declare(&mut participants, &mut have, &id, None);
            }
            push_item(&mut stack, &mut top, Item::Activate(id));
            continue;
        }
        if let Some(rest) = strip_keyword(line, "deactivate") {
            let id = rest.trim().to_string();
            if id.is_empty() {
                return Err(format!("deactivate missing participant: {line:?}"));
            }
            if !have.contains(&id) {
                declare(&mut participants, &mut have, &id, None);
            }
            push_item(&mut stack, &mut top, Item::Deactivate(id));
            continue;
        }

        // `participant <id> [as <label>]` or `actor <id> [as <label>]`.
        if let Some(rest) = strip_keyword(line, "participant").or_else(|| strip_keyword(line, "actor")) {
            let (id, label) = parse_participant_decl(rest)?;
            declare(&mut participants, &mut have, &id, label);
            continue;
        }

        // Otherwise it must be a message line.
        let mut msg = parse_message(line)?;
        // Assign an autonumber if numbering is on (messages only, not notes).
        if auto_on {
            msg.number = Some(auto_next);
            auto_next = auto_next.saturating_add(auto_step);
        }
        // Auto-create endpoints in first-appearance order.
        for end in [&msg.from, &msg.to] {
            if !have.contains(end) {
                declare(&mut participants, &mut have, end, None);
            }
        }
        push_item(&mut stack, &mut top, Item::Message(msg));
    }

    if !seen_header {
        return Err("missing 'sequenceDiagram' header".to_string());
    }
    if !stack.is_empty() {
        return Err("unterminated block (missing 'end')".to_string());
    }

    Ok(SequenceDiagram { participants, items: top })
}

/// Push a leaf item into the innermost open block, or the top level.
fn push_item(stack: &mut [BlockFrame], top: &mut Vec<Item>, item: Item) {
    match stack.last_mut() {
        Some(f) => f.items.push(item),
        None => top.push(item),
    }
}

/// Recognize a section divider line and return its (possibly empty) label.
/// Matches `else`, `and`, `option` as whole words.
fn section_divider(lower: &str, line: &str) -> Option<String> {
    for kw in ["else", "and", "option"] {
        if lower == kw {
            return Some(String::new());
        }
        if let Some(rest) = lower.strip_prefix(kw) {
            if rest.starts_with(char::is_whitespace) {
                // Preserve original-case label from `line`.
                return Some(line[kw.len()..].trim().to_string());
            }
        }
    }
    None
}

/// Parse a `Note left of A: t` / `Note right of A: t` / `Note over A[,B]: t`.
fn parse_note(line: &str) -> Result<Note, String> {
    // `line` starts with `Note ` (case-insensitively). Take the rest.
    let rest = &line[4..]; // after "Note"
    let rest = rest.trim_start();
    let lower = rest.to_ascii_lowercase();

    let (placement, after) = if let Some(a) = lower.strip_prefix("left of") {
        (NotePlacement::LeftOf, &rest[rest.len() - a.len()..])
    } else if let Some(a) = lower.strip_prefix("right of") {
        (NotePlacement::RightOf, &rest[rest.len() - a.len()..])
    } else if let Some(a) = lower.strip_prefix("over") {
        (NotePlacement::Over, &rest[rest.len() - a.len()..])
    } else {
        return Err(format!("note missing placement (left of/right of/over): {line:?}"));
    };

    let (targets_part, text) = match after.split_once(':') {
        Some((t, x)) => (t.trim(), x.trim().to_string()),
        None => (after.trim(), String::new()),
    };

    let targets: Vec<String> = targets_part
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if targets.is_empty() {
        return Err(format!("note missing participant target: {line:?}"));
    }
    if placement != NotePlacement::Over && targets.len() != 1 {
        return Err(format!("'{}' note takes exactly one participant: {line:?}",
            if placement == NotePlacement::LeftOf { "left of" } else { "right of" }));
    }
    if targets.len() > 2 {
        return Err(format!("note over takes at most two participants: {line:?}"));
    }

    Ok(Note { placement, targets, text })
}

/// Add a participant (id + optional explicit label) once, recording it in the
/// `have` set. A later `as` alias for an already-seen id updates its label.
fn declare(
    participants: &mut Vec<Participant>,
    have: &mut std::collections::HashSet<String>,
    id: &str,
    label: Option<String>,
) {
    if let Some(p) = participants.iter_mut().find(|p| p.id == id) {
        if let Some(l) = label {
            p.label = l;
        }
        return;
    }
    have.insert(id.to_string());
    participants.push(Participant {
        id: id.to_string(),
        label: label.unwrap_or_else(|| id.to_string()),
    });
}

/// If `line` starts with the whole word `kw` (followed by whitespace), return
/// the trimmed remainder; else `None`.
fn strip_keyword<'a>(line: &'a str, kw: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(kw)?;
    let mut chars = rest.chars();
    match chars.next() {
        Some(c) if c.is_whitespace() => Some(rest.trim()),
        _ => None,
    }
}

/// Parse the part after `participant`/`actor`: `<id>` or `<id> as <label>`.
fn parse_participant_decl(rest: &str) -> Result<(String, Option<String>), String> {
    // Split on ` as ` (case-insensitive), once.
    let lower = rest.to_ascii_lowercase();
    if let Some(pos) = lower.find(" as ") {
        let id = rest[..pos].trim();
        let label = rest[pos + 4..].trim();
        if id.is_empty() {
            return Err(format!("participant declaration missing id: {rest:?}"));
        }
        if label.is_empty() {
            return Err(format!("participant alias missing label: {rest:?}"));
        }
        Ok((id.to_string(), Some(label.to_string())))
    } else {
        let id = rest.trim();
        // A bare id must be a single token.
        if id.is_empty() || id.split_whitespace().count() != 1 {
            return Err(format!("invalid participant declaration: {rest:?}"));
        }
        Ok((id.to_string(), None))
    }
}

/// Parse one message line `A<arrow>B[: text]`. Lenient about spaces around the
/// arrow and the colon.
fn parse_message(line: &str) -> Result<Message, String> {
    // Split off the optional `: text`.
    let (link_part, text) = match line.split_once(':') {
        Some((l, t)) => (l.trim(), t.trim().to_string()),
        None => (line.trim(), String::new()),
    };

    let (from, arrow, to_raw) = split_arrow(link_part)
        .ok_or_else(|| format!("not a valid message (no arrow): {line:?}"))?;

    // A leading `+`/`-` on the target id is an activation suffix:
    //   `A->>+B` activates B on arrival; `A->>-B` deactivates the sender A.
    let mut to = to_raw;
    let mut activate_to = false;
    let mut deactivate_from = false;
    if let Some(stripped) = to.strip_prefix('+') {
        activate_to = true;
        to = stripped.trim().to_string();
    } else if let Some(stripped) = to.strip_prefix('-') {
        deactivate_from = true;
        to = stripped.trim().to_string();
    }

    if from.is_empty() || to.is_empty() {
        return Err(format!("message missing a participant id: {line:?}"));
    }

    let (style, dashed) = decode_arrow(arrow)
        .ok_or_else(|| format!("unrecognized arrow token {arrow:?} in {line:?}"))?;

    Ok(Message { from, to, text, style, dashed, activate_to, deactivate_from, number: None })
}

/// Find the arrow token inside `link` and return `(from_id, arrow, to_id)`.
///
/// Scans for the longest known arrow token (so `-->>` wins over `->`). Ids are
/// whatever sits on each side (trimmed). Returns `None` when no token matches.
fn split_arrow(link: &str) -> Option<(String, &str, String)> {
    // Arrow tokens, longest first so we match greedily.
    const ARROWS: [&str; 10] = ["-->>", "--x", "--)", "-->", "->>", "-x", "-)", "->", "--", ">>"];
    // Search left-to-right for the earliest position where any token matches;
    // among ties at a position, prefer the longest token.
    let mut best: Option<(usize, &str)> = None;
    for (i, _) in link.char_indices() {
        for tok in ARROWS {
            if link[i..].starts_with(tok) {
                // Prefer the earliest start; at the same start, the longest.
                let better = match best {
                    None => true,
                    Some((bi, bt)) => i < bi || (i == bi && tok.len() > bt.len()),
                };
                if better {
                    best = Some((i, tok));
                }
            }
        }
        if best.is_some() {
            // Earliest position found; lock it in (longest already chosen).
            let (bi, bt) = best.unwrap();
            let from = link[..bi].trim().to_string();
            let to = link[bi + bt.len()..].trim().to_string();
            return Some((from, bt, to));
        }
    }
    None
}

/// Decode an arrow token into `(style, dashed)`. `--` prefix ⇒ dashed.
fn decode_arrow(arrow: &str) -> Option<(ArrowStyle, bool)> {
    let style = match arrow {
        "->>" | "-->>" => ArrowStyle::Filled,
        "->" | "-->" | "--" => ArrowStyle::Open,
        "-)" | "--)" => ArrowStyle::Async,
        "-x" | "--x" => ArrowStyle::Cross,
        _ => return None,
    };
    let dashed = arrow.starts_with("--");
    Some((style, dashed))
}

/// Parse a `rect` background color: `rgb(r,g,b)`, `rgba(r,g,b,a)` (a is 0..=1 or
/// 0..=255), or a `#rgb`/`#rrggbb` hex. Whitespace is tolerated. `rgb`/hex with
/// no alpha default to a translucent highlight (alpha ~80/255) so the fill reads
/// as a highlight rather than an opaque block. Returns `None` if unrecognized.
fn parse_color(s: &str) -> Option<Rgba> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex(hex).map(|(r, g, b)| Rgba { r, g, b, a: 80 });
    }
    let lower = s.to_ascii_lowercase();
    let rgba = lower.starts_with("rgba");
    let prefix = if rgba { "rgba" } else { "rgb" };
    let rest = lower.strip_prefix(prefix)?.trim_start();
    let inner = rest.strip_prefix('(')?.strip_suffix(')')?;
    let parts: Vec<&str> = inner.split(',').map(|p| p.trim()).collect();
    let want = if rgba { 4 } else { 3 };
    if parts.len() != want {
        return None;
    }
    let r = parts[0].parse::<u8>().ok()?;
    let g = parts[1].parse::<u8>().ok()?;
    let b = parts[2].parse::<u8>().ok()?;
    let a = if rgba {
        let av = parts[3];
        // Alpha may be 0..=1 (CSS float) or 0..=255.
        if let Ok(f) = av.parse::<f32>() {
            if av.contains('.') || f <= 1.0 {
                (f.clamp(0.0, 1.0) * 255.0).round() as u8
            } else {
                f.clamp(0.0, 255.0) as u8
            }
        } else {
            return None;
        }
    } else {
        80
    };
    Some(Rgba { r, g, b, a })
}

/// Parse a `#rgb` or `#rrggbb` hex body (no leading `#`) into `(r, g, b)`.
fn parse_hex(hex: &str) -> Option<(u8, u8, u8)> {
    match hex.len() {
        3 => {
            let f = |c: u8| {
                let v = (c as char).to_digit(16)? as u8;
                Some(v * 16 + v)
            };
            let b = hex.as_bytes();
            Some((f(b[0])?, f(b[1])?, f(b[2])?))
        }
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some((r, g, b))
        }
        _ => None,
    }
}

// ----------------------------------------------------------------------------
// Layout constants
// ----------------------------------------------------------------------------

/// Per-char advance fraction of font size (kept in sync with `measure`).
const CHAR_ADVANCE_EM: f32 = 0.6;
/// Outer canvas margin, px.
const MARGIN: f32 = 16.0;
/// Participant box vertical padding (each side), px.
const BOX_PAD_Y: f32 = 8.0;
/// Participant box horizontal padding (each side), px.
const BOX_PAD_X: f32 = 12.0;
/// Minimum participant box width, px.
const MIN_BOX_W: f32 = 40.0;
/// Horizontal gap between adjacent participant boxes, px.
const COL_GAP: f32 = 40.0;
/// Vertical gap between consecutive message rows, px. A little roomy so a
/// message's lifted text label clears the line above it.
const MESSAGE_GAP: f32 = 46.0;
/// Gap between the participant boxes and the first message row, px.
const TOP_GAP: f32 = 30.0;
/// Width of a self-message loop, px.
const SELF_LOOP_W: f32 = 36.0;
/// Height of a self-message loop, px.
const SELF_LOOP_H: f32 = 28.0;
/// Arrowhead length / half-width, px.
const ARROW_LEN: f32 = 9.0;
const ARROW_HALF: f32 = 4.0;
/// Stroke width, px.
const STROKE_W: f32 = 1.5;
/// Width of an activation bar, px.
const ACT_W: f32 = 10.0;
/// Horizontal offset added per nested activation level, px.
const ACT_NEST_DX: f32 = 5.0;
/// Vertical row consumed by a note, px.
const NOTE_GAP: f32 = 50.0;
/// Note rectangle internal padding (each side), px.
const NOTE_PAD_X: f32 = 10.0;
const NOTE_PAD_Y: f32 = 6.0;
/// Min note rectangle width, px.
const NOTE_MIN_W: f32 = 60.0;
/// How far a left/right note sits from the lifeline, px.
const NOTE_SIDE_GAP: f32 = 14.0;
/// Inset (each side) added per nested block frame, px.
const FRAME_INSET: f32 = 10.0;
/// Base vertical padding inside a frame below the last row, px.
const FRAME_PAD_BOTTOM: f32 = 12.0;
/// Height of the frame's keyword label tab, px.
const FRAME_TAB_H: f32 = 16.0;
/// Radius of the autonumber badge circle, px.
const BADGE_R: f32 = 9.0;

/// Vertical headroom a frame reserves between its top edge and its first
/// contained row, so the centered opening label (a line of text at `font_size`)
/// clears the first message's lifted text label with a small gap.
///
/// The opening label is centered at `y0 + FRAME_TAB_H/2` and a message's text is
/// lifted ~`font_size*0.4 + 3` above its line; we add a full label height plus a
/// gap on top of that so the two never touch.
fn frame_pad_top(fs: f32) -> f32 {
    // Base tab/label band + ~font_size * 1.4 of clearance.
    FRAME_TAB_H + fs * 1.4 + 6.0
}

/// Vertical headroom reserved after a section divider before its first row, so
/// the section label (drawn `font_size*0.7` below the divider, ~`font_size`
/// tall) clears the next message's lifted text label.
fn section_pad_top(fs: f32) -> f32 {
    fs * 1.6 + 12.0
}

/// Heuristic label width (font-free), matching the flowchart `measure` rule.
/// Used for non-label decorations (the frame keyword tab) where rich markup
/// never applies.
fn label_width(label: &str, font_size: f32) -> f32 {
    label.chars().count() as f32 * font_size * CHAR_ADVANCE_EM
}

/// Rich-aware label width: equals [`label_width`] for plain labels but accounts
/// for markdown/math in message, note, and participant labels.
fn rich_label_width(label: &str, font_size: f32) -> f32 {
    crate::label::measure(label, font_size).0
}

/// Participant box width for a label (with padding + minimum).
fn box_width(label: &str, font_size: f32) -> f32 {
    (rich_label_width(label, font_size) + 2.0 * BOX_PAD_X).max(MIN_BOX_W)
}

// ----------------------------------------------------------------------------
// Render
// ----------------------------------------------------------------------------

/// Render mermaid sequence-diagram source to an SVG document.
pub fn render_sequence(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let diagram = parse_sequence(src).map_err(MermaidError::Parse)?;
    if diagram.participants.is_empty() {
        return Err(MermaidError::Empty);
    }
    Ok(draw_sequence(&diagram, opts))
}

// ----------------------------------------------------------------------------
// Layout records
// ----------------------------------------------------------------------------

/// A message ready to draw at a row y, with resolved column endpoints and the
/// activation-edge x positions (so arrows can land on the activation bar rather
/// than the bare lifeline when one is active).
struct PlacedMessage<'a> {
    msg: &'a Message,
    y: f32,
    from_col: usize,
    to_col: usize,
    /// x where the line should start (the active edge of `from`'s bar, or its
    /// lifeline center).
    from_x: f32,
    /// x where the line should end (the active edge of `to`'s bar, or its
    /// lifeline center).
    to_x: f32,
}

/// A note ready to draw.
struct PlacedNote<'a> {
    note: &'a Note,
    y: f32,
    /// Rectangle left/right x and the center for the text.
    x0: f32,
    x1: f32,
}

/// A block frame ready to draw.
struct PlacedFrame {
    kind: BlockKind,
    label: String,
    x0: f32,
    x1: f32,
    y0: f32,
    y1: f32,
    /// (divider y, section label) for each else/and/option section.
    dividers: Vec<(f32, String)>,
}

/// A `rect` background highlight ready to draw: a color + the rectangle that
/// spans the contained rows and involved participants. Drawn behind everything.
struct PlacedRect {
    color: Rgba,
    x0: f32,
    x1: f32,
    y0: f32,
    y1: f32,
}

/// An activation bar ready to draw: a column, nesting level (for x offset), and
/// vertical span.
struct PlacedAct {
    col: usize,
    level: usize,
    y0: f32,
    y1: f32,
}

/// Mutable state threaded through the layout walk.
struct Layout<'a> {
    centers: &'a [f32],
    /// Per-participant stack of open activations: each entry is the y where the
    /// bar started. Stack depth = nesting level.
    act_stacks: Vec<Vec<f32>>,
    messages: Vec<PlacedMessage<'a>>,
    notes: Vec<PlacedNote<'a>>,
    frames: Vec<PlacedFrame>,
    rects: Vec<PlacedRect>,
    acts: Vec<PlacedAct>,
    /// Tracks the widest x reached (for canvas sizing).
    max_x: f32,
}

impl<'a> Layout<'a> {
    /// Center x of the currently-active edge of `col` on the right side: the
    /// outer x of its top-of-stack activation bar, used so an arrow arriving at
    /// an active participant lands on the bar.
    fn active_edge(&self, col: usize, from_left: bool) -> f32 {
        let cx = self.centers[col];
        let depth = self.act_stacks[col].len();
        if depth == 0 {
            return cx;
        }
        // Outer edge of the top bar.
        let level = depth - 1;
        let half = ACT_W / 2.0 + level as f32 * ACT_NEST_DX;
        if from_left { cx - half } else { cx + half }
    }
}

/// Lay out + draw the parsed diagram into an SVG document.
fn draw_sequence(diagram: &SequenceDiagram, opts: &MermaidOptions) -> MermaidRender {
    let fs = opts.font_size_px;
    let box_h = fs * 1.2 + 2.0 * BOX_PAD_Y;

    // --- Columns: each participant gets a center x. Uniform column width =
    // widest box, so lifelines are evenly spaced. ---
    let widths: Vec<f32> = diagram.participants.iter().map(|p| box_width(&p.label, fs)).collect();
    let col_w = widths.iter().cloned().fold(MIN_BOX_W, f32::max);

    let n = diagram.participants.len();
    let mut centers: Vec<f32> = Vec::with_capacity(n);
    for i in 0..n {
        let cx = MARGIN + col_w / 2.0 + i as f32 * (col_w + COL_GAP);
        centers.push(cx);
    }

    // Index id → column.
    let col_of = |id: &str| -> Option<usize> {
        diagram.participants.iter().position(|p| p.id == id)
    };

    // --- Vertical extents ---
    let box_top = MARGIN;
    let box_bottom = box_top + box_h;
    let first_row_y = box_bottom + TOP_GAP;

    // --- Walk the item tree assigning y to leaves, computing frame extents and
    // activation spans. ---
    let mut lay = Layout {
        centers: &centers,
        act_stacks: vec![Vec::new(); n],
        messages: Vec::new(),
        notes: Vec::new(),
        frames: Vec::new(),
        rects: Vec::new(),
        acts: Vec::new(),
        max_x: centers.last().copied().unwrap_or(MARGIN + col_w / 2.0) + col_w / 2.0,
    };

    let mut y = first_row_y;
    layout_items(&mut lay, &diagram.items, &col_of, fs, &mut y, 0);

    // Close any activations left open at end of script: run them to the bottom.
    let content_bottom = (y - MESSAGE_GAP * 0.5).max(first_row_y);
    for col in 0..n {
        let stack = std::mem::take(&mut lay.act_stacks[col]);
        for (level, y0) in stack.into_iter().enumerate() {
            lay.acts.push(PlacedAct { col, level, y0, y1: content_bottom });
        }
    }

    let messages_bottom = content_bottom + MESSAGE_GAP * 0.5;

    // --- Canvas size ---
    let last_cx = centers.last().copied().unwrap_or(MARGIN + col_w / 2.0);
    let right_extent = (last_cx + col_w / 2.0 + SELF_LOOP_W + MARGIN).max(lay.max_x + MARGIN);
    let width = right_extent.max(MARGIN * 2.0 + col_w);
    let height = messages_bottom + MARGIN;

    let mut svg = String::new();
    let w = (width.ceil() + 1.0).max(1.0);
    let h = (height.ceil() + 1.0).max(1.0);
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"0 0 {w} {h}\">"
    );

    // Shared filled-arrowhead marker.
    emit_defs(&mut svg, opts);

    // --- Rect background highlights (drawn first, behind everything) ---
    for rb in &lay.rects {
        emit_rect_bg(&mut svg, rb);
    }

    // --- Frames (behind lifelines/messages, above rect backgrounds) ---
    for f in &lay.frames {
        emit_frame(&mut svg, f, opts);
    }

    // --- Lifelines (dashed verticals) ---
    let (edge, eo) = stroke_attrs(opts.edge_stroke);
    for &cx in &centers {
        let _ = write!(
            svg,
            "<line x1=\"{cx:.2}\" y1=\"{y1:.2}\" x2=\"{cx:.2}\" y2=\"{y2:.2}\" \
             stroke=\"{edge}\"{eo} stroke-width=\"{STROKE_W}\" stroke-dasharray=\"3 3\"/>",
            y1 = box_bottom,
            y2 = messages_bottom,
        );
    }

    // --- Activation bars (over lifelines, under messages) ---
    for a in &lay.acts {
        emit_activation(&mut svg, centers[a.col], a, opts);
    }

    // --- Participant boxes (rect + centered label) ---
    let (fill, fo) = fill_attrs(opts.node_fill);
    let (nstroke, nso) = stroke_attrs(opts.node_stroke);
    for (i, p) in diagram.participants.iter().enumerate() {
        let cx = centers[i];
        let bw = widths[i].max(MIN_BOX_W);
        let x = cx - bw / 2.0;
        let _ = write!(
            svg,
            "<rect x=\"{x:.2}\" y=\"{box_top:.2}\" width=\"{bw:.2}\" height=\"{box_h:.2}\" \
             rx=\"3\" ry=\"3\" fill=\"{fill}\"{fo} stroke=\"{nstroke}\"{nso} stroke-width=\"{STROKE_W}\"/>",
        );
        emit_text(&mut svg, &p.label, cx, box_top + box_h / 2.0, opts);
    }

    // --- Notes ---
    for pn in &lay.notes {
        emit_note(&mut svg, pn, opts);
    }

    // --- Messages ---
    for pm in &lay.messages {
        if pm.from_col == pm.to_col {
            emit_self_message(&mut svg, lay.centers[pm.from_col], pm.y, pm.msg, opts);
        } else {
            emit_message(&mut svg, pm.from_x, pm.to_x, pm.y, pm.msg, opts);
        }
    }

    svg.push_str("</svg>");

    MermaidRender { svg, width_px: w, height_px: h }
}

/// Recursively place items, advancing `*y` for each leaf row and recording
/// frames/activations. `depth` is the current block-nesting depth (for insets).
fn layout_items<'a>(
    lay: &mut Layout<'a>,
    items: &'a [Item],
    col_of: &impl Fn(&str) -> Option<usize>,
    fs: f32,
    y: &mut f32,
    depth: usize,
) {
    for item in items {
        match item {
            Item::Message(m) => {
                let from_col = col_of(&m.from);
                let to_col = col_of(&m.to);
                let (Some(fc), Some(tc)) = (from_col, to_col) else { continue };
                let my = *y;

                // Activation: `+` activates the target on arrival.
                if m.activate_to {
                    lay.act_stacks[tc].push(my);
                }
                // Compute landing edges using the *current* activation depth
                // (after a possible +activate, so the arrow lands on the new
                // bar's outer edge facing the sender).
                let going_right = lay.centers[tc] >= lay.centers[fc];
                let from_x = lay.active_edge(fc, !going_right);
                let to_x = lay.active_edge(tc, going_right);

                lay.messages.push(PlacedMessage {
                    msg: m,
                    y: my,
                    from_col: fc,
                    to_col: tc,
                    from_x,
                    to_x,
                });

                // `-` deactivates the sender's current activation on send.
                if m.deactivate_from {
                    if let Some(y0) = lay.act_stacks[fc].pop() {
                        let level = lay.act_stacks[fc].len();
                        lay.acts.push(PlacedAct { col: fc, level, y0, y1: my });
                    }
                }

                if m.from == m.to {
                    *y += SELF_LOOP_H + MESSAGE_GAP * 0.5;
                } else {
                    *y += MESSAGE_GAP;
                }
            }
            Item::Note(note) => {
                let (x0, x1) = note_extents(lay, note, col_of, fs);
                lay.max_x = lay.max_x.max(x1);
                lay.notes.push(PlacedNote { note, y: *y, x0, x1 });
                *y += NOTE_GAP;
            }
            Item::Activate(id) => {
                if let Some(c) = col_of(id) {
                    lay.act_stacks[c].push(*y - MESSAGE_GAP * 0.3);
                }
            }
            Item::Deactivate(id) => {
                if let Some(c) = col_of(id) {
                    if let Some(y0) = lay.act_stacks[c].pop() {
                        let level = lay.act_stacks[c].len();
                        lay.acts.push(PlacedAct { col: c, level, y0, y1: *y - MESSAGE_GAP * 0.3 });
                    }
                }
            }
            Item::Block(b) => {
                layout_block(lay, b, col_of, fs, y, depth);
            }
            Item::Rect(rb) => {
                layout_rect(lay, rb, col_of, fs, y, depth);
            }
        }
    }
}

/// Place a `rect` background highlight: lay out its children (advancing `*y`),
/// then record a background rectangle spanning those rows and the involved
/// participants. No label tab. Nesting depth widens the span slightly so a rect
/// inside another frame still reads as a band.
fn layout_rect<'a>(
    lay: &mut Layout<'a>,
    rb: &'a RectBlock,
    col_of: &impl Fn(&str) -> Option<usize>,
    fs: f32,
    y: &mut f32,
    depth: usize,
) {
    // A little top padding so the band doesn't clip the first row's lifted text.
    let pad = MESSAGE_GAP * 0.35;
    let y_top = *y;
    *y += pad;
    layout_items(lay, &rb.items, col_of, fs, y, depth);
    let y_bottom = *y - MESSAGE_GAP * 0.25 + pad;
    *y = y_bottom + MESSAGE_GAP * 0.2;

    // Horizontal span across the involved participants (fall back to all).
    let (lo, hi) =
        rect_col_span(&rb.items, col_of).unwrap_or((0, lay.centers.len().saturating_sub(1)));
    let inset = depth as f32 * FRAME_INSET;
    let left = lay.centers[lo] - col_half(lay, fs) - FRAME_INSET - inset;
    let right = lay.centers[hi] + col_half(lay, fs) + FRAME_INSET + inset;
    lay.max_x = lay.max_x.max(right);

    lay.rects.push(PlacedRect {
        color: rb.color,
        x0: left,
        x1: right,
        y0: y_top,
        y1: y_bottom,
    });
}

/// Place a block: reserve top padding, lay out children (tracking divider ys),
/// reserve bottom padding, and record the frame rectangle spanning the involved
/// participants.
fn layout_block<'a>(
    lay: &mut Layout<'a>,
    block: &'a Block,
    col_of: &impl Fn(&str) -> Option<usize>,
    fs: f32,
    y: &mut f32,
    depth: usize,
) {
    let pad_top = frame_pad_top(fs);
    let y_top = *y;
    *y += pad_top;
    let first_child_y = *y;

    // Section divider ys, keyed by child index where the section begins.
    let mut div_by_idx: Vec<(usize, String)> = block.sections.clone();
    div_by_idx.sort_by_key(|(i, _)| *i);
    let mut next_div = 0;
    let mut dividers: Vec<(f32, String)> = Vec::new();

    for (idx, child) in block.items.iter().enumerate() {
        // Emit any divider that begins at this child index (before placing it).
        while next_div < div_by_idx.len() && div_by_idx[next_div].0 == idx {
            // Place the divider a little above the upcoming row, then reserve
            // headroom below it so the section label clears that row's text.
            let dy = (*y - MESSAGE_GAP * 0.2).max(first_child_y - pad_top * 0.4);
            dividers.push((dy, div_by_idx[next_div].1.clone()));
            *y += section_pad_top(fs);
            next_div += 1;
        }
        layout_items(lay, std::slice::from_ref(child), col_of, fs, y, depth + 1);
    }
    // Dividers that begin after the last child (empty trailing section).
    while next_div < div_by_idx.len() {
        dividers.push((*y - MESSAGE_GAP * 0.2, div_by_idx[next_div].1.clone()));
        next_div += 1;
    }

    let y_bottom = *y + FRAME_PAD_BOTTOM;
    *y = y_bottom + MESSAGE_GAP * 0.2;

    // Horizontal span: the min/max column involved by any descendant. Fall back
    // to all columns if none resolved.
    let (lo, hi) = block_col_span(block, col_of).unwrap_or((0, lay.centers.len().saturating_sub(1)));
    let inset = depth as f32 * FRAME_INSET;
    let left = lay.centers[lo] - col_half(lay, fs) - FRAME_INSET - inset;
    let right = lay.centers[hi] + col_half(lay, fs) + FRAME_INSET + inset;
    lay.max_x = lay.max_x.max(right);

    lay.frames.push(PlacedFrame {
        kind: block.kind,
        label: block.label.clone(),
        x0: left,
        x1: right,
        y0: y_top,
        y1: y_bottom,
        dividers,
    });

    let _ = first_child_y;
}

/// Half the inter-lifeline padding to extend a frame past edge lifelines.
fn col_half(_lay: &Layout, _fs: f32) -> f32 {
    COL_GAP / 2.0
}

/// Compute the min/max participant column referenced anywhere inside a block
/// (messages' endpoints, note targets, nested blocks). `None` if nothing maps.
fn block_col_span(block: &Block, col_of: &impl Fn(&str) -> Option<usize>) -> Option<(usize, usize)> {
    rect_col_span(&block.items, col_of)
}

/// Compute the min/max participant column referenced anywhere within `items`
/// (messages' endpoints, note targets, nested blocks/rects). `None` if nothing
/// maps. Shared by labeled-block frames and `rect` background blocks.
fn rect_col_span(items: &[Item], col_of: &impl Fn(&str) -> Option<usize>) -> Option<(usize, usize)> {
    let mut lo = usize::MAX;
    let mut hi = 0usize;
    let mut any = false;
    fn visit(
        items: &[Item],
        col_of: &impl Fn(&str) -> Option<usize>,
        lo: &mut usize,
        hi: &mut usize,
        any: &mut bool,
    ) {
        for it in items {
            let note = |c: usize, lo: &mut usize, hi: &mut usize, any: &mut bool| {
                *lo = (*lo).min(c);
                *hi = (*hi).max(c);
                *any = true;
            };
            match it {
                Item::Message(m) => {
                    if let Some(c) = col_of(&m.from) { note(c, lo, hi, any); }
                    if let Some(c) = col_of(&m.to) { note(c, lo, hi, any); }
                }
                Item::Note(n) => {
                    for t in &n.targets {
                        if let Some(c) = col_of(t) { note(c, lo, hi, any); }
                    }
                }
                Item::Activate(id) | Item::Deactivate(id) => {
                    if let Some(c) = col_of(id) { note(c, lo, hi, any); }
                }
                Item::Block(b) => visit(&b.items, col_of, lo, hi, any),
                Item::Rect(rb) => visit(&rb.items, col_of, lo, hi, any),
            }
        }
    }
    visit(items, col_of, &mut lo, &mut hi, &mut any);
    if any { Some((lo, hi)) } else { None }
}

/// Compute a note's rectangle x extents for the given placement/targets.
fn note_extents(
    lay: &Layout,
    note: &Note,
    col_of: &impl Fn(&str) -> Option<usize>,
    fs: f32,
) -> (f32, f32) {
    let text_w = rich_label_width(&note.text, fs) + 2.0 * NOTE_PAD_X;
    let w = text_w.max(NOTE_MIN_W);
    match note.placement {
        NotePlacement::LeftOf => {
            let c = note.targets.first().and_then(|t| col_of(t)).unwrap_or(0);
            let cx = lay.centers[c];
            let x1 = cx - NOTE_SIDE_GAP;
            (x1 - w, x1)
        }
        NotePlacement::RightOf => {
            let c = note.targets.first().and_then(|t| col_of(t)).unwrap_or(0);
            let cx = lay.centers[c];
            let x0 = cx + NOTE_SIDE_GAP;
            (x0, x0 + w)
        }
        NotePlacement::Over => {
            let cols: Vec<usize> =
                note.targets.iter().filter_map(|t| col_of(t)).collect();
            if cols.is_empty() {
                let cx = lay.centers.first().copied().unwrap_or(MARGIN);
                return (cx - w / 2.0, cx + w / 2.0);
            }
            let lo = *cols.iter().min().unwrap();
            let hi = *cols.iter().max().unwrap();
            let cl = lay.centers[lo];
            let cr = lay.centers[hi];
            let mid = (cl + cr) / 2.0;
            // Span at least the lifelines plus padding, or the text width.
            let span = (cr - cl + 2.0 * NOTE_PAD_X * 2.0).max(w);
            (mid - span / 2.0, mid + span / 2.0)
        }
    }
}

/// `<defs>` with the filled-triangle end marker (oriented along the path).
fn emit_defs(svg: &mut String, opts: &MermaidOptions) {
    let (fill, fo) = fill_attrs(opts.edge_stroke);
    let _ = write!(
        svg,
        "<defs><marker id=\"seq-arrow\" markerWidth=\"{len}\" markerHeight=\"{w}\" \
         refX=\"{len}\" refY=\"{half}\" orient=\"auto\" markerUnits=\"userSpaceOnUse\">\
         <path d=\"M0,0 L{len},{half} L0,{w} Z\" fill=\"{fill}\"{fo}/></marker></defs>",
        len = ARROW_LEN,
        w = ARROW_HALF * 2.0,
        half = ARROW_HALF,
    );
}

/// A normal (cross-lifeline) message: horizontal line + head + centered text.
fn emit_message(svg: &mut String, x_from: f32, x_to: f32, y: f32, m: &Message, opts: &MermaidOptions) {
    let (edge, eo) = stroke_attrs(opts.edge_stroke);
    let dash = if m.dashed { " stroke-dasharray=\"4 3\"" } else { "" };

    // Pull the line back from the target so the head's tip lands on the lifeline.
    let dir = if x_to >= x_from { 1.0 } else { -1.0 };
    let line_end = match m.style {
        ArrowStyle::Filled => x_to - dir * ARROW_LEN,
        _ => x_to,
    };
    let marker = if m.style == ArrowStyle::Filled {
        " marker-end=\"url(#seq-arrow)\""
    } else {
        ""
    };

    let _ = write!(
        svg,
        "<line x1=\"{x_from:.2}\" y1=\"{y:.2}\" x2=\"{line_end:.2}\" y2=\"{y:.2}\" \
         stroke=\"{edge}\"{eo} stroke-width=\"{STROKE_W}\"{dash}{marker}/>",
    );

    // Open / async heads: a small V at the target. Cross: an ✗.
    match m.style {
        ArrowStyle::Open | ArrowStyle::Async => emit_open_head(svg, x_to, y, dir, opts),
        ArrowStyle::Cross => emit_cross(svg, x_to, y, opts),
        ArrowStyle::Filled => {}
    }

    // Autonumber badge at the sending end, just inside the line start.
    if let Some(num) = m.number {
        let bx = x_from + dir * (BADGE_R + 2.0);
        emit_number_badge(svg, bx, y, num, opts);
    }

    // Text centered clearly above the line (lift it so descenders clear the
    // line, not sitting on top of it).
    if !m.text.is_empty() {
        let cx = (x_from + x_to) / 2.0;
        let ty = y - (opts.font_size_px * 0.4 + 3.0);
        emit_text(svg, &m.text, cx, ty, opts);
    }
}

/// A self-message: a small rectangular loop to the right of the lifeline, with
/// a head returning to the lifeline and the text to the loop's right.
fn emit_self_message(svg: &mut String, cx: f32, y: f32, m: &Message, opts: &MermaidOptions) {
    let (edge, eo) = stroke_attrs(opts.edge_stroke);
    let dash = if m.dashed { " stroke-dasharray=\"4 3\"" } else { "" };

    let right = cx + SELF_LOOP_W;
    let y0 = y;
    let y1 = y + SELF_LOOP_H;

    // Out from lifeline, down, back toward lifeline (stop short for the head).
    let back_end = match m.style {
        ArrowStyle::Filled => cx + ARROW_LEN,
        _ => cx,
    };
    let marker = if m.style == ArrowStyle::Filled {
        " marker-end=\"url(#seq-arrow)\""
    } else {
        ""
    };

    let _ = write!(
        svg,
        "<path d=\"M{cx:.2},{y0:.2} L{right:.2},{y0:.2} L{right:.2},{y1:.2} L{back_end:.2},{y1:.2}\" \
         fill=\"none\" stroke=\"{edge}\"{eo} stroke-width=\"{STROKE_W}\"{dash}{marker}/>",
    );

    // Heads pointing left (dir = -1) back at the lifeline on the lower segment.
    match m.style {
        ArrowStyle::Open | ArrowStyle::Async => emit_open_head(svg, cx, y1, -1.0, opts),
        ArrowStyle::Cross => emit_cross(svg, cx, y1, opts),
        ArrowStyle::Filled => {}
    }

    // Autonumber badge at the top-out point of the loop.
    if let Some(num) = m.number {
        emit_number_badge(svg, cx + BADGE_R + 2.0, y0, num, opts);
    }

    if !m.text.is_empty() {
        let tx = right + 4.0;
        let ty = (y0 + y1) / 2.0;
        emit_text_left(svg, &m.text, tx, ty, opts);
    }
}

/// Draw a small themed circular badge containing the autonumber `num`, centered
/// at `(cx, cy)`. Filled with the node fill / stroke so it matches the theme.
fn emit_number_badge(svg: &mut String, cx: f32, cy: f32, num: u32, opts: &MermaidOptions) {
    let (fill, fo) = fill_attrs(opts.node_fill);
    let (stroke, so) = stroke_attrs(opts.node_stroke);
    let _ = write!(
        svg,
        "<circle cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{r:.2}\" \
         fill=\"{fill}\"{fo} stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
        r = BADGE_R,
    );
    // Number text: slightly smaller than the body font so it fits the badge.
    let small = opts.font_size_px * 0.8;
    crate::label::emit(
        svg,
        &num.to_string(),
        cx,
        cy,
        crate::label::Anchor::Middle,
        small,
        opts.text_color,
        &opts.font_family,
    );
}

/// Draw a `rect` background highlight: a translucent filled rectangle behind the
/// messages in its span. No stroke, no label.
fn emit_rect_bg(svg: &mut String, rb: &PlacedRect) {
    let c = rb.color;
    let w = (rb.x1 - rb.x0).max(0.0);
    let h = (rb.y1 - rb.y0).max(0.0);
    let opacity = c.a as f32 / 255.0;
    let _ = write!(
        svg,
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" \
         fill=\"rgb({r},{g},{b})\" fill-opacity=\"{op:.4}\"/>",
        x = rb.x0,
        y = rb.y0,
        r = c.r,
        g = c.g,
        b = c.b,
        op = opacity,
    );
}

/// Draw a labeled block frame: outer rectangle, a keyword tab in the top-left,
/// the opening label centered along the top, and dashed dividers per section.
fn emit_frame(svg: &mut String, f: &PlacedFrame, opts: &MermaidOptions) {
    let (stroke, so) = stroke_attrs(opts.node_stroke);
    let w = f.x1 - f.x0;
    let hgt = f.y1 - f.y0;

    // Outer rectangle (transparent fill so content shows through).
    let _ = write!(
        svg,
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{hgt:.2}\" \
         fill=\"none\" stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
        x = f.x0,
        y = f.y0,
    );

    // Keyword tab: a small filled rectangle with a notched bottom-right corner.
    let kw = f.kind.keyword();
    let tab_w = (label_width(kw, opts.font_size_px) + 14.0).max(34.0);
    let notch = 8.0;
    let (tfill, tfo) = fill_attrs(opts.node_fill);
    let tx = f.x0;
    let ty = f.y0;
    let _ = write!(
        svg,
        "<path d=\"M{x:.2},{y:.2} L{xr:.2},{y:.2} L{xr:.2},{yb0:.2} L{xn:.2},{yb1:.2} L{x:.2},{yb1:.2} Z\" \
         fill=\"{tfill}\"{tfo} stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
        x = tx,
        y = ty,
        xr = tx + tab_w,
        xn = tx + tab_w - notch,
        yb0 = ty + FRAME_TAB_H - notch,
        yb1 = ty + FRAME_TAB_H,
    );
    emit_text(svg, kw, tx + tab_w / 2.0 - notch / 2.0, ty + FRAME_TAB_H / 2.0, opts);

    // Opening label centered in the space to the right of the keyword tab, but
    // never overlapping it: if centering would push the label's left edge over
    // the tab, shift the whole label right so it starts past the tab + a gap.
    if !f.label.is_empty() {
        let tab_right = tx + tab_w;
        let label_w = label_width(&f.label, opts.font_size_px);
        let gap = 6.0;
        let mut cx = (tab_right + f.x1) / 2.0;
        let min_cx = tab_right + gap + label_w / 2.0;
        if cx < min_cx {
            cx = min_cx;
        }
        emit_text(svg, &f.label, cx, ty + FRAME_TAB_H / 2.0, opts);
    }

    // Section dividers (dashed horizontal line + that section's label).
    for (dy, slabel) in &f.dividers {
        let _ = write!(
            svg,
            "<line x1=\"{x0:.2}\" y1=\"{dy:.2}\" x2=\"{x1:.2}\" y2=\"{dy:.2}\" \
             stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\" stroke-dasharray=\"3 3\"/>",
            x0 = f.x0,
            x1 = f.x1,
        );
        if !slabel.is_empty() {
            let cx = (f.x0 + f.x1) / 2.0;
            emit_text(svg, slabel, cx, dy + opts.font_size_px * 0.7, opts);
        }
    }
}

/// Draw an activation bar: a narrow themed rectangle on a lifeline, offset
/// horizontally by its nesting level.
fn emit_activation(svg: &mut String, cx: f32, a: &PlacedAct, opts: &MermaidOptions) {
    let (fill, fo) = fill_attrs(opts.node_fill);
    let (stroke, so) = stroke_attrs(opts.node_stroke);
    let dx = a.level as f32 * ACT_NEST_DX;
    let x = cx - ACT_W / 2.0 + dx;
    let y = a.y0;
    let h = (a.y1 - a.y0).max(1.0);
    let _ = write!(
        svg,
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{ACT_W:.2}\" height=\"{h:.2}\" \
         fill=\"{fill}\"{fo} stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
    );
}

/// Draw a note: a themed rectangle (pale fill) with wrapped/centered text.
fn emit_note(svg: &mut String, pn: &PlacedNote, opts: &MermaidOptions) {
    let (stroke, so) = stroke_attrs(opts.node_stroke);
    // Pale-yellow note fill (mermaid-like), opaque.
    let fill = "rgb(255,255,221)";
    let h = NOTE_GAP - 2.0 * NOTE_PAD_Y;
    let y = pn.y - h / 2.0;
    let w = (pn.x1 - pn.x0).max(NOTE_MIN_W);
    let _ = write!(
        svg,
        "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" \
         rx=\"2\" ry=\"2\" fill=\"{fill}\" stroke=\"{stroke}\"{so} stroke-width=\"{STROKE_W}\"/>",
        x = pn.x0,
    );
    let cx = (pn.x0 + pn.x1) / 2.0;
    emit_text(svg, &pn.note.text, cx, pn.y, opts);
}

/// An open ("V") arrowhead at `(x, y)` pointing in `dir` (+1 right / -1 left).
fn emit_open_head(svg: &mut String, x: f32, y: f32, dir: f32, opts: &MermaidOptions) {
    let (edge, eo) = stroke_attrs(opts.edge_stroke);
    let bx = x - dir * ARROW_LEN;
    let _ = write!(
        svg,
        "<path d=\"M{bx:.2},{ty:.2} L{x:.2},{y:.2} L{bx:.2},{by:.2}\" \
         fill=\"none\" stroke=\"{edge}\"{eo} stroke-width=\"{STROKE_W}\"/>",
        ty = y - ARROW_HALF,
        by = y + ARROW_HALF,
    );
}

/// A small cross (`✗`) end-marker centered at `(x, y)`.
fn emit_cross(svg: &mut String, x: f32, y: f32, opts: &MermaidOptions) {
    let (edge, eo) = stroke_attrs(opts.edge_stroke);
    let r = ARROW_HALF;
    let _ = write!(
        svg,
        "<path d=\"M{x0:.2},{y0:.2} L{x1:.2},{y1:.2} M{x0:.2},{y1:.2} L{x1:.2},{y0:.2}\" \
         fill=\"none\" stroke=\"{edge}\"{eo} stroke-width=\"{STROKE_W}\"/>",
        x0 = x - r,
        x1 = x + r,
        y0 = y - r,
        y1 = y + r,
    );
}

// ----------------------------------------------------------------------------
// Text + color helpers (mirrors draw.rs conventions)
// ----------------------------------------------------------------------------

/// A centered single-line label at `(cx, cy)`, routed through the rich-label
/// renderer so message/note/participant labels support markdown (`**bold**`,
/// `*italic*`, `<br>`) and inline math (`$…$`). Plain labels emit a single
/// centered `<text>` identical to the previous output.
fn emit_text(svg: &mut String, label: &str, cx: f32, cy: f32, opts: &MermaidOptions) {
    crate::label::emit(
        svg,
        label,
        cx,
        cy,
        crate::label::Anchor::Middle,
        opts.font_size_px,
        opts.text_color,
        &opts.font_family,
    );
}

/// A left-anchored single-line label at `(x, cy)` (for self-message labels),
/// routed through the rich-label renderer (see [`emit_text`]).
fn emit_text_left(svg: &mut String, label: &str, x: f32, cy: f32, opts: &MermaidOptions) {
    crate::label::emit(
        svg,
        label,
        x,
        cy,
        crate::label::Anchor::Start,
        opts.font_size_px,
        opts.text_color,
        &opts.font_family,
    );
}

/// `fill="rgb(r,g,b)"` plus optional ` fill-opacity`.
fn fill_attrs(color: [u8; 4]) -> (String, String) {
    let [r, g, b, a] = color;
    let fill = format!("rgb({r},{g},{b})");
    let opacity = if a < 255 {
        format!(" fill-opacity=\"{:.4}\"", a as f32 / 255.0)
    } else {
        String::new()
    };
    (fill, opacity)
}

/// Same as [`fill_attrs`] but the opacity attribute is `stroke-opacity`.
fn stroke_attrs(color: [u8; 4]) -> (String, String) {
    let [r, g, b, a] = color;
    let stroke = format!("rgb({r},{g},{b})");
    let opacity = if a < 255 {
        format!(" stroke-opacity=\"{:.4}\"", a as f32 / 255.0)
    } else {
        String::new()
    };
    (stroke, opacity)
}

// ----------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> MermaidOptions {
        MermaidOptions::default()
    }

    /// Flatten the item tree (recursing into blocks) into the messages it
    /// contains, in order — keeps the older message-shape tests concise.
    fn flat_messages(d: &SequenceDiagram) -> Vec<Message> {
        fn walk(items: &[Item], out: &mut Vec<Message>) {
            for it in items {
                match it {
                    Item::Message(m) => out.push(m.clone()),
                    Item::Block(b) => walk(&b.items, out),
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&d.items, &mut out);
        out
    }

    // --- Parsing ---

    #[test]
    fn parses_declared_and_alias_participants() {
        let d = parse_sequence(
            "sequenceDiagram\n participant A\n participant B as Bob\n actor C\n A->>B: hi\n",
        )
        .unwrap();
        assert_eq!(d.participants.len(), 3);
        assert_eq!(d.participants[0], Participant { id: "A".into(), label: "A".into() });
        assert_eq!(d.participants[1], Participant { id: "B".into(), label: "Bob".into() });
        assert_eq!(d.participants[2], Participant { id: "C".into(), label: "C".into() });
    }

    #[test]
    fn auto_creates_undeclared_participants_in_first_appearance_order() {
        let d = parse_sequence("sequenceDiagram\n X->>Y: hi\n Y-->>Z: bye\n").unwrap();
        let ids: Vec<_> = d.participants.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["X", "Y", "Z"]);
    }

    #[test]
    fn parses_each_arrow_kind() {
        let d = parse_sequence(
            "sequenceDiagram\n A->>B: a\n A-->>B: b\n A->B: c\n A-->B: d\n A-)B: e\n A-xB: f\n A--xB: g\n",
        )
        .unwrap();
        let msgs = flat_messages(&d);
        assert_eq!(msgs.len(), 7);
        assert_eq!(msgs[0].style, ArrowStyle::Filled);
        assert!(!msgs[0].dashed);
        assert_eq!(msgs[1].style, ArrowStyle::Filled);
        assert!(msgs[1].dashed);
        assert_eq!(msgs[2].style, ArrowStyle::Open);
        assert_eq!(msgs[3].style, ArrowStyle::Open);
        assert!(msgs[3].dashed);
        assert_eq!(msgs[4].style, ArrowStyle::Async);
        assert_eq!(msgs[5].style, ArrowStyle::Cross);
        assert_eq!(msgs[6].style, ArrowStyle::Cross);
        assert!(msgs[6].dashed);
    }

    #[test]
    fn parses_message_text_and_endpoints() {
        let d = parse_sequence("sequenceDiagram\n Alice ->> Bob : Hello Bob\n").unwrap();
        let msgs = flat_messages(&d);
        let m = &msgs[0];
        assert_eq!(m.from, "Alice");
        assert_eq!(m.to, "Bob");
        assert_eq!(m.text, "Hello Bob");
    }

    #[test]
    fn parses_self_message() {
        let d = parse_sequence("sequenceDiagram\n A->>A: think\n").unwrap();
        let msgs = flat_messages(&d);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].from, "A");
        assert_eq!(msgs[0].to, "A");
        // Only one participant created.
        assert_eq!(d.participants.len(), 1);
    }

    #[test]
    fn parses_blocks_and_notes_into_tree() {
        let d = parse_sequence(
            "sequenceDiagram\n participant A\n participant B\n loop every minute\n A->>B: ping\n end\n Note over A,B: hi\n",
        )
        .unwrap();
        assert_eq!(d.participants.len(), 2);
        // The loop is a Block; the note follows at top level.
        assert_eq!(d.items.len(), 2);
        match &d.items[0] {
            Item::Block(b) => {
                assert_eq!(b.kind, BlockKind::Loop);
                assert_eq!(b.label, "every minute");
                assert_eq!(b.items.len(), 1);
                assert!(matches!(b.items[0], Item::Message(_)));
            }
            other => panic!("expected a loop block, got {other:?}"),
        }
        match &d.items[1] {
            Item::Note(n) => {
                assert_eq!(n.placement, NotePlacement::Over);
                assert_eq!(n.targets, vec!["A".to_string(), "B".to_string()]);
                assert_eq!(n.text, "hi");
            }
            other => panic!("expected a note, got {other:?}"),
        }
        // Still exactly one message total.
        assert_eq!(flat_messages(&d).len(), 1);
    }

    #[test]
    fn alias_updates_label_for_message_first_id() {
        // B appears in a message first, then a declaration aliases it.
        let d = parse_sequence("sequenceDiagram\n A->>B: hi\n participant B as Bob\n").unwrap();
        let b = d.participants.iter().find(|p| p.id == "B").unwrap();
        assert_eq!(b.label, "Bob");
    }

    #[test]
    fn missing_header_is_parse_error() {
        assert!(parse_sequence("participant A\n A->>B: hi\n").is_err());
    }

    #[test]
    fn malformed_message_is_parse_error() {
        // No arrow token.
        assert!(parse_sequence("sequenceDiagram\n A B C\n").is_err());
    }

    // --- Rendering ---

    #[test]
    fn renders_svg_envelope() {
        let r = render_sequence("sequenceDiagram\n A->>B: hi\n", &opts()).unwrap();
        assert!(r.svg.starts_with("<svg"), "got {}", &r.svg[..r.svg.len().min(40)]);
        assert!(r.svg.trim_end().ends_with("</svg>"));
        assert!(r.svg.contains("viewBox="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn one_box_and_lifeline_per_participant() {
        let r = render_sequence(
            "sequenceDiagram\n participant A\n participant B\n A->>B: hi\n",
            &opts(),
        )
        .unwrap();
        // Two participant <rect>s.
        assert_eq!(r.svg.matches("<rect").count(), 2);
        // Two dashed lifelines (3 3 dash); message dashes use 4 3.
        assert_eq!(r.svg.matches("stroke-dasharray=\"3 3\"").count(), 2);
    }

    #[test]
    fn message_line_and_text_present() {
        let r = render_sequence("sequenceDiagram\n A->>B: Hello\n", &opts()).unwrap();
        // At least one <line> for the message (plus 2 lifelines).
        assert!(r.svg.matches("<line").count() >= 3);
        assert!(r.svg.contains(">Hello</text>"));
    }

    #[test]
    fn filled_arrow_uses_marker() {
        let r = render_sequence("sequenceDiagram\n A->>B: hi\n", &opts()).unwrap();
        assert!(r.svg.contains("marker-end=\"url(#seq-arrow)\""));
        assert!(r.svg.contains("<marker id=\"seq-arrow\""));
    }

    #[test]
    fn dashed_message_has_dash_pattern() {
        let r = render_sequence("sequenceDiagram\n A-->>B: hi\n", &opts()).unwrap();
        // Message dash pattern (distinct from lifeline 3 3).
        assert!(r.svg.contains("stroke-dasharray=\"4 3\""));
    }

    #[test]
    fn solid_message_has_no_message_dash() {
        let r = render_sequence("sequenceDiagram\n A->>B: hi\n", &opts()).unwrap();
        assert!(!r.svg.contains("stroke-dasharray=\"4 3\""));
    }

    #[test]
    fn cross_arrow_draws_a_cross() {
        let r = render_sequence("sequenceDiagram\n A-xB: bye\n", &opts()).unwrap();
        // The cross has two strokes joined by an `M` move within one path.
        assert!(r.svg.contains(" M"), "expected a cross path with a move-to: {}", r.svg);
    }

    #[test]
    fn self_message_renders_loop_path() {
        let r = render_sequence("sequenceDiagram\n A->>A: think\n", &opts()).unwrap();
        // The loop is a multi-segment <path>.
        assert!(r.svg.contains("<path d=\"M"));
        assert!(r.svg.contains(">think</text>"));
    }

    #[test]
    fn auto_created_participant_from_message_renders() {
        // Z never declared; should still get a box.
        let r = render_sequence("sequenceDiagram\n participant A\n A->>Z: hi\n", &opts()).unwrap();
        assert_eq!(r.svg.matches("<rect").count(), 2);
        assert!(r.svg.contains(">Z</text>"));
    }

    #[test]
    fn xml_escapes_message_text() {
        let r = render_sequence("sequenceDiagram\n A->>B: a & b < c\n", &opts()).unwrap();
        assert!(r.svg.contains("a &amp; b &lt; c"));
        assert!(!r.svg.contains("a & b"));
    }

    #[test]
    fn empty_input_is_error() {
        // No header at all → a Parse error (not Empty).
        assert!(matches!(render_sequence("", &opts()), Err(MermaidError::Parse(_))));
    }

    #[test]
    fn no_participants_is_empty_error() {
        // Header only, no participants / messages.
        assert!(matches!(render_sequence("sequenceDiagram\n", &opts()), Err(MermaidError::Empty)));
    }

    #[test]
    fn deterministic_output() {
        let src = "sequenceDiagram\n participant A\n participant B as Bob\n A->>B: hi\n B-->>A: yo\n A->>A: think\n";
        let a = render_sequence(src, &opts()).unwrap();
        let b = render_sequence(src, &opts()).unwrap();
        assert_eq!(a.svg, b.svg);
        assert_eq!(a.width_px, b.width_px);
        assert_eq!(a.height_px, b.height_px);
    }

    // --- Advanced: parsing ---

    #[test]
    fn parses_alt_else_block_tree() {
        let d = parse_sequence(
            "sequenceDiagram\n alt is ok\n A->>B: yes\n else not ok\n A->>B: no\n end\n",
        )
        .unwrap();
        assert_eq!(d.items.len(), 1);
        let Item::Block(b) = &d.items[0] else { panic!("expected block") };
        assert_eq!(b.kind, BlockKind::Alt);
        assert_eq!(b.label, "is ok");
        // Two messages, one section divider before the second.
        assert_eq!(b.items.len(), 2);
        assert_eq!(b.sections.len(), 1);
        assert_eq!(b.sections[0], (1, "not ok".to_string()));
    }

    #[test]
    fn parses_nested_loop_in_alt() {
        let d = parse_sequence(
            "sequenceDiagram\n alt x\n loop y\n A->>B: m\n end\n else z\n A->>B: n\n end\n",
        )
        .unwrap();
        let Item::Block(alt) = &d.items[0] else { panic!() };
        assert_eq!(alt.kind, BlockKind::Alt);
        assert!(matches!(alt.items[0], Item::Block(_)));
        // The else divider sits before the second item (index 1).
        assert_eq!(alt.sections, vec![(1, "z".to_string())]);
    }

    #[test]
    fn parses_note_placements() {
        let d = parse_sequence(
            "sequenceDiagram\n participant A\n participant B\n Note left of A: l\n Note right of B: r\n Note over A,B: o\n",
        )
        .unwrap();
        let notes: Vec<&Note> = d.items.iter().filter_map(|i| match i {
            Item::Note(n) => Some(n),
            _ => None,
        }).collect();
        assert_eq!(notes.len(), 3);
        assert_eq!(notes[0].placement, NotePlacement::LeftOf);
        assert_eq!(notes[0].targets, vec!["A".to_string()]);
        assert_eq!(notes[1].placement, NotePlacement::RightOf);
        assert_eq!(notes[2].placement, NotePlacement::Over);
        assert_eq!(notes[2].targets, vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn parses_activation_suffixes() {
        let d = parse_sequence("sequenceDiagram\n A->>+B: go\n B-->>-A: done\n").unwrap();
        let msgs = flat_messages(&d);
        assert_eq!(msgs.len(), 2);
        assert!(msgs[0].activate_to);
        assert!(!msgs[0].deactivate_from);
        assert_eq!(msgs[0].to, "B");
        assert!(msgs[1].deactivate_from);
        assert!(!msgs[1].activate_to);
        assert_eq!(msgs[1].to, "A");
        assert_eq!(msgs[1].from, "B");
    }

    #[test]
    fn parses_explicit_activate_deactivate() {
        let d = parse_sequence(
            "sequenceDiagram\n activate A\n A->>B: hi\n deactivate A\n",
        )
        .unwrap();
        assert!(matches!(d.items[0], Item::Activate(ref s) if s == "A"));
        assert!(matches!(d.items[2], Item::Deactivate(ref s) if s == "A"));
    }

    #[test]
    fn unterminated_block_is_error() {
        assert!(parse_sequence("sequenceDiagram\n loop x\n A->>B: hi\n").is_err());
    }

    #[test]
    fn stray_end_is_error() {
        assert!(parse_sequence("sequenceDiagram\n A->>B: hi\n end\n").is_err());
    }

    // --- Advanced: rendering ---

    #[test]
    fn renders_loop_frame_with_keyword() {
        let r = render_sequence(
            "sequenceDiagram\n loop retry\n A->>B: ping\n end\n",
            &opts(),
        )
        .unwrap();
        assert!(r.svg.starts_with("<svg"));
        assert!(r.svg.trim_end().ends_with("</svg>"));
        // Frame keyword tab label + opening label.
        assert!(r.svg.contains(">loop</text>"), "no loop keyword: {}", r.svg);
        assert!(r.svg.contains(">retry</text>"));
        // The frame and the tab both draw <rect>/<path>; at least a frame rect.
        assert!(r.svg.matches("<rect").count() >= 3);
    }

    #[test]
    fn renders_alt_else_divider() {
        let r = render_sequence(
            "sequenceDiagram\n alt ok\n A->>B: yes\n else fail\n A->>B: no\n end\n",
            &opts(),
        )
        .unwrap();
        assert!(r.svg.contains(">alt</text>"));
        assert!(r.svg.contains(">ok</text>"));
        // The else section label.
        assert!(r.svg.contains(">fail</text>"));
        // A dashed divider line uses the 3 3 pattern (like lifelines): lifelines
        // = 2, plus at least one divider ⇒ >= 3.
        assert!(r.svg.matches("stroke-dasharray=\"3 3\"").count() >= 3);
    }

    #[test]
    fn renders_note_rect_and_text() {
        let r = render_sequence(
            "sequenceDiagram\n participant A\n participant B\n Note over A,B: hello note\n",
            &opts(),
        )
        .unwrap();
        // Pale-yellow note fill.
        assert!(r.svg.contains("fill=\"rgb(255,255,221)\""), "no note rect: {}", r.svg);
        assert!(r.svg.contains(">hello note</text>"));
    }

    #[test]
    fn renders_activation_bar_on_lifeline() {
        let r = render_sequence(
            "sequenceDiagram\n A->>+B: go\n B-->>-A: done\n",
            &opts(),
        )
        .unwrap();
        // The activation bar is a node-fill rect (same fill as participant
        // boxes). Boxes (2) + at least one activation bar ⇒ >= 3 such rects.
        let fill = "fill=\"rgb(236,236,255)\"";
        assert!(r.svg.matches(fill).count() >= 3, "no activation bar rect: {}", r.svg);
    }

    #[test]
    fn plain_diagram_unchanged_by_advanced_code() {
        // A diagram with no blocks/notes/activations should render exactly the
        // same structure as before: only participant rects + lifelines + the
        // message, no frames/notes/bars.
        let r = render_sequence(
            "sequenceDiagram\n participant A\n participant B\n A->>B: hi\n",
            &opts(),
        )
        .unwrap();
        // Exactly two rects (the two participant boxes; no frames/notes/bars).
        assert_eq!(r.svg.matches("<rect").count(), 2);
        // Exactly two dashed 3 3 lifelines, no dividers.
        assert_eq!(r.svg.matches("stroke-dasharray=\"3 3\"").count(), 2);
        // No note fill.
        assert!(!r.svg.contains("rgb(255,255,221)"));
    }

    #[test]
    fn nested_activation_offsets_horizontally() {
        // Two stacked activations on B → two bars, the inner offset.
        let r = render_sequence(
            "sequenceDiagram\n A->>+B: a\n A->>+B: b\n B-->>-A: c\n B-->>-A: d\n",
            &opts(),
        )
        .unwrap();
        let fill = "fill=\"rgb(236,236,255)\"";
        // 2 boxes + 2 activation bars.
        assert!(r.svg.matches(fill).count() >= 4, "expected nested bars: {}", r.svg);
    }

    #[test]
    fn advanced_features_deterministic() {
        let src = "sequenceDiagram\n participant A\n participant B\n loop r\n A->>+B: go\n Note right of B: working\n B-->>-A: ok\n end\n alt x\n A->>B: y\n else z\n A->>B: n\n end\n";
        let a = render_sequence(src, &opts()).unwrap();
        let b = render_sequence(src, &opts()).unwrap();
        assert_eq!(a.svg, b.svg);
        assert_eq!(a.width_px, b.width_px);
        assert_eq!(a.height_px, b.height_px);
    }

    #[test]
    fn message_label_renders_inline_math() {
        // A message whose text contains `$…$` now renders the embedded math
        // group rather than a plain `<text>` with the raw dollar signs.
        let r = render_sequence("sequenceDiagram\n A->>B: speed $x^2$\n", &opts()).unwrap();
        assert!(r.svg.contains("<g transform"), "expected math group: {}", r.svg);
        assert!(r.svg.contains("<path"), "expected math path: {}", r.svg);
    }

    #[test]
    fn note_label_renders_bold_markdown() {
        // A `**bold**` note renders a bold tspan/text instead of literal `**`.
        let r = render_sequence(
            "sequenceDiagram\n participant A\n Note over A: **warn**\n",
            &opts(),
        )
        .unwrap();
        assert!(r.svg.contains("font-weight=\"bold\""), "expected bold run: {}", r.svg);
        assert!(!r.svg.contains("**warn**"), "raw markdown leaked: {}", r.svg);
    }

    #[test]
    fn note_xml_escapes() {
        let r = render_sequence(
            "sequenceDiagram\n participant A\n Note over A: a & b < c\n",
            &opts(),
        )
        .unwrap();
        assert!(r.svg.contains("a &amp; b &lt; c"));
    }

    // --- autonumber ---

    #[test]
    fn autonumber_assigns_sequential_numbers_to_messages_only() {
        let d = parse_sequence(
            "sequenceDiagram\n autonumber\n A->>B: a\n Note over A: skip\n A->>B: b\n A->>B: c\n",
        )
        .unwrap();
        let msgs = flat_messages(&d);
        assert_eq!(msgs.len(), 3);
        // Notes don't consume a number; messages are 1,2,3.
        assert_eq!(msgs[0].number, Some(1));
        assert_eq!(msgs[1].number, Some(2));
        assert_eq!(msgs[2].number, Some(3));
    }

    #[test]
    fn no_autonumber_leaves_messages_unnumbered() {
        let d = parse_sequence("sequenceDiagram\n A->>B: a\n A->>B: b\n").unwrap();
        let msgs = flat_messages(&d);
        assert!(msgs.iter().all(|m| m.number.is_none()));
    }

    #[test]
    fn autonumber_start_and_step() {
        let d = parse_sequence(
            "sequenceDiagram\n autonumber 10 5\n A->>B: a\n A->>B: b\n",
        )
        .unwrap();
        let msgs = flat_messages(&d);
        assert_eq!(msgs[0].number, Some(10));
        assert_eq!(msgs[1].number, Some(15));
    }

    #[test]
    fn autonumber_renders_badge_with_number() {
        let r = render_sequence(
            "sequenceDiagram\n autonumber\n A->>B: hi\n B->>A: yo\n",
            &opts(),
        )
        .unwrap();
        // Two badge circles, one per message.
        assert_eq!(r.svg.matches("<circle").count(), 2);
        assert!(r.svg.contains(">1</text>"), "no number 1 badge: {}", r.svg);
        assert!(r.svg.contains(">2</text>"));
    }

    #[test]
    fn no_autonumber_renders_no_badge() {
        let r = render_sequence("sequenceDiagram\n A->>B: hi\n", &opts()).unwrap();
        assert!(!r.svg.contains("<circle"));
    }

    // --- rect background blocks ---

    #[test]
    fn parses_rect_block_with_rgb_color() {
        let d = parse_sequence(
            "sequenceDiagram\n rect rgb(230,230,250)\n A->>B: x\n end\n",
        )
        .unwrap();
        assert_eq!(d.items.len(), 1);
        let Item::Rect(rb) = &d.items[0] else { panic!("expected rect block: {:?}", d.items) };
        assert_eq!(rb.color, Rgba { r: 230, g: 230, b: 250, a: 80 });
        assert_eq!(rb.items.len(), 1);
        assert!(matches!(rb.items[0], Item::Message(_)));
    }

    #[test]
    fn parses_rect_rgba_and_hex() {
        let d = parse_sequence(
            "sequenceDiagram\n rect rgba(10,20,30,0.5)\n A->>B: x\n end\n rect #abc\n A->>B: y\n end\n",
        )
        .unwrap();
        let Item::Rect(a) = &d.items[0] else { panic!() };
        assert_eq!(a.color, Rgba { r: 10, g: 20, b: 30, a: 128 });
        let Item::Rect(b) = &d.items[1] else { panic!() };
        assert_eq!(b.color, Rgba { r: 0xaa, g: 0xbb, b: 0xcc, a: 80 });
    }

    #[test]
    fn renders_rect_translucent_background() {
        let r = render_sequence(
            "sequenceDiagram\n rect rgb(230,230,250)\n A->>B: x\n end\n",
            &opts(),
        )
        .unwrap();
        // A translucent fill behind the message.
        assert!(
            r.svg.contains("fill=\"rgb(230,230,250)\" fill-opacity="),
            "no rect background: {}",
            r.svg
        );
        // The contained message still renders.
        assert!(r.svg.contains(">x</text>"));
    }

    #[test]
    fn rect_nests_with_block_frames() {
        let r = render_sequence(
            "sequenceDiagram\n loop r\n rect rgb(200,255,200)\n A->>B: x\n end\n end\n",
            &opts(),
        )
        .unwrap();
        // Both the loop frame keyword and the rect background show.
        assert!(r.svg.contains(">loop</text>"));
        assert!(r.svg.contains("fill=\"rgb(200,255,200)\" fill-opacity="));
    }

    #[test]
    fn bad_rect_color_is_parse_error() {
        assert!(parse_sequence(
            "sequenceDiagram\n rect notacolor\n A->>B: x\n end\n"
        )
        .is_err());
    }

    #[test]
    fn plain_diagram_unchanged_by_autonumber_and_rect_code() {
        // No autonumber, no rect ⇒ no badges, no translucent backgrounds.
        let r = render_sequence(
            "sequenceDiagram\n participant A\n participant B\n A->>B: hi\n",
            &opts(),
        )
        .unwrap();
        assert!(!r.svg.contains("<circle"));
        assert!(!r.svg.contains("fill-opacity"));
    }

    #[test]
    fn autonumber_and_rect_deterministic() {
        let src = "sequenceDiagram\n autonumber\n rect rgb(230,230,250)\n A->>B: a\n B-->>A: b\n end\n A->>B: c\n";
        let a = render_sequence(src, &opts()).unwrap();
        let b = render_sequence(src, &opts()).unwrap();
        assert_eq!(a.svg, b.svg);
        assert_eq!(a.width_px, b.width_px);
        assert_eq!(a.height_px, b.height_px);
    }
}
