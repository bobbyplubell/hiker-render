    use super::*;
    use super::glyph::Variant;

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
