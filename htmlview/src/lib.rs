//! `hiker-htmlview` — a from-scratch HTML/CSS renderer that emits egui paint
//! primitives. The host owns scrolling, clipping, and input; this crate parses,
//! styles, lays out, and produces a display list.
//!
//! See `BUILD_PLAN.md` (the contract) and `references/ARCHITECTURE.md`.

#![allow(dead_code)]

pub mod css;
pub mod dom;
pub mod geom;
pub mod layout;
pub mod paint;

use std::collections::HashMap;
use std::sync::Arc;

use crate::dom::{Document, NodeData, NodeId};
use crate::layout::fonts::FontCtx;
use crate::layout::LayoutTree;
use crate::paint::{DisplayList, TextureMap};

// --- key re-exports for downstream consumers ---
pub use crate::css::computed::ComputedStyle;
pub use crate::dom::Node;
pub use crate::geom::Edges;
pub use crate::layout::ContentSizes;
pub use crate::paint::DisplayList as DisplayListPub;

/// Color scheme for `prefers-color-scheme` + UA defaults.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Theme {
    #[default]
    Light,
    Dark,
}

/// Synchronous, offline subresource resolver (CSS / images).
///
/// Implemented by the host (e.g. backed by a directory). Must be `Send + Sync`
/// so it can live in an `Arc` shared with the view.
pub trait ResourceProvider: Send + Sync {
    /// Resolve an absolute subresource URL to `(bytes, mime)`. `None` if missing.
    fn fetch(&self, url: &str) -> Option<(Vec<u8>, String)>;
}

/// A rendered HTML document: parsed DOM + computed style + layout/display-list
/// caches. May be `!Send` (it caches egui types). One per document/widget.
pub struct HtmlView {
    /// Raw HTML, retained so parsing/cascade can be (re)run lazily.
    html: String,
    base_url: Option<String>,
    provider: Arc<dyn ResourceProvider>,
    theme: Theme,
    zoom: f32,

    /// Parsed + styled DOM. `None` until parse/cascade runs.
    document: Option<Document>,
    /// Cached layout for `last_width`.
    layout_cache: Option<LayoutTree>,
    /// Cached display list built from `layout_cache`.
    display_list: Option<DisplayList>,
    /// Live texture handles for `<img>` nodes (kept alive so their ids stay valid).
    texture_handles: HashMap<NodeId, egui::TextureHandle>,
    /// Pre-rendered SVG documents for visible `<math>` elements, keyed by NodeId.
    /// Filled by [`Self::prerender_math`] after cascade, consumed by
    /// [`build_textures`] to rasterize each `<math>` replaced box.
    math_svgs: HashMap<NodeId, String>,
    /// Content width the cache was computed at; `None` invalidates the cache.
    last_width: Option<f32>,
    /// Cached content size for the current cache.
    content_size: egui::Vec2,
    /// (theme, zoom) the current caches were built with.
    cache_theme: Theme,
    cache_zoom: f32,
    /// Count of times the expensive layout pipeline actually ran (cache misses).
    /// Profiling aid: during pure scrolling this should not increase.
    layout_runs: usize,
    /// Viewport width (CSS px) the styled `document` was cascaded at, so width
    /// media features (`min-width`/`max-width`) can be re-evaluated when the
    /// width crosses a responsive breakpoint.
    cascade_width: Option<f32>,
}

impl HtmlView {
    /// Construct a view over `html`. Does not parse/lay out yet (deferred to the
    /// first `layout` call).
    pub fn new(html: &str, base_url: Option<&str>, provider: Arc<dyn ResourceProvider>) -> Self {
        HtmlView {
            html: html.to_owned(),
            base_url: base_url.map(|s| s.to_owned()),
            provider,
            theme: Theme::default(),
            zoom: 1.0,
            document: None,
            layout_cache: None,
            display_list: None,
            texture_handles: HashMap::new(),
            math_svgs: HashMap::new(),
            last_width: None,
            content_size: egui::Vec2::ZERO,
            cache_theme: Theme::default(),
            cache_zoom: 1.0,
            layout_runs: 0,
            cascade_width: None,
        }
    }

    /// Replace the document HTML. Invalidates all caches (incl. parse).
    pub fn set_html(&mut self, html: &str) {
        self.html = html.to_owned();
        self.document = None;
        self.invalidate_layout();
    }

    /// Set the color theme. Invalidates style/layout caches (re-cascade needed).
    pub fn set_theme(&mut self, theme: Theme) {
        if self.theme != theme {
            self.theme = theme;
            // Theme affects the cascade, so drop the styled document too.
            self.document = None;
            self.invalidate_layout();
        }
    }

    /// Set the zoom factor (multiplies CSS px). Invalidates layout caches.
    pub fn set_zoom(&mut self, zoom: f32) {
        if self.zoom != zoom {
            self.zoom = zoom;
            self.invalidate_layout();
        }
    }

    /// Lay out at content `width` (CSS px). Cached; cheap when inputs are
    /// unchanged. Returns the full content size.
    pub fn layout(&mut self, ctx: &egui::Context, width: f32) -> egui::Vec2 {
        // Cache hit: same width, theme and zoom, and we have a display list.
        if self.display_list.is_some()
            && self.last_width == Some(width)
            && self.cache_theme == self.theme
            && self.cache_zoom == self.zoom
        {
            return self.content_size;
        }

        // (1) Parse + cascade if we don't have a styled document, or if the width
        // crossed a responsive breakpoint (so width media features re-evaluate).
        let need_cascade = self.document.is_none()
            || self
                .cascade_width
                .map_or(true, |w| crosses_media_breakpoint(w, width));
        if need_cascade {
            let mut doc = match self.document.take() {
                // Re-cascade reuses the parsed tree only if HTML is unchanged; we
                // always reparse here because cascade mutates per-node style and a
                // clean parse is cheap relative to cascade. Keep it simple.
                _ => dom::parse_html(&self.html),
            };
            css::stylo::style_document_stylo(
                &mut doc,
                &*self.provider,
                self.base_url.as_deref(),
                self.theme,
                width,
                Some(ctx),
            );
            // Pre-render each visible `<math>` to an SVG once, stashing its
            // rendered px size on the element's `replaced_size` so the existing
            // replaced-box sizing picks it up (see `prerender_math`).
            Self::prerender_math(&mut doc, &mut self.math_svgs);
            self.document = Some(doc);
            self.cascade_width = Some(width);
        }
        let doc = self.document.as_ref().expect("document just set");

        // (2) Layout.
        let mut fonts = FontCtx::new(ctx.clone(), self.zoom);
        let (tree, content) = layout::layout_document(doc, &mut fonts, width, self.zoom);

        // (3) Build textures for <img> elements, then the display list.
        let textures = build_textures(
            ctx,
            doc,
            &tree,
            self.base_url.as_deref(),
            &*self.provider,
            &mut self.texture_handles,
            &self.math_svgs,
        );
        // Opaque page background covers at least the full layout width so the
        // host canvas never shows through (see `page_bg_color`).
        let bg_size = egui::vec2(content.x.max(width), content.y);
        let display_list =
            DisplayList::build(&tree, doc, &textures, page_bg_color(self.theme), bg_size);

        // (4) Cache.
        self.layout_runs += 1;
        self.layout_cache = Some(tree);
        self.display_list = Some(display_list);
        self.last_width = Some(width);
        self.content_size = content;
        self.cache_theme = self.theme;
        self.cache_zoom = self.zoom;

        content
    }

    /// Paint into the host painter. `origin` = document (0,0) in screen space;
    /// only shapes intersecting `clip_rect` are emitted (cheap viewport culling).
    pub fn paint(&self, painter: &egui::Painter, origin: egui::Pos2, clip_rect: egui::Rect) {
        if let Some(dl) = self.display_list.as_ref() {
            dl.paint_into(painter, origin.to_vec2(), clip_rect);
        }
    }

    /// Number of shapes in the built display list (0 before layout). Exposed for
    /// profiling: `paint()` currently iterates all of these every frame.
    pub fn shape_count(&self) -> usize {
        self.display_list.as_ref().map_or(0, |dl| dl.shapes.len())
    }

    /// How many times the expensive layout pipeline has run (cache misses).
    /// Should stay constant while only scrolling; if it climbs per frame, the
    /// `width` passed to [`Self::layout`] is jittering and thrashing the cache.
    pub fn layout_runs(&self) -> usize {
        self.layout_runs
    }

    /// The href of the link at `doc_point` (document coordinates), if any.
    pub fn link_at(&self, doc_point: egui::Pos2) -> Option<String> {
        self.display_list
            .as_ref()
            .and_then(|dl| dl.link_at(doc_point))
            .map(|s| s.to_owned())
    }

    /// Whether a link exists at `doc_point`.
    pub fn is_link_at(&self, doc_point: egui::Pos2) -> bool {
        self.link_at(doc_point).is_some()
    }

    /// Drop cached layout/paint state (but keep the parsed/styled document).
    fn invalidate_layout(&mut self) {
        self.layout_cache = None;
        self.display_list = None;
        self.texture_handles.clear();
        self.last_width = None;
        self.content_size = egui::Vec2::ZERO;
    }

    /// Pre-render every visible `<math>` element to an SVG once (after cascade,
    /// before layout). A `<math>`'s intrinsic size is its rendered math size, so
    /// we render it here and stamp that px size onto the element's computed style
    /// — the existing replaced-box path then sizes the box correctly without any
    /// layout-API change. The rendered SVG is stashed in `out` keyed by NodeId
    /// for [`build_textures`] to rasterize.
    ///
    /// LaTeX comes from the `alttext` attribute or, failing that, the text of a
    /// descendant `<annotation encoding="…x-tex">`. Math whose ancestor chain is
    /// invisible (`opacity <= 0`) is skipped — that's Wikipedia's hidden-MathML
    /// a11y case, where a fallback `<img>` is shown instead (handled by 7a).
    fn prerender_math(doc: &mut Document, out: &mut HashMap<NodeId, String>) {
        use crate::css::stylo::{primary_computed, read};

        out.clear();
        let ids: Vec<NodeId> = doc
            .nodes
            .iter()
            .filter(|n| n.tag() == Some("math"))
            .map(|n| n.id)
            .collect();

        for id in ids {
            // Skip math hidden behind an invisible ancestor UNLESS it's Wikipedia
            // math-element math with no `<img>` fallback (block display math) — that
            // we render and un-hide (`force_math_visible`) so it isn't lost. Inline
            // math (has a fallback `<img>`) and genuinely author-hidden bare math
            // (no math-element wrapper) stay skipped.
            if math_ancestor_invisible(doc, id) && !math_needs_resurrection(doc, id) {
                continue;
            }
            let Some(latex) = math_latex(doc, id) else {
                continue;
            };
            let cv = primary_computed(doc, id);
            let font_size_px = cv.as_ref().map(|cv| read::font_size(cv)).unwrap_or(16.0);
            let color = cv
                .as_ref()
                .map(|cv| read::color(cv).to_srgba_unmultiplied())
                .unwrap_or([0, 0, 0, 255]);
            // Display math when the element is block-level or carries display="block".
            let is_display = cv
                .as_ref()
                .map_or(false, |cv| read::display(cv) == crate::css::values::Display::Block)
                || doc.nodes[id].attr("display") == Some("block");
            let opts = hiker_math::MathOptions {
                font_size_px,
                color,
                style: if is_display {
                    hiker_math::MathStyle::Display
                } else {
                    hiker_math::MathStyle::Inline
                },
            };
            let Some(r) = hiker_math::render_latex(&latex, &opts) else {
                continue;
            };
            out.insert(id, r.svg);
            // Stamp the rendered (unzoomed) px size onto the node so the replaced
            // box sizes to the math; layout applies zoom later.
            doc.nodes[id].replaced_size = Some((r.width_px, r.height_px));
            // Wikipedia hides the MathML in a `display:none` a11y wrapper and shows
            // a sibling `<img>` fallback instead — EXCEPT for block display math,
            // which ships no `<img>` at all. When there's no fallback image, the
            // only copy of the formula is this hidden `<math>`, so un-hide its
            // wrapper chain to let it render (as block when it's display math).
            if math_needs_resurrection(doc, id) {
                force_math_visible(doc, id);
            }
        }
    }
}

/// Nearest ancestor of `<math>` `id` that is Wikipedia's visible `mwe-math-element`
/// wrapper, if any. Bare `<math>` outside that structure returns `None`.
fn math_wiki_container(doc: &Document, id: NodeId) -> Option<NodeId> {
    let mut cur = doc.nodes[id].parent;
    while let Some(p) = cur {
        if doc.nodes[p]
            .attr("class")
            .is_some_and(|c| c.contains("mwe-math-element"))
        {
            return Some(p);
        }
        cur = doc.nodes[p].parent;
    }
    None
}

/// Whether `<math>` `id` is Wikipedia math-element math whose container has NO
/// visible `<img>` fallback (`mwe-math-fallback-image-*`). That's block display
/// math in these dumps: its only copy is the hidden a11y MathML, so it must be
/// un-hidden and rendered ([`force_math_visible`]). Inline math (with an `<img>`)
/// and bare author-hidden `<math>` (no math-element wrapper) return `false`.
fn math_needs_resurrection(doc: &Document, id: NodeId) -> bool {
    let Some(container) = math_wiki_container(doc, id) else {
        return false;
    };
    // DFS the container subtree for a `mwe-math-fallback-image` `<img>`.
    let mut stack: Vec<NodeId> = doc.nodes[container].children.clone();
    while let Some(n) = stack.pop() {
        let node = &doc.nodes[n];
        if node.tag() == Some("img")
            && node
                .attr("class")
                .is_some_and(|c| c.contains("mwe-math-fallback-image"))
        {
            return false; // has a fallback image; no resurrection needed
        }
        stack.extend(node.children.iter().copied());
    }
    true
}

/// Un-hide the MathML a11y wrapper chain around `<math>` `id` so the math renders
/// in place. Wikipedia hides the wrapper with `opacity:0; position:absolute;
/// width:1px; height:1px`; we mark the `<math>` and every ancestor up to (and
/// including) the visible `mwe-math-element` container as [`force_visible`], which
/// resets opacity/position/size so the box lays out in normal flow. The `<math>`
/// is also forced `inline`, so it sizes via the atomic inline-replaced path (the
/// one that reads the pre-rendered math size). Block equations live in their own
/// `<p>`, so an inline-replaced box still sits alone on its line. Only called
/// when the container has no `<img>` fallback ([`math_has_img_fallback`]).
fn force_math_visible(doc: &mut Document, id: NodeId) {
    use crate::css::values::Display;

    // Walk up to the `mwe-math-element` container, marking each node visible and
    // forcing it `inline`. The a11y wrapper is `position:absolute`, which Stylo
    // blockifies to `display:block`; left as a block box nested in an inline
    // context it would never be sized (the inline-atomic sizer skips blocks).
    // Forcing the whole chain inline makes the `<math>` an atomic inline-replaced
    // box that sizes from its pre-rendered dimensions.
    let mut cur = Some(id);
    while let Some(n) = cur {
        doc.nodes[n].force_visible = true;
        doc.nodes[n].display_override = Some(Display::Inline);
        let is_container = doc.nodes[n]
            .attr("class")
            .is_some_and(|c| c.contains("mwe-math-element"));
        if is_container {
            break;
        }
        cur = doc.nodes[n].parent;
    }
}

/// Whether any ancestor of `id` has computed `opacity <= 0` (fully invisible).
/// Used to skip pre-rendering Wikipedia's hidden MathML (shown via an `<img>`).
fn math_ancestor_invisible(doc: &Document, id: NodeId) -> bool {
    use crate::css::stylo::{primary_computed, read};
    let mut cur = doc.nodes[id].parent;
    while let Some(p) = cur {
        if doc.nodes[p].is_element()
            && primary_computed(doc, p).map_or(false, |cv| read::opacity(&cv) <= 0.0)
        {
            return true;
        }
        cur = doc.nodes[p].parent;
    }
    false
}

/// Extract the LaTeX source for a `<math>` element: the `alttext` attribute if
/// present, else the text content of a descendant `<annotation>` whose
/// `encoding` attribute mentions `x-tex` (e.g. `application/x-tex`). Returns
/// `None` when neither is found or the text is blank.
fn math_latex(doc: &Document, id: NodeId) -> Option<String> {
    if let Some(alt) = doc.nodes[id].attr("alttext") {
        let alt = alt.trim();
        if !alt.is_empty() {
            return Some(alt.to_owned());
        }
    }
    // Depth-first search the subtree for an <annotation encoding="…x-tex">.
    let mut stack: Vec<NodeId> = doc.nodes[id].children.clone();
    while let Some(n) = stack.pop() {
        let node = &doc.nodes[n];
        if node.tag() == Some("annotation")
            && node.attr("encoding").is_some_and(|e| e.contains("x-tex"))
        {
            // Concatenate this annotation's direct text children.
            let mut tex = String::new();
            for &c in &node.children {
                if let NodeData::Text(t) = &doc.nodes[c].data {
                    tex.push_str(t);
                }
            }
            let tex = tex.trim();
            if !tex.is_empty() {
                return Some(tex.to_owned());
            }
        }
        stack.extend(node.children.iter().copied());
    }
    None
}

/// Standard responsive breakpoints (CSS px) used by the stylesheets we target
/// (Wikipedia/Vector/Minerva). A width change that stays between the same pair of
/// breakpoints cannot change any `min-width`/`max-width` media outcome, so we can
/// skip the (expensive) re-cascade. Crossing one means width media features may
/// flip, so we re-cascade. Kept slightly generous; extra entries only cost an
/// occasional redundant cascade, never correctness.
const MEDIA_BREAKPOINTS: &[f32] = &[320.0, 500.0, 640.0, 720.0, 1000.0, 1120.0, 1399.0, 1680.0];

/// Whether moving the viewport width from `old` to `new` crosses a breakpoint
/// (so a re-cascade is needed to re-evaluate width media features).
fn crosses_media_breakpoint(old: f32, new: f32) -> bool {
    if old == new {
        return false;
    }
    let (lo, hi) = if old < new { (old, new) } else { (new, old) };
    // A breakpoint b flips if it lies in the half-open span we moved across.
    // Use the same comparison semantics as the parser (>= for min, <= for max):
    // crossing exactly at b can change a `max-width:b`/`min-width:b` outcome.
    MEDIA_BREAKPOINTS.iter().any(|&b| lo < b && b <= hi || lo <= b && b < hi)
}

/// The opaque page background color for a theme. Light is a near-white page;
/// dark a deep slate. Kept in sync with the UA sheet's per-theme root background
/// (`css::ua`) so the base rect matches `<html>`/`<body>` bg.
pub fn page_bg_color(theme: Theme) -> egui::Color32 {
    match theme {
        Theme::Light => egui::Color32::from_rgb(0xff, 0xff, 0xff),
        Theme::Dark => egui::Color32::from_rgb(0x10, 0x14, 0x18),
    }
}

/// Decode `<img>` subresources into egui textures, returning a map of
/// `NodeId -> TextureId`. Handles are stored in `handles` so they stay alive.
///
/// Raster formats decode via the `image` crate; SVG (`image/svg+xml` or a
/// `.svg`/`<svg` payload) is rendered to a bitmap via [`render_svg`] at the
/// laid-out box size times the display density, so it stays crisp at any zoom.
/// Undecodable bytes are skipped (no texture) so paint draws a placeholder box.
/// Never panics on bad image bytes.
fn build_textures(
    ctx: &egui::Context,
    doc: &Document,
    tree: &LayoutTree,
    base_url: Option<&str>,
    provider: &dyn ResourceProvider,
    handles: &mut HashMap<NodeId, egui::TextureHandle>,
    math_svgs: &HashMap<NodeId, String>,
) -> TextureMap {
    handles.clear();
    let mut map = TextureMap::new();

    // Only <img> elements (and pre-rendered <math>) need decoding. Bail early
    // when the page has neither — a cheap scan, no hashing — so we don't touch
    // the box tree at all.
    // Process if the page has ANY <img> (even a src-less math fallback we render
    // from its `alt`) or any pre-rendered <math>. Bail only when there's nothing.
    let has_img = doc.nodes.iter().any(|n| n.tag() == Some("img"));
    if !has_img && math_svgs.is_empty() {
        return map;
    }

    let ppp = ctx.pixels_per_point().max(1.0);

    // node -> laid-out border-box size (document px), for sizing SVG rasters.
    // A `NodeId` is an index into `doc.nodes`, so we tag-check each box by direct
    // array access (no hashing over all boxes) and only insert the <img> ones —
    // the map ends up with one entry per image, not one per box.
    let mut box_size: HashMap<NodeId, egui::Vec2> = HashMap::new();
    for b in &tree.boxes {
        if let Some(n) = b.node {
            if matches!(doc.nodes[n].tag(), Some("img") | Some("math")) {
                box_size.entry(n).or_insert_with(|| b.rect.size());
            }
        }
    }

    for node in &doc.nodes {
        let NodeData::Element { name, .. } = &node.data else {
            continue;
        };

        // <math>: rasterize the SVG we pre-rendered (after cascade) to the
        // laid-out box size. Sized like an <img> SVG; painted via the same
        // texture map keyed by NodeId.
        if name == "math" {
            let Some(svg) = math_svgs.get(&node.id) else { continue };
            let target = box_size.get(&node.id).copied().unwrap_or(egui::Vec2::ZERO);
            let Some(color_image) = render_svg(svg.as_bytes(), target, ppp) else { continue };
            let tex_name = format!("htmlview-math-{}", node.id);
            let handle = ctx.load_texture(tex_name, color_image, egui::TextureOptions::default());
            map.insert(node.id, handle.id());
            handles.insert(node.id, handle);
            continue;
        }

        if name != "img" {
            continue;
        }
        // Display size (box px) × density, for crisp SVG/math rasters; falls back
        // to intrinsic size when the box is degenerate.
        let target = box_size.get(&node.id).copied().unwrap_or(egui::Vec2::ZERO);

        // Wikipedia math fallback images (`<img class="mwe-math-fallback-image-*"
        // alt="{\displaystyle …}">`): render the LaTeX with OUR engine (hiker-math)
        // and never touch the pre-rendered SVG — it's the *fallback*, and offline
        // it's usually absent anyway. Everything else fetches the real subresource,
        // with math-alt only as a last resort (gated so plain images aren't
        // mistaken for math).
        let is_math_img = node
            .attr("class")
            .is_some_and(|c| c.contains("mwe-math-fallback-image"));

        let color_image = if is_math_img {
            render_math_alt(node, target, ppp, false)
                .or_else(|| fetch_image(provider, base_url, node, target, ppp))
        } else {
            fetch_image(provider, base_url, node, target, ppp)
                .or_else(|| render_math_alt(node, target, ppp, true))
        };

        let Some(color_image) = color_image else { continue };

        let tex_name = format!("htmlview-img-{}", node.id);
        let handle = ctx.load_texture(tex_name, color_image, egui::TextureOptions::default());
        map.insert(node.id, handle.id());
        handles.insert(node.id, handle);
    }

    map
}

/// Fetch an `<img>`'s real subresource via the provider and decode it: SVG via
/// resvg (rasterized to `target` × density), other formats via the `image`
/// crate. `None` when there's no `src`, the host lacks the bytes, or decode
/// fails (the caller then falls back to math-alt rendering / a placeholder).
fn fetch_image(
    provider: &dyn ResourceProvider,
    base_url: Option<&str>,
    node: &Node,
    target: egui::Vec2,
    ppp: f32,
) -> Option<egui::ColorImage> {
    let src = node.attr("src")?;
    if src.trim().is_empty() {
        return None;
    }
    let url = paint::resolve_url(base_url, src);
    let (bytes, mime) = provider.fetch(&url)?;
    let is_svg = mime.contains("svg")
        || url.split('?').next().unwrap_or(&url).to_ascii_lowercase().ends_with(".svg")
        || looks_like_svg(&bytes);
    if is_svg {
        render_svg(&bytes, target, ppp)
    } else {
        // Decode defensively; never panic on bad bytes.
        image::load_from_memory(&bytes).ok().and_then(|img| {
            let (w, h) = (img.width(), img.height());
            (w != 0 && h != 0).then(|| {
                egui::ColorImage::from_rgba_unmultiplied(
                    [w as usize, h as usize],
                    img.to_rgba8().as_raw(),
                )
            })
        })
    }
}

/// Render an `<img>`'s LaTeX `alt` as math via `hiker-math`, rasterized to the
/// image's box (à la [`render_svg`]). The primary renderer for Wikipedia's math
/// fallback `<img class="mwe-math-fallback-image-*" alt="{\displaystyle …}">`.
///
/// When `gated`, the `alt` must *look* like TeX ([`looks_like_latex`]) to be
/// rendered — used for non-math images so a plain `alt` is never mistaken for
/// math. For a known math image (`gated == false`) the `alt` is rendered
/// unconditionally, so even brace-free expressions like `x^2` work. Returns
/// `None` when there's no `alt` (or it's gated-out, or doesn't render). Font
/// size/color come from the element's computed style, so math tracks the
/// surrounding text (incl. dark theme).
fn render_math_alt(
    node: &Node,
    target: egui::Vec2,
    ppp: f32,
    gated: bool,
) -> Option<egui::ColorImage> {
    let alt = node.attr("alt")?;
    if alt.trim().is_empty() || (gated && !looks_like_latex(alt)) {
        return None;
    }
    use crate::css::stylo::read;
    use style::properties::ComputedValues;
    // `<img>` is an element, so it carries its own primary `ComputedValues`.
    let styles = node.stylo_element_data.primary_styles();
    let cv: Option<&ComputedValues> = styles.as_deref().map(|arc| &**arc);
    let font_size_px = cv.map(read::font_size).unwrap_or(16.0);
    let color = cv
        .map(|cv| read::color(cv).to_srgba_unmultiplied())
        .unwrap_or([0, 0, 0, 255]);
    let display = node
        .attr("class")
        .is_some_and(|c| c.contains("fallback-image-display"))
        || node.attr("display") == Some("block");
    let opts = hiker_math::MathOptions {
        font_size_px,
        color,
        style: if display {
            hiker_math::MathStyle::Display
        } else {
            hiker_math::MathStyle::Inline
        },
    };
    let render = hiker_math::render_latex(alt, &opts)?;
    render_svg(render.svg.as_bytes(), target, ppp)
}

/// Heuristic: does this `alt`/string look like a TeX math expression? Backslash
/// commands or `{}` grouping are strong signals that don't appear in ordinary
/// alt text. (Only consulted when an image's real bitmap is missing, so the
/// false-positive cost is nil.)
fn looks_like_latex(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty() && (s.contains('\\') || s.contains('{'))
}

/// Process-wide system-font database for SVG text, loaded once on first use.
fn svg_fontdb() -> std::sync::Arc<resvg::usvg::fontdb::Database> {
    use std::sync::OnceLock;
    static DB: OnceLock<std::sync::Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = resvg::usvg::fontdb::Database::new();
        db.load_system_fonts();
        std::sync::Arc::new(db)
    })
    .clone()
}

/// Cheap sniff for an SVG payload (XML prolog or a `<svg` tag near the start).
fn looks_like_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(512)];
    let s = String::from_utf8_lossy(head);
    let s = s.trim_start();
    s.starts_with("<?xml") && s.contains("<svg") || s.starts_with("<svg")
}

/// Render SVG `bytes` to an egui `ColorImage`. The raster size is the laid-out
/// box `display_size` (document px) × `ppp` (device pixels per point); when the
/// box size is unknown/degenerate we fall back to the SVG's intrinsic size. The
/// SVG is scaled (preserving its own aspect via independent x/y scales to fill
/// the box, matching how `<img>` stretches a raster). Returns `None` on parse
/// failure or a degenerate size. Pure-Rust (usvg/resvg/tiny-skia); no network.
fn render_svg(bytes: &[u8], display_size: egui::Vec2, ppp: f32) -> Option<egui::ColorImage> {
    let mut opt = resvg::usvg::Options::default();
    // Share a process-wide system-font database so `<text>` in SVGs (diagrams,
    // labels) renders. Path-based SVGs (Wikipedia math, most icons) don't need
    // it. Loaded once; empty if the platform has no fonts (text just skips).
    opt.fontdb = svg_fontdb();
    let rtree = resvg::usvg::Tree::from_data(bytes, &opt).ok()?;
    let intrinsic = rtree.size();
    let (iw, ih) = (intrinsic.width(), intrinsic.height());
    if iw <= 0.0 || ih <= 0.0 {
        return None;
    }

    // Target raster size in physical pixels. Prefer the laid-out box; fall back
    // to intrinsic. Cap to keep textures bounded.
    const MAX_DIM: f32 = 4096.0;
    let (tw_pt, th_pt) = if display_size.x >= 1.0 && display_size.y >= 1.0 {
        (display_size.x, display_size.y)
    } else {
        (iw, ih)
    };
    let tw = ((tw_pt * ppp).round()).clamp(1.0, MAX_DIM);
    let th = ((th_pt * ppp).round()).clamp(1.0, MAX_DIM);

    let mut pixmap = resvg::tiny_skia::Pixmap::new(tw as u32, th as u32)?;
    let transform = resvg::tiny_skia::Transform::from_scale(tw / iw, th / ih);
    resvg::render(&rtree, transform, &mut pixmap.as_mut());

    // tiny-skia stores premultiplied RGBA; egui's Color32 is premultiplied too.
    let pixels: Vec<egui::Color32> = pixmap
        .pixels()
        .iter()
        .map(|p| egui::Color32::from_rgba_premultiplied(p.red(), p.green(), p.blue(), p.alpha()))
        .collect();
    Some(egui::ColorImage {
        size: [tw as usize, th as usize],
        pixels,
        source_size: egui::vec2(tw, th),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct DirProvider {
        root: PathBuf,
    }

    impl ResourceProvider for DirProvider {
        fn fetch(&self, url: &str) -> Option<(Vec<u8>, String)> {
            let rel = url.trim_start_matches("./").trim_start_matches('/');
            let path = self.root.join(rel);
            let bytes = std::fs::read(&path).ok()?;
            let mime = if rel.ends_with(".css") {
                "text/css".to_string()
            } else if rel.ends_with(".svg") {
                "image/svg+xml".to_string()
            } else if rel.ends_with(".png") {
                "image/png".to_string()
            } else if rel.ends_with(".jpg") || rel.ends_with(".jpeg") {
                "image/jpeg".to_string()
            } else {
                "application/octet-stream".to_string()
            };
            Some((bytes, mime))
        }
    }

    /// Headless egui context with fonts primed (matches the layout tests).
    fn headless_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(egui::FontDefinitions::default());
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        ctx
    }

    #[test]
    fn end_to_end_wiki_article() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/wiki-sample/article.html");
        let html = std::fs::read_to_string(path).expect("read article.html");

        let provider = Arc::new(DirProvider {
            root: PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/wiki-sample")),
        });

        let ctx = headless_ctx();
        let mut view = HtmlView::new(&html, Some("./"), provider);

        let size = view.layout(&ctx, 800.0);
        eprintln!("article.html content_size = {size:?}");
        assert!(
            size.y > 1000.0,
            "expected content height > 1000, got {}",
            size.y
        );

        let dl = view.display_list.as_ref().expect("display list built");
        eprintln!(
            "article.html display list: {} shapes, {} links",
            dl.shapes.len(),
            dl.links.len()
        );
        assert!(
            dl.shapes.len() > 100,
            "expected a non-trivial number of shapes, got {}",
            dl.shapes.len()
        );

        // Exercise paint into a throwaway headless painter.
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Background,
            egui::Id::new("htmlview-test"),
        ));
        view.paint(
            &painter,
            egui::Pos2::ZERO,
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0)),
        );

        // Cache hit: identical inputs return the same size cheaply.
        let again = view.layout(&ctx, 800.0);
        assert_eq!(size, again, "layout should be cached for identical inputs");
    }

    #[test]
    fn render_svg_rasterizes_to_requested_size_and_color() {
        // A 10×10 SVG fully filled red. Request a 40×20 raster (box-sized) at 2×
        // density -> expect an 80×40 bitmap of opaque red.
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" viewBox="0 0 10 10"><rect x="0" y="0" width="10" height="10" fill="rgb(255,0,0)"/></svg>"#;
        let ci = render_svg(svg, egui::vec2(40.0, 20.0), 2.0).expect("svg renders");
        assert_eq!(ci.size, [80, 40], "raster size = box × density");
        // Center pixel should be opaque red.
        let center = ci.pixels[(20 * 80) + 40];
        assert_eq!(center.a(), 255, "filled region is opaque");
        assert!(center.r() > 200 && center.g() < 40 && center.b() < 40, "red, got {center:?}");
    }

    #[test]
    fn render_svg_falls_back_to_intrinsic_size() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="12" viewBox="0 0 16 12"><rect width="16" height="12" fill="black"/></svg>"#;
        // Degenerate box -> intrinsic size × density.
        let ci = render_svg(svg, egui::Vec2::ZERO, 1.0).expect("svg renders");
        assert_eq!(ci.size, [16, 12]);
    }

    #[test]
    fn looks_like_svg_sniffs_payloads() {
        assert!(looks_like_svg(b"<svg xmlns=...>"));
        assert!(looks_like_svg(b"<?xml version=\"1.0\"?><svg>"));
        assert!(!looks_like_svg(b"\x89PNG\r\n"));
    }

    #[test]
    fn img_svg_src_produces_a_texture() {
        // An <img> pointing at an SVG should yield a texture (not a placeholder).
        let html = r#"<p><img src="dot.svg" width="20" height="20"></p>"#;
        let dir = tempdir_with_svg();
        let provider = Arc::new(DirProvider { root: dir.clone() });
        let ctx = headless_ctx();
        let mut view = HtmlView::new(html, Some("./"), provider);
        let _ = view.layout(&ctx, 800.0);

        // Find the img node id and confirm a texture was registered for it.
        let doc = view.document.as_ref().unwrap();
        let img = doc
            .nodes
            .iter()
            .find(|n| n.tag() == Some("img"))
            .expect("img node");
        assert!(
            view.texture_handles.contains_key(&img.id),
            "expected an SVG-backed texture for the <img>"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A provider that resolves nothing — for documents with no subresources.
    struct NullProvider;
    impl ResourceProvider for NullProvider {
        fn fetch(&self, _url: &str) -> Option<(Vec<u8>, String)> {
            None
        }
    }

    #[test]
    fn math_element_produces_a_texture_and_sized_box() {
        // A visible <math alttext="…"> is rendered as a replaced element sized to
        // the math, with a texture registered for its node.
        let html = r#"<p><math alttext="x^2 + 1"></math></p>"#;
        let ctx = headless_ctx();
        let mut view = HtmlView::new(html, None, Arc::new(NullProvider));
        let _ = view.layout(&ctx, 800.0);

        let doc = view.document.as_ref().unwrap();
        let math = doc
            .nodes
            .iter()
            .find(|n| n.tag() == Some("math"))
            .expect("math node");

        // Eyeball: dump the rendered SVG (ignore write errors).
        if let Some(svg) = view.math_svgs.get(&math.id) {
            let path = concat!(env!("CARGO_MANIFEST_DIR"), "/target/math-element-7b.svg");
            let _ = std::fs::write(path, svg);
            eprintln!("math element eyeball SVG: {path}");
        }

        assert!(
            view.texture_handles.contains_key(&math.id),
            "expected a texture for the <math> element"
        );

        // The replaced box for the math node must be sized > 0.
        let tree = view.layout_cache.as_ref().expect("layout cache");
        let mbox = tree
            .boxes
            .iter()
            .find(|b| b.node == Some(math.id))
            .expect("math box");
        assert!(
            mbox.rect.width() > 0.0 && mbox.rect.height() > 0.0,
            "math replaced box should be sized > 0, got {:?}",
            mbox.rect.size()
        );
    }

    #[test]
    fn hidden_math_gets_no_texture() {
        // A <math> under an opacity:0 ancestor (Wikipedia's a11y MathML) is
        // skipped by the pre-render pass and gets no texture.
        let html =
            r#"<p style="opacity:0"><math alttext="x^2 + 1"></math></p>"#;
        let ctx = headless_ctx();
        let mut view = HtmlView::new(html, None, Arc::new(NullProvider));
        let _ = view.layout(&ctx, 800.0);

        let doc = view.document.as_ref().unwrap();
        let math = doc
            .nodes
            .iter()
            .find(|n| n.tag() == Some("math"))
            .expect("math node");
        assert!(
            !view.texture_handles.contains_key(&math.id),
            "hidden <math> should not get a texture"
        );
        assert!(
            !view.math_svgs.contains_key(&math.id),
            "hidden <math> should not be pre-rendered"
        );
    }

    #[test]
    fn block_math_without_img_fallback_renders() {
        // Wikipedia block display math ships ONLY a hidden MathML a11y wrapper
        // (`opacity:0; position:absolute; width:1px; height:1px`) and NO `<img>`
        // fallback. The hidden `<math>` must be un-hidden and rendered in flow as
        // a sized replaced box (the empty-gap bug).
        let html = r#"<p><span class="mwe-math-element mwe-math-element-block">
            <span class="mwe-math-mathml-display mwe-math-mathml-a11y" style="opacity:0;position:absolute;width:1px;height:1px;overflow:hidden">
              <math display="block" alttext="{\displaystyle x^2+1}"></math>
            </span>
          </span></p>"#;
        let ctx = headless_ctx();
        let mut view = HtmlView::new(html, None, Arc::new(NullProvider));
        let _ = view.layout(&ctx, 800.0);

        let doc = view.document.as_ref().unwrap();
        let math = doc
            .nodes
            .iter()
            .find(|n| n.tag() == Some("math"))
            .expect("math node");
        assert!(
            view.texture_handles.contains_key(&math.id),
            "block <math> with no <img> fallback must render a texture"
        );
        assert!(
            math.force_visible,
            "no-fallback block math must be force-shown"
        );
        // The un-hidden math must be a sized replaced box.
        let tree = view.layout_cache.as_ref().expect("layout cache");
        let mbox = tree
            .boxes
            .iter()
            .find(|b| b.node == Some(math.id))
            .expect("math box exists (no longer hidden)");
        assert!(
            mbox.rect.width() > 0.0 && mbox.rect.height() > 0.0,
            "block math box should be sized > 0, got {:?}",
            mbox.rect.size()
        );
    }

    #[test]
    fn inline_math_with_img_fallback_is_not_double_rendered() {
        // When an `<img>` fallback exists (Wikipedia inline math), the hidden
        // MathML must STAY hidden — the img renders; un-hiding it would double up.
        let html = r#"<p><span class="mwe-math-element mwe-math-element-inline">
            <span class="mwe-math-mathml-inline mwe-math-mathml-a11y" style="opacity:0;position:absolute;width:1px;height:1px;overflow:hidden">
              <math alttext="{\displaystyle v}"></math>
            </span>
            <img class="mwe-math-fallback-image-inline" alt="{\displaystyle v}" width="10" height="12">
          </span></p>"#;
        let ctx = headless_ctx();
        let mut view = HtmlView::new(html, None, Arc::new(NullProvider));
        let _ = view.layout(&ctx, 800.0);

        let doc = view.document.as_ref().unwrap();
        let math = doc.nodes.iter().find(|n| n.tag() == Some("math")).unwrap();
        // The img-fallback math must NOT be force-shown (the img renders instead).
        assert!(
            !math.force_visible && math.display_override.is_none(),
            "math with an <img> fallback must stay hidden, not be force-shown"
        );
        // And the visible `<img>` fallback must itself get a texture.
        let img = doc.nodes.iter().find(|n| n.tag() == Some("img")).unwrap();
        assert!(
            view.texture_handles.contains_key(&img.id),
            "the <img> fallback should render via hiker-math"
        );
    }

    #[test]
    fn math_fallback_img_renders_via_our_engine() {
        // Wikipedia's *visible* math is an `<img class="mwe-math-fallback-image-*"
        // alt="…tex…">` whose SVG is typically absent offline. It must render via
        // hiker-math (a texture), not show a placeholder — and even brace-free
        // alts like `x^2` must work (the gated heuristic must NOT reject a known
        // math image). NullProvider => no fetchable src, so only our engine can
        // produce the texture.
        let html = r#"<p><img class="mwe-math-fallback-image-inline" alt="x^2" width="40" height="16"></p>"#;
        let ctx = headless_ctx();
        let mut view = HtmlView::new(html, None, Arc::new(NullProvider));
        let _ = view.layout(&ctx, 800.0);

        let doc = view.document.as_ref().unwrap();
        let img = doc
            .nodes
            .iter()
            .find(|n| n.tag() == Some("img"))
            .expect("img node");
        assert!(
            view.texture_handles.contains_key(&img.id),
            "math-fallback <img> must be rendered by hiker-math, not a placeholder"
        );
    }

    /// Write a tiny SVG into a unique temp dir and return the dir path.
    fn tempdir_with_svg() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hiker-svg-test-{}",
            concat!(env!("CARGO_PKG_NAME"), "-dot")
        ));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(
            dir.join("dot.svg"),
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><circle cx="10" cy="10" r="9" fill="blue"/></svg>"#,
        )
        .unwrap();
        dir
    }
}
