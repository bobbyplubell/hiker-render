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

use parse::{
    parse_list, Align, Atom, BarThickness, Class, MathList, MathNode, MatrixKind, ScriptPos, Style,
};
use super::MathOptions;
use crate::font;

mod accent;
pub mod delim;
pub mod glyph;
mod matrix;
pub mod parse;
mod radical;

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
            ctx.layout_radical(index.as_ref(), radicand, style, cramped)
        }),
        MathNode::Accent { accent, stretchy, under, base, color } => ctx.with_color(*color, || {
            ctx.layout_accent(*accent, *stretchy, *under, base, style, cramped)
        }),
        MathNode::Matrix { rows, col_align, kind, col_seps, row_lines } => {
            ctx.layout_matrix(rows, col_align, *kind, col_seps, row_lines, style)
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
mod tests;

#[cfg(test)]
pub mod tests_geometry;

