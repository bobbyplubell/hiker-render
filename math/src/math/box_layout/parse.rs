//! Parse pass: pulldown-latex event stream → the math-list IR tree.
//!
//! This module owns the intermediate representation the layout engine consumes —
//! the [`MathNode`] tree (a [`MathList`] of nodes: atoms, groups, scripts,
//! fractions, radicals, delimiters, matrices, accents) plus the shared TeX
//! vocabulary ([`Class`], [`Style`]) — and the recursive reader that builds it
//! from the parser's buffered events. It does no box layout: it turns LaTeX into
//! the typed tree, and [`super`]'s layout pass turns that tree into boxes.

use super::glyph::{self, Variant};

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
pub(crate) struct Atom {
    /// The character to render from the math font.
    pub(crate) ch: char,
    /// The TeX class, used for spacing and (for [`Class::Ord`]) italicization.
    pub(crate) class: Class,
    /// The letterform to render in (italic for variables, upright otherwise).
    pub(crate) variant: Variant,
    /// True for a *symbol* large operator (`\sum`, `\int`, `\prod`, `\bigcup`,
    /// …): in Display style its glyph grows to `display_operator_min_height` and
    /// it straddles the math axis. Named operators (`\lim`, `\max`) are plain
    /// upright [`Class::Op`] atoms with this `false` (no glyph growth).
    pub(crate) large_op: bool,
    /// The straight RGBA fill in effect at parse time (`None` = inherit the
    /// default [`MathOptions::color`]), set by an enclosing `\color`/`\textcolor`
    /// scope. Resolved to a concrete color when the atom's glyph is laid out.
    pub(crate) color: Option<[u8; 4]>,
}

/// Where a [`MathNode::Script`]'s scripts sit, mirroring pulldown's
/// [`pulldown_latex::event::ScriptPosition`]: beside the base (`Right`), stacked
/// above/below (`AboveBelow`), or `Movable` (above/below in Display, beside in
/// Text — what `\sum` / `\lim` use).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScriptPos {
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
pub(crate) enum BarThickness {
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
pub(crate) enum Align {
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
pub(crate) enum MatrixKind {
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
    pub(crate) fn smaller(self) -> Style {
        match self {
            Style::Display | Style::Text => Style::Script,
            Style::Script | Style::ScriptScript => Style::ScriptScript,
        }
    }

    /// The style a fraction's numerator/denominator render in (TeXbook rule 15b):
    /// Display→Text, Text→Script, Script/ScriptScript→ScriptScript.
    pub(crate) fn frac_child(self) -> Style {
        match self {
            Style::Display => Style::Text,
            Style::Text => Style::Script,
            Style::Script | Style::ScriptScript => Style::ScriptScript,
        }
    }

    /// Whether this style is display-sized (selects the larger display-style
    /// fraction shift/gap constants).
    pub(crate) fn is_display(self) -> bool {
        matches!(self, Style::Display)
    }

    /// Bin/Rel inter-atom spacing is suppressed in the two script styles.
    pub(crate) fn is_tight(self) -> bool {
        matches!(self, Style::Script | Style::ScriptScript)
    }
}

/// The math-list tree IR: a row is a list of nodes, each of which may itself be a
/// nested row (a `{…}` group) or carry scripts. This replaces layer 2's flat
/// atom row so scripts (and later fractions/radicals/delimiters) compose cleanly.
pub(crate) type MathList = Vec<MathNode>;

/// One element of a [`MathList`].
pub(crate) enum MathNode {
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
pub(crate) fn parse_list(src: &str) -> Option<MathList> {
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
pub(crate) fn variant_for(font: Option<pulldown_latex::event::Font>, ch: char) -> Variant {
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
