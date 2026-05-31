# hiker-htmlview — BUILD CONTRACT (read with references/ARCHITECTURE.md)

This is the **fixed contract** every implementation agent builds against. Do not change the
public API or the core shared type *signatures* without a very good reason; if you must, note
it loudly in your summary. The design rationale is in `references/ARCHITECTURE.md`.

## Targets & invariants
- Pure Rust, single library crate `hiker-htmlview` + `examples/viewer.rs` (eframe demo).
- **egui / eframe / epaint = 0.32** (resolved 0.32.3 — matches the host at `/home/bobby/projects/notes`).
  The compiler is the source of truth for egui API; run `cargo check`/`cargo build` and fix
  against errors. Do NOT guess egui APIs — verify by compiling.
- No JS, no network, no threads, no GPU device, no global/thread-local state. `HtmlView` may be !Send.
- Renderer emits egui paint primitives; the **host owns scrolling/clip/input**.
- Hand-rolled layout (NO taffy, NO blitz). Text measured via egui `Fonts`.

## Crate deps (let cargo resolve exact patch versions; fix conflicts)
- `egui = "0.32"`, `eframe = "0.32"` (dev/example only is fine, but egui is a normal dep).
- `html5ever = "0.38"` + `markup5ever = "0.38"` (implement our own TreeSink; do NOT use rcdom).
- `cssparser = "0.35"` (or whatever resolves cleanly; we own property interpretation).
- `image = "0.25"` for PNG/JPEG decode (feature-gate if convenient).
- SVG: defer — emit a placeholder box for `image/svg+xml` in v1 (samples use .svg heavily).

## Module layout (one concern per file; keep files focused)
```
src/lib.rs            public API (below) + re-exports; HtmlView orchestration
src/geom.rs           geometry helpers (mostly egui::{Vec2,Pos2,Rect}; Edges, etc.)
src/dom.rs            arena DOM + html5ever TreeSink
src/css/mod.rs
src/css/values.rs     CSS value enums (Display, Length, Color, FontStyle, ...)
src/css/computed.rs   ComputedStyle (resolved, owned)
src/css/selector.rs   selector types + matching + specificity
src/css/parser.rs     stylesheet + declaration parsing (cssparser)
src/css/cascade.rs    cascade: match rules → ComputedStyle per node (inheritance)
src/css/ua.rs         UA stylesheet (const &str, ported from litehtml master.css)
src/layout/mod.rs     layout entry; LayoutTree; ContentSizes; LayoutBox
src/layout/fonts.rs   FontCtx: map ComputedStyle font -> egui FontId + metrics; measure cache
src/layout/block.rs   block formatting context (vertical stacking, margin collapse)
src/layout/inline.rs  inline formatting context (line boxes; uses fonts.rs to measure)
src/layout/table.rs   table layout (grid, colspan/rowspan, auto widths, border-collapse)
src/layout/float.rs   float manager (bands)
src/paint.rs          walk laid-out tree -> Vec<egui::Shape> (paint order) + link rects
```

## PUBLIC API (keep this shape exactly — from the spec)
```rust
pub struct HtmlView { /* parsed DOM + computed style + layout cache */ }

impl HtmlView {
    pub fn new(html: &str, base_url: Option<&str>, provider: std::sync::Arc<dyn ResourceProvider>) -> Self;
    pub fn set_html(&mut self, html: &str);
    pub fn set_theme(&mut self, theme: Theme);
    pub fn set_zoom(&mut self, zoom: f32);
    /// Lay out at content width (CSS px). Cached; cheap if inputs unchanged. Returns full content size.
    pub fn layout(&mut self, ctx: &egui::Context, width: f32) -> egui::Vec2;
    /// Paint into host painter. `origin` = document (0,0) in screen space (scroll-adjusted top-left).
    /// Only paint shapes intersecting `clip_rect`.
    pub fn paint(&self, painter: &egui::Painter, origin: egui::Pos2, clip_rect: egui::Rect);
    pub fn link_at(&self, doc_point: egui::Pos2) -> Option<String>;
    pub fn is_link_at(&self, doc_point: egui::Pos2) -> bool;
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Theme { Light, Dark }

pub trait ResourceProvider: Send + Sync {
    /// Synchronous, offline. Resolve an absolute subresource URL (CSS/image) to (bytes, mime).
    fn fetch(&self, url: &str) -> Option<(Vec<u8>, String)>;
}
```
NOTE vs spec: `layout` takes `&egui::Context` because text measurement needs `ctx.fonts(...)`.
This is the one allowed deviation. Everything else matches the spec verbatim.

## Core shared types (the contract the modules agree on)
```rust
// dom.rs
pub type NodeId = usize; // index into Document.nodes (arena)
pub struct Document { pub nodes: Vec<Node>, pub root: NodeId } // root = the Document node
pub struct Node {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub data: NodeData,
    pub style: Option<css::computed::ComputedStyle>, // filled by cascade
}
pub enum NodeData {
    Document,
    Element { name: String, attrs: Vec<(String, String)> }, // name lowercased; namespace ignored
    Text(String),
    Comment(String),
    Doctype,
}
impl Node {
    pub fn attr(&self, name: &str) -> Option<&str>;
    pub fn tag(&self) -> Option<&str>;
    pub fn classes(&self) -> impl Iterator<Item = &str>; // split class attr on whitespace
    pub fn id_attr(&self) -> Option<&str>;
}
```
`ComputedStyle` (css/computed.rs): an owned, fully-resolved style. Include at least: display
(block/inline/inline-block/list-item/none/table/table-row/table-cell/table-row-group/flex),
position (static/relative), float (none/left/right), clear, box-sizing; margin/padding/border
widths (Edges<f32> after px resolution — but keep `auto` for margins), border colors+styles,
width/height/min/max as a `LengthOrAuto`/`LengthOrPercent`, color, background-color, font
(family list, size px, weight, style, line-height), text-align, text-decoration (underline),
white-space, list-style-type, vertical-align. Inheritance handled in cascade.

Geometry: prefer `egui::Vec2/Pos2/Rect`. `Edges { top,right,bottom,left: f32 }` helper in geom.rs.

`ContentSizes { min_content: f32, max_content: f32 }` (layout/mod.rs) with `max`/`union`.

## Cascade origins/order (low→high)
UA sheet → external `<link>` (provider) → `<style>` blocks → inline `style=`; `!important`
lifts above normal; ties broken by specificity then source order. `prefers-color-scheme`
selected by `Theme`. Resolve inheritance into owned ComputedStyle.

## Layout contract
- `layout(node, containing_block) -> size`; pass available inline width down as a parameter
  (don't store on nodes). Each FC returns used size; intrinsic pass returns ContentSizes.
- FormattingContext = enum { Block, Inline, Table, Replaced } (+ Flex later). A block container
  holds EITHER block children OR one inline context (anon-box fixup enforces this).
- Floats via `Vec<FloatBand>` sorted by top; lines query left/right edges at a y.
- Tables: `Vec<Vec<TableSlot>>` grid; min/max-content column sizing; see ARCHITECTURE.md §6.

## Paint contract
- `paint.rs` walks the laid-out tree and pushes `egui::Shape`s in paint order
  (bg/border → floats → in-flow inline/block → relative-positioned). Text via `Shape::galley`
  using the SAME galleys produced/measured during inline layout (cache them on the fragment).
  Collect `(egui::Rect, href)` for `link_at`. Skip shapes not intersecting `clip_rect`.

## Test corpus
`wiki-sample/article.html` = the "Water" Wikipedia article (~650KB) + `style-0.css`..`style-11.css`
(relative `<link href>`), plus many inline `<style>` blocks and `.svg` images under
`./_assets_/...`. Back the demo's `ResourceProvider` with the `wiki-sample/` dir (resolve
relative URLs against it; return None for anything missing → render placeholder). Images may be
absent/SVG → placeholder box is acceptable for v1. The user verifies output visually.

## Working agreement for agents
1. Read `BUILD_PLAN.md` (this) + `references/ARCHITECTURE.md` first.
2. Implement ONLY your assigned module(s). Don't rewrite others' files unless fixing a
   compile error you caused.
3. **Leave the whole crate `cargo check`-clean** (and `cargo build` if you touch the example).
   Fix all errors/warnings you introduce. Run the commands; paste the final clean status.
4. Keep files focused; match Rust idiom; comment density like surrounding code.
5. Report a TIGHT summary (what you built, key decisions, any contract deviations, test status).
   Do NOT dump full file contents back to the orchestrator.
```
