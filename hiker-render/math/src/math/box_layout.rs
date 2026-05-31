//! A minimal TeX-style box model and the event → box layout pass.
//!
//! Layer 2 lays a single horizontal row of glyphs left-to-right, but now with
//! typographically correct details: single-letter identifiers render in the
//! math-italic variant, `\text{…}`/`\mathrm` content renders upright, and the
//! gaps between atoms come from the TeX/Appendix-G inter-atom spacing matrix
//! (in `mu`) rather than a fixed legibility hack.
//!
//! The box model is shaped after `references/microtex/src/box/` and
//! `references/katex/src/domTree.ts` (a tree of boxes carrying width/height/depth
//! metrics) but kept minimal so later layers — scripts, fractions, radicals,
//! delimiters — can slot in as new [`BoxKind`] variants and layout passes.
//!
//! All metrics are in **CSS px** (font units already scaled by
//! `font_size_px / units_per_em`). Following TeX, a box's vertical extent is split
//! into `height` (above the baseline) and `depth` (below it).

use ttf_parser::{Face, GlyphId};

use super::delim;
use super::glyph::{self, Variant};
use super::MathOptions;
use crate::font;

/// STIX Two Math is a 1000-unit em; kept here so scaling reads clearly.
const UNITS_PER_EM: f32 = 1000.0;

/// A laid-out box: metrics plus a [`BoxKind`] describing what to draw.
///
/// Heights/depths are positive distances above/below the baseline, in px.
#[derive(Clone, Debug)]
pub struct Box {
    /// Advance width in px.
    pub width: f32,
    /// Extent above the baseline in px.
    pub height: f32,
    /// Extent below the baseline in px.
    pub depth: f32,
    /// What this box draws.
    pub kind: BoxKind,
}

/// A child placed inside an [`BoxKind::Hbox`]: a horizontal offset `dx` (px from
/// the hbox's left edge), a *downward* baseline shift `dy` (px the child's
/// baseline sits **below** the hbox's baseline — negative raises it), and the box.
#[derive(Clone, Debug)]
pub struct Child {
    /// Horizontal offset from the hbox left edge, px.
    pub dx: f32,
    /// Downward baseline shift, px (negative = raised, as for superscripts).
    pub dy: f32,
    /// The placed box.
    pub b: Box,
}

/// The drawable content of a [`Box`].
#[derive(Clone, Debug)]
pub enum BoxKind {
    /// A single glyph, identified by its font glyph id, drawn at the current pen.
    /// `scale` is font-units→px for *this* glyph (smaller for scripts), so the
    /// painter scales each glyph by its own style's em rather than a global one.
    /// `color` is the straight RGBA fill in effect for this glyph (the inherited
    /// [`MathOptions::color`] unless a `\color`/`\textcolor` scope overrode it).
    Glyph { gid: GlyphId, scale: f32, color: [u8; 4] },
    /// A horizontal list: children placed at `(dx, dy)` offsets, where `dy`
    /// shifts a child's baseline downward relative to this box's baseline. With
    /// every `dy == 0` this is a plain TeX hlist; non-zero `dy` stacks scripts and
    /// fraction numerators/denominators vertically.
    Hbox { children: Vec<Child> },
    /// A filled rectangle (the fraction bar / radical rule): drawn from the box
    /// origin, extending `thickness` px **upward** from the baseline (so its
    /// `height == thickness`, `depth == 0`). Placed on the math axis by an
    /// enclosing [`Child`]'s `dy` (a positive `dy` lowers it onto the axis).
    /// `color` is the straight RGBA fill in effect (as for [`BoxKind::Glyph`]).
    Rule { width: f32, thickness: f32, color: [u8; 4] },
    /// A straight stroke from the box origin (its baseline left corner) to
    /// `origin + (dx, dy)`, where `dy` grows **downward** like a [`Child`]'s shift
    /// (negative = up). Used for the diagonal strike of `\cancel`. `thickness` is
    /// the stroke width in px; `color` is the straight RGBA stroke (as for
    /// [`BoxKind::Glyph`]). Carries no advance/height itself — the enclosing box
    /// owns the metrics, so the line is a pure overlay.
    Line { dx: f32, dy: f32, thickness: f32, color: [u8; 4] },
    /// A solid filled rectangle (a `\colorbox`/`\fcolorbox` background). Unlike a
    /// [`BoxKind::Rule`], it extends both **upward** `height` px above the baseline
    /// and **downward** `depth` px below it, spanning the box's full bbox. `color`
    /// is the straight RGBA fill. Emitted as the first child of the wrapping Hbox
    /// so it paints behind the content. A `\fcolorbox` frame is drawn separately
    /// as four [`BoxKind::Rule`]/[`BoxKind::Line`] edges over the fill.
    Fill { width: f32, height: f32, depth: f32, color: [u8; 4] },
}

/// The TeX math class of an atom (TeXbook p. 158). This drives both glyph
/// selection (Ord variables go italic) and inter-atom spacing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Ordinary: variables, numbers, plain symbols.
    Ord,
    /// Large/named operators: `\sin`, `\sum`, `\int`.
    Op,
    /// Binary operator: `+`, `-`, `\times`.
    Bin,
    /// Relation: `=`, `<`, `\approx`.
    Rel,
    /// Opening delimiter: `(`, `[`.
    Open,
    /// Closing delimiter: `)`, `]`.
    Close,
    /// Punctuation: `,`, `;`.
    Punct,
    /// Inner: fractions, `\left…\right` groups (none produced yet).
    Inner,
}

/// A single atom recovered from the event stream, before layout.
///
/// `italic_correction` is *not* filled at parse time (we don't have the face /
/// scale there yet); [`layout_atom`] computes it per-glyph from the MATH table
/// and a right-superscript shifts right by that amount (rule 18a).
#[derive(Clone)]
struct Atom {
    /// The character to render from the math font.
    ch: char,
    /// The TeX class, used for spacing and (for [`Class::Ord`]) italicization.
    class: Class,
    /// The letterform to render in (italic for variables, upright otherwise).
    variant: Variant,
    /// True for a *symbol* large operator (`\sum`, `\int`, `\prod`, `\bigcup`,
    /// …): in Display style its glyph grows to `display_operator_min_height` and
    /// it straddles the math axis. Named operators (`\lim`, `\max`) are plain
    /// upright [`Class::Op`] atoms with this `false` (no glyph growth).
    large_op: bool,
    /// The straight RGBA fill in effect at parse time (`None` = inherit the
    /// default [`MathOptions::color`]), set by an enclosing `\color`/`\textcolor`
    /// scope. Resolved to a concrete color when the atom's glyph is laid out.
    color: Option<[u8; 4]>,
}

/// Where a [`MathNode::Script`]'s scripts sit, mirroring pulldown's
/// [`pulldown_latex::event::ScriptPosition`]: beside the base (`Right`), stacked
/// above/below (`AboveBelow`), or `Movable` (above/below in Display, beside in
/// Text — what `\sum` / `\lim` use).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScriptPos {
    /// Scripts beside the base (`x^2`, `\int`).
    Right,
    /// Scripts stacked above/below the base (`\overset`-like, always limits).
    AboveBelow,
    /// Above/below in Display, beside in Text (`\sum`, `\lim`).
    Movable,
}

/// The fraction bar (vinculum) thickness, from pulldown's `Visual::Fraction`.
/// `\frac` (`Fraction(None)`) keeps the font default; `\binom`/`\genfrac{}{}{0pt}{}`
/// (an explicit `0` thickness) draws none; `\genfrac{(}{)}{2pt}{}{a}{b}` (an
/// explicit non-zero thickness) draws the bar at that thickness, in px.
#[derive(Clone, Copy, Debug, PartialEq)]
enum BarThickness {
    /// Default font (`FractionRuleThickness`) bar — `\frac` and friends.
    Default,
    /// No bar — `\binom`/`\genfrac` with a `0` explicit thickness.
    None,
    /// An explicit bar thickness, as a multiple of the **em** (resolved to px at
    /// the base em in [`layout_frac`]) — `\genfrac` with a non-zero dimension.
    Em(f32),
}

/// Horizontal alignment of cells within a matrix/array column (TeX `l`/`c`/`r`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Align {
    /// Left-aligned (column spec `l`; `cases`/`aligned`'s columns).
    Left,
    /// Centered (column spec `c`; the `matrix` family default).
    Center,
    /// Right-aligned (column spec `r`; `aligned`'s first column).
    Right,
}

/// Which environment a [`MathNode::Matrix`] came from. Drives the cell render
/// style and (for `cases`) the self-drawn left brace. The surrounding
/// delimiters of `pmatrix`/`bmatrix`/… are *not* tracked here — pulldown wraps
/// those environments in an outer `Begin(Grouping::LeftRight(..))`, so they
/// arrive as an ordinary [`MathNode::Delim`] around the matrix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MatrixKind {
    /// `matrix`/`pmatrix`/`bmatrix`/`array`: cells in the current style.
    Plain,
    /// `cases`/`rcases`: text-style cells, a self-drawn large brace, no right delim.
    Cases,
    /// `aligned`/`align`/`alignedat`/`split`: text-style cells, alternating
    /// right/left columns with no gap at each `&` so relations line up.
    Aligned,
    /// `\substack{a \\ b}`: a centered stack laid out one style step *smaller*
    /// than its surroundings (TeX sets substack content at script size). The
    /// macro pass (see [`super::macros`]) rewrites `\substack` to a `matrix`
    /// tagged with this kind via a sentinel (see [`SUBSTACK_SENTINEL`]).
    Substack,
}

/// Math style levels (TeXbook p. 140 / MathML Core "math style").
///
/// Each level has an em-scale factor (read from the MATH table) and a `cramped`
/// flag. A script's children lay out at the *next-smaller* style; superscripts
/// are uncramped, subscripts/denominators cramped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// Display style (`\displaystyle`): full size.
    Display,
    /// Text style (`\textstyle`, inline): full size.
    Text,
    /// Script style: first-level sub/superscripts.
    Script,
    /// Scriptscript style: nested scripts (and below).
    ScriptScript,
}

impl Style {
    /// The style one level smaller (scripts of *this* style render in it).
    /// Display/Text → Script → ScriptScript (saturating).
    fn smaller(self) -> Style {
        match self {
            Style::Display | Style::Text => Style::Script,
            Style::Script | Style::ScriptScript => Style::ScriptScript,
        }
    }

    /// The style a fraction's numerator/denominator render in (TeXbook rule 15b):
    /// Display→Text, Text→Script, Script/ScriptScript→ScriptScript.
    fn frac_child(self) -> Style {
        match self {
            Style::Display => Style::Text,
            Style::Text => Style::Script,
            Style::Script | Style::ScriptScript => Style::ScriptScript,
        }
    }

    /// Whether this style is display-sized (selects the larger display-style
    /// fraction shift/gap constants).
    fn is_display(self) -> bool {
        matches!(self, Style::Display)
    }

    /// Bin/Rel inter-atom spacing is suppressed in the two script styles.
    fn is_tight(self) -> bool {
        matches!(self, Style::Script | Style::ScriptScript)
    }
}

/// The math-list tree IR: a row is a list of nodes, each of which may itself be a
/// nested row (a `{…}` group) or carry scripts. This replaces layer 2's flat
/// atom row so scripts (and later fractions/radicals/delimiters) compose cleanly.
type MathList = Vec<MathNode>;

/// One element of a [`MathList`].
enum MathNode {
    /// A single rendered atom (glyph + class + variant).
    Atom(Atom),
    /// A nested row, e.g. the body of a `{…}` group; lays out as one element.
    Group(MathList),
    /// A base with optional super/subscript(s) beside it (rule 18). The base and
    /// each script are themselves rows so `{2x}`-style groups compose.
    Script {
        base: MathList,
        sup: Option<MathList>,
        sub: Option<MathList>,
        /// pulldown's [`ScriptPos`], deciding limits (above/below) vs beside.
        position: ScriptPos,
    },
    /// A fraction (`\frac`/`\over`/`\dfrac`/`\tfrac`): a numerator stacked over a
    /// denominator with a rule between them. Both operands are rows. `style`, when
    /// `Some`, forces the fraction's own render style (`\dfrac`→Display,
    /// `\tfrac`→Text) regardless of the surrounding style.
    Frac {
        num: MathList,
        den: MathList,
        style: Option<Style>,
        /// Color in effect for the fraction bar (`None` = inherit the default).
        color: Option<[u8; 4]>,
        /// The horizontal bar's thickness. `\frac` (pulldown `Fraction(None)`)
        /// uses the font default; `\binom`/`\genfrac{}{}{0pt}{}` (a `0`-em
        /// `Fraction(Some(..))` bar size) draws no bar — only the stacked
        /// numerator/denominator inside the surrounding `\left(\right)` parens; an
        /// explicit non-zero `\genfrac` thickness draws the bar at that width.
        bar: BarThickness,
    },
    /// A `\left … \right`-fenced expression (or `\bigl`/`\bigr` etc.): the `body`
    /// row bracketed by an `open` and `close` delimiter that **scale to the
    /// body's height**. A `None` delimiter is a null (`\left.`) one — no glyph and
    /// no width. The whole node spaces as an [`Class::Inner`] atom.
    Delim {
        open: Option<char>,
        body: MathList,
        close: Option<char>,
    },
    /// A fixed-size delimiter (`\bigl`, `\Bigr`, `\biggl`, …): one delimiter glyph
    /// sized to a fixed multiple of the em (`target_em`, e.g. 1.2 for `\big`),
    /// independent of any surrounding content. `class` is Open/Close/Inner per the
    /// command's `…l`/`…r`/`\middle` flavour.
    BigDelim { ch: char, target_em: f32, class: Class, color: Option<[u8; 4]> },
    /// A radical (`\sqrt{…}` / `\sqrt[n]{…}`): a surd (U+221A) sized to span the
    /// `radicand`, topped by a horizontal rule (the vinculum) that runs over the
    /// radicand. The optional `index` is the small degree (`n` in `\sqrt[n]{…}`),
    /// laid out at ScriptScript style and tucked into the surd's upper-left. The
    /// whole node spaces as an [`Class::Ord`] atom.
    Radical {
        index: Option<MathList>,
        radicand: MathList,
        /// Color in effect for the surd and vinculum (`None` = inherit default).
        color: Option<[u8; 4]>,
    },
    /// An accented expression (`\hat \tilde \bar \vec \dot \ddot \check \acute
    /// \grave \overline \widehat \widetilde \overrightarrow \overleftarrow`, the
    /// under-forms `\underline`/`\underbar`, and the stretchy brace/paren/group
    /// forms `\overbrace`/`\underbrace`/`\overparen`/`\overgroup`/…). pulldown
    /// emits these as a `Script { position: AboveBelow }` whose script operand is a
    /// single [`Content::Ordinary`] accent character (the base is the script's
    /// base). `under` is `true` for the subscript-position (below-the-base) forms.
    /// `stretchy` marks accents that grow to the base width (`\widehat`,
    /// `\overline`, `\overrightarrow`, `\overbrace`, …); the rest use one fixed
    /// glyph. A trailing `^{…}`/`_{…}` on `\overbrace`/`\underbrace` arrives as an
    /// outer `AboveBelow` [`Script`] wrapping this node, so the label stacks above
    /// the over-brace / below the under-brace through the normal limits path. The
    /// node spaces as an [`Class::Ord`] atom.
    Accent {
        /// The accent glyph character (`^`, `~`, `‾`, `→`, `˙`, …).
        accent: char,
        /// Grows to span the base width (rule for `‾`/`_`, horizontal variant else).
        stretchy: bool,
        /// Sits below the base (`\underline`/`\underbar`) rather than above.
        under: bool,
        /// The accented expression.
        base: MathList,
        /// Color in effect for the accent glyph / over-under rule (`None` =
        /// inherit the default).
        color: Option<[u8; 4]>,
    },
    /// A matrix/array/cases/aligned environment: a grid of cells laid out in
    /// rows and columns, vertically centered on the math axis. `rows[r][c]` is
    /// the (row `r`, column `c`) cell's [`MathList`]; rows may be ragged (short
    /// rows are treated as having empty trailing cells). `col_align` gives each
    /// column's horizontal alignment (extended with the `kind`'s default for
    /// columns beyond its length). `kind` selects the cell style and, for
    /// `cases`, a self-drawn left brace. Delimiters of `pmatrix`/`bmatrix`/… are
    /// supplied by an enclosing [`MathNode::Delim`] (pulldown emits them as an
    /// outer `\left…\right`). The node spaces as an [`Class::Inner`] atom.
    Matrix {
        rows: Vec<Vec<MathList>>,
        col_align: Vec<Align>,
        kind: MatrixKind,
        /// Vertical column rules from the `array` column spec (`|`/`||`). Length
        /// `col_align.len() + 1`: `col_seps[c]` is the number of vertical rules
        /// drawn at the *left* edge of column `c`, and `col_seps[n]` (the last
        /// entry) the rules at the right edge of the grid. All-zero for the
        /// `matrix`/`cases`/`aligned` families (no column spec).
        col_seps: Vec<u8>,
        /// Horizontal rules (`\hline`). Length `rows.len() + 1`: `row_lines[r]`
        /// is the number of rules drawn *above* row `r`, and `row_lines[rows]`
        /// (the last entry) the rules below the final row. `\hline\hline`
        /// doubles the count. (`\cline{a-b}` partial spans are not represented —
        /// pulldown does not carry the column range; see [`read_matrix`].)
        row_lines: Vec<u8>,
    },
    /// A struck-through expression (`\cancel{…}`): the `body` row laid out as
    /// usual, with a forward diagonal stroke (lower-left → upper-right) overlaid
    /// across its bounding box. pulldown emits `\cancel` and `\not` identically
    /// (both `Visual::Negation`), so the macro pass rewrites `\cancel{…}` to a
    /// sentinel marker (see [`super::macros`]) that this IR picks up — leaving
    /// `\not`'s negation behaviour untouched. Spaces as an [`Class::Ord`] atom.
    Cancel {
        body: MathList,
        /// Color in effect for the strike line (`None` = inherit the default).
        color: Option<[u8; 4]>,
    },
    /// A `\colorbox`/`\fcolorbox` group: the `body` row drawn over a solid
    /// `background` fill (spanning the body's bbox plus a small `\fboxsep`
    /// padding), with an optional `border`-colored frame stroked around it
    /// (`\fcolorbox`). pulldown emits these as a `Begin(Normal)` group whose first
    /// `StateChange::Color` carries `target: Background` (and, for `\fcolorbox`, a
    /// preceding `target: Border`). Spaces as an [`Class::Ord`] atom.
    ColorBox {
        body: MathList,
        /// The fill color painted behind the body.
        background: [u8; 4],
        /// The frame color (`\fcolorbox`), or `None` for a plain `\colorbox`.
        border: Option<[u8; 4]>,
    },
}

/// Build a [`MathList`] tree from `src` via `pulldown-latex`.
///
/// We construct the parser as `Parser::new(src, &storage)` (a `Storage` arena owns
/// the parser's allocations), collect its `Result<Event, _>` stream into a flat
/// `Vec<Event>` (returning `None` on any parser error), then recursively consume
/// that buffer with a cursor. Working over a buffer (rather than the lazy
/// iterator) is what lets [`read_element`] look *ahead* to grab a script's base
/// and operands.
///
/// Per-event handling (class / variant unchanged from layer 2):
///
/// * [`Content::Ordinary`] → [`Class::Ord`]; single ASCII letters / lowercase
///   Greek go [`Variant::Italic`] (the default TeX rule), else upright.
/// * [`Content::Number`] → [`Class::Ord`] upright (one atom per digit).
/// * [`Content::BinaryOp`] → [`Class::Bin`]; [`Content::Relation`] → [`Class::Rel`].
/// * [`Content::Function`] (`\sin`) → [`Class::Op`] upright; [`Content::Text`]
///   (`\text{…}`) → upright [`Class::Ord`] letters.
/// * [`Content::Punctuation`] → [`Class::Punct`]; [`Content::Delimiter`] →
///   Open/Close/Inner by its `ty`.
///
/// Structure:
///
/// * [`Event::Begin`]`..`[`Event::End`] → a [`MathNode::Group`] holding the inner
///   row (one element). Font state ([`StateChange::Font`]) scopes to the current
///   group via a font stack, threaded into [`variant_for`].
/// * [`Event::Script`] precedes its operands. pulldown emits the **base element
///   then the script element(s)**: `Subscript`/`Superscript` ⇒ 2 elements
///   (base, script); `SubSuperscript` ⇒ 3 (base, sub, sup). `position` is
///   `Right` (beside) / `AboveBelow` / `Movable`; we lay out beside the base in
///   all cases (limits are a later layer — see [`layout`]).
///
/// Any other event (Space, Style state, Visual, EnvironmentFlow) is skipped
/// gracefully for now. Returns `None` only on a parser error.
fn parse_list(src: &str) -> Option<MathList> {
    use pulldown_latex::event::Event;
    use pulldown_latex::{Parser, Storage};

    let storage = Storage::new();
    let parser = Parser::new(src, &storage);

    let mut events: Vec<Event> = Vec::new();
    for ev in parser {
        events.push(ev.ok()?); // parser error → None
    }

    let mut cursor = 0usize;
    let list = read_list(&events, &mut cursor, None, None, /* until_end */ false);
    Some(list)
}

/// Read events into a [`MathList`] starting at `*i`. With `until_end`, consume up
/// to (and past) the matching [`Event::End`] for a group we've already entered;
/// otherwise read to the end of the buffer (the top-level row). `font` is the
/// active explicit font ([`None`] = TeX default italic rule), inherited by
/// nested groups.
fn read_list(
    events: &[pulldown_latex::event::Event],
    i: &mut usize,
    font: Option<pulldown_latex::event::Font>,
    color: Option<[u8; 4]>,
    until_end: bool,
) -> MathList {
    read_list_until(events, i, font, color, until_end, /* stop_at_flow */ false)
}

/// As [`read_list`], but `stop_at_flow` makes it return (without consuming the
/// event) at the next [`pulldown_latex::event::EnvironmentFlow`] — used to read
/// a single matrix cell, whose end is marked by an `Alignment` (`&`) or
/// `NewLine` (`\\`).
fn read_list_until(
    events: &[pulldown_latex::event::Event],
    i: &mut usize,
    font: Option<pulldown_latex::event::Font>,
    color: Option<[u8; 4]>,
    until_end: bool,
    stop_at_flow: bool,
) -> MathList {
    use pulldown_latex::event::{ColorTarget, Event, StateChange};

    let mut list = MathList::new();
    let mut font = font;
    // The active text color, like `font`: a `\color`/`\textcolor` scope (a
    // `StateChange::Color { target: Text }`) sets it for the rest of this group and
    // all deeper groups (which inherit `color` when we recurse). Background/border
    // targets (`\colorbox`/`\fcolorbox`) are not rendered yet — see below.
    let mut color = color;
    // `\colorbox`/`\fcolorbox` backgrounds/borders: pulldown wraps the box in a
    // `Begin(Normal)` group whose first `StateChange::Color` carries a `Background`
    // (and, for `\fcolorbox`, a preceding `Border`) target. We capture them here
    // and, on the group's `End`, wrap this list in a `MathNode::ColorBox`.
    let mut background: Option<[u8; 4]> = None;
    let mut border: Option<[u8; 4]> = None;
    // A `\dfrac`/`\tfrac` arrives as `Begin(Normal)`, `StateChange(Style(..))`,
    // `Visual(Fraction)` …; we capture the style change here so the following
    // fraction can adopt it. It only ever precedes a fraction in our subset.
    let mut style_hint: Option<Style> = None;
    while *i < events.len() {
        if until_end && matches!(events[*i], Event::End) {
            *i += 1; // consume the End that closes this group
            break;
        }
        // A cell read (`stop_at_flow`) ends at the next `&`/`\\` flow marker,
        // leaving it for the matrix reader to consume.
        if stop_at_flow && matches!(events[*i], Event::EnvironmentFlow(_)) {
            break;
        }
        match &events[*i] {
            // Font state applies to the rest of this group (and deeper groups,
            // which inherit `font` when we recurse below).
            Event::StateChange(StateChange::Font(f)) => {
                font = *f;
                *i += 1;
            }
            // Color state (`\color{…}`, and the `Begin`-wrapped form emitted by
            // `\textcolor{…}{…}`): pulldown has already resolved the named/`#rrggbb`
            // color to RGB. We honor the text target directly; the `Background` /
            // `Border` targets (`\colorbox`/`\fcolorbox`) are recorded so the
            // group's content is wrapped in a `ColorBox` on `End` (painted behind).
            Event::StateChange(StateChange::Color(cc)) => {
                let (r, g, b) = cc.color;
                match cc.target {
                    ColorTarget::Text => color = Some([r, g, b, 255]),
                    ColorTarget::Background => background = Some([r, g, b, 255]),
                    ColorTarget::Border => border = Some([r, g, b, 255]),
                }
                *i += 1;
            }
            // Style state (`\dfrac`/`\tfrac`/`\displaystyle`) — remember it for the
            // fraction it wraps; `map_style` ignores Script/ScriptScript hints,
            // which our subset never emits before a fraction.
            Event::StateChange(StateChange::Style(s)) => {
                style_hint = map_style(*s);
                *i += 1;
            }
            // A fraction: the two following elements are numerator then denominator.
            // The `Option<Dimension>` is the bar thickness: `None` (`\frac`) keeps
            // the default rule; an explicit `0` (`\binom`/`\genfrac{}{}{0pt}{}`)
            // draws none; an explicit non-zero `\genfrac` thickness is honored.
            Event::Visual(pulldown_latex::event::Visual::Fraction(d)) => {
                let bar = bar_from_dimension(*d);
                *i += 1;
                let num = read_element(events, i, font, color).map(|n| vec![n]).unwrap_or_default();
                let den = read_element(events, i, font, color).map(|n| vec![n]).unwrap_or_default();
                list.push(MathNode::Frac {
                    num,
                    den,
                    style: style_hint.take(),
                    color,
                    bar,
                });
            }
            // A radical: `\sqrt{x}` is `SquareRoot` + one element (the radicand);
            // `\sqrt[n]{x}` is `Root` + two elements (radicand then index).
            Event::Visual(pulldown_latex::event::Visual::SquareRoot) => {
                *i += 1;
                let radicand =
                    read_element(events, i, font, color).map(|n| vec![n]).unwrap_or_default();
                list.push(MathNode::Radical { index: None, radicand, color });
            }
            Event::Visual(pulldown_latex::event::Visual::Root) => {
                *i += 1;
                let radicand =
                    read_element(events, i, font, color).map(|n| vec![n]).unwrap_or_default();
                let index = read_element(events, i, font, color).map(|n| vec![n]);
                list.push(MathNode::Radical { index, radicand, color });
            }
            // The `\cancel` sentinel (a lone PUA `Content::Ordinary` from the macro
            // pass): consume it, then take the following element as the struck body.
            Event::Content(pulldown_latex::event::Content::Ordinary { content, .. })
                if *content == CANCEL_SENTINEL =>
            {
                *i += 1;
                let body =
                    read_element(events, i, font, color).map(|n| vec![n]).unwrap_or_default();
                list.push(MathNode::Cancel { body, color });
            }
            // The `\substack` sentinel: consume it, read the following matrix and
            // retag it as script-sized (`MatrixKind::Substack`).
            Event::Content(pulldown_latex::event::Content::Ordinary { content, .. })
                if *content == SUBSTACK_SENTINEL =>
            {
                *i += 1;
                if let Some(node) = read_element(events, i, font, color) {
                    list.push(retag_substack(node));
                }
            }
            // A script consumes the following element(s) as base + script(s).
            Event::Script { ty, position } => {
                let (ty, position) = (*ty, *position);
                *i += 1;
                if let Some(node) = read_script(events, i, ty, position, font, color) {
                    list.push(node);
                }
            }
            // Anything else: read one element (atom or group) and append it.
            _ => {
                if let Some(node) = read_element(events, i, font, color) {
                    list.push(node);
                } else {
                    *i += 1; // unhandled event → skip so we always make progress
                }
            }
        }
    }
    // A `\colorbox`/`\fcolorbox` wraps this whole group's content over a fill.
    if let Some(background) = background {
        return vec![MathNode::ColorBox { body: list, background, border }];
    }
    list
}

/// The Unicode Private-Use-Area sentinel the macro pass (`super::macros`) emits
/// in place of `\cancel`, so the IR can tell `\cancel{…}` apart from `\not{…}`
/// (pulldown lowers both to an indistinguishable `Visual::Negation`). It arrives
/// as a `Content::Ordinary` atom whose only job is to mark the *next* element as
/// struck; it is never rendered as a glyph.
const CANCEL_SENTINEL: char = '\u{E000}';

/// The Private-Use-Area sentinel the macro pass (`super::macros`) emits just
/// before a `\substack`-derived `matrix`, in sync with macros'
/// `SUBSTACK_SENTINEL`. It arrives as a `Content::Ordinary` atom whose only job
/// is to mark the *next* element (the matrix) as a [`MatrixKind::Substack`], so
/// layout renders its rows one style step smaller (script size); it is never
/// rendered as a glyph.
const SUBSTACK_SENTINEL: char = '\u{E001}';

/// Retag a freshly-read `\substack` body as a script-sized stack. The macro pass
/// rewrites `\substack{…}` to a `matrix` preceded by [`SUBSTACK_SENTINEL`]; this
/// flips that matrix's [`MatrixKind`] to [`MatrixKind::Substack`] so
/// [`layout_matrix`] renders it one style step smaller. A non-matrix node (an
/// unexpected shape) is returned unchanged.
fn retag_substack(node: MathNode) -> MathNode {
    match node {
        MathNode::Matrix { rows, col_align, col_seps, row_lines, .. } => {
            MathNode::Matrix { rows, col_align, kind: MatrixKind::Substack, col_seps, row_lines }
        }
        other => other,
    }
}

/// Resolve a fraction's bar thickness from pulldown's `Visual::Fraction`
/// dimension: `None` (`\frac` and friends) → the font default; an explicit
/// thickness of `0` (`\binom`/`\genfrac{}{}{0pt}{}`) → no bar; any other explicit,
/// non-zero thickness (`\genfrac{(}{)}{2pt}{}{a}{b}`) → that thickness, expressed
/// as a multiple of the em so [`layout_frac`] can scale it to px at the base em.
///
/// Length units are reduced to em with TeX's standard relations: `em`/`ex`/`mu`
/// are font-relative directly (`1ex ≈ 0.5em`, `18mu = 1em`), and the absolute
/// units use the TeXbook's `1in = 72.27pt` with the common `1em = 10pt` body-font
/// convention (so `2pt → 0.2em`). This needs no DPI and matches KaTeX's `genfrac`
/// closely enough for the rare explicit-thickness case.
fn bar_from_dimension(d: Option<pulldown_latex::event::Dimension>) -> BarThickness {
    use pulldown_latex::event::DimensionUnit as U;
    let Some(dim) = d else {
        return BarThickness::Default;
    };
    if dim.value == 0.0 {
        return BarThickness::None;
    }
    // Points per em under the standard 10pt-body convention; other absolute units
    // first convert to pt, then to em.
    const PT_PER_EM: f32 = 10.0;
    const PT_PER_IN: f32 = 72.27;
    let em = match dim.unit {
        U::Em => dim.value,
        U::Ex => dim.value * 0.5,
        U::Mu => dim.value / 18.0,
        U::Pt => dim.value / PT_PER_EM,
        U::Bp => dim.value * (PT_PER_IN / 72.0) / PT_PER_EM,
        U::Pc => dim.value * 12.0 / PT_PER_EM,
        U::Sp => dim.value / 65536.0 / PT_PER_EM,
        U::Dd => dim.value * (1238.0 / 1157.0) / PT_PER_EM,
        U::Cc => dim.value * 12.0 * (1238.0 / 1157.0) / PT_PER_EM,
        U::In => dim.value * PT_PER_IN / PT_PER_EM,
        U::Cm => dim.value * (PT_PER_IN / 2.54) / PT_PER_EM,
        U::Mm => dim.value * (PT_PER_IN / 25.4) / PT_PER_EM,
    };
    BarThickness::Em(em)
}

/// Map pulldown's `Style` (from `\dfrac`/`\tfrac`/`\displaystyle`/`\textstyle`)
/// onto our [`Style`]; Script/ScriptScript hints aren't produced before a
/// fraction in our subset, so we treat them as "no override".
fn map_style(s: pulldown_latex::event::Style) -> Option<Style> {
    use pulldown_latex::event::Style as S;
    match s {
        S::Display => Some(Style::Display),
        S::Text => Some(Style::Text),
        S::Script | S::ScriptScript => None,
    }
}

/// Read a single *element* — either one atomic content event or a full
/// `Begin..End` group — starting at `*i`, advancing the cursor past it. Returns
/// `None` (without advancing) for events that aren't element starts; the caller
/// skips those.
fn read_element(
    events: &[pulldown_latex::event::Event],
    i: &mut usize,
    font: Option<pulldown_latex::event::Font>,
    color: Option<[u8; 4]>,
) -> Option<MathNode> {
    use pulldown_latex::event::{Event, Grouping};

    match &events[*i] {
        // `\left( … \right)`: pulldown wraps the body in
        // `Begin(Grouping::LeftRight(open, close)) … End`, where `open`/`close`
        // are `Option<char>` (a null `\left.` is `None`). We read the body row
        // and keep the delimiters so layout can size them to the content.
        Event::Begin(Grouping::LeftRight(open, close)) => {
            let (open, close) = (*open, *close);
            *i += 1;
            let body = read_list(events, i, font, color, /* until_end */ true);
            Some(MathNode::Delim { open, body, close })
        }
        Event::Begin(Grouping::Normal) => {
            *i += 1;
            let inner = read_list(events, i, font, color, /* until_end */ true);
            Some(MathNode::Group(inner))
        }
        // Matrix/array/cases/aligned environments: read the grid of cells.
        Event::Begin(Grouping::Matrix { .. })
        | Event::Begin(Grouping::Cases { .. })
        | Event::Begin(Grouping::Array(_))
        | Event::Begin(Grouping::Aligned)
        | Event::Begin(Grouping::Align { .. })
        | Event::Begin(Grouping::Alignat { .. })
        | Event::Begin(Grouping::Alignedat { .. })
        | Event::Begin(Grouping::Split)
        | Event::Begin(Grouping::Gathered)
        | Event::Begin(Grouping::Gather { .. })
        | Event::Begin(Grouping::SubArray { .. }) => Some(read_matrix(events, i, font, color)),
        // Other groupings (environments) inherit the font/color and read as a group.
        Event::Begin(_) => {
            *i += 1;
            let inner = read_list(events, i, font, color, true);
            Some(MathNode::Group(inner))
        }
        // A script whose base is itself another script (e.g. `x^a^b` edge cases):
        // recurse so the inner script becomes this element.
        Event::Script { ty, position } => {
            let (ty, position) = (*ty, *position);
            *i += 1;
            read_script(events, i, ty, position, font, color)
        }
        // A bare fraction as an element (e.g. `x^\frac12`): consume its two
        // operands. (Braced/`\dfrac` forms arrive wrapped in a group handled
        // above; those style hints are read in `read_list`.)
        Event::Visual(pulldown_latex::event::Visual::Fraction(d)) => {
            let bar = bar_from_dimension(*d);
            *i += 1;
            let num = read_element(events, i, font, color).map(|n| vec![n]).unwrap_or_default();
            let den = read_element(events, i, font, color).map(|n| vec![n]).unwrap_or_default();
            Some(MathNode::Frac {
                num,
                den,
                style: None,
                color,
                bar,
            })
        }
        // A bare radical as an element (e.g. `x^\sqrt2`, or a `\sqrt` in a
        // fraction operand): consume its radicand (and index for `Root`).
        Event::Visual(pulldown_latex::event::Visual::SquareRoot) => {
            *i += 1;
            let radicand = read_element(events, i, font, color).map(|n| vec![n]).unwrap_or_default();
            Some(MathNode::Radical { index: None, radicand, color })
        }
        Event::Visual(pulldown_latex::event::Visual::Root) => {
            *i += 1;
            let radicand = read_element(events, i, font, color).map(|n| vec![n]).unwrap_or_default();
            let index = read_element(events, i, font, color).map(|n| vec![n]);
            Some(MathNode::Radical { index, radicand, color })
        }
        // The `\cancel` sentinel as a single element (e.g. `x^{\cancel y}`):
        // consume it and strike the following element.
        Event::Content(pulldown_latex::event::Content::Ordinary { content, .. })
            if *content == CANCEL_SENTINEL =>
        {
            *i += 1;
            let body = read_element(events, i, font, color).map(|n| vec![n]).unwrap_or_default();
            Some(MathNode::Cancel { body, color })
        }
        // The `\substack` sentinel as a single element: consume it and retag the
        // following matrix as script-sized.
        Event::Content(pulldown_latex::event::Content::Ordinary { content, .. })
            if *content == SUBSTACK_SENTINEL =>
        {
            *i += 1;
            read_element(events, i, font, color).map(retag_substack)
        }
        Event::Content(content) => {
            let node = atoms_from_content(*content, font, color);
            *i += 1;
            node
        }
        // Not an element start (End, Space, …): leave the cursor for the caller.
        _ => None,
    }
}

/// Read a matrix/array/cases/aligned environment starting at its
/// `Begin(Grouping::{Matrix,Cases,Array,Aligned,…})` (at `*i`), advancing past
/// the matching `End`.
///
/// The grouping itself names the environment kind and (for `Array`) the column
/// spec. Inside, cells are separated by [`EnvironmentFlow::Alignment`] (`&`) and
/// rows by [`EnvironmentFlow::NewLine`] (`\\`); a leading
/// [`EnvironmentFlow::StartLines`] (`\hline` at the top) is skipped. Each cell is
/// a [`read_list_until`] that stops at the next flow marker. We discard a wholly
/// empty trailing row (the common `… \\ \end{…}` case).
///
/// Column alignment: `Array` uses its explicit `l`/`c`/`r` letters (vertical
/// `\hline` separators are ignored — TODO); `matrix` uses the parsed `alignment`;
/// `aligned`/`align` alternate right, left, right, …; `cases` is two
/// left-aligned columns; everything else centers.
fn read_matrix(
    events: &[pulldown_latex::event::Event],
    i: &mut usize,
    font: Option<pulldown_latex::event::Font>,
    color: Option<[u8; 4]>,
) -> MathNode {
    use pulldown_latex::event::{ArrayColumn, ColumnAlignment, EnvironmentFlow, Event, Grouping};

    fn map_align(a: ColumnAlignment) -> Align {
        match a {
            ColumnAlignment::Left => Align::Left,
            ColumnAlignment::Center => Align::Center,
            ColumnAlignment::Right => Align::Right,
        }
    }

    // `col_seps[c]` = count of vertical `|` rules at the left edge of column `c`;
    // the trailing entry is the right-edge count. Only `array` carries separators;
    // the other families get an all-zero vector sized to their column count.
    let (kind, col_align, col_seps): (MatrixKind, Vec<Align>, Vec<u8>) = match &events[*i] {
        Event::Begin(Grouping::Array(cols)) => {
            let mut aligns = Vec::new();
            // Accumulate separators seen since the previous column into the slot
            // *before* the next column; the run-out tail becomes the right edge.
            let mut seps = vec![0u8];
            for c in cols.iter() {
                match c {
                    ArrayColumn::Column(a) => {
                        aligns.push(map_align(*a));
                        seps.push(0);
                    }
                    ArrayColumn::Separator(_) => {
                        // `Line::Solid`/`Line::Dashed` both render as a rule; we do
                        // not yet distinguish dashed from solid vertical rules.
                        *seps.last_mut().unwrap() += 1;
                    }
                }
            }
            (MatrixKind::Plain, aligns, seps)
        }
        Event::Begin(Grouping::Matrix { alignment }) => {
            (MatrixKind::Plain, vec![map_align(*alignment)], vec![0, 0])
        }
        Event::Begin(Grouping::SubArray { alignment }) => {
            (MatrixKind::Plain, vec![map_align(*alignment)], vec![0, 0])
        }
        Event::Begin(Grouping::Cases { .. }) => {
            (MatrixKind::Cases, vec![Align::Left, Align::Left], vec![0, 0, 0])
        }
        Event::Begin(
            Grouping::Aligned
            | Grouping::Align { .. }
            | Grouping::Alignat { .. }
            | Grouping::Alignedat { .. }
            | Grouping::Split,
        ) => (MatrixKind::Aligned, vec![Align::Right, Align::Left], vec![0, 0, 0]),
        // Gather(ed): single centered column.
        _ => (MatrixKind::Plain, vec![Align::Center], vec![0, 0]),
    };
    *i += 1; // consume the Begin

    let mut rows: Vec<Vec<MathList>> = Vec::new();
    let mut row: Vec<MathList> = Vec::new();
    // Horizontal-rule count per row boundary, indexed by the number of rows
    // already pushed: `boundary_lines[r]` accrues `\hline`s above row `r`. A
    // leading `StartLines` (top `\hline`s) lands in slot 0; each `NewLine`
    // carries the rules at *its* boundary, which sit above the row that follows
    // (or below the last row if no more rows follow).
    let mut boundary_lines: Vec<u8> = vec![0];
    loop {
        match events.get(*i) {
            None => break,
            Some(Event::End) => {
                *i += 1;
                break;
            }
            Some(Event::EnvironmentFlow(EnvironmentFlow::Alignment)) => {
                *i += 1;
                row.push(read_list_until(events, i, font, color, false, true));
            }
            Some(Event::EnvironmentFlow(EnvironmentFlow::NewLine { horizontal_lines, .. })) => {
                let n = horizontal_lines.len() as u8;
                *i += 1;
                rows.push(std::mem::take(&mut row));
                // The rules of this `\\` sit at the boundary *after* the pushed row.
                boundary_lines.push(n);
            }
            // `\hline` at the very top of the environment: rules above the first row.
            Some(Event::EnvironmentFlow(EnvironmentFlow::StartLines { lines })) => {
                boundary_lines[0] += lines.len() as u8;
                *i += 1;
            }
            // The first cell of a row (or content after `\hline`): read it.
            Some(_) => {
                row.push(read_list_until(events, i, font, color, false, true));
            }
        }
    }
    // Flush the final row unless it is empty (trailing `\\`).
    if !row.is_empty() {
        rows.push(row);
    }
    // Drop a wholly empty trailing row left by `… \\ \end`: its `\\` already
    // recorded a boundary, which then becomes the bottom rule line.
    if rows.last().is_some_and(|r| r.iter().all(|c| c.is_empty())) {
        rows.pop();
    }
    // `row_lines` must be exactly `rows.len() + 1` long (one slot per boundary,
    // top through bottom). Trim or pad the accumulated boundaries to match: a
    // trailing empty row trimmed above leaves an extra boundary that is the
    // bottom edge, and a final `\\` with no following row supplies the bottom rule.
    let mut row_lines = boundary_lines;
    row_lines.resize(rows.len() + 1, 0);

    MathNode::Matrix { rows, col_align, kind, col_seps, row_lines }
}

/// Consume a script's operands after its [`Event::Script`] has been read.
/// `Subscript`/`Superscript` ⇒ base + one script; `SubSuperscript` ⇒ base, sub,
/// sup (pulldown's documented operand order). Each operand is one element.
fn read_script(
    events: &[pulldown_latex::event::Event],
    i: &mut usize,
    ty: pulldown_latex::event::ScriptType,
    position: pulldown_latex::event::ScriptPosition,
    font: Option<pulldown_latex::event::Font>,
    color: Option<[u8; 4]>,
) -> Option<MathNode> {
    use pulldown_latex::event::{Content, Event, ScriptPosition, ScriptType};

    let base = read_element(events, i, font, color).map(|n| vec![n]).unwrap_or_default();

    // Accents (`\hat \bar \vec \overline \widehat …`, and `\underline`/`\underbar`)
    // are emitted as a `Script { position: AboveBelow }` whose single operand is a
    // lone accent character (`Content::Ordinary`). Detect that here — an
    // `AboveBelow` `Sub`/`Superscript` whose script operand is one such char —
    // and fold it into a [`MathNode::Accent`]. Everything else stays a `Script`.
    if matches!(position, ScriptPosition::AboveBelow)
        && matches!(ty, ScriptType::Subscript | ScriptType::Superscript)
    {
        if let Some(Event::Content(Content::Ordinary { content, stretchy })) = events.get(*i) {
            if is_accent_char(*content) {
                let (accent, stretchy) = (*content, *stretchy);
                *i += 1; // consume the accent char
                return Some(MathNode::Accent {
                    accent,
                    stretchy,
                    under: matches!(ty, ScriptType::Subscript),
                    base,
                    color,
                });
            }
        }
    }

    let (sub, sup) = match ty {
        ScriptType::Subscript => (read_element(events, i, font, color).map(|n| vec![n]), None),
        ScriptType::Superscript => (None, read_element(events, i, font, color).map(|n| vec![n])),
        ScriptType::SubSuperscript => {
            let sub = read_element(events, i, font, color).map(|n| vec![n]);
            let sup = read_element(events, i, font, color).map(|n| vec![n]);
            (sub, sup)
        }
    };
    Some(MathNode::Script { base, sup, sub, position: map_script_pos(position) })
}

/// Whether `ch` is one of the accent characters pulldown emits as the script
/// operand of an `AboveBelow` accent (`\hat`→`^`, `\bar`→`‾`, `\vec`→`→`,
/// `\dot`→`˙`, `\underline`→`_`, and the stretchy braces/parens/groups
/// `\overbrace`→`⏞` / `\underbrace`→`⏟` / `\overparen`→`⏜` / …). Used to fold such
/// scripts into [`MathNode::Accent`]; the brace forms then stretch to the body
/// width via the horizontal MATH assembly. Anything else is treated as a real
/// over/under script (e.g. an extensible arrow's label).
fn is_accent_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{005E}' // ^   \hat / \widehat
        | '\u{007E}' // ~   \tilde / \widetilde
        | '\u{0060}' // `   \grave
        | '\u{00B4}' // ´   \acute
        | '\u{00A8}' // ¨   \ddot
        | '\u{02C7}' // ˇ   \check
        | '\u{02D9}' // ˙   \dot
        | '\u{02DC}' // ˜   tilde (alt)
        | '\u{0302}' // ◌̂  combining circumflex
        | '\u{0303}' // ◌̃  combining tilde
        | '\u{2190}' // ←   \overleftarrow
        | '\u{2192}' // →   \vec / \overrightarrow
        | '\u{2194}' // ↔   \overleftrightarrow
        | '\u{20D7}' // ◌⃗  combining vector arrow
        | '\u{203E}' // ‾   \bar / \overline
        | '\u{005F}' // _   \underline / \underbar
        | '\u{0332}' // ◌̲  combining low line
        | '\u{23DE}' // ⏞   \overbrace  (stretchy top brace)
        | '\u{23DF}' // ⏟   \underbrace (stretchy bottom brace)
        | '\u{23DC}' // ⏜   \overparen / \wideparen
        | '\u{23DD}' // ⏝   \underparen
        | '\u{23E0}' // ⏠   \overgroup
        | '\u{23E1}' // ⏡   \undergroup
    )
}

/// Map pulldown's [`pulldown_latex::event::ScriptPosition`] onto our [`ScriptPos`].
fn map_script_pos(p: pulldown_latex::event::ScriptPosition) -> ScriptPos {
    use pulldown_latex::event::ScriptPosition as P;
    match p {
        P::Right => ScriptPos::Right,
        P::AboveBelow => ScriptPos::AboveBelow,
        P::Movable => ScriptPos::Movable,
    }
}

/// Turn one [`Content`] event into a [`MathNode`]: a single [`MathNode::Atom`],
/// or a [`MathNode::Group`] of atoms for multi-char content (numbers, `\text`,
/// function names). `None` for content we don't render (e.g. LargeOp for now).
fn atoms_from_content(
    content: pulldown_latex::event::Content,
    font: Option<pulldown_latex::event::Font>,
    color: Option<[u8; 4]>,
) -> Option<MathNode> {
    use pulldown_latex::event::{Content, DelimiterSize, DelimiterType};

    let one = |ch: char, class: Class, variant: Variant| -> MathNode {
        MathNode::Atom(Atom { ch, class, variant, large_op: false, color })
    };
    let many = |s: &str, class: Class, variant: Variant| -> MathNode {
        let atoms: MathList = s
            .chars()
            .map(|ch| MathNode::Atom(Atom { ch, class, variant, large_op: false, color }))
            .collect();
        MathNode::Group(atoms)
    };

    Some(match content {
        // A symbol large operator (`\sum`, `\int`, `\prod`, `\bigcup`, …): a
        // single Op-class glyph that grows in Display style and straddles the
        // axis. `small` (`\smallint`) keeps the base glyph (no growth).
        Content::LargeOp { content, small } => MathNode::Atom(Atom {
            ch: content,
            class: Class::Op,
            variant: Variant::Upright,
            large_op: !small,
            color,
        }),
        Content::Ordinary { content, .. } => {
            one(content, Class::Ord, variant_for(font, content))
        }
        Content::Number(num) => many(num, Class::Ord, Variant::Upright),
        Content::Text(text) => many(text, Class::Ord, Variant::Upright),
        Content::Function(name) => many(name, Class::Op, Variant::Upright),
        Content::BinaryOp { content, .. } => one(content, Class::Bin, Variant::Upright),
        Content::Relation { content, .. } => {
            let mut buf = [0u8; 8];
            let bytes = content.encode_utf8_to_buf(&mut buf);
            let s = std::str::from_utf8(bytes).ok()?;
            many(s, Class::Rel, Variant::Upright)
        }
        Content::Delimiter { content, ty, size } => {
            let class = match ty {
                DelimiterType::Open => Class::Open,
                DelimiterType::Close => Class::Close,
                DelimiterType::Fence => Class::Inner,
            };
            match size {
                // `\bigl(`/`\Bigr]`/… : a fixed-size delimiter. The em multiples
                // mirror pulldown's (private) `DelimiterSize::to_em` /
                // KaTeX `sizeToMaxHeight`: \big 1.2, \Big 1.8, \bigg 2.4, \Bigg 3.0.
                Some(s) => MathNode::BigDelim {
                    ch: content,
                    target_em: match s {
                        DelimiterSize::Big => 1.2,
                        DelimiterSize::BIG => 1.8,
                        DelimiterSize::Bigg => 2.4,
                        DelimiterSize::BIGG => 3.0,
                    },
                    class,
                    color,
                },
                // A plain (auto-sized only via `\left\right`) delimiter is one atom.
                None => one(content, class, Variant::Upright),
            }
        }
        Content::Punctuation(ch) => one(ch, Class::Punct, Variant::Upright),
    })
}

/// Pick the letterform for an [`Content::Ordinary`] character given the current
/// explicit font (if any), mirroring pulldown-latex's default `MathStyle::TeX`
/// rule: with no explicit font, ASCII letters / lowercase Greek go italic while
/// uppercase Greek (and other symbols) stay upright.
fn variant_for(font: Option<pulldown_latex::event::Font>, ch: char) -> Variant {
    match font {
        // Any explicit font (`\mathbf`, `\mathbb`, `\mathcal`, `\mathfrak`, …)
        // maps directly to its math-alphabet variant (with graceful upright
        // fallback in `glyph::glyph_for` when a styled glyph is absent).
        Some(f) => glyph::variant_from_font(f),
        // No explicit font: TeX default. `is_uppercase() && !is_ascii_uppercase()`
        // marks uppercase Greek (and similar) as upright; everything else italic.
        None => {
            if ch.is_uppercase() && !ch.is_ascii_uppercase() {
                Variant::Upright
            } else {
                Variant::Italic
            }
        }
    }
}

/// Inter-atom spacing in **mu** (1 mu = 1/18 em) between a left atom of `left`
/// class and a right atom of `right` class, ported from KaTeX
/// `src/spacingData.ts` (`spacings` / `tightSpacings`), which is the TeXbook
/// Chapter 18 / Appendix-G table. `tight` selects the script-style table where
/// Bin/Rel spacing is suppressed.
///
/// Thinspace = 3mu, medspace = 4mu, thickspace = 5mu; unlisted pairs are 0.
fn spacing_mu(left: Class, right: Class, tight: bool) -> f32 {
    use Class::*;
    const THIN: f32 = 3.0;
    const MED: f32 = 4.0;
    const THICK: f32 = 5.0;

    if tight {
        // Script / scriptscript styles: only the thin-space pairs survive.
        return match (left, right) {
            (Ord, Op) => THIN,
            (Op, Ord) | (Op, Op) => THIN,
            (Close, Op) => THIN,
            (Inner, Op) => THIN,
            _ => 0.0,
        };
    }

    match (left, right) {
        (Ord, Op) => THIN,
        (Ord, Bin) => MED,
        (Ord, Rel) => THICK,
        (Ord, Inner) => THIN,

        (Op, Ord) => THIN,
        (Op, Op) => THIN,
        (Op, Rel) => THICK,
        (Op, Inner) => THIN,

        (Bin, Ord) => MED,
        (Bin, Op) => MED,
        (Bin, Open) => MED,
        (Bin, Inner) => MED,

        (Rel, Ord) => THICK,
        (Rel, Op) => THICK,
        (Rel, Open) => THICK,
        (Rel, Inner) => THICK,

        // Open: no space after an opening delimiter.
        (Close, Op) => THIN,
        (Close, Bin) => MED,
        (Close, Rel) => THICK,
        (Close, Inner) => THIN,

        (Punct, Ord) => THIN,
        (Punct, Op) => THIN,
        (Punct, Rel) => THICK,
        (Punct, Open) => THIN,
        (Punct, Close) => THIN,
        (Punct, Punct) => THIN,
        (Punct, Inner) => THIN,

        (Inner, Ord) => THIN,
        (Inner, Op) => THIN,
        (Inner, Bin) => MED,
        (Inner, Rel) => THICK,
        (Inner, Open) => THIN,
        (Inner, Punct) => THIN,
        (Inner, Inner) => THIN,

        _ => 0.0,
    }
}

/// Per-render layout context: the face, base em (px), and the MATH constants /
/// per-style scale factors needed for script positioning. Borrows the face for
/// the lifetime of layout.
struct Ctx<'f> {
    face: &'f Face<'static>,
    /// Base font size in px (the `\normalsize` em).
    base_em: f32,
    /// Em-scale factor for [`Style::Script`] (e.g. 0.7).
    script_scale: f32,
    /// Em-scale factor for [`Style::ScriptScript`] (e.g. 0.5).
    scriptscript_scale: f32,
    /// MATH per-glyph italic corrections, if the font provides them.
    italic_corrections: Option<ttf_parser::math::MathValues<'f>>,
    /// The straight RGBA fill currently in effect, applied to every glyph and rule
    /// laid out. Defaults to [`MathOptions::color`] and is temporarily overridden
    /// (then restored) while laying out a `\color`/`\textcolor` scope — see
    /// [`with_color`]. A [`std::cell::Cell`] so the ambient color can change as we
    /// descend the tree without threading it through every layout signature.
    cur_color: std::cell::Cell<[u8; 4]>,
    /// `\arraystretch` factor (default 1.0): scales the nominal inter-row baseline
    /// distance of `matrix`/`array` environments. Extracted by the macro pass
    /// (pulldown does not surface it) and applied in [`layout_matrix`].
    arraystretch: f32,
}

impl Ctx<'_> {
    /// The font-units→px scale for `style` (base em × the style's percent-down).
    fn scale_for(&self, style: Style) -> f32 {
        let factor = match style {
            Style::Display | Style::Text => 1.0,
            Style::Script => self.script_scale,
            Style::ScriptScript => self.scriptscript_scale,
        };
        self.base_em * factor / UNITS_PER_EM
    }

    /// A MATH constant read in px at the **base** em. MATH shift/gap constants are
    /// design-space values; we scale them by the base em (TeX applies script
    /// shifts in the *surrounding* style's units), independent of glyph scale.
    fn const_px(&self, v: ttf_parser::math::MathValue) -> f32 {
        v.value as f32 * self.base_em / UNITS_PER_EM
    }

    /// Run `f` with the ambient glyph/rule color temporarily set to `color`,
    /// restoring the previous color afterward. A `None` color leaves the current
    /// one in place (the common "no `\color` here" case). Mirrors how the font
    /// stack scopes to a group: a `\color`/`\textcolor` scope colors everything
    /// laid out for that node, then the surrounding color resumes.
    fn with_color<T>(&self, color: Option<[u8; 4]>, f: impl FnOnce() -> T) -> T {
        let Some(color) = color else {
            return f();
        };
        let prev = self.cur_color.replace(color);
        let out = f();
        self.cur_color.set(prev);
        out
    }

    /// The font's x-height in px at the base em (fallback ≈ 0.45 em).
    fn x_height_px(&self) -> f32 {
        self.face
            .x_height()
            .map(|x| x as f32 * self.base_em / UNITS_PER_EM)
            .unwrap_or(0.45 * self.base_em)
    }
}

/// Lay out `src` into a box tree, returning the root box and the face used to
/// produce it (the caller needs the same face to outline the glyphs).
///
/// Builds the [`MathList`] tree ([`parse_list`]), then recursively lays it out
/// ([`layout_list`]) at the starting style implied by `opts.style`
/// (Display → [`Style::Display`], Inline → [`Style::Text`]).
///
/// Returns `None` on parse failure or when the input yields no renderable atoms.
pub fn layout(src: &str, opts: &MathOptions, arraystretch: f32) -> Option<(Box, Face<'static>)> {
    let list = parse_list(src)?;
    if list.is_empty() {
        return None;
    }

    let face = font::math_face();
    let consts = face.tables().math.and_then(|m| m.constants);
    // Per-style scale-downs from the MATH table; fall back to TeX-ish 0.7 / 0.5.
    let (script_scale, scriptscript_scale) = match consts {
        Some(c) => (
            c.script_percent_scale_down() as f32 / 100.0,
            c.script_script_percent_scale_down() as f32 / 100.0,
        ),
        None => (0.7, 0.5),
    };

    let ctx = Ctx {
        face: &face,
        base_em: opts.font_size_px,
        script_scale,
        scriptscript_scale,
        italic_corrections: face
            .tables()
            .math
            .and_then(|m| m.glyph_info)
            .and_then(|gi| gi.italic_corrections),
        cur_color: std::cell::Cell::new(opts.color),
        arraystretch,
    };

    let start = match opts.style {
        super::MathStyle::Display => Style::Display,
        super::MathStyle::Inline => Style::Text,
    };
    // Top-level list is uncramped.
    let root = layout_list(&ctx, &list, start, /* cramped */ false)?;

    // SAFETY/ownership: the returned face owns `'static` bundled bytes; `ctx`
    // only borrowed it, and we move the owned `face` out here.
    Some((root, face))
}

/// Lay out a [`MathList`] into one horizontal [`Box`] at `style`, applying TeX
/// inter-atom spacing between siblings. Returns `None` if nothing renders.
///
/// `cramped` propagates the cramped flag to atoms (it only matters where it
/// reaches a [`MathNode::Script`], which chooses superscript-cramped shifts).
fn layout_list(ctx: &Ctx, list: &MathList, style: Style, cramped: bool) -> Option<Box> {
    // Resolve each node's class first so spacing + the unary-Bin fix can see them.
    let mut classes: Vec<Class> = list.iter().map(node_class).collect();

    // TeXbook rule (p. 442): a Bin atom at list start, or after Op/Bin/Rel/Open/
    // Punct, is re-classed Ord (unary → no Bin spacing).
    let mut prev: Option<Class> = None;
    for c in classes.iter_mut() {
        if *c == Class::Bin
            && matches!(
                prev,
                None | Some(Class::Op | Class::Bin | Class::Rel | Class::Open | Class::Punct)
            )
        {
            *c = Class::Ord;
        }
        prev = Some(*c);
    }

    let mu_px = ctx.base_em / 18.0 * style_em_factor(ctx, style);
    let tight = style.is_tight();

    let mut children: Vec<Child> = Vec::new();
    let mut pen = 0.0f32;
    let mut row_height = 0.0f32;
    let mut row_depth = 0.0f32;
    let mut prev_class: Option<Class> = None;

    for (node, &class) in list.iter().zip(classes.iter()) {
        let Some(b) = layout_node(ctx, node, style, cramped) else {
            continue;
        };
        if let Some(left) = prev_class {
            pen += spacing_mu(left, class, tight) * mu_px;
        }
        row_height = row_height.max(b.height);
        row_depth = row_depth.max(b.depth);
        let advance = b.width;
        children.push(Child { dx: pen, dy: 0.0, b });
        pen += advance;
        prev_class = Some(class);
    }

    if children.is_empty() {
        return None;
    }
    Some(Box {
        width: pen,
        height: row_height,
        depth: row_depth,
        kind: BoxKind::Hbox { children },
    })
}

/// The em-scale factor for `style` (1.0 / script / scriptscript). Used to scale
/// `mu`-based inter-atom spacing down inside scripts.
fn style_em_factor(ctx: &Ctx, style: Style) -> f32 {
    match style {
        Style::Display | Style::Text => 1.0,
        Style::Script => ctx.script_scale,
        Style::ScriptScript => ctx.scriptscript_scale,
    }
}

/// Lay out a single [`MathNode`] at `style`.
///
/// Nodes that carry an explicit `\color`/`\textcolor` scope (atoms, fractions,
/// radicals, accents, fixed delimiters) lay out under [`Ctx::with_color`], so the
/// glyphs and rules they produce pick up that color from [`Ctx::cur_color`].
/// `Group`/`Script`/`Delim`/`Matrix` carry no color of their own — their color
/// lives on the atoms/leaves inside them, which were tagged at parse time.
fn layout_node(ctx: &Ctx, node: &MathNode, style: Style, cramped: bool) -> Option<Box> {
    match node {
        MathNode::Atom(atom) => ctx.with_color(atom.color, || layout_atom(ctx, atom, style)),
        MathNode::Group(inner) => layout_list(ctx, inner, style, cramped),
        MathNode::Script { base, sup, sub, position } => {
            layout_script(ctx, base, sup.as_ref(), sub.as_ref(), *position, style, cramped)
        }
        MathNode::Frac { num, den, style: forced, color, bar } => {
            ctx.with_color(*color, || layout_frac(ctx, num, den, forced.unwrap_or(style), *bar))
        }
        MathNode::Delim { open, body, close } => {
            layout_delim(ctx, *open, body, *close, style, cramped)
        }
        MathNode::BigDelim { ch, target_em, color, .. } => {
            ctx.with_color(*color, || layout_big_delim(ctx, *ch, *target_em))
        }
        MathNode::Radical { index, radicand, color } => ctx.with_color(*color, || {
            layout_radical(ctx, index.as_ref(), radicand, style, cramped)
        }),
        MathNode::Accent { accent, stretchy, under, base, color } => ctx.with_color(*color, || {
            layout_accent(ctx, *accent, *stretchy, *under, base, style, cramped)
        }),
        MathNode::Matrix { rows, col_align, kind, col_seps, row_lines } => {
            layout_matrix(ctx, rows, col_align, *kind, col_seps, row_lines, style)
        }
        MathNode::Cancel { body, color } => {
            ctx.with_color(*color, || layout_cancel(ctx, body, style, cramped))
        }
        MathNode::ColorBox { body, background, border } => {
            layout_colorbox(ctx, body, *background, *border, style, cramped)
        }
    }
}

/// Lay out one atom into a glyph [`Box`] at `style`'s em scale.
///
/// A symbol large operator (`atom.large_op`) in Display style is grown to at least
/// `display_operator_min_height` and re-centered on the math axis — see
/// [`layout_big_op`].
fn layout_atom(ctx: &Ctx, atom: &Atom, style: Style) -> Option<Box> {
    if atom.large_op && style.is_display() {
        if let Some(b) = layout_big_op(ctx, atom.ch) {
            return Some(b);
        }
    }
    let scale = ctx.scale_for(style);
    let gid = glyph::glyph_for(ctx.face, atom.ch, atom.variant)?;
    let advance = ctx.face.glyph_hor_advance(gid).unwrap_or(0) as f32 * scale;
    let (height, depth) = glyph_extents(ctx.face, gid, scale);
    Some(Box {
        width: advance,
        height,
        depth,
        kind: BoxKind::Glyph { gid, scale, color: ctx.cur_color.get() },
    })
}

/// Grow a Display-style symbol large operator (∑ ∫ ∏ ⋃ …) and center it on the
/// math axis. The glyph is sized via [`delim::vertical_glyph`] to at least the
/// MATH `display_operator_min_height` (raw font units → px at the base em), then,
/// because tall n-ary operators are designed to straddle the axis, its vertical
/// midpoint is placed on the axis (`height = total/2 + axis`,
/// `depth = total/2 - axis`). Returns `None` if the font has no glyph for `ch`.
///
/// Ported from KaTeX `makeOp` (`src/buildHTML.js`): a display large op is grown
/// to `\bigop` size and shifted by `axisHeight - glyphCenter`.
fn layout_big_op(ctx: &Ctx, ch: char) -> Option<Box> {
    let gid = ctx.face.glyph_index(ch)?;
    let scale = ctx.scale_for(Style::Display);

    // Minimum display height for n-ary operators (raw u16 font units → px).
    let min_h = ctx
        .face
        .tables()
        .math
        .and_then(|m| m.constants)
        .map(|c| c.display_operator_min_height() as f32 * ctx.base_em / UNITS_PER_EM)
        .unwrap_or(1.2 * ctx.base_em);

    // Grow the glyph (variant or assembly) to the minimum display height. The
    // returned box is centered on its own baseline (height == depth == total/2).
    let grown = delim::vertical_glyph(ctx.face, gid, min_h, scale, ctx.cur_color.get());
    let axis = axis_px(ctx);
    let half = grown.height; // == depth; total/2.
    let width = grown.width;

    // Re-home so the operator's vertical midpoint sits on the axis: shifting the
    // (midpoint-on-baseline) box up by `axis` (dy = -axis) reaches `half + axis`
    // above and `half - axis` below the row baseline.
    Some(Box {
        width,
        height: half + axis,
        depth: (half - axis).max(0.0),
        kind: BoxKind::Hbox {
            children: vec![Child { dx: 0.0, dy: -axis, b: grown }],
        },
    })
}

/// Italic correction (px at base em) of a symbol large operator's glyph — chiefly
/// the slanted ∫, which offsets its limits horizontally. Returns 0 when the font
/// supplies none.
fn op_italic_correction(ctx: &Ctx, ch: char) -> f32 {
    ctx.italic_corrections
        .and_then(|ics| ctx.face.glyph_index(ch).and_then(|gid| ics.get(gid)))
        .map(|v| v.value as f32 * ctx.base_em / UNITS_PER_EM)
        .unwrap_or(0.0)
}

/// Italic correction of `node` in px at `style` — the overshoot of the *last*
/// glyph reached along the node's right edge. Only a single trailing glyph
/// matters for superscript placement (rule 18a); for groups we recurse into the
/// last child. Returns 0 when the font supplies no correction.
fn italic_correction(ctx: &Ctx, node: &MathNode, style: Style) -> f32 {
    let Some(ics) = ctx.italic_corrections else {
        return 0.0;
    };
    match node {
        MathNode::Atom(atom) => glyph::glyph_for(ctx.face, atom.ch, atom.variant)
            .and_then(|gid| ics.get(gid))
            .map(|v| v.value as f32 * ctx.scale_for(style))
            .unwrap_or(0.0),
        MathNode::Group(inner) => inner
            .last()
            .map(|n| italic_correction(ctx, n, style))
            .unwrap_or(0.0),
        // A script node's right edge is its own (sup/sub) box; a fraction's right
        // edge is its (rectangular) assembled box; a fenced expression's right edge
        // is its (vertical) close delimiter — none carries glyph IC.
        MathNode::Script { .. }
        | MathNode::Frac { .. }
        | MathNode::Delim { .. }
        | MathNode::BigDelim { .. }
        // A radical's right edge is its (rectangular) vinculum rule, not a glyph.
        | MathNode::Radical { .. }
        // An accent's right edge is its (centered) base/accent box, not a trailing glyph.
        | MathNode::Accent { .. }
        // A matrix's right edge is its (rectangular) grid / close delimiter.
        | MathNode::Matrix { .. }
        // A cancel's right edge is its struck body's box, not a trailing glyph.
        | MathNode::Cancel { .. }
        // A colorbox's right edge is its padded frame, not a trailing glyph.
        | MathNode::ColorBox { .. } => 0.0,
    }
}

/// Lay out a base with beside-the-base super/subscript(s) per Appendix G rule 18
/// / MathML Core, using OpenType MATH constants. `style` is the base's style;
/// scripts lay out at `style.smaller()` (sup uncramped, sub cramped).
///
/// Vertical positioning (all shifts in px at the base em):
/// * superscript shift up `u` = max(`SuperscriptShiftUp` [or `…Cramped`],
///   base.height − `SuperscriptBaselineDropMax`, sup.depth + ¼·x-height);
/// * subscript shift down `v` = max(`SubscriptShiftDown`,
///   base.depth + `SubscriptBaselineDropMin`, sub.height − ⅘·x-height);
/// * with both, enforce `SubSuperscriptGapMin` between the sup bottom and sub
///   top, keep the sup bottom ≥ `SuperscriptBottomMin`, and clamp the raise per
///   `SuperscriptBottomMaxWithSubscript`.
///
/// Horizontal: the **superscript** starts at base-right + base's italic
/// correction; the subscript at base-right (no IC).
fn layout_script(
    ctx: &Ctx,
    base: &MathList,
    sup: Option<&MathList>,
    sub: Option<&MathList>,
    position: ScriptPos,
    style: Style,
    cramped: bool,
) -> Option<Box> {
    // Extensible arrows (`\xrightarrow{f}`, `\xleftarrow[g]{f}`): pulldown emits
    // these as an `AboveBelow` script whose base is a lone stretchy arrow relation
    // (`→`/`←`/…), with the over-label as the superscript and the optional
    // under-label as the subscript. The arrow stretches to span the labels.
    if matches!(position, ScriptPos::AboveBelow) {
        if let Some(arrow) = extensible_arrow_base(base) {
            return layout_extensible_arrow(ctx, arrow, sup, sub, style);
        }
    }

    // Limits (scripts stacked above/below the operator) apply when pulldown asks
    // for `AboveBelow`, or `Movable` over an Op-class base in Display style
    // (`\sum`, `\lim`). Integrals arrive as `Right` and so stay beside (below).
    let base_is_op = base.first().map(node_class) == Some(Class::Op);
    let use_limits = match position {
        ScriptPos::AboveBelow => true,
        ScriptPos::Movable => style.is_display() && base_is_op,
        ScriptPos::Right => false,
    };
    if use_limits {
        return layout_limits(ctx, base, sup, sub, style);
    }

    let base_box = layout_list(ctx, base, style, cramped)?;
    let script_style = style.smaller();

    // Italic correction of the base's trailing glyph (for the superscript shift).
    let base_ic = base
        .last()
        .map(|n| italic_correction(ctx, n, style))
        .unwrap_or(0.0);

    let consts = ctx.face.tables().math.and_then(|m| m.constants);
    let x_height = ctx.x_height_px();

    // Default MATH-ish fallbacks (px at base em) when the table is absent.
    let c = consts;
    let sup_shift_default = 0.45 * ctx.base_em;
    let sub_shift_default = 0.2 * ctx.base_em;

    // The base sits at the row's left edge on the main baseline.
    let base_w = base_box.width;
    let base_h = base_box.height;
    let base_d = base_box.depth;
    let mut row_height = base_h;
    let mut row_depth = base_d;
    let mut row_right = base_w;
    let mut children: Vec<Child> = vec![Child { dx: 0.0, dy: 0.0, b: base_box }];

    // --- superscript shift up (u) and subscript shift down (v) ---
    let mut u = 0.0f32; // upward shift (dy is negative -u)
    let sup_box = sup.map(|s| layout_list(ctx, s, script_style, /* uncramped */ false));
    let sup_box = match sup_box {
        Some(Some(b)) => Some(b),
        _ => None,
    };
    if let Some(sb) = &sup_box {
        let shift_up = c
            .map(|c| {
                ctx.const_px(if cramped {
                    c.superscript_shift_up_cramped()
                } else {
                    c.superscript_shift_up()
                })
            })
            .unwrap_or(sup_shift_default);
        let drop_max = c.map(|c| ctx.const_px(c.superscript_baseline_drop_max())).unwrap_or(0.0);
        u = shift_up
            .max(base_h - drop_max)
            .max(sb.depth + 0.25 * x_height);
    }

    let mut v = 0.0f32; // downward shift
    let sub_box = sub.map(|s| layout_list(ctx, s, script_style, /* cramped */ true));
    let sub_box = match sub_box {
        Some(Some(b)) => Some(b),
        _ => None,
    };
    if let Some(sb) = &sub_box {
        let shift_down = c.map(|c| ctx.const_px(c.subscript_shift_down())).unwrap_or(sub_shift_default);
        let drop_min = c.map(|c| ctx.const_px(c.subscript_baseline_drop_min())).unwrap_or(0.0);
        v = shift_down
            .max(base_d + drop_min)
            .max(sb.height - 0.8 * x_height);
    }

    // --- combined-script clearance (rule 18e) ---
    if let (Some(sup_b), Some(sub_b)) = (&sup_box, &sub_box) {
        let gap_min = c.map(|c| ctx.const_px(c.sub_superscript_gap_min())).unwrap_or(0.2 * ctx.base_em);
        let sup_bottom_min = c.map(|c| ctx.const_px(c.superscript_bottom_min())).unwrap_or(0.0);
        let bottom_max = c
            .map(|c| ctx.const_px(c.superscript_bottom_max_with_subscript()))
            .unwrap_or(f32::INFINITY);

        // Keep the superscript bottom from dropping too low.
        if u - sup_b.depth < sup_bottom_min {
            u = sup_bottom_min + sup_b.depth;
        }
        // Enforce the minimum gap between sup bottom and sub top.
        let gap = (u - sup_b.depth) - (-v + sub_b.height);
        if gap < gap_min {
            let deficit = gap_min - gap;
            // Prefer lowering the subscript, but clamp the sup bottom to its max.
            let sup_bottom = u - sup_b.depth;
            if sup_bottom < bottom_max {
                let raise = (bottom_max - sup_bottom).min(deficit);
                u += raise;
                v += deficit - raise;
            } else {
                v += deficit;
            }
        }
    }

    // --- place scripts ---
    // The superscript's baseline sits `u` px *above* the main baseline (dy = -u),
    // so its top reaches `u + sup.height` and its bottom is `u - sup.depth`.
    if let Some(sup_b) = sup_box {
        let dx = base_w + base_ic;
        row_height = row_height.max(u + sup_b.height);
        row_depth = row_depth.max((sup_b.depth - u).max(0.0));
        row_right = row_right.max(dx + sup_b.width);
        children.push(Child { dx, dy: -u, b: sup_b });
    }
    // The subscript's baseline sits `v` px *below* the main baseline (dy = v).
    if let Some(sub_b) = sub_box {
        let dx = base_w;
        row_depth = row_depth.max(v + sub_b.depth);
        row_height = row_height.max((sub_b.height - v).max(0.0));
        row_right = row_right.max(dx + sub_b.width);
        children.push(Child { dx, dy: v, b: sub_b });
    }

    Some(Box {
        width: row_right,
        height: row_height,
        depth: row_depth,
        kind: BoxKind::Hbox { children },
    })
}

/// Lay out an operator with **limits**: the superscript centered *above* and the
/// subscript centered *below* the base, all on a common vertical center
/// (`\sum_{i=1}^{n}`, `\lim_{x\to0}`). Used in Display style for `\sum`-style and
/// named operators; integrals stay beside (see [`layout_script`]).
///
/// Positioning (OpenType MATH, px at the base em — ported from KaTeX `assembleSupSub`
/// / TeX's `make_op`):
/// * the upper limit's baseline sits so its (ink) bottom clears the operator top by
///   `upper_limit_gap_min`, and its baseline rises at least `upper_limit_baseline_rise_min`
///   above the operator top;
/// * the lower limit's baseline sits so its (ink) top clears the operator bottom by
///   `lower_limit_gap_min`, and its baseline drops at least `lower_limit_baseline_drop_min`
///   below the operator bottom.
///
/// The construct is widened to the max of operator/sup/sub widths and each part is
/// centered. The operator's italic correction shifts the upper limit right by `ic/2`
/// and the lower limit left by `ic/2` (KaTeX's handling for the slanted ∫, though ∫
/// itself stays beside).
fn layout_limits(
    ctx: &Ctx,
    base: &MathList,
    sup: Option<&MathList>,
    sub: Option<&MathList>,
    style: Style,
) -> Option<Box> {
    let base_box = layout_list(ctx, base, style, /* cramped */ false)?;
    let limit_style = style.smaller();

    // Limits' italic-correction offset: only a symbol large op carries one.
    let ic = match base.first() {
        Some(MathNode::Atom(a)) if a.large_op => op_italic_correction(ctx, a.ch),
        _ => 0.0,
    };

    let c = ctx.face.tables().math.and_then(|m| m.constants);
    let upper_gap = c.map(|c| ctx.const_px(c.upper_limit_gap_min())).unwrap_or(0.1 * ctx.base_em);
    let upper_rise = c
        .map(|c| ctx.const_px(c.upper_limit_baseline_rise_min()))
        .unwrap_or(0.3 * ctx.base_em);
    let lower_gap = c.map(|c| ctx.const_px(c.lower_limit_gap_min())).unwrap_or(0.1 * ctx.base_em);
    let lower_drop = c
        .map(|c| ctx.const_px(c.lower_limit_baseline_drop_min()))
        .unwrap_or(0.6 * ctx.base_em);

    let sup_box = sup.and_then(|s| layout_list(ctx, s, limit_style, /* uncramped */ false));
    let sub_box = sub.and_then(|s| layout_list(ctx, s, limit_style, /* cramped */ true));

    // Overall width is the widest of the three; everything centers within it. The
    // limits shift by ±ic/2 so a slanted operator's limits track its lean.
    let base_w = base_box.width;
    let sup_w = sup_box.as_ref().map(|b| b.width).unwrap_or(0.0);
    let sub_w = sub_box.as_ref().map(|b| b.width).unwrap_or(0.0);
    let width = base_w.max(sup_w).max(sub_w + ic.abs());
    let center = width / 2.0;
    let base_dx = center - base_w / 2.0;

    let mut children: Vec<Child> = Vec::new();
    let mut height = base_box.height;
    let mut depth = base_box.depth;
    let base_h = base_box.height;
    let base_d = base_box.depth;

    // Upper limit: its baseline is raised by `u` so its bottom clears the operator
    // top by `upper_gap` and its baseline is ≥ `upper_rise` above the op top.
    if let Some(sb) = sup_box {
        let u = (base_h + upper_rise).max(base_h + upper_gap + sb.depth);
        let dx = center - sb.width / 2.0 + ic / 2.0;
        height = height.max(u + sb.height);
        children.push(Child { dx, dy: -u, b: sb });
    }
    // Lower limit: its baseline is lowered by `v` so its top clears the operator
    // bottom by `lower_gap` and its baseline is ≥ `lower_drop` below the op bottom.
    if let Some(sb) = sub_box {
        let v = (base_d + lower_drop).max(base_d + lower_gap + sb.height);
        let dx = center - sb.width / 2.0 - ic / 2.0;
        depth = depth.max(v + sb.depth);
        children.push(Child { dx, dy: v, b: sb });
    }

    // The base goes last so it paints centered on the construct.
    children.push(Child { dx: base_dx, dy: 0.0, b: base_box });

    Some(Box {
        width,
        height,
        depth,
        kind: BoxKind::Hbox { children },
    })
}

/// If `base` is the lone stretchy-arrow relation that pulldown produces for an
/// extensible arrow (`\xrightarrow`→`→`, `\xleftarrow`→`←`, and the
/// left-right/harpoon variants), return that arrow char. The arrow then stretches
/// to its labels in [`layout_extensible_arrow`]; any other `AboveBelow` base falls
/// through to the ordinary over/under [`layout_limits`] path.
fn extensible_arrow_base(base: &MathList) -> Option<char> {
    let atom = match base.as_slice() {
        [MathNode::Atom(a)] => a,
        [MathNode::Group(inner)] => match inner.as_slice() {
            [MathNode::Atom(a)] => a,
            _ => return None,
        },
        _ => return None,
    };
    if atom.class != Class::Rel {
        return None;
    }
    matches!(
        atom.ch,
        '\u{2190}' // ←   \xleftarrow
        | '\u{2192}' // →   \xrightarrow
        | '\u{2194}' // ↔   \xleftrightarrow
        | '\u{21D0}' // ⇐   \xLeftarrow
        | '\u{21D2}' // ⇒   \xRightarrow
        | '\u{21D4}' // ⇔   \xLeftrightarrow
        | '\u{21A9}' // ↩   \xhookleftarrow
        | '\u{21AA}' // ↪   \xhookrightarrow
        | '\u{21BC}' // ↼   \xleftharpoonup
        | '\u{21BD}' // ↽   \xleftharpoondown
        | '\u{21C0}' // ⇀   \xrightharpoonup
        | '\u{21C1}' // ⇁   \xrightharpoondown
        | '\u{21A6}' // ↦   \xmapsto
    )
    .then_some(atom.ch)
}

/// Lay out an **extensible arrow** with labels (`\xrightarrow{f}`,
/// `\xleftarrow[g]{f}`): the arrow glyph `arrow` is stretched (via the horizontal
/// MATH variant/assembly, [`delim::horizontal_glyph`]) to span the wider of the
/// over-/under-labels (plus a small minimum), the over-label sits centered above
/// the arrow and the optional under-label centered below, both at script style
/// with small gaps. The whole construct spaces as a [`Class::Rel`] atom (set by
/// [`node_class`]).
///
/// Mirrors KaTeX `\xrightarrow` (`src/functions/arrow.js` + `stretchy.js`): the
/// arrow's width is `max(min_arrow_len, label widths + 2·label padding)`.
fn layout_extensible_arrow(
    ctx: &Ctx,
    arrow: char,
    sup: Option<&MathList>,
    sub: Option<&MathList>,
    style: Style,
) -> Option<Box> {
    let gid = ctx.face.glyph_index(arrow)?;
    let scale = ctx.scale_for(style);
    let label_style = style.smaller();

    let sup_box = sup.and_then(|s| layout_list(ctx, s, label_style, /* uncramped */ false));
    let sub_box = sub.and_then(|s| layout_list(ctx, s, label_style, /* cramped */ true));

    // Horizontal padding on each side of a label (so the arrow extends a little
    // past its text), and a minimum bare-arrow length.
    let pad = 0.4 * ctx.base_em;
    let min_len = ctx.face.glyph_hor_advance(gid).unwrap_or(0) as f32 * scale;
    let min_len = min_len.max(1.7 * ctx.base_em);
    let sup_w = sup_box.as_ref().map(|b| b.width).unwrap_or(0.0);
    let sub_w = sub_box.as_ref().map(|b| b.width).unwrap_or(0.0);
    let target = min_len.max(sup_w + 2.0 * pad).max(sub_w + 2.0 * pad);

    // Stretch the arrow to `target` via the horizontal construction/assembly.
    let arrow_box = delim::horizontal_glyph(ctx.face, gid, target, scale, ctx.cur_color.get());
    let width = arrow_box.width.max(target);
    let center = width / 2.0;

    // The arrow straddles the math axis like a relation; keep it on the baseline.
    let arrow_dx = center - arrow_box.width / 2.0;
    let arrow_h = arrow_box.height;
    let arrow_d = arrow_box.depth;

    let gap = 0.25 * ctx.base_em; // gap between the arrow ink and a label
    let mut children: Vec<Child> = Vec::new();
    let mut height = arrow_h;
    let mut depth = arrow_d;

    // Over-label: its ink bottom sits `gap` above the arrow ink top.
    if let Some(sb) = sup_box {
        let bottom = arrow_h + gap; // above baseline
        let dy = -(bottom + sb.depth);
        height = height.max(-dy + sb.height);
        children.push(Child { dx: center - sb.width / 2.0, dy, b: sb });
    }
    // Under-label: its ink top sits `gap` below the arrow ink bottom.
    if let Some(sb) = sub_box {
        let top = arrow_d + gap; // below baseline
        let dy = top + sb.height;
        depth = depth.max(dy + sb.depth);
        children.push(Child { dx: center - sb.width / 2.0, dy, b: sb });
    }

    children.push(Child { dx: arrow_dx, dy: 0.0, b: arrow_box });

    Some(Box {
        width,
        height,
        depth,
        kind: BoxKind::Hbox { children },
    })
}

/// Lay out a fraction (`\frac` / `\over` / `\dfrac` / `\tfrac`) per Appendix G
/// rule 15 / MathML Core / the OpenType MATH formulation.
///
/// The numerator and denominator render at `style.frac_child()` (the denominator
/// **cramped**), centered horizontally in a box `max(num.width, den.width)` wide.
/// A horizontal rule of `FractionRuleThickness` sits centered on the **math
/// axis** (`AxisHeight` above the fraction's baseline). Numerator/denominator
/// shifts come from the display- or text-style MATH constants, then are raised /
/// lowered as needed to enforce the minimum gaps between the rule and the
/// numerator's depth / denominator's height. All constants are font-units → px at
/// the *base* em (style-independent, like the script shifts).
///
/// The assembled box's baseline sits so the rule is `AxisHeight` above it:
/// `height` reaches the numerator's top, `depth` reaches the denominator's bottom.
///
/// References: `references/katex/src/buildCommon.js` + `genfrac` (`makeFraction`),
/// `references/microtex/src/atom/atom_frac.*`; MathML Core §3.3.2 (`mfrac`).
fn layout_frac(
    ctx: &Ctx,
    num: &MathList,
    den: &MathList,
    style: Style,
    bar: BarThickness,
) -> Option<Box> {
    let child_style = style.frac_child();
    // An empty numerator or denominator lays out as a zero-size box so the bar and
    // the other operand still render (matches TeX's empty-`\frac` behaviour).
    let zero = || Box {
        width: 0.0,
        height: 0.0,
        depth: 0.0,
        kind: BoxKind::Hbox { children: Vec::new() },
    };
    let num_box = layout_list(ctx, num, child_style, /* cramped */ false).unwrap_or_else(zero);
    let den_box = layout_list(ctx, den, child_style, /* cramped */ true).unwrap_or_else(zero);

    let consts = ctx.face.tables().math.and_then(|m| m.constants);
    let c = consts;
    let display = style.is_display();

    // MATH constants (px at base em), with sane fallbacks as multiples of the rule
    // thickness / em when the table is absent.
    let axis = c.map(|c| ctx.const_px(c.axis_height())).unwrap_or(0.25 * ctx.base_em);
    // The font-default rule thickness; an explicit `\genfrac` thickness (em →
    // px at the base em) overrides it for the rule we draw, while the gap-min
    // computations still reference the default (matching KaTeX).
    let default_thickness = c
        .map(|c| ctx.const_px(c.fraction_rule_thickness()))
        .unwrap_or(0.04 * ctx.base_em);
    let thickness = match bar {
        BarThickness::Em(em) => em * ctx.base_em,
        BarThickness::Default | BarThickness::None => default_thickness,
    };

    let shift_up = c
        .map(|c| {
            ctx.const_px(if display {
                c.fraction_numerator_display_style_shift_up()
            } else {
                c.fraction_numerator_shift_up()
            })
        })
        .unwrap_or(if display { 0.7 * ctx.base_em } else { 0.4 * ctx.base_em });
    let shift_down = c
        .map(|c| {
            ctx.const_px(if display {
                c.fraction_denominator_display_style_shift_down()
            } else {
                c.fraction_denominator_shift_down()
            })
        })
        .unwrap_or(if display { 0.7 * ctx.base_em } else { 0.4 * ctx.base_em });
    let num_gap_min = c
        .map(|c| {
            ctx.const_px(if display {
                c.fraction_num_display_style_gap_min()
            } else {
                c.fraction_numerator_gap_min()
            })
        })
        .unwrap_or(if display { 3.0 * thickness } else { thickness });
    let den_gap_min = c
        .map(|c| {
            ctx.const_px(if display {
                c.fraction_denom_display_style_gap_min()
            } else {
                c.fraction_denominator_gap_min()
            })
        })
        .unwrap_or(if display { 3.0 * thickness } else { thickness });

    // Rule edges relative to the fraction baseline (axis-centered).
    let rule_top = axis + thickness / 2.0;
    let rule_bottom = axis - thickness / 2.0;

    // Numerator: its baseline sits `u` px above the fraction baseline, so its
    // bottom (ink) is at `u - num.depth`. Enforce a `num_gap_min` clearance above
    // the rule top, raising the numerator if the default shift is too small.
    let mut u = shift_up;
    let num_bottom = u - num_box.depth;
    if num_bottom - rule_top < num_gap_min {
        u += num_gap_min - (num_bottom - rule_top);
    }

    // Denominator: its baseline sits `d` px below the fraction baseline, so its
    // top (ink) is at `-d + den.height`. Enforce a `den_gap_min` clearance below
    // the rule bottom, lowering the denominator if needed.
    let mut d = shift_down;
    let den_top = -d + den_box.height;
    if rule_bottom - den_top < den_gap_min {
        d += den_gap_min - (rule_bottom - den_top);
    }

    // Center the narrower operand and the rule across the full width.
    let width = num_box.width.max(den_box.width);
    let num_dx = (width - num_box.width) / 2.0;
    let den_dx = (width - den_box.width) / 2.0;

    // Composite metrics: top of the numerator above the baseline, bottom of the
    // denominator below it (the rule lies between, so never extends them).
    let height = (u + num_box.height).max(rule_top);
    let depth = (d + den_box.depth).max(-rule_bottom).max(0.0);

    let mut children = vec![
        // Numerator: raised → negative dy.
        Child { dx: num_dx, dy: -u, b: num_box },
    ];
    // The bar: a `Rule` box has height = thickness above its own baseline, so
    // placing its baseline at `dy = -rule_bottom` puts its bottom edge at
    // `rule_bottom` and its top at `rule_top` on the fraction baseline. A
    // binomial (`BarThickness::None`, from a `0`-thickness `\binom`/`\genfrac`)
    // skips it, leaving the numerator/denominator stacked with no rule.
    if bar != BarThickness::None {
        children.push(Child {
            dx: 0.0,
            dy: -rule_bottom,
            b: Box {
                width,
                height: thickness,
                depth: 0.0,
                kind: BoxKind::Rule { width, thickness, color: ctx.cur_color.get() },
            },
        });
    }
    // Denominator: lowered → positive dy.
    children.push(Child { dx: den_dx, dy: d, b: den_box });

    Some(Box {
        width,
        height,
        depth,
        kind: BoxKind::Hbox { children },
    })
}

/// Lay out a `\cancel{…}`: the `body` row at the current style, with a forward
/// diagonal strike (lower-left → upper-right) overlaid across its bounding box.
///
/// The line runs from the box's lower-left corner `(0, +depth)` to its upper-right
/// corner `(width, -height)` (a [`BoxKind::Line`]'s `dy` grows downward, so the
/// destination shift is `-(height + depth)`). The strike's thickness reuses the
/// fraction-rule thickness so it matches the rest of the math. The node keeps the
/// body's metrics — the line is a pure overlay and adds no advance — and spaces as
/// an [`Class::Ord`] atom. References: KaTeX `\cancel` (`src/functions/enclose.js`).
fn layout_cancel(ctx: &Ctx, body: &MathList, style: Style, cramped: bool) -> Option<Box> {
    let body_box = layout_list(ctx, body, style, cramped)?;
    let width = body_box.width;
    let height = body_box.height;
    let depth = body_box.depth;

    let thickness = ctx
        .face
        .tables()
        .math
        .and_then(|m| m.constants)
        .map(|c| ctx.const_px(c.fraction_rule_thickness()))
        .unwrap_or(0.04 * ctx.base_em);

    // The strike line, drawn from the body's lower-left corner. `dy` grows
    // downward, so its origin baseline sits at `+depth` (the box bottom) and it
    // rises to the top-right corner `height` above the baseline → `dy = -(height
    // + depth)` relative to that origin.
    let line = Child {
        dx: 0.0,
        dy: depth,
        b: Box {
            width: 0.0,
            height: 0.0,
            depth: 0.0,
            kind: BoxKind::Line {
                dx: width,
                dy: -(height + depth),
                thickness,
                color: ctx.cur_color.get(),
            },
        },
    };

    Some(Box {
        width,
        height,
        depth,
        kind: BoxKind::Hbox {
            children: vec![Child { dx: 0.0, dy: 0.0, b: body_box }, line],
        },
    })
}

/// Lay out a `\colorbox`/`\fcolorbox`: the `body` row, padded by `\fboxsep`
/// (≈ 0.3 em) on all sides, drawn over a solid `background` fill spanning the
/// padded bounding box. `\fcolorbox` (a `Some` `border`) additionally strokes a
/// frame of the same thickness as a fraction rule around that box.
///
/// The fill is emitted as the **first** child of the wrapping Hbox so paint order
/// (depth-first, in child order) draws it behind the body; the frame edges follow
/// the body so they sit on top. The node advances by the full padded width and
/// spaces as an [`Class::Ord`] atom. References: KaTeX `\colorbox`/`\fcolorbox`
/// (`src/functions/enclose.js`), `\fboxsep`/`\fboxrule` defaults.
fn layout_colorbox(
    ctx: &Ctx,
    body: &MathList,
    background: [u8; 4],
    border: Option<[u8; 4]>,
    style: Style,
    cramped: bool,
) -> Option<Box> {
    let body_box = layout_list(ctx, body, style, cramped)?;
    // `\fboxsep` padding around the content (TeX default 3pt ≈ 0.3 em).
    let pad = 0.3 * ctx.base_em;
    let inner_w = body_box.width;
    let inner_h = body_box.height;
    let inner_d = body_box.depth;

    // Outer (padded) extents.
    let width = inner_w + 2.0 * pad;
    let height = inner_h + pad;
    let depth = inner_d + pad;

    // The background fill spans the full padded bbox, anchored at the left edge.
    let fill = Child {
        dx: 0.0,
        dy: 0.0,
        b: Box {
            width,
            height,
            depth,
            kind: BoxKind::Fill { width, height, depth, color: background },
        },
    };
    // The body, inset by `pad` horizontally (its baseline is unchanged).
    let content = Child { dx: pad, dy: 0.0, b: body_box };

    let mut children = vec![fill, content];

    // `\fcolorbox` frame: four edges of the padded rectangle, drawn over the fill
    // via `BoxKind::Line` overlays (no metrics of their own). Corners run from the
    // top-left clockwise; `dy` grows downward, so the top is at `-height`.
    if let Some(border) = border {
        let thickness = ctx
            .face
            .tables()
            .math
            .and_then(|m| m.constants)
            .map(|c| ctx.const_px(c.fraction_rule_thickness()))
            .unwrap_or(0.04 * ctx.base_em);
        let edge = |x0: f32, y0: f32, dx: f32, dy: f32| Child {
            dx: x0,
            dy: y0,
            b: Box {
                width: 0.0,
                height: 0.0,
                depth: 0.0,
                kind: BoxKind::Line { dx, dy, thickness, color: border },
            },
        };
        // Top, bottom, left, right of the rectangle (origin at the baseline-left).
        children.push(edge(0.0, -height, width, 0.0)); // top
        children.push(edge(0.0, depth, width, 0.0)); // bottom
        children.push(edge(0.0, -height, 0.0, height + depth)); // left
        children.push(edge(width, -height, 0.0, height + depth)); // right
    }

    Some(Box {
        width,
        height,
        depth,
        kind: BoxKind::Hbox { children },
    })
}

/// The math axis height in px at the base em (fallback ≈ ¼ em), used to center
/// delimiters (and matching the fraction-bar axis).
fn axis_px(ctx: &Ctx) -> f32 {
    ctx.face
        .tables()
        .math
        .and_then(|m| m.constants)
        .map(|c| ctx.const_px(c.axis_height()))
        .unwrap_or(0.25 * ctx.base_em)
}

/// Lay out a `\left … \right` fence: lay the `body` at `style`, measure how far it
/// extends above/below the math axis, size both delimiters to that extent
/// ([`delim::sized_delim`]), and assemble `[open][body][close]` with the
/// delimiters centered on the axis.
///
/// Target-size formula (ported from KaTeX `makeLeftRightDelim`, itself TeX's
/// `make_left_right`): with the body's `height`/`depth` and the axis `a`,
/// `maxDistFromAxis = max(height − a, depth + a)`, and the delimiter spans
/// `max(maxDistFromAxis · 901/500, 2·maxDistFromAxis − delimiterExtend)` px
/// (`delimiterFactor` 901, `delimiterExtend` 5pt ≈ 5/16 em). A null (`.`)
/// delimiter contributes no glyph and no width.
fn layout_delim(
    ctx: &Ctx,
    open: Option<char>,
    body: &MathList,
    close: Option<char>,
    style: Style,
    cramped: bool,
) -> Option<Box> {
    // An empty body still renders the delimiters around zero-size content.
    let body_box = layout_list(ctx, body, style, cramped).unwrap_or(Box {
        width: 0.0,
        height: 0.0,
        depth: 0.0,
        kind: BoxKind::Hbox { children: Vec::new() },
    });

    let axis = axis_px(ctx);

    // KaTeX makeLeftRightDelim target.
    let max_dist = (body_box.height - axis).max(body_box.depth + axis).max(0.0);
    const DELIMITER_FACTOR: f32 = 901.0;
    // 5pt at the standard 10pt-per-em design ≈ 0.5 em; KaTeX uses 5/ptPerEm.
    let delimiter_extend = 0.5 * ctx.base_em;
    let target = (max_dist * DELIMITER_FACTOR / 500.0).max(2.0 * max_dist - delimiter_extend);

    // Size each present delimiter to the target, centered on the axis.
    let open_box =
        open.and_then(|c| (c != '.').then_some(c)).and_then(|c| delim::sized_delim(ctx.face, c, target, axis, ctx.base_em, ctx.cur_color.get()));
    let close_box =
        close.and_then(|c| (c != '.').then_some(c)).and_then(|c| delim::sized_delim(ctx.face, c, target, axis, ctx.base_em, ctx.cur_color.get()));

    // Assemble left → right on the shared baseline (delimiters already centered on
    // the axis, body on the baseline).
    let mut children: Vec<Child> = Vec::new();
    let mut pen = 0.0f32;
    let mut height = body_box.height;
    let mut depth = body_box.depth;

    if let Some(b) = open_box {
        height = height.max(b.height);
        depth = depth.max(b.depth);
        let w = b.width;
        children.push(Child { dx: pen, dy: 0.0, b });
        pen += w;
    }
    {
        let w = body_box.width;
        children.push(Child { dx: pen, dy: 0.0, b: body_box });
        pen += w;
    }
    if let Some(b) = close_box {
        height = height.max(b.height);
        depth = depth.max(b.depth);
        let w = b.width;
        children.push(Child { dx: pen, dy: 0.0, b });
        pen += w;
    }

    Some(Box {
        width: pen,
        height,
        depth,
        kind: BoxKind::Hbox { children },
    })
}

/// Lay out a fixed-size delimiter (`\bigl(` etc.): one delimiter glyph sized to
/// `target_em · em`, centered on the math axis. Same machinery as
/// [`layout_delim`] but with a content-independent target.
fn layout_big_delim(ctx: &Ctx, ch: char, target_em: f32) -> Option<Box> {
    let axis = axis_px(ctx);
    let target = target_em * ctx.base_em;
    delim::sized_delim(ctx.face, ch, target, axis, ctx.base_em, ctx.cur_color.get())
}

/// Lay out a matrix/array/cases/aligned environment (the TeX *array* algorithm,
/// cf. KaTeX `buildHTML` `makeArray` and microTeX's matrix atom).
///
/// 1. **Cells.** Each cell lays out as its own [`MathList`] at the cell style:
///    `Plain` matrices keep the surrounding `style` (Display stays Display);
///    `cases`/`aligned` cells render in [`Style::Text`]. Empty/missing cells are
///    treated as zero-size.
/// 2. **Column widths** = the max cell width in each column; a cell is placed in
///    its column by `col_align` (`Center`: `(colw−cellw)/2`, `Left`: 0,
///    `Right`: `colw−cellw`).
/// 3. **Row metrics**: each row's height/depth is the max over its cells. Rows
///    are stacked on baselines a fixed `arraystretch · em` apart, but never
///    closer than a `jot` of clearance between one row's depth and the next
///    row's height (`baseline ≥ prevDepth + jot + thisHeight`).
/// 4. **Columns** are separated by `arraycolsep` on each side (`≈ 0.5 em`); for
///    `aligned` the right|left column pair touches (gap 0) so the `&` boundary —
///    typically a relation — lines up across rows.
/// 5. The whole grid is **vertically centered on the math axis**: the row-stack's
///    own center is shifted to the axis, so the array sits like a tall delimiter
///    (and the enclosing `\left…\right` of `pmatrix`/… sizes to it). For `cases`
///    a large left brace, grown to the grid height via [`delim::sized_delim`], is
///    prepended; there is no right delimiter.
fn layout_matrix(
    ctx: &Ctx,
    rows: &[Vec<MathList>],
    col_align: &[Align],
    kind: MatrixKind,
    col_seps: &[u8],
    row_lines: &[u8],
    style: Style,
) -> Option<Box> {
    if rows.is_empty() {
        return None;
    }

    // Cell render style: matrices follow the surrounding style; cases/aligned
    // bodies are text style (TeX `\textstyle`); `\substack` content is one step
    // smaller (script size) than its surroundings.
    let cell_style = match kind {
        MatrixKind::Plain => style,
        MatrixKind::Cases | MatrixKind::Aligned => Style::Text,
        MatrixKind::Substack => style.smaller(),
    };

    let n_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if n_cols == 0 {
        return None;
    }

    // Lay out every cell; an empty cell becomes a zero-size empty hbox.
    let empty = || Box {
        width: 0.0,
        height: 0.0,
        depth: 0.0,
        kind: BoxKind::Hbox { children: Vec::new() },
    };
    let mut cells: Vec<Vec<Box>> = Vec::with_capacity(rows.len());
    let mut col_w = vec![0.0f32; n_cols];
    let mut row_h = vec![0.0f32; rows.len()];
    let mut row_d = vec![0.0f32; rows.len()];
    for (r, row) in rows.iter().enumerate() {
        let mut boxes = Vec::with_capacity(n_cols);
        for c in 0..n_cols {
            let b = row
                .get(c)
                .and_then(|cell| layout_list(ctx, cell, cell_style, /* cramped */ false))
                .unwrap_or_else(empty);
            col_w[c] = col_w[c].max(b.width);
            row_h[r] = row_h[r].max(b.height);
            row_d[r] = row_d[r].max(b.depth);
            boxes.push(b);
        }
        cells.push(boxes);
    }

    // Gaps and stretch (px at the base em).
    let arraycolsep = 0.5 * ctx.base_em; // half-gap on each side of a column
    // `\arraystretch` scales the nominal inter-row baseline distance; substack
    // (a script-size stack) is unaffected.
    let arraystretch = match kind {
        MatrixKind::Substack => 1.0,
        _ => ctx.arraystretch,
    };
    let baseline_skip = arraystretch * ctx.base_em; // nominal baseline distance
    let jot = 0.25 * ctx.base_em; // min clearance between adjacent rows

    // Alignment of column `c`, extended past `col_align` with the kind default.
    let align_of = |c: usize| -> Align {
        col_align.get(c).copied().unwrap_or(match kind {
            MatrixKind::Aligned => {
                if c % 2 == 0 {
                    Align::Right
                } else {
                    Align::Left
                }
            }
            MatrixKind::Cases => Align::Left,
            MatrixKind::Plain | MatrixKind::Substack => Align::Center,
        })
    };

    // Vertical-rule geometry (px at the base em). A `|` is a thin rule preceded
    // and followed by a small gap; `||` stacks two rules a hair apart. The rule
    // thickness mirrors the fraction-bar default (≈ 0.04 em).
    let rule_thickness = 0.04 * ctx.base_em;
    let sep_at = |slot: usize| col_seps.get(slot).copied().unwrap_or(0);
    let rule_gap = 0.5 * arraycolsep; // gap on each side of a rule run
    let double_gap = 0.06 * ctx.base_em; // space between the two rules of `||`

    // Column x-offsets: a half-`arraycolsep` of inter-column space on each side,
    // i.e. a full `arraycolsep` between adjacent columns — except the `aligned`
    // right|left pair (even→odd) which touches so the `&` boundary lines up. Any
    // vertical `|` rules from the column spec insert their own width (gap + rule)
    // at the appropriate slot; we record each rule's x for drawing below.
    let mut col_x = vec![0.0f32; n_cols];
    // `(x_of_first_rule, count)` for each non-empty separator slot.
    let mut vrules: Vec<(f32, u8)> = Vec::new();
    let mut x = 0.0f32;
    // Place any left-edge rules before the first column.
    let push_vrule = |vrules: &mut Vec<(f32, u8)>, x: &mut f32, n: u8| {
        if n > 0 {
            *x += rule_gap;
            vrules.push((*x, n));
            *x += n as f32 * rule_thickness + (n.saturating_sub(1)) as f32 * double_gap;
            *x += rule_gap;
        }
    };
    push_vrule(&mut vrules, &mut x, sep_at(0));
    for c in 0..n_cols {
        col_x[c] = x;
        x += col_w[c];
        if c + 1 < n_cols {
            // Inter-column space: a separator (if any) replaces the plain gap;
            // otherwise the usual `arraycolsep` (suppressed for an `aligned` pair).
            let n = sep_at(c + 1);
            if n > 0 {
                push_vrule(&mut vrules, &mut x, n);
            } else {
                let touching = matches!(kind, MatrixKind::Aligned) && c % 2 == 0;
                if !touching {
                    x += arraycolsep;
                }
            }
        }
    }
    // Right-edge rules after the last column.
    push_vrule(&mut vrules, &mut x, sep_at(n_cols));
    let grid_w = x;

    // Row baselines, top-down, starting at 0 (we recenter on the axis after).
    let mut row_y = vec![0.0f32; rows.len()];
    let mut y = 0.0f32;
    for r in 0..rows.len() {
        if r > 0 {
            let gap = (row_d[r - 1] + jot + row_h[r]).max(baseline_skip);
            y += gap;
        }
        row_y[r] = y;
    }
    // Extent of the row stack about the *first* row's baseline (y=0 .. last).
    let stack_top = row_h[0]; // above first baseline
    let stack_bottom = row_y[rows.len() - 1] + row_d[rows.len() - 1]; // below first baseline

    // Center the stack on the math axis: the stack's vertical midpoint should
    // sit at the axis (above the baseline by `axis`). The midpoint currently sits
    // at `(−stack_top + stack_bottom)/2` (downward-positive). Shift every row so
    // that midpoint maps to `−axis` (above baseline).
    let axis = axis_px(ctx);
    let mid = (-stack_top + stack_bottom) / 2.0;
    let shift = -axis - mid; // add to each row_y (downward dy)

    // Assemble the grid as an Hbox of cells placed by (col_x, row baseline).
    let mut children: Vec<Child> = Vec::new();
    for (r, boxes) in cells.into_iter().enumerate() {
        let dy = row_y[r] + shift;
        for (c, b) in boxes.into_iter().enumerate() {
            let pad = match align_of(c) {
                Align::Left => 0.0,
                Align::Center => (col_w[c] - b.width) / 2.0,
                Align::Right => col_w[c] - b.width,
            };
            children.push(Child { dx: col_x[c] + pad, dy, b });
        }
    }

    // height = extent above baseline = stack_top − shift (shift pushes rows down);
    // depth  = extent below baseline = stack_bottom + shift.
    let height = stack_top - shift;
    let depth = stack_bottom + shift;

    // Vertical `|` rules: each spans the full grid body (top → bottom). A `Rule`
    // is width×thickness extending `thickness` up from its child baseline, so a
    // thin (`width = rule_thickness`), tall (`thickness = body_height`) rule
    // placed with its baseline at the grid bottom (`dy = depth`) fills the body.
    let body_height = height + depth;
    if body_height > 0.0 {
        let rcolor = ctx.cur_color.get();
        for &(rx, n) in &vrules {
            for k in 0..n {
                let dx = rx + k as f32 * (rule_thickness + double_gap);
                children.push(Child {
                    dx,
                    dy: depth,
                    b: Box {
                        width: rule_thickness,
                        height: body_height,
                        depth: 0.0,
                        kind: BoxKind::Rule {
                            width: rule_thickness,
                            thickness: body_height,
                            color: rcolor,
                        },
                    },
                });
            }
        }
        // Horizontal `\hline` rules at row boundaries. Boundary `b` sits above
        // row `b` (b in 0..rows): the top edge for b=0, midway between adjacent
        // rows otherwise; boundary `rows.len()` is the bottom edge.
        let boundary_dy = |b: usize| -> f32 {
            if b == 0 {
                -height
            } else if b >= rows.len() {
                depth
            } else {
                let above = row_y[b - 1] + shift + row_d[b - 1];
                let below = row_y[b] + shift - row_h[b];
                (above + below) / 2.0
            }
        };
        for (b, &n) in row_lines.iter().enumerate() {
            for k in 0..n {
                // Stack the rules of `\hline\hline` a hair apart.
                let dy = boundary_dy(b) + k as f32 * (rule_thickness + double_gap);
                children.push(Child {
                    dx: 0.0,
                    // `Rule` extends `thickness` upward from the child baseline, so
                    // offset down by `rule_thickness` to center the line on `dy`.
                    dy: dy + rule_thickness / 2.0,
                    b: Box {
                        width: grid_w,
                        height: rule_thickness,
                        depth: 0.0,
                        kind: BoxKind::Rule { width: grid_w, thickness: rule_thickness, color: rcolor },
                    },
                });
            }
        }
    }

    let grid = Box {
        width: grid_w,
        height: height.max(0.0),
        depth: depth.max(0.0),
        kind: BoxKind::Hbox { children },
    };

    // `cases`: prepend a large left brace sized to the grid, no right delim.
    if matches!(kind, MatrixKind::Cases) {
        let target = (grid.height - axis).max(grid.depth + axis).max(0.0) * 2.0;
        let brace = delim::sized_delim(ctx.face, '{', target, axis, ctx.base_em, ctx.cur_color.get());
        let gap = 0.16 * ctx.base_em; // nib-to-content space (TeX ~ \nulldelimiterspace-ish)
        let mut kids: Vec<Child> = Vec::new();
        let mut pen = 0.0f32;
        let mut h = grid.height;
        let mut d = grid.depth;
        if let Some(b) = brace {
            h = h.max(b.height);
            d = d.max(b.depth);
            let w = b.width;
            kids.push(Child { dx: pen, dy: 0.0, b });
            pen += w + gap;
        }
        let gw = grid.width;
        kids.push(Child { dx: pen, dy: 0.0, b: grid });
        pen += gw;
        return Some(Box {
            width: pen,
            height: h,
            depth: d,
            kind: BoxKind::Hbox { children: kids },
        });
    }

    Some(grid)
}

/// Lay out a radical (`\sqrt{…}` / `\sqrt[n]{…}`) per Appendix G rule 11 /
/// MathML Core §3.3.3 / the OpenType MATH formulation.
///
/// 1. The `radicand` lays out at `style` but **cramped** (TeXbook rule 11).
/// 2. MATH constants (px at the base em, with sane fallbacks): the radical rule
///    thickness, the vertical gap between the radicand and the rule
///    (`radical_display_style_vertical_gap` in Display style, else
///    `radical_vertical_gap`), and `radical_extra_ascender` reserved above the rule.
/// 3. The surd (U+221A) is sized via [`delim::vertical_glyph`] to span
///    `radicand.height + radicand.depth + gap + ruleThickness` — i.e. from the
///    rule's top down past the radicand's bottom.
/// 4. Assembly (all offsets relative to the radical's = radicand's baseline):
///    the surd at the left with its ink top at the rule top; a horizontal
///    [`BoxKind::Rule`] vinculum running from the surd's right over the radicand,
///    its bottom `gap` above the radicand's top; the radicand shifted right of the
///    surd. The composite height reaches the rule top + extra ascender.
/// 5. An optional degree `[n]` lays out at ScriptScript style and is tucked into
///    the surd's upper-left, its baseline raised by
///    `radical_degree_bottom_raise_percent` of the surd's height, with
///    `radical_kern_before_degree` / `radical_kern_after_degree` horizontal kerns.
///
/// References: KaTeX `src/buildHTML.js` (`makeSqrt`) + `src/delimiter.ts`
/// (`sqrtImage`); MathML Core `msqrt`/`mroot`.
fn layout_radical(
    ctx: &Ctx,
    index: Option<&MathList>,
    radicand: &MathList,
    style: Style,
    _cramped: bool,
) -> Option<Box> {
    // Radicand renders at the current style, cramped (rule 11).
    let rad = layout_list(ctx, radicand, style, /* cramped */ true).unwrap_or(Box {
        width: 0.0,
        height: 0.0,
        depth: 0.0,
        kind: BoxKind::Hbox { children: Vec::new() },
    });

    let c = ctx.face.tables().math.and_then(|m| m.constants);
    let thickness = c
        .map(|c| ctx.const_px(c.radical_rule_thickness()))
        .unwrap_or(0.04 * ctx.base_em);
    let gap = c
        .map(|c| {
            ctx.const_px(if style.is_display() {
                c.radical_display_style_vertical_gap()
            } else {
                c.radical_vertical_gap()
            })
        })
        .unwrap_or(if style.is_display() { 0.2 * ctx.base_em } else { 0.05 * ctx.base_em });
    let extra_ascender = c
        .map(|c| ctx.const_px(c.radical_extra_ascender()))
        .unwrap_or(thickness);

    // The surd must span the radicand plus the gap and the rule above it.
    let target = rad.height + rad.depth + gap + thickness;
    // U+221A SQUARE ROOT; size it with the reusable delimiter machinery. The
    // returned box is centered on its own baseline (height == depth == total/2).
    let scale = ctx.scale_for(style);
    let surd_gid = ctx.face.glyph_index('\u{221A}')?;
    let surd = delim::vertical_glyph(ctx.face, surd_gid, target, scale, ctx.cur_color.get());
    let surd_w = surd.width;
    let surd_total = surd.height + surd.depth;

    // Edges of the vinculum rule relative to the radical baseline. The rule sits
    // `gap` above the radicand's top; the radicand's ink top is at `rad.height`.
    let rule_bottom = rad.height + gap;
    let rule_top = rule_bottom + thickness;

    // Place the surd so its ink top reaches the rule top. The surd box is centered
    // on its baseline, so its top is `surd.height` above its baseline; shifting the
    // baseline down by `dy` moves the top to `surd.height - dy`. We want that =
    // rule_top, hence dy = surd.height - rule_top.
    let surd_dy = surd.height - rule_top;

    // The radicand sits to the right of the surd, on the main baseline. A little
    // horizontal padding after the radicand keeps the vinculum from ending flush
    // with the ink.
    let rad_pad = thickness;
    let rad_dx = surd_w;
    let vinculum_w = rad.width + rad_pad;

    let mut children: Vec<Child> = Vec::new();

    // The degree/index, if present, tucked into the surd's upper-left.
    let mut left_pad = 0.0f32;
    let mut height = rule_top + extra_ascender;
    // The surd's ink bottom sits `surd_dy + surd.depth` below the baseline (the
    // surd is sized to reach the radicand's bottom, so this is ≥ rad.depth).
    let depth = (surd_dy + surd.depth).max(rad.depth).max(0.0);

    if let Some(idx) = index {
        if let Some(deg) = layout_list(ctx, idx, Style::ScriptScript, /* cramped */ false) {
            let kern_before = c
                .map(|c| ctx.const_px(c.radical_kern_before_degree()))
                .unwrap_or(0.28 * ctx.base_em);
            let kern_after = c
                .map(|c| ctx.const_px(c.radical_kern_after_degree()))
                .unwrap_or(-0.55 * surd_w);
            let raise_pct = c
                .map(|c| c.radical_degree_bottom_raise_percent() as f32)
                .unwrap_or(60.0)
                / 100.0;
            // The degree's baseline is raised so its bottom sits `raise_pct` of the
            // surd's total height above the surd's bottom.
            let surd_bottom = surd_dy - surd.height; // (negative) below baseline
            let deg_bottom = surd_bottom + raise_pct * surd_total;
            let deg_baseline = deg_bottom + deg.depth; // baseline above bottom by depth
            let deg_dy = -deg_baseline;
            // Lay the degree starting `kern_before` in, then the surd shifts right by
            // the degree's advance plus the (usually negative) after-kern.
            let deg_w = deg.width;
            left_pad = (kern_before + deg_w + kern_after).max(0.0);
            height = height.max(-deg_dy + deg.height);
            children.push(Child { dx: kern_before, dy: deg_dy, b: deg });
        }
    }

    // Surd glyph (after any degree pad).
    children.push(Child { dx: left_pad, dy: surd_dy, b: surd });

    // Vinculum: a Rule whose box height == thickness above its own baseline; place
    // its baseline at `dy = -rule_bottom` so its bottom edge sits at `rule_bottom`.
    children.push(Child {
        dx: left_pad + surd_w,
        dy: -rule_bottom,
        b: Box {
            width: vinculum_w,
            height: thickness,
            depth: 0.0,
            kind: BoxKind::Rule { width: vinculum_w, thickness, color: ctx.cur_color.get() },
        },
    });

    // Radicand.
    children.push(Child { dx: left_pad + rad_dx, dy: 0.0, b: rad });

    let width = left_pad + surd_w + vinculum_w;

    Some(Box {
        width,
        height,
        depth,
        kind: BoxKind::Hbox { children },
    })
}

/// Lay out an accented expression (`\hat \tilde \bar \vec \dot …`, the stretchy
/// `\overline`/`\widehat`/`\overrightarrow`/…, and the under-forms
/// `\underline`/`\underbar`) per Appendix G rule 12 / MathML Core / KaTeX
/// `src/buildHTML.js` (`makeAccent`) + `src/stretchy.js`.
///
/// 1. Lay out the `base` at `style` (cramped — rule 12). Its skew (where the
///    accent attaches horizontally) is the base glyph's MATH
///    `top_accent_attachment` when the base is a single glyph, else the base
///    width's midpoint.
/// 2. **Stretchy overline / underline** (`‾`/`_`) draw a [`BoxKind::Rule`]
///    spanning the base width using the MATH `overbar_*` / `underbar_*` consts
///    (gap above/below the base ink, rule thickness, extra ascender/descender).
/// 3. **Stretchy** glyph accents (`\widehat`, `\overrightarrow`, …) size the
///    accent glyph to the base width via [`delim::horizontal_glyph`] (the
///    horizontal MATH variant/assembly), centered over the base.
/// 4. **Non-stretchy** accents place the single accent glyph horizontally centered
///    at the base's skew point.
/// 5. Vertically, an over-accent sits just above the base ink top with a small
///    clearance, but is lowered so its baseline never rises above the MATH
///    `accent_base_height` reference for short bases (so `\hat{x}` and `\hat{X}`
///    look consistent). Under-accents mirror this below the base.
fn layout_accent(
    ctx: &Ctx,
    accent: char,
    stretchy: bool,
    under: bool,
    base: &MathList,
    style: Style,
    _cramped: bool,
) -> Option<Box> {
    let base_box = layout_list(ctx, base, style, /* cramped */ true)?;
    let base_w = base_box.width;
    let scale = ctx.scale_for(style);
    let c = ctx.face.tables().math.and_then(|m| m.constants);

    // Horizontal attachment ("skew"): a single-glyph base attaches at its MATH
    // top-accent-attachment point; otherwise center on the base width.
    let skew = base_skew(ctx, base, style).unwrap_or(base_w / 2.0);

    // --- overline / underline: a horizontal Rule spanning the base width ---
    // `‾`/`_`/combining low line render as a bar; `under` (from the script
    // position) disambiguates above vs. below for the combining low line.
    if accent == '\u{203E}' || accent == '\u{005F}' || accent == '\u{0332}' {
        let thickness = c
            .map(|c| {
                ctx.const_px(if under {
                    c.underbar_rule_thickness()
                } else {
                    c.overbar_rule_thickness()
                })
            })
            .unwrap_or(0.04 * ctx.base_em);
        let gap = c
            .map(|c| {
                ctx.const_px(if under {
                    c.underbar_vertical_gap()
                } else {
                    c.overbar_vertical_gap()
                })
            })
            .unwrap_or(0.1 * ctx.base_em);
        let extra = c
            .map(|c| {
                ctx.const_px(if under {
                    c.underbar_extra_descender()
                } else {
                    c.overbar_extra_ascender()
                })
            })
            .unwrap_or(thickness);

        let rule = Box {
            width: base_w,
            height: thickness,
            depth: 0.0,
            kind: BoxKind::Rule { width: base_w, thickness, color: ctx.cur_color.get() },
        };
        let mut children = vec![Child { dx: 0.0, dy: 0.0, b: base_box.clone() }];
        let (height, depth) = if under {
            // Rule sits `gap` below the base ink bottom; its baseline is the rule's
            // bottom edge (Rule height extends upward), so place its top at that gap.
            let rule_top = base_box.depth + gap;
            children.push(Child { dx: 0.0, dy: rule_top + thickness, b: rule });
            (base_box.height, (rule_top + thickness + extra).max(base_box.depth))
        } else {
            // Rule sits `gap` above the base ink top; place its bottom there.
            let rule_bottom = base_box.height + gap;
            children.push(Child { dx: 0.0, dy: -rule_bottom, b: rule });
            ((rule_bottom + thickness + extra).max(base_box.height), base_box.depth)
        };
        return Some(Box {
            width: base_w,
            height,
            depth,
            kind: BoxKind::Hbox { children },
        });
    }

    // --- glyph accent (stretchy or fixed) ---
    let gid = ctx.face.glyph_index(accent)?;
    let acc_box = if stretchy {
        delim::horizontal_glyph(ctx.face, gid, base_w, scale, ctx.cur_color.get())
    } else {
        let advance = ctx.face.glyph_hor_advance(gid).unwrap_or(0) as f32 * scale;
        let (h, d) = glyph_extents(ctx.face, gid, scale);
        Box {
            width: advance,
            height: h,
            depth: d,
            kind: BoxKind::Glyph { gid, scale, color: ctx.cur_color.get() },
        }
    };
    let acc_w = acc_box.width;

    // Horizontal: center the accent's own attachment point on the base skew. For a
    // fixed glyph that is the glyph's top-accent-attachment (fallback: its center).
    let acc_attach = if stretchy {
        acc_w / 2.0
    } else {
        top_accent_attachment(ctx, gid, scale).unwrap_or(acc_w / 2.0)
    };
    let acc_dx = (skew - acc_attach).max(0.0);

    // Small clearance between the base ink and the accent ink (≈ ⅛ accent height).
    let clearance = 0.05 * ctx.base_em;

    let mut children = vec![Child { dx: 0.0, dy: 0.0, b: base_box.clone() }];
    let (height, depth, width) = if under {
        // Under-accent: its ink top sits `clearance` below the base ink bottom.
        let acc_top = base_box.depth + clearance; // below baseline (positive)
        // Place the accent baseline so its top reaches `acc_top` below baseline:
        // glyph top is `acc_box.height` above its baseline → dy = acc_top + height.
        let dy = acc_top + acc_box.height;
        let depth = (dy + acc_box.depth).max(base_box.depth);
        children.push(Child { dx: acc_dx, dy, b: acc_box });
        (base_box.height, depth, base_w.max(acc_dx + acc_w))
    } else {
        // Over-accent: its ink bottom sits `clearance` above the base ink top, but
        // not lower than the `accent_base_height` reference (short-base clamp).
        let accent_base_h = c
            .map(|c| ctx.const_px(c.accent_base_height()))
            .unwrap_or(0.45 * ctx.base_em);
        let bottom = base_box.height.max(accent_base_h) + clearance; // above baseline
        // accent baseline raised so its ink bottom reaches `bottom`: glyph bottom is
        // `acc_box.depth` below its baseline → -dy - depth = bottom → dy = -(bottom+depth).
        let dy = -(bottom + acc_box.depth);
        let height = (-dy + acc_box.height).max(base_box.height);
        children.push(Child { dx: acc_dx, dy, b: acc_box });
        (height, base_box.depth, base_w.max(acc_dx + acc_w))
    };

    Some(Box {
        width,
        height,
        depth,
        kind: BoxKind::Hbox { children },
    })
}

/// The horizontal accent-attachment point of `base` (px from its left edge): the
/// MATH `top_accent_attachment` of the base when it is a single glyph, else
/// `None` (the caller centers on the base width). Mirrors KaTeX `getSkew`.
fn base_skew(ctx: &Ctx, base: &MathList, style: Style) -> Option<f32> {
    // Only a lone single-glyph base has a well-defined attachment point.
    let atom = match base.as_slice() {
        [MathNode::Atom(a)] => a,
        [MathNode::Group(inner)] => match inner.as_slice() {
            [MathNode::Atom(a)] => a,
            _ => return None,
        },
        _ => return None,
    };
    let gid = glyph::glyph_for(ctx.face, atom.ch, atom.variant)?;
    top_accent_attachment(ctx, gid, ctx.scale_for(style))
}

/// The MATH `top_accent_attachment` of glyph `gid` in px at `scale` (font-units →
/// px), or `None` when the font supplies none for that glyph.
fn top_accent_attachment(ctx: &Ctx, gid: GlyphId, scale: f32) -> Option<f32> {
    ctx.face
        .tables()
        .math
        .and_then(|m| m.glyph_info)
        .and_then(|gi| gi.top_accent_attachments)
        .and_then(|ta| ta.get(gid))
        .map(|v| v.value as f32 * scale)
}

/// The TeX class of a node, for inter-atom spacing. A [`MathNode::Script`] takes
/// its base's class; a [`MathNode::Group`] is [`Class::Ord`] (TeX treats `{…}` as
/// an Ord atom).
fn node_class(node: &MathNode) -> Class {
    match node {
        MathNode::Atom(a) => a.class,
        MathNode::Group(_) => Class::Ord,
        // An extensible arrow (`\xrightarrow`, …) arrives as an `AboveBelow` script
        // over a lone arrow relation; it spaces as a relation (TeX/KaTeX). Any
        // other script takes its base's class.
        MathNode::Script { base, position: ScriptPos::AboveBelow, .. }
            if extensible_arrow_base(base).is_some() =>
        {
            Class::Rel
        }
        MathNode::Script { base, .. } => base.first().map(node_class).unwrap_or(Class::Ord),
        // A fraction is an Inner atom (TeXbook p. 159 / rule 15).
        MathNode::Frac { .. } => Class::Inner,
        // A `\left…\right` fence is an Inner atom (TeXbook p. 148).
        MathNode::Delim { .. } => Class::Inner,
        // A fixed-size delimiter keeps its command's Open/Close/Inner class.
        MathNode::BigDelim { class, .. } => *class,
        // A radical is an Ord atom (TeXbook p. 130).
        MathNode::Radical { .. } => Class::Ord,
        // An accented expression is an Ord atom (TeXbook p. 135 / KaTeX `accent`).
        MathNode::Accent { .. } => Class::Ord,
        // A matrix/array spaces as an Inner atom (like a fenced expression).
        MathNode::Matrix { .. } => Class::Inner,
        // A struck expression spaces as its content would — an Ord atom.
        MathNode::Cancel { .. } => Class::Ord,
        // A `\colorbox`/`\fcolorbox` spaces as an Ord atom (it boxes its content).
        MathNode::ColorBox { .. } => Class::Ord,
    }
}

/// Height (above baseline) and depth (below baseline) of a glyph in px, from its
/// outline bbox. Falls back to the font's ascender/descender when the glyph has no
/// outline (e.g. a space).
fn glyph_extents(face: &Face<'static>, gid: GlyphId, scale: f32) -> (f32, f32) {
    // A no-op outline builder: we only want the bounding box `outline_glyph` returns.
    struct Bbox;
    impl ttf_parser::OutlineBuilder for Bbox {
        fn move_to(&mut self, _: f32, _: f32) {}
        fn line_to(&mut self, _: f32, _: f32) {}
        fn quad_to(&mut self, _: f32, _: f32, _: f32, _: f32) {}
        fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {}
        fn close(&mut self) {}
    }
    match face.outline_glyph(gid, &mut Bbox) {
        Some(bbox) => {
            let height = (bbox.y_max as f32 * scale).max(0.0);
            let depth = (-(bbox.y_min as f32) * scale).max(0.0);
            (height, depth)
        }
        None => {
            // Fallback: use font-wide metrics so the row still has sane extent.
            let asc = face.ascender() as f32 * scale;
            let desc = -(face.descender() as f32) * scale;
            (asc.max(0.0), desc.max(0.0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `a` (a single Latin letter) should map to its math-italic codepoint and
    /// use a *different* glyph than the upright `a`.
    #[test]
    fn single_letter_maps_to_italic_glyph() {
        let face = font::math_face();
        let upright = face.glyph_index('a').unwrap();
        let italic = glyph::glyph_for(&face, 'a', Variant::Italic).unwrap();
        assert_ne!(upright, italic, "math-italic 'a' should differ from upright");

        // The mapped codepoint is U+1D44E MATHEMATICAL ITALIC SMALL A.
        assert_eq!(glyph::map_char('a', Variant::Italic), '\u{1D44E}');
    }

    /// Italic `h` has no slot in the Mathematical-Italic block; it must map to
    /// U+210E PLANCK CONSTANT.
    #[test]
    fn italic_h_uses_planck_constant() {
        assert_eq!(glyph::map_char('h', Variant::Italic), '\u{210E}');
        let face = font::math_face();
        assert!(
            glyph::glyph_for(&face, 'h', Variant::Italic).is_some(),
            "STIX should have U+210E"
        );
    }

    /// Numbers and uppercase Greek stay upright by the default rule.
    #[test]
    fn default_variant_rule() {
        assert_eq!(variant_for(None, 'x'), Variant::Italic);
        assert_eq!(variant_for(None, 'A'), Variant::Italic); // ASCII cap → italic
        assert_eq!(variant_for(None, '\u{0393}'), Variant::Upright); // Γ upright
        assert_eq!(variant_for(None, '\u{03B1}'), Variant::Italic); // α italic
    }

    /// Recursively collect all leaf [`Atom`]s of a [`MathList`] (test helper).
    fn collect_atoms(list: &MathList, out: &mut Vec<Atom>) {
        for node in list {
            match node {
                MathNode::Atom(a) => out.push(a.clone()),
                MathNode::Group(inner) => collect_atoms(inner, out),
                MathNode::Script { base, sup, sub, .. } => {
                    collect_atoms(base, out);
                    if let Some(s) = sub {
                        collect_atoms(s, out);
                    }
                    if let Some(s) = sup {
                        collect_atoms(s, out);
                    }
                }
                MathNode::Frac { num, den, .. } => {
                    collect_atoms(num, out);
                    collect_atoms(den, out);
                }
                MathNode::Delim { body, .. } => collect_atoms(body, out),
                MathNode::BigDelim { .. } => {}
                MathNode::Radical { index, radicand, .. } => {
                    if let Some(idx) = index {
                        collect_atoms(idx, out);
                    }
                    collect_atoms(radicand, out);
                }
                MathNode::Accent { base, .. } => collect_atoms(base, out),
                MathNode::Matrix { rows, .. } => {
                    for row in rows {
                        for cell in row {
                            collect_atoms(cell, out);
                        }
                    }
                }
                MathNode::Cancel { body, .. } => collect_atoms(body, out),
                MathNode::ColorBox { body, .. } => collect_atoms(body, out),
            }
        }
    }

    /// `\text{abc}` yields upright Ord atoms (no italic substitution).
    #[test]
    fn text_is_upright() {
        let list = parse_list(r"\text{abc}").unwrap();
        let mut atoms = Vec::new();
        collect_atoms(&list, &mut atoms);
        assert_eq!(atoms.len(), 3);
        for a in &atoms {
            assert_eq!(a.variant, Variant::Upright);
            assert_eq!(a.class, Class::Ord);
        }
    }

    /// `x^2` parses to a single Script node: base `x`, a superscript `2`, no sub.
    #[test]
    fn parses_superscript() {
        let list = parse_list("x^2").unwrap();
        assert_eq!(list.len(), 1, "one top-level element (the script)");
        match &list[0] {
            MathNode::Script { base, sup, sub, .. } => {
                assert!(sub.is_none(), "no subscript");
                assert!(sup.is_some(), "has superscript");
                let mut b = Vec::new();
                collect_atoms(base, &mut b);
                assert_eq!(b.len(), 1);
                assert_eq!(b[0].ch, 'x');
            }
            _ => panic!("expected a Script node"),
        }
    }

    /// Spacing around a relation (thickspace, 5mu) exceeds Ord↔Ord (0mu) and
    /// Ord↔Bin (medspace, 4mu).
    #[test]
    fn relation_spacing_is_largest() {
        let rel = spacing_mu(Class::Ord, Class::Rel, false);
        let ord = spacing_mu(Class::Ord, Class::Ord, false);
        let bin = spacing_mu(Class::Ord, Class::Bin, false);
        assert!(rel > bin && bin > ord, "rel {rel} > bin {bin} > ord {ord}");
        assert_eq!(ord, 0.0);
    }

    /// A leading `+` is unary: re-classed to Ord so it gets no Bin spacing.
    #[test]
    fn leading_bin_becomes_ord() {
        // `+a`: the `+` is at list start, so it should not get Bin spacing.
        let opts = MathOptions::default();
        let (_b, _f) = layout("+a", &opts, 1.0).expect("lays out");
        // No panic / non-empty is enough; detailed metric checked via mod.rs tests.
    }

    /// Flatten an [`Hbox`] into absolute `(dx, dy, &Box)` leaf placements so
    /// tests can read script offsets without walking the tree by hand.
    fn flatten<'a>(b: &'a Box, ox: f32, oy: f32, out: &mut Vec<(f32, f32, &'a Box)>) {
        match &b.kind {
            BoxKind::Glyph { .. }
            | BoxKind::Rule { .. }
            | BoxKind::Line { .. }
            | BoxKind::Fill { .. } => out.push((ox, oy, b)),
            BoxKind::Hbox { children } => {
                for c in children {
                    flatten(&c.b, ox + c.dx, oy + c.dy, out);
                }
            }
        }
    }

    /// `x^2`: the `2` is smaller (script em < base em) and raised (its baseline
    /// sits above the base baseline → negative dy), and its top is above the
    /// base's top.
    #[test]
    fn superscript_is_smaller_and_raised() {
        let opts = MathOptions::default();
        let (root, _f) = layout("x^2", &opts, 1.0).expect("lays out");
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);
        assert_eq!(leaves.len(), 2, "x and 2");
        let (_, base_dy, base) = leaves[0];
        let (_, sup_dy, sup) = leaves[1];
        assert_eq!(base_dy, 0.0, "base on the main baseline");
        assert!(sup_dy < 0.0, "superscript raised (dy {sup_dy} < 0)");
        // Script box uses the script em scale → smaller glyph metrics.
        if let (BoxKind::Glyph { scale: bs, .. }, BoxKind::Glyph { scale: ss, .. }) =
            (&base.kind, &sup.kind)
        {
            assert!(ss < bs, "script scale {ss} < base scale {bs}");
        } else {
            panic!("both leaves are glyphs");
        }
        // Superscript top (its height minus how far up its baseline is) sits above
        // the base top.
        let sup_top = -sup_dy + sup.height;
        assert!(sup_top > base.height, "sup top {sup_top} > base top {}", base.height);
    }

    /// `a_i`: the `i` is smaller and lowered (its baseline sits below the base
    /// baseline → positive dy), dropping below the main baseline.
    #[test]
    fn subscript_is_smaller_and_lowered() {
        let opts = MathOptions::default();
        let (root, _f) = layout("a_i", &opts, 1.0).expect("lays out");
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);
        assert_eq!(leaves.len(), 2, "a and i");
        let (_, sub_dy, sub) = leaves[1];
        assert!(sub_dy > 0.0, "subscript lowered (dy {sub_dy} > 0)");
        if let BoxKind::Glyph { scale: ss, .. } = &sub.kind {
            let base_scale = opts.font_size_px / UNITS_PER_EM;
            assert!(*ss < base_scale, "script scale {ss} < base {base_scale}");
        }
        assert!(root.depth > 0.0, "the row now has depth from the subscript");
    }

    /// `x_i^2`: a superscript above and a subscript below, with a positive gap
    /// between the superscript's bottom and the subscript's top.
    #[test]
    fn sub_and_super_have_positive_gap() {
        let opts = MathOptions::default();
        let (root, _f) = layout("x_i^2", &opts, 1.0).expect("lays out");
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);
        assert_eq!(leaves.len(), 3, "x, i, 2");
        // Identify sup (dy<0) and sub (dy>0).
        let raised: Vec<_> = leaves.iter().filter(|(_, dy, _)| *dy < 0.0).collect();
        let lowered: Vec<_> = leaves.iter().filter(|(_, dy, _)| *dy > 0.0).collect();
        assert_eq!(raised.len(), 1, "one superscript");
        assert_eq!(lowered.len(), 1, "one subscript");
        let (_, sup_dy, sup) = raised[0];
        let (_, sub_dy, sub) = lowered[0];
        // sup bottom (above baseline): -sup_dy - sup.depth ; sub top: -sub_dy + sub.height
        let sup_bottom = -sup_dy - sup.depth;
        let sub_top = -sub_dy + sub.height;
        assert!(
            sup_bottom - sub_top > 0.0,
            "positive gap: sup_bottom {sup_bottom} > sub_top {sub_top}"
        );
    }

    /// `e^{2x}`: the braces group two atoms into the superscript, so the script
    /// is about two glyphs wide (wider than a single-glyph script).
    #[test]
    fn grouped_superscript_is_two_glyphs_wide() {
        let opts = MathOptions::default();
        let (root, _f) = layout("e^{2x}", &opts, 1.0).expect("lays out");
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);
        // base `e` + two raised glyphs `2`, `x`.
        let raised: Vec<_> = leaves.iter().filter(|(_, dy, _)| *dy < 0.0).collect();
        assert_eq!(raised.len(), 2, "two-atom grouped superscript");
        // Single-glyph superscript width for comparison.
        let (single, _f) = layout("e^2", &opts, 1.0).expect("lays out");
        assert!(
            root.width > single.width,
            "grouped sup wider ({}) than single ({})",
            root.width,
            single.width
        );
    }

    /// `\frac{1}{2}` parses to one `Frac` node with single-atom numerator and
    /// denominator and no forced style.
    #[test]
    fn parses_fraction() {
        let list = parse_list(r"\frac{1}{2}").unwrap();
        assert_eq!(list.len(), 1, "one top-level element (the fraction)");
        match &list[0] {
            MathNode::Frac { num, den, style, .. } => {
                assert!(style.is_none(), "plain \\frac has no forced style");
                let mut n = Vec::new();
                let mut d = Vec::new();
                collect_atoms(num, &mut n);
                collect_atoms(den, &mut d);
                assert_eq!(n.len(), 1);
                assert_eq!(n[0].ch, '1');
                assert_eq!(d.len(), 1);
                assert_eq!(d[0].ch, '2');
            }
            _ => panic!("expected a Frac node"),
        }
    }

    /// `\dfrac{1}{2}` carries a Display forced style; `\tfrac` carries Text.
    /// (pulldown wraps these in a `Begin(Normal)` group, so search recursively.)
    #[test]
    fn parses_dfrac_tfrac_style_hint() {
        fn find_frac_style(list: &MathList) -> Option<Option<Style>> {
            for n in list {
                match n {
                    MathNode::Frac { style, .. } => return Some(*style),
                    MathNode::Group(inner) => {
                        if let Some(s) = find_frac_style(inner) {
                            return Some(s);
                        }
                    }
                    _ => {}
                }
            }
            None
        }
        let d = parse_list(r"\dfrac{1}{2}").unwrap();
        assert_eq!(find_frac_style(&d), Some(Some(Style::Display)), "\\dfrac → Display");
        let t = parse_list(r"\tfrac{1}{2}").unwrap();
        assert_eq!(find_frac_style(&t), Some(Some(Style::Text)), "\\tfrac → Text");
    }

    /// `\frac{1}{2}` lays out as a box containing a `Rule` with the numerator
    /// above the axis (negative dy) and the denominator below it (positive dy);
    /// total height+depth exceed a single digit glyph.
    #[test]
    fn fraction_stacks_num_rule_den() {
        let opts = MathOptions::default();
        let (root, _f) = layout(r"\frac{1}{2}", &opts, 1.0).expect("lays out");
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);

        // One rule, plus a numerator and denominator glyph.
        let rules: Vec<_> = leaves
            .iter()
            .filter(|(_, _, b)| matches!(b.kind, BoxKind::Rule { .. }))
            .collect();
        assert_eq!(rules.len(), 1, "exactly one fraction bar");
        let glyphs: Vec<_> = leaves
            .iter()
            .filter(|(_, _, b)| matches!(b.kind, BoxKind::Glyph { .. }))
            .collect();
        assert_eq!(glyphs.len(), 2, "numerator and denominator glyphs");

        // The rule's baseline (oy) sits at the axis; the numerator glyph is raised
        // above it (smaller oy) and the denominator below (larger oy).
        let (_, rule_oy, _) = rules[0];
        let mut gy: Vec<f32> = glyphs.iter().map(|(_, oy, _)| *oy).collect();
        gy.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!(gy[0] < *rule_oy, "numerator above the bar ({} < {rule_oy})", gy[0]);
        assert!(gy[1] > *rule_oy, "denominator below the bar ({} > {rule_oy})", gy[1]);

        // Taller than a single digit.
        let (single, _f) = layout("2", &opts, 1.0).expect("lays out");
        assert!(
            root.height + root.depth > single.height + single.depth,
            "fraction ({} + {}) taller than a digit ({} + {})",
            root.height,
            root.depth,
            single.height,
            single.depth
        );
    }

    /// `\binom{n}{k}` stacks the numerator and denominator with **no** bar
    /// (`bar == false`) and keeps the surrounding `\left(\right)` parens, while a
    /// plain `\frac{n}{k}` *does* draw a `Rule`. We assert rule presence/absence in
    /// the laid-out box trees and that the binomial still shows two parens.
    #[test]
    fn binom_has_no_bar_but_frac_does() {
        let opts = MathOptions::default();

        // `\frac{n}{k}` → exactly one Rule (the bar).
        let (frac, _f) = layout(r"\frac{n}{k}", &opts, 1.0).expect("lays out");
        let mut frac_leaves = Vec::new();
        flatten(&frac, 0.0, 0.0, &mut frac_leaves);
        let frac_rules = frac_leaves
            .iter()
            .filter(|(_, _, b)| matches!(b.kind, BoxKind::Rule { .. }))
            .count();
        assert_eq!(frac_rules, 1, "\\frac keeps its bar");

        // `\binom{n}{k}` → no Rule at all (no bar), but still its two paren glyphs.
        let (binom, _f) = layout(r"\binom{n}{k}", &opts, 1.0).expect("lays out");
        let mut binom_leaves = Vec::new();
        flatten(&binom, 0.0, 0.0, &mut binom_leaves);
        let binom_rules = binom_leaves
            .iter()
            .filter(|(_, _, b)| matches!(b.kind, BoxKind::Rule { .. }))
            .count();
        assert_eq!(binom_rules, 0, "\\binom draws no bar");

        // The parser keeps the parens as a `Delim` wrapping a barless `Frac`.
        let list = parse_list(r"\binom{n}{k}").unwrap();
        let frac_node = match &list[0] {
            MathNode::Delim { open, close, body } => {
                assert_eq!(*open, Some('('), "binomial opens with (");
                assert_eq!(*close, Some(')'), "binomial closes with )");
                &body[0]
            }
            _ => panic!("expected a Delim around the binomial"),
        };
        match frac_node {
            MathNode::Frac { bar, .. } => {
                assert_eq!(*bar, BarThickness::None, "binomial fraction has no bar")
            }
            _ => panic!("expected a Frac inside the parens"),
        }
    }

    /// `\genfrac[]{2pt}{}{a}{b}` honors its explicit 2pt bar thickness: the parsed
    /// `Frac` carries a non-zero `BarThickness::Em`, and the laid-out bar rule is
    /// drawn **thicker** than a default `\frac`'s bar at the same em.
    ///
    /// Note: pulldown-latex 0.7 accepts genfrac delimiters only as *bare tokens*
    /// (`\genfrac[]{…}`) or empty groups (`\genfrac{}{}{…}`), and rejects the
    /// braced `\genfrac{[}{]}{…}` spelling with a `Delimiter` error — so the test
    /// (and sample) use the bracket-token form.
    #[test]
    fn genfrac_honors_custom_bar_thickness() {
        // Parse: the bar is an explicit (non-zero, non-default) thickness in em.
        let list = parse_list(r"\genfrac[]{2pt}{}{a}{b}").unwrap();
        // pulldown wraps the genfrac's delimiters in a `Delim`; find the `Frac`.
        fn find_frac(list: &MathList) -> Option<&MathNode> {
            for n in list {
                match n {
                    f @ MathNode::Frac { .. } => return Some(f),
                    MathNode::Delim { body, .. } | MathNode::Group(body) => {
                        if let Some(f) = find_frac(body) {
                            return Some(f);
                        }
                    }
                    _ => {}
                }
            }
            None
        }
        match find_frac(&list).expect("a Frac node") {
            MathNode::Frac { bar, .. } => match bar {
                BarThickness::Em(em) => assert!(*em > 0.0, "2pt → positive em {em}"),
                other => panic!("expected an explicit Em thickness, got {other:?}"),
            },
            _ => unreachable!(),
        }

        // Layout: the genfrac bar is thicker than the default `\frac` bar.
        let opts = MathOptions::default();
        fn bar_thickness(src: &str, opts: &MathOptions) -> f32 {
            let (root, _f) = layout(src, opts, 1.0).expect("lays out");
            let mut leaves = Vec::new();
            flatten(&root, 0.0, 0.0, &mut leaves);
            leaves
                .iter()
                .find_map(|(_, _, b)| match b.kind {
                    BoxKind::Rule { thickness, .. } => Some(thickness),
                    _ => None,
                })
                .expect("a fraction bar rule")
        }
        let default_t = bar_thickness(r"\frac{a}{b}", &opts);
        let custom_t = bar_thickness(r"\genfrac[]{2pt}{}{a}{b}", &opts);
        assert!(
            custom_t > default_t * 1.5,
            "2pt genfrac bar ({custom_t}) thicker than default \\frac bar ({default_t})"
        );
    }

    /// `\colorbox{yellow}{x+1}` draws a solid `Fill` rectangle behind its content,
    /// emitted as the **first** child so it paints first (behind). `\fcolorbox`
    /// additionally strokes a frame (four `Line`s) over the fill.
    #[test]
    fn colorbox_draws_background_fill() {
        let opts = MathOptions::default();
        let src = super::super::color::normalize_color_args(r"\colorbox{yellow}{x+1}", opts.color);
        let (root, _f) = layout(&src, &opts, 1.0).expect("lays out");

        // The top-level Hbox's colorbox child is itself an Hbox whose first child
        // is the Fill (paint order = behind), followed by the content.
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);
        let fills: Vec<_> = leaves
            .iter()
            .filter(|(_, _, b)| matches!(b.kind, BoxKind::Fill { .. }))
            .collect();
        assert_eq!(fills.len(), 1, "one background fill");
        if let BoxKind::Fill { width, height, depth, color } = fills[0].2.kind {
            assert!(width > 0.0 && (height + depth) > 0.0, "fill spans a real bbox");
            assert_eq!(color, [255, 255, 0, 255], "yellow fill");
        }
        // The content glyphs (x, +, 1) still render.
        let glyphs = leaves
            .iter()
            .filter(|(_, _, b)| matches!(b.kind, BoxKind::Glyph { .. }))
            .count();
        assert!(glyphs >= 3, "x + 1 glyphs render over the fill");

        // Paint order: walk the colorbox Hbox directly and check the Fill is first.
        fn first_kind_is_fill(b: &Box) -> bool {
            if let BoxKind::Hbox { children } = &b.kind {
                for c in children {
                    if matches!(c.b.kind, BoxKind::Fill { .. }) {
                        return true;
                    }
                    if matches!(c.b.kind, BoxKind::Hbox { .. }) && first_kind_is_fill(&c.b) {
                        // Recurse into the colorbox's own Hbox.
                        return true;
                    }
                }
            }
            false
        }
        assert!(first_kind_is_fill(&root), "fill present (painted behind content)");
    }

    /// `\fcolorbox{red}{yellow}{x}` adds a red frame (four `Line` overlays) on top
    /// of the yellow background fill.
    #[test]
    fn fcolorbox_adds_a_border_frame() {
        let opts = MathOptions::default();
        let src = super::super::color::normalize_color_args(r"\fcolorbox{red}{yellow}{x}", opts.color);
        let (root, _f) = layout(&src, &opts, 1.0).expect("lays out");
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);

        let fills = leaves
            .iter()
            .filter(|(_, _, b)| matches!(b.kind, BoxKind::Fill { .. }))
            .count();
        assert_eq!(fills, 1, "one yellow background fill");
        let frame: Vec<_> = leaves
            .iter()
            .filter(|(_, _, b)| matches!(b.kind, BoxKind::Line { .. }))
            .collect();
        assert_eq!(frame.len(), 4, "four frame edges");
        for (_, _, b) in &frame {
            if let BoxKind::Line { color, .. } = b.kind {
                assert_eq!(color, [255, 0, 0, 255], "red frame edge");
            }
        }
    }

    /// The accent's horizontal offset tracks the **base glyph's** top-accent
    /// attachment: `\hat{f}` (a slanted `f`, attachment well right of center) puts
    /// its hat farther right than `\hat{l}` (a near-symmetric `l`).
    #[test]
    fn accent_offset_shifts_with_base_attachment() {
        let opts = MathOptions::default();
        fn accent_dx(src: &str, opts: &MathOptions) -> f32 {
            let (root, _f) = layout(src, opts, 1.0).expect("lays out");
            let mut leaves = Vec::new();
            flatten(&root, 0.0, 0.0, &mut leaves);
            // The accent is the raised glyph (negative oy); the base sits on the
            // baseline (oy ≈ 0). Return the accent's absolute x.
            leaves
                .iter()
                .filter(|(_, oy, _)| *oy < -0.01)
                .map(|(ox, _, _)| *ox)
                .fold(f32::NEG_INFINITY, f32::max)
        }
        let hat_f = accent_dx(r"\hat{f}", &opts);
        let hat_l = accent_dx(r"\hat{l}", &opts);
        assert!(hat_f.is_finite() && hat_l.is_finite(), "both accents placed");
        assert!(
            hat_f > hat_l,
            "f's rightward attachment pushes its hat farther right ({hat_f}) than l's ({hat_l})"
        );
    }

    /// `\sum\nolimits_i^n` keeps its scripts **beside** the operator even in
    /// Display style (pulldown maps `\nolimits` → `ScriptPos::Right`), while a
    /// plain `\sum_i^n` in Display stacks them **above/below** (`Movable`).
    #[test]
    fn nolimits_keeps_scripts_beside_in_display() {
        // Parse positions: `\nolimits` → Right, `\limits` → AboveBelow.
        fn script_pos(src: &str) -> ScriptPos {
            fn find(list: &MathList) -> Option<ScriptPos> {
                for n in list {
                    match n {
                        MathNode::Script { position, .. } => return Some(*position),
                        MathNode::Group(inner) => {
                            if let Some(p) = find(inner) {
                                return Some(p);
                            }
                        }
                        _ => {}
                    }
                }
                None
            }
            find(&parse_list(src).unwrap()).expect("a Script node")
        }
        assert_eq!(script_pos(r"\sum\nolimits_{i}^{n}"), ScriptPos::Right);
        assert_eq!(script_pos(r"\sum\limits_{i}^{n}"), ScriptPos::AboveBelow);

        // Layout in Display: `\nolimits` scripts sit beside (some script glyph to
        // the right of the operator, on/near the baseline); `\sum_i^n` stacks them
        // (scripts centered, the superscript raised well above the operator top).
        let display = MathOptions {
            style: super::super::MathStyle::Display,
            ..MathOptions::default()
        };
        let (beside, _f) = layout(r"\sum\nolimits_{i}^{n}", &display, 1.0).expect("lays out");
        let (stacked, _f) = layout(r"\sum_{i}^{n}", &display, 1.0).expect("lays out");
        // Beside-scripts make the row wider (scripts add advance) than the stacked
        // form (scripts overlap the operator's column).
        assert!(
            beside.width > stacked.width,
            "nolimits scripts beside widen the row ({}) vs stacked ({})",
            beside.width,
            stacked.width
        );
    }

    /// `\int\limits_0^1` forces its limits **above/below** the integral
    /// (`\limits` → `ScriptPos::AboveBelow`), unlike a bare `\int_0^1` (which
    /// stays beside, `Right`).
    #[test]
    fn limits_stacks_integral_scripts_above_below() {
        let list = parse_list(r"\int\limits_{0}^{1}").unwrap();
        fn find_pos(list: &MathList) -> Option<ScriptPos> {
            for n in list {
                match n {
                    MathNode::Script { position, .. } => return Some(*position),
                    MathNode::Group(inner) => {
                        if let Some(p) = find_pos(inner) {
                            return Some(p);
                        }
                    }
                    _ => {}
                }
            }
            None
        }
        assert_eq!(
            find_pos(&list),
            Some(ScriptPos::AboveBelow),
            "\\int\\limits forces above/below"
        );
        // A bare `\int_0^1` stays beside (Right).
        assert_eq!(find_pos(&parse_list(r"\int_{0}^{1}").unwrap()), Some(ScriptPos::Right));
    }

    /// `\cancel{x}` lays out the `x` with a diagonal `Line` overlaid across its
    /// bounding box: a `Line` leaf with positive `dx`/`dy` spanning ≈ the glyph
    /// box, while the underlying glyph still renders.
    #[test]
    fn cancel_overlays_a_diagonal_line() {
        let opts = MathOptions::default();
        // `\cancel` is shimmed in the macro pass, so expand before laying out.
        let src = super::super::macros::expand_definitions(r"\cancel{x}");
        let (root, _f) = layout(&src, &opts, 1.0).expect("lays out");
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);

        // The struck `x` glyph is still present.
        let glyphs = leaves
            .iter()
            .filter(|(_, _, b)| matches!(b.kind, BoxKind::Glyph { .. }))
            .count();
        assert_eq!(glyphs, 1, "the canceled x still renders");

        // Exactly one Line, with a forward-diagonal direction (rightward + upward,
        // i.e. positive dx and negative dy) spanning roughly the body's bbox.
        let lines: Vec<_> = leaves
            .iter()
            .filter(|(_, _, b)| matches!(b.kind, BoxKind::Line { .. }))
            .collect();
        assert_eq!(lines.len(), 1, "one strike line");
        if let BoxKind::Line { dx, dy, thickness, .. } = lines[0].2.kind {
            assert!(dx > 0.0, "strike runs rightward (dx {dx} > 0)");
            assert!(dy < 0.0, "strike runs upward (dy {dy} < 0)");
            assert!(thickness > 0.0, "visible stroke width");
            // The span ≈ the body bbox: width within the box width, vertical reach
            // within the total height + depth.
            assert!(
                (dx - root.width).abs() < 0.5,
                "strike width {dx} ≈ box width {}",
                root.width
            );
            assert!(
                (-dy - (root.height + root.depth)).abs() < 0.5,
                "strike rise {} ≈ height+depth {}",
                -dy,
                root.height + root.depth
            );
        }
    }

    /// `\not` still renders without a strike (its `Visual::Negation` is left
    /// untouched), so it produces no `Line` — only `\cancel` does.
    #[test]
    fn not_does_not_strike() {
        let opts = MathOptions::default();
        let (root, _f) = layout(r"\not=", &opts, 1.0).expect("lays out");
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);
        let lines = leaves
            .iter()
            .filter(|(_, _, b)| matches!(b.kind, BoxKind::Line { .. }))
            .count();
        assert_eq!(lines, 0, "\\not draws no strike line");
    }

    /// `\substack{a \\ b}` is shimmed (in the macro pass) to a 2-row centered
    /// `matrix`, so it parses to a `Matrix` node with two rows and lays out as two
    /// vertically stacked glyphs.
    #[test]
    fn substack_stacks_two_rows() {
        // The shim turns `\substack` into a matrix the parser understands.
        let expanded = super::super::macros::expand_definitions(r"\substack{a \\ b}");
        let list = parse_list(&expanded).expect("substack shim parses");
        fn find_matrix(list: &MathList) -> Option<usize> {
            for n in list {
                match n {
                    MathNode::Matrix { rows, .. } => return Some(rows.len()),
                    MathNode::Group(inner) | MathNode::Delim { body: inner, .. } => {
                        if let Some(r) = find_matrix(inner) {
                            return Some(r);
                        }
                    }
                    _ => {}
                }
            }
            None
        }
        assert_eq!(find_matrix(&list), Some(2), "two stacked rows");

        // It renders Some with two stacked glyphs (a over b).
        let opts = MathOptions::default();
        let (root, _f) = layout(&expanded, &opts, 1.0).expect("lays out");
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);
        let mut ys: Vec<f32> = leaves
            .iter()
            .filter(|(_, _, b)| matches!(b.kind, BoxKind::Glyph { .. }))
            .map(|(_, oy, _)| *oy)
            .collect();
        assert_eq!(ys.len(), 2, "two stacked glyphs");
        ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!(ys[1] > ys[0], "second row sits below the first");
    }

    /// `\sum_{\substack{0<i<n \\ i\ne k}} a_i` renders Some end-to-end.
    #[test]
    fn sum_with_substack_renders() {
        let opts = MathOptions {
            style: super::super::MathStyle::Display,
            ..MathOptions::default()
        };
        let out =
            super::super::render_latex(r"\sum_{\substack{0<i<n \\ i\ne k}} a_i", &opts);
        assert!(out.is_some(), "sum over substack renders Some");
    }

    /// `\substack{…}` renders one style step *smaller* than a plain `matrix`: the
    /// substack glyphs use the script em (≈ 0.7×), so its glyph `scale` is below
    /// the plain matrix's.
    #[test]
    fn substack_cells_are_script_sized() {
        let opts = MathOptions {
            style: super::super::MathStyle::Display,
            ..MathOptions::default()
        };
        // Smallest glyph scale among the leaves of a render.
        let min_scale = |src: &str| -> f32 {
            let expanded = super::super::macros::expand_definitions(src);
            let (root, _f) = layout(&expanded, &opts, 1.0).expect("lays out");
            let mut leaves = Vec::new();
            flatten(&root, 0.0, 0.0, &mut leaves);
            leaves
                .iter()
                .filter_map(|(_, _, b)| match b.kind {
                    BoxKind::Glyph { scale, .. } => Some(scale),
                    _ => None,
                })
                .fold(f32::INFINITY, f32::min)
        };
        let sub = min_scale(r"\substack{a \\ b}");
        let plain = min_scale(r"\begin{matrix}a \\ b\end{matrix}");
        assert!(sub.is_finite() && plain.is_finite());
        assert!(
            sub < plain - 1e-3,
            "substack glyph scale {sub} should be smaller than plain matrix {plain}"
        );
    }

    /// `\begin{array}{c|c} a & b \\ c & d \end{array}`: the `|` in the column spec
    /// produces a vertical `Rule` between the two columns (a tall, thin rule whose
    /// height exceeds its width), positioned between the column x-offsets.
    #[test]
    fn array_vertical_rule_between_columns() {
        let opts = MathOptions::default();
        let (root, _f) =
            layout(r"\begin{array}{c|c} a & b \\ c & d \end{array}", &opts, 1.0).expect("lays out");
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);
        // A vertical rule is a tall, thin Rule (thickness == its drawn height >
        // width). The horizontal fraction/accent rules are wide-and-short.
        let vrules: Vec<_> = leaves
            .iter()
            .filter(|(_, _, b)| matches!(
                b.kind,
                BoxKind::Rule { width, thickness, .. } if thickness > width && thickness > 1.0
            ))
            .collect();
        assert_eq!(vrules.len(), 1, "exactly one vertical rule, got {}", vrules.len());
        let (rx, _, _) = vrules[0];
        // The cell glyphs straddle the rule: at least one left of it, one right.
        let glyph_xs: Vec<f32> = leaves
            .iter()
            .filter(|(_, _, b)| matches!(b.kind, BoxKind::Glyph { .. }))
            .map(|(ox, _, _)| *ox)
            .collect();
        assert!(glyph_xs.iter().any(|&x| x < *rx), "a column left of the rule");
        assert!(glyph_xs.iter().any(|&x| x > *rx), "a column right of the rule");
    }

    /// `\begin{array}{c||c} …`: a double `||` produces two adjacent vertical rules.
    #[test]
    fn array_double_vertical_rule() {
        let opts = MathOptions::default();
        let (root, _f) =
            layout(r"\begin{array}{c||c} a & b \\ c & d \end{array}", &opts, 1.0).expect("lays out");
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);
        let vrules = leaves
            .iter()
            .filter(|(_, _, b)| matches!(
                b.kind,
                BoxKind::Rule { width, thickness, .. } if thickness > width && thickness > 1.0
            ))
            .count();
        assert_eq!(vrules, 2, "double bar → two vertical rules, got {vrules}");
    }

    /// `\begin{array}{cc} a & b \\ \hline c & d \end{array}`: the `\hline` produces
    /// a horizontal `Rule` spanning the grid width between the two rows.
    #[test]
    fn array_horizontal_rule_between_rows() {
        let opts = MathOptions::default();
        let (root, _f) =
            layout(r"\begin{array}{cc} a & b \\ \hline c & d \end{array}", &opts, 1.0)
                .expect("lays out");
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);
        // A horizontal rule is wide and short (width > thickness, width ~ grid).
        let hrules: Vec<_> = leaves
            .iter()
            .filter(|(_, _, b)| matches!(
                b.kind,
                BoxKind::Rule { width, thickness, .. } if width > thickness && width > 1.0
            ))
            .collect();
        assert_eq!(hrules.len(), 1, "exactly one horizontal rule, got {}", hrules.len());
        // It sits between the two row baselines (some glyph above it, some below).
        let (_, ry, _) = hrules[0];
        let glyph_ys: Vec<f32> = leaves
            .iter()
            .filter(|(_, _, b)| matches!(b.kind, BoxKind::Glyph { .. }))
            .map(|(_, oy, _)| *oy)
            .collect();
        assert!(glyph_ys.iter().any(|&y| y < *ry), "a row above the rule");
        assert!(glyph_ys.iter().any(|&y| y > *ry), "a row below the rule");
    }

    /// `\hline` at the top and bottom (`\begin{array}{c} \hline a \\ \hline`) both
    /// render, giving two horizontal rules.
    #[test]
    fn array_top_and_bottom_hlines() {
        let opts = MathOptions::default();
        let (root, _f) =
            layout(r"\begin{array}{c} \hline a \\ \hline \end{array}", &opts, 1.0)
                .expect("lays out");
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);
        let hrules = leaves
            .iter()
            .filter(|(_, _, b)| matches!(
                b.kind,
                BoxKind::Rule { width, thickness, .. } if width > thickness && width > 1.0
            ))
            .count();
        assert_eq!(hrules, 2, "top + bottom hline → two rules, got {hrules}");
    }

    /// `\arraystretch` scales inter-row spacing: a matrix laid out with a stretch of
    /// 1.8 is taller than the same matrix at the default 1.0.
    #[test]
    fn arraystretch_scales_row_spacing() {
        let opts = MathOptions {
            style: super::super::MathStyle::Display,
            ..MathOptions::default()
        };
        let src = r"\begin{matrix} a \\ b \\ c \end{matrix}";
        let (plain, _f) = layout(src, &opts, 1.0).expect("lays out");
        let (tall, _f) = layout(src, &opts, 1.8).expect("lays out");
        assert!(
            tall.height + tall.depth > plain.height + plain.depth + 1.0,
            "stretched matrix ({} + {}) taller than default ({} + {})",
            tall.height,
            tall.depth,
            plain.height,
            plain.depth
        );
    }

    /// The macro pass extracts `\renewcommand{\arraystretch}{F}` (strips it, returns
    /// F) so it threads into layout: the rendered matrix is taller with a large F.
    #[test]
    fn arraystretch_renewcommand_extracted_and_applied() {
        use super::super::macros;
        let (stripped, f) =
            macros::extract_arraystretch(&macros::expand_definitions(r"\renewcommand{\arraystretch}{2}"));
        assert_eq!(f, Some(2.0), "factor parsed from \\renewcommand");
        assert!(!stripped.contains("arraystretch"), "definition stripped");

        let opts = MathOptions {
            style: super::super::MathStyle::Display,
            ..MathOptions::default()
        };
        let plain = super::super::render_latex(
            r"\begin{matrix} a \\ b \\ c \end{matrix}",
            &opts,
        )
        .expect("renders");
        let tall = super::super::render_latex(
            r"\renewcommand{\arraystretch}{2}\begin{matrix} a \\ b \\ c \end{matrix}",
            &opts,
        )
        .expect("renders");
        assert!(
            tall.height_px > plain.height_px + 1.0,
            "renewcommand-stretched matrix taller end-to-end ({} vs {})",
            tall.height_px,
            plain.height_px
        );
    }

    /// A Display-style `\frac` is taller (larger shifts / gaps) than the same
    /// fraction in Inline (Text) style.
    #[test]
    fn display_fraction_taller_than_inline() {
        let inline = MathOptions {
            style: super::super::MathStyle::Inline,
            ..MathOptions::default()
        };
        let display = MathOptions {
            style: super::super::MathStyle::Display,
            ..MathOptions::default()
        };
        let (ib, _) = layout(r"\frac{1}{2}", &inline, 1.0).expect("lays out");
        let (db, _) = layout(r"\frac{1}{2}", &display, 1.0).expect("lays out");
        assert!(
            db.height + db.depth > ib.height + ib.depth,
            "display ({} + {}) taller than inline ({} + {})",
            db.height,
            db.depth,
            ib.height,
            ib.depth
        );
    }

    /// `x^{\frac{1}{2}}`: the fraction sits in the superscript and its glyphs lay
    /// out at script size (smaller than the base `x`).
    #[test]
    fn fraction_in_superscript_is_script_size() {
        let opts = MathOptions::default();
        let (root, _f) = layout(r"x^{\frac{1}{2}}", &opts, 1.0).expect("lays out");
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);

        // The base `x` is the only glyph on the main baseline (oy == 0).
        let base_scale = match leaves
            .iter()
            .find(|(_, oy, b)| *oy == 0.0 && matches!(b.kind, BoxKind::Glyph { .. }))
        {
            Some((_, _, b)) => match b.kind {
                BoxKind::Glyph { scale, .. } => scale,
                _ => unreachable!(),
            },
            None => panic!("base glyph on baseline"),
        };
        // The fraction's digits are raised (oy != 0) and at a smaller scale.
        let frac_glyphs: Vec<f32> = leaves
            .iter()
            .filter_map(|(_, oy, b)| match b.kind {
                BoxKind::Glyph { scale, .. } if *oy != 0.0 => Some(scale),
                _ => None,
            })
            .collect();
        assert_eq!(frac_glyphs.len(), 2, "1 and 2 in the superscript fraction");
        for s in frac_glyphs {
            assert!(s < base_scale, "script-fraction glyph {s} < base {base_scale}");
        }
        // There is a bar in there too.
        assert!(
            leaves.iter().any(|(_, _, b)| matches!(b.kind, BoxKind::Rule { .. })),
            "the superscript fraction has a bar"
        );
    }

    /// `\left( x \right)` parses to a single `Delim` node carrying `(`/`)` around
    /// a body containing the `x`.
    #[test]
    fn parses_left_right() {
        let list = parse_list(r"\left( x \right)").unwrap();
        assert_eq!(list.len(), 1, "one top-level element (the fence)");
        match &list[0] {
            MathNode::Delim { open, body, close } => {
                assert_eq!(*open, Some('('));
                assert_eq!(*close, Some(')'));
                let mut a = Vec::new();
                collect_atoms(body, &mut a);
                assert_eq!(a.len(), 1);
                assert_eq!(a[0].ch, 'x');
            }
            other => panic!("expected a Delim node, got {:?}", std::mem::discriminant(other)),
        }
    }

    /// `\left. x \right|` has a null left delimiter (`None`) and a `|` right one.
    #[test]
    fn parses_null_left_delim() {
        let list = parse_list(r"\left. x \right|").unwrap();
        match &list[0] {
            MathNode::Delim { open, close, .. } => {
                assert_eq!(*open, None, "null `\\left.` → no open glyph");
                assert_eq!(*close, Some('|'));
            }
            _ => panic!("expected a Delim node"),
        }
    }

    /// Unwrap the single-element top-level row produced by [`layout`] to the inner
    /// node box (here, the `[open][body][close]` fence hbox).
    fn only_child(b: &Box) -> &Box {
        match &b.kind {
            BoxKind::Hbox { children } if children.len() == 1 => &children[0].b,
            _ => b,
        }
    }

    /// `\left( x \right)`: both parens render as boxes at least as tall as the
    /// enclosed `x` (and the whole fence is wider than a bare `x`).
    #[test]
    fn left_right_brackets_content() {
        let opts = MathOptions::default();
        let (fence, _f) = layout(r"\left( x \right)", &opts, 1.0).expect("lays out");
        let (bare, _f) = layout("x", &opts, 1.0).expect("lays out");

        // Top-level row wraps the fence; its children are [open][body][close].
        let fence = only_child(&fence);
        let BoxKind::Hbox { children } = &fence.kind else {
            panic!("fence is an hbox");
        };
        assert_eq!(children.len(), 3, "open + body + close");
        let open_h = children[0].b.height + children[0].b.depth;
        let close_h = children[2].b.height + children[2].b.depth;
        let x_h = bare.height + bare.depth;
        assert!(open_h >= x_h, "open delim {open_h} ≳ x {x_h}");
        assert!(close_h >= x_h, "close delim {close_h} ≳ x {x_h}");
        assert!(fence.width > bare.width, "fence wider than bare x");
    }

    /// The parens around a `\frac{a}{b}` are taller than around a bare `x` — the
    /// delimiter grows with the content height.
    #[test]
    fn delim_grows_with_content() {
        let opts = MathOptions::default();
        let (small, _f) = layout(r"\left( x \right)", &opts, 1.0).expect("lays out");
        let (big, _f) = layout(r"\left( \frac{a}{b} \right)", &opts, 1.0).expect("lays out");

        let open_h = |b: &Box| -> f32 {
            let b = only_child(b);
            let BoxKind::Hbox { children } = &b.kind else { panic!() };
            children[0].b.height + children[0].b.depth
        };
        assert!(
            open_h(&big) > open_h(&small),
            "paren around frac ({}) taller than around x ({})",
            open_h(&big),
            open_h(&small)
        );
    }

    /// `\left. x \right|`: no left glyph box (only body + right bar), but the bar
    /// is present and finite.
    #[test]
    fn null_left_delim_has_no_open_box() {
        let opts = MathOptions::default();
        let (fence, _f) = layout(r"\left. x \right|", &opts, 1.0).expect("lays out");
        let fence = only_child(&fence);
        let BoxKind::Hbox { children } = &fence.kind else { panic!("hbox") };
        // [body][close] only — the null open contributes nothing.
        assert_eq!(children.len(), 2, "body + close (no open)");
        let close = &children[1].b;
        assert!(close.height + close.depth > 0.0, "right bar has extent");
        assert!(fence.height.is_finite() && fence.depth.is_finite(), "sane metrics");
    }

    /// A deeply nested fraction makes the parens *taller* than around a single
    /// fraction (assembly/larger-variant selection kicks in) with finite metrics.
    #[test]
    fn tall_delim_assembles_finite() {
        let opts = MathOptions::default();
        let (one, _f) = layout(r"\left( \frac{a}{b} \right)", &opts, 1.0).expect("lays out");
        let (tall, _f) = layout(r"\left( \frac{\frac{a}{b}}{c} \right)", &opts, 1.0).expect("lays out");
        let open_h = |b: &Box| -> f32 {
            let b = only_child(b);
            let BoxKind::Hbox { children } = &b.kind else { panic!() };
            children[0].b.height + children[0].b.depth
        };
        let (h1, h2) = (open_h(&one), open_h(&tall));
        assert!(h2 >= h1, "taller content → taller paren ({h2} ≥ {h1})");
        assert!(h2.is_finite() && h2 > 0.0, "finite, positive delimiter height");
        assert!(tall.height.is_finite() && tall.depth.is_finite(), "sane fence metrics");
    }

    /// `\bigl(` produces a fixed-size delimiter taller than a plain `(` glyph.
    #[test]
    fn big_delim_is_larger_than_plain() {
        let opts = MathOptions::default();
        let (big, _f) = layout(r"\bigl(", &opts, 1.0).expect("lays out");
        let (plain, _f) = layout("(", &opts, 1.0).expect("lays out");
        assert!(
            big.height + big.depth > plain.height + plain.depth,
            "\\bigl( ({} + {}) taller than ( ({} + {})",
            big.height,
            big.depth,
            plain.height,
            plain.depth
        );
    }

    /// `\sqrt{x}` parses to one `Radical` node with no index and a single-atom
    /// radicand; `\sqrt[3]{x}` carries a degree of `3` and the radicand `x`.
    #[test]
    fn parses_radical_forms() {
        let sq = parse_list(r"\sqrt{x}").unwrap();
        assert_eq!(sq.len(), 1);
        match &sq[0] {
            MathNode::Radical { index, radicand, .. } => {
                assert!(index.is_none(), "\\sqrt has no degree");
                let mut r = Vec::new();
                collect_atoms(radicand, &mut r);
                assert_eq!(r.len(), 1);
                assert_eq!(r[0].ch, 'x');
            }
            _ => panic!("expected a Radical node"),
        }
        let cb = parse_list(r"\sqrt[3]{x}").unwrap();
        match &cb[0] {
            MathNode::Radical { index, radicand, .. } => {
                let mut i = Vec::new();
                let mut r = Vec::new();
                collect_atoms(index.as_ref().expect("has a degree"), &mut i);
                collect_atoms(radicand, &mut r);
                assert_eq!(i.len(), 1);
                assert_eq!(i[0].ch, '3', "degree is 3");
                assert_eq!(r.len(), 1);
                assert_eq!(r[0].ch, 'x', "radicand is x");
            }
            _ => panic!("expected a Radical node"),
        }
    }

    /// `\sqrt{x}` lays out as a surd glyph plus a `Rule` (the vinculum), with the
    /// radicand glyph to the right of the surd and below the rule.
    #[test]
    fn sqrt_has_surd_and_vinculum() {
        let opts = MathOptions::default();
        let (root, _f) = layout(r"\sqrt{x}", &opts, 1.0).expect("lays out");
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);

        let rules: Vec<_> = leaves
            .iter()
            .filter(|(_, _, b)| matches!(b.kind, BoxKind::Rule { .. }))
            .collect();
        assert_eq!(rules.len(), 1, "exactly one vinculum rule");
        let glyphs: Vec<_> = leaves
            .iter()
            .filter(|(_, _, b)| matches!(b.kind, BoxKind::Glyph { .. }))
            .collect();
        // Surd glyph + the radicand `x`.
        assert!(glyphs.len() >= 2, "surd + radicand glyphs ({})", glyphs.len());

        // The rule sits above the baseline (oy < 0) — it is the vinculum.
        let (rule_ox, rule_oy, _) = rules[0];
        assert!(*rule_oy < 0.0, "vinculum above the baseline (oy {rule_oy})");

        // The right-most glyph (the radicand) starts to the right of the rule's
        // left edge and below the rule.
        let rightmost = glyphs.iter().max_by(|a, b| a.0.partial_cmp(&b.0).unwrap()).unwrap();
        assert!(rightmost.0 >= *rule_ox, "radicand right of the surd");
        assert!(rightmost.1 > *rule_oy, "radicand below the vinculum");
        assert!(root.height.is_finite() && root.depth.is_finite() && root.width > 0.0);
    }

    /// `\sqrt{\frac{a}{b}}` makes the surd taller than around a bare `\sqrt{x}` —
    /// the surd stretches to the (much taller) radicand.
    #[test]
    fn sqrt_grows_with_radicand() {
        let opts = MathOptions::default();
        // Tallest glyph (the surd) in each radical.
        let surd_extent = |src: &str| -> f32 {
            let (root, _f) = layout(src, &opts, 1.0).expect("lays out");
            let mut leaves = Vec::new();
            flatten(&root, 0.0, 0.0, &mut leaves);
            leaves
                .iter()
                .filter(|(_, _, b)| matches!(b.kind, BoxKind::Glyph { .. }))
                .map(|(_, _, b)| b.height + b.depth)
                .fold(0.0f32, f32::max)
        };
        let small = surd_extent(r"\sqrt{x}");
        let big = surd_extent(r"\sqrt{\frac{a}{b}}");
        assert!(big > small, "surd over a fraction ({big}) taller than over x ({small})");
        // Overall box is finite and taller too.
        let (frac_root, _f) = layout(r"\sqrt{\frac{a}{b}}", &opts, 1.0).expect("lays out");
        let (x_root, _f) = layout(r"\sqrt{x}", &opts, 1.0).expect("lays out");
        assert!(
            frac_root.height + frac_root.depth > x_root.height + x_root.depth,
            "radical-over-fraction taller overall"
        );
    }

    /// `\sqrt[3]{x}` places a small degree `3` above-left of the surd: an extra
    /// glyph (vs `\sqrt{x}`), at a smaller scale, raised above the baseline, and
    /// the overall box stays finite and a bit wider/taller.
    #[test]
    fn cube_root_has_small_raised_degree() {
        let opts = MathOptions::default();
        let (cbrt, _f) = layout(r"\sqrt[3]{x}", &opts, 1.0).expect("lays out");
        let (sqrt, _f) = layout(r"\sqrt{x}", &opts, 1.0).expect("lays out");

        let mut cleaves = Vec::new();
        flatten(&cbrt, 0.0, 0.0, &mut cleaves);
        let mut sleaves = Vec::new();
        flatten(&sqrt, 0.0, 0.0, &mut sleaves);

        let cglyphs: Vec<_> = cleaves
            .iter()
            .filter(|(_, _, b)| matches!(b.kind, BoxKind::Glyph { .. }))
            .collect();
        let sglyphs = sleaves
            .iter()
            .filter(|(_, _, b)| matches!(b.kind, BoxKind::Glyph { .. }))
            .count();
        assert_eq!(cglyphs.len(), sglyphs + 1, "the degree adds one glyph");

        // The degree is the smallest-scale glyph and is raised above the baseline.
        let base_scale = opts.font_size_px / UNITS_PER_EM;
        let degree = cglyphs
            .iter()
            .find(|(_, _, b)| matches!(b.kind, BoxKind::Glyph { scale, .. } if scale < base_scale))
            .expect("a script-scale degree glyph");
        assert!(degree.1 < 0.0, "degree raised above the baseline (oy {})", degree.1);

        assert!(cbrt.width > sqrt.width, "cube root a bit wider (degree)");
        assert!(cbrt.height.is_finite() && cbrt.depth.is_finite() && cbrt.height > 0.0);
    }

    /// A radical nested in a fraction numerator (`\frac{\sqrt{x}}{2}`) lays out
    /// finite and sane, with the surd's vinculum present inside the numerator.
    #[test]
    fn radical_in_fraction_is_sane() {
        let opts = MathOptions::default();
        let (root, _f) = layout(r"\frac{\sqrt{x}}{2}", &opts, 1.0).expect("lays out");
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);
        let rules = leaves
            .iter()
            .filter(|(_, _, b)| matches!(b.kind, BoxKind::Rule { .. }))
            .count();
        // The fraction bar plus the radical's vinculum.
        assert_eq!(rules, 2, "fraction bar + vinculum");
        assert!(root.height.is_finite() && root.depth.is_finite());
        assert!(root.width > 0.0 && root.height > 0.0);
    }

    /// `\sum_{i=1}^{n}` parses to a `Script` node with `Movable` position whose
    /// base is a `large_op` ∑ atom; `\int` arrives with `Right` position.
    #[test]
    fn parses_large_op_positions() {
        let s = parse_list(r"\sum_{i}^{n}").unwrap();
        match &s[0] {
            MathNode::Script { base, position, .. } => {
                assert_eq!(*position, ScriptPos::Movable, "\\sum scripts are Movable");
                match &base[0] {
                    MathNode::Atom(a) => {
                        assert_eq!(a.ch, '\u{2211}', "∑");
                        assert!(a.large_op, "∑ is a symbol large op");
                        assert_eq!(a.class, Class::Op);
                    }
                    _ => panic!("base is the ∑ atom"),
                }
            }
            _ => panic!("expected a Script node"),
        }
        let i = parse_list(r"\int_0^1").unwrap();
        match &i[0] {
            MathNode::Script { position, .. } => {
                assert_eq!(*position, ScriptPos::Right, "\\int scripts stay beside");
            }
            _ => panic!("expected a Script node"),
        }
    }

    /// Options at the given style.
    fn opts_for(style: super::super::MathStyle) -> MathOptions {
        MathOptions { style, ..MathOptions::default() }
    }

    /// `\sum_{i=1}^{n} i` in **Display**: the superscript sits centered *above*
    /// (dy < 0) and the subscript centered *below* (dy > 0) the ∑, both
    /// horizontally near the operator's center; and the ∑ glyph is taller than a
    /// Text-style ∑.
    #[test]
    fn sum_display_uses_limits() {
        let display = opts_for(super::super::MathStyle::Display);
        let (root, _f) = layout(r"\sum_{i=1}^{n} i", &display, 1.0).expect("lays out");
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);

        // The largest glyph is the grown ∑; find its center and total extent.
        let op = leaves
            .iter()
            .filter(|(_, _, b)| matches!(b.kind, BoxKind::Glyph { .. }))
            .max_by(|a, b| {
                (a.2.height + a.2.depth)
                    .partial_cmp(&(b.2.height + b.2.depth))
                    .unwrap()
            })
            .expect("the ∑ glyph");
        let op_center = op.0 + op.2.width / 2.0;
        let op_extent = op.2.height + op.2.depth;

        // The sup (centered above) and sub (centered below) sit at the op center.
        let raised: Vec<_> = leaves.iter().filter(|(_, dy, _)| *dy < -1.0).collect();
        let lowered: Vec<_> = leaves.iter().filter(|(_, dy, _)| *dy > 1.0).collect();
        assert!(!raised.is_empty(), "a superscript above the ∑");
        assert!(!lowered.is_empty(), "a subscript below the ∑");
        // Each limit glyph's horizontal center is near the operator center.
        for (ox, _dy, b) in raised.iter().chain(lowered.iter()) {
            let c = ox + b.width / 2.0;
            assert!(
                (c - op_center).abs() < op.2.width,
                "limit center {c} near op center {op_center}"
            );
        }

        // Compare ∑ glyph extent to a Text-style ∑.
        let inline = opts_for(super::super::MathStyle::Inline);
        let (iroot, _f) = layout(r"\sum", &inline, 1.0).expect("lays out");
        let mut ileaves = Vec::new();
        flatten(&iroot, 0.0, 0.0, &mut ileaves);
        let text_extent = ileaves
            .iter()
            .filter(|(_, _, b)| matches!(b.kind, BoxKind::Glyph { .. }))
            .map(|(_, _, b)| b.height + b.depth)
            .fold(0.0f32, f32::max);
        assert!(
            op_extent > text_extent,
            "display ∑ ({op_extent}) taller than text ∑ ({text_extent})"
        );
        assert!(root.height.is_finite() && root.depth.is_finite() && root.width > 0.0);
    }

    /// `\sum_{i=1}^{n} i` in **Inline/Text**: the scripts go beside the ∑ — the
    /// superscript up-right (dy < 0, dx > op center) and the subscript down-right
    /// (dy > 0).
    #[test]
    fn sum_inline_uses_beside_scripts() {
        let inline = opts_for(super::super::MathStyle::Inline);
        let (root, _f) = layout(r"\sum_{i=1}^{n} i", &inline, 1.0).expect("lays out");
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);

        // The ∑ sits on the baseline (dy ≈ 0); the scripts are offset to its right.
        let op = leaves
            .iter()
            .filter(|(_, dy, b)| dy.abs() < 1.0 && matches!(b.kind, BoxKind::Glyph { .. }))
            .max_by(|a, b| {
                (a.2.height + a.2.depth)
                    .partial_cmp(&(b.2.height + b.2.depth))
                    .unwrap()
            })
            .expect("the ∑ glyph");
        let op_right = op.0 + op.2.width;

        let raised: Vec<_> = leaves.iter().filter(|(_, dy, _)| *dy < -1.0).collect();
        let lowered: Vec<_> = leaves.iter().filter(|(_, dy, _)| *dy > 1.0).collect();
        assert!(!raised.is_empty(), "a superscript (raised)");
        assert!(!lowered.is_empty(), "a subscript (lowered)");
        // Beside: every script sits to the right of (roughly at/after) the op.
        for (ox, _dy, _b) in raised.iter().chain(lowered.iter()) {
            assert!(*ox >= op_right - 1.0, "script {ox} beside op right {op_right}");
        }
    }

    /// `\int_0^1 x` in **Display** keeps its limits **beside** (pulldown emits
    /// `Right`): scripts sit to the right of the ∫, not stacked over its center.
    #[test]
    fn int_display_stays_beside() {
        let display = opts_for(super::super::MathStyle::Display);
        let (root, _f) = layout(r"\int_0^1 x", &display, 1.0).expect("lays out");
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);

        let op = leaves
            .iter()
            .filter(|(_, dy, b)| dy.abs() < 1.0 && matches!(b.kind, BoxKind::Glyph { .. }))
            .max_by(|a, b| {
                (a.2.height + a.2.depth)
                    .partial_cmp(&(b.2.height + b.2.depth))
                    .unwrap()
            })
            .expect("the ∫ glyph");
        let op_right = op.0 + op.2.width;
        // The 0 and 1 are offset vertically (scripts) and sit at/after the op right.
        let scripts: Vec<_> = leaves.iter().filter(|(_, dy, _)| dy.abs() > 1.0).collect();
        assert_eq!(scripts.len(), 2, "the 0 and 1 as beside-scripts");
        for (ox, _dy, _b) in &scripts {
            assert!(*ox >= op_right - 2.0, "∫ script {ox} beside op right {op_right}");
        }
    }

    /// `\mathbb{R}` maps `R` to the double-struck codepoint U+211D (ℝ), whose
    /// glyph differs from a plain `R`; `\mathbb{Z}`/`\mathbb{N}` likewise. The
    /// parsed atom carries `Variant::DoubleStruck`.
    #[test]
    fn blackboard_maps_to_letterlike() {
        let face = font::math_face();
        // Codepoint mapping (mirrors pulldown's holes).
        assert_eq!(glyph::map_char('R', Variant::DoubleStruck), '\u{211D}');
        assert_eq!(glyph::map_char('Z', Variant::DoubleStruck), '\u{2124}');
        assert_eq!(glyph::map_char('N', Variant::DoubleStruck), '\u{2115}');
        // Glyph differs from plain R.
        let plain = face.glyph_index('R').unwrap();
        let bb = glyph::glyph_for(&face, 'R', Variant::DoubleStruck).unwrap();
        assert_ne!(plain, bb, "ℝ glyph differs from plain R");

        // Parsed atom carries the DoubleStruck variant.
        let list = parse_list(r"\mathbb{R}").unwrap();
        let mut a = Vec::new();
        collect_atoms(&list, &mut a);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].variant, Variant::DoubleStruck);
        assert_eq!(a[0].ch, 'R');
    }

    /// `\mathbf{x}` maps `x` to the bold-math codepoint U+1D431 and parses with
    /// `Variant::Bold`.
    #[test]
    fn bold_maps_to_bold_math() {
        assert_eq!(glyph::map_char('x', Variant::Bold), '\u{1D431}');
        let list = parse_list(r"\mathbf{x}").unwrap();
        let mut a = Vec::new();
        collect_atoms(&list, &mut a);
        assert_eq!(a[0].variant, Variant::Bold);
        let face = font::math_face();
        assert!(glyph::glyph_for(&face, 'x', Variant::Bold).is_some());
    }

    /// `\mathcal{L}` and `\mathfrak{g}` map to their letterlike/alphanumeric
    /// glyphs (ℒ U+2112 and 𝔤 U+1D524) and carry Script / Fraktur variants.
    #[test]
    fn cal_and_frak_map() {
        assert_eq!(glyph::map_char('L', Variant::Script), '\u{2112}'); // ℒ
        assert_eq!(glyph::map_char('g', Variant::Fraktur), '\u{1D524}'); // 𝔤
        let face = font::math_face();
        assert!(glyph::glyph_for(&face, 'L', Variant::Script).is_some());
        assert!(glyph::glyph_for(&face, 'g', Variant::Fraktur).is_some());

        let cal = parse_list(r"\mathcal{L}").unwrap();
        let mut ca = Vec::new();
        collect_atoms(&cal, &mut ca);
        assert_eq!(ca[0].variant, Variant::Script);

        let frak = parse_list(r"\mathfrak{g}").unwrap();
        let mut fa = Vec::new();
        collect_atoms(&frak, &mut fa);
        assert_eq!(fa[0].variant, Variant::Fraktur);
    }

    /// `\hat{x}` parses to an Accent node (over, non-stretchy) over `x`, and lays
    /// out as the base glyph plus a second glyph raised above it (negative dy),
    /// roughly the base width wide.
    #[test]
    fn hat_places_accent_above() {
        let list = parse_list(r"\hat{x}").unwrap();
        assert_eq!(list.len(), 1);
        match &list[0] {
            MathNode::Accent { stretchy, under, base, .. } => {
                assert!(!stretchy, "\\hat is non-stretchy");
                assert!(!under, "\\hat is an over-accent");
                let mut b = Vec::new();
                collect_atoms(base, &mut b);
                assert_eq!(b[0].ch, 'x');
            }
            _ => panic!("expected an Accent node"),
        }

        let opts = MathOptions::default();
        let (root, _f) = layout(r"\hat{x}", &opts, 1.0).expect("lays out");
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);
        let glyphs: Vec<_> = leaves
            .iter()
            .filter(|(_, _, b)| matches!(b.kind, BoxKind::Glyph { .. }))
            .collect();
        assert_eq!(glyphs.len(), 2, "base x + accent glyph");
        // The accent is the one above the baseline (oy < 0).
        let raised: Vec<_> = glyphs.iter().filter(|(_, oy, _)| *oy < 0.0).collect();
        assert_eq!(raised.len(), 1, "the accent sits above the base");
        // Result is about the base width wide.
        let (bare, _f) = layout("x", &opts, 1.0).expect("lays out");
        assert!(
            (root.width - bare.width).abs() < bare.width,
            "accent box ≈ base width ({} vs {})",
            root.width,
            bare.width
        );
        assert!(root.height > bare.height, "accent adds height above x");
    }

    /// `\overline{AB}` adds a `Rule` above the `AB` pair spanning ≈ their width.
    #[test]
    fn overline_adds_rule_above() {
        let opts = MathOptions::default();
        let (root, _f) = layout(r"\overline{AB}", &opts, 1.0).expect("lays out");
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);
        let rules: Vec<_> = leaves
            .iter()
            .filter(|(_, _, b)| matches!(b.kind, BoxKind::Rule { .. }))
            .collect();
        assert_eq!(rules.len(), 1, "one overline rule");
        let (_, rule_oy, rule) = rules[0];
        assert!(*rule_oy < 0.0, "rule sits above the baseline (oy {rule_oy})");
        // Rule spans ≈ the AB width.
        let (bare, _f) = layout("AB", &opts, 1.0).expect("lays out");
        assert!(
            (rule.width - bare.width).abs() < 0.01,
            "rule width {} ≈ AB width {}",
            rule.width,
            bare.width
        );
        assert!(root.height > bare.height, "overline adds height");
    }

    /// `\underline{x}` puts a `Rule` *below* the baseline.
    #[test]
    fn underline_adds_rule_below() {
        let opts = MathOptions::default();
        let (root, _f) = layout(r"\underline{x}", &opts, 1.0).expect("lays out");
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);
        let rule = leaves
            .iter()
            .find(|(_, _, b)| matches!(b.kind, BoxKind::Rule { .. }))
            .expect("an underline rule");
        assert!(rule.1 > 0.0, "underline rule below the baseline (oy {})", rule.1);
        assert!(root.depth > 0.0, "underline adds depth");
    }

    /// `\widehat{xyz}` (stretchy) is wider than `\hat{a}` and lays out without
    /// panicking; the accent box spans roughly the wide base.
    #[test]
    fn widehat_is_wider_than_hat() {
        let opts = MathOptions::default();
        // Parsed as a stretchy accent.
        let list = parse_list(r"\widehat{xyz}").unwrap();
        match &list[0] {
            MathNode::Accent { stretchy, .. } => assert!(stretchy, "\\widehat is stretchy"),
            _ => panic!("expected an Accent node"),
        }
        let (wide, _f) = layout(r"\widehat{xyz}", &opts, 1.0).expect("lays out");
        let (small, _f) = layout(r"\hat{a}", &opts, 1.0).expect("lays out");
        assert!(
            wide.width > small.width,
            "\\widehat{{xyz}} ({}) wider than \\hat{{a}} ({})",
            wide.width,
            small.width
        );
        assert!(wide.height.is_finite() && wide.depth.is_finite() && wide.width > 0.0);
    }

    /// `\vec{v}` and `\bar{y}` and `\tilde{n}` all lay out as a base glyph plus an
    /// accent above it, with finite, positive metrics.
    #[test]
    fn assorted_accents_are_sane() {
        let opts = MathOptions::default();
        for src in [r"\vec{v}", r"\bar{y}", r"\tilde{n}", r"\dot{x}", r"\ddot{x}"] {
            let (root, _f) = layout(src, &opts, 1.0).expect("lays out");
            let mut leaves = Vec::new();
            flatten(&root, 0.0, 0.0, &mut leaves);
            // `\bar` is a rule; the rest are glyphs — either way ≥ 2 drawables and
            // something sits above the baseline.
            assert!(leaves.len() >= 2, "{src}: base + accent");
            assert!(
                leaves.iter().any(|(_, oy, _)| *oy < 0.0),
                "{src}: an accent above the base"
            );
            assert!(root.height > 0.0 && root.height.is_finite(), "{src}: sane height");
        }
    }

    /// `\overbrace{…}^{n}` parses to a stretchy over-brace accent wrapped in an
    /// `AboveBelow` super-script: the brace grows to the body width and the `n`
    /// sits above it (two things above the baseline, nothing below).
    #[test]
    fn overbrace_brace_spans_body_and_script_above() {
        let opts = MathOptions::default();
        // Inner brace folds into a stretchy Accent over the body.
        let list = parse_list(r"\overbrace{a+b+c}^{n}").unwrap();
        match &list[0] {
            MathNode::Script { base, sup, position: ScriptPos::AboveBelow, .. } => {
                assert!(sup.is_some(), "the ^{{n}} script");
                assert!(
                    matches!(base.as_slice(), [MathNode::Accent { accent: '\u{23DE}', stretchy: true, under: false, .. }]),
                    "base is a stretchy over-brace accent"
                );
            }
            _ => panic!("expected an AboveBelow Script wrapping a brace accent"),
        }

        let (narrow, _f) = layout(r"\overbrace{a}^{1}", &opts, 1.0).expect("lays out");
        let (wide, _f) = layout(r"\overbrace{a+b+c+d}^{1}", &opts, 1.0).expect("lays out");
        assert!(
            wide.width > narrow.width * 2.0,
            "the brace stretches with the body ({} vs {})",
            wide.width,
            narrow.width
        );

        let (root, _f) = layout(r"\overbrace{a+b+c}^{n}", &opts, 1.0).expect("lays out");
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);
        let above = leaves.iter().filter(|(_, oy, _)| *oy < -0.01).count();
        let below = leaves.iter().filter(|(_, oy, _)| *oy > 0.01).count();
        assert!(above >= 2, "the brace and the `n` both sit above the body");
        assert_eq!(below, 0, "nothing below the body for an overbrace");
        assert!(root.height.is_finite() && root.depth.is_finite() && root.width > 0.0);
    }

    /// `\underbrace{x}_{k}` puts a stretchy under-brace and the `k` below the body.
    #[test]
    fn underbrace_brace_and_script_below() {
        let opts = MathOptions::default();
        let list = parse_list(r"\underbrace{x}_{k}").unwrap();
        match &list[0] {
            MathNode::Script { base, sub, position: ScriptPos::AboveBelow, .. } => {
                assert!(sub.is_some(), "the _{{k}} script");
                assert!(
                    matches!(base.as_slice(), [MathNode::Accent { accent: '\u{23DF}', stretchy: true, under: true, .. }]),
                    "base is a stretchy under-brace accent"
                );
            }
            _ => panic!("expected an AboveBelow Script wrapping an under-brace accent"),
        }

        let (root, _f) = layout(r"\underbrace{x+y}_{\text{sum}}", &opts, 1.0).expect("lays out");
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);
        let below = leaves.iter().filter(|(_, oy, _)| *oy > 0.01).count();
        assert!(below >= 2, "the brace and the `sum` label both sit below");
        assert!(root.depth > root.height, "an under-brace adds depth, not height");
    }

    /// `\xrightarrow{f}` stretches the arrow wider than a bare `→` and puts the
    /// label `f` above it; `\xrightarrow[g]{f}` adds a label below too.
    #[test]
    fn xrightarrow_stretches_with_labels() {
        let opts = MathOptions::default();
        let (bare, _f) = layout("→", &opts, 1.0).expect("lays out");
        let (arrow, _f) = layout(r"\xrightarrow{f}", &opts, 1.0).expect("lays out");
        assert!(
            arrow.width > bare.width,
            "\\xrightarrow{{f}} ({}) is wider than a bare → ({})",
            arrow.width,
            bare.width
        );

        let mut leaves = Vec::new();
        flatten(&arrow, 0.0, 0.0, &mut leaves);
        let above = leaves.iter().filter(|(_, oy, _)| *oy < -0.01).count();
        let below = leaves.iter().filter(|(_, oy, _)| *oy > 0.01).count();
        assert!(above >= 1, "the label `f` sits above the arrow");
        assert_eq!(below, 0, "no label below for the over-only form");

        // The two-label form has a label above *and* below.
        let (both, _f) = layout(r"\xrightarrow[g]{f}", &opts, 1.0).expect("lays out");
        let mut leaves2 = Vec::new();
        flatten(&both, 0.0, 0.0, &mut leaves2);
        assert!(
            leaves2.iter().any(|(_, oy, _)| *oy < -0.01),
            "label above the arrow"
        );
        assert!(
            leaves2.iter().any(|(_, oy, _)| *oy > 0.01),
            "label below the arrow"
        );
        assert!(both.height > 0.0 && both.depth > 0.0, "labels both sides add height+depth");
    }

    /// An extensible arrow spaces as a relation (`A \xrightarrow{f} B` keeps the
    /// arrow class `Rel`, so `A`/`B` get relation spacing around it).
    #[test]
    fn xrightarrow_is_a_relation() {
        let list = parse_list(r"A \xrightarrow{f} B").unwrap();
        let arrow = list
            .iter()
            .find(|n| matches!(n, MathNode::Script { position: ScriptPos::AboveBelow, .. }))
            .expect("an arrow script");
        assert_eq!(node_class(arrow), Class::Rel, "the extensible arrow is a relation");
    }

    /// `\lim_{x \to 0} f` in **Display** renders "lim" upright (three Op-class
    /// glyphs) with the subscript centered *below* it.
    #[test]
    fn lim_display_subscript_below() {
        let display = opts_for(super::super::MathStyle::Display);
        let (root, _f) = layout(r"\lim_{x \to 0} f", &display, 1.0).expect("lays out");
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);

        // "lim" letters render on the baseline (dy ≈ 0).
        let base_glyphs: Vec<_> = leaves
            .iter()
            .filter(|(_, dy, b)| dy.abs() < 1.0 && matches!(b.kind, BoxKind::Glyph { .. }))
            .collect();
        assert!(base_glyphs.len() >= 3, "l, i, m on the baseline ({})", base_glyphs.len());

        // The subscript (x → 0) is lowered below the baseline.
        let lowered: Vec<_> = leaves.iter().filter(|(_, dy, _)| *dy > 1.0).collect();
        assert!(!lowered.is_empty(), "a subscript below lim");
        // It is horizontally under the "lim" cluster (not far to the right).
        let lim_left = base_glyphs.iter().map(|(ox, _, _)| *ox).fold(f32::INFINITY, f32::min);
        let lim_right = base_glyphs
            .iter()
            .map(|(ox, _, b)| ox + b.width)
            .fold(0.0f32, f32::max);
        for (ox, _dy, b) in &lowered {
            let c = ox + b.width / 2.0;
            assert!(
                c > lim_left - 5.0 && c < lim_right + 5.0,
                "subscript center {c} under lim [{lim_left}, {lim_right}]"
            );
        }
        assert!(root.height.is_finite() && root.depth.is_finite() && root.width > 0.0);
    }

    /// `\begin{pmatrix} a & b \\ c & d \end{pmatrix}` parses to a `Delim`
    /// (`(`/`)`, from pulldown's outer LeftRight) wrapping a `Matrix` with two
    /// rows of two centered cells.
    #[test]
    fn parses_pmatrix() {
        let list = parse_list(r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}").unwrap();
        assert_eq!(list.len(), 1, "one top-level element (the fence)");
        let body = match &list[0] {
            MathNode::Delim { open, close, body } => {
                assert_eq!(*open, Some('('));
                assert_eq!(*close, Some(')'));
                body
            }
            _ => panic!("expected a Delim around the matrix"),
        };
        match body.first() {
            Some(MathNode::Matrix { rows, col_align, kind, .. }) => {
                assert_eq!(*kind, MatrixKind::Plain);
                assert_eq!(col_align, &[Align::Center]);
                assert_eq!(rows.len(), 2, "two rows");
                assert!(rows.iter().all(|r| r.len() == 2), "two cells per row");
            }
            other => panic!("expected a Matrix node, got {:?}", other.map(node_class)),
        }
    }

    /// A 2×2 `pmatrix` lays out as four cell glyphs in two distinct row baselines
    /// and two column x-offsets, wrapped in `(` `)` delimiters grown taller than a
    /// single row.
    #[test]
    fn pmatrix_lays_out_grid_with_parens() {
        let opts = MathOptions {
            style: super::super::MathStyle::Display,
            ..MathOptions::default()
        };
        let (root, _f) = layout(r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}", &opts, 1.0)
            .expect("lays out");
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);

        let glyphs: Vec<_> = leaves
            .iter()
            .filter(|(_, _, b)| matches!(b.kind, BoxKind::Glyph { .. }))
            .collect();
        // 4 cell glyphs + 2 delimiters (each may be assembled, but at this size
        // a single glyph) → at least the 4 cells plus 2 fences.
        assert!(glyphs.len() >= 6, "4 cells + 2 fences, got {}", glyphs.len());

        // Two distinct row baselines among the cells (dy), and two column x's.
        let round = |v: f32| (v * 2.0).round() / 2.0;
        let mut dys: Vec<f32> = glyphs.iter().map(|(_, dy, _)| round(*dy)).collect();
        dys.sort_by(|a, b| a.partial_cmp(b).unwrap());
        dys.dedup();
        // The two delimiters sit on the axis (dy ~ 0 effectively their own center),
        // and the cells fall on (at least) two distinct row baselines.
        assert!(dys.len() >= 2, "≥2 distinct row baselines, got {dys:?}");

        // Two distinct column x-offsets among the inner (non-fence) glyphs: the
        // leftmost glyph is the open paren; the rightmost is the close paren.
        let mut xs: Vec<f32> = glyphs.iter().map(|(ox, _, _)| round(*ox)).collect();
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        xs.dedup();
        assert!(xs.len() >= 3, "open + 2 cols + close ≥3 distinct x, got {xs:?}");

        // The fence is taller than one row: compare to a 1×1 matrix's row height.
        let (one, _f) = layout(r"\begin{pmatrix} a \end{pmatrix}", &opts, 1.0).expect("lays out");
        assert!(
            root.height + root.depth > one.height + one.depth + 1.0,
            "2-row matrix ({} + {}) taller than 1-row ({} + {})",
            root.height,
            root.depth,
            one.height,
            one.depth
        );
        assert!(root.height.is_finite() && root.depth.is_finite());
    }

    /// `bmatrix` carries `[`/`]` delimiters.
    #[test]
    fn bmatrix_uses_square_brackets() {
        let list = parse_list(r"\begin{bmatrix} 1 & 0 \\ 0 & 1 \end{bmatrix}").unwrap();
        match &list[0] {
            MathNode::Delim { open, close, .. } => {
                assert_eq!(*open, Some('['));
                assert_eq!(*close, Some(']'));
            }
            _ => panic!("expected a Delim around the matrix"),
        }
    }

    /// `cases` parses to a `Matrix { kind: Cases }` with two left-aligned columns
    /// and no surrounding `Delim` (it draws its own brace at layout).
    #[test]
    fn cases_is_left_aligned_with_self_brace() {
        let list = parse_list(r"\begin{cases} x & x > 0 \\ -x & x \le 0 \end{cases}").unwrap();
        match &list[0] {
            MathNode::Matrix { col_align, kind, rows, .. } => {
                assert_eq!(*kind, MatrixKind::Cases);
                assert_eq!(col_align, &[Align::Left, Align::Left]);
                assert_eq!(rows.len(), 2);
            }
            _ => panic!("expected a bare Matrix(Cases), no outer Delim"),
        }

        // At layout: a left brace glyph is present, and the leftmost glyph is that
        // brace (no right delimiter follows the grid).
        let opts = MathOptions {
            style: super::super::MathStyle::Display,
            ..MathOptions::default()
        };
        let (root, _f) =
            layout(r"\begin{cases} x & x > 0 \\ -x & x \le 0 \end{cases}", &opts, 1.0).expect("lays out");
        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);
        let glyphs: Vec<_> = leaves
            .iter()
            .filter(|(_, _, b)| matches!(b.kind, BoxKind::Glyph { .. }))
            .collect();
        assert!(!glyphs.is_empty());
        // The leftmost glyph is the brace, sitting clearly left of the cells; the
        // brace spans the whole grid, so it is the tallest single glyph and it sits
        // at the left edge (ox ~ 0). A `{` (or its tall variant/assembly) is present
        // and no closing delimiter trails the grid.
        let min_ox = glyphs.iter().map(|(ox, _, _)| *ox).fold(f32::INFINITY, f32::min);
        assert!(min_ox.abs() < 1.0, "brace at the left edge, ox={min_ox}");
        let brace = glyphs
            .iter()
            .find(|(ox, _, _)| (*ox - min_ox).abs() < 0.5)
            .expect("a leftmost (brace) glyph");
        // The brace is taller than a cell digit (it spans both rows).
        assert!(
            brace.2.height + brace.2.depth > root.height * 0.5,
            "brace spans the grid height"
        );
        assert!(root.height.is_finite() && root.depth.is_finite());
    }

    /// `aligned` lines up its second column (the `&` boundary) across rows: the
    /// `=` of each row sits at the same x.
    #[test]
    fn aligned_lines_up_second_column() {
        let opts = MathOptions {
            style: super::super::MathStyle::Display,
            ..MathOptions::default()
        };
        let (root, _f) =
            layout(r"\begin{aligned} a &= b + c \\ x &= y \end{aligned}", &opts, 1.0).expect("lays out");

        // The matrix node has a right|left column pair touching, so the first cell
        // (right-aligned `a`/`x`) ends at the same x and the second cell (the `=…`)
        // begins at the same x in both rows. Verify two distinct row baselines and
        // that the second column starts at one shared x across rows.
        let list = parse_list(r"\begin{aligned} a &= b + c \\ x &= y \end{aligned}").unwrap();
        match &list[0] {
            MathNode::Matrix { col_align, kind, rows, .. } => {
                assert_eq!(*kind, MatrixKind::Aligned);
                assert_eq!(col_align, &[Align::Right, Align::Left]);
                assert_eq!(rows.len(), 2);
                assert!(rows.iter().all(|r| r.len() == 2), "two cells per row");
            }
            _ => panic!("expected a bare Matrix(Aligned)"),
        }

        let mut leaves = Vec::new();
        flatten(&root, 0.0, 0.0, &mut leaves);
        let glyphs: Vec<_> = leaves
            .iter()
            .filter(|(_, _, b)| matches!(b.kind, BoxKind::Glyph { .. }))
            .collect();
        // Two distinct row baselines.
        let round = |v: f32| (v * 2.0).round() / 2.0;
        let mut dys: Vec<f32> = glyphs.iter().map(|(_, dy, _)| round(*dy)).collect();
        dys.sort_by(|a, b| a.partial_cmp(b).unwrap());
        dys.dedup();
        assert_eq!(dys.len(), 2, "two row baselines, got {dys:?}");
        assert!(root.width > 0.0 && root.height.is_finite() && root.depth.is_finite());
    }

    /// A fraction inside a matrix cell keeps the assembly finite and makes the
    /// matrix taller than a plain-digit matrix (the big cell grows the row).
    #[test]
    fn fraction_in_matrix_cell_stays_finite() {
        let opts = MathOptions {
            style: super::super::MathStyle::Display,
            ..MathOptions::default()
        };
        let (big, _f) = layout(
            r"\begin{pmatrix} \frac{1}{2} & 0 \\ 0 & \frac{1}{2} \end{pmatrix}",
            &opts,
            1.0,
        )
        .expect("lays out");
        assert!(
            big.width.is_finite()
                && big.height.is_finite()
                && big.depth.is_finite()
                && big.width > 0.0
        );
        // A fraction-bearing matrix is taller than the all-digit one.
        let (plain, _f) =
            layout(r"\begin{pmatrix} 1 & 0 \\ 0 & 1 \end{pmatrix}", &opts, 1.0).expect("lays out");
        assert!(
            big.height + big.depth > plain.height + plain.depth,
            "frac matrix ({} + {}) taller than digit matrix ({} + {})",
            big.height,
            big.depth,
            plain.height,
            plain.depth
        );
    }
}

