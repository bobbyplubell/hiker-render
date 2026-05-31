//! CSS value enums: the parsed, but not-yet-resolved, property value vocabulary.
//!
//! Lengths keep their units so the cascade/layout can resolve them lazily
//! against font metrics + the containing block. Colors reuse `egui::Color32`
//! so paint can consume them directly without a conversion step.

/// A CSS color. We reuse `egui::Color32` (sRGBA, premultiplied-friendly) so the
/// paint layer can pass it straight through to egui shapes with no conversion.
pub type Color = egui::Color32;

/// `display` property. Subset relevant to a static document renderer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Display {
    None,
    Inline,
    Block,
    InlineBlock,
    ListItem,
    Table,
    TableRow,
    TableCell,
    TableRowGroup,
    TableHeaderGroup,
    TableFooterGroup,
    TableColumn,
    TableColumnGroup,
    TableCaption,
    Flex,
}

/// A length with units. Resolved to px lazily at use sites.
///
/// `Px` is already absolute. `Em`/`Ex`/`Rem` resolve against font metrics;
/// `Vw`/`Vh` against the viewport. Percentages are a separate type because they
/// resolve against different bases depending on the property.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Length {
    Px(f32),
    Em(f32),
    Ex(f32),
    Rem(f32),
    Vw(f32),
    Vh(f32),
}

impl Length {
    pub const ZERO: Length = Length::Px(0.0);
}

/// `<length> | auto` — used by margins, width/height, top/left, etc.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LengthOrAuto {
    Auto,
    Length(Length),
}

/// `<length> | <percentage>` — used by width/height/min/max and similar.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LengthOrPercent {
    Length(Length),
    Percent(f32), // 0.0..=1.0 (i.e. 50% -> 0.5)
}

/// `<length> | <percentage> | auto`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LengthPercentOrAuto {
    Auto,
    Length(Length),
    Percent(f32),
}

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

/// `float`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Float {
    None,
    Left,
    Right,
}

/// `clear`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Clear {
    None,
    Left,
    Right,
    Both,
}

/// `position` (subset: only static + relative supported in v1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Position {
    Static,
    Relative,
    Absolute,
    Fixed,
}

/// `box-sizing`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BoxSizing {
    ContentBox,
    BorderBox,
}

/// `border-*-style`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BorderStyle {
    None,
    Hidden,
    Solid,
    Dotted,
    Dashed,
    Double,
    Groove,
    Ridge,
    Inset,
    Outset,
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
