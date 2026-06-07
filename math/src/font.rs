//! Bundled OpenType **MATH** font and thin [`ttf_parser`] helpers.
//!
//! We ship **STIX Two Math** (SIL OFL 1.1 — see `fonts/OFL.txt`) so math layout
//! is self-contained and deterministic, independent of host system fonts. The
//! layout engine reads glyph metrics + the MATH table from here and emits glyph
//! outlines as SVG paths, so rasterization needs no font at all.

/// Raw bytes of the bundled STIX Two Math font (CFF/OpenType).
pub const STIX_TWO_MATH: &[u8] = include_bytes!("../fonts/STIXTwoMath-Regular.otf");

/// Parse the bundled math font into a [`ttf_parser::Face`]. The returned face
/// borrows `'static` bundled bytes, so it is freely constructible anywhere.
pub fn math_face() -> ttf_parser::Face<'static> {
    ttf_parser::Face::parse(STIX_TWO_MATH, 0).expect("bundled STIX Two Math is a valid font")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Counts outline segments so we can confirm CFF outlining works.
    #[derive(Default, Debug)]
    struct Counter {
        moves: u32,
        lines: u32,
        curves: u32,
    }
    impl ttf_parser::OutlineBuilder for Counter {
        fn move_to(&mut self, _: f32, _: f32) {
            self.moves += 1;
        }
        fn line_to(&mut self, _: f32, _: f32) {
            self.lines += 1;
        }
        fn quad_to(&mut self, _: f32, _: f32, _: f32, _: f32) {
            self.curves += 1;
        }
        fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {
            self.curves += 1;
        }
        fn close(&mut self) {}
    }

    #[test]
    fn font_parses_and_exposes_math_table() {
        let face = math_face();
        // STIX Two Math uses a 1000-unit em.
        assert_eq!(face.units_per_em(), 1000, "expected 1000 upm");

        // The MATH table and its constants must be present.
        let math = face.tables().math.expect("MATH table present");
        let consts = math.constants.expect("MATH constants present");
        let axis = consts.axis_height().value;
        assert!(axis > 0, "math axis height should be positive, got {axis}");
        let rule = consts.fraction_rule_thickness().value;
        assert!(rule > 0, "fraction rule thickness positive, got {rule}");
        eprintln!("[math-font] upm=1000 axis_height={axis} frac_rule={rule}");
    }

    #[test]
    fn can_map_and_outline_a_glyph() {
        let face = math_face();
        let gid = face.glyph_index('x').expect("glyph for 'x'");
        let mut c = Counter::default();
        let bbox = face.outline_glyph(gid, &mut c).expect("outline for 'x'");
        assert!(c.moves >= 1 && (c.lines + c.curves) > 0, "non-empty outline: {c:?}");
        assert!(bbox.x_max > bbox.x_min && bbox.y_max > bbox.y_min, "sane bbox {bbox:?}");
        eprintln!(
            "[math-font] 'x' gid={} segments: moves={} lines={} curves={} bbox={:?}",
            gid.0, c.moves, c.lines, c.curves, bbox
        );
    }
}
