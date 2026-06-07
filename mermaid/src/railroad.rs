//! `railroad` (syntax) diagram — self-contained: parse + recursive track layout.
//!
//! Renders EBNF-style grammars as classic *railroad diagrams*: each production
//! `name = <expr> ;` becomes a horizontal "rail" that the eye follows
//! left→right, with terminals drawn as rounded stadium capsules, non-terminals
//! as plain rectangles, choices as vertically-stacked branches, optionals as a
//! straight-through bypass rail, and repetitions as a loop-back rail beneath the
//! item.
//!
//! We target the **EBNF** dialect (upstream header `railroad-ebnf`), but the
//! dispatch in [`crate::lib`] also routes the bare `railroad`,
//! `railroad-diagram`, `railroad-abnf` and `railroad-peg` headers here; we
//! accept any of them leniently and parse the body as the EBNF subset below.
//!
//! ## EBNF subset parsed
//! A source is one or more **productions**:
//! ```text
//! name = <expr> ;        // also  name ::= <expr> ;   and  name : <expr> ;
//! ```
//! The expression grammar (recursive descent, choice → sequence → postfix →
//! primary):
//! - **choice**:     `a | b | c`
//! - **sequence**:   `a b c`  or  `a , b , c`
//! - **postfix**:    `a*` (zero-or-more) / `a+` (one-or-more) / `a?` (optional)
//! - **optional**:   `[ a ]`
//! - **repetition**: `{ a }`  (zero or more)
//! - **grouping**:   `( a )`
//! - **terminal**:   `"literal"` or `'literal'`
//! - **non-terminal**: a bare identifier
//!
//! Simplifications vs. full ISO EBNF / upstream mermaid: we do **not** model
//! ABNF / PEG dialect specifics, EBNF *special sequences* (`? … ?`) — they parse
//! as an opaque terminal — nor the `- except` exception postfix (the `-` and its
//! operand are skipped). Curve geometry is approximate (quarter-circle arcs),
//! not a pixel-match of `railroad-diagrams.js`.
//!
//! Reference: `references/mermaid/packages/mermaid/src/diagrams/railroad/` and
//! `references/mermaid/packages/parser/src/language/railroad-ebnf/`.

use std::fmt::Write as _;

use crate::svgutil::{escape, opacity_attr, rgb, text_size};
use crate::{MermaidError, MermaidOptions, MermaidRender};

// ---------------------------------------------------------------------------
// Expression tree
// ---------------------------------------------------------------------------

/// A node in a production's expression tree.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    /// A quoted literal, e.g. `"+"` — drawn as a stadium capsule.
    Terminal(String),
    /// A bare identifier referencing another production — drawn as a rectangle.
    NonTerminal(String),
    /// Items in a row (`a b c` / `a , b , c`).
    Seq(Vec<Node>),
    /// Alternatives (`a | b | c`).
    Choice(Vec<Node>),
    /// `[ a ]` or `a?` — the child may be skipped via a bypass rail.
    Optional(Box<Node>),
    /// `{ a }` or `a*` — zero or more, with a loop-back rail.
    Repeat(Box<Node>),
}

/// A single production: a `name` and its expression tree.
#[derive(Clone, Debug, PartialEq)]
pub struct Production {
    pub name: String,
    pub expr: Node,
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    /// A bare identifier.
    Ident(String),
    /// A quoted literal (the inner text, quotes stripped, escapes resolved).
    Str(String),
    /// A special sequence `? … ?` (kept as opaque text including delimiters).
    Special(String),
    /// `=` or `::=` or `:` — the production-definition operator.
    Define,
    /// `;` — end of a production.
    Semi,
    /// `|`
    Bar,
    /// `,`
    Comma,
    /// `(`  `)`  `[`  `]`  `{`  `}`
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    /// `*` `+` `?`
    Star,
    Plus,
    Question,
    /// `-` (exception postfix; operand skipped).
    Minus,
}

/// Tokenize one production's source text (operator already-or-not consumed).
/// Returns `Err` with a message on an unterminated string/special.
fn tokenize(src: &str) -> Result<Vec<Tok>, String> {
    let bytes: Vec<char> = src.chars().collect();
    let mut i = 0;
    let n = bytes.len();
    let mut out = Vec::new();
    while i < n {
        let c = bytes[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        match c {
            '=' => {
                out.push(Tok::Define);
                i += 1;
            }
            ':' => {
                // `::=` or `:` both mean "define".
                if i + 2 < n && bytes[i + 1] == ':' && bytes[i + 2] == '=' {
                    i += 3;
                } else if i + 1 < n && bytes[i + 1] == '=' {
                    i += 2;
                } else {
                    i += 1;
                }
                out.push(Tok::Define);
            }
            ';' => {
                out.push(Tok::Semi);
                i += 1;
            }
            '|' => {
                out.push(Tok::Bar);
                i += 1;
            }
            ',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            '(' => {
                // ISO comment `(* … *)` — skip.
                if i + 1 < n && bytes[i + 1] == '*' {
                    i += 2;
                    while i + 1 < n && !(bytes[i] == '*' && bytes[i + 1] == ')') {
                        i += 1;
                    }
                    i += 2;
                } else {
                    out.push(Tok::LParen);
                    i += 1;
                }
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            '[' => {
                out.push(Tok::LBracket);
                i += 1;
            }
            ']' => {
                out.push(Tok::RBracket);
                i += 1;
            }
            '{' => {
                out.push(Tok::LBrace);
                i += 1;
            }
            '}' => {
                out.push(Tok::RBrace);
                i += 1;
            }
            '*' => {
                out.push(Tok::Star);
                i += 1;
            }
            '+' => {
                out.push(Tok::Plus);
                i += 1;
            }
            '-' => {
                out.push(Tok::Minus);
                i += 1;
            }
            '"' | '\'' => {
                let quote = c;
                i += 1;
                let mut s = String::new();
                let mut closed = false;
                while i < n {
                    let d = bytes[i];
                    if d == '\\' && i + 1 < n {
                        // Keep the escaped char literally (drop the backslash).
                        s.push(bytes[i + 1]);
                        i += 2;
                        continue;
                    }
                    if d == quote {
                        closed = true;
                        i += 1;
                        break;
                    }
                    s.push(d);
                    i += 1;
                }
                if !closed {
                    return Err(format!("unterminated string literal: {quote}{s}"));
                }
                out.push(Tok::Str(s));
            }
            '?' => {
                // Could be a special sequence `? … ?` or a postfix `?`. Peek for a
                // closing `?` before the next `;`/EOL with non-ws content between.
                if let Some(end) = find_special_end(&bytes, i + 1) {
                    let text: String = bytes[i..=end].iter().collect();
                    out.push(Tok::Special(text));
                    i = end + 1;
                } else {
                    out.push(Tok::Question);
                    i += 1;
                }
            }
            _ => {
                if is_ident_start(c) {
                    let start = i;
                    i += 1;
                    while i < n && is_ident_part(bytes[i]) {
                        i += 1;
                    }
                    out.push(Tok::Ident(bytes[start..i].iter().collect()));
                } else {
                    // Unknown char — skip it leniently.
                    i += 1;
                }
            }
        }
    }
    Ok(out)
}

/// Find the closing `?` of a special sequence beginning at `start`, requiring at
/// least one non-whitespace char before it and stopping at `;`. Returns the
/// index of the closing `?`, or `None` (then `?` is a postfix operator).
fn find_special_end(bytes: &[char], start: usize) -> Option<usize> {
    let mut j = start;
    let mut saw_content = false;
    while j < bytes.len() {
        match bytes[j] {
            '?' => return if saw_content { Some(j) } else { None },
            ';' => return None,
            c if !c.is_whitespace() => {
                saw_content = true;
            }
            _ => {}
        }
        j += 1;
    }
    None
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_part(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

// ---------------------------------------------------------------------------
// Recursive-descent parser
// ---------------------------------------------------------------------------

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn new(toks: Vec<Tok>) -> Self {
        Parser { toks, pos: 0 }
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, want: &Tok) -> bool {
        if self.peek() == Some(want) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// `choice := sequence ('|' sequence)*`
    fn parse_choice(&mut self) -> Result<Node, String> {
        let mut alts = vec![self.parse_sequence()?];
        while self.eat(&Tok::Bar) {
            alts.push(self.parse_sequence()?);
        }
        Ok(if alts.len() == 1 { alts.pop().unwrap() } else { Node::Choice(alts) })
    }

    /// `sequence := term (','? term)*` — stops at `|`, `)`, `]`, `}`, `;`, EOF.
    fn parse_sequence(&mut self) -> Result<Node, String> {
        let mut items = vec![self.parse_term()?];
        loop {
            // An optional comma between elements.
            self.eat(&Tok::Comma);
            match self.peek() {
                None
                | Some(Tok::Bar)
                | Some(Tok::RParen)
                | Some(Tok::RBracket)
                | Some(Tok::RBrace)
                | Some(Tok::Semi) => break,
                _ => {}
            }
            items.push(self.parse_term()?);
        }
        Ok(if items.len() == 1 { items.pop().unwrap() } else { Node::Seq(items) })
    }

    /// `term := primary postfix*` where postfix is `* + ?` or `- primary`.
    fn parse_term(&mut self) -> Result<Node, String> {
        let mut node = self.parse_primary()?;
        loop {
            match self.peek() {
                Some(Tok::Star) => {
                    self.pos += 1;
                    node = Node::Repeat(Box::new(node));
                }
                Some(Tok::Plus) => {
                    // one-or-more: the item, then a zero-or-more loop. Model as a
                    // Seq[item, Repeat(item)] would duplicate; keep it readable as
                    // a Repeat (the loop-back conveys "more"). Simplification.
                    self.pos += 1;
                    node = Node::Repeat(Box::new(node));
                }
                Some(Tok::Question) => {
                    self.pos += 1;
                    node = Node::Optional(Box::new(node));
                }
                Some(Tok::Minus) => {
                    // Exception `- primary`: consume and discard the operand.
                    self.pos += 1;
                    let _ = self.parse_primary()?;
                }
                _ => break,
            }
        }
        Ok(node)
    }

    /// `primary := terminal | nonterminal | '(' choice ')' | '[' choice ']'
    ///           | '{' choice '}' | special`
    fn parse_primary(&mut self) -> Result<Node, String> {
        match self.next() {
            Some(Tok::Str(s)) => Ok(Node::Terminal(s)),
            Some(Tok::Special(s)) => Ok(Node::Terminal(s)),
            Some(Tok::Ident(s)) => Ok(Node::NonTerminal(s)),
            Some(Tok::LParen) => {
                let inner = self.parse_choice()?;
                if !self.eat(&Tok::RParen) {
                    return Err("expected ')'".to_string());
                }
                Ok(inner)
            }
            Some(Tok::LBracket) => {
                let inner = self.parse_choice()?;
                if !self.eat(&Tok::RBracket) {
                    return Err("expected ']'".to_string());
                }
                Ok(Node::Optional(Box::new(inner)))
            }
            Some(Tok::LBrace) => {
                let inner = self.parse_choice()?;
                if !self.eat(&Tok::RBrace) {
                    return Err("expected '}'".to_string());
                }
                Ok(Node::Repeat(Box::new(inner)))
            }
            other => Err(format!("expected a term, got {other:?}")),
        }
    }
}

// ---------------------------------------------------------------------------
// Source → productions
// ---------------------------------------------------------------------------

/// Strip the diagram header line, comments and directive/title lines, returning
/// the grammar body. Returns `Err` if the header is not a recognized
/// `railroad*` keyword.
fn strip_header(src: &str) -> Result<String, String> {
    // First non-blank, non-comment line must be a railroad header.
    let mut header_seen = false;
    let mut body = String::new();
    for raw in src.lines() {
        let trimmed = raw.trim_start();
        if !header_seen {
            let line = trimmed.split("%%").next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let kw = line.split_whitespace().next().unwrap_or("");
            if !is_railroad_header(kw) {
                return Err(format!("expected a 'railroad' header, got: {line:?}"));
            }
            header_seen = true;
            // Anything after the header keyword on the same line is body.
            let rest = &line[kw.len()..];
            body.push_str(rest);
            body.push('\n');
            continue;
        }
        // Body: drop full-line `%%` comments and `title`/`accTitle`/`accDescr`.
        let no_comment = strip_line_comment(raw);
        let probe = no_comment.trim_start();
        let first = probe.split_whitespace().next().unwrap_or("");
        if first == "title" || first == "accTitle" || first == "accDescr" {
            continue;
        }
        body.push_str(&no_comment);
        body.push('\n');
    }
    if !header_seen {
        return Err("empty input / no 'railroad' header".to_string());
    }
    Ok(body)
}

/// Remove a trailing `%%` single-line comment from a line (leaving content).
fn strip_line_comment(line: &str) -> String {
    line.split("%%").next().unwrap_or("").to_string()
}

fn is_railroad_header(kw: &str) -> bool {
    matches!(
        kw,
        "railroad"
            | "railroad-diagram"
            | "railroad-ebnf"
            | "railroad-abnf"
            | "railroad-peg"
    )
}

/// Split the body into productions (terminated by `;`) and parse each. A
/// production lacking `;` but ending the input is still accepted. Bad
/// productions are skipped; returns `Err` only if *none* parse.
fn parse_productions(body: &str) -> Result<Vec<Production>, String> {
    let toks = tokenize(body)?;
    // Slice token stream into productions at `Semi`.
    let mut prods = Vec::new();
    let mut last_err: Option<String> = None;
    let mut start = 0;
    let mut i = 0;
    while i <= toks.len() {
        let at_end = i == toks.len();
        if at_end || toks[i] == Tok::Semi {
            if i > start {
                let slice = toks[start..i].to_vec();
                match parse_one(slice) {
                    Ok(Some(p)) => prods.push(p),
                    Ok(None) => {}
                    Err(e) => last_err = Some(e),
                }
            }
            start = i + 1;
        }
        i += 1;
    }
    if prods.is_empty() {
        return Err(last_err.unwrap_or_else(|| "no productions found".to_string()));
    }
    Ok(prods)
}

/// Parse a single production's tokens: `Ident Define choice`. Returns `Ok(None)`
/// for an empty slice.
fn parse_one(toks: Vec<Tok>) -> Result<Option<Production>, String> {
    if toks.is_empty() {
        return Ok(None);
    }
    let mut p = Parser::new(toks);
    let name = match p.next() {
        Some(Tok::Ident(s)) => s,
        other => return Err(format!("expected production name, got {other:?}")),
    };
    if !p.eat(&Tok::Define) {
        return Err(format!("expected '=' / '::=' / ':' after {name:?}"));
    }
    let expr = p.parse_choice()?;
    if p.pos != p.toks.len() {
        return Err(format!("trailing tokens in production {name:?}"));
    }
    Ok(Some(Production { name, expr }))
}

// ---------------------------------------------------------------------------
// Layout: measure
// ---------------------------------------------------------------------------

/// A node's measured extent relative to its entry/exit rail line: width, plus
/// the rail-relative heights above (`up`) and below (`down`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub w: f32,
    pub up: f32,
    pub down: f32,
}

/// Layout tunables, derived from the font size.
#[derive(Clone, Copy)]
struct Metrics {
    fs: f32,
    /// Box half-height (capsule/rect extend ±this from the rail).
    box_half: f32,
    /// Horizontal padding inside a box, each side.
    pad_x: f32,
    /// Straight rail run between sequence items.
    seq_gap: f32,
    /// Horizontal lead-in/out for choice/optional/repeat connectors.
    lead: f32,
    /// Vertical gap between stacked branches (between adjacent rails).
    branch_gap: f32,
    /// Arc radius for curved connectors.
    arc: f32,
}

impl Metrics {
    fn new(fs: f32) -> Self {
        let box_h = fs * 1.7;
        Metrics {
            fs,
            box_half: box_h / 2.0,
            pad_x: fs * 0.7,
            seq_gap: fs * 0.9,
            lead: fs * 1.1,
            branch_gap: fs * 1.1,
            arc: fs * 0.55,
        }
    }
}

/// Width of a leaf box (capsule/rect) for the given label.
fn box_width(label: &str, m: &Metrics, capsule: bool) -> f32 {
    let (tw, _) = text_size(label, m.fs);
    // Capsules get extra room for the rounded caps (rx = box_half each side).
    let caps = if capsule { m.box_half * 2.0 } else { 0.0 };
    tw + 2.0 * m.pad_x + caps
}

/// Recursively measure a node.
fn measure(node: &Node, m: &Metrics) -> Size {
    match node {
        Node::Terminal(s) => Size {
            w: box_width(s, m, true).max(m.box_half * 2.0),
            up: m.box_half,
            down: m.box_half,
        },
        Node::NonTerminal(s) => Size {
            w: box_width(s, m, false).max(m.box_half * 2.0),
            up: m.box_half,
            down: m.box_half,
        },
        Node::Seq(items) => {
            if items.is_empty() {
                return Size { w: m.seq_gap, up: m.box_half, down: m.box_half };
            }
            let mut w = 0.0;
            let mut up = 0.0f32;
            let mut down = 0.0f32;
            for (i, it) in items.iter().enumerate() {
                let s = measure(it, m);
                if i > 0 {
                    w += m.seq_gap;
                }
                w += s.w;
                up = up.max(s.up);
                down = down.max(s.down);
            }
            Size { w, up, down }
        }
        Node::Choice(alts) => {
            // Branches are stacked; the main rail aligns with the first branch's
            // rail. Width is the widest branch plus lead-in/out on both sides.
            let inner_w = alts
                .iter()
                .map(|a| measure(a, m).w)
                .fold(0.0f32, f32::max);
            let w = inner_w + 2.0 * (m.lead + m.arc);
            // First branch sits on the rail; subsequent branches stack below it.
            let mut up = m.box_half;
            let mut down = m.box_half;
            let mut offset = 0.0f32; // rail-to-rail vertical offset of current branch
            for (i, a) in alts.iter().enumerate() {
                let s = measure(a, m);
                if i == 0 {
                    up = up.max(s.up);
                    down = down.max(s.down);
                } else {
                    // Stack below the previous branch.
                    offset += s.up.max(m.box_half)
                        + m.branch_gap
                        + prev_down(alts, i, m);
                    down = down.max(offset + s.down);
                }
            }
            Size { w, up, down }
        }
        Node::Optional(child) => {
            // A 2-way choice between a straight bypass (on the rail) and the child
            // below it.
            let s = measure(child, m);
            let w = s.w + 2.0 * (m.lead + m.arc);
            let down = m.box_half + m.branch_gap + s.up + s.down;
            Size { w, up: m.box_half, down }
        }
        Node::Repeat(child) => {
            // Child on the main rail; loop-back rail below it.
            let s = measure(child, m);
            let w = s.w + 2.0 * (m.lead + m.arc);
            let down = s.down + m.branch_gap + m.box_half;
            Size { w, up: s.up.max(m.box_half), down }
        }
    }
}

/// Vertical room the previous branch's `down` contributes when stacking.
fn prev_down(alts: &[Node], i: usize, m: &Metrics) -> f32 {
    if i == 0 {
        0.0
    } else {
        measure(&alts[i - 1], m).down.max(m.box_half)
    }
}

// ---------------------------------------------------------------------------
// Layout: draw
// ---------------------------------------------------------------------------

/// Accumulates SVG path/box fragments plus the active theme colors.
struct Canvas<'a> {
    out: String,
    opts: &'a MermaidOptions,
    m: Metrics,
}

impl<'a> Canvas<'a> {
    fn rail_attrs(&self) -> String {
        format!(
            "fill=\"none\" stroke=\"{s}\"{so} stroke-width=\"1.5\"",
            s = rgb(self.opts.edge_stroke),
            so = opacity_attr("stroke-opacity", self.opts.edge_stroke),
        )
    }

    /// A straight horizontal rail segment from `(x0,y)` to `(x1,y)`.
    fn hline(&mut self, x0: f32, x1: f32, y: f32) {
        if (x1 - x0).abs() < 0.01 {
            return;
        }
        let _ = write!(
            self.out,
            "<line x1=\"{x0:.2}\" y1=\"{y:.2}\" x2=\"{x1:.2}\" y2=\"{y:.2}\" {a}/>",
            a = self.rail_attrs(),
        );
    }

    /// A free-form path with the rail stroke.
    fn path(&mut self, d: &str) {
        let _ = write!(self.out, "<path d=\"{d}\" {a}/>", a = self.rail_attrs());
    }

    /// Draw a leaf box (capsule if `capsule`) centered on rail line `y`, left
    /// edge at `x`, given width `w`.
    fn draw_box(&mut self, x: f32, y: f32, w: f32, label: &str, capsule: bool) {
        let h = self.m.box_half * 2.0;
        let top = y - self.m.box_half;
        let rx = if capsule { self.m.box_half } else { (self.m.fs * 0.25).min(8.0) };
        let _ = write!(
            self.out,
            "<rect x=\"{x:.2}\" y=\"{top:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" \
             rx=\"{rx:.2}\" ry=\"{rx:.2}\" fill=\"{fill}\"{fo} stroke=\"{stroke}\"{so} \
             stroke-width=\"1.5\"/>",
            fill = rgb(self.opts.node_fill),
            fo = opacity_attr("fill-opacity", self.opts.node_fill),
            stroke = rgb(self.opts.node_stroke),
            so = opacity_attr("stroke-opacity", self.opts.node_stroke),
        );
        let cx = x + w / 2.0;
        let _ = write!(
            self.out,
            "<text x=\"{cx:.2}\" y=\"{y:.2}\" text-anchor=\"middle\" \
             dominant-baseline=\"central\" font-family=\"{family}\" font-size=\"{fs}\" \
             fill=\"{tc}\"{to}>{txt}</text>",
            family = escape(&self.opts.font_family),
            fs = self.m.fs,
            tc = rgb(self.opts.text_color),
            to = opacity_attr("fill-opacity", self.opts.text_color),
            txt = escape(label),
        );
    }

    /// Draw a node whose rail enters at `(x, y)` on the left and exits on the
    /// right after `measure(node).w`. The node is responsible for any internal
    /// rails and for landing its exit rail back on `y`.
    fn draw(&mut self, node: &Node, x: f32, y: f32) {
        let m = self.m;
        match node {
            Node::Terminal(s) => {
                let w = measure(node, &m).w;
                self.draw_box(x, y, w, s, true);
            }
            Node::NonTerminal(s) => {
                let w = measure(node, &m).w;
                self.draw_box(x, y, w, s, false);
            }
            Node::Seq(items) => {
                let mut cx = x;
                for (i, it) in items.iter().enumerate() {
                    if i > 0 {
                        self.hline(cx, cx + m.seq_gap, y);
                        cx += m.seq_gap;
                    }
                    let s = measure(it, &m);
                    self.draw(it, cx, y);
                    cx += s.w;
                }
            }
            Node::Choice(alts) => self.draw_choice(alts, x, y),
            Node::Optional(child) => self.draw_optional(child, x, y),
            Node::Repeat(child) => self.draw_repeat(child, x, y),
        }
    }

    /// Choice: stacked branches with curved split/join connectors. The first
    /// branch is on the main rail line `y`; the rest stack downward.
    fn draw_choice(&mut self, alts: &[Node], x: f32, y: f32) {
        let m = self.m;
        let total = measure(&Node::Choice(alts.to_vec()), &m);
        let inner_w = total.w - 2.0 * (m.lead + m.arc);
        let left_split = x + m.arc; // where the curved fan-out begins
        let inner_x = x + m.lead + m.arc;
        let right_join = x + total.w - m.arc;
        let exit_x = x + total.w;

        // Lead-in / lead-out straight stubs on the main rail.
        self.hline(x, left_split, y);
        self.hline(right_join, exit_x, y);

        // Track each branch's rail-y as we stack downward.
        let mut branch_y = y;
        for (i, a) in alts.iter().enumerate() {
            let s = measure(a, &m);
            if i > 0 {
                let prev_down = prev_down(alts, i, &m);
                branch_y += prev_down + m.branch_gap + s.up.max(m.box_half);
            }
            // Center the branch within inner_w.
            let bx = inner_x + (inner_w - s.w) / 2.0;
            // Connector from the split point to this branch's entry, and from its
            // exit to the join point.
            if i == 0 {
                // Straight through on the main rail (with centering stubs).
                self.hline(left_split, bx, y);
                self.draw(a, bx, y);
                self.hline(bx + s.w, right_join, y);
            } else {
                self.branch_in(left_split, y, inner_x, branch_y);
                self.hline(inner_x, bx, branch_y);
                self.draw(a, bx, branch_y);
                self.hline(bx + s.w, right_join - m.arc, branch_y);
                self.branch_out(right_join - m.arc, branch_y, right_join, y);
            }
        }
    }

    /// Optional: a straight bypass on the main rail, with the child on a branch
    /// below reached by curved connectors (a 2-way choice with an empty top).
    fn draw_optional(&mut self, child: &Node, x: f32, y: f32) {
        let m = self.m;
        let s = measure(child, &m);
        let total = measure(&Node::Optional(Box::new(child.clone())), &m);
        let inner_w = total.w - 2.0 * (m.lead + m.arc);
        let left_split = x + m.arc;
        let inner_x = x + m.lead + m.arc;
        let right_join = x + total.w - m.arc;
        let exit_x = x + total.w;

        // Bypass (straight through, top).
        self.hline(x, exit_x, y);

        // Child branch below.
        let branch_y = y + m.box_half + m.branch_gap + s.up;
        let bx = inner_x + (inner_w - s.w) / 2.0;
        self.branch_in(left_split, y, inner_x, branch_y);
        self.hline(inner_x, bx, branch_y);
        self.draw(child, bx, branch_y);
        self.hline(bx + s.w, right_join - m.arc, branch_y);
        self.branch_out(right_join - m.arc, branch_y, right_join, y);
    }

    /// Repeat: child on the main rail, with a loop-back rail beneath returning
    /// the exit to the entry.
    fn draw_repeat(&mut self, child: &Node, x: f32, y: f32) {
        let m = self.m;
        let s = measure(child, &m);
        let total = measure(&Node::Repeat(Box::new(child.clone())), &m);
        let inner_x = x + m.lead + m.arc;
        let inner_w = total.w - 2.0 * (m.lead + m.arc);
        let exit_x = x + total.w;
        let right_inner = x + total.w - m.lead - m.arc;

        // Lead-in / lead-out on the main rail.
        self.hline(x, inner_x, y);
        let bx = inner_x + (inner_w - s.w) / 2.0;
        self.hline(inner_x, bx, y);
        self.draw(child, bx, y);
        self.hline(bx + s.w, right_inner, y);
        self.hline(right_inner, exit_x, y);

        // Loop-back rail below: down at the right, across leftward, up at the
        // left — two curved corners.
        let loop_y = y + s.down + m.branch_gap;
        let r = m.arc;
        // Right corner: from (right_inner, y) curve down to (right_inner, loop_y).
        self.path(&format!(
            "M {rx:.2} {y:.2} A {r:.2} {r:.2} 0 0 1 {rx2:.2} {ya:.2} L {rx2:.2} {lyb:.2} \
             A {r:.2} {r:.2} 0 0 1 {rx3:.2} {ly:.2}",
            rx = right_inner,
            y = y,
            r = r,
            rx2 = right_inner + r,
            ya = y + r,
            lyb = loop_y - r,
            rx3 = right_inner,
            ly = loop_y,
        ));
        // Horizontal loop-back run (right→left).
        self.hline(inner_x, right_inner, loop_y);
        // Left corner: from (inner_x, loop_y) curve up to (inner_x, y).
        self.path(&format!(
            "M {ix:.2} {ly:.2} A {r:.2} {r:.2} 0 0 1 {ixl:.2} {lyb:.2} L {ixl:.2} {ya:.2} \
             A {r:.2} {r:.2} 0 0 1 {ix:.2} {y:.2}",
            ix = inner_x,
            ly = loop_y,
            r = r,
            ixl = inner_x - r,
            lyb = loop_y - r,
            ya = y + r,
            y = y,
        ));
    }

    /// A curved connector fanning out from the split `(x0,y0)` down to a branch
    /// entry at `(x1,y1)` (y1 > y0). Quarter-circle out, then in.
    fn branch_in(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) {
        let r = self.m.arc.min((y1 - y0).abs() / 2.0).max(1.0);
        // down-curve at x0, vertical, then in-curve at x1.
        self.path(&format!(
            "M {x0:.2} {y0:.2} A {r:.2} {r:.2} 0 0 1 {xa:.2} {ya:.2} L {xa:.2} {yb:.2} \
             A {r:.2} {r:.2} 0 0 0 {x1:.2} {y1:.2}",
            xa = x0 + r,
            ya = y0 + r,
            yb = y1 - r,
        ));
    }

    /// A curved connector from a branch exit `(x0,y0)` back up to the join
    /// `(x1,y1)` (y0 > y1).
    fn branch_out(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) {
        let r = self.m.arc.min((y0 - y1).abs() / 2.0).max(1.0);
        self.path(&format!(
            "M {x0:.2} {y0:.2} A {r:.2} {r:.2} 0 0 0 {xa:.2} {ya:.2} L {xa:.2} {yb:.2} \
             A {r:.2} {r:.2} 0 0 1 {x1:.2} {y1:.2}",
            xa = x0 + r,
            ya = y0 - r,
            yb = y1 + r,
        ));
    }

    /// A filled start/end marker (a small vertical bar with a stub) on the rail.
    fn end_marker(&mut self, x: f32, y: f32, start: bool) {
        let r = self.m.fs * 0.32;
        let _ = write!(
            self.out,
            "<circle cx=\"{x:.2}\" cy=\"{y:.2}\" r=\"{r:.2}\" fill=\"{s}\"{so}/>",
            s = rgb(self.opts.edge_stroke),
            so = opacity_attr("fill-opacity", self.opts.edge_stroke),
        );
        let _ = start; // both ends drawn identically.
    }
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Outer margins and inter-production spacing.
const MARGIN: f32 = 16.0;

/// Render a mermaid `railroad` (EBNF) diagram to SVG.
pub fn render_railroad(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let body = strip_header(src).map_err(MermaidError::Parse)?;
    let prods = parse_productions(&body).map_err(MermaidError::Parse)?;
    if prods.is_empty() {
        return Err(MermaidError::Empty);
    }

    let m = Metrics::new(opts.font_size_px);
    let label_fs = opts.font_size_px;
    let label_h = text_size("Xy", label_fs).1;
    let label_gap = m.fs * 0.5;
    // Lead room on each side of a track for the start/end markers + stub.
    let marker_lead = m.fs * 1.4;
    let track_gap = m.fs * 1.6; // vertical gap between productions

    // Measure each production to compute the canvas size.
    struct Placed {
        prod_idx: usize,
        label_y: f32,
        rail_y: f32,
        size: Size,
    }
    let mut placed = Vec::with_capacity(prods.len());
    let mut cur_y = MARGIN;
    let mut max_w = 0.0f32;
    for (idx, p) in prods.iter().enumerate() {
        let s = measure(&p.expr, &m);
        let label_y = cur_y + label_h / 2.0;
        let rail_y = cur_y + label_h + label_gap + s.up;
        let track_w = s.w + 2.0 * marker_lead;
        max_w = max_w.max(track_w);
        placed.push(Placed { prod_idx: idx, label_y, rail_y, size: s });
        cur_y = rail_y + s.down + track_gap;
    }
    let content_h = cur_y - track_gap + MARGIN;
    let width = max_w + 2.0 * MARGIN;
    let height = content_h;
    let w = (width.ceil() + 1.0).max(1.0);
    let h = (height.ceil() + 1.0).max(1.0);

    let mut canvas = Canvas { out: String::new(), opts, m };

    for pl in &placed {
        let p = &prods[pl.prod_idx];
        // Production name label, left-aligned above the track.
        let _ = write!(
            canvas.out,
            "<text x=\"{x:.2}\" y=\"{y:.2}\" text-anchor=\"start\" \
             dominant-baseline=\"central\" font-family=\"{family}\" font-size=\"{fs}\" \
             font-weight=\"bold\" fill=\"{tc}\"{to}>{txt}</text>",
            x = MARGIN,
            y = pl.label_y,
            family = escape(&opts.font_family),
            fs = label_fs,
            tc = rgb(opts.text_color),
            to = opacity_attr("fill-opacity", opts.text_color),
            txt = escape(&p.name),
        );

        let track_x = MARGIN;
        let rail_y = pl.rail_y;
        // Start marker + stub.
        canvas.end_marker(track_x, rail_y, true);
        canvas.hline(track_x, track_x + marker_lead, rail_y);
        // The expression.
        canvas.draw(&p.expr, track_x + marker_lead, rail_y);
        // Exit stub + end marker.
        let exit_x = track_x + marker_lead + pl.size.w;
        canvas.hline(exit_x, exit_x + marker_lead, rail_y);
        canvas.end_marker(exit_x + marker_lead, rail_y, false);
    }

    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"0 0 {w} {h}\">"
    );
    svg.push_str(&canvas.out);
    svg.push_str("</svg>");

    Ok(MermaidRender { svg, width_px: w, height_px: h })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Vec<Production> {
        let body = strip_header(src).expect("header");
        parse_productions(&body).expect("parse")
    }

    #[test]
    fn parses_repeat_production() {
        // expr = term { "+" term } ;
        let prods = parse("railroad-ebnf\nexpr = term { \"+\" term } ;\n");
        assert_eq!(prods.len(), 1);
        assert_eq!(prods[0].name, "expr");
        match &prods[0].expr {
            Node::Seq(items) => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0], Node::NonTerminal("term".to_string()));
                match &items[1] {
                    Node::Repeat(inner) => match &**inner {
                        Node::Seq(s) => {
                            assert_eq!(s[0], Node::Terminal("+".to_string()));
                            assert_eq!(s[1], Node::NonTerminal("term".to_string()));
                        }
                        other => panic!("expected Seq in repeat, got {other:?}"),
                    },
                    other => panic!("expected Repeat, got {other:?}"),
                }
            }
            other => panic!("expected Seq, got {other:?}"),
        }
    }

    #[test]
    fn parses_choice_and_group() {
        // term = "a" | "b" | ( expr ) ;
        let prods = parse("railroad-ebnf\nterm = \"a\" | \"b\" | ( expr ) ;\n");
        assert_eq!(prods.len(), 1);
        match &prods[0].expr {
            Node::Choice(alts) => {
                assert_eq!(alts.len(), 3);
                assert_eq!(alts[0], Node::Terminal("a".to_string()));
                assert_eq!(alts[1], Node::Terminal("b".to_string()));
                // Group unwraps to its inner expression.
                assert_eq!(alts[2], Node::NonTerminal("expr".to_string()));
            }
            other => panic!("expected Choice, got {other:?}"),
        }
    }

    #[test]
    fn parses_optional_and_postfix() {
        let prods = parse("railroad-ebnf\nx = [ \"a\" ] \"b\"* \"c\"? ;\n");
        match &prods[0].expr {
            Node::Seq(items) => {
                assert!(matches!(items[0], Node::Optional(_)));
                assert!(matches!(items[1], Node::Repeat(_)));
                assert!(matches!(items[2], Node::Optional(_)));
            }
            other => panic!("expected Seq, got {other:?}"),
        }
    }

    #[test]
    fn accepts_alternate_define_ops_and_headers() {
        assert_eq!(parse("railroad\na ::= \"x\" ;").len(), 1);
        assert_eq!(parse("railroad-diagram\na : \"x\" ;").len(), 1);
        assert_eq!(parse("railroad-peg\na = \"x\" ;").len(), 1);
    }

    #[test]
    fn multiple_productions() {
        let prods = parse(
            "railroad-ebnf\nexpr = term { \"+\" term } ;\nterm = \"a\" | \"b\" ;\n",
        );
        assert_eq!(prods.len(), 2);
        assert_eq!(prods[0].name, "expr");
        assert_eq!(prods[1].name, "term");
    }

    #[test]
    fn skips_bad_production_but_keeps_good() {
        // First production is malformed (no operator); second is fine.
        let body = strip_header("railroad-ebnf\nbad bad bad ;\ngood = \"x\" ;\n").unwrap();
        let prods = parse_productions(&body).expect("at least one parses");
        assert_eq!(prods.len(), 1);
        assert_eq!(prods[0].name, "good");
    }

    #[test]
    fn header_error_and_empty() {
        // Bad header → Parse error.
        match render_railroad("graph TD\nA-->B\n", &MermaidOptions::default()) {
            Err(MermaidError::Parse(_)) => {}
            other => panic!("expected Parse, got {other:?}"),
        }
        // No productions at all → Parse error (nothing parsed).
        match render_railroad("railroad-ebnf\n", &MermaidOptions::default()) {
            Err(MermaidError::Parse(_)) => {}
            other => panic!("expected Parse (no productions), got {other:?}"),
        }
    }

    // ---- measure ---------------------------------------------------------

    #[test]
    fn seq_wider_than_widest_child() {
        let m = Metrics::new(16.0);
        let a = Node::NonTerminal("alpha".to_string());
        let b = Node::NonTerminal("b".to_string());
        let seq = Node::Seq(vec![a.clone(), b.clone()]);
        let sw = measure(&seq, &m).w;
        let aw = measure(&a, &m).w;
        let bw = measure(&b, &m).w;
        assert!(sw > aw.max(bw), "seq {sw} should exceed widest child {}", aw.max(bw));
    }

    #[test]
    fn choice_taller_than_single_child() {
        let m = Metrics::new(16.0);
        let a = Node::NonTerminal("a".to_string());
        let b = Node::NonTerminal("b".to_string());
        let single = measure(&a, &m);
        let choice = measure(&Node::Choice(vec![a, b]), &m);
        let single_h = single.up + single.down;
        let choice_h = choice.up + choice.down;
        assert!(choice_h > single_h, "choice {choice_h} should exceed single {single_h}");
    }

    // ---- render ----------------------------------------------------------

    fn render(src: &str) -> MermaidRender {
        render_railroad(src, &MermaidOptions::default()).expect("render")
    }

    #[test]
    fn render_well_formed_svg() {
        let r = render("railroad-ebnf\nexpr = term { \"+\" term } ;\nterm = \"a\" | \"b\" ;\n");
        assert!(r.svg.starts_with("<svg"));
        assert!(r.svg.trim_end().ends_with("</svg>"));
        assert!(r.svg.contains("viewBox="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn render_has_capsules_rects_and_names() {
        let r = render("railroad-ebnf\nexpr = term { \"+\" term } ;\nterm = \"a\" | \"b\" ;\n");
        // Production names present.
        assert!(r.svg.contains(">expr<"), "expr label");
        assert!(r.svg.contains(">term<"), "term label");
        // Terminals (capsules) and non-terminals (rects) both produce <rect>;
        // capsules have rx == box_half (large). At least one large-rx rect and
        // one small-rx rect should exist. We just assert rects + the terminal
        // texts are present.
        assert!(r.svg.matches("<rect").count() >= 4, "rects: {}", r.svg);
        assert!(r.svg.contains(">+<") || r.svg.contains(">&plus;<"));
        assert!(r.svg.contains(">a<"));
        assert!(r.svg.contains(">b<"));
    }

    #[test]
    fn choice_produces_multiple_branch_rails() {
        // A choice draws branch connectors as <path> elements; a pure sequence of
        // leaves draws none. So a choice production has more paths.
        let choice = render("railroad-ebnf\nt = \"a\" | \"b\" | \"c\" ;\n");
        let seq = render("railroad-ebnf\nt = \"a\" \"b\" \"c\" ;\n");
        let choice_paths = choice.svg.matches("<path").count();
        let seq_paths = seq.svg.matches("<path").count();
        assert!(choice_paths > seq_paths, "choice {choice_paths} vs seq {seq_paths}");
        assert!(choice_paths >= 2, "choice should fan out into branches");
    }

    #[test]
    fn repetition_produces_loop_back_path() {
        // A repeat draws curved loop-back corners (arcs) — at least one <path>
        // with an arc command.
        let r = render("railroad-ebnf\nx = { \"a\" } ;\n");
        assert!(r.svg.contains("<path"), "repeat should draw a loop-back path");
        assert!(r.svg.contains(" A "), "loop-back uses arc segments");
    }

    #[test]
    fn xml_is_escaped() {
        // A terminal containing markup must be escaped.
        let r = render("railroad-ebnf\nx = \"<a&b>\" ;\n");
        assert!(r.svg.contains("&lt;a&amp;b&gt;"));
        assert!(!r.svg.contains(">< a"));
    }

    #[test]
    fn deterministic_output() {
        let opts = MermaidOptions::default();
        let src = "railroad-ebnf\nexpr = term { \"+\" term } ;\nterm = \"a\" | \"b\" ;\n";
        let a = render_railroad(src, &opts).expect("a");
        let b = render_railroad(src, &opts).expect("b");
        assert_eq!(a.svg, b.svg);
        assert_eq!(a.width_px, b.width_px);
        assert_eq!(a.height_px, b.height_px);
    }
}
