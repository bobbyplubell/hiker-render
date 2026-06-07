//! Accent layout (`\hat \tilde \bar \vec \overline \widehat \overbrace …` and the
//! under-forms) — split out of the layout engine as an [`Ctx`] impl-continuation.
//! Places a fixed or stretchy accent glyph (or an over/under rule) above or below
//! the base, attached at the base's MATH top-accent point, optically corrected.

use ttf_parser::GlyphId;

use super::{delim, glyph, glyph_extents, layout_list, Box, BoxKind, Child, Ctx, MathList, MathNode, Style};

impl Ctx<'_> {
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
    pub(crate) fn layout_accent(
        &self,
        accent: char,
        stretchy: bool,
        under: bool,
        base: &MathList,
        style: Style,
        _cramped: bool,
    ) -> Option<Box> {
        let ctx = self;
        let base_box = layout_list(ctx, base, style, /* cramped */ true)?;
        let base_w = base_box.width;
        let scale = ctx.scale_for(style);
        let c = ctx.face.tables().math.and_then(|m| m.constants);

        // Horizontal attachment ("skew"): a single-glyph base attaches at its MATH
        // top-accent-attachment point; otherwise center on the base width.
        let skew = ctx.base_skew(base, style).unwrap_or(base_w / 2.0);

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
            ctx.top_accent_attachment(gid, scale).unwrap_or(acc_w / 2.0)
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
    pub(crate) fn base_skew(&self, base: &MathList, style: Style) -> Option<f32> {
        let ctx = self;
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
        ctx.top_accent_attachment(gid, ctx.scale_for(style))
    }

    /// The MATH `top_accent_attachment` of glyph `gid` in px at `scale` (font-units →
    /// px), or `None` when the font supplies none for that glyph.
    pub(crate) fn top_accent_attachment(&self, gid: GlyphId, scale: f32) -> Option<f32> {
        let ctx = self;
        ctx.face
            .tables()
            .math
            .and_then(|m| m.glyph_info)
            .and_then(|gi| gi.top_accent_attachments)
            .and_then(|ta| ta.get(gid))
            .map(|v| v.value as f32 * scale)
    }
}
