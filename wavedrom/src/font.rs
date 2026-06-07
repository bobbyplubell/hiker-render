//! Real text metrics from the bundled **Liberation Sans** (SIL OFL 1.1 — see
//! `fonts/LICENSE`), read with [`ttf_parser`]. Same approach as the mermaid
//! crate: sum real glyph advances so measured widths match the drawn `<text>`.

use std::sync::OnceLock;

/// Bundled Liberation Sans Regular (SIL OFL 1.1 — see `fonts/LICENSE`). Exposed
/// so a rasterizer can load the exact font we measured with into its fontdb.
pub const FONT_BYTES: &[u8] = include_bytes!("../fonts/LiberationSans-Regular.ttf");

/// The bundled font's family name — what generic `sans-serif` should resolve to.
pub const FONT_FAMILY: &str = "Liberation Sans";

struct Metrics {
    upm: f32,
    ascii_adv: [u16; 256],
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
        units += if adv == 0 { m.default_adv } else { adv } as u32;
    }
    units as f32 * font_size / m.upm
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variable_width() {
        let narrow = line_width("iiiiiiii", 16.0);
        let wide = line_width("WWWWWWWW", 16.0);
        assert!(wide > narrow * 1.5, "W-run {wide} should dwarf i-run {narrow}");
    }
}
