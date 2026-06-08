//! Golden SVG snapshot corpus for the math engine.
//!
//! This is a behavior-preservation SAFETY NET captured before a refactor of
//! `box_layout.rs`: each `(name, style, latex)` case is rendered to SVG and
//! compared byte-for-byte against `tests/golden/<name>.svg`.
//!
//! The math engine's SVG output is deterministic (no random ids, timestamps, or
//! hashmap-ordered attributes — the emitter is a plain left-to-right tree walk),
//! so a committed golden can be diffed exactly across runs.
//!
//! ## Capturing / updating goldens
//!
//! Set `UPDATE_GOLDENS=1` to (re)write every golden instead of asserting:
//!
//! ```sh
//! UPDATE_GOLDENS=1 cargo test -p hiker-math golden
//! ```
//!
//! Then run `cargo test -p hiker-math` to confirm the committed goldens match.
//!
//! Zero-dependency: std + the crate only.

use std::path::PathBuf;

use hiker_math::{render_latex, MathOptions, MathStyle};

/// Which math style to render a case in.
#[derive(Clone, Copy)]
enum Style {
    Inline,
    Display,
}

impl Style {
    fn options(self) -> MathOptions {
        MathOptions {
            font_size_px: 48.0,
            color: [0, 0, 0, 255],
            style: match self {
                Style::Inline => MathStyle::Inline,
                Style::Display => MathStyle::Display,
            },
        }
    }
}

use Style::{Display as D, Inline as I};

/// The corpus: `(name, style, latex)`. `name` is the golden file stem.
///
/// Names are grouped/prefixed by feature so the golden directory reads as a
/// feature map. Multiple cases per construct intentionally exercise the surface.
#[rustfmt::skip]
const CASES: &[(&str, Style, &str)] = &[
    // ----- atoms / basic layout -----
    ("atom-var",            I, "x"),
    ("atom-sum-expr",       I, "a + b = c"),
    ("atom-emc2",           I, "E = mc^2"),
    ("atom-mixed-ops",      I, "a - b \\cdot c / d"),
    ("atom-relations",      I, "a < b \\le c \\ne d \\ge e > f"),

    // ----- fractions -----
    ("frac-half",           I, r"\frac{1}{2}"),
    ("frac-dtdp",           I, r"\frac{dT}{dP}"),
    ("frac-nested",         I, r"\frac{a + \frac{b}{c}}{d}"),
    ("frac-deep-nested",    D, r"\frac{\frac{a}{b}}{\frac{c}{d}}"),
    ("frac-display",        D, r"\frac{1}{2}"),
    ("frac-dfrac",          I, r"\dfrac{1}{2}"),
    ("frac-tfrac",          D, r"\tfrac{1}{2}"),
    ("frac-as-script",      I, r"x^{\frac{1}{2}}"),
    // NOTE: `a \over b` (`\over`) is unsupported by the current engine
    // (render_latex returns None) and is intentionally excluded from goldens.

    // ----- radicals -----
    ("sqrt-2",              I, r"\sqrt{2}"),
    ("sqrt-sum",            I, r"\sqrt{x + 1}"),
    ("sqrt-frac",           D, r"\sqrt{\frac{a}{b}}"),
    ("root-cbrt",           I, r"\sqrt[3]{x}"),
    ("root-nth",            I, r"\sqrt[n]{x + y}"),
    ("sqrt-nested",         D, r"\sqrt{1 + \sqrt{x}}"),

    // ----- super / subscripts -----
    ("script-sup",          I, "x^2"),
    ("script-sub",          I, "a_i"),
    ("script-both",         I, "x_i^2"),
    ("script-stacked-sup",  I, "x^{y^z}"),
    ("script-multilevel",   I, "a_{i_j}^{k^l}"),
    ("script-sub-text",     I, r"v_{\text{L}}"),
    ("script-epi",          I, r"e^{i\pi}"),
    ("script-prime",        I, "f'(x)"),

    // ----- big operators (limits vs nolimits) -----
    ("op-sum-display",      D, r"\sum_{i=1}^{n} i"),
    ("op-sum-inline",       I, r"\sum_{i=1}^{n} i"),
    ("op-int",              D, r"\int_0^1 x^2 \, dx"),
    ("op-int-inline",       I, r"\int_0^1 x^2 \, dx"),
    ("op-prod",             D, r"\prod_{k=1}^{n} k"),
    ("op-bigcup",           D, r"\bigcup_{i=1}^{n} A_i"),
    ("op-lim",              D, r"\lim_{x \to 0} f(x)"),
    ("op-sum-nolimits",     D, r"\sum\nolimits_{i=1}^{n} i"),

    // ----- delimiters -----
    ("delim-paren",         I, r"\left( x + 1 \right)"),
    ("delim-bigfrac",       I, r"\left( \frac{a}{b} \right)"),
    ("delim-brackets",      I, r"\left[ \frac{1}{2} \right]"),
    ("delim-braces",        I, r"\left\{ x \right\}"),
    ("delim-tall",          D, r"\left( \frac{\frac{a}{b}}{c} \right)"),
    ("delim-nested",        D, r"\left[ \left( a + b \right) c \right]"),
    ("delim-big",           I, r"\big( x \big)"),
    ("delim-Big",           I, r"\Big[ y \Big]"),
    ("delim-vert",          D, r"\left| \frac{a}{b} \right|"),

    // ----- accents -----
    ("accent-hat",          I, r"\hat{x}"),
    ("accent-bar",          I, r"\bar{y}"),
    ("accent-vec",          I, r"\vec{v}"),
    ("accent-tilde",        I, r"\tilde{n}"),
    ("accent-combo",        I, r"\hat{x} + \vec{v} + \bar{y} + \tilde{n}"),
    ("accent-widehat",      I, r"\widehat{xyz}"),
    ("accent-overline",     I, r"\overline{a + b}"),
    ("accent-wide-combo",   I, r"\overline{a + b} + \widehat{xyz}"),

    // ----- binom / braces over spans -----
    ("binom",               I, r"\binom{n}{k}"),
    ("binom-display",       D, r"\binom{n}{k}"),
    ("overbrace",           I, r"\overbrace{a + b + c}^{n}"),
    ("underbrace",          I, r"\underbrace{x + y}_{\text{sum}}"),

    // ----- matrices / cases -----
    ("mat-pmatrix",         D, r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}"),
    ("mat-bmatrix",         D, r"\begin{bmatrix} 1 & 0 \\ 0 & 1 \end{bmatrix}"),
    ("mat-vmatrix",         D, r"\begin{vmatrix} a & b \\ c & d \end{vmatrix}"),
    ("mat-cases",           D, r"f(x) = \begin{cases} x & x > 0 \\ -x & x \le 0 \end{cases}"),
    ("mat-bigentries",      D, r"\begin{pmatrix} \frac{1}{2} & 0 \\ 0 & \frac{1}{2} \end{pmatrix}"),

    // ----- spacing -----
    ("space-thin",          I, r"a \, b"),
    ("space-quad",          I, r"a \quad b"),
    ("space-mixed",         I, r"a \, b \quad c"),

    // ----- text mode -----
    ("text-mode",           I, r"x = \text{velocity}"),
    ("text-in-frac",        I, r"\frac{\text{rise}}{\text{run}}"),

    // ----- font commands -----
    ("font-bb",             I, r"\mathbb{R} \mathbb{Z} \mathbb{N}"),
    ("font-cal",            I, r"\mathcal{L}"),
    ("font-frak",           I, r"\mathfrak{g}"),
    ("font-bf",             I, r"\mathbf{x}"),
    ("font-mixed",          I, r"\mathcal{L}(\mathfrak{g})"),

    // ----- Greek + operators -----
    ("greek-lower",         I, r"\alpha \beta \gamma \delta \epsilon"),
    ("greek-upper",         I, r"\Gamma \Delta \Theta \Lambda \Omega"),
    ("greek-in-expr",       I, r"\theta = \frac{\pi}{2}"),

    // ----- realistic compound formulas -----
    ("real-quadratic",      D, r"x = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a}"),
    ("real-integral",       D, r"\int_{-\infty}^{\infty} e^{-x^2} \, dx = \sqrt{\pi}"),
    ("real-matrix-eq",      D, r"\begin{pmatrix} a & b \\ c & d \end{pmatrix} \begin{pmatrix} x \\ y \end{pmatrix}"),
    ("real-limit",          D, r"\lim_{n \to \infty} \left( 1 + \frac{1}{n} \right)^n = e"),
    ("real-sum-series",     D, r"\sum_{n=1}^{\infty} \frac{1}{n^2} = \frac{\pi^2}{6}"),
    ("real-clausius",       I, r"T\left(v_{\text{L}} - v_{\text{S}}\right)"),
];

/// Directory that holds the committed golden files.
fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("golden")
}

/// Render a case to SVG, or panic with a clear message on a render error.
fn render_case(name: &str, style: Style, latex: &str) -> String {
    render_latex(latex, &style.options())
        .unwrap_or_else(|e| panic!("case `{name}` failed ({e:?}) for input: {latex}"))
        .svg
}

/// All case names are unique (a duplicate would silently clobber a golden).
#[test]
fn case_names_are_unique() {
    let mut names: Vec<&str> = CASES.iter().map(|(n, _, _)| *n).collect();
    names.sort_unstable();
    for pair in names.windows(2) {
        assert_ne!(pair[0], pair[1], "duplicate case name: {}", pair[0]);
    }
}

/// Rendering the same input twice yields byte-identical SVG (determinism guard).
#[test]
fn rendering_is_deterministic() {
    for (name, style, latex) in CASES {
        let a = render_case(name, *style, latex);
        let b = render_case(name, *style, latex);
        assert_eq!(a, b, "non-deterministic render for case `{name}`");
    }
}

/// Compare each case's render against its committed golden, or — when
/// `UPDATE_GOLDENS=1` — (re)write the golden file.
#[test]
fn golden_svgs_match() {
    let update = std::env::var_os("UPDATE_GOLDENS").is_some();
    let dir = golden_dir();
    if update {
        std::fs::create_dir_all(&dir).expect("create golden dir");
    }

    let mut mismatches = Vec::new();
    for (name, style, latex) in CASES {
        let svg = render_case(name, *style, latex);
        let path = dir.join(format!("{name}.svg"));

        if update {
            std::fs::write(&path, &svg)
                .unwrap_or_else(|e| panic!("write golden {}: {e}", path.display()));
            continue;
        }

        match std::fs::read_to_string(&path) {
            Ok(expected) if expected == svg => {}
            Ok(_) => mismatches.push(format!("  mismatch: {name} ({})", path.display())),
            Err(e) => mismatches.push(format!(
                "  missing/unreadable golden for {name}: {e} (run with UPDATE_GOLDENS=1)"
            )),
        }
    }

    assert!(
        mismatches.is_empty(),
        "{} golden mismatch(es):\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}
