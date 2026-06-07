//! Small SVG string helpers shared by the bitfield and timing renderers.

use std::fmt::Write as _;

/// XML-escape text for inclusion in an SVG `<text>` body or attribute.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// `rgb(r,g,b)` for an RGBA color (alpha carried separately via opacity attrs).
pub fn rgb(c: [u8; 4]) -> String {
    format!("rgb({},{},{})", c[0], c[1], c[2])
}

/// ` {name}="…"` opacity attribute for a non-opaque color, else empty.
pub fn opacity_attr(name: &str, c: [u8; 4]) -> String {
    if c[3] < 255 {
        format!(" {name}=\"{:.4}\"", c[3] as f32 / 255.0)
    } else {
        String::new()
    }
}

/// Append a `<text>` element (centered or anchored) to `svg`.
pub fn text(
    svg: &mut String,
    s: &str,
    x: f32,
    y: f32,
    anchor: &str,
    font_size: f32,
    family: &str,
    fill: [u8; 4],
    weight: Option<&str>,
) {
    if s.is_empty() {
        return;
    }
    let w = weight.map(|w| format!(" font-weight=\"{w}\"")).unwrap_or_default();
    let _ = write!(
        svg,
        "<text x=\"{x:.2}\" y=\"{y:.2}\" text-anchor=\"{anchor}\" \
         dominant-baseline=\"central\" font-family=\"{fam}\" font-size=\"{font_size}\" \
         fill=\"{fill}\"{fo}{w}>{txt}</text>",
        fam = escape(family),
        fill = rgb(fill),
        fo = opacity_attr("fill-opacity", fill),
        txt = escape(s),
    );
}
