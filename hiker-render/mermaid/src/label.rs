//! Rich label rendering: markdown (`**bold**`, `*italic*`, `[text](url)` links,
//! `` `code` `` inline code, `<br>`/`\n` line breaks) plus inline LaTeX math
//! (`$…$`) rendered via the pure-Rust math engine ([`hiker_math`]).
//!
//! A label is parsed into **lines** (split on `<br>`/`<br/>`/`<br />` and `\n`),
//! each line a sequence of **runs**:
//! - [`Run::Text`] — a span of plain text with `bold`/`italic` flags from a small
//!   markdown subset (`**x**`/`__x__` → bold, `*x*`/`_x_` → italic; single level,
//!   non-nested). The markers are stripped.
//! - [`Run::Math`] — LaTeX between unescaped single `$ … $` (a `\$` is a literal
//!   dollar). Rendered to an embedded SVG fragment.
//! - [`Run::Link`] — a markdown link `[text](url)`. The visible `text` is rendered
//!   in the themed link color and underlined; the `url` is parsed but, since a
//!   static SVG has no navigation, it is only used to wrap the text in an
//!   `<a xlink:href>` (harmless, optionally honored by viewers). Width is measured
//!   from the visible `text` only.
//! - [`Run::Code`] — inline code `` `code` `` rendered in a monospace family with a
//!   faint rounded background box behind it.
//!
//! Layout is one baseline per line, runs laid left→right; a text run advances by
//! its real font width ([`crate::font::line_width`]) and a math run by its
//! rendered `width_px`, with the math **baseline** aligned to the text baseline.
//! Lines stack at `1.2em`. Plain labels (no markdown/math markers) measure and
//! emit *identically* to the previous plain-`<text>` path, so non-rich labels are
//! visually unchanged.

use std::fmt::Write as _;

use hiker_math::{render_latex, MathOptions, MathStyle};

use crate::font;
use crate::svgutil::{escape, rgb};

/// Line height as a fraction of the font size (matches the plain-text path).
const LINE_HEIGHT_EM: f32 = 1.2;

/// Horizontal anchoring of the whole label block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    /// Left edge of each line sits at `cx`.
    Start,
    /// Each line is centered horizontally on `cx`.
    Middle,
}

/// One inline run within a line.
#[derive(Clone, Debug, PartialEq)]
enum Run {
    /// Plain text with markdown emphasis flags; markers already stripped.
    Text { s: String, bold: bool, italic: bool },
    /// Inline LaTeX (the content between `$ … $`, delimiters stripped).
    Math { latex: String },
    /// Markdown link `[text](url)`: visible `text` plus its `url`.
    Link { text: String, url: String },
    /// Inline code `` `code` `` (the content between backticks, delimiters
    /// stripped).
    Code { s: String },
}

/// Themed link color: a blue, used for `[text](url)` regardless of the base text
/// color.
const LINK_COLOR: [u8; 4] = [0, 90, 200, 255];

/// Faint background fill behind inline code (a light grey).
const CODE_BG: [u8; 4] = [235, 235, 235, 255];

/// Whether `label` contains any rich markers (markdown emphasis, math, or an
/// explicit `<br>`). A `\n` alone is already handled by the plain-text path
/// (which splits on `\n`), so it does *not* count as rich — that keeps simple
/// multi-line labels on the identical code path they used before.
fn is_rich(label: &str) -> bool {
    label.contains('*')
        || label.contains('_')
        || label.contains('$')
        || label.contains('[')
        || label.contains('`')
        || label.contains("<br")
}

/// Split a label into line strings on `<br>`/`<br/>`/`<br />` and `\n`.
fn split_lines(label: &str) -> Vec<String> {
    // Normalize the `<br>` variants to `\n`, then split on `\n`.
    let mut s = label.to_string();
    for br in ["<br/>", "<br />", "<br>", "<BR/>", "<BR />", "<BR>"] {
        s = s.replace(br, "\n");
    }
    s.split('\n').map(|l| l.to_string()).collect()
}

/// Parse one line string into runs: pull out `$…$` math spans, then apply the
/// markdown emphasis subset to the text between them.
fn parse_line(line: &str) -> Vec<Run> {
    let mut runs = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    let mut text = String::new();

    let flush_text = |text: &mut String, runs: &mut Vec<Run>| {
        if !text.is_empty() {
            runs.extend(parse_emphasis(text));
            text.clear();
        }
    };

    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() && chars[i + 1] == '$' {
            // Escaped dollar → literal `$` in text.
            text.push('$');
            i += 2;
            continue;
        }
        if c == '$' {
            // Find the closing unescaped `$`.
            let mut j = i + 1;
            let mut latex = String::new();
            let mut closed = false;
            while j < chars.len() {
                if chars[j] == '\\' && j + 1 < chars.len() && chars[j + 1] == '$' {
                    latex.push('$');
                    j += 2;
                    continue;
                }
                if chars[j] == '$' {
                    closed = true;
                    break;
                }
                latex.push(chars[j]);
                j += 1;
            }
            if closed && !latex.trim().is_empty() {
                flush_text(&mut text, &mut runs);
                runs.push(Run::Math { latex });
                i = j + 1;
                continue;
            }
            // Unterminated or empty `$…$` → treat the `$` as literal text.
            text.push('$');
            i += 1;
            continue;
        }
        if c == '`' {
            // Inline code: find the closing backtick.
            let mut j = i + 1;
            let mut code = String::new();
            let mut closed = false;
            while j < chars.len() {
                if chars[j] == '`' {
                    closed = true;
                    break;
                }
                code.push(chars[j]);
                j += 1;
            }
            if closed && !code.is_empty() {
                flush_text(&mut text, &mut runs);
                runs.push(Run::Code { s: code });
                i = j + 1;
                continue;
            }
            // Unterminated or empty `` `…` `` → literal backtick.
            text.push('`');
            i += 1;
            continue;
        }
        if c == '[' {
            // Markdown link `[text](url)`. Find `](`, then the closing `)`.
            if let Some((link_text, url, next)) = parse_link(&chars, i) {
                flush_text(&mut text, &mut runs);
                runs.push(Run::Link { text: link_text, url });
                i = next;
                continue;
            }
            // Not a well-formed link → literal `[`.
            text.push('[');
            i += 1;
            continue;
        }
        text.push(c);
        i += 1;
    }
    flush_text(&mut text, &mut runs);
    runs
}

/// Try to parse a markdown link `[text](url)` starting at `start` (the `[`).
/// Returns `(text, url, index_past_close_paren)` on success.
fn parse_link(chars: &[char], start: usize) -> Option<(String, String, usize)> {
    // Scan the visible text up to the first unescaped `]`.
    let mut j = start + 1;
    let mut text = String::new();
    let mut closed_bracket = None;
    while j < chars.len() {
        if chars[j] == ']' {
            closed_bracket = Some(j);
            break;
        }
        if chars[j] == '[' {
            // No nested brackets in the link text.
            return None;
        }
        text.push(chars[j]);
        j += 1;
    }
    let rb = closed_bracket?;
    // Require `(` immediately after `]`.
    if rb + 1 >= chars.len() || chars[rb + 1] != '(' {
        return None;
    }
    // Scan the url up to the closing `)`.
    let mut k = rb + 2;
    let mut url = String::new();
    let mut closed_paren = None;
    while k < chars.len() {
        if chars[k] == ')' {
            closed_paren = Some(k);
            break;
        }
        url.push(chars[k]);
        k += 1;
    }
    let rp = closed_paren?;
    if text.is_empty() {
        return None;
    }
    Some((text, url, rp + 1))
}

/// Apply the markdown emphasis subset to a plain-text span. `**x**`/`__x__` →
/// bold, `*x*`/`_x_` → italic (single level, non-nested). Markers are stripped.
/// Returns a sequence of text runs (different emphasis = different run).
fn parse_emphasis(text: &str) -> Vec<Run> {
    let chars: Vec<char> = text.chars().collect();
    let mut runs = Vec::new();
    let mut buf = String::new();
    let mut i = 0;

    let push_run = |buf: &mut String, runs: &mut Vec<Run>, bold: bool, italic: bool| {
        if !buf.is_empty() {
            runs.push(Run::Text { s: std::mem::take(buf), bold, italic });
        }
    };

    while i < chars.len() {
        let c = chars[i];
        // Double marker → bold.
        if (c == '*' || c == '_') && i + 1 < chars.len() && chars[i + 1] == c {
            if let Some(end) = find_close(&chars, i + 2, c, true) {
                push_run(&mut buf, &mut runs, false, false);
                let inner: String = chars[i + 2..end].iter().collect();
                runs.push(Run::Text { s: inner, bold: true, italic: false });
                i = end + 2;
                continue;
            }
        }
        // Single marker → italic.
        if c == '*' || c == '_' {
            if let Some(end) = find_close(&chars, i + 1, c, false) {
                push_run(&mut buf, &mut runs, false, false);
                let inner: String = chars[i + 1..end].iter().collect();
                runs.push(Run::Text { s: inner, bold: false, italic: true });
                i = end + 1;
                continue;
            }
        }
        buf.push(c);
        i += 1;
    }
    push_run(&mut buf, &mut runs, false, false);
    if runs.is_empty() {
        // Preserve an all-marker / empty span as plain (possibly empty) text so a
        // line never collapses to nothing unexpectedly.
        runs.push(Run::Text { s: text.to_string(), bold: false, italic: false });
    }
    runs
}

/// Find the closing emphasis marker `c` starting at `from`. For `double`, the
/// close is two consecutive `c`; otherwise a single `c`. The inner span must be
/// non-empty. Returns the index of the first closing marker char, or `None`.
fn find_close(chars: &[char], from: usize, c: char, double: bool) -> Option<usize> {
    let mut j = from;
    while j < chars.len() {
        if chars[j] == c {
            if double {
                if j + 1 < chars.len() && chars[j + 1] == c {
                    return if j > from { Some(j) } else { None };
                }
            } else {
                return if j > from { Some(j) } else { None };
            }
        }
        j += 1;
    }
    None
}

/// The rendered width of a math run at `font_size`, or `None` if it failed to
/// render (caller falls back to drawing the raw `$latex$` as text).
fn math_render(latex: &str, font_size: f32, color: [u8; 4]) -> Option<hiker_math::MathRender> {
    render_latex(
        latex,
        &MathOptions { font_size_px: font_size, color, style: MathStyle::Inline },
    )
}

/// Width of a single run at `font_size`. Math width falls back to the raw
/// `$latex$` text width when the engine can't render it.
fn run_width(run: &Run, font_size: f32) -> f32 {
    match run {
        Run::Text { s, .. } => font::line_width(s, font_size),
        // Measure only the visible link text (real font metrics), not the url.
        Run::Link { text, .. } => font::line_width(text, font_size),
        // Approximate code width with the existing sans metric.
        Run::Code { s } => font::line_width(s, font_size),
        Run::Math { latex } => match math_render(latex, font_size, [0, 0, 0, 255]) {
            Some(m) => m.width_px,
            None => font::line_width(&format!("${latex}$"), font_size),
        },
    }
}

/// Intrinsic `(width, height)` px of a rich label at `font_size`.
///
/// For a plain label (no markers) this returns exactly [`font::text_size`], so
/// node sizing of non-rich labels is unchanged.
pub fn measure(label: &str, font_size: f32) -> (f32, f32) {
    if !is_rich(label) {
        return font::text_size(label, font_size);
    }
    let lines = split_lines(label);
    let line_h = font_size * LINE_HEIGHT_EM;
    let mut max_w = 0.0f32;
    for line in &lines {
        let runs = parse_line(line);
        let w: f32 = runs.iter().map(|r| run_width(r, font_size)).sum();
        max_w = max_w.max(w);
    }
    (max_w, lines.len().max(1) as f32 * line_h)
}

/// Emit the rich label centered (or anchored) at `(cx, cy)` into `svg`.
///
/// For a plain label (no markers) this emits a single centered `<text>` matching
/// the previous node-label look exactly.
pub fn emit(
    svg: &mut String,
    label: &str,
    cx: f32,
    cy: f32,
    anchor: Anchor,
    font_size: f32,
    color: [u8; 4],
    font_family: &str,
) {
    if label.is_empty() {
        return;
    }
    if !is_rich(label) {
        emit_plain(svg, label, cx, cy, anchor, font_size, color, font_family);
        return;
    }

    let lines = split_lines(label);
    let line_h = font_size * LINE_HEIGHT_EM;
    // Vertically center the block of lines on cy. Each line's baseline sits a bit
    // below its row top; we approximate the baseline at ~0.32em above the row
    // center (matching the central-baseline look of the plain path).
    let total_h = lines.len() as f32 * line_h;
    let top = cy - total_h / 2.0;
    let family = escape(font_family);
    let fill = rgb(color);
    let fo = opacity(color);

    for (li, line) in lines.iter().enumerate() {
        let runs = parse_line(line);
        let line_w: f32 = runs.iter().map(|r| run_width(r, font_size)).sum();
        // Row top for this line, then its baseline within the row.
        let row_top = top + li as f32 * line_h;
        let baseline_y = row_top + line_h * 0.5 + font_size * 0.32;
        let mut x = match anchor {
            Anchor::Start => cx,
            Anchor::Middle => cx - line_w / 2.0,
        };
        for run in &runs {
            match run {
                Run::Text { s, bold, italic } => {
                    if !s.is_empty() {
                        let weight = if *bold { " font-weight=\"bold\"" } else { "" };
                        let fstyle = if *italic { " font-style=\"italic\"" } else { "" };
                        let _ = write!(
                            svg,
                            "<text x=\"{x:.2}\" y=\"{baseline_y:.2}\" text-anchor=\"start\" \
                             font-family=\"{family}\" font-size=\"{font_size}\" \
                             fill=\"{fill}\"{fo}{weight}{fstyle}>{}</text>",
                            escape(s),
                        );
                    }
                    x += font::line_width(s, font_size);
                }
                Run::Link { text, url } => {
                    let w = font::line_width(text, font_size);
                    if !text.is_empty() {
                        let link_fill = rgb(LINK_COLOR);
                        let link_fo = opacity(LINK_COLOR);
                        // Wrap in <a xlink:href> (harmless; some viewers honor it),
                        // and underline the visible text.
                        let _ = write!(svg, "<a xlink:href=\"{}\">", escape(url));
                        let _ = write!(
                            svg,
                            "<text x=\"{x:.2}\" y=\"{baseline_y:.2}\" text-anchor=\"start\" \
                             font-family=\"{family}\" font-size=\"{font_size}\" \
                             fill=\"{link_fill}\"{link_fo} \
                             text-decoration=\"underline\">{}</text>",
                            escape(text),
                        );
                        svg.push_str("</a>");
                    }
                    x += w;
                }
                Run::Code { s } => {
                    let w = font::line_width(s, font_size);
                    if !s.is_empty() {
                        // Faint rounded background box behind the code.
                        let pad = font_size * 0.15;
                        let bx = x - pad;
                        let bw = w + pad * 2.0;
                        let bh = font_size * 1.1;
                        let by = baseline_y - font_size * 0.85;
                        let bg = rgb(CODE_BG);
                        let bg_fo = opacity(CODE_BG);
                        let r = font_size * 0.15;
                        let _ = write!(
                            svg,
                            "<rect x=\"{bx:.2}\" y=\"{by:.2}\" width=\"{bw:.2}\" \
                             height=\"{bh:.2}\" rx=\"{r:.2}\" ry=\"{r:.2}\" \
                             fill=\"{bg}\"{bg_fo}/>",
                        );
                        let _ = write!(
                            svg,
                            "<text x=\"{x:.2}\" y=\"{baseline_y:.2}\" text-anchor=\"start\" \
                             font-family=\"monospace\" font-size=\"{font_size}\" \
                             fill=\"{fill}\"{fo}>{}</text>",
                            escape(s),
                        );
                    }
                    x += w;
                }
                Run::Math { latex } => {
                    match math_render(latex, font_size, color) {
                        Some(m) => {
                            // Align the math baseline (baseline_px below the SVG
                            // top) onto the text baseline.
                            let inner = strip_svg(&m.svg);
                            let ry = baseline_y - m.baseline_px;
                            let _ = write!(
                                svg,
                                "<g transform=\"translate({x:.2}, {ry:.2})\">{inner}</g>",
                            );
                            x += m.width_px;
                        }
                        None => {
                            // Bad LaTeX: draw the raw `$latex$` as plain text.
                            let raw = format!("${latex}$");
                            let _ = write!(
                                svg,
                                "<text x=\"{x:.2}\" y=\"{baseline_y:.2}\" text-anchor=\"start\" \
                                 font-family=\"{family}\" font-size=\"{font_size}\" \
                                 fill=\"{fill}\"{fo}>{}</text>",
                                escape(&raw),
                            );
                            x += font::line_width(&raw, font_size);
                        }
                    }
                }
            }
        }
    }
}

/// Plain-label fast path: one centered `<text>` (with `<tspan>` rows for `\n`),
/// identical to the previous node-label rendering.
fn emit_plain(
    svg: &mut String,
    label: &str,
    cx: f32,
    cy: f32,
    anchor: Anchor,
    font_size: f32,
    color: [u8; 4],
    font_family: &str,
) {
    let fill = rgb(color);
    let fo = opacity(color);
    let family = escape(font_family);
    let (text_anchor, x) = match anchor {
        Anchor::Start => ("start", cx),
        Anchor::Middle => ("middle", cx),
    };
    let lines: Vec<&str> = label.split('\n').collect();
    let _ = write!(
        svg,
        "<text x=\"{x:.2}\" y=\"{cy:.2}\" text-anchor=\"{text_anchor}\" \
         dominant-baseline=\"central\" font-family=\"{family}\" \
         font-size=\"{font_size}\" fill=\"{fill}\"{fo}>",
    );
    if lines.len() == 1 {
        let _ = write!(svg, "{}", escape(lines[0]));
    } else {
        let line_h = font_size * LINE_HEIGHT_EM;
        let first_dy = -(line_h * (lines.len() as f32 - 1.0)) / 2.0;
        for (i, line) in lines.iter().enumerate() {
            let dy = if i == 0 { first_dy } else { line_h };
            let _ = write!(svg, "<tspan x=\"{x:.2}\" dy=\"{dy:.2}\">{}</tspan>", escape(line));
        }
    }
    svg.push_str("</text>");
}

/// Strip the outer `<svg …>` open tag and the trailing `</svg>` from a complete
/// math SVG document, returning just the inner drawing content to wrap in a `<g>`.
fn strip_svg(doc: &str) -> &str {
    let inner = match doc.find('>') {
        Some(gt) => &doc[gt + 1..],
        None => doc,
    };
    match inner.rfind("</svg>") {
        Some(end) => &inner[..end],
        None => inner,
    }
}

/// ` fill-opacity="…"` for a non-opaque color, else empty (mirrors draw.rs).
fn opacity(color: [u8; 4]) -> String {
    if color[3] < 255 {
        format!(" fill-opacity=\"{:.4}\"", color[3] as f32 / 255.0)
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f32 = 16.0;
    const BLACK: [u8; 4] = [0, 0, 0, 255];

    // ---- parsing ----

    #[test]
    fn parses_bold_italic() {
        let runs = parse_line("a **b** c *d*");
        assert!(runs.contains(&Run::Text { s: "b".into(), bold: true, italic: false }));
        assert!(runs.contains(&Run::Text { s: "d".into(), bold: false, italic: true }));
    }

    #[test]
    fn underscore_emphasis() {
        let runs = parse_line("__x__ _y_");
        assert!(runs.iter().any(|r| matches!(r, Run::Text { s, bold: true, .. } if s == "x")));
        assert!(runs.iter().any(|r| matches!(r, Run::Text { s, italic: true, .. } if s == "y")));
    }

    #[test]
    fn parses_math_run() {
        let runs = parse_line("E = $x^2$ done");
        assert!(runs.iter().any(|r| matches!(r, Run::Math { latex } if latex == "x^2")));
    }

    #[test]
    fn escaped_dollar_is_literal() {
        let runs = parse_line(r"costs \$5");
        assert!(runs.iter().all(|r| matches!(r, Run::Text { .. })));
        let joined: String = runs
            .iter()
            .map(|r| match r {
                Run::Text { s, .. } => s.clone(),
                _ => String::new(),
            })
            .collect();
        assert_eq!(joined, "costs $5");
    }

    #[test]
    fn parses_link_run() {
        let runs = parse_line("see [click here](http://x) now");
        assert!(runs.iter().any(
            |r| matches!(r, Run::Link { text, url } if text == "click here" && url == "http://x")
        ));
        // The raw markdown must not survive as text.
        let joined: String = runs
            .iter()
            .filter_map(|r| match r {
                Run::Text { s, .. } => Some(s.clone()),
                _ => None,
            })
            .collect();
        assert!(!joined.contains('['), "raw link leaked: {joined:?}");
        assert!(!joined.contains("http://x"), "url leaked: {joined:?}");
    }

    #[test]
    fn parses_code_run() {
        let runs = parse_line("run `cargo test` ok");
        assert!(runs.iter().any(|r| matches!(r, Run::Code { s } if s == "cargo test")));
        let joined: String = runs
            .iter()
            .filter_map(|r| match r {
                Run::Text { s, .. } => Some(s.clone()),
                _ => None,
            })
            .collect();
        assert!(!joined.contains('`'), "raw backticks leaked: {joined:?}");
    }

    #[test]
    fn malformed_link_stays_literal() {
        // No `(url)` after the `]` → not a link.
        let runs = parse_line("[not a link]");
        assert!(runs.iter().all(|r| matches!(r, Run::Text { .. })));
    }

    #[test]
    fn split_on_br_variants() {
        assert_eq!(split_lines("a<br>b<br/>c<br />d").len(), 4);
        assert_eq!(split_lines("a\nb").len(), 2);
    }

    // ---- measure ----

    #[test]
    fn bold_measures_at_least_plain() {
        let plain = measure("bold", FS).0;
        let bold = measure("**bold**", FS).0;
        assert!(bold >= plain - 0.5, "bold {bold} vs plain {plain}");
    }

    #[test]
    fn br_label_is_two_lines_tall() {
        let one = measure("a", FS).1;
        let two = measure("a<br>b", FS).1;
        assert!(two > one * 1.5, "two-line {two} vs one-line {one}");
    }

    #[test]
    fn math_label_has_positive_size() {
        let (w, h) = measure("$x^2$", FS);
        assert!(w > 0.0 && h > 0.0, "({w},{h})");
    }

    #[test]
    fn link_measures_visible_text_not_url() {
        // Width should reflect "click here", independent of the (long) url.
        let visible = font::line_width("click here", FS);
        let short = measure("[click here](http://x)", FS).0;
        let long = measure("[click here](http://an-extremely-long-url-here)", FS).0;
        assert!((short - visible).abs() < 0.5, "{short} vs {visible}");
        assert!((short - long).abs() < 0.5, "url affected width: {short} vs {long}");
    }

    #[test]
    fn code_measures_positive() {
        let (w, h) = measure("`code`", FS);
        assert!(w > 0.0 && h > 0.0, "({w},{h})");
    }

    #[test]
    fn plain_label_measures_exactly_font_text_size() {
        let label = "Hello world";
        assert_eq!(measure(label, FS), font::text_size(label, FS));
        let ml = "two\nlines";
        assert_eq!(measure(ml, FS), font::text_size(ml, FS));
    }

    // ---- emit ----

    #[test]
    fn br_emits_two_lines() {
        let mut s = String::new();
        emit(&mut s, "a<br>b", 50.0, 30.0, Anchor::Middle, FS, BLACK, "sans-serif");
        assert_eq!(s.matches("<text").count(), 2, "got: {s}");
    }

    #[test]
    fn bold_run_emits_font_weight() {
        let mut s = String::new();
        emit(&mut s, "**x**", 50.0, 30.0, Anchor::Middle, FS, BLACK, "sans-serif");
        assert!(s.contains("font-weight=\"bold\""), "got: {s}");
    }

    #[test]
    fn italic_run_emits_font_style() {
        let mut s = String::new();
        emit(&mut s, "*x*", 50.0, 30.0, Anchor::Middle, FS, BLACK, "sans-serif");
        assert!(s.contains("font-style=\"italic\""), "got: {s}");
    }

    #[test]
    fn link_emits_underlined_colored_text() {
        let mut s = String::new();
        emit(&mut s, "[click here](http://x)", 50.0, 30.0, Anchor::Middle, FS, BLACK, "sans-serif");
        // Visible text present, raw markdown not leaked.
        assert!(s.contains(">click here</text>"), "got: {s}");
        assert!(!s.contains("[click here]"), "raw markdown leaked: {s}");
        // Underlined.
        assert!(s.contains("text-decoration=\"underline\""), "got: {s}");
        // Themed link color (blue), regardless of BLACK base color.
        assert!(s.contains(&rgb(LINK_COLOR)), "link color missing: {s}");
        // url wrapped in <a> (not leaked as visible text).
        assert!(s.contains("xlink:href=\"http://x\""), "got: {s}");
    }

    #[test]
    fn code_emits_monospace_with_background() {
        let mut s = String::new();
        emit(&mut s, "`code`", 50.0, 30.0, Anchor::Middle, FS, BLACK, "sans-serif");
        assert!(s.contains("font-family=\"monospace\""), "got: {s}");
        assert!(s.contains("<rect"), "expected code background rect: {s}");
        assert!(s.contains(">code</text>"), "got: {s}");
        assert!(!s.contains('`'), "raw backticks leaked: {s}");
    }

    #[test]
    fn math_embeds_g_transform_with_path() {
        let mut s = String::new();
        emit(&mut s, "$x^2$", 50.0, 30.0, Anchor::Middle, FS, BLACK, "sans-serif");
        assert!(s.contains("<g transform"), "expected <g transform, got: {s}");
        assert!(s.contains("<path"), "expected math <path, got: {s}");
        // The outer math <svg> wrapper must have been stripped.
        assert!(!s.contains("<svg"), "inner svg not stripped: {s}");
    }

    #[test]
    fn plain_label_emits_single_centered_text() {
        let mut s = String::new();
        emit(&mut s, "Hello", 50.0, 30.0, Anchor::Middle, FS, BLACK, "sans-serif");
        assert_eq!(s.matches("<text").count(), 1, "got: {s}");
        assert!(s.contains("text-anchor=\"middle\""));
        assert!(s.contains("dominant-baseline=\"central\""));
        assert!(s.contains(">Hello</text>"));
    }

    #[test]
    fn xml_escapes_text() {
        let mut s = String::new();
        emit(&mut s, "**a & b**", 50.0, 30.0, Anchor::Middle, FS, BLACK, "sans-serif");
        assert!(s.contains("a &amp; b"), "got: {s}");
        assert!(!s.contains("a & b"));
    }

    #[test]
    fn bad_latex_falls_back_to_raw_text() {
        let mut s = String::new();
        // An empty math span won't even be treated as math; use clearly-bad latex.
        emit(&mut s, "$\\thisisnotacommand{}$", 50.0, 30.0, Anchor::Middle, FS, BLACK, "sans-serif");
        // Either it renders (a <g>) or falls back to text — never panics, always
        // emits something.
        assert!(!s.is_empty());
    }

    #[test]
    fn deterministic() {
        let render = || {
            let mut s = String::new();
            emit(&mut s, "**a** $x^2$ b", 50.0, 30.0, Anchor::Middle, FS, BLACK, "sans-serif");
            s
        };
        assert_eq!(render(), render());
    }

    #[test]
    fn strip_svg_removes_wrapper() {
        let doc = "<svg xmlns=\"...\" width=\"5\" height=\"6\"><path d=\"M0 0\"/></svg>";
        assert_eq!(strip_svg(doc), "<path d=\"M0 0\"/>");
    }
}
