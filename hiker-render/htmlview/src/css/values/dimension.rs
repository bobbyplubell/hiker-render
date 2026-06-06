//! Length, length-or-percent, and color value types.
//!
//! Lengths keep their units so the cascade/layout can resolve them lazily
//! against font metrics and the containing block; percentages are a separate
//! arm because they resolve against different bases per property.

/// A CSS color. We reuse `egui::Color32` (sRGBA, premultiplied-friendly) so the
/// paint layer can pass it straight through to egui shapes with no conversion.
pub type Color = egui::Color32;

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
