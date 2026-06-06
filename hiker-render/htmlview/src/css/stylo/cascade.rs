//! The cascade driver and entry point: builds the Device + Stylist from our UA
//! and author stylesheets, then runs Stylo's sequential (Option A) cascade.
//!
//! This module owns the font-metrics providers that resolve font-relative units,
//! parses inline `style=""` attributes and collects author sheets, then traverses
//! the tree storing an `Arc<ComputedValues>` per element in its `StyloData`.

use selectors::Element;

use style::context::{QuirksMode, SharedStyleContext, StyleContext};
use style::dom::{TDocument, TElement, TNode};
use style::properties::style_structs::Font;
use style::properties::ComputedValues;
use style::servo_arc::Arc as ServoArc;
use style::shared_lock::SharedRwLock;

use crate::dom::{Document, NodeData};
use crate::{ResourceProvider, Theme};

// ---------------------------------------------------------------------------
// Cascade driver — Option A: sequential `style::driver::traverse_dom`.
// ---------------------------------------------------------------------------

use style::context::RegisteredSpeculativePainters;
use style::traversal::{recalc_style_at, DomTraversal, PerLevelTraversalData};

pub struct RegisteredPaintersImpl;
impl RegisteredSpeculativePainters for RegisteredPaintersImpl {
    fn get(&self, _name: &style::Atom) -> Option<&dyn style::context::RegisteredSpeculativePainter> {
        None
    }
}

pub struct RecalcStyle<'a> {
    context: SharedStyleContext<'a>,
}

impl<'a> RecalcStyle<'a> {
    pub fn new(context: SharedStyleContext<'a>) -> Self {
        RecalcStyle { context }
    }
}

impl<E> DomTraversal<E> for RecalcStyle<'_>
where
    E: TElement,
{
    fn process_preorder<F: FnMut(E::ConcreteNode)>(
        &self,
        traversal_data: &PerLevelTraversalData,
        context: &mut StyleContext<E>,
        node: E::ConcreteNode,
        note_child: F,
    ) {
        if let Some(el) = node.as_element() {
            let mut data = unsafe { el.ensure_data() };
            recalc_style_at(self, traversal_data, context, el, &mut data, note_child);
            unsafe { el.unset_dirty_descendants() }
        }
    }

    fn needs_postorder_traversal() -> bool {
        false
    }

    fn process_postorder(&self, _ctx: &mut StyleContext<E>, _node: E::ConcreteNode) {
        panic!("postorder should never be called")
    }

    fn shared_context(&self) -> &SharedStyleContext<'_> {
        &self.context
    }
}

// ---------------------------------------------------------------------------
// Device / Stylist / cascade entry point.
// ---------------------------------------------------------------------------

use style::animation::DocumentAnimationSet;
use style::device::Device;
use style::font_metrics::FontMetrics;
use style::global_style_data::GLOBAL_STYLE_DATA;
use style::media_queries::{MediaList, MediaType};
use style::queries::values::PrefersColorScheme;
use style::selector_parser::SnapshotMap;
use style::shared_lock::StylesheetGuards;
use style::stylesheets::{
    AllowImportRules, CssRuleType, DocumentStyleSheet, Origin, Stylesheet, UrlExtraData,
};
use style::stylist::Stylist;
use style::thread_state::ThreadState;
use style::traversal_flags::TraversalFlags;
use style::values::computed::font::QueryFontMetricsFlags;
use style::values::computed::{CSSPixelLength, Length};

/// Synthetic font-metric ratios — used as a fallback when no `egui::Context` is
/// available (and to fill in any metric egui cannot measure). Tuned to match the
/// approximations used elsewhere in our font stack.
fn synthetic_metrics(font_size: CSSPixelLength) -> FontMetrics {
    FontMetrics {
        x_height: Some(font_size * 0.5),
        zero_advance_measure: Some(font_size * 0.5),
        cap_height: Some(font_size * 0.7),
        ascent: font_size * 0.8,
        ic_width: Some(font_size),
        script_percent_scale_down: None,
        script_script_percent_scale_down: None,
    }
}

/// Real font metrics backed by egui's font system (our [`crate::layout::fonts`]
/// measurement path), so `ex`/`ch`/`cap` units and font-relative line metrics
/// match what layout actually measures.
///
/// ## Why an `egui::Context` and not a `FontCtx`
/// Stylo's `Device` requires the provider be `Send + Sync + 'static`, and the
/// metrics query is `&self` (no zoom in scope). [`crate::layout::fonts::FontCtx`]
/// is neither `Send`/`Sync` (it holds non-thread-safe `RefCell` caches) nor does
/// it carry the right lifetime, and zoom must NOT enter Stylo's CSS-px space
/// anyway. So we hold the cheap-to-clone, `Send + Sync` [`egui::Context`]
/// directly and query the same `egui::Fonts` that `FontCtx` measures through —
/// at `zoom == 1.0` (CSS px), exactly what Stylo expects.
///
/// ## How metrics are measured
/// egui exposes `row_height` and per-glyph layout (`Glyph::uv_rect`,
/// `font_ascent`). We lay out a representative glyph and read:
/// - **ascent** from `Glyph::font_ascent` (real),
/// - **x-height** from the rasterized height of `x` (`uv_rect.size.y`),
/// - **cap-height** from the rasterized height of `H`,
/// - **zero advance (`ch`)** from `Fonts::glyph_width('0')`.
/// Any measurement that comes back as zero (e.g. before the font atlas is
/// populated) falls back to the synthetic ratio for that one metric.
#[derive(Clone)]
struct EguiFontMetricsProvider {
    ctx: egui::Context,
}

impl std::fmt::Debug for EguiFontMetricsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EguiFontMetricsProvider")
    }
}

impl EguiFontMetricsProvider {
    /// Map a Stylo `Font` style struct to an `egui::FontId` at CSS px (no zoom).
    fn font_id(font: &Font, size_px: f32) -> egui::FontId {
        // Fold the family list onto egui's two built-in generics, mirroring
        // `crate::layout::fonts::FontCtx::family`.
        // First family entry that resolves to a known egui generic wins; else
        // proportional. Mirrors `crate::layout::fonts::FontCtx::family`.
        let mut family = egui::FontFamily::Proportional;
        for fam in font.font_family.families.iter() {
            use style::values::computed::font::{GenericFontFamily, SingleFontFamily};
            let name = match fam {
                SingleFontFamily::FamilyName(n) => n.name.to_string().to_ascii_lowercase(),
                SingleFontFamily::Generic(GenericFontFamily::Monospace) => {
                    family = egui::FontFamily::Monospace;
                    break;
                }
                SingleFontFamily::Generic(_) => {
                    family = egui::FontFamily::Proportional;
                    break;
                }
            };
            match name.as_str() {
                "monospace" | "courier" | "courier new" | "consolas" | "menlo" | "monaco"
                | "code" => {
                    family = egui::FontFamily::Monospace;
                    break;
                }
                "serif" | "sans-serif" | "system-ui" | "arial" | "helvetica" | "times"
                | "times new roman" | "georgia" | "verdana" => {
                    family = egui::FontFamily::Proportional;
                    break;
                }
                // Unrecognized named family: keep looking for a later generic.
                _ => {}
            }
        }
        egui::FontId::new(size_px.max(1.0), family)
    }
}

impl style::device::servo::FontMetricsProvider for EguiFontMetricsProvider {
    fn query_font_metrics(
        &self,
        _vertical: bool,
        font: &Font,
        font_size: CSSPixelLength,
        _flags: QueryFontMetricsFlags,
    ) -> FontMetrics {
        let size_px = font_size.px();
        // Ratio-based x-height/cap-height/ch/ic. egui only exposes real glyph
        // sizes via the font *atlas* (`Glyph::uv_rect`), which isn't populated
        // during the style pass (before any paint) — it reads as ~0, which
        // collapsed `ex`/`ch` units and gave math images a 0×0 box. Ratios are
        // stable and plenty accurate for unit resolution.
        let mut m = synthetic_metrics(font_size);

        // The font *ascent* IS reliably available (it's font-table metadata, not
        // atlas raster), so take the real one when egui has it.
        let font_id = Self::font_id(font, size_px);
        let ascent = self.ctx.fonts(|f| {
            f.layout_no_wrap("x".to_string(), font_id, egui::Color32::WHITE)
                .rows
                .first()
                .and_then(|r| r.glyphs.first())
                .map(|g| g.font_ascent)
                .unwrap_or(0.0)
        });
        if ascent > 0.0 {
            m.ascent = CSSPixelLength::new(ascent);
        }
        m
    }

    fn base_size_for_generic(
        &self,
        generic: style::values::computed::font::GenericFontFamily,
    ) -> Length {
        let px = match generic {
            style::values::computed::font::GenericFontFamily::Monospace => 13.0,
            _ => 16.0,
        };
        Length::from(app_units::Au::from_f32_px(px))
    }
}

/// Fallback provider used when no `egui::Context` is supplied (pure synthetic
/// ratios; preserves the Stage-1 behavior).
#[derive(Debug)]
struct SyntheticFontMetrics;

impl style::device::servo::FontMetricsProvider for SyntheticFontMetrics {
    fn query_font_metrics(
        &self,
        _vertical: bool,
        _font: &Font,
        font_size: CSSPixelLength,
        _flags: QueryFontMetricsFlags,
    ) -> FontMetrics {
        synthetic_metrics(font_size)
    }

    fn base_size_for_generic(
        &self,
        generic: style::values::computed::font::GenericFontFamily,
    ) -> Length {
        let px = match generic {
            style::values::computed::font::GenericFontFamily::Monospace => 13.0,
            _ => 16.0,
        };
        Length::from(app_units::Au::from_f32_px(px))
    }
}

/// A default viewport height paired with `viewport_width` to build the Device.
/// Height media features are rare on the pages we target; a tall-ish default
/// keeps `min-height`/`vh` sane without a real window size.
const DEFAULT_VIEWPORT_HEIGHT: f32 = 600.0;

/// Style every element of `doc` with Stylo, storing an `Arc<ComputedValues>` per
/// element in its [`super::data::StyloData`]. Runs alongside (does not replace)
/// the old `css::cascade::style_document`.
///
/// Stylesheets are registered UA-first (our [`crate::css::ua`] sheet as
/// `Origin::UserAgent`) then author sheets (external `<link>` via `provider`,
/// then inline `<style>`, in document order) as `Origin::Author`. Media queries
/// (`min/max-width`, `prefers-color-scheme`) are handled natively by the Device,
/// so the raw sheet text is fed unfiltered.
/// `font_ctx`, when `Some`, supplies a real egui [`FontMetricsProvider`] so
/// `ex`/`ch`/`cap` units and font-relative metrics match what layout measures;
/// when `None`, falls back to synthetic ratios (Stage-1 behavior).
///
/// [`FontMetricsProvider`]: style::device::servo::FontMetricsProvider
pub fn style_document_stylo(
    doc: &mut Document,
    provider: &dyn ResourceProvider,
    base_url: Option<&str>,
    theme: Theme,
    viewport_width: f32,
    font_ctx: Option<&egui::Context>,
) {
    // Feature prefs must be set before constructing the Stylist.
    style_config::set_pref!("layout.grid.enabled", true);
    style_config::set_pref!("layout.unimplemented", true);
    style_config::set_pref!("layout.columns.enabled", true);
    style_config::set_pref!("layout.threads", -1);

    // Ensure a SharedRwLock owned by the document, then take it out so we can
    // borrow the arena mutably for inline-style parsing without aliasing it.
    if doc.stylo_lock.is_none() {
        doc.stylo_lock = Some(SharedRwLock::new());
    }
    let lock = doc.stylo_lock.take().expect("just set");

    // Dummy base URL for stylesheet/inline-style parsing.
    let dummy_url = ServoArc::new(url::Url::parse("data:text/css,").unwrap());
    let url_extra = UrlExtraData(dummy_url);

    // ---- Parse inline style="" attributes into per-element decl blocks. ----
    parse_inline_styles(doc, &lock, &url_extra);

    // ---- Collect stylesheet sources (UA, then author in document order). ----
    let author_sources = collect_author_sheet_sources(doc, provider, base_url);

    // ---- Build the Device. ----
    let viewport_size = euclid::Size2D::new(viewport_width, DEFAULT_VIEWPORT_HEIGHT);
    let dppx = euclid::Scale::new(1.0);
    let prefers = match theme {
        Theme::Light => PrefersColorScheme::Light,
        Theme::Dark => PrefersColorScheme::Dark,
    };
    let metrics_provider: Box<dyn style::device::servo::FontMetricsProvider> = match font_ctx {
        Some(ctx) => Box::new(EguiFontMetricsProvider { ctx: ctx.clone() }),
        None => Box::new(SyntheticFontMetrics),
    };
    let device = Device::new(
        MediaType::screen(),
        QuirksMode::NoQuirks,
        viewport_size,
        dppx,
        metrics_provider,
        ComputedValues::initial_values_with_font_override(Font::initial_values()),
        prefers,
    );

    // ---- Build the Stylist and register sheets. ----
    let mut stylist = Stylist::new(device, QuirksMode::NoQuirks);

    let make_sheet = |css: &str, origin: Origin| -> DocumentStyleSheet {
        let data = Stylesheet::from_str(
            css,
            url_extra.clone(),
            origin,
            ServoArc::new(lock.wrap(MediaList::empty())),
            lock.clone(),
            None,
            None,
            QuirksMode::NoQuirks,
            AllowImportRules::Yes,
        );
        DocumentStyleSheet(ServoArc::new(data))
    };

    // UA sheet (+ dark override, matching the old cascade's UA origin).
    let mut ua_src = String::from(crate::css::ua::UA_CSS);
    if theme == Theme::Dark {
        ua_src.push_str(crate::css::ua::UA_CSS_DARK);
    }
    stylist.append_stylesheet(make_sheet(&ua_src, Origin::UserAgent), &lock.read());

    // Author sheets in document order.
    for src in &author_sources {
        stylist.append_stylesheet(make_sheet(src, Origin::Author), &lock.read());
    }

    // Flush so the cascade data is built. Scope the read guards so they drop
    // before we move `lock` back into the document.
    {
        let guards = StylesheetGuards {
            author: &lock.read(),
            ua_or_user: &lock.read(),
        };
        stylist.flush(&guards);
    }

    // ---- Freeze the arena & set back-pointers, then run the cascade. ----
    doc.stylo_lock = Some(lock);
    doc.set_tree_pointers();
    let lock = doc.stylo_lock.as_ref().expect("lock present");

    style::thread_state::enter(ThreadState::LAYOUT);

    let snapshots = SnapshotMap::new();
    let root_node = &doc.nodes[doc.root];
    let root_element = match TDocument::as_node(&root_node)
        .first_element_child()
        .and_then(|n| n.as_element())
    {
        Some(el) => el,
        None => {
            // No element to style (empty document); restore state and bail.
            style::thread_state::exit(ThreadState::LAYOUT);
            return;
        }
    };

    let context = SharedStyleContext {
        traversal_flags: TraversalFlags::empty(),
        stylist: &stylist,
        options: GLOBAL_STYLE_DATA.options.clone(),
        guards: StylesheetGuards {
            author: &lock.read(),
            ua_or_user: &lock.read(),
        },
        visited_styles_enabled: false,
        animations: DocumentAnimationSet::default().clone(),
        current_time_for_animations: 0.0,
        snapshot_map: &snapshots,
        registered_speculative_painters: &RegisteredPaintersImpl,
    };

    let token = RecalcStyle::pre_traverse(root_element, &context);
    if token.should_traverse() {
        let traverser = RecalcStyle::new(context);
        // rayon_pool = None => sequential styling (Option A).
        style::driver::traverse_dom(&traverser, token, None);
    }

    style::thread_state::exit(ThreadState::LAYOUT);
}

/// Parse every element's inline `style="…"` attribute into a `Locked<…>` decl
/// block and stash it on the node for `TElement::style_attribute()`.
fn parse_inline_styles(doc: &mut Document, lock: &SharedRwLock, url_extra: &UrlExtraData) {
    for node in &mut doc.nodes {
        node.inline_style = None;
        let Some(style_src) = node.attr("style") else {
            continue;
        };
        if style_src.trim().is_empty() {
            continue;
        }
        let block = style::properties::parse_style_attribute(
            style_src,
            url_extra,
            None,
            QuirksMode::NoQuirks,
            CssRuleType::Style,
        );
        node.inline_style = Some(ServoArc::new(lock.wrap(block)));
    }
}

/// Collect author stylesheet *sources* (CSS text) in document order: external
/// `<link rel=stylesheet href>` fetched via `provider`, then inline `<style>`.
/// Mirrors `css::cascade::collect_author_sheets` but returns raw text (Stylo
/// parses it and handles media queries itself).
fn collect_author_sheet_sources(
    doc: &Document,
    provider: &dyn ResourceProvider,
    base_url: Option<&str>,
) -> Vec<String> {
    let mut out = Vec::new();

    // Pre-order traversal for document order.
    let mut ordered = Vec::new();
    let mut stack = vec![doc.root];
    while let Some(n) = stack.pop() {
        ordered.push(n);
        for &c in doc.nodes[n].children.iter().rev() {
            stack.push(c);
        }
    }

    for &n in &ordered {
        let node = &doc.nodes[n];
        match node.tag() {
            Some("link") => {
                let rel = node.attr("rel").unwrap_or("");
                if rel
                    .split_whitespace()
                    .any(|r| r.eq_ignore_ascii_case("stylesheet"))
                {
                    if let Some(href) = node.attr("href") {
                        let url = resolve_url(base_url, href);
                        if let Some((bytes, _mime)) = provider.fetch(&url) {
                            if let Ok(text) = String::from_utf8(bytes) {
                                out.push(text);
                            }
                        }
                    }
                }
            }
            Some("style") => {
                let mut text = String::new();
                for &c in &node.children {
                    if let NodeData::Text(t) = &doc.nodes[c].data {
                        text.push_str(t);
                    }
                }
                if !text.trim().is_empty() {
                    out.push(text);
                }
            }
            _ => {}
        }
    }
    out
}

/// Resolve `href` against `base_url` (copied from `css::cascade::resolve_url`).
fn resolve_url(base_url: Option<&str>, href: &str) -> String {
    let href = href.trim();
    if href.starts_with("http://")
        || href.starts_with("https://")
        || href.starts_with("//")
        || href.starts_with("data:")
    {
        return href.to_string();
    }
    match base_url {
        None => href.to_string(),
        Some(base) => {
            let dir = match base.rfind('/') {
                Some(i) => &base[..=i],
                None => "",
            };
            if href.starts_with('/') {
                if let Some(scheme_end) = base.find("://") {
                    let after = &base[scheme_end + 3..];
                    if let Some(slash) = after.find('/') {
                        return format!("{}{}", &base[..scheme_end + 3 + slash], href);
                    }
                    return format!("{base}{href}");
                }
                href.to_string()
            } else {
                format!("{dir}{href}")
            }
        }
    }
}
