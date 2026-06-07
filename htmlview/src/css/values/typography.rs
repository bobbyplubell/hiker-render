//! Text- and font-related value enums: font style/weight, text alignment,
//! white-space handling, list-marker style, and inline vertical alignment.

use super::dimension::Length;

/// `font-style`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FontStyle {
    Normal,
    Italic,
    Oblique,
}

/// `font-weight` resolved to a numeric value (100..=900). `normal` = 400,
/// `bold` = 700. Kept numeric so relative `bolder`/`lighter` resolve cleanly.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FontWeight(pub u16);

impl FontWeight {
    pub const NORMAL: FontWeight = FontWeight(400);
    pub const BOLD: FontWeight = FontWeight(700);

    pub fn is_bold(self) -> bool {
        self.0 >= 600
    }
}

/// `text-align`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextAlign {
    Left,
    Right,
    Center,
    Justify,
}

/// `white-space`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WhiteSpace {
    Normal,
    Pre,
    Nowrap,
    PreWrap,
    PreLine,
}

/// `list-style-type` (subset).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ListStyleType {
    None,
    Disc,
    Circle,
    Square,
    Decimal,
    LowerAlpha,
    UpperAlpha,
    LowerRoman,
    UpperRoman,
}

/// `vertical-align` (inline-level + table-cell subset).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum VerticalAlign {
    Baseline,
    Sub,
    Super,
    Top,
    Middle,
    Bottom,
    TextTop,
    TextBottom,
    Length(Length),
}
