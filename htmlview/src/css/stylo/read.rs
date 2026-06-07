//! A read-layer over Stylo's `ComputedValues` that returns exactly the values
//! our layout consumes — mirroring [`crate::css::computed::ComputedStyle`].
//!
//! Each function maps a Stylo computed property to OUR value vocabulary
//! ([`crate::css::values`] enums, [`crate::geom::Edges`], etc.) so the rest of
//! the codebase is unaffected. This is the seam Stage 2b will use to flip every
//! layout module from reading a field off `ComputedStyle` to calling
//! `read::fn(computed_values)`.
//!
//! ## Conventions (load-bearing — must match `ComputedStyle`)
//! - Absolute lengths are returned in **UNZOOMED CSS px**. Layout multiplies by
//!   `zoom` at use sites; we must NOT bake zoom in here.
//! - Percentages and `auto` stay symbolic ([`LengthPercentOrAuto`] /
//!   [`LengthOrPercent`]) so layout resolves them against the containing block
//!   exactly as today.
//! - `calc()` values are flattened to their absolute-length component where the
//!   percentage basis is unknown here (best-effort; see notes in `lp_or_auto`).
//!
//! ## Known Stage-2b gaps (Stylo 0.17 / Servo build)
//! - `vertical-align` is **not a longhand** in this Stylo build (Servo omits
//!   it). [`vertical_align`] always returns `VerticalAlign::Baseline`. Layout
//!   that needs real vertical-align must keep the old cascade for that one
//!   property, or we upgrade Stylo.
//! - `list-style-type` only round-trips the predefined keyword set we support;
//!   `symbols()` / unknown counter styles fall back to `Disc`.

use style::properties::ComputedValues;

use crate::css::values::{
    BorderStyle, BoxSizing, Clear, Color, Display, Float, FontStyle, FontWeight, Length,
    LengthOrPercent, LengthPercentOrAuto, ListStyleType, Position, TextAlign, VerticalAlign,
    WhiteSpace,
};
use crate::geom::Edges;

// ---------------------------------------------------------------------------
// Stylo type aliases (mirroring blitz's `stylo_taffy` module so the mapping
// code reads the same way as the upstream reference).
// ---------------------------------------------------------------------------
mod stylo {
    pub use style::properties::longhands::position::computed_value::T as Position;
    pub use style::values::computed::length_percentage::Unpacked as UnpackedLengthPercentage;
    pub use style::values::computed::{BorderSideWidth, LengthPercentage};
    pub use style::values::generics::length::{
        GenericLengthPercentageOrNormal, GenericMargin, GenericMaxSize, GenericSize,
    };
    pub use style::values::generics::position::Inset as GenericInset;
    pub use style::values::generics::NonNegative;
    pub use style::properties::longhands::box_sizing::computed_value::T as BoxSizing;
    pub use style::values::specified::border::BorderStyle;
    pub use style::values::specified::box_::{Clear, Display, Float};

    pub use style::values::computed::Percentage;
    pub type MarginVal = GenericMargin<LengthPercentage>;
    pub type InsetVal = GenericInset<Percentage, LengthPercentage>;
    pub type Size = GenericSize<NonNegative<LengthPercentage>>;
    pub type MaxSize = GenericMaxSize<NonNegative<LengthPercentage>>;
    pub type LpOrNormal = GenericLengthPercentageOrNormal<LengthPercentage>;

    pub use style::values::computed::font::{GenericFontFamily, LineHeight, SingleFontFamily};
    pub use style::values::specified::box_::{DisplayInside, DisplayOutside};
}

// ---------------------------------------------------------------------------
// Length / length-percentage primitives.
// ---------------------------------------------------------------------------

/// A computed `LengthPercentage` → our [`LengthOrPercent`] (UNZOOMED px).
///
/// `calc()` mixing length+percent collapses to the percentage component if any,
/// else the length; pure forms map exactly.
fn lp(value: &stylo::LengthPercentage) -> LengthOrPercent {
    match value.unpack() {
        stylo::UnpackedLengthPercentage::Length(len) => {
            LengthOrPercent::Length(Length::Px(len.px()))
        }
        stylo::UnpackedLengthPercentage::Percentage(pct) => LengthOrPercent::Percent(pct.0),
        stylo::UnpackedLengthPercentage::Calc(_) => {
            // calc() with a known absolute resolution: use the resolved px when
            // there is no percentage basis (percentage==0), else fall back to the
            // percentage. `to_used_value(None)`-style resolution is unavailable
            // without a basis, so approximate via the length component.
            if let Some(px) = value.to_length().map(|l| l.px()) {
                LengthOrPercent::Length(Length::Px(px))
            } else if let Some(p) = value.to_percentage() {
                LengthOrPercent::Percent(p.0)
            } else {
                LengthOrPercent::Length(Length::ZERO)
            }
        }
    }
}

/// A computed `LengthPercentage` → our [`LengthPercentOrAuto`] (never `Auto`).
fn lp_as_lpa(value: &stylo::LengthPercentage) -> LengthPercentOrAuto {
    match lp(value) {
        LengthOrPercent::Length(l) => LengthPercentOrAuto::Length(l),
        LengthOrPercent::Percent(p) => LengthPercentOrAuto::Percent(p),
    }
}

// ---------------------------------------------------------------------------
// Box / formatting context.
// ---------------------------------------------------------------------------

/// `display` — maps Stylo's outside×inside display to our flat [`Display`].
///
/// This is the load-bearing mapping: table-internal display values must come
/// through intact for table layout to trigger.
pub fn display(cv: &ComputedValues) -> Display {
    let d: stylo::Display = cv.clone_display();
    let inside = d.inside();
    let outside = d.outside();

    // `display: none` (either axis None).
    if matches!(outside, stylo::DisplayOutside::None)
        || matches!(inside, stylo::DisplayInside::None)
    {
        return Display::None;
    }

    // list-item is encoded as a flag on top of an outside/inside pair.
    if d.is_list_item() {
        return Display::ListItem;
    }

    use stylo::DisplayInside as DI;
    use stylo::DisplayOutside as DO;
    match inside {
        DI::Flex => Display::Flex,
        DI::Table => Display::Table,
        DI::TableRowGroup => Display::TableRowGroup,
        DI::TableHeaderGroup => Display::TableHeaderGroup,
        DI::TableFooterGroup => Display::TableFooterGroup,
        DI::TableRow => Display::TableRow,
        DI::TableColumn => Display::TableColumn,
        DI::TableColumnGroup => Display::TableColumnGroup,
        DI::TableCell => Display::TableCell,
        // Flow / FlowRoot / Contents resolve by outside axis.
        DI::Flow | DI::FlowRoot | DI::Contents | DI::Grid => match outside {
            DO::Block => {
                if matches!(outside, DO::Block) && matches!(inside, DI::FlowRoot) {
                    // block flow-root is still a block box for our purposes.
                    Display::Block
                } else {
                    Display::Block
                }
            }
            DO::Inline => {
                // inline flow-root == inline-block.
                if matches!(inside, DI::FlowRoot) {
                    Display::InlineBlock
                } else {
                    Display::Inline
                }
            }
            DO::TableCaption => Display::TableCaption,
            DO::InternalTable => Display::TableRow, // shouldn't occur with Flow inside
            DO::None => Display::None,
        },
        // Grid handled in arm above (mapped onto Block — we have no grid layout).
        _ => match outside {
            DO::Block => Display::Block,
            DO::Inline => Display::Inline,
            DO::TableCaption => Display::TableCaption,
            DO::InternalTable | DO::None => Display::Block,
        },
    }
}

/// `position`.
pub fn position(cv: &ComputedValues) -> Position {
    match cv.clone_position() {
        stylo::Position::Static => Position::Static,
        stylo::Position::Relative => Position::Relative,
        stylo::Position::Absolute => Position::Absolute,
        stylo::Position::Fixed => Position::Fixed,
        // Sticky has no dedicated variant in our model; treat as relative.
        stylo::Position::Sticky => Position::Relative,
    }
}

/// `float`.
pub fn float(cv: &ComputedValues) -> Float {
    match cv.clone_float() {
        stylo::Float::None => Float::None,
        stylo::Float::Left | stylo::Float::InlineStart => Float::Left,
        stylo::Float::Right | stylo::Float::InlineEnd => Float::Right,
    }
}

/// `clear`.
pub fn clear(cv: &ComputedValues) -> Clear {
    match cv.clone_clear() {
        stylo::Clear::None => Clear::None,
        stylo::Clear::Left | stylo::Clear::InlineStart => Clear::Left,
        stylo::Clear::Right | stylo::Clear::InlineEnd => Clear::Right,
        stylo::Clear::Both => Clear::Both,
    }
}

/// `box-sizing`.
pub fn box_sizing(cv: &ComputedValues) -> BoxSizing {
    match cv.clone_box_sizing() {
        stylo::BoxSizing::ContentBox => BoxSizing::ContentBox,
        stylo::BoxSizing::BorderBox => BoxSizing::BorderBox,
    }
}

// ---------------------------------------------------------------------------
// Sizing (kept symbolic).
// ---------------------------------------------------------------------------

fn size_to_lpa(size: &stylo::Size) -> LengthPercentOrAuto {
    match size {
        stylo::Size::LengthPercentage(v) => lp_as_lpa(&v.0),
        stylo::Size::Auto => LengthPercentOrAuto::Auto,
        // Intrinsic keywords have no fixed length; treat as `auto`.
        _ => LengthPercentOrAuto::Auto,
    }
}

fn size_to_lp(size: &stylo::Size) -> LengthOrPercent {
    match size {
        stylo::Size::LengthPercentage(v) => lp(&v.0),
        // `auto` min-width/min-height behaves as 0 in our model.
        _ => LengthOrPercent::Length(Length::ZERO),
    }
}

fn max_size_to_lp(size: &stylo::MaxSize) -> Option<LengthOrPercent> {
    match size {
        stylo::MaxSize::LengthPercentage(v) => Some(lp(&v.0)),
        stylo::MaxSize::None => None,
        _ => None,
    }
}

/// `width`.
pub fn width(cv: &ComputedValues) -> LengthPercentOrAuto {
    size_to_lpa(&cv.get_position().width)
}
/// `height`.
pub fn height(cv: &ComputedValues) -> LengthPercentOrAuto {
    size_to_lpa(&cv.get_position().height)
}
/// `min-width` (`auto` → 0).
pub fn min_width(cv: &ComputedValues) -> LengthOrPercent {
    size_to_lp(&cv.get_position().min_width)
}
/// `max-width` (`none` → `None`).
pub fn max_width(cv: &ComputedValues) -> Option<LengthOrPercent> {
    max_size_to_lp(&cv.get_position().max_width)
}
/// `min-height` (`auto` → 0).
pub fn min_height(cv: &ComputedValues) -> LengthOrPercent {
    size_to_lp(&cv.get_position().min_height)
}
/// `max-height` (`none` → `None`).
pub fn max_height(cv: &ComputedValues) -> Option<LengthOrPercent> {
    max_size_to_lp(&cv.get_position().max_height)
}

// ---------------------------------------------------------------------------
// Box edges: margin / padding / border.
// ---------------------------------------------------------------------------

fn margin_to_lpa(m: &stylo::MarginVal) -> LengthPercentOrAuto {
    match m {
        stylo::MarginVal::Auto => LengthPercentOrAuto::Auto,
        stylo::MarginVal::LengthPercentage(lpv) => lp_as_lpa(lpv),
        _ => LengthPercentOrAuto::Auto,
    }
}

/// `margin` edges (each may be `auto`).
pub fn margin(cv: &ComputedValues) -> Edges<LengthPercentOrAuto> {
    let m = cv.get_margin();
    Edges {
        top: margin_to_lpa(&m.margin_top),
        right: margin_to_lpa(&m.margin_right),
        bottom: margin_to_lpa(&m.margin_bottom),
        left: margin_to_lpa(&m.margin_left),
    }
}

/// `padding` edges (length-or-percent, no `auto`).
pub fn padding(cv: &ComputedValues) -> Edges<LengthOrPercent> {
    let p = cv.get_padding();
    Edges {
        top: lp(&p.padding_top.0),
        right: lp(&p.padding_right.0),
        bottom: lp(&p.padding_bottom.0),
        left: lp(&p.padding_left.0),
    }
}

fn border_width_px(width: &stylo::BorderSideWidth, style: stylo::BorderStyle) -> f32 {
    // CSS: a `none`/`hidden` border has zero used width regardless of width.
    if style.none_or_hidden() {
        return 0.0;
    }
    width.0.to_f32_px()
}

/// `border-*-width` edges resolved to UNZOOMED px (honoring none/hidden → 0).
pub fn border_width(cv: &ComputedValues) -> Edges<f32> {
    let b = cv.get_border();
    Edges {
        top: border_width_px(&b.border_top_width, b.border_top_style),
        right: border_width_px(&b.border_right_width, b.border_right_style),
        bottom: border_width_px(&b.border_bottom_width, b.border_bottom_style),
        left: border_width_px(&b.border_left_width, b.border_left_style),
    }
}

fn map_border_style(s: stylo::BorderStyle) -> BorderStyle {
    match s {
        stylo::BorderStyle::None => BorderStyle::None,
        stylo::BorderStyle::Hidden => BorderStyle::Hidden,
        stylo::BorderStyle::Solid => BorderStyle::Solid,
        stylo::BorderStyle::Dotted => BorderStyle::Dotted,
        stylo::BorderStyle::Dashed => BorderStyle::Dashed,
        stylo::BorderStyle::Double => BorderStyle::Double,
        stylo::BorderStyle::Groove => BorderStyle::Groove,
        stylo::BorderStyle::Ridge => BorderStyle::Ridge,
        stylo::BorderStyle::Inset => BorderStyle::Inset,
        stylo::BorderStyle::Outset => BorderStyle::Outset,
    }
}

/// `border-*-style` edges.
pub fn border_style(cv: &ComputedValues) -> Edges<BorderStyle> {
    let b = cv.get_border();
    Edges {
        top: map_border_style(b.border_top_style),
        right: map_border_style(b.border_right_style),
        bottom: map_border_style(b.border_bottom_style),
        left: map_border_style(b.border_left_style),
    }
}

/// `border-*-color` edges resolved against the element's own `color`
/// (`currentColor`).
pub fn border_color(cv: &ComputedValues) -> Edges<Color> {
    let b = cv.get_border();
    let current = color(cv);
    Edges {
        top: resolve_color(&b.border_top_color, current),
        right: resolve_color(&b.border_right_color, current),
        bottom: resolve_color(&b.border_bottom_color, current),
        left: resolve_color(&b.border_left_color, current),
    }
}

// ---------------------------------------------------------------------------
// Positioned offsets.
// ---------------------------------------------------------------------------

fn inset_to_lpa(v: &stylo::InsetVal) -> LengthPercentOrAuto {
    match v {
        stylo::InsetVal::Auto => LengthPercentOrAuto::Auto,
        stylo::InsetVal::LengthPercentage(lpv) => lp_as_lpa(lpv),
        _ => LengthPercentOrAuto::Auto,
    }
}

/// `top`.
pub fn top(cv: &ComputedValues) -> LengthPercentOrAuto {
    inset_to_lpa(&cv.get_position().top)
}
/// `right`.
pub fn right(cv: &ComputedValues) -> LengthPercentOrAuto {
    inset_to_lpa(&cv.get_position().right)
}
/// `bottom`.
pub fn bottom(cv: &ComputedValues) -> LengthPercentOrAuto {
    inset_to_lpa(&cv.get_position().bottom)
}
/// `left`.
pub fn left(cv: &ComputedValues) -> LengthPercentOrAuto {
    inset_to_lpa(&cv.get_position().left)
}

// ---------------------------------------------------------------------------
// Color / background.
// ---------------------------------------------------------------------------

/// Resolve a Stylo computed `Color` (which may be `currentColor`) to our
/// [`Color`], using `current` as the currentColor value.
fn resolve_color(c: &style::values::computed::Color, current: Color) -> Color {
    // `into_color()` resolves currentColor against the given foreground.
    let absolute = c.resolve_to_absolute(&into_absolute(current));
    absolute_to_color(absolute)
}

fn into_absolute(c: Color) -> style::color::AbsoluteColor {
    let [r, g, b, a] = c.to_array();
    style::color::AbsoluteColor::srgb_legacy(r, g, b, a as f32 / 255.0)
}

fn absolute_to_color(absolute: style::color::AbsoluteColor) -> Color {
    let srgb = absolute.into_srgb_legacy();
    let c = srgb.raw_components();
    Color::from_rgba_unmultiplied(
        (c[0] * 255.0).round() as u8,
        (c[1] * 255.0).round() as u8,
        (c[2] * 255.0).round() as u8,
        (c[3] * 255.0).round() as u8,
    )
}

/// `color` (the inherited foreground color). `clone_color()` is already an
/// absolute `AbsoluteColor` (currentColor is resolved during the cascade).
pub fn color(cv: &ComputedValues) -> Color {
    absolute_to_color(cv.clone_color())
}

/// `background-color`; `None` == fully transparent.
pub fn background_color(cv: &ComputedValues) -> Option<Color> {
    let bg = cv.clone_background_color();
    let resolved = resolve_color(&bg, color(cv));
    if resolved.a() == 0 {
        None
    } else {
        Some(resolved)
    }
}

// ---------------------------------------------------------------------------
// Font.
// ---------------------------------------------------------------------------

/// `font-family` as our ordered list of family names (generics lowercased to
/// `serif`/`sans-serif`/`monospace`/… so `FontCtx::family` folds them).
pub fn font_family(cv: &ComputedValues) -> Vec<String> {
    let font = cv.get_font();
    let mut out = Vec::new();
    for fam in font.font_family.families.iter() {
        match fam {
            stylo::SingleFontFamily::FamilyName(name) => out.push(name.name.to_string()),
            stylo::SingleFontFamily::Generic(g) => out.push(generic_family_name(*g).to_string()),
        }
    }
    if out.is_empty() {
        out.push("sans-serif".to_string());
    }
    out
}

fn generic_family_name(g: stylo::GenericFontFamily) -> &'static str {
    match g {
        stylo::GenericFontFamily::Serif => "serif",
        stylo::GenericFontFamily::SansSerif | stylo::GenericFontFamily::None => "sans-serif",
        stylo::GenericFontFamily::Monospace => "monospace",
        stylo::GenericFontFamily::Cursive => "cursive",
        stylo::GenericFontFamily::Fantasy => "fantasy",
        stylo::GenericFontFamily::SystemUi => "system-ui",
    }
}

/// `font-size` in UNZOOMED CSS px.
pub fn font_size(cv: &ComputedValues) -> f32 {
    cv.clone_font_size().used_size().px()
}

/// `font-weight` as a numeric value (100..=900).
pub fn font_weight(cv: &ComputedValues) -> FontWeight {
    let w = cv.clone_font_weight().value();
    FontWeight(w.round().clamp(1.0, 1000.0) as u16)
}

/// `font-style`.
pub fn font_style(cv: &ComputedValues) -> FontStyle {
    let s = cv.clone_font_style();
    if s == style::values::computed::font::FontStyle::NORMAL {
        FontStyle::Normal
    } else if s == style::values::computed::font::FontStyle::ITALIC {
        FontStyle::Italic
    } else {
        // Any oblique angle.
        FontStyle::Oblique
    }
}

/// `line-height` resolved to UNZOOMED px. `None` == `normal` (layout derives it
/// from font metrics, matching `ComputedStyle::line_height == None`).
pub fn line_height(cv: &ComputedValues) -> Option<f32> {
    let lh: stylo::LineHeight = cv.clone_line_height();
    match lh {
        stylo::LineHeight::Normal => None,
        stylo::LineHeight::Number(n) => {
            // Unitless multiplier against the element's own font-size.
            Some(n.0 * font_size(cv))
        }
        stylo::LineHeight::Length(len) => Some(len.0.px()),
        #[allow(unreachable_patterns)]
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Text.
// ---------------------------------------------------------------------------

/// `text-align`.
pub fn text_align(cv: &ComputedValues) -> TextAlign {
    use style::values::computed::text::TextAlign as TA;
    match cv.clone_text_align() {
        TA::Left | TA::MozLeft | TA::Start => TextAlign::Left,
        TA::Right | TA::MozRight | TA::End => TextAlign::Right,
        TA::Center | TA::MozCenter => TextAlign::Center,
        TA::Justify => TextAlign::Justify,
    }
}

/// Whether `text-decoration-line` includes `underline` on THIS element.
///
/// Note: text-decoration does not inherit in CSS; layout that wants the
/// "an ancestor underlined me" effect must propagate it itself, exactly as the
/// old cascade required.
pub fn text_decoration_underline(cv: &ComputedValues) -> bool {
    let line = cv.clone_text_decoration_line();
    line.contains(style::values::specified::text::TextDecorationLine::UNDERLINE)
}

/// `white-space`, reconstructed from `white-space-collapse` + `text-wrap-mode`
/// (Stylo 0.17 splits the legacy shorthand into these two longhands).
pub fn white_space(cv: &ComputedValues) -> WhiteSpace {
    use style::computed_values::text_wrap_mode::T as TextWrapMode;
    use style::computed_values::white_space_collapse::T as WhiteSpaceCollapse;

    let collapse = cv.clone_white_space_collapse();
    let wrap = cv.clone_text_wrap_mode();
    let nowrap = matches!(wrap, TextWrapMode::Nowrap);

    match collapse {
        WhiteSpaceCollapse::Collapse => {
            if nowrap {
                WhiteSpace::Nowrap
            } else {
                WhiteSpace::Normal
            }
        }
        WhiteSpaceCollapse::Preserve => {
            if nowrap {
                WhiteSpace::Pre
            } else {
                WhiteSpace::PreWrap
            }
        }
        WhiteSpaceCollapse::PreserveBreaks => WhiteSpace::PreLine,
        // `break-spaces` (and any gecko-only extra) behaves like pre-wrap for us.
        _ => WhiteSpace::PreWrap,
    }
}

/// `vertical-align`.
///
/// **Stage-2b gap:** this Stylo build (Servo) has no `vertical-align` longhand,
/// so this always returns `Baseline`. See module docs.
pub fn vertical_align(_cv: &ComputedValues) -> VerticalAlign {
    VerticalAlign::Baseline
}

// ---------------------------------------------------------------------------
// Lists.
// ---------------------------------------------------------------------------

/// `list-style-type` (predefined keyword subset; unknown styles → `Disc`).
pub fn list_style_type(cv: &ComputedValues) -> ListStyleType {
    use style::counter_style::CounterStyle;
    let lst = cv.clone_list_style_type();
    match &lst.0 {
        CounterStyle::None => ListStyleType::None,
        CounterStyle::Name(name) => match name.0.to_string().as_str() {
            "disc" => ListStyleType::Disc,
            "circle" => ListStyleType::Circle,
            "square" => ListStyleType::Square,
            "decimal" => ListStyleType::Decimal,
            "lower-alpha" | "lower-latin" => ListStyleType::LowerAlpha,
            "upper-alpha" | "upper-latin" => ListStyleType::UpperAlpha,
            "lower-roman" => ListStyleType::LowerRoman,
            "upper-roman" => ListStyleType::UpperRoman,
            _ => ListStyleType::Disc,
        },
        // `symbols()` and other forms: fall back to disc.
        _ => ListStyleType::Disc,
    }
}

// ---------------------------------------------------------------------------
// Effects.
// ---------------------------------------------------------------------------

/// `opacity` in [0, 1].
pub fn opacity(cv: &ComputedValues) -> f32 {
    cv.get_effects().opacity.clamp(0.0, 1.0)
}

// Keep the `LpOrNormal` alias referenced so it doesn't warn if a future caller
// (e.g. gap) is added; currently unused by the property set ComputedStyle has.
#[allow(dead_code)]
type _LpOrNormalUnused = stylo::LpOrNormal;
