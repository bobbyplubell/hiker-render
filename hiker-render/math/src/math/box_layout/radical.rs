//! Radical layout (`\sqrt{…}` / `\sqrt[n]{…}`) — Appendix G rule 11 / MathML Core
//! §3.3.3 / the OpenType MATH formulation, split out of the layout engine as an
//! [`Ctx`] impl-continuation. Sizes the surd to span the radicand, lays the
//! vinculum over it, and tucks an optional degree into the surd's upper-left.

use super::{delim, layout_list, Box, BoxKind, Child, Ctx, MathList, Style};

impl Ctx<'_> {
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
    pub(crate) fn layout_radical(
        &self,
        index: Option<&MathList>,
        radicand: &MathList,
        style: Style,
        _cramped: bool,
    ) -> Option<Box> {
        let ctx = self;
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
}
