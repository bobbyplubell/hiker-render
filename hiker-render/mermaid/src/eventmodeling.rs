//! `eventmodeling` diagram (self-contained: parse + swimlane layout + draw, no
//! graph/dagre layout).
//!
//! Mermaid header is `eventmodeling` (a bare keyword, optionally `eventmodeling:`).
//! Event modeling lays "frames" (cards) on horizontal **swimlanes** along a
//! left→right timeline. Each card has a *kind* (model-entity type) that fixes
//! both its lane and its color.
//!
//! ## Lanes (top → bottom), from the upstream `db.ts` `calculateSwimlaneProps`
//! - **UI/Automation** — kinds `ui`, `pcr`/`processor` (swimlane base index 0).
//! - **Command/Read Model** — kinds `cmd`/`command`, `rmo`/`readmodel`
//!   (swimlane base index 100).
//! - **Events** — kinds `evt`/`event` (swimlane base index 200).
//!
//! Cards are colored by kind, mirroring upstream's `calculateEntityVisualProps`:
//! ui = white/grey, processor = purple, read-model = green, command = blue,
//! event = orange.
//!
//! ## Syntax (from `event-modeling.langium`)
//! ```text
//! eventmodeling
//! tf 01 ui CartUI
//! tf 02 cmd AddItem
//! tf 03 evt ItemAdded
//! tf 04 rmo CartItems ->> 03
//! tf 05 evt AccountingItemAdded
//! ```
//! Each *frame* line is `('tf'|'timeframe'|'rf'|'resetframe') <id> <kind>
//! <entityIdentifier> ('->>' <sourceFrameId>)* ('[[' <dataRef> ']]')? <inline
//! data>?`. The `<id>` is a 1–3 digit number; `<entityIdentifier>` is a
//! (possibly dotted) name whose last segment is the card text. `->> NN`
//! declares a connection from an earlier frame `NN`; a `resetframe` (`rf`) draws
//! no incoming connector. `title <text>` is honored. Blank lines, `%%`
//! comments, and the `data`/`note`/`gwt`/`entity` block keywords are skipped
//! (their bodies are tolerated but not rendered).
//!
//! Layout/draw: horizontal lane bands (each labeled on the left, faintly tinted),
//! a left→right timeline; each frame becomes a colored rounded rect in its lane
//! at the next timeline column, with its text. Connections (`->>`, or the
//! implicit previous-frame link) are drawn as arrows between card centers.
//!
//! See `references/mermaid/packages/mermaid/src/diagrams/eventmodeling/` for the
//! upstream parser/renderer this mirrors.

use std::fmt::Write as _;

use crate::svgutil::{escape, rgb, text_size};
use crate::{MermaidError, MermaidOptions, MermaidRender};

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// A model-entity *kind*. Determines a card's lane and color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Ui,
    Processor,
    Command,
    ReadModel,
    Event,
}

impl Kind {
    /// Parse a model-entity-type keyword (the grammar's `EmModelEntityType`).
    fn parse(s: &str) -> Option<Kind> {
        match s {
            "ui" => Some(Kind::Ui),
            "pcr" | "processor" => Some(Kind::Processor),
            "cmd" | "command" => Some(Kind::Command),
            "rmo" | "readmodel" => Some(Kind::ReadModel),
            "evt" | "event" => Some(Kind::Event),
            _ => None,
        }
    }

    /// Index of this kind's lane within [`LANES`] (0 = top).
    fn lane(self) -> usize {
        match self {
            Kind::Ui | Kind::Processor => 0,
            Kind::Command | Kind::ReadModel => 1,
            Kind::Event => 2,
        }
    }

    /// Card fill/stroke, mirroring upstream `calculateEntityVisualProps`.
    fn colors(self) -> ([u8; 4], [u8; 4]) {
        match self {
            Kind::Ui => ([255, 255, 255, 255], [219, 218, 218, 255]),
            Kind::Processor => ([237, 179, 246, 255], [184, 140, 191, 255]),
            Kind::ReadModel => ([211, 241, 162, 255], [163, 183, 50, 255]),
            Kind::Command => ([188, 214, 254, 255], [103, 154, 195, 255]),
            Kind::Event => ([255, 183, 120, 255], [193, 154, 15, 255]),
        }
    }
}

/// The three swimlanes, top → bottom, with their default labels.
const LANES: [&str; 3] = ["UI/Automation", "Command/Read Model", "Events"];

/// A parsed frame (a card on the timeline).
#[derive(Clone, Debug, PartialEq)]
struct Frame {
    /// Frame id as written (e.g. `01`); used to resolve `->>` references.
    id: String,
    kind: Kind,
    /// Display text (last segment of the entity identifier).
    text: String,
    /// `true` for `rf`/`resetframe` (no implicit incoming connector).
    reset: bool,
    /// Explicit source frame ids from `->>` clauses.
    sources: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Model {
    title: Option<String>,
    frames: Vec<Frame>,
}

impl Model {
    fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Parse
// ---------------------------------------------------------------------------

/// Block keywords whose (possibly multi-line `{ … }`) bodies we skip.
fn is_block_keyword(w: &str) -> bool {
    matches!(w, "data" | "note" | "gwt" | "entity")
}

fn parse(src: &str) -> Result<Model, String> {
    let mut lines = src.lines();

    // Header: first non-blank, non-comment line must start with `eventmodeling`.
    let header = lines
        .by_ref()
        .map(|l| l.split("%%").next().unwrap_or("").trim())
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let head_kw = header.split_whitespace().next().unwrap_or("");
    if head_kw.trim_end_matches(':') != "eventmodeling" {
        return Err(format!("eventmodeling: expected header `eventmodeling`, got {header:?}"));
    }

    let mut model = Model::default();
    // Depth of an open `{ … }` data/note block we're skipping.
    let mut brace_depth: i32 = 0;

    for raw in lines {
        let line = raw.split("%%").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }

        // If we're inside a skipped brace block, just track nesting and continue.
        if brace_depth > 0 {
            brace_depth += brace_delta(line);
            continue;
        }

        let mut words = line.split_whitespace();
        let first = words.next().unwrap_or("");

        // `title <text>`.
        if first == "title" {
            let t = line["title".len()..].trim();
            if !t.is_empty() {
                model.title = Some(t.to_string());
            }
            continue;
        }

        // Block keywords (data/note/gwt/entity): skip the statement and any
        // `{ … }` body that may follow on this or subsequent lines.
        if is_block_keyword(first) {
            brace_depth += brace_delta(line);
            continue;
        }

        // Frame line: `tf|timeframe|rf|resetframe <id> <kind> <ident> ...`.
        let reset = match first {
            "tf" | "timeframe" => false,
            "rf" | "resetframe" => true,
            _ => continue, // unknown line — tolerate and skip.
        };

        let Some(id) = words.next() else { continue };
        let Some(kind_kw) = words.next() else { continue };
        let Some(kind) = Kind::parse(kind_kw) else { continue };
        let Some(ident) = words.next() else { continue };

        // Last dotted segment is the visible text.
        let text = ident.rsplit('.').next().unwrap_or(ident).to_string();

        // Collect `->> NN` source references (each `->>` is followed by an id).
        let mut sources = Vec::new();
        let mut rest = words.peekable();
        while let Some(tok) = rest.next() {
            if tok == "->>" {
                if let Some(sid) = rest.peek() {
                    // Only treat it as a frame ref if it looks like an id.
                    if sid.chars().all(|c| c.is_ascii_digit()) && !sid.is_empty() {
                        sources.push((*sid).to_string());
                        rest.next();
                    }
                }
            }
            // Everything else (data refs `[[...]]`, inline data) is ignored.
        }

        model.frames.push(Frame {
            id: id.to_string(),
            kind,
            text,
            reset,
            sources,
        });
    }

    Ok(model)
}

/// Net change in brace nesting for a line (counts `{` minus `}`), used to skip
/// data/note block bodies.
fn brace_delta(line: &str) -> i32 {
    let mut d = 0;
    for c in line.chars() {
        match c {
            '{' => d += 1,
            '}' => d -= 1,
            _ => {}
        }
    }
    d
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// A positioned card.
struct Card {
    kind: Kind,
    text: String,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

/// A drawn connector between two card centers (with an arrowhead at the target).
struct Conn {
    sx: f32,
    sy: f32,
    tx: f32,
    ty: f32,
}

struct Layout {
    width: f32,
    height: f32,
    title_h: f32,
    /// Per-lane band rectangles: (y, height).
    bands: [(f32, f32); 3],
    cards: Vec<Card>,
    conns: Vec<Conn>,
}

// Geometry constants (loosely mirror upstream `diagramProps`, scaled down).
const LABEL_W: f32 = 150.0; // left gutter for lane labels
const LANE_H: f32 = 96.0; // band height
const LANE_GAP: f32 = 8.0;
const CARD_W: f32 = 130.0;
const COL_GAP: f32 = 28.0; // horizontal gap between successive columns
const PAD: f32 = 16.0;
const CONTENT_X0: f32 = LABEL_W + PAD;

fn layout(model: &Model, opts: &MermaidOptions) -> Layout {
    let fs = opts.font_size_px;
    let title_h = if model.title.is_some() { fs * 1.6 + 8.0 } else { 0.0 };
    let top = PAD + title_h;

    // Lane bands.
    let mut bands = [(0.0f32, 0.0f32); 3];
    for i in 0..3 {
        bands[i] = (top + i as f32 * (LANE_H + LANE_GAP), LANE_H);
    }

    // Place each frame in its lane at the next timeline column. Columns advance
    // left→right by frame order; each frame gets its own column.
    let mut cards = Vec::with_capacity(model.frames.len());
    // Map frame id -> card index (last writer wins, as upstream resolves by name).
    let mut by_id: Vec<(String, usize)> = Vec::new();
    let mut x = CONTENT_X0;

    for f in &model.frames {
        let (tw, th) = text_size(&f.text, fs);
        let w = (tw + 24.0).max(CARD_W);
        let h = (th + 24.0).min(LANE_H - 16.0).max(40.0);
        let (band_y, band_h) = bands[f.kind.lane()];
        let y = band_y + (band_h - h) / 2.0;

        let idx = cards.len();
        cards.push(Card { kind: f.kind, text: f.text.clone(), x, y, w, h });
        by_id.push((f.id.clone(), idx));

        x += w + COL_GAP;
    }

    let find = |id: &str| -> Option<usize> {
        by_id.iter().rev().find(|(fid, _)| fid == id).map(|(_, i)| *i)
    };

    // Connectors: explicit `->>` sources, else the implicit previous-frame link
    // (skipped for the first frame and for reset frames).
    let mut conns = Vec::new();
    for (i, f) in model.frames.iter().enumerate() {
        let mut srcs: Vec<usize> = Vec::new();
        if !f.sources.is_empty() {
            for s in &f.sources {
                if let Some(si) = find(s) {
                    if si != i {
                        srcs.push(si);
                    }
                }
            }
        } else if !f.reset && i > 0 {
            srcs.push(i - 1);
        }
        for si in srcs {
            let s = &cards[si];
            let t = &cards[i];
            conns.push(Conn {
                sx: s.x + s.w / 2.0,
                sy: s.y + s.h / 2.0,
                tx: t.x + t.w / 2.0,
                ty: t.y + t.h / 2.0,
            });
        }
    }

    let content_right = cards.iter().map(|c| c.x + c.w).fold(CONTENT_X0, f32::max);
    let width = content_right + PAD;
    let height = bands[2].0 + bands[2].1 + PAD;

    Layout { width, height, title_h, bands, cards, conns }
}

// ---------------------------------------------------------------------------
// Draw
// ---------------------------------------------------------------------------

/// Render a mermaid `eventmodeling` diagram to SVG.
pub fn render_eventmodeling(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let model = parse(src).map_err(MermaidError::Parse)?;
    if model.is_empty() {
        return Err(MermaidError::Empty);
    }

    let lo = layout(&model, opts);
    let fs = opts.font_size_px;
    let width = lo.width;
    let height = lo.height;

    let stroke = rgb(opts.node_stroke);
    let edge = rgb(opts.edge_stroke);
    let text = rgb(opts.text_color);

    let mut s = String::with_capacity(2048 + model.frames.len() * 256);
    let _ = write!(
        s,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width:.0}\" \
         height=\"{height:.0}\" viewBox=\"0 0 {width:.0} {height:.0}\">",
    );

    // Arrowhead marker for connectors.
    let _ = write!(
        s,
        "<defs><marker id=\"em-arrow\" viewBox=\"0 0 10 10\" refX=\"9\" refY=\"5\" \
         markerWidth=\"7\" markerHeight=\"7\" orient=\"auto-start-reverse\">\
         <path d=\"M0,0 L10,5 L0,10 z\" fill=\"{edge}\"/></marker></defs>",
    );

    // Title.
    if let Some(t) = &model.title {
        let _ = write!(
            s,
            "<text x=\"{:.1}\" y=\"{:.1}\" font-family=\"{ff}\" font-size=\"{:.1}\" \
             font-weight=\"bold\" text-anchor=\"middle\" dominant-baseline=\"middle\" \
             fill=\"{text}\">{}</text>",
            width / 2.0,
            PAD + lo.title_h / 2.0,
            fs * 1.2,
            escape(t),
            ff = escape(&opts.font_family),
        );
    }

    // Lane bands: faint background + left label.
    for (i, lane) in LANES.iter().enumerate() {
        let (y, h) = lo.bands[i];
        let _ = write!(
            s,
            "<rect x=\"0\" y=\"{y:.1}\" width=\"{width:.1}\" height=\"{h:.1}\" \
             fill=\"{stroke}\" fill-opacity=\"0.06\" stroke=\"{stroke}\" \
             stroke-opacity=\"0.35\" stroke-width=\"1\"/>",
        );
        // Vertical divider after the label gutter.
        let _ = write!(
            s,
            "<line x1=\"{LABEL_W:.1}\" y1=\"{y:.1}\" x2=\"{LABEL_W:.1}\" y2=\"{:.1}\" \
             stroke=\"{stroke}\" stroke-opacity=\"0.35\" stroke-width=\"1\"/>",
            y + h,
        );
        let _ = write!(
            s,
            "<text x=\"{:.1}\" y=\"{:.1}\" font-family=\"{ff}\" font-size=\"{:.1}\" \
             font-weight=\"bold\" text-anchor=\"middle\" dominant-baseline=\"middle\" \
             fill=\"{text}\">{}</text>",
            LABEL_W / 2.0,
            y + h / 2.0,
            fs,
            escape(lane),
            ff = escape(&opts.font_family),
        );
    }

    // Connectors (drawn before cards so cards sit on top).
    for c in &lo.conns {
        let _ = write!(
            s,
            "<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" \
             stroke=\"{edge}\" stroke-width=\"1.5\" marker-end=\"url(#em-arrow)\"/>",
            c.sx, c.sy, c.tx, c.ty,
        );
    }

    // Cards.
    for card in &lo.cards {
        let (fill, cstroke) = card.kind.colors();
        let _ = write!(
            s,
            "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" rx=\"6\" ry=\"6\" \
             fill=\"{}\" stroke=\"{}\" stroke-width=\"1.5\"/>",
            card.x,
            card.y,
            card.w,
            card.h,
            rgb(fill),
            rgb(cstroke),
        );
        let _ = write!(
            s,
            "<text x=\"{:.1}\" y=\"{:.1}\" font-family=\"{ff}\" font-size=\"{:.1}\" \
             font-weight=\"bold\" text-anchor=\"middle\" dominant-baseline=\"middle\" \
             fill=\"{text}\">{}</text>",
            card.x + card.w / 2.0,
            card.y + card.h / 2.0,
            fs,
            escape(&card.text),
            ff = escape(&opts.font_family),
        );
    }

    s.push_str("</svg>");

    Ok(MermaidRender { svg: s, width_px: width, height_px: height })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "eventmodeling\n\
        tf 01 ui CartUI\n\
        tf 02 cmd AddItem\n\
        tf 03 evt ItemAdded\n\
        tf 04 rmo CartItems ->> 03\n\
        tf 05 evt AccountingItemAdded\n";

    fn opts() -> MermaidOptions {
        MermaidOptions::default()
    }

    #[test]
    fn parses_kinds_lanes_text_order() {
        let m = parse(SAMPLE).unwrap();
        assert_eq!(m.frames.len(), 5);

        // Order preserved (left→right by appearance).
        let ids: Vec<&str> = m.frames.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["01", "02", "03", "04", "05"]);

        // Kinds + lane assignment.
        assert_eq!(m.frames[0].kind, Kind::Ui);
        assert_eq!(m.frames[0].kind.lane(), 0);
        assert_eq!(m.frames[0].text, "CartUI");

        assert_eq!(m.frames[1].kind, Kind::Command);
        assert_eq!(m.frames[1].kind.lane(), 1);

        assert_eq!(m.frames[2].kind, Kind::Event);
        assert_eq!(m.frames[2].kind.lane(), 2);

        assert_eq!(m.frames[3].kind, Kind::ReadModel);
        assert_eq!(m.frames[3].kind.lane(), 1);
        assert_eq!(m.frames[3].text, "CartItems");
        assert_eq!(m.frames[3].sources, vec!["03".to_string()]);

        assert_eq!(m.frames[4].kind, Kind::Event);
    }

    #[test]
    fn parses_aliases_qualified_names_and_reset() {
        let src = "eventmodeling\n\
            timeframe 01 event Start\n\
            tf 02 command DoThing\n\
            rf 03 readmodel Cart.ItemAdded ->> 01 ->> 02\n";
        let m = parse(src).unwrap();
        assert_eq!(m.frames.len(), 3);
        assert_eq!(m.frames[0].kind, Kind::Event);
        assert_eq!(m.frames[1].kind, Kind::Command);
        assert_eq!(m.frames[2].kind, Kind::ReadModel);
        // Qualified name → last segment is the text.
        assert_eq!(m.frames[2].text, "ItemAdded");
        assert!(m.frames[2].reset);
        assert_eq!(m.frames[2].sources, vec!["01".to_string(), "02".to_string()]);
    }

    #[test]
    fn parses_title_and_skips_block_bodies() {
        let src = "eventmodeling\n\
            title My Model\n\
            tf 01 evt Start\n\
            data Foo {\n  a: b\n}\n\
            note 01 {\n  hello { nested }\n}\n\
            tf 02 cmd DoIt\n";
        let m = parse(src).unwrap();
        assert_eq!(m.title.as_deref(), Some("My Model"));
        // Block bodies skipped; both frames parsed.
        assert_eq!(m.frames.len(), 2);
        assert_eq!(m.frames[0].text, "Start");
        assert_eq!(m.frames[1].text, "DoIt");
    }

    #[test]
    fn bad_header_errors() {
        let e = parse("flowchart TD\nA-->B\n").unwrap_err();
        assert!(matches!(render_eventmodeling("flowchart TD", &opts()), Err(MermaidError::Parse(_))));
        let _ = e;
    }

    #[test]
    fn empty_is_empty_error() {
        let r = render_eventmodeling("eventmodeling\n", &opts());
        assert_eq!(r, Err(MermaidError::Empty));
    }

    #[test]
    fn renders_wellformed_svg_with_lanes_and_cards() {
        let r = render_eventmodeling(SAMPLE, &opts()).unwrap();
        let svg = &r.svg;
        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(svg.contains("viewBox="));
        assert!(svg.trim_end().ends_with("</svg>"));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);

        // All three lane labels present.
        for lane in LANES {
            assert!(svg.contains(lane), "missing lane label {lane}");
        }

        // One card rect per frame: count rounded rects (rx="6").
        let card_rects = svg.matches("rx=\"6\"").count();
        assert_eq!(card_rects, 5, "expected 5 card rects");

        // Card text present.
        assert!(svg.contains(">CartUI<"));
        assert!(svg.contains(">AddItem<"));
        assert!(svg.contains(">ItemAdded<"));
    }

    #[test]
    fn cards_colored_by_kind_in_right_lane() {
        let r = render_eventmodeling(SAMPLE, &opts()).unwrap();
        let svg = &r.svg;
        // Event fill orange, command fill blue, ui fill white, read-model green.
        let (evt_fill, _) = Kind::Event.colors();
        let (cmd_fill, _) = Kind::Command.colors();
        let (rmo_fill, _) = Kind::ReadModel.colors();
        assert!(svg.contains(&rgb(evt_fill)), "event color missing");
        assert!(svg.contains(&rgb(cmd_fill)), "command color missing");
        assert!(svg.contains(&rgb(rmo_fill)), "read-model color missing");

        // Verify lane placement via layout: ui card y inside band 0, event in band 2.
        let m = parse(SAMPLE).unwrap();
        let lo = layout(&m, &opts());
        // ui card (index 0) center within band 0.
        let ui = &lo.cards[0];
        let (b0y, b0h) = lo.bands[0];
        let uic = ui.y + ui.h / 2.0;
        assert!(uic >= b0y && uic <= b0y + b0h, "ui card not in lane 0");
        // event card (index 2) center within band 2.
        let ev = &lo.cards[2];
        let (b2y, b2h) = lo.bands[2];
        let evc = ev.y + ev.h / 2.0;
        assert!(evc >= b2y && evc <= b2y + b2h, "event card not in lane 2");
    }

    #[test]
    fn connectors_drawn() {
        let m = parse(SAMPLE).unwrap();
        let lo = layout(&m, &opts());
        // 4 implicit prev-links would be there, but frame 04 has an explicit
        // `->> 03`, so: 02->01, 03->02, 04->03(explicit), 05->04 = 4 conns.
        assert_eq!(lo.conns.len(), 4);
        let r = render_eventmodeling(SAMPLE, &opts()).unwrap();
        assert!(r.svg.contains("marker-end=\"url(#em-arrow)\""));
    }

    #[test]
    fn xml_escaped() {
        let src = "eventmodeling\ntf 01 evt A<&>B\n";
        let r = render_eventmodeling(src, &opts()).unwrap();
        assert!(r.svg.contains("A&lt;&amp;&gt;B"));
        assert!(!r.svg.contains(">A<&>B<"));
    }

    #[test]
    fn deterministic() {
        let a = render_eventmodeling(SAMPLE, &opts()).unwrap();
        let b = render_eventmodeling(SAMPLE, &opts()).unwrap();
        assert_eq!(a, b);
    }
}
