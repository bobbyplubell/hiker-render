//! Stretchy delimiter sizing and assembly (`\left … \right`, fences).
//!
//! Given a delimiter character and a *target size* (the total height+depth the
//! delimiter must span, in px), [`sized_delim`] returns a [`Box`] for the
//! delimiter that brackets content symmetrically about the **math axis**:
//!
//! 1. Map the char to its base glyph in the math face.
//! 2. Read the OpenType **MATH `MathVariants`** table's *vertical glyph
//!    construction* for that glyph (ttf-parser 0.25:
//!    `math.variants.vertical_constructions.get(gid)` → a
//!    [`ttf_parser::math::GlyphConstruction`] with a list of
//!    `GlyphVariant { variant_glyph, advance_measurement }` records and an
//!    optional [`ttf_parser::math::GlyphAssembly`] of repeatable parts).
//! 3. Pick the **smallest variant** whose advance (height) ≥ target. If none is
//!    large enough but an assembly exists, **assemble** the glyph from its parts
//!    (top / repeatable extender(s) / bottom, plus any non-extender middles)
//!    stacked vertically to reach the target.
//! 4. If the font carries no vertical construction at all, fall back to the base
//!    glyph drawn unscaled (never crash, never panic).
//!
//! The sizing/selection logic ports KaTeX `src/delimiter.ts`
//! (`makeLeftRightDelim` for the target, `traverseSequence` for variant
//! selection, `makeStackedDelim` for assembly). The result is a single
//! [`BoxKind::Glyph`] for a variant or an [`BoxKind::Hbox`] of stacked glyph
//! [`Child`]ren for an assembly, with its baseline placed so the delimiter is
//! vertically centered on the axis (see [`sized_delim`]).
//!
//! This module is intentionally reusable: a future radical layer can call
//! [`sized_delim`] (or [`vertical_glyph`]) to size a surd to its radicand.

use ttf_parser::{Face, GlyphId};

use super::box_layout::{Box, BoxKind, Child};

/// STIX Two Math is a 1000-unit em (font units → px = `em / 1000`).
const UNITS_PER_EM: f32 = 1000.0;

/// A delimiter sized (and centered on the math axis) to span `target_px`.
///
/// `ch` is the delimiter character (e.g. `(`, `[`, `{`, `|`); `axis_px` is the
/// math axis height in px (the assembled box is centered on it). `em_px` is the
/// base em in px (font-units → px scale). Returns `None` only when the font has
/// no glyph for `ch` at all (a null `.` delimiter is handled by the caller, which
/// simply contributes no box).
///
/// The returned box's `height`/`depth` split the delimiter's total extent so it
/// is centered on the axis: `height = total/2 + axis`, `depth = total/2 - axis`.
/// The painter draws glyph children relative to this baseline.
pub fn sized_delim(
    face: &Face<'static>,
    ch: char,
    target_px: f32,
    axis_px: f32,
    em_px: f32,
    color: [u8; 4],
) -> Option<Box> {
    let scale = em_px / UNITS_PER_EM;
    let gid = face.glyph_index(ch)?;

    // Build the (un-centered) delimiter box: its baseline sits at the *middle*
    // of the glyph/stack, i.e. height == depth == total/2, so that re-centering
    // on the axis is a simple split below.
    let inner = vertical_glyph(face, gid, target_px, scale, color);

    // Total extent we actually achieved (variant advance or assembly height).
    let total = inner.height + inner.depth;
    let half = total / 2.0;

    // Center on the axis: the box baseline is the row baseline, and we want the
    // glyph's vertical midpoint to sit on the axis. We re-home the inner box so
    // its own midpoint lands `axis_px` above the baseline.
    //
    // `inner` is built with its midpoint at its own baseline (height==depth==half),
    // so shifting its baseline *up* by `axis_px` (dy = -axis_px) puts the midpoint
    // on the axis. The composite then reaches `half + axis` up and `half - axis` down.
    let height = half + axis_px;
    let depth = (half - axis_px).max(0.0);
    let width = inner.width;

    Some(Box {
        width,
        height,
        depth,
        kind: BoxKind::Hbox {
            children: vec![Child { dx: 0.0, dy: -axis_px, b: inner }],
        },
    })
}

/// Build a vertical delimiter glyph (or stacked assembly) of base glyph `gid`
/// reaching at least `target_px`, with its **vertical midpoint on its own
/// baseline** (so `height == depth`). `scale` is font-units → px.
///
/// Selection order (KaTeX `traverseSequence`):
/// * the base glyph itself if it is already ≥ target;
/// * otherwise the smallest registered *variant* whose advance ≥ target;
/// * otherwise the largest variant available, unless an *assembly* exists and can
///   grow further, in which case we assemble parts to reach the target.
///
/// Reusable by a future radical layer (size a surd to its radicand).
pub fn vertical_glyph(
    face: &Face<'static>,
    gid: GlyphId,
    target_px: f32,
    scale: f32,
    color: [u8; 4],
) -> Box {
    let variants = face.tables().math.and_then(|m| m.variants);

    // Without a MATH variants table, fall back to the base glyph unscaled.
    let Some(variants) = variants else {
        return centered_glyph(face, gid, scale, color);
    };
    let Some(construction) = variants.vertical_constructions.get(gid) else {
        return centered_glyph(face, gid, scale, color);
    };

    // Track the largest variant (its advance, in px) so we know whether an
    // assembly is actually needed.
    let mut best: Option<(GlyphId, f32)> = None; // (glyph, advance_px) ≥ target
    let mut largest: Option<(GlyphId, f32)> = None;
    for v in construction.variants {
        let adv = v.advance_measurement as f32 * scale;
        match largest {
            Some((_, a)) if a >= adv => {}
            _ => largest = Some((v.variant_glyph, adv)),
        }
        if adv >= target_px {
            // Smallest variant that is large enough wins.
            match best {
                Some((_, a)) if a <= adv => {}
                _ => best = Some((v.variant_glyph, adv)),
            }
        }
    }

    if let Some((vg, _adv)) = best {
        return centered_glyph(face, vg, scale, color);
    }

    // No single variant is large enough: assemble from parts if we can, else use
    // the largest variant (or the base glyph) we have.
    if let Some(assembly) = construction.assembly {
        if assembly.parts.len() > 0 {
            return assemble(face, &variants, assembly, target_px, scale, color);
        }
    }

    if let Some((vg, _)) = largest {
        return centered_glyph(face, vg, scale, color);
    }
    centered_glyph(face, gid, scale, color)
}

/// Build a **horizontally** stretchy glyph (e.g. a wide hat / tilde / arrow over
/// an accent's base) of base glyph `gid` reaching at least `target_px` wide,
/// returning a plain glyph [`Box`] on the natural baseline. `scale` is
/// font-units → px.
///
/// Mirrors [`vertical_glyph`] but reads the MATH `variants.horizontal_constructions`:
/// the smallest registered variant whose advance (width) ≥ target wins. If no
/// single variant is wide enough, the glyph is **assembled** from its construction
/// parts (left / repeatable extender(s) / right) via [`assemble_horizontal`],
/// growing to reach `target_px` (this is what makes wide braces and extensible
/// arrows actually stretch). Failing that, it falls back to the largest registered
/// variant, then the base glyph. Returns the base glyph unscaled when the font has
/// no horizontal construction for `gid`.
pub fn horizontal_glyph(
    face: &Face<'static>,
    gid: GlyphId,
    target_px: f32,
    scale: f32,
    color: [u8; 4],
) -> Box {
    let plain = |g: GlyphId| -> Box {
        let (h, d) = glyph_extents(face, g, scale);
        let advance = (face.glyph_hor_advance(g).unwrap_or(0) as f32) * scale;
        Box {
            width: advance,
            height: h,
            depth: d,
            kind: BoxKind::Glyph { gid: g, scale, color },
        }
    };

    let Some(variants) = face.tables().math.and_then(|m| m.variants) else {
        return plain(gid);
    };
    let Some(construction) = variants.horizontal_constructions.get(gid) else {
        return plain(gid);
    };

    let mut best: Option<(GlyphId, f32)> = None; // smallest variant ≥ target
    let mut largest: Option<(GlyphId, f32)> = None;
    for v in construction.variants {
        let adv = v.advance_measurement as f32 * scale;
        match largest {
            Some((_, a)) if a >= adv => {}
            _ => largest = Some((v.variant_glyph, adv)),
        }
        if adv >= target_px {
            match best {
                Some((_, a)) if a <= adv => {}
                _ => best = Some((v.variant_glyph, adv)),
            }
        }
    }

    if let Some((vg, _)) = best {
        return plain(vg);
    }

    // No single variant is wide enough: assemble from parts (left→right) if the
    // font supplies an assembly, else fall back to the largest variant/base glyph.
    if let Some(assembly) = construction.assembly {
        if assembly.parts.len() > 0 {
            return assemble_horizontal(face, &variants, assembly, target_px, scale, color);
        }
    }

    if let Some((vg, _)) = largest {
        return plain(vg);
    }
    plain(gid)
}

/// Assemble a wide horizontal glyph (brace / extensible arrow / over-paren) by
/// laying out the assembly's parts **left→right** to reach `target_px`, repeating
/// any extender parts as needed. The horizontal mirror of [`assemble`]: parts are
/// listed left→right in the font, each carrying a `full_advance` (its own width);
/// successive parts overlap by exactly `min_connector_overlap`. Extenders
/// (`part_flags.extender()`) may be repeated; non-extenders appear once.
///
/// The result is a plain [`BoxKind::Hbox`] sitting on its natural baseline (the
/// row baseline of the part glyphs), so callers position it like any glyph box.
fn assemble_horizontal(
    face: &Face<'static>,
    variants: &ttf_parser::math::Variants<'static>,
    assembly: ttf_parser::math::GlyphAssembly<'static>,
    target_px: f32,
    scale: f32,
    color: [u8; 4],
) -> Box {
    let overlap = variants.min_connector_overlap as f32 * scale;

    let parts: Vec<ttf_parser::math::GlyphPart> = assembly.parts.into_iter().collect();
    let n_ext = parts.iter().filter(|p| p.part_flags.extender()).count();
    let fixed_advance: f32 = parts
        .iter()
        .filter(|p| !p.part_flags.extender())
        .map(|p| p.full_advance as f32 * scale)
        .sum();
    let ext_advance: f32 = parts
        .iter()
        .filter(|p| p.part_flags.extender())
        .map(|p| p.full_advance as f32 * scale)
        .sum();
    let fixed_count = parts.len() - n_ext;

    // Assembled width with each extender placed `r` times, reduced by the overlap
    // at every join: width(r) = fixed + r*ext - overlap*(placed_count - 1).
    let width_for = |r: usize| -> f32 {
        let placed = fixed_count + r * n_ext;
        let joins = placed.saturating_sub(1) as f32;
        fixed_advance + r as f32 * ext_advance - overlap * joins
    };

    let mut repeat = if n_ext == 0 { 0 } else { 1 };
    if n_ext > 0 {
        while width_for(repeat) < target_px && repeat < 256 {
            repeat += 1;
        }
    }

    // Lay the parts left→right; `cursor` is the x of the next part's left edge.
    let mut children: Vec<Child> = Vec::new();
    let mut height = 0.0f32;
    let mut depth = 0.0f32;
    let mut cursor = 0.0f32;
    let mut first = true;
    for part in &parts {
        let reps = if part.part_flags.extender() { repeat } else { 1 };
        let part_adv = part.full_advance as f32 * scale;
        for _ in 0..reps {
            if !first {
                cursor -= overlap; // overlap with the previously placed part
            }
            first = false;
            let gid = part.glyph_id;
            let advance = (face.glyph_hor_advance(gid).unwrap_or(0) as f32) * scale;
            let (gh, gd) = glyph_extents(face, gid, scale);
            height = height.max(gh);
            depth = depth.max(gd);
            children.push(Child {
                dx: cursor,
                dy: 0.0,
                b: Box {
                    width: advance,
                    height: gh,
                    depth: gd,
                    kind: BoxKind::Glyph { gid, scale, color },
                },
            });
            cursor += part_adv;
        }
    }

    Box {
        width: cursor.max(0.0),
        height,
        depth,
        kind: BoxKind::Hbox { children },
    }
}

/// A single glyph box with its **vertical midpoint on the baseline** (`height ==
/// depth == (glyph height+depth)/2`), so that axis-centering is a clean split.
///
/// The drawn glyph keeps its natural ink position, but we shift it down by its
/// own midpoint so the box's baseline lands at the glyph's vertical center.
fn centered_glyph(face: &Face<'static>, gid: GlyphId, scale: f32, color: [u8; 4]) -> Box {
    let (h, d) = glyph_extents(face, gid, scale);
    let advance = (face.glyph_hor_advance(gid).unwrap_or(0) as f32) * scale;
    let total = h + d;
    let half = total / 2.0;
    // Glyph ink currently reaches `h` up / `d` down from *its* baseline. We want
    // the box baseline at the ink midpoint: shift the glyph's baseline down by
    // `mid = (h - d)/2` so the new top is `half`, new bottom `half`.
    let mid = (h - d) / 2.0;
    Box {
        width: advance,
        height: half,
        depth: half,
        kind: BoxKind::Hbox {
            children: vec![Child {
                dx: 0.0,
                dy: mid,
                b: Box {
                    width: advance,
                    height: h,
                    depth: d,
                    kind: BoxKind::Glyph { gid, scale, color },
                },
            }],
        },
    }
}

/// Assemble a tall delimiter by stacking the assembly's parts vertically to reach
/// `target_px`, repeating any extender parts as needed (KaTeX `makeStackedDelim`,
/// simplified). Returns a box with its vertical midpoint on its baseline.
///
/// Parts are listed bottom→top in the font; each carries a `full_advance` (its
/// own height) and start/end connector lengths. Adjacent parts overlap by at
/// least `min_connector_overlap` (we use exactly that minimum overlap, which is
/// the common, robust choice). Extenders (`part_flags.extender()`) may be
/// repeated; non-extenders appear once.
fn assemble(
    face: &Face<'static>,
    variants: &ttf_parser::math::Variants<'static>,
    assembly: ttf_parser::math::GlyphAssembly<'static>,
    target_px: f32,
    scale: f32,
    color: [u8; 4],
) -> Box {
    let overlap = variants.min_connector_overlap as f32 * scale;

    // Split parts into the fixed (non-extender) and repeatable (extender) groups,
    // preserving order. We need the widest part for the box width.
    let parts: Vec<ttf_parser::math::GlyphPart> = assembly.parts.into_iter().collect();
    let n_ext = parts.iter().filter(|p| p.part_flags.extender()).count();
    let fixed_advance: f32 = parts
        .iter()
        .filter(|p| !p.part_flags.extender())
        .map(|p| p.full_advance as f32 * scale)
        .sum();
    let ext_advance: f32 = parts
        .iter()
        .filter(|p| p.part_flags.extender())
        .map(|p| p.full_advance as f32 * scale)
        .sum();

    // Number of joins if every extender appears `r` times: between every pair of
    // consecutive placed parts there is one overlap. With `r` repeats per
    // extender, the placed-part count is `(fixed count) + r*(ext count)`.
    let fixed_count = parts.len() - n_ext;

    // Solve for the smallest repeat count `r` (≥ 1 when extenders exist) so the
    // stacked, overlap-reduced height ≥ target.  height(r) =
    //   fixed_advance + r*ext_advance - overlap*(placed_count - 1).
    let height_for = |r: usize| -> f32 {
        let placed = fixed_count + r * n_ext;
        let joins = placed.saturating_sub(1) as f32;
        fixed_advance + r as f32 * ext_advance - overlap * joins
    };

    let mut repeat = if n_ext == 0 { 0 } else { 1 };
    if n_ext > 0 {
        // Grow until tall enough (bounded to avoid pathological loops).
        while height_for(repeat) < target_px && repeat < 256 {
            repeat += 1;
        }
    }

    // Build the stack bottom→top. Each part's glyph box has its own ink extent;
    // we lay them with `min_connector_overlap` between successive advances. We
    // position by the running "top" coordinate measured upward from the stack
    // bottom, then convert to baseline-relative `dy` at the end.
    let mut children: Vec<Child> = Vec::new();
    let mut width = 0.0f32;
    // `cursor` is the distance from the stack bottom to the bottom of the next
    // part to place (parts placed bottom→top).
    let mut cursor = 0.0f32;
    let mut first = true;
    for part in &parts {
        let reps = if part.part_flags.extender() { repeat } else { 1 };
        let part_adv = part.full_advance as f32 * scale;
        for _ in 0..reps {
            if !first {
                cursor -= overlap; // overlap with the previously placed part
            }
            first = false;
            let gid = part.glyph_id;
            let advance = (face.glyph_hor_advance(gid).unwrap_or(0) as f32) * scale;
            width = width.max(advance);
            let (gh, gd) = glyph_extents(face, gid, scale);
            // The glyph's *cell* spans `part_adv` from `cursor` (bottom) upward.
            // Place the glyph's baseline so its ink sits in that cell: its natural
            // baseline is `gd` above its ink bottom. We align the ink bottom to
            // `cursor`, so the glyph baseline is at `cursor + gd` from stack bottom.
            children.push(Child {
                dx: 0.0,
                // dy filled below once we know the stack midpoint (placeholder:
                // store the baseline height above the stack bottom, fixed up next).
                dy: cursor + gd,
                b: Box {
                    width: advance,
                    height: gh,
                    depth: gd,
                    kind: BoxKind::Glyph { gid, scale, color },
                },
            });
            cursor += part_adv;
        }
    }

    // `cursor` now equals the full stacked height (bottom→top).
    let total = cursor.max(1.0);
    let half = total / 2.0;
    // Each child's `dy` currently holds "baseline height above stack bottom".
    // Convert to a baseline-relative downward shift so the box midpoint sits on
    // the box baseline: a part whose baseline is `b` above the stack bottom is
    // `(b - half)` above the box midpoint, i.e. `dy = -(b - half) = half - b`.
    for c in &mut children {
        c.dy = half - c.dy;
    }

    Box {
        width,
        height: half,
        depth: half,
        kind: BoxKind::Hbox { children },
    }
}

/// Height (above baseline) and depth (below baseline) of a glyph in px, from its
/// outline bbox; font ascender/descender fallback for outline-less glyphs.
/// (Mirrors `box_layout::glyph_extents`, duplicated here to keep that one private.)
fn glyph_extents(face: &Face<'static>, gid: GlyphId, scale: f32) -> (f32, f32) {
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
            let asc = face.ascender() as f32 * scale;
            let desc = -(face.descender() as f32) * scale;
            (asc.max(0.0), desc.max(0.0))
        }
    }
}
