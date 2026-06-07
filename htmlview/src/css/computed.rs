//! `ComputedStyle`: a fully-resolved, owned style for a single node.
//!
//! Produced by the cascade. Inheritance is resolved explicitly (no parent
//! pointer trick). Lengths that depend on layout context (percentages, `auto`)
//! are kept symbolic; absolute bits (border widths, colors, font size) are
//! resolved to px / `Color` here where it is unambiguous.

use crate::css::values::*;
use crate::geom::Edges;

/// One font-family entry in the family list.
pub type FontFamily = String;

/// The resolved style of a node. Every field has a defined initial value (see
/// [`ComputedStyle::initial`] / the `Default` impl).
#[derive(Clone, Debug, PartialEq)]
pub struct ComputedStyle {
    // --- box / formatting ---
    pub display: Display,
    pub position: Position,
    pub float: Float,
    pub clear: Clear,
    pub box_sizing: BoxSizing,

    // --- sizing (kept symbolic; resolved during layout) ---
    pub width: LengthPercentOrAuto,
    pub height: LengthPercentOrAuto,
    pub min_width: LengthOrPercent,
    pub max_width: Option<LengthOrPercent>, // None == `none`
    pub min_height: LengthOrPercent,
    pub max_height: Option<LengthOrPercent>, // None == `none`

    // --- box edges ---
    /// Margins keep `auto` (needed for centering / shrink-to-fit).
    pub margin: Edges<LengthPercentOrAuto>,
    /// Padding resolved to a length-or-percent (no `auto`).
    pub padding: Edges<LengthOrPercent>,
    /// Border widths resolved to px.
    pub border_width: Edges<f32>,
    pub border_style: Edges<BorderStyle>,
    pub border_color: Edges<Color>,

    // --- positioned offsets ---
    pub top: LengthPercentOrAuto,
    pub right: LengthPercentOrAuto,
    pub bottom: LengthPercentOrAuto,
    pub left: LengthPercentOrAuto,

    // --- color / background ---
    pub color: Color,
    /// Background color; `None` == transparent.
    pub background_color: Option<Color>,

    // --- font (inherited) ---
    pub font_family: Vec<FontFamily>,
    pub font_size: f32, // px
    pub font_weight: FontWeight,
    pub font_style: FontStyle,
    /// Resolved line height in px. `None` == `normal` (derive from font metrics).
    pub line_height: Option<f32>,

    // --- text (inherited) ---
    pub text_align: TextAlign,
    pub text_decoration_underline: bool,
    pub white_space: WhiteSpace,
    pub vertical_align: VerticalAlign,

    // --- lists (inherited) ---
    pub list_style_type: ListStyleType,

    /// Opacity in [0, 1]. 0 means the box (and its subtree) is fully invisible —
    /// honored by the paint pass to skip painting. (Partial alpha is not
    /// composited; any value > 0 paints fully.)
    pub opacity: f32,
}

impl ComputedStyle {
    /// The CSS initial-value style (root of inheritance before any author CSS).
    pub fn initial() -> Self {
        ComputedStyle {
            display: Display::Inline,
            position: Position::Static,
            float: Float::None,
            clear: Clear::None,
            box_sizing: BoxSizing::ContentBox,

            width: LengthPercentOrAuto::Auto,
            height: LengthPercentOrAuto::Auto,
            min_width: LengthOrPercent::Length(Length::ZERO),
            max_width: None,
            min_height: LengthOrPercent::Length(Length::ZERO),
            max_height: None,

            margin: Edges::splat(LengthPercentOrAuto::Length(Length::ZERO)),
            padding: Edges::splat(LengthOrPercent::Length(Length::ZERO)),
            border_width: Edges::ZERO,
            border_style: Edges::splat(BorderStyle::None),
            border_color: Edges::splat(Color::BLACK),

            top: LengthPercentOrAuto::Auto,
            right: LengthPercentOrAuto::Auto,
            bottom: LengthPercentOrAuto::Auto,
            left: LengthPercentOrAuto::Auto,

            color: Color::BLACK,
            background_color: None,

            font_family: vec!["sans-serif".to_string()],
            font_size: 16.0,
            font_weight: FontWeight::NORMAL,
            font_style: FontStyle::Normal,
            line_height: None,

            text_align: TextAlign::Left,
            text_decoration_underline: false,
            white_space: WhiteSpace::Normal,
            vertical_align: VerticalAlign::Baseline,

            list_style_type: ListStyleType::Disc,

            opacity: 1.0,
        }
    }
}

impl Default for ComputedStyle {
    fn default() -> Self {
        ComputedStyle::initial()
    }
}
