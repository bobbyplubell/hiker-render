//! LaTeX math typesetting → SVG.
//!
//! Front-end: [`pulldown_latex`]'s pull parser (we consume its `Event` stream,
//! not its MathML writer). Back-end (ours): Appendix-G / MathML-Core box layout
//! over an OpenType MATH-table font, emitting an SVG document. References
//! (read-only): `references/microtex` (C++), `references/katex` (JS), the
//! TeXbook Appendix G, the OpenType MATH table spec, and MathML Core.
//!
//! Status: scaffolding. [`render_latex`] returns `None` until the layout engine
//! lands; callers fall back to whatever they show for "no render" (e.g. a
//! placeholder or the source text). Built up subset-first — identifiers,
//! numbers, operators, sub/superscripts, fractions, radicals, common symbols,
//! and basic delimiters — then display-math features (big operators, matrices,
//! large delimiters).
//!
//! This file is a thin orchestrator: [`box_layout`] turns the parser's event
//! stream into a box tree, and [`svg`] emits a self-contained SVG document from it.

mod box_layout;
mod color;
mod delim;
mod glyph;
mod macros;
mod svg;

/// Math layout style: inline (`\textstyle`) vs. display (`\displaystyle`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MathStyle {
    /// Inline with surrounding text (smaller, limits beside operators).
    #[default]
    Inline,
    /// Displayed block (larger, limits above/below operators).
    Display,
}

/// Inputs for a single render.
#[derive(Clone, Debug)]
pub struct MathOptions {
    /// Base font size in CSS px (the `\normalsize` em).
    pub font_size_px: f32,
    /// Glyph color as straight (un-premultiplied) RGBA.
    pub color: [u8; 4],
    /// Inline vs. display style.
    pub style: MathStyle,
}

impl Default for MathOptions {
    fn default() -> Self {
        MathOptions {
            font_size_px: 16.0,
            color: [0, 0, 0, 255],
            style: MathStyle::Inline,
        }
    }
}

/// A rendered equation: an SVG document plus the metrics a host needs to place
/// it inline (so it can vertically align the math axis with surrounding text).
#[derive(Clone, Debug)]
pub struct MathRender {
    /// A complete, self-contained SVG document (glyph outlines as paths; no
    /// external font needed to rasterize).
    pub svg: String,
    /// Rendered size in CSS px.
    pub width_px: f32,
    pub height_px: f32,
    /// Distance from the top of the SVG box down to the baseline, in px, so the
    /// caller can align the equation's baseline with text.
    pub baseline_px: f32,
}

/// Render a LaTeX math string to SVG. Returns `None` if the input cannot be
/// parsed/laid out (or while the engine is still scaffolding).
///
/// The input is math-mode LaTeX *without* surrounding `$`/`\[` delimiters
/// (e.g. `\frac{dT}{dP}` or `v_{\text{L}}`); Wikipedia's `\displaystyle …`
/// prefix and bare delimiters are tolerated by the parser.
pub fn render_latex(src: &str, opts: &MathOptions) -> Option<MathRender> {
    render_latex_with_preamble(src, "", opts)
}

/// Render a LaTeX math string with a reusable `preamble` of macro definitions
/// prepended (so a host can register global macros once and reuse them across
/// many renders). `preamble` is any string of `\def`/`\newcommand`/`\renewcommand`/
/// `\providecommand`/`\DeclareMathOperator` definitions; it is prepended to `src`
/// (separated by a space) and the combined string goes through the same
/// definition-expansion + layout path as [`render_latex`].
///
/// Returns `None` if the (combined) input cannot be parsed/laid out, or if `src`
/// is empty. See [`render_latex`] for the input conventions.
pub fn render_latex_with_preamble(
    src: &str,
    preamble: &str,
    opts: &MathOptions,
) -> Option<MathRender> {
    // Translate LaTeX-flavored macro definitions (in both preamble and source)
    // into the `\def` form pulldown-latex understands, then parse the in-scope
    // atoms, lay them out left-to-right into one Hbox (with math-italic variables,
    // upright `\text`, and the Appendix-G inter-atom spacing matrix), and emit a
    // self-contained SVG. Returns None on parse failure / empty input.
    if src.trim().is_empty() {
        return None;
    }
    let combined = if preamble.is_empty() {
        macros::expand_definitions(src)
    } else {
        macros::expand_definitions(&format!("{preamble} {src}"))
    };
    // Pull out `\arraystretch` (a macro pulldown can't surface as a value) and
    // strip its definition; the factor scales matrix/array inter-row spacing.
    let (combined, arraystretch) = macros::extract_arraystretch(&combined);
    // Normalize `\color`/`\textcolor`/… arguments to canonical `#rrggbb` (named
    // colors + hex resolved by our table; unknown colors fall back to the default
    // text color) so an unrecognized color name can't fail the whole parse.
    let combined = color::normalize_color_args(&combined, opts.color);
    let (root, face) = box_layout::layout(&combined, opts, arraystretch.unwrap_or(1.0))?;
    let (svg, width_px, height_px, baseline_px) = svg::emit(&root, &face, opts);
    Some(MathRender {
        svg,
        width_px,
        height_px,
        baseline_px,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Counts `<path` occurrences (one per emitted glyph).
    fn count_paths(svg: &str) -> usize {
        svg.matches("<path").count()
    }

    #[test]
    fn renders_a_simple_formula() {
        let out = render_latex("a+b=c", &MathOptions::default()).expect("renders Some");
        assert!(count_paths(&out.svg) >= 3, "expected >=3 glyph paths");
        assert!(out.width_px > 0.0, "positive width");
        assert!(out.height_px > 0.0, "positive height");
        assert!(
            out.baseline_px > 0.0 && out.baseline_px <= out.height_px,
            "baseline within box: {} of {}",
            out.baseline_px,
            out.height_px
        );
        assert!(out.svg.starts_with("<svg"), "is an svg document");
    }

    #[test]
    fn single_glyph_width_matches_advance() {
        let opts = MathOptions::default();
        let out = render_latex("x", &opts).expect("renders Some");
        assert_eq!(count_paths(&out.svg), 1, "exactly one glyph path");

        // A lone `x` is a math variable, so it renders in the math-italic variant
        // (U+1D465); expect *that* glyph's advance, not the upright one.
        let face = crate::font::math_face();
        let gid = face.glyph_index('\u{1D465}').unwrap();
        let scale = opts.font_size_px / 1000.0;
        let expected = face.glyph_hor_advance(gid).unwrap() as f32 * scale;
        assert!(
            (out.width_px - expected).abs() < 0.01,
            "width {} ≈ advance {}",
            out.width_px,
            expected
        );
    }

    #[test]
    fn empty_input_is_none() {
        assert!(render_latex("", &MathOptions::default()).is_none());
        assert!(render_latex("   ", &MathOptions::default()).is_none());
    }

    #[test]
    fn options_default_is_sane() {
        let o = MathOptions::default();
        assert_eq!(o.font_size_px, 16.0);
        assert_eq!(o.style, MathStyle::Inline);
    }

    /// `x^2` raises a smaller `2` beside the `x`: the render is wider than a lone
    /// `x` and taller than a lone `x` (the script reaches above the base top).
    #[test]
    fn superscript_render_metrics() {
        let opts = MathOptions::default();
        let sup = render_latex("x^2", &opts).expect("renders Some");
        let base = render_latex("x", &opts).expect("renders Some");
        assert_eq!(count_paths(&sup.svg), 2, "x and 2");
        assert!(sup.width_px > base.width_px, "wider with a script");
        assert!(sup.height_px > base.height_px, "taller (script above base)");
        assert!(sup.baseline_px > 0.0 && sup.baseline_px <= sup.height_px);
    }

    /// `a_i` lowers a smaller `i`: the render gains depth below the baseline, so
    /// the baseline is above the box bottom by more than for a lone `a`.
    #[test]
    fn subscript_render_metrics() {
        let opts = MathOptions::default();
        let sub = render_latex("a_i", &opts).expect("renders Some");
        let base = render_latex("a", &opts).expect("renders Some");
        assert_eq!(count_paths(&sub.svg), 2, "a and i");
        // Depth = height below baseline = height_px - baseline_px.
        let sub_depth = sub.height_px - sub.baseline_px;
        let base_depth = base.height_px - base.baseline_px;
        assert!(sub_depth > base_depth, "subscript adds depth");
    }

    /// `E = mc^2` lays out without panicking and yields sane metrics with the
    /// expected number of glyph paths (E = m c 2).
    #[test]
    fn display_formula_metrics() {
        let opts = MathOptions::default();
        let out = render_latex("E = mc^2", &opts).expect("renders Some");
        assert_eq!(count_paths(&out.svg), 5, "E, =, m, c, 2");
        assert!(out.width_px > 0.0 && out.height_px > 0.0);
        assert!(out.baseline_px > 0.0 && out.baseline_px <= out.height_px);
    }

    /// `\frac{1}{2}` renders a fraction bar (`<rect>`) and is both wider and taller
    /// than a lone digit, with sane baseline metrics.
    #[test]
    fn fraction_render_metrics() {
        let opts = MathOptions::default();
        let out = render_latex(r"\frac{1}{2}", &opts).expect("renders Some");
        assert_eq!(count_paths(&out.svg), 2, "numerator and denominator glyphs");
        assert_eq!(out.svg.matches("<rect").count(), 1, "one fraction bar");
        let digit = render_latex("2", &opts).expect("renders Some");
        assert!(out.height_px > digit.height_px, "taller than a digit");
        assert!(out.baseline_px > 0.0 && out.baseline_px <= out.height_px);
    }

    /// A Display-style `\frac{1}{2}` is taller than the same fraction inline.
    #[test]
    fn display_fraction_is_taller() {
        let inline = render_latex(r"\frac{1}{2}", &MathOptions::default()).expect("renders");
        let display = render_latex(
            r"\frac{1}{2}",
            &MathOptions {
                style: MathStyle::Display,
                ..MathOptions::default()
            },
        )
        .expect("renders");
        assert!(
            display.height_px > inline.height_px,
            "display ({}) taller than inline ({})",
            display.height_px,
            inline.height_px
        );
    }

    /// `\left( x \right)`: renders the `x` plus two delimiter glyphs, taller and
    /// wider than a bare `x`, with sane baseline metrics.
    #[test]
    fn left_right_render_metrics() {
        let opts = MathOptions::default();
        let fence = render_latex(r"\left( x \right)", &opts).expect("renders");
        let bare = render_latex("x", &opts).expect("renders");
        // x + open paren + close paren = at least 3 glyph paths.
        assert!(count_paths(&fence.svg) >= 3, "x and two parens");
        assert!(fence.width_px > bare.width_px, "wider than bare x");
        assert!(fence.height_px > bare.height_px, "taller than bare x");
        assert!(fence.baseline_px > 0.0 && fence.baseline_px <= fence.height_px);
    }

    /// The parens around `\frac{a}{b}` are taller (bigger render box) than around
    /// a bare `x` — the delimiter scales to the content.
    #[test]
    fn left_right_grows_with_content() {
        let opts = MathOptions::default();
        let small = render_latex(r"\left( x \right)", &opts).expect("renders");
        let big = render_latex(r"\left( \frac{a}{b} \right)", &opts).expect("renders");
        assert!(
            big.height_px > small.height_px,
            "fence around frac ({}) taller than around x ({})",
            big.height_px,
            small.height_px
        );
    }

    /// `\newcommand{\R}{\mathbb{R}}\R` (no args) renders after expansion.
    #[test]
    fn newcommand_no_args_renders() {
        let out = render_latex(r"\newcommand{\R}{\mathbb{R}}\R", &MathOptions::default());
        assert!(out.is_some(), "expanded \\newcommand should render Some");
    }

    /// `\newcommand{\v}[1]{\vec{#1}}\v{x}` renders, and matches the equivalent
    /// hand-written `\def` form in width to within a small epsilon.
    #[test]
    fn newcommand_one_arg_matches_def() {
        let opts = MathOptions::default();
        let via_new = render_latex(r"\newcommand{\v}[1]{\vec{#1}}\v{x}", &opts)
            .expect("newcommand renders");
        let via_def = render_latex(r"\def\v#1{\vec{#1}}\v{x}", &opts).expect("def renders");
        assert!(
            (via_new.width_px - via_def.width_px).abs() < 0.01,
            "newcommand width {} ≈ def width {}",
            via_new.width_px,
            via_def.width_px
        );
    }

    /// `\renewcommand` and `\providecommand` behave like `\newcommand`.
    #[test]
    fn renew_and_provide_render() {
        let opts = MathOptions::default();
        assert!(render_latex(r"\renewcommand{\R}{\mathbb{R}}\R", &opts).is_some());
        assert!(render_latex(r"\providecommand{\R}{\mathbb{R}}\R", &opts).is_some());
    }

    /// `\DeclareMathOperator{\lcm}{lcm}\lcm(a,b)` renders via `\operatorname`.
    #[test]
    fn declare_math_operator_renders() {
        let out = render_latex(r"\DeclareMathOperator{\lcm}{lcm}\lcm(a,b)", &MathOptions::default());
        assert!(out.is_some(), "\\DeclareMathOperator should render Some");
    }

    /// A reusable preamble of definitions is applied to the source.
    #[test]
    fn preamble_macros_apply() {
        let opts = MathOptions::default();
        let out = render_latex_with_preamble(r"\R^n", r"\newcommand{\R}{\mathbb{R}}", &opts);
        assert!(out.is_some(), "preamble macro should render Some");
    }

    /// A `\newcommand` with an optional-argument default is skipped gracefully
    /// (no panic). Such a definition is left verbatim, so it does not render, but
    /// the call must not crash.
    #[test]
    fn optional_arg_default_does_not_panic() {
        let opts = MathOptions::default();
        let _ = render_latex(r"\newcommand{\inc}[1][0]{#1+1}\inc{x}", &opts);
        // No assertion on Some/None — only that we returned without panicking.
    }

    /// `\textcolor{red}{x} + y`: the `x` glyph is red, while `+` and `y` keep the
    /// default (black) color.
    #[test]
    fn textcolor_colors_only_its_argument() {
        let out = render_latex(r"\textcolor{red}{x} + y", &MathOptions::default())
            .expect("renders Some");
        assert!(
            out.svg.contains(r#"fill="rgb(255,0,0)""#),
            "expected a red glyph, got: {}",
            out.svg
        );
        assert!(
            out.svg.contains(r#"fill="rgb(0,0,0)""#),
            "expected default-black glyphs for + and y"
        );
        // Exactly one red glyph (the x); + and y are black.
        assert_eq!(out.svg.matches(r#"fill="rgb(255,0,0)""#).count(), 1, "only x is red");
    }

    /// `{\color{blue} a b} c`: the scope form colors `a` and `b` blue but leaves
    /// `c` in the default color.
    #[test]
    fn color_scope_form_colors_rest_of_group() {
        let out =
            render_latex(r"{\color{blue} a b} c", &MathOptions::default()).expect("renders Some");
        assert_eq!(
            out.svg.matches(r#"fill="rgb(0,0,255)""#).count(),
            2,
            "a and b are blue: {}",
            out.svg
        );
        assert_eq!(out.svg.matches(r#"fill="rgb(0,0,0)""#).count(), 1, "c stays black");
    }

    /// `\textcolor{#00ff00}{z}` resolves the hex literal to green.
    #[test]
    fn textcolor_hex_resolves_to_green() {
        let out =
            render_latex(r"\textcolor{#00ff00}{z}", &MathOptions::default()).expect("renders Some");
        assert!(
            out.svg.contains(r#"fill="rgb(0,255,0)""#),
            "expected green glyph, got: {}",
            out.svg
        );
    }

    /// An unknown color name falls back to the default color without panicking,
    /// and the rest of the equation still renders.
    #[test]
    fn unknown_color_falls_back_to_default() {
        let out = render_latex(r"\textcolor{notacolor}{x} + y", &MathOptions::default())
            .expect("renders Some despite unknown color");
        assert_eq!(count_paths(&out.svg), 3, "x, +, y all render");
        assert!(
            !out.svg.contains(r#"fill="rgb(255,0,0)""#),
            "unknown color must not become red"
        );
        // Everything in the default black color.
        assert_eq!(out.svg.matches(r#"fill="rgb(0,0,0)""#).count(), 3, "all default-colored");
    }

    /// A colored fraction (`\textcolor{red}{\frac{1}{2}}`) paints its bar (`<rect>`)
    /// in the scope color, not just the digits.
    #[test]
    fn colored_fraction_bar_is_colored() {
        let out = render_latex(r"\textcolor{red}{\frac{1}{2}}", &MathOptions::default())
            .expect("renders Some");
        assert!(
            out.svg.contains(r#"<rect"#) && out.svg.contains(r#"rgb(255,0,0)"#),
            "fraction bar should be red: {}",
            out.svg
        );
    }

    /// A non-opaque default color is honored via `fill-opacity` on uncolored glyphs.
    #[test]
    fn alpha_default_emits_fill_opacity() {
        let opts = MathOptions {
            color: [0, 0, 0, 128],
            ..MathOptions::default()
        };
        let out = render_latex("x", &opts).expect("renders Some");
        assert!(out.svg.contains("fill-opacity"), "alpha < 255 emits fill-opacity");
    }

    /// Writes sample SVGs to `target/` so a human can eyeball them.
    #[test]
    fn writes_sample_svgs_for_eyeballing() {
        let inline = MathOptions::default();
        let display = MathOptions {
            style: MathStyle::Display,
            ..MathOptions::default()
        };
        for (name, src, opts) in [
            ("layer3-sup", "x^2", &inline),
            ("layer3-sub", r"v_{\text{L}}", &inline),
            ("layer3-both", "x_i^2", &inline),
            ("layer3-emc2", "E = mc^2", &inline),
            ("layer3-epi", r"e^{i\pi}", &inline),
            ("layer4-half", r"\frac{1}{2}", &inline),
            ("layer4-dtdp", r"\frac{dT}{dP}", &inline),
            ("layer4-nested", r"\frac{a + \frac{b}{c}}{d}", &inline),
            ("layer4-display", r"\frac{1}{2}", &display),
            ("layer4-frac-script", r"x^{\frac{1}{2}}", &inline),
            ("layer5-paren", r"\left( x + 1 \right)", &inline),
            ("layer5-bigfrac", r"\left( \frac{a}{b} \right)", &inline),
            ("layer5-brackets", r"\left[ \frac{1}{2} \right]", &inline),
            ("layer5-clausius-num", r"T\left(v_{\text{L}} - v_{\text{S}}\right)", &inline),
            ("layer5-tall", r"\left( \frac{\frac{a}{b}}{c} \right)", &inline),
            ("layer6-sqrt2", r"\sqrt{2}", &inline),
            ("layer6-sqrtsum", r"\sqrt{x+1}", &inline),
            ("layer6-cbrt", r"\sqrt[3]{x}", &inline),
            ("layer6-sqrtfrac", r"\sqrt{\frac{a}{b}}", &display),
            ("layer6-quad", r"\frac{-b + \sqrt{b^2 - 4ac}}{2a}", &display),
            ("layer7-sum-display", r"\sum_{i=1}^{n} i", &display),
            ("layer7-sum-inline", r"\sum_{i=1}^{n} i", &inline),
            ("layer7-int", r"\int_0^1 x^2 dx", &display),
            ("layer7-prod", r"\prod_{k=1}^{n}", &display),
            ("layer7-lim", r"\lim_{x \to 0} f(x)", &display),
            ("layer8-blackboard", r"\mathbb{R} \mathbb{Z} \mathbb{N}", &inline),
            ("layer8-bold", r"\mathbf{x} + \boldsymbol{\alpha}", &inline),
            ("layer8-cal", r"\mathcal{L}(\mathfrak{g})", &inline),
            ("layer8-accents", r"\hat{x} + \vec{v} + \bar{y} + \tilde{n}", &inline),
            ("layer8-wide", r"\overline{a+b} + \widehat{xyz}", &inline),
            ("layer9-pmatrix", r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}", &display),
            ("layer9-bmatrix", r"\begin{bmatrix} 1 & 0 \\ 0 & 1 \end{bmatrix}", &display),
            (
                "layer9-cases",
                r"f(x) = \begin{cases} x & x > 0 \\ -x & x \le 0 \end{cases}",
                &display,
            ),
            (
                "layer9-aligned",
                r"\begin{aligned} a &= b + c \\ x &= y \end{aligned}",
                &display,
            ),
            (
                "layer9-bigmatrix",
                r"\begin{pmatrix} \frac{1}{2} & 0 \\ 0 & \frac{1}{2} \end{pmatrix}",
                &display,
            ),
            ("color-textcolor", r"\textcolor{red}{E} = \textcolor{blue}{mc^2}", &inline),
            ("color-scope", r"{\color{green} x + y} + z", &inline),
            ("binom", r"\binom{n}{k}", &inline),
            ("cancel", r"\cancel{x} + \cancel{5y}", &inline),
            ("substack", r"\sum_{\substack{0<i<n \\ i\ne k}} a_i", &display),
            ("overbrace", r"\overbrace{a+b+c}^{n}", &inline),
            ("underbrace", r"\underbrace{x+y}_{\text{sum}}", &inline),
            ("xrightarrow", r"A \xrightarrow{f} B", &inline),
            ("xarrow-both", r"\xrightarrow[g]{f}", &inline),
        ] {
            if let Some(out) = render_latex(src, opts) {
                let path = format!(
                    "{}/../target/math-{name}.svg",
                    env!("CARGO_MANIFEST_DIR")
                );
                let _ = std::fs::write(&path, &out.svg);
                eprintln!("[math] wrote {path} ({} bytes)", out.svg.len());
            }
        }

        // Layer-9 column/row-rule + substack-size samples, written to the
        // workspace `target/` with the exact names the task asks for.
        for (name, src, opts) in [
            ("math-array-vrule", r"\begin{array}{c|c} a & b \\ c & d \end{array}", &display),
            ("math-array-hline", r"\begin{array}{cc} a & b \\ \hline c & d \end{array}", &display),
            ("math-substack-size", r"\sum_{\substack{0<i<n \\ i\ne k}} a_i", &display),
            // Custom-bar `\genfrac` (a 2pt vinculum), `\colorbox`/`\fcolorbox`
            // backgrounds + frame, and optically-attached accents over slanted
            // bases — the polish-pass deliverables.
            // pulldown 0.7 needs bare-token genfrac delimiters (`[]`), not braced.
            ("math-genfrac", r"\genfrac[]{2pt}{}{a}{b}", &display),
            (
                "math-colorbox",
                r"\colorbox{yellow}{x+1} + \fcolorbox{red}{lightgray}{y}",
                &inline,
            ),
            ("math-accent-slant", r"\hat{f} + \vec{A}", &inline),
        ] {
            if let Some(out) = render_latex(src, opts) {
                let path = format!("/home/bobby/projects/html-widget/target/{name}.svg");
                let _ = std::fs::write(&path, &out.svg);
                eprintln!("[math] wrote {path} ({} bytes)", out.svg.len());
            }
        }

        // A `\newcommand`-defined macro, rendered through the normal path.
        if let Some(out) = render_latex(
            r"\newcommand{\v}[1]{\vec{#1}}\v{F} = m\v{a}",
            &inline,
        ) {
            let path = format!("{}/../target/math-macros.svg", env!("CARGO_MANIFEST_DIR"));
            let _ = std::fs::write(&path, &out.svg);
            eprintln!("[math] wrote {path} ({} bytes)", out.svg.len());
        }
    }
}
