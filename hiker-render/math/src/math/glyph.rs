//! Character → font-glyph mapping for math, including the math-variant
//! (Mathematical Alphanumeric Symbols) substitutions.
//!
//! Math uses *different Unicode codepoints* for styled letters: a math-italic
//! `a` is U+1D44E, not U+0061. The font (STIX Two Math) carries glyphs at those
//! codepoints, so we translate the source char to the right variant codepoint
//! and then ask the face for its glyph id.
//!
//! References: KaTeX `src/buildCommon.ts` (`makeOrd` / `variantFromStyle`) and
//! the MathML-Core `mathvariant` → Mathematical-Alphanumeric mapping. The exact
//! offsets (and the well-known "holes", e.g. italic `h` is U+210E PLANCK
//! CONSTANT, not the missing slot in the italic block) match pulldown-latex's
//! own `Font::map_char` in `mathml.rs`.

use ttf_parser::{Face, GlyphId};

/// Which letterform an atom should be rendered in.
///
/// Covers the default TeX cases (upright roman and math-italic) plus the math
/// font alphabets selected by `\mathbf`, `\mathbb`, `\mathcal`, `\mathfrak`,
/// `\mathsf`, `\mathtt`, `\boldsymbol`, … . Each maps ASCII letters (and, where
/// the alphabet exists, digits / Greek) into the **Mathematical Alphanumeric
/// Symbols** block (U+1D400…), honoring the Letterlike-Symbols holes. The exact
/// per-variant offset arithmetic mirrors pulldown-latex's `Font::map_char` in
/// `mathml.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// Upright roman: the plain codepoint, no substitution.
    Upright,
    /// Math italic (`x`, `\mathnormal`): letters + lowercase/uppercase Greek.
    Italic,
    /// Bold upright (`\mathbf`): letters, digits, Greek.
    Bold,
    /// Bold italic (`\boldsymbol`, `\mathbfit`): letters + Greek.
    BoldItalic,
    /// Blackboard / double-struck (`\mathbb`): letters + digits (the ℂℍℕℙℚℝℤ holes).
    DoubleStruck,
    /// Calligraphic / script (`\mathcal`, `\mathscr`): letters (the ℬℰℱℋℐℒℳℛℯℊℴ holes).
    Script,
    /// Bold calligraphic (`\mathbfscr`): letters.
    BoldScript,
    /// Fraktur (`\mathfrak`): letters (the ℭℌℑℜℨ holes).
    Fraktur,
    /// Bold fraktur (`\mathbffrak`): letters.
    BoldFraktur,
    /// Sans-serif upright (`\mathsf`): letters, digits.
    SansSerif,
    /// Sans-serif bold (`\mathbfsf`): letters, digits, Greek.
    SansSerifBold,
    /// Sans-serif italic (`\mathsfit`): letters.
    SansSerifItalic,
    /// Sans-serif bold italic: letters + Greek.
    SansSerifBoldItalic,
    /// Monospace / typewriter (`\mathtt`): letters, digits.
    Monospace,
}

/// Map a source character to the codepoint that carries its glyph in the chosen
/// [`Variant`], honoring the Unicode block holes.
///
/// Upright leaves the character unchanged. Every other variant mirrors the
/// matching arm of pulldown-latex's `Font::map_char`: only characters that have
/// a styled form are remapped; anything else (digits in alphabets that lack
/// math digits, punctuation, symbols, uppercase Greek without an italic form, …)
/// is returned as-is so it renders upright.
pub fn map_char(ch: char, variant: Variant) -> char {
    let mapped: u32 = match variant {
        Variant::Upright => return ch,
        Variant::Italic => return italic_char(ch),

        // Bold script: U+1D4D0… (caps) / U+1D4EA… (lowercase), no holes.
        Variant::BoldScript => match ch {
            'A'..='Z' => ch as u32 + 0x1D48F,
            'a'..='z' => ch as u32 + 0x1D489,
            _ => return ch,
        },

        // Bold italic: letters, Greek (with the usual ∇/∂/ϵ/ϑ/ϰ/ϕ/ϱ/ϖ extras).
        Variant::BoldItalic => match ch {
            'A'..='Z' => ch as u32 + 0x1D427,
            'a'..='z' => ch as u32 + 0x1D421,
            '\u{0391}'..='\u{03A1}' | '\u{03A3}'..='\u{03A9}' => ch as u32 + 0x1D38B,
            '\u{03F4}' => ch as u32 + 0x1D339,
            '\u{2207}' => ch as u32 + 0x1B52E,
            '\u{03B1}'..='\u{03C9}' => ch as u32 + 0x1D385,
            '\u{2202}' => ch as u32 + 0x1B54D,
            '\u{03F5}' => ch as u32 + 0x1D35B,
            '\u{03D1}' => ch as u32 + 0x1D380,
            '\u{03F0}' => ch as u32 + 0x1D362,
            '\u{03D5}' => ch as u32 + 0x1D37E,
            '\u{03F1}' => ch as u32 + 0x1D363,
            '\u{03D6}' => ch as u32 + 0x1D37F,
            _ => return ch,
        },

        // Bold upright: letters, Greek, digamma, and bold digits.
        Variant::Bold => match ch {
            'A'..='Z' => ch as u32 + 0x1D3BF,
            'a'..='z' => ch as u32 + 0x1D3B9,
            '\u{0391}'..='\u{03A1}' | '\u{03A3}'..='\u{03A9}' => ch as u32 + 0x1D317,
            '\u{03F4}' => ch as u32 + 0x1D2C5,
            '\u{2207}' => ch as u32 + 0x1B4BA,
            '\u{03B1}'..='\u{03C9}' => ch as u32 + 0x1D311,
            '\u{2202}' => ch as u32 + 0x1B4D9,
            '\u{03F5}' => ch as u32 + 0x1D2E7,
            '\u{03D1}' => ch as u32 + 0x1D30C,
            '\u{03F0}' => ch as u32 + 0x1D2EE,
            '\u{03D5}' => ch as u32 + 0x1D30A,
            '\u{03F1}' => ch as u32 + 0x1D2EF,
            '\u{03D6}' => ch as u32 + 0x1D30B,
            '\u{03DC}' | '\u{03DD}' => ch as u32 + 0x1D7CA,
            '0'..='9' => ch as u32 + 0x1D79E,
            _ => return ch,
        },

        // Fraktur: the ℭ ℌ ℑ ℜ ℨ caps live in Letterlike Symbols.
        Variant::Fraktur => match ch {
            'A' | 'B' | 'D'..='G' | 'J'..='Q' | 'S'..='Y' => ch as u32 + 0x1D4C3,
            'C' => ch as u32 + 0x20EA, // ℭ U+212D
            'H' | 'I' => ch as u32 + 0x20C4, // ℌ/ℑ U+210C/2111
            'R' => ch as u32 + 0x20CA, // ℜ U+211C
            'Z' => ch as u32 + 0x20CE, // ℨ U+2128
            'a'..='z' => ch as u32 + 0x1D4BD,
            _ => return ch,
        },

        // Script / calligraphic: the ℬℰℱℋℐℒℳℛ caps and ℯℊℴ lowercase are holes.
        Variant::Script => match ch {
            'A' | 'C' | 'D' | 'G' | 'J' | 'K' | 'N'..='Q' | 'S'..='Z' => ch as u32 + 0x1D45B,
            'B' => ch as u32 + 0x20EA, // ℬ U+212C
            'E' | 'F' => ch as u32 + 0x20EB, // ℰ/ℱ U+2130/2131
            'H' => ch as u32 + 0x20C3, // ℋ U+210B
            'I' => ch as u32 + 0x20C7, // ℐ U+2110
            'L' => ch as u32 + 0x20C6, // ℒ U+2112
            'M' => ch as u32 + 0x20E6, // ℳ U+2133
            'R' => ch as u32 + 0x20C9, // ℛ U+211B
            'a'..='d' | 'f' | 'h'..='n' | 'p'..='z' => ch as u32 + 0x1D455,
            'e' => ch as u32 + 0x20CA, // ℯ U+212F
            'g' => ch as u32 + 0x20A3, // ℊ U+210A
            'o' => ch as u32 + 0x20C5, // ℴ U+2134
            _ => return ch,
        },

        // Monospace: letters + digits, no holes.
        Variant::Monospace => match ch {
            'A'..='Z' => ch as u32 + 0x1D62F,
            'a'..='z' => ch as u32 + 0x1D629,
            '0'..='9' => ch as u32 + 0x1D7C6,
            _ => return ch,
        },

        // Sans-serif upright: letters + digits.
        Variant::SansSerif => match ch {
            'A'..='Z' => ch as u32 + 0x1D55F,
            'a'..='z' => ch as u32 + 0x1D559,
            '0'..='9' => ch as u32 + 0x1D7B2,
            _ => return ch,
        },

        // Double-struck / blackboard: the ℂℍℕℙℚℝℤ caps are holes; bb digits exist.
        Variant::DoubleStruck => match ch {
            'A' | 'B' | 'D'..='G' | 'I'..='M' | 'O' | 'S'..='Y' => ch as u32 + 0x1D4F7,
            'C' => ch as u32 + 0x20BF, // ℂ U+2102
            'H' => ch as u32 + 0x20C5, // ℍ U+210D
            'N' => ch as u32 + 0x20C7, // ℕ U+2115
            'P' | 'Q' => ch as u32 + 0x20C9, // ℙ/ℚ U+2119/211A
            'R' => ch as u32 + 0x20CB, // ℝ U+211D
            'Z' => ch as u32 + 0x20CA, // ℤ U+2124
            'a'..='z' => ch as u32 + 0x1D4F1,
            '0'..='9' => ch as u32 + 0x1D7A8,
            _ => return ch,
        },

        // Bold fraktur: letters, no holes.
        Variant::BoldFraktur => match ch {
            'A'..='Z' => ch as u32 + 0x1D52B,
            'a'..='z' => ch as u32 + 0x1D525,
            _ => return ch,
        },

        // Sans-serif bold italic: letters + Greek.
        Variant::SansSerifBoldItalic => match ch {
            'A'..='Z' => ch as u32 + 0x1D5FB,
            'a'..='z' => ch as u32 + 0x1D5F5,
            '\u{0391}'..='\u{03A1}' | '\u{03A3}'..='\u{03A9}' => ch as u32 + 0x1D3FF,
            '\u{03F4}' => ch as u32 + 0x1D3AD,
            '\u{2207}' => ch as u32 + 0x1B5A2,
            '\u{03B1}'..='\u{03C9}' => ch as u32 + 0x1D3F9,
            '\u{2202}' => ch as u32 + 0x1B5C1,
            '\u{03F5}' => ch as u32 + 0x1D3CF,
            '\u{03D1}' => ch as u32 + 0x1D3F4,
            '\u{03F0}' => ch as u32 + 0x1D3D6,
            '\u{03D5}' => ch as u32 + 0x1D3F2,
            '\u{03F1}' => ch as u32 + 0x1D3D7,
            '\u{03D6}' => ch as u32 + 0x1D3F3,
            _ => return ch,
        },

        // Sans-serif italic: letters only (no math digits/Greek).
        Variant::SansSerifItalic => match ch {
            'A'..='Z' => ch as u32 + 0x1D5D7,
            'a'..='z' => ch as u32 + 0x1D5C1,
            _ => return ch,
        },

        // Sans-serif bold: letters, Greek, digits.
        Variant::SansSerifBold => match ch {
            'A'..='Z' => ch as u32 + 0x1D593,
            'a'..='z' => ch as u32 + 0x1D58D,
            '\u{0391}'..='\u{03A1}' | '\u{03A3}'..='\u{03A9}' => ch as u32 + 0x1D3C5,
            '\u{03F4}' => ch as u32 + 0x1D373,
            '\u{2207}' => ch as u32 + 0x1B568,
            '\u{03B1}'..='\u{03C9}' => ch as u32 + 0x1D3BF,
            '\u{2202}' => ch as u32 + 0x1B587,
            '\u{03F5}' => ch as u32 + 0x1D395,
            '\u{03D1}' => ch as u32 + 0x1D3BA,
            '\u{03F0}' => ch as u32 + 0x1D39C,
            '\u{03D5}' => ch as u32 + 0x1D3B8,
            '\u{03F1}' => ch as u32 + 0x1D39D,
            '\u{03D6}' => ch as u32 + 0x1D3B9,
            '0'..='9' => ch as u32 + 0x1D7BC,
            _ => return ch,
        },
    };
    char::from_u32(mapped).unwrap_or(ch)
}

/// Map a pulldown-latex [`Font`] to our [`Variant`]. The TeX-default arms
/// (`UpRight`, `Italic`) are handled here so callers can map an explicit font
/// directly; the no-explicit-font case is decided by `variant_for` in
/// `box_layout`.
pub fn variant_from_font(font: pulldown_latex::event::Font) -> Variant {
    use pulldown_latex::event::Font;
    match font {
        Font::UpRight => Variant::Upright,
        Font::Italic => Variant::Italic,
        Font::Bold => Variant::Bold,
        Font::BoldItalic => Variant::BoldItalic,
        Font::DoubleStruck => Variant::DoubleStruck,
        Font::Script => Variant::Script,
        Font::BoldScript => Variant::BoldScript,
        Font::Fraktur => Variant::Fraktur,
        Font::BoldFraktur => Variant::BoldFraktur,
        Font::SansSerif => Variant::SansSerif,
        Font::BoldSansSerif => Variant::SansSerifBold,
        Font::SansSerifItalic => Variant::SansSerifItalic,
        Font::SansSerifBoldItalic => Variant::SansSerifBoldItalic,
        Font::Monospace => Variant::Monospace,
    }
}

/// Mathematical-Italic substitution for a single character, or the character
/// itself when no italic form exists. Offsets match pulldown-latex's
/// `Font::Italic` mapping (see module docs).
fn italic_char(ch: char) -> char {
    let mapped: u32 = match ch {
        // Latin: A–Z and a–z, with the `h` hole (no U+1D455; uses U+210E).
        'A'..='Z' => ch as u32 + 0x1D3F3,
        'a'..='g' | 'i'..='z' => ch as u32 + 0x1D3ED,
        'h' => ch as u32 + 0x20A6, // 'h' (U+0068) → U+210E PLANCK CONSTANT
        // Lowercase Greek α–ω → Mathematical Italic small letters.
        '\u{03B1}'..='\u{03C9}' => ch as u32 + 0x1D34B,
        // Uppercase Greek (Α–Ρ, Σ–Ω) → Mathematical Italic capitals.
        '\u{0391}'..='\u{03A1}' | '\u{03A3}'..='\u{03A9}' => ch as u32 + 0x1D351,
        // Anything else has no italic form: render upright.
        _ => return ch,
    };
    // The arithmetic above is over valid Unicode scalar ranges by construction.
    char::from_u32(mapped).unwrap_or(ch)
}

/// Resolve a character + variant to a glyph id in `face`, falling back to the
/// unmapped (upright) glyph when the variant glyph is absent. Returns `None`
/// only if the font has no glyph for the character at all.
pub fn glyph_for(face: &Face<'_>, ch: char, variant: Variant) -> Option<GlyphId> {
    let mapped = map_char(ch, variant);
    face.glyph_index(mapped)
        .or_else(|| face.glyph_index(ch)) // variant glyph missing → upright
}
