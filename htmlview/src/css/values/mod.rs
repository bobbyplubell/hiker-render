//! CSS value enums: the parsed, but not-yet-resolved, property value vocabulary.
//!
//! Lengths keep their units so the cascade/layout can resolve them lazily
//! against font metrics + the containing block. Colors reuse `egui::Color32`
//! so paint can consume them directly without a conversion step. The vocabulary
//! is grouped by category: [`dimension`] (lengths/percentages/color),
//! [`typography`] (font/text enums), and [`box_model`] (display/position/border).

pub mod box_model;
pub mod dimension;
pub mod typography;

pub use box_model::{BorderStyle, BoxSizing, Clear, Display, Float, Position};
pub use dimension::{Color, Length, LengthOrAuto, LengthOrPercent, LengthPercentOrAuto};
pub use typography::{
    FontStyle, FontWeight, ListStyleType, TextAlign, VerticalAlign, WhiteSpace,
};
