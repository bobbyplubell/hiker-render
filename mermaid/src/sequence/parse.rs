//! Sequence-diagram parser: `sequenceDiagram` source → [`super::model::SequenceDiagram`].
//! Handles participant/actor declarations and `as` aliases, messages with the
//! various arrow tokens and `+`/`-` activation suffixes, notes, `activate`/
//! `deactivate`, block frames (`loop`/`opt`/`alt`/`par`/`break`/`critical` with
//! `else`/`and`/`option` sections), `rect <color>` background blocks, and
//! `autonumber`.

use super::model;

/// While parsing a block we are inside, this records the kind, opening label,
/// items collected so far, and section markers.
struct BlockFrame {
    /// `None` for a `rect <color>` background block (it has no keyword/sections,
    /// only a `color`); `Some(kind)` for a labeled block frame.
    kind: Option<model::BlockKind>,
    label: String,
    /// Background color for `rect` frames; ignored for labeled blocks.
    color: Option<model::Rgba>,
    items: Vec<model::Item>,
    sections: Vec<(usize, String)>,
}

/// Decode a `loop`/`opt`/`alt`/`par`/`break`/`critical` opener into its kind.
fn block_opener(lower: &str) -> Option<model::BlockKind> {
    let kw = lower.split_whitespace().next()?;
    match kw {
        "loop" => Some(model::BlockKind::Loop),
        "opt" => Some(model::BlockKind::Opt),
        "alt" => Some(model::BlockKind::Alt),
        "par" => Some(model::BlockKind::Par),
        "break" => Some(model::BlockKind::Break),
        "critical" => Some(model::BlockKind::Critical),
        _ => None,
    }
}

/// Parse `sequenceDiagram` source into a [`SequenceDiagram`].
///
/// Participants are collected in declaration order; any id referenced by a
/// message/note but not declared is appended in first-appearance order. Block
/// frames nest; `else`/`and`/`option` add section markers to the open block.
/// Returns the raw parse error string on malformed input.
pub(super) fn parse(src: &str) -> Result<model::SequenceDiagram, String> {
    let mut participants: Vec<model::Participant> = Vec::new();
    let mut seen_header = false;

    // Track explicitly-declared ids so auto-created ones don't duplicate.
    let mut have: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Items at the top level, plus the stack of currently-open block frames.
    let mut top: Vec<model::Item> = Vec::new();
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
                    model::Item::Rect(model::RectBlock {
                        color: frame.color.unwrap_or(model::Rgba { r: 200, g: 200, b: 255, a: 80 }),
                        items: frame.items,
                    })
                }
                Some(kind) => model::Item::Block(model::Block {
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
            push_item(&mut stack, &mut top, model::Item::Note(note));
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
            push_item(&mut stack, &mut top, model::Item::Activate(id));
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
            push_item(&mut stack, &mut top, model::Item::Deactivate(id));
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
        push_item(&mut stack, &mut top, model::Item::Message(msg));
    }

    if !seen_header {
        return Err("missing 'sequenceDiagram' header".to_string());
    }
    if !stack.is_empty() {
        return Err("unterminated block (missing 'end')".to_string());
    }

    Ok(model::SequenceDiagram { participants, items: top })
}

/// Push a leaf item into the innermost open block, or the top level.
fn push_item(stack: &mut [BlockFrame], top: &mut Vec<model::Item>, item: model::Item) {
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
fn parse_note(line: &str) -> Result<model::Note, String> {
    // `line` starts with `Note ` (case-insensitively). Take the rest.
    let rest = &line[4..]; // after "model::Note"
    let rest = rest.trim_start();
    let lower = rest.to_ascii_lowercase();

    let (placement, after) = if let Some(a) = lower.strip_prefix("left of") {
        (model::NotePlacement::LeftOf, &rest[rest.len() - a.len()..])
    } else if let Some(a) = lower.strip_prefix("right of") {
        (model::NotePlacement::RightOf, &rest[rest.len() - a.len()..])
    } else if let Some(a) = lower.strip_prefix("over") {
        (model::NotePlacement::Over, &rest[rest.len() - a.len()..])
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
    if placement != model::NotePlacement::Over && targets.len() != 1 {
        return Err(format!("'{}' note takes exactly one participant: {line:?}",
            if placement == model::NotePlacement::LeftOf { "left of" } else { "right of" }));
    }
    if targets.len() > 2 {
        return Err(format!("note over takes at most two participants: {line:?}"));
    }

    Ok(model::Note { placement, targets, text })
}

/// Add a participant (id + optional explicit label) once, recording it in the
/// `have` set. A later `as` alias for an already-seen id updates its label.
fn declare(
    participants: &mut Vec<model::Participant>,
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
    participants.push(model::Participant {
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
fn parse_message(line: &str) -> Result<model::Message, String> {
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

    Ok(model::Message { from, to, text, style, dashed, activate_to, deactivate_from, number: None })
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
fn decode_arrow(arrow: &str) -> Option<(model::ArrowStyle, bool)> {
    let style = match arrow {
        "->>" | "-->>" => model::ArrowStyle::Filled,
        "->" | "-->" | "--" => model::ArrowStyle::Open,
        "-)" | "--)" => model::ArrowStyle::Async,
        "-x" | "--x" => model::ArrowStyle::Cross,
        _ => return None,
    };
    let dashed = arrow.starts_with("--");
    Some((style, dashed))
}

/// Parse a `rect` background color: `rgb(r,g,b)`, `rgba(r,g,b,a)` (a is 0..=1 or
/// 0..=255), or a `#rgb`/`#rrggbb` hex. Whitespace is tolerated. `rgb`/hex with
/// no alpha default to a translucent highlight (alpha ~80/255) so the fill reads
/// as a highlight rather than an opaque block. Returns `None` if unrecognized.
fn parse_color(s: &str) -> Option<model::Rgba> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex(hex).map(|(r, g, b)| model::Rgba { r, g, b, a: 80 });
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
    Some(model::Rgba { r, g, b, a })
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
