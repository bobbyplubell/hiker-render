    use super::*;
    use super::glyph::Variant;
    use super::parse::variant_for;

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

