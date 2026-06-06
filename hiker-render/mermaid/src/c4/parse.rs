//! C4-diagram parser: `C4Context`/`C4Container`/… source →
//! [`super::model::C4Diagram`]. Recognizes element keywords (person/system/
//! container/component and their `_Ext`/`Db`/`Queue` variants), `Rel`/`BiRel`
//! relationships, and `*_Boundary` nesting blocks with `( … )` call args.

use std::collections::HashMap;

use super::model;

pub(super) fn parse(src: &str) -> Result<model::C4Diagram, String> {
    let mut diag = model::C4Diagram::default();
    let mut seen_ids: HashMap<String, ()> = HashMap::new();
    let mut pending_header = true;
    // Stack of currently-open boundary indices (into `diag.boundaries`). The top
    // is the boundary that newly-declared elements / boundaries belong to.
    let mut boundary_stack: Vec<usize> = Vec::new();

    for raw in src.lines() {
        // Strip `%%` comments and surrounding whitespace.
        let line = raw.split("%%").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }

        if pending_header {
            let kw = line.split_whitespace().next().unwrap_or("");
            if !is_header(kw) {
                return Err(format!("expected a C4 header, got {kw:?}"));
            }
            pending_header = false;
            continue;
        }

        // Boundary close brace(s): pop the innermost open boundary. A line may be
        // a bare `}` (possibly several) — pop one per `}`.
        if line.chars().all(|c| c == '}') {
            for _ in 0..line.chars().count() {
                boundary_stack.pop();
            }
            continue;
        }

        // Boundary opener: `<kind>_Boundary(id, "label") {`. We strip a trailing
        // `{` and remember that an opener introduced a block.
        let opens_block = line.ends_with('{');
        let line_no_brace = line.strip_suffix('{').map(str::trim_end).unwrap_or(line);

        if let Some((kw, args_str)) = split_call(line_no_brace) {
            // Boundary opener: create a boundary node, link it to its parent (the
            // current top of the stack), and push it so inner elements/boundaries
            // attach to it.
            if let Some(kind) = boundary_keyword(kw) {
                let args = split_args(args_str);
                if let Some(b) = build_boundary(kind, boundary_stack.last().copied(), &args) {
                    let idx = diag.boundaries.len();
                    diag.boundaries.push(b);
                    if opens_block {
                        boundary_stack.push(idx);
                    }
                }
                continue;
            }

            if let Some((kind, external)) = element_keyword(kw) {
                let args = split_args(args_str);
                if let Some(elem) = build_element(kind, external, &args) {
                    if seen_ids.insert(elem.id.clone(), ()).is_none() {
                        // Register membership in the enclosing boundary, if any.
                        if let Some(&bi) = boundary_stack.last() {
                            diag.boundaries[bi].member_elems.push(elem.id.clone());
                        }
                        diag.elements.push(elem);
                    }
                }
                continue;
            }

            if let Some(bidir) = relationship_keyword(kw) {
                let args = split_args(args_str);
                if let Some(rel) = build_relationship(&args) {
                    if bidir {
                        diag.relationships.push(model::Relationship {
                            from: rel.to.clone(),
                            to: rel.from.clone(),
                            label: rel.label.clone(),
                            tech: rel.tech.clone(),
                        });
                    }
                    diag.relationships.push(rel);
                }
                continue;
            }

            // Unknown call (UpdateElementStyle / UpdateRelStyle /
            // UpdateLayoutConfig / sprites / etc.): ignore.
            continue;
        }

        // Anything else (directives, stray tokens): ignore.
    }

    if pending_header {
        return Err("empty input / no C4 diagram header".to_string());
    }
    Ok(diag)
}

fn is_header(kw: &str) -> bool {
    matches!(
        kw,
        "C4Context" | "C4Container" | "C4Component" | "C4Dynamic" | "C4Deployment"
    )
}

/// Map an element keyword to its `(kind, external)`, if it is one. Database /
/// queue variants collapse onto their base kind (we don't draw distinct
/// cylinder/queue shapes in v1).
fn element_keyword(kw: &str) -> Option<(model::ElemKind, bool)> {
    Some(match kw {
        "Person" => (model::ElemKind::Person, false),
        "Person_Ext" => (model::ElemKind::Person, true),

        "System" | "SystemDb" | "SystemQueue" => (model::ElemKind::System, false),
        "System_Ext" | "SystemDb_Ext" | "SystemQueue_Ext" => (model::ElemKind::System, true),

        "Container" | "ContainerDb" | "ContainerQueue" => (model::ElemKind::Container, false),
        "Container_Ext" | "ContainerDb_Ext" | "ContainerQueue_Ext" => (model::ElemKind::Container, true),

        "Component" | "ComponentDb" | "ComponentQueue" => (model::ElemKind::Component, false),
        "Component_Ext" | "ComponentDb_Ext" | "ComponentQueue_Ext" => (model::ElemKind::Component, true),

        // Deployment nodes: treated as plain container-like boxes.
        "Deployment_Node" | "Node" | "Node_L" | "Node_R" => (model::ElemKind::Container, false),

        _ => return None,
    })
}

/// Whether a keyword carries a technology arg (between label and description):
/// containers, components, and deployment nodes do; persons and systems don't.
fn keyword_has_tech(kind: model::ElemKind) -> bool {
    matches!(kind, model::ElemKind::Container | model::ElemKind::Component)
}

/// Map a relationship keyword to `Some(is_bidirectional)`, if it is one.
fn relationship_keyword(kw: &str) -> Option<bool> {
    Some(match kw {
        "Rel" => false,
        "BiRel" => true,
        "Rel_U" | "Rel_Up" => false,
        "Rel_D" | "Rel_Down" => false,
        "Rel_L" | "Rel_Left" => false,
        "Rel_R" | "Rel_Right" => false,
        "Rel_Back" | "Rel_B" => false,
        _ => return None,
    })
}

/// Split the keyword off a statement like `Person(a, "b")`. Returns
/// `(keyword, args_str)` where `args_str` is the text inside the outer parens.
/// `None` if there's no `(...)`.
fn split_call(line: &str) -> Option<(&str, &str)> {
    let open = line.find('(')?;
    // Match the final close paren so trailing `{` (boundary openers) is excluded
    // by the caller before this is reached.
    let close = line.rfind(')')?;
    if close <= open {
        return None;
    }
    let kw = line[..open].trim();
    let args = &line[open + 1..close];
    if kw.is_empty() {
        return None;
    }
    Some((kw, args))
}

/// Split a comma-separated argument list, honoring double quotes (commas inside
/// quotes are kept). Surrounding quotes on each arg are stripped; bare args are
/// trimmed. Empty trailing/positional args are preserved as empty strings.
fn split_args(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = args.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                // Keep the quote marker out of the value; quotes are stripped.
            }
            ',' if !in_quotes => {
                out.push(cur.trim().to_string());
                cur = String::new();
            }
            _ => cur.push(c),
        }
        let _ = chars.peek();
    }
    out.push(cur.trim().to_string());
    out
}

/// Word-wrap `text` to at most `max_chars` per line (greedy by words). Returns
/// the lines joined with `\n`. Long single words are left intact.
fn wrap(text: &str, max_chars: usize) -> String {
    if text.is_empty() {
        return String::new();
    }
    let max = max_chars.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if cur.is_empty() {
            cur = word.to_string();
        } else if cur.chars().count() + 1 + word.chars().count() <= max {
            cur.push(' ');
            cur.push_str(word);
        } else {
            lines.push(std::mem::take(&mut cur));
            cur = word.to_string();
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines.join("\n")
}

/// Description wrap width, in characters.
const WRAP_CHARS: usize = 28;

/// Map a boundary keyword to its [`BoundaryKind`], if it is one.
fn boundary_keyword(kw: &str) -> Option<model::BoundaryKind> {
    Some(match kw {
        "System_Boundary" => model::BoundaryKind::System,
        "Enterprise_Boundary" => model::BoundaryKind::Enterprise,
        "Container_Boundary" => model::BoundaryKind::Container,
        "model::Boundary" => model::BoundaryKind::Generic,
        _ => return None,
    })
}

/// Build a [`Boundary`] from a keyword's kind, its parent (the enclosing open
/// boundary) and its split args: `id, "label"`. `None` if there is no id.
fn build_boundary(kind: model::BoundaryKind, parent: Option<usize>, args: &[String]) -> Option<model::Boundary> {
    let id = args.first().map(|s| s.trim().to_string())?;
    if id.is_empty() {
        return None;
    }
    let label = args.get(1).cloned().unwrap_or_default();
    let label = if label.is_empty() { id.clone() } else { label };
    Some(model::Boundary {
        id,
        label,
        kind,
        parent,
        member_elems: Vec::new(),
    })
}

/// Build an [`Element`] from a keyword's kind and its split args. The first arg
/// is the id; remaining args are label, (tech,) description. `None` if there is
/// no id.
fn build_element(kind: model::ElemKind, external: bool, args: &[String]) -> Option<model::Element> {
    let id = args.first().map(|s| s.trim().to_string())?;
    if id.is_empty() {
        return None;
    }
    let label = args.get(1).cloned().unwrap_or_default();
    let (tech, descr) = if keyword_has_tech(kind) {
        let tech = args.get(2).cloned().unwrap_or_default();
        let descr = args.get(3).cloned().unwrap_or_default();
        (tech, descr)
    } else {
        let descr = args.get(2).cloned().unwrap_or_default();
        (String::new(), descr)
    };
    // A missing label falls back to the id.
    let label = if label.is_empty() { id.clone() } else { label };
    Some(model::Element {
        id,
        label,
        tech,
        descr: wrap(&descr, WRAP_CHARS),
        kind,
        external,
    })
}

/// Build a [`Relationship`] from split args: `from, to, label, tech?`. `None` if
/// from/to are missing.
fn build_relationship(args: &[String]) -> Option<model::Relationship> {
    let from = args.first().map(|s| s.trim().to_string())?;
    let to = args.get(1).map(|s| s.trim().to_string())?;
    if from.is_empty() || to.is_empty() {
        return None;
    }
    let label = args.get(2).cloned().unwrap_or_default();
    let tech = args.get(3).cloned().unwrap_or_default();
    Some(model::Relationship {
        from,
        to,
        label,
        tech,
    })
}
