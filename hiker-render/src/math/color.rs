//! Color-name / hex → RGBA resolution and a small pre-pass that normalizes the
//! color argument of `\color`/`\textcolor`/`\colorbox`/`\fcolorbox` to a hex form
//! pulldown-latex always accepts.
//!
//! pulldown-latex resolves a color command's argument itself (named CSS3 colors
//! and `#RRGGBB`), but it *rejects* anything else — an unknown name, or a short
//! `#RGB` — by returning a parser error, which would otherwise fail the whole
//! render. To keep the rest of an equation rendering (TeX/LaTeX color is a visual
//! nicety, not a structural feature), [`normalize_color_args`] rewrites each color
//! argument through [`resolve`] into a canonical `#rrggbb` literal before parsing:
//! recognized names and hex pass through as their resolved value, and anything we
//! cannot resolve falls back to `default` (the inherited text color). The painter
//! then sees a valid color and the surrounding atoms render normally.

/// Resolve a TeX/LaTeX color token to straight RGBA, or `None` if unrecognized.
///
/// Accepts `#rgb` / `#rrggbb` hex (with or without the `#`) and the common CSS3 /
/// LaTeX `xcolor` named colors. Names are matched case-insensitively. The returned
/// alpha is always `255` (opaque); callers blend their own alpha if needed.
pub fn resolve(token: &str) -> Option<[u8; 4]> {
    let t = token.trim();
    if let Some(rgb) = parse_hex(t) {
        return Some(rgb);
    }
    named(&t.to_ascii_lowercase())
}

/// Parse a `#rgb` / `#rrggbb` hex color (the leading `#` is optional). A 3-digit
/// form expands each nibble (`#abc` → `#aabbcc`). Returns `None` on any other
/// length or a non-hex digit.
fn parse_hex(token: &str) -> Option<[u8; 4]> {
    let h = token.strip_prefix('#').unwrap_or(token);
    if !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    match h.len() {
        3 => {
            let n = |i: usize| {
                let d = u8::from_str_radix(&h[i..i + 1], 16).ok()?;
                Some(d * 17) // 0xA → 0xAA
            };
            Some([n(0)?, n(1)?, n(2)?, 255])
        }
        6 => {
            let n = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
            Some([n(0)?, n(2)?, n(4)?, 255])
        }
        _ => None,
    }
}

/// The hand-rolled named-color table (lowercased name → RGB). Covers the common
/// CSS3 / LaTeX `xcolor` names; unknown names return `None`.
fn named(name: &str) -> Option<[u8; 4]> {
    let rgb: [u8; 3] = match name {
        "black" => [0, 0, 0],
        "white" => [255, 255, 255],
        "red" => [255, 0, 0],
        "green" => [0, 128, 0],
        "lime" => [0, 255, 0],
        "blue" => [0, 0, 255],
        "yellow" => [255, 255, 0],
        "cyan" => [0, 255, 255],
        "magenta" => [255, 0, 255],
        "orange" => [255, 165, 0],
        "purple" => [128, 0, 128],
        "brown" => [165, 42, 42],
        "pink" => [255, 192, 203],
        "teal" => [0, 128, 128],
        "navy" => [0, 0, 128],
        "maroon" => [128, 0, 0],
        "olive" => [128, 128, 0],
        "gray" | "grey" => [128, 128, 128],
        "darkgray" | "darkgrey" => [169, 169, 169],
        "lightgray" | "lightgrey" => [211, 211, 211],
        _ => return None,
    };
    Some([rgb[0], rgb[1], rgb[2], 255])
}

/// Rewrite the color argument of every `\color`/`\textcolor`/`\colorbox`/
/// `\fcolorbox` command in `src` to a canonical `#rrggbb` literal, resolving each
/// token via [`resolve`] and falling back to `default` for anything unrecognized.
///
/// This guarantees pulldown-latex sees only `#rrggbb` colors (which it always
/// accepts), so an unknown color name no longer fails the whole parse — its
/// command simply renders in the default color. Non-color text is left untouched.
/// The scan is a light hand-rolled pass (no regex / new deps): it looks for the
/// command names, then for each it rewrites the immediately following `{…}` group
/// (two groups for `\fcolorbox`).
pub fn normalize_color_args(src: &str, default: [u8; 4]) -> String {
    // Commands and how many color arguments each takes up front.
    const CMDS: &[(&str, usize)] = &[
        ("\\textcolor", 1),
        ("\\colorbox", 1),
        ("\\fcolorbox", 2),
        ("\\color", 1),
    ];

    let mut out = String::with_capacity(src.len());
    let bytes = src.as_bytes();
    let mut i = 0;
    'outer: while i < bytes.len() {
        if bytes[i] == b'\\' {
            for &(cmd, n_args) in CMDS {
                if src[i..].starts_with(cmd)
                    // Ensure a full command match (next char isn't a letter, so
                    // `\colorbox` isn't matched as `\color` + `box`).
                    && !src[i + cmd.len()..]
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphabetic())
                {
                    out.push_str(cmd);
                    let mut j = i + cmd.len();
                    for _ in 0..n_args {
                        // Skip whitespace, then rewrite the next `{…}` group.
                        let ws_start = j;
                        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                            j += 1;
                        }
                        if j < bytes.len() && bytes[j] == b'{' {
                            if let Some(end) = src[j + 1..].find('}') {
                                let token = &src[j + 1..j + 1 + end];
                                let rgb = resolve(token).unwrap_or(default);
                                out.push_str(&src[ws_start..j]); // preserved whitespace
                                out.push('{');
                                let _ = std::fmt::Write::write_fmt(
                                    &mut out,
                                    format_args!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]),
                                );
                                out.push('}');
                                j = j + 1 + end + 1;
                                continue;
                            }
                        }
                        // No `{…}` group where expected: bail out, copy verbatim.
                        out.push_str(&src[ws_start..j]);
                    }
                    i = j;
                    continue 'outer;
                }
            }
        }
        // Copy this byte's char verbatim.
        let ch = src[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_named_colors() {
        assert_eq!(resolve("red"), Some([255, 0, 0, 255]));
        assert_eq!(resolve("BLUE"), Some([0, 0, 255, 255]));
        assert_eq!(resolve(" green "), Some([0, 128, 0, 255]));
        assert_eq!(resolve("grey"), resolve("gray"));
        assert!(resolve("notacolor").is_none());
    }

    #[test]
    fn resolves_hex() {
        assert_eq!(resolve("#00ff00"), Some([0, 255, 0, 255]));
        assert_eq!(resolve("#abc"), Some([0xaa, 0xbb, 0xcc, 255]));
        assert_eq!(resolve("00FF00"), Some([0, 255, 0, 255]));
        assert!(resolve("#12").is_none());
        assert!(resolve("#gggggg").is_none());
    }

    #[test]
    fn normalizes_known_and_unknown() {
        let d = [0, 0, 0, 255];
        assert_eq!(
            normalize_color_args(r"\textcolor{red}{x}", d),
            r"\textcolor{#ff0000}{x}"
        );
        // Unknown name falls back to the default color, leaving the body intact.
        assert_eq!(
            normalize_color_args(r"\textcolor{bogus}{x}+y", d),
            r"\textcolor{#000000}{x}+y"
        );
        // 3-digit hex (which pulldown would reject) is expanded.
        assert_eq!(
            normalize_color_args(r"{\color{#abc} z}", d),
            r"{\color{#aabbcc} z}"
        );
        // `\colorbox` is matched as itself, not `\color` + `box`.
        assert_eq!(
            normalize_color_args(r"\colorbox{blue}{x}", d),
            r"\colorbox{#0000ff}{x}"
        );
    }

    #[test]
    fn leaves_non_color_text_untouched() {
        let d = [0, 0, 0, 255];
        let s = r"\frac{1}{2} + \sqrt{x}";
        assert_eq!(normalize_color_args(s, d), s);
    }
}
