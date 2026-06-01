//! Real text metrics from the bundled **Liberation Sans** (SIL OFL 1.1 — see
//! `fonts/LICENSE`), read with [`ttf_parser`].
//!
//! Node boxes need to know a label's pixel size at *layout* time, but the crate
//! emits SVG and never rasterizes — so we measure here by summing real glyph
//! advances instead of a fixed-width heuristic. The bundled font is also what
//! the SVG `<text>` resolves to (the rasterizer maps generic `sans-serif` to /
//! loads this font), so what we **measure** matches what gets **drawn**.

use std::sync::OnceLock;

/// Bundled Liberation Sans Regular (SIL OFL 1.1 — see `fonts/LICENSE`). Exposed
/// so a rasterizer can load the exact font we measured with into its fontdb.
pub const FONT_BYTES: &[u8] = include_bytes!("../fonts/LiberationSans-Regular.ttf");

/// The bundled font's family name — what generic `sans-serif` should resolve to.
pub const FONT_FAMILY: &str = "Liberation Sans";

/// Line height as a fraction of the font size.
const LINE_HEIGHT_EM: f32 = 1.2;

struct Metrics {
    /// Units per em (the font's design grid).
    upm: f32,
    /// Horizontal advance (font units) for each `char` in `0..256`.
    ascii_adv: [u16; 256],
    /// Advance used for any char outside the ASCII/Latin-1 table.
    default_adv: u16,
}

fn metrics() -> &'static Metrics {
    static M: OnceLock<Metrics> = OnceLock::new();
    M.get_or_init(|| {
        let face =
            ttf_parser::Face::parse(FONT_BYTES, 0).expect("bundled Liberation Sans is valid");
        let upm = face.units_per_em() as f32;
        let adv_of = |ch: char| -> u16 {
            face.glyph_index(ch)
                .and_then(|g| face.glyph_hor_advance(g))
                .unwrap_or(0)
        };
        let mut ascii_adv = [0u16; 256];
        for c in 0u32..256 {
            if let Some(ch) = char::from_u32(c) {
                ascii_adv[c as usize] = adv_of(ch);
            }
        }
        // Non-ASCII fallback: a typical lowercase advance ('n'), else ~0.5em.
        let default_adv = {
            let n = adv_of('n');
            if n > 0 { n } else { (upm * 0.5) as u16 }
        };
        Metrics { upm, ascii_adv, default_adv }
    })
}

/// Advance width in px of a single (one-line) string at `font_size`.
pub fn line_width(line: &str, font_size: f32) -> f32 {
    let m = metrics();
    let mut units: u32 = 0;
    for ch in line.chars() {
        let adv = if (ch as u32) < 256 {
            m.ascii_adv[ch as usize]
        } else {
            m.default_adv
        };
        // Some control chars have a 0 advance; treat them as the fallback so a
        // stray char doesn't collapse the width to nothing.
        units += if adv == 0 { m.default_adv } else { adv } as u32;
    }
    units as f32 * font_size / m.upm
}

/// `(width, height)` in px for `label`: width = widest line's advance, height =
/// `line_count × 1.2em`. Real glyph advances from the bundled font (variable
/// width — `iiii` is narrow, `WWWW` is wide), so boxes hug the text.
pub fn text_size(label: &str, font_size: f32) -> (f32, f32) {
    let mut max_w = 0.0f32;
    let mut lines = 0usize;
    for line in label.split('\n') {
        max_w = max_w.max(line_width(line, font_size));
        lines += 1;
    }
    (max_w, lines.max(1) as f32 * font_size * LINE_HEIGHT_EM)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variable_width() {
        // Real metrics: a run of wide glyphs must measure wider than the same
        // count of narrow ones (a fixed-width heuristic would tie them).
        let narrow = line_width("iiiiiiii", 16.0);
        let wide = line_width("WWWWWWWW", 16.0);
        assert!(wide > narrow * 1.5, "W-run {wide} should dwarf i-run {narrow}");
    }

    #[test]
    fn scales_with_size_and_length() {
        assert!(line_width("hello", 32.0) > line_width("hello", 16.0));
        assert!(line_width("hello world", 16.0) > line_width("hello", 16.0));
        assert!(line_width("", 16.0).abs() < 0.001);
    }

    #[test]
    fn multiline_height_and_max_width() {
        let (w, h1) = text_size("short", 16.0);
        let (w2, h2) = text_size("short\nmuch longer line", 16.0);
        assert!(h2 > h1, "two lines taller");
        assert!(w2 > w, "width tracks the widest line");
    }
}
