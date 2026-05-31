//! `FontCtx`: map a `ComputedStyle` font to an `egui::FontId` + metrics, lay out
//! text runs into galleys, and cache both measurements and galleys.
//!
//! Text is *measured* through egui's `Fonts`; we own line breaking. Word galleys
//! repeat heavily, so `(text, font_id, color)` -> `Arc<Galley>` is cached and
//! the resulting galleys are reused directly by the paint pass.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use egui::{Color32, FontFamily, FontId, Galley};

use crate::css::computed::ComputedStyle;
use crate::css::values::WhiteSpace;

/// Font metrics derived from egui for a given resolved font.
#[derive(Clone, Copy, Debug, Default)]
pub struct FontMetrics {
    pub ascent: f32,
    pub descent: f32,
    pub line_height: f32,
}

/// Cache key for a laid-out run: text + font (size bits + family) + color.
type GalleyKey = (String, u32, u64, u32);

/// Holds the egui `Context` handle for measurement plus measurement/galley
/// caches. Constructed per-layout from the `egui::Context` passed into
/// `HtmlView::layout`. `zoom` multiplies every px-derived dimension (font size
/// and line-height live here; margins/padding/border are zoomed at construction
/// time).
pub struct FontCtx {
    ctx: egui::Context,
    zoom: f32,
    galley_cache: RefCell<HashMap<GalleyKey, Arc<Galley>>>,
    width_cache: RefCell<HashMap<(String, u32, u64), f32>>,
}

impl FontCtx {
    pub fn new(ctx: egui::Context, zoom: f32) -> Self {
        FontCtx {
            ctx,
            zoom,
            galley_cache: RefCell::new(HashMap::new()),
            width_cache: RefCell::new(HashMap::new()),
        }
    }

    /// The egui context used for measurement.
    pub fn ctx(&self) -> &egui::Context {
        &self.ctx
    }

    /// The zoom factor applied to px dimensions.
    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    /// Resolve a computed style's font-family list into an egui `FontFamily`.
    ///
    /// egui ships only `Proportional` and `Monospace` families by default; we
    /// fold the CSS generic families onto those. The first family entry that
    /// maps to a known generic wins; otherwise we default to proportional.
    fn family(style: &ComputedStyle) -> FontFamily {
        for fam in &style.font_family {
            let f = fam.trim().trim_matches(|c| c == '"' || c == '\'').to_ascii_lowercase();
            match f.as_str() {
                "monospace" | "courier" | "courier new" | "code" | "consolas" | "menlo"
                | "monaco" => return FontFamily::Monospace,
                "serif" | "sans-serif" | "system-ui" | "ui-serif" | "ui-sans-serif" | "ui-system"
                | "system" | "arial" | "helvetica" | "times" | "times new roman" | "georgia"
                | "verdana" => return FontFamily::Proportional,
                _ => {}
            }
        }
        FontFamily::Proportional
    }

    /// Resolve a computed style's font into an `egui::FontId` (size includes zoom).
    pub fn font_id(&self, style: &ComputedStyle) -> FontId {
        let size = (style.font_size * self.zoom).max(1.0);
        FontId::new(size, Self::family(style))
    }

    /// Font metrics for the given style. `line_height` honors the computed
    /// `line-height` (None == normal ≈ 1.2×size); ascent/descent are
    /// approximated from egui's row height (egui does not expose them directly).
    pub fn metrics(&self, style: &ComputedStyle) -> FontMetrics {
        let font = self.font_id(style);
        let row_height = self.ctx.fonts(|f| f.row_height(&font));
        let size = font.size;
        // `line-height` is already resolved to px by the cascade (None == normal).
        let line_height = match style.line_height {
            Some(px) => px * self.zoom,
            None => row_height.max(size * 1.2),
        };
        // egui has no public ascent/descent; approximate from the row height.
        let ascent = row_height * 0.8;
        let descent = row_height - ascent;
        FontMetrics {
            ascent,
            descent,
            line_height,
        }
    }

    /// Lay out a single (already-split) run with no wrapping, returning a cached
    /// `Arc<Galley>`. Color comes from the style.
    pub fn layout_run(&self, text: &str, style: &ComputedStyle) -> Arc<Galley> {
        let font = self.font_id(style);
        let color = style.color;
        let key = (
            text.to_owned(),
            font.size.to_bits(),
            family_bits(&font.family),
            color.to_array_u32(),
        );
        if let Some(g) = self.galley_cache.borrow().get(&key) {
            return g.clone();
        }
        let galley = self
            .ctx
            .fonts(|f| f.layout_no_wrap(text.to_owned(), font.clone(), color));
        self.galley_cache.borrow_mut().insert(key, galley.clone());
        galley
    }

    /// Measure the unwrapped width of `text` in the given style, with caching.
    pub fn measure_width(&self, text: &str, style: &ComputedStyle) -> f32 {
        let font = self.font_id(style);
        let key = (text.to_owned(), font.size.to_bits(), family_bits(&font.family));
        if let Some(&w) = self.width_cache.borrow().get(&key) {
            return w;
        }
        let color = style.color;
        let galley = self
            .ctx
            .fonts(|f| f.layout_no_wrap(text.to_owned(), font.clone(), color));
        let w = galley.size().x;
        self.width_cache.borrow_mut().insert(key, w);
        w
    }
}

/// Stable bits for a `FontFamily` so it can join a hashable cache key.
fn family_bits(family: &FontFamily) -> u64 {
    match family {
        FontFamily::Monospace => 1,
        FontFamily::Proportional => 2,
        FontFamily::Name(name) => {
            // Cheap hash of the name; collisions only cost a re-layout.
            let mut h: u64 = 1469598103934665603;
            for b in name.as_bytes() {
                h ^= *b as u64;
                h = h.wrapping_mul(1099511628211);
            }
            h | (1 << 63)
        }
    }
}

/// Whether a `white-space` value collapses runs of whitespace.
pub fn collapses_whitespace(ws: WhiteSpace) -> bool {
    matches!(ws, WhiteSpace::Normal | WhiteSpace::Nowrap | WhiteSpace::PreLine)
}

/// Whether a `white-space` value allows soft wrapping between words.
pub fn allows_wrap(ws: WhiteSpace) -> bool {
    matches!(ws, WhiteSpace::Normal | WhiteSpace::PreWrap | WhiteSpace::PreLine)
}

/// Helper extension: pack a `Color32` into a `u32` cache component.
trait Color32Bits {
    fn to_array_u32(self) -> u32;
}
impl Color32Bits for Color32 {
    fn to_array_u32(self) -> u32 {
        let [r, g, b, a] = self.to_array();
        u32::from_le_bytes([r, g, b, a])
    }
}
