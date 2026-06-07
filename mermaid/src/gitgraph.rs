//! `gitGraph` diagram (self-contained: parse + self-layout + draw, no dagre).
//!
//! Mermaid gitGraph syntax (the subset we support):
//! ```text
//! gitGraph
//!     commit
//!     commit id: "abc" tag: "v1" type: HIGHLIGHT
//!     branch develop
//!     checkout develop
//!     commit
//!     checkout main
//!     merge develop
//! ```
//! The header is `gitGraph`, optionally with a trailing direction and/or `:`
//! (`gitGraph`, `gitGraph:`, `gitGraph TB:`, `gitGraph LR:`, `gitGraph BT:`).
//! Default direction is `LR`.
//!
//! ## Layout model (self-layout — no graph engine)
//! Each branch gets a **lane** (index in order of first appearance) mapped to a
//! perpendicular offset and a distinct palette color. Commits advance along the
//! main axis by sequence order. For `LR` (default): x = `seq * commit_spacing`,
//! lane y = `lane * lane_spacing`. For `TB`/`BT`: axes swap (commits down, lanes
//! across); `BT` also flips the commit axis.
//!
//! We maintain `current_branch`, each branch's `head` commit, and an ordered
//! list of commits, each carrying (seq for the main axis, branch/lane, id label,
//! tag, type, parents). A normal `commit` advances seq, sits on the current
//! branch's lane, parent = the current branch's previous head. `branch X` records
//! a new lane whose head = the current head (no commit emitted) and switches to
//! it. `merge X` emits a commit on the current lane whose parents are
//! `{current head, branch X's head}` and curves to branch X.
//!
//! `cherry-pick id: "X"` (optionally `tag: "Y"`) emits a commit on the current
//! lane flagged as a cherry-pick carrying source id `X`; it is drawn with a
//! distinct marker (an outlined circle with a cross through it) and a
//! `cherry-pick:X` (or the given tag) label.
//!
//! Skipped / noted: `REVERSE`/`HIGHLIGHT` type styling beyond a simple
//! shape/outline hint, custom themes, commit `order:` semantics beyond lane
//! assignment by first appearance.
//!
//! See `references/mermaid/packages/mermaid/src/diagrams/git/` for the upstream.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::svgutil::{escape, rgb, text_size};
use crate::{MermaidError, MermaidOptions, MermaidRender};

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// Reading/layout direction. `LR` is mermaid's gitGraph default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dir {
    /// Left-to-right: commits advance +x, lanes stack in y.
    Lr,
    /// Top-to-bottom: commits advance +y, lanes spread in x.
    Tb,
    /// Bottom-to-top: like `Tb` but the commit axis is flipped.
    Bt,
}

/// A commit's visual/semantic type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommitType {
    Normal,
    Reverse,
    Highlight,
}

/// One commit in the timeline.
#[derive(Clone, Debug, PartialEq)]
struct Commit {
    /// Sequence index along the main axis (monotonically increasing).
    seq: usize,
    /// Lane index (the branch this commit lives on).
    lane: usize,
    /// Display id label (auto-generated if not given via `id:`).
    id: String,
    /// Optional `tag:` flag label.
    tag: Option<String>,
    /// Commit type (styling hint).
    ctype: CommitType,
    /// Indices (into the commits vec) of parent commits.
    parents: Vec<usize>,
    /// True if this is a merge commit (parents from two branches).
    is_merge: bool,
    /// If set, this is a cherry-pick commit carrying the cherry-picked source id.
    cherry_pick: Option<String>,
}

/// A parsed gitGraph: direction, lane→branch-name order, and the commit list.
#[derive(Clone, Debug, PartialEq)]
struct GitGraph {
    dir: Dir,
    /// Branch names in lane order (index = lane).
    branches: Vec<String>,
    commits: Vec<Commit>,
    /// Notes about constructs we parsed but only partially rendered.
    notes: Vec<String>,
}

// ---------------------------------------------------------------------------
// Parse
// ---------------------------------------------------------------------------

/// Mutable parse state: which branch is current, and each branch's head.
struct ParseState {
    /// Lane index per branch name.
    lane_of: HashMap<String, usize>,
    /// Head commit index per branch name (None = no commit yet on it).
    head_of: HashMap<String, Option<usize>>,
    /// Lane order (index = lane → name).
    branches: Vec<String>,
    current: String,
    commits: Vec<Commit>,
    seq: usize,
    auto_id: usize,
    notes: Vec<String>,
}

impl ParseState {
    fn new() -> Self {
        let mut s = ParseState {
            lane_of: HashMap::new(),
            head_of: HashMap::new(),
            branches: Vec::new(),
            current: "main".to_string(),
            commits: Vec::new(),
            seq: 0,
            auto_id: 0,
            notes: Vec::new(),
        };
        // The initial branch `main` always occupies lane 0.
        s.lane_of.insert("main".to_string(), 0);
        s.head_of.insert("main".to_string(), None);
        s.branches.push("main".to_string());
        s
    }

    /// Lane for `name`, creating it (next free lane) if new.
    fn ensure_branch(&mut self, name: &str) -> usize {
        if let Some(&l) = self.lane_of.get(name) {
            return l;
        }
        let lane = self.branches.len();
        self.lane_of.insert(name.to_string(), lane);
        self.head_of.insert(name.to_string(), None);
        self.branches.push(name.to_string());
        lane
    }
}

/// Parse the direction/colon tail of the `gitGraph` header. Returns the direction
/// (default `LR`) or an error for an unrecognised token.
fn parse_header(line: &str) -> Result<Dir, String> {
    // The keyword is `gitGraph`, optionally followed by a direction and/or `:`.
    let rest = line
        .strip_prefix("gitGraph")
        .ok_or_else(|| format!("expected 'gitGraph' header, got: {line:?}"))?;
    // Strip an optional trailing colon and whitespace; e.g. `gitGraph LR:`.
    let mut tok = rest.trim();
    tok = tok.trim_end_matches(':').trim();
    if tok.is_empty() {
        return Ok(Dir::Lr);
    }
    match tok {
        "LR" => Ok(Dir::Lr),
        "TB" => Ok(Dir::Tb),
        "BT" => Ok(Dir::Bt),
        other => Err(format!("unknown gitGraph direction: {other:?}")),
    }
}

/// Parse mermaid gitGraph source into a [`GitGraph`].
fn parse_gitgraph(src: &str) -> Result<GitGraph, String> {
    let mut dir: Option<Dir> = None;
    let mut st = ParseState::new();

    for raw in src.lines() {
        // Strip `%%` comments and surrounding whitespace.
        let line = raw.split("%%").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }

        if dir.is_none() {
            dir = Some(parse_header(line)?);
            continue;
        }

        // Dispatch on the first token (the command keyword).
        let (cmd, args) = split_first(line);
        match cmd {
            "commit" => parse_commit(args, &mut st),
            "merge" => parse_merge(args, &mut st)?,
            "branch" => parse_branch(args, &mut st)?,
            "checkout" | "switch" => parse_checkout(args, &mut st)?,
            "cherry-pick" => {
                st.notes.push(format!("cherry-pick: {args}"));
                parse_cherry_pick(args, &mut st);
            }
            other => return Err(format!("unknown gitGraph command: {other:?}")),
        }
    }

    let dir = dir.ok_or_else(|| "empty input / no 'gitGraph' header".to_string())?;
    Ok(GitGraph {
        dir,
        branches: st.branches,
        commits: st.commits,
        notes: st.notes,
    })
}

/// Split a line into (first token, remaining-after-token-trimmed).
fn split_first(line: &str) -> (&str, &str) {
    match line.find(char::is_whitespace) {
        Some(i) => (&line[..i], line[i..].trim_start()),
        None => (line, ""),
    }
}

/// Append a normal commit on the current branch.
fn parse_commit(args: &str, st: &mut ParseState) {
    let attrs = parse_attrs(args);
    let lane = *st.lane_of.get(&st.current).expect("current branch has a lane");
    let parent = st.head_of.get(&st.current).copied().flatten();
    let parents = parent.into_iter().collect::<Vec<_>>();

    let id = attrs.id.unwrap_or_else(|| {
        let n = st.auto_id;
        st.auto_id += 1;
        format!("c{n}")
    });
    let idx = st.commits.len();
    st.commits.push(Commit {
        seq: st.seq,
        lane,
        id,
        tag: attrs.tag,
        ctype: attrs.ctype.unwrap_or(CommitType::Normal),
        parents,
        is_merge: false,
        cherry_pick: None,
    });
    st.seq += 1;
    st.head_of.insert(st.current.clone(), Some(idx));
}

/// Append a cherry-pick commit on the current branch. Syntax:
/// `cherry-pick id: "X"` (the picked source id, required by mermaid) with an
/// optional `tag: "Y"`. The commit sits on the current lane at the next seq with
/// its parent edge as a normal commit, but is flagged as a cherry-pick carrying
/// the source id so the renderer can draw a distinct marker + label.
fn parse_cherry_pick(args: &str, st: &mut ParseState) {
    let attrs = parse_attrs(args);
    let lane = *st.lane_of.get(&st.current).expect("current branch has a lane");
    let parent = st.head_of.get(&st.current).copied().flatten();
    let parents = parent.into_iter().collect::<Vec<_>>();

    // The picked source id (from `id:`). Fall back to an auto id if missing.
    let picked = attrs.id.clone().unwrap_or_default();
    let id = attrs.id.unwrap_or_else(|| {
        let n = st.auto_id;
        st.auto_id += 1;
        format!("cp{n}")
    });
    let idx = st.commits.len();
    st.commits.push(Commit {
        seq: st.seq,
        lane,
        id,
        tag: attrs.tag,
        ctype: CommitType::Normal,
        parents,
        is_merge: false,
        cherry_pick: Some(picked),
    });
    st.seq += 1;
    st.head_of.insert(st.current.clone(), Some(idx));
}

/// Create (and switch to) a new branch off the current head.
fn parse_branch(args: &str, st: &mut ParseState) -> Result<(), String> {
    // `branch <name>` optionally `order: N` (we ignore order's numeric value;
    // lanes are assigned by first appearance).
    let (name, _rest) = split_first(args);
    let name = unquote(name);
    if name.is_empty() {
        return Err("branch requires a name".to_string());
    }
    if st.lane_of.contains_key(&name) {
        return Err(format!("branch already exists: {name:?}"));
    }
    st.ensure_branch(&name);
    // The new branch's head starts at the current branch's head.
    let cur_head = st.head_of.get(&st.current).copied().flatten();
    st.head_of.insert(name.clone(), cur_head);
    // Mermaid switches to the new branch on `branch`.
    st.current = name;
    Ok(())
}

/// Switch the current branch (`checkout`/`switch`).
fn parse_checkout(args: &str, st: &mut ParseState) -> Result<(), String> {
    let (name, _rest) = split_first(args);
    let name = unquote(name);
    if name.is_empty() {
        return Err("checkout requires a branch name".to_string());
    }
    if !st.lane_of.contains_key(&name) {
        return Err(format!("checkout of unknown branch: {name:?}"));
    }
    st.current = name;
    Ok(())
}

/// Merge `<name>` into the current branch: a merge commit on the current lane
/// with parents {current head, merged branch head}.
fn parse_merge(args: &str, st: &mut ParseState) -> Result<(), String> {
    let (name, rest) = split_first(args);
    let name = unquote(name);
    if name.is_empty() {
        return Err("merge requires a branch name".to_string());
    }
    let other_lane = *st
        .lane_of
        .get(&name)
        .ok_or_else(|| format!("merge of unknown branch: {name:?}"))?;
    let _ = other_lane;
    let other_head = st.head_of.get(&name).copied().flatten();
    let cur_head = st.head_of.get(&st.current).copied().flatten();

    let attrs = parse_attrs(rest);
    let mut parents: Vec<usize> = Vec::new();
    if let Some(h) = cur_head {
        parents.push(h);
    }
    if let Some(h) = other_head {
        parents.push(h);
    }

    let lane = *st.lane_of.get(&st.current).expect("current branch has a lane");
    let id = attrs.id.unwrap_or_else(|| {
        let n = st.auto_id;
        st.auto_id += 1;
        format!("m{n}")
    });
    let idx = st.commits.len();
    st.commits.push(Commit {
        seq: st.seq,
        lane,
        id,
        tag: attrs.tag,
        ctype: CommitType::Normal,
        parents,
        is_merge: true,
        cherry_pick: None,
    });
    st.seq += 1;
    st.head_of.insert(st.current.clone(), Some(idx));
    Ok(())
}

/// Parsed key:value attributes from a `commit`/`merge` argument string.
#[derive(Default)]
struct Attrs {
    id: Option<String>,
    tag: Option<String>,
    ctype: Option<CommitType>,
}

/// Parse `id: "x" tag: "y" type: NORMAL|REVERSE|HIGHLIGHT` in any order. Values
/// may be quoted (`"..."`) or bare. Unknown keys are ignored.
fn parse_attrs(args: &str) -> Attrs {
    let mut attrs = Attrs::default();
    let toks = tokenize(args);
    let mut i = 0;
    while i < toks.len() {
        let key = toks[i].trim_end_matches(':');
        // A key must be followed by `:` (possibly attached) and a value.
        let has_colon = toks[i].ends_with(':');
        if has_colon && i + 1 < toks.len() {
            let val = unquote(&toks[i + 1]);
            match key {
                "id" => attrs.id = Some(val),
                "tag" => attrs.tag = Some(val),
                "type" => {
                    attrs.ctype = Some(match val.as_str() {
                        "REVERSE" => CommitType::Reverse,
                        "HIGHLIGHT" => CommitType::Highlight,
                        _ => CommitType::Normal,
                    })
                }
                _ => {}
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    attrs
}

/// Tokenize a string into whitespace-separated tokens, keeping quoted spans
/// (`"..."`) as a single token (with the quotes preserved for [`unquote`]).
fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for c in s.chars() {
        match c {
            '"' => {
                in_quote = !in_quote;
                cur.push(c);
            }
            c if c.is_whitespace() && !in_quote => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Strip surrounding double quotes from a token, if present.
fn unquote(s: &str) -> String {
    let t = s.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------

/// Per-lane branch colors (mermaid's git theme uses a similar rotation).
const PALETTE: [[u8; 3]; 8] = [
    [0x33, 0x66, 0xCC], // blue
    [0x00, 0x99, 0x66], // green
    [0xCC, 0x66, 0x00], // orange
    [0x99, 0x33, 0xCC], // purple
    [0xCC, 0x33, 0x33], // red
    [0x00, 0x99, 0x99], // teal
    [0xCC, 0x99, 0x00], // gold
    [0x66, 0x66, 0x66], // gray
];

/// The palette color for lane `i` (cycling).
fn lane_color(i: usize) -> [u8; 3] {
    PALETTE[i % PALETTE.len()]
}

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------

/// Spacing between consecutive commit sequence positions, px.
const COMMIT_SPACING: f32 = 50.0;
/// Spacing between branch lanes, px.
const LANE_SPACING: f32 = 50.0;
/// Commit circle radius, px.
const COMMIT_R: f32 = 8.0;
/// Margin around the whole drawing, px.
const MARGIN: f32 = 40.0;
/// Extra left/top room for branch-name labels, px.
const LABEL_PAD: f32 = 70.0;
/// Lane-line stroke width, px.
const LANE_W: f32 = 4.0;

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Render mermaid gitGraph source to an SVG document.
pub fn render_gitgraph(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let g = parse_gitgraph(src).map_err(MermaidError::Parse)?;
    if g.commits.is_empty() {
        return Err(MermaidError::Empty);
    }

    let fs = opts.font_size_px;
    let max_seq = g.commits.iter().map(|c| c.seq).max().unwrap_or(0);
    let num_lanes = g.branches.len().max(1);

    // Position helper: map (seq, lane) → (x, y) per direction. We reserve
    // LABEL_PAD on the leading edge for branch-name labels, plus MARGIN.
    let main_span = max_seq as f32 * COMMIT_SPACING;
    let lane_span = (num_lanes - 1) as f32 * LANE_SPACING;

    let pos = |seq: usize, lane: usize| -> (f32, f32) {
        match g.dir {
            Dir::Lr => (
                MARGIN + LABEL_PAD + seq as f32 * COMMIT_SPACING,
                MARGIN + lane as f32 * LANE_SPACING,
            ),
            Dir::Tb => (
                MARGIN + lane as f32 * LANE_SPACING,
                MARGIN + LABEL_PAD + seq as f32 * COMMIT_SPACING,
            ),
            Dir::Bt => (
                MARGIN + lane as f32 * LANE_SPACING,
                MARGIN + LABEL_PAD + (max_seq - seq) as f32 * COMMIT_SPACING,
            ),
        }
    };

    // Canvas size. Allow trailing room for labels/tags on the far edge too.
    let (width, height) = match g.dir {
        Dir::Lr => (
            MARGIN + LABEL_PAD + main_span + MARGIN + COMMIT_R + 40.0,
            MARGIN + lane_span + MARGIN + fs * 2.0,
        ),
        Dir::Tb | Dir::Bt => (
            MARGIN + lane_span + MARGIN + 80.0,
            MARGIN + LABEL_PAD + main_span + MARGIN + fs * 2.0,
        ),
    };

    let mut svg = String::new();
    let w = (width.ceil() + 1.0).max(1.0);
    let h = (height.ceil() + 1.0).max(1.0);
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"0 0 {w} {h}\">"
    );

    // --- Lane lines: for each branch lane, a colored line spanning its commits.
    // A lane's extent runs from its earliest to its latest commit seq (so a
    // branch that never got a commit draws nothing).
    for lane in 0..num_lanes {
        let mut min_s: Option<usize> = None;
        let mut max_s: Option<usize> = None;
        for c in g.commits.iter().filter(|c| c.lane == lane) {
            min_s = Some(min_s.map_or(c.seq, |m| m.min(c.seq)));
            max_s = Some(max_s.map_or(c.seq, |m| m.max(c.seq)));
        }
        let (Some(a), Some(b)) = (min_s, max_s) else {
            continue;
        };
        let [r, gg, bb] = lane_color(lane);
        let (x0, y0) = pos(a, lane);
        let (x1, y1) = pos(b, lane);
        let _ = write!(
            svg,
            "<line x1=\"{x0:.2}\" y1=\"{y0:.2}\" x2=\"{x1:.2}\" y2=\"{y1:.2}\" \
             stroke=\"rgb({r},{gg},{bb})\" stroke-width=\"{LANE_W}\"/>",
        );
    }

    // --- Parent edges (drawn before commit circles so circles sit on top).
    for c in &g.commits {
        let (cx, cy) = pos(c.seq, c.lane);
        for &p in &c.parents {
            let parent = &g.commits[p];
            let (px, py) = pos(parent.seq, parent.lane);
            let [r, gg, bb] = lane_color(c.lane);
            if parent.lane == c.lane {
                // Same lane: straight segment (usually already covered by the
                // lane line, but drawn for robustness across gaps).
                let _ = write!(
                    svg,
                    "<line x1=\"{px:.2}\" y1=\"{py:.2}\" x2=\"{cx:.2}\" y2=\"{cy:.2}\" \
                     stroke=\"rgb({r},{gg},{bb})\" stroke-width=\"{LANE_W}\"/>",
                );
            } else {
                // Cross-lane (a merge or a branch point): a smooth curve.
                let (c1x, c1y, c2x, c2y) = match g.dir {
                    Dir::Lr => ((px + cx) / 2.0, py, (px + cx) / 2.0, cy),
                    Dir::Tb | Dir::Bt => (px, (py + cy) / 2.0, cx, (py + cy) / 2.0),
                };
                let _ = write!(
                    svg,
                    "<path d=\"M{px:.2},{py:.2} C{c1x:.2},{c1y:.2} {c2x:.2},{c2y:.2} {cx:.2},{cy:.2}\" \
                     fill=\"none\" stroke=\"rgb({r},{gg},{bb})\" stroke-width=\"{LANE_W}\"/>",
                );
            }
        }
    }

    // --- Commit circles + id labels + tags.
    let label_dy = COMMIT_R + fs; // id label offset below (LR) the commit.
    for c in &g.commits {
        let (cx, cy) = pos(c.seq, c.lane);
        let [r, gg, bb] = lane_color(c.lane);
        let fill = rgb([r, gg, bb, 255]);

        if let Some(picked) = &c.cherry_pick {
            // Cherry-pick marker: an outlined (white-filled) circle in the branch
            // color with a small ✗/cross of two crossing line segments through it,
            // setting it apart from a normal filled commit dot.
            let _ = write!(
                svg,
                "<circle cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{COMMIT_R:.2}\" fill=\"white\" \
                 stroke=\"{fill}\" stroke-width=\"2\"/>",
            );
            let d = COMMIT_R * 0.55;
            let _ = write!(
                svg,
                "<line x1=\"{:.2}\" y1=\"{:.2}\" x2=\"{:.2}\" y2=\"{:.2}\" stroke=\"{fill}\" stroke-width=\"2\"/>\
                 <line x1=\"{:.2}\" y1=\"{:.2}\" x2=\"{:.2}\" y2=\"{:.2}\" stroke=\"{fill}\" stroke-width=\"2\"/>",
                cx - d, cy - d, cx + d, cy + d,
                cx - d, cy + d, cx + d, cy - d,
            );

            // id label below (LR) / beside the commit.
            let (lx, ly, anchor) = match g.dir {
                Dir::Lr => (cx, cy + label_dy, "middle"),
                Dir::Tb | Dir::Bt => (cx + COMMIT_R + 4.0, cy, "start"),
            };
            emit_text(&mut svg, &c.id, lx, ly, fs * 0.85, opts, anchor);

            // Cherry-pick label (above the commit): the given tag, or
            // `cherry-pick:<picked-id>`.
            let label = match &c.tag {
                Some(t) => t.clone(),
                None => format!("cherry-pick:{picked}"),
            };
            let (tx, ty, ta) = match g.dir {
                Dir::Lr => (cx, cy - COMMIT_R - fs * 0.5, "middle"),
                Dir::Tb | Dir::Bt => (cx - COMMIT_R - 4.0, cy, "end"),
            };
            emit_tag(&mut svg, &label, tx, ty, fs * 0.8, opts, ta);
            continue;
        }

        match c.ctype {
            CommitType::Highlight => {
                // Larger, outlined square-ish marker (we use a bigger circle with
                // a heavy stroke as the "highlight" shape).
                let rr = COMMIT_R + 3.0;
                let _ = write!(
                    svg,
                    "<circle cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{rr:.2}\" fill=\"{fill}\" \
                     stroke=\"black\" stroke-width=\"3\"/>",
                );
            }
            CommitType::Reverse => {
                // Filled circle with a cross through it.
                let _ = write!(
                    svg,
                    "<circle cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{COMMIT_R:.2}\" fill=\"{fill}\" \
                     stroke=\"black\" stroke-width=\"1\"/>",
                );
                let d = COMMIT_R * 0.7;
                let _ = write!(
                    svg,
                    "<line x1=\"{:.2}\" y1=\"{:.2}\" x2=\"{:.2}\" y2=\"{:.2}\" stroke=\"black\" stroke-width=\"1.5\"/>\
                     <line x1=\"{:.2}\" y1=\"{:.2}\" x2=\"{:.2}\" y2=\"{:.2}\" stroke=\"black\" stroke-width=\"1.5\"/>",
                    cx - d, cy - d, cx + d, cy + d,
                    cx - d, cy + d, cx + d, cy - d,
                );
            }
            CommitType::Normal => {
                if c.is_merge {
                    // Merge commit: a smaller outlined circle to set it apart.
                    let _ = write!(
                        svg,
                        "<circle cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{:.2}\" fill=\"white\" \
                         stroke=\"{fill}\" stroke-width=\"2\"/>",
                        COMMIT_R - 2.0,
                    );
                } else {
                    let _ = write!(
                        svg,
                        "<circle cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{COMMIT_R:.2}\" fill=\"{fill}\"/>",
                    );
                }
            }
        }

        // id label.
        let (lx, ly, anchor) = match g.dir {
            Dir::Lr => (cx, cy + label_dy, "middle"),
            Dir::Tb | Dir::Bt => (cx + COMMIT_R + 4.0, cy, "start"),
        };
        emit_text(&mut svg, &c.id, lx, ly, fs * 0.85, opts, anchor);

        // tag flag (above the commit).
        if let Some(tag) = &c.tag {
            let (tx, ty, ta) = match g.dir {
                Dir::Lr => (cx, cy - COMMIT_R - fs * 0.5, "middle"),
                Dir::Tb | Dir::Bt => (cx - COMMIT_R - 4.0, cy, "end"),
            };
            emit_tag(&mut svg, tag, tx, ty, fs * 0.8, opts, ta);
        }
    }

    // --- Branch-name labels at the lane's leading edge.
    for (lane, name) in g.branches.iter().enumerate() {
        // Only label lanes that actually have commits.
        let has = g.commits.iter().any(|c| c.lane == lane);
        if !has {
            continue;
        }
        let [r, gg, bb] = lane_color(lane);
        let (lx, ly, anchor) = match g.dir {
            Dir::Lr => (MARGIN, MARGIN + lane as f32 * LANE_SPACING, "start"),
            Dir::Tb => (MARGIN + lane as f32 * LANE_SPACING, MARGIN, "middle"),
            Dir::Bt => (
                MARGIN + lane as f32 * LANE_SPACING,
                MARGIN + LABEL_PAD + main_span + fs,
                "middle",
            ),
        };
        let _ = write!(
            svg,
            "<text x=\"{lx:.2}\" y=\"{ly:.2}\" text-anchor=\"{anchor}\" dominant-baseline=\"central\" \
             font-family=\"{family}\" font-size=\"{ffs}\" font-weight=\"bold\" fill=\"rgb({r},{gg},{bb})\">{txt}</text>",
            family = escape(&opts.font_family),
            ffs = fs * 0.9,
            txt = escape(name),
        );
    }

    svg.push_str("</svg>");

    Ok(MermaidRender { svg, width_px: w, height_px: h })
}

/// A centered/anchored `<text>` for a commit id label.
fn emit_text(svg: &mut String, text: &str, x: f32, y: f32, fs: f32, opts: &MermaidOptions, anchor: &str) {
    let [r, g, b, _] = opts.text_color;
    let _ = write!(
        svg,
        "<text x=\"{x:.2}\" y=\"{y:.2}\" text-anchor=\"{anchor}\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs}\" fill=\"rgb({r},{g},{b})\">{txt}</text>",
        family = escape(&opts.font_family),
        txt = escape(text),
    );
}

/// A small flag-style tag label (rounded rect + text).
fn emit_tag(svg: &mut String, text: &str, x: f32, y: f32, fs: f32, opts: &MermaidOptions, anchor: &str) {
    let (tw, th) = text_size(text, fs);
    let pad = 3.0;
    let bw = tw + pad * 2.0;
    let bh = th + pad;
    // Background rect positioned per anchor so it frames the text.
    let rx = match anchor {
        "middle" => x - bw / 2.0,
        "end" => x - bw,
        _ => x,
    };
    let _ = write!(
        svg,
        "<rect x=\"{rx:.2}\" y=\"{ry:.2}\" width=\"{bw:.2}\" height=\"{bh:.2}\" rx=\"3\" \
         fill=\"rgb(255,245,200)\" stroke=\"rgb(200,170,60)\" stroke-width=\"1\"/>",
        ry = y - bh / 2.0,
    );
    let [r, g, b, _] = opts.text_color;
    let _ = write!(
        svg,
        "<text x=\"{x:.2}\" y=\"{y:.2}\" text-anchor=\"{anchor}\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs}\" fill=\"rgb({r},{g},{b})\">{txt}</text>",
        family = escape(&opts.font_family),
        txt = escape(text),
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "gitGraph\n\
        commit\n\
        branch dev\n\
        checkout dev\n\
        commit\n\
        checkout main\n\
        merge dev\n";

    #[test]
    fn parses_header_directions() {
        assert_eq!(parse_gitgraph("gitGraph\ncommit\n").unwrap().dir, Dir::Lr);
        assert_eq!(parse_gitgraph("gitGraph:\ncommit\n").unwrap().dir, Dir::Lr);
        assert_eq!(parse_gitgraph("gitGraph LR:\ncommit\n").unwrap().dir, Dir::Lr);
        assert_eq!(parse_gitgraph("gitGraph TB:\ncommit\n").unwrap().dir, Dir::Tb);
        assert_eq!(parse_gitgraph("gitGraph BT:\ncommit\n").unwrap().dir, Dir::Bt);
    }

    #[test]
    fn bad_header_errors() {
        assert!(parse_gitgraph("graph TD\nA-->B\n").is_err());
        assert!(parse_gitgraph("gitGraph XX:\ncommit\n").is_err());
    }

    #[test]
    fn parse_sample_commit_count_and_lanes() {
        let g = parse_gitgraph(SAMPLE).expect("parse");
        // 1 initial commit on main + 1 on dev + 1 merge commit on main = 3.
        assert_eq!(g.commits.len(), 3);
        // Two lanes: main (0), dev (1).
        assert_eq!(g.branches, vec!["main".to_string(), "dev".to_string()]);
        assert_eq!(g.commits[0].lane, 0); // first commit on main
        assert_eq!(g.commits[1].lane, 1); // dev commit
        assert_eq!(g.commits[2].lane, 0); // merge commit on main
    }

    #[test]
    fn merge_commit_has_two_parents() {
        let g = parse_gitgraph(SAMPLE).expect("parse");
        let merge = g.commits.iter().find(|c| c.is_merge).expect("a merge commit");
        assert_eq!(merge.parents.len(), 2, "merge has two parents");
        // Parents are the main head (commit 0) and the dev head (commit 1).
        assert!(merge.parents.contains(&0));
        assert!(merge.parents.contains(&1));
    }

    #[test]
    fn parses_id_tag_type() {
        let g = parse_gitgraph(
            "gitGraph\ncommit id: \"abc\" tag: \"v1\" type: HIGHLIGHT\n",
        )
        .expect("parse");
        assert_eq!(g.commits.len(), 1);
        assert_eq!(g.commits[0].id, "abc");
        assert_eq!(g.commits[0].tag.as_deref(), Some("v1"));
        assert_eq!(g.commits[0].ctype, CommitType::Highlight);
    }

    #[test]
    fn switch_is_checkout_alias() {
        let g = parse_gitgraph(
            "gitGraph\ncommit\nbranch dev\ncheckout main\ncommit\nswitch dev\ncommit\n",
        )
        .expect("parse");
        // dev got one commit via `switch`.
        let dev_lane = g.branches.iter().position(|b| b == "dev").unwrap();
        assert_eq!(g.commits.iter().filter(|c| c.lane == dev_lane).count(), 1);
    }

    #[test]
    fn branch_switches_current() {
        // After `branch dev` with no checkout, the next commit lands on dev.
        let g = parse_gitgraph("gitGraph\ncommit\nbranch dev\ncommit\n").expect("parse");
        assert_eq!(g.commits[1].lane, 1, "commit after branch goes to dev");
    }

    #[test]
    fn parent_chain_on_main() {
        let g = parse_gitgraph("gitGraph\ncommit\ncommit\ncommit\n").expect("parse");
        assert!(g.commits[0].parents.is_empty());
        assert_eq!(g.commits[1].parents, vec![0]);
        assert_eq!(g.commits[2].parents, vec![1]);
    }

    #[test]
    fn cherry_pick_noted_and_drawn() {
        let g = parse_gitgraph(
            "gitGraph\ncommit\ncherry-pick id: \"x\"\n",
        )
        .expect("parse");
        assert_eq!(g.commits.len(), 2);
        assert!(g.notes.iter().any(|n| n.contains("cherry-pick")));
    }

    #[test]
    fn cherry_pick_flagged_with_source_id() {
        let g = parse_gitgraph(
            "gitGraph\ncommit\ncherry-pick id: \"abc\"\n",
        )
        .expect("parse");
        assert_eq!(g.commits.len(), 2);
        let cp = &g.commits[1];
        assert_eq!(cp.cherry_pick.as_deref(), Some("abc"), "carries source id");
        assert!(!cp.is_merge);
        assert_eq!(cp.parents, vec![0], "parent edge is the prior commit");
        // A normal commit is not flagged.
        assert!(g.commits[0].cherry_pick.is_none());
    }

    #[test]
    fn cherry_pick_with_tag() {
        let g = parse_gitgraph(
            "gitGraph\ncommit\ncherry-pick id: \"abc\" tag: \"v9\"\n",
        )
        .expect("parse");
        let cp = &g.commits[1];
        assert_eq!(cp.cherry_pick.as_deref(), Some("abc"));
        assert_eq!(cp.tag.as_deref(), Some("v9"));
    }

    #[test]
    fn render_cherry_pick_marker_distinct() {
        let normal = render_gitgraph("gitGraph\ncommit\ncommit\n", &MermaidOptions::default())
            .expect("render normal");
        let cp = render_gitgraph(
            "gitGraph\ncommit\ncherry-pick id: \"abc\"\n",
            &MermaidOptions::default(),
        )
        .expect("render cherry-pick");

        // The cherry-pick marker draws crossing line segments + an outlined
        // circle + a `cherry-pick:abc` label, none of which a plain two-commit
        // graph produces.
        assert!(cp.svg.contains("cherry-pick:abc"), "picked-id label present");
        // The marker adds extra <line> segments (the cross) vs. the normal graph,
        // which on a single lane has no cross-lane connectors.
        assert!(
            cp.svg.matches("<line").count() > normal.svg.matches("<line").count(),
            "cherry-pick adds crossing line segments"
        );
        // The cherry-pick circle is outlined (white fill + colored stroke),
        // unlike a normal filled commit dot.
        assert!(cp.svg.contains("fill=\"white\""), "outlined circle");
    }

    #[test]
    fn render_cherry_pick_shows_tag_label() {
        let r = render_gitgraph(
            "gitGraph\ncommit\ncherry-pick id: \"abc\" tag: \"v9\"\n",
            &MermaidOptions::default(),
        )
        .expect("render");
        assert!(r.svg.contains(">v9<"), "shows the given tag");
        assert!(!r.svg.contains("cherry-pick:abc"), "tag overrides default label");
    }

    #[test]
    fn unknown_command_errors() {
        assert!(parse_gitgraph("gitGraph\nfrobnicate\n").is_err());
    }

    #[test]
    fn merge_unknown_branch_errors() {
        assert!(parse_gitgraph("gitGraph\ncommit\nmerge nope\n").is_err());
    }

    #[test]
    fn render_well_formed_svg() {
        let r = render_gitgraph(SAMPLE, &MermaidOptions::default()).expect("render");
        assert!(r.svg.starts_with("<svg"), "got: {}", &r.svg[..40.min(r.svg.len())]);
        assert!(r.svg.trim_end().ends_with("</svg>"));
        assert!(r.svg.contains("viewBox="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn render_one_circle_per_commit() {
        let r = render_gitgraph(SAMPLE, &MermaidOptions::default()).expect("render");
        // 3 commits → 3 commit circles.
        assert_eq!(r.svg.matches("<circle").count(), 3, "one circle per commit");
    }

    #[test]
    fn render_has_lane_lines_per_branch() {
        let r = render_gitgraph(SAMPLE, &MermaidOptions::default()).expect("render");
        // main has commits 0 and 2 (a span), dev has a single commit (no span).
        // At least the main lane line must be present.
        assert!(r.svg.contains("<line"), "expected lane lines");
    }

    #[test]
    fn render_has_merge_connector() {
        let r = render_gitgraph(SAMPLE, &MermaidOptions::default()).expect("render");
        // The cross-lane merge parent edge is drawn as a <path> curve.
        assert!(r.svg.contains("<path"), "expected a merge connector curve");
    }

    #[test]
    fn render_has_branch_labels() {
        let r = render_gitgraph(SAMPLE, &MermaidOptions::default()).expect("render");
        assert!(r.svg.contains(">main<"), "main branch label");
        assert!(r.svg.contains(">dev<"), "dev branch label");
    }

    #[test]
    fn render_xml_escapes() {
        let src = "gitGraph\ncommit tag: \"a & b <x>\"\n";
        let r = render_gitgraph(src, &MermaidOptions::default()).expect("render");
        assert!(r.svg.contains("a &amp; b &lt;x&gt;"), "tag escaped");
        assert!(!r.svg.contains("a & b <x>"));
    }

    #[test]
    fn empty_input_errors() {
        match render_gitgraph("", &MermaidOptions::default()) {
            Err(MermaidError::Parse(_)) => {}
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn header_only_is_empty() {
        let r = render_gitgraph("gitGraph\n", &MermaidOptions::default());
        assert!(matches!(r, Err(MermaidError::Empty)));
    }

    #[test]
    fn deterministic_output() {
        let opts = MermaidOptions::default();
        let a = render_gitgraph(SAMPLE, &opts).expect("a");
        let b = render_gitgraph(SAMPLE, &opts).expect("b");
        assert_eq!(a.svg, b.svg);
        assert_eq!(a.width_px, b.width_px);
        assert_eq!(a.height_px, b.height_px);
    }

    #[test]
    fn tb_direction_renders() {
        let src = "gitGraph TB:\ncommit\nbranch dev\ncommit\ncheckout main\nmerge dev\n";
        let r = render_gitgraph(src, &MermaidOptions::default()).expect("render");
        assert!(r.svg.starts_with("<svg"));
        assert_eq!(r.svg.matches("<circle").count(), 3);
    }
}
