//! The Stylo→[`ComputedStyle`] projection boundary: the only place layout/paint
//! obtain a node's style, materializing our owned [`ComputedStyle`] from Stylo's
//! primary `ComputedValues` via the [`read`](super::read) accessors.
//!
//! It also exposes a few computed-value read-back helpers used by tests and the
//! cached CSS initial-value style used for anonymous boxes and unstyled fallbacks.

use style::properties::style_structs::Font;
use style::properties::ComputedValues;
use style::servo_arc::Arc as ServoArc;

use crate::dom::{Document, Node, NodeId};

use super::read;

/// Cached CSS initial-value `ComputedValues` for anonymous boxes / fallback
/// (e.g. a node with no Stylo data, or layout asking for style before a pass).
/// Built once on first use; cheap to clone (Arc bump) thereafter.
pub fn initial_computed_values() -> ServoArc<ComputedValues> {
    use std::sync::OnceLock;
    // `ComputedValues` is `Send + Sync`, so a process-wide cache is fine. We
    // wrap the servo `Arc` (not std `Arc`) in a struct so it can live in the
    // `OnceLock`.
    struct Holder(ServoArc<ComputedValues>);
    // SAFETY: `ComputedValues` is Send + Sync (it's shared across Stylo's
    // parallel traversal), so the holder is too.
    unsafe impl Send for Holder {}
    unsafe impl Sync for Holder {}
    static INITIAL: OnceLock<Holder> = OnceLock::new();
    INITIAL
        .get_or_init(|| Holder(ComputedValues::initial_values_with_font_override(Font::initial_values())))
        .0
        .clone()
}

/// The primary `ComputedValues` for a node, as a cheap Arc clone.
///
/// For an ELEMENT node, returns its own Stylo primary style. For a TEXT node
/// (which Stylo does not style directly), returns the nearest ELEMENT
/// ancestor's primary style — text inherits its parent's style, exactly as the
/// old `style_for` did by walking up. Returns `None` only when no ancestor has
/// been styled (e.g. a node outside a completed Stylo pass).
pub fn primary_computed(doc: &Document, node: NodeId) -> Option<ServoArc<ComputedValues>> {
    let mut cur = Some(node);
    while let Some(id) = cur {
        let n = &doc.nodes[id];
        if let Some(styles) = n.stylo_element_data.primary_styles() {
            // `StyleDataRef` derefs to the primary `Arc<ComputedValues>`; clone
            // the Arc (cheap refcount bump).
            let arc: &ServoArc<ComputedValues> = &styles;
            return Some(arc.clone());
        }
        cur = n.parent;
    }
    None
}

/// The single Stylo→[`ComputedStyle`] projection boundary.
///
/// Materializes our owned [`ComputedStyle`] for a node from Stylo's primary
/// `ComputedValues` (via the [`read`] accessors). Text nodes inherit the nearest
/// styled element ancestor's style (handled by [`primary_computed`]). Unstyled
/// nodes fall back to the CSS initial style. The `<math>` pre-render path stamps
/// an intrinsic replaced size onto `node.replaced_size`, which overrides
/// width/height here.
///
/// This is the ONLY function layout/paint may use to obtain a node's style;
/// Stylo's `ComputedValues` and the `read::*` seam stay confined to this module.
pub fn computed_style_for(doc: &Document, node: NodeId) -> crate::css::computed::ComputedStyle {
    let mut style = match primary_computed(doc, node) {
        Some(cv) => style_from_cv(&cv, doc.nodes[node].replaced_size),
        None => crate::css::computed::ComputedStyle::initial(),
    };
    // Overrides set by the `<math>` pre-render pass to un-hide block math trapped
    // in Wikipedia's MathML a11y wrapper win over Stylo's computed values.
    if let Some(d) = doc.nodes[node].display_override {
        style.display = d;
    }
    if doc.nodes[node].force_visible {
        use crate::css::values::{LengthPercentOrAuto, Position};
        style.opacity = 1.0;
        style.position = Position::Static;
        // The a11y wrapper is clamped to 1px; let it size to its content. Replaced
        // nodes (the `<math>` itself) keep their stamped intrinsic size.
        if doc.nodes[node].replaced_size.is_none() {
            style.width = LengthPercentOrAuto::Auto;
            style.height = LengthPercentOrAuto::Auto;
        }
    }
    style
}

/// Build our [`ComputedStyle`] from Stylo's [`ComputedValues`] using the [`read`]
/// accessors. `replaced` (when `Some`) overrides width/height with an intrinsic
/// replaced size in UNZOOMED px (the `<math>` pre-render path).
fn style_from_cv(
    cv: &ComputedValues,
    replaced: Option<(f32, f32)>,
) -> crate::css::computed::ComputedStyle {
    use crate::css::computed::ComputedStyle;
    use crate::css::values::{Length, LengthPercentOrAuto};
    let (width, height) = match replaced {
        Some((w, h)) => (
            LengthPercentOrAuto::Length(Length::Px(w)),
            LengthPercentOrAuto::Length(Length::Px(h)),
        ),
        None => (read::width(cv), read::height(cv)),
    };
    ComputedStyle {
        display: read::display(cv),
        position: read::position(cv),
        float: read::float(cv),
        clear: read::clear(cv),
        box_sizing: read::box_sizing(cv),

        width,
        height,
        min_width: read::min_width(cv),
        max_width: read::max_width(cv),
        min_height: read::min_height(cv),
        max_height: read::max_height(cv),

        margin: read::margin(cv),
        padding: read::padding(cv),
        border_width: read::border_width(cv),
        border_style: read::border_style(cv),
        border_color: read::border_color(cv),

        top: read::top(cv),
        right: read::right(cv),
        bottom: read::bottom(cv),
        left: read::left(cv),

        color: read::color(cv),
        background_color: read::background_color(cv),

        font_family: read::font_family(cv),
        font_size: read::font_size(cv),
        font_weight: read::font_weight(cv),
        font_style: read::font_style(cv),
        line_height: read::line_height(cv),

        text_align: read::text_align(cv),
        text_decoration_underline: read::text_decoration_underline(cv),
        white_space: read::white_space(cv),
        vertical_align: read::vertical_align(cv),

        list_style_type: read::list_style_type(cv),

        opacity: read::opacity(cv),
    }
}

// ---------------------------------------------------------------------------
// Computed-value read-back helpers (used by tests; Stage 2 layout will grow
// these into the real bridge to our box tree).
// ---------------------------------------------------------------------------

/// The computed `display` of an element, or `None` if it was not styled.
pub fn computed_display(node: &Node) -> Option<style::values::computed::Display> {
    let styles = node.stylo_element_data.primary_styles()?;
    Some(styles.clone_display())
}

/// The computed text `color` of an element as `(r, g, b, a)` (a in 0..=1).
pub fn computed_color(node: &Node) -> Option<(u8, u8, u8, f32)> {
    let styles = node.stylo_element_data.primary_styles()?;
    let cv: &ComputedValues = &styles;
    let srgb = cv.clone_color().into_srgb_legacy();
    let c = srgb.raw_components();
    Some((
        (c[0] * 255.0).round() as u8,
        (c[1] * 255.0).round() as u8,
        (c[2] * 255.0).round() as u8,
        c[3],
    ))
}

/// The computed `font-size` of an element in CSS px, or `None` if not styled.
pub fn computed_font_size_px(node: &Node) -> Option<f32> {
    let styles = node.stylo_element_data.primary_styles()?;
    let cv: &ComputedValues = &styles;
    Some(cv.clone_font_size().used_size().px())
}
