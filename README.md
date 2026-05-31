# hiker-render

An umbrella workspace of **reusable, mostly egui-agnostic renderers** for static, offline
content. The flagship is **`hiker-htmlview`**, a from-scratch HTML/CSS renderer that emits egui
paint primitives — "**Blitz, but for egui**": Servo's real CSS engine, our own layout, egui
paint, and the host owns scrolling. Alongside it live standalone typesetting/diagram crates
(LaTeX math, graph layout, mermaid) that the renderer composes but that have no egui dependency
of their own.

## Workspace layout

The crates live under `hiker-render/`:

| crate | path | what it is |
|-------|------|------------|
| **`hiker-htmlview`** | `hiker-render/htmlview` | HTML/CSS renderer → egui paint primitives. Servo **Stylo** cascade + our block/inline/float/table layout + egui paint. Host owns scroll/clip/input. |
| **`hiker-math`** | `hiker-render/math` | egui-agnostic LaTeX math typesetting: `pulldown-latex` parse + Appendix-G / MathML-Core layout over an OpenType **MATH** font → **SVG** + metrics. |
| **`hiker-graph`** | `hiker-render/graph` | egui-agnostic graph layout: tree/radial + ForceAtlas2, and a from-scratch **dagre** (Sugiyama) port. std-only, no graphics deps. |
| **`hiker-mermaid`** | `hiker-render/mermaid` | Pure-Rust mermaid diagram renderer (flowcharts first) → **SVG**; layout via `hiker-graph`. |

These are deliberately decoupled: the math/graph/mermaid crates take a source string + options and
return an **SVG string** (plus layout metrics). The host — `hiker-htmlview`, or any other app —
rasterizes that SVG with whatever backend it already has (htmlview uses `resvg` → egui texture).
This keeps the typesetting crates free of any graphics dependency.

> The `hiker-render` root package is a legacy copy of the math engine being consolidated into
> `hiker-render/math` (`hiker-math`); nothing depends on it.

---

# hiker-htmlview

A from-scratch HTML/CSS renderer for **static, offline** pages (Wikipedia / ZIM / web-archive)
that **emits egui paint primitives** — the host app owns scrolling, clipping, and input. No
JavaScript, no network, no GPU device of its own. Built to replace Blitz (which we couldn't get to
render/scroll cleanly in egui) while keeping the same "real browser engine, our integration"
shape. Targets **egui 0.32** (matches the `notes` host app).

## Pipeline

```
HTML ─html5ever (custom TreeSink)→ arena DOM (dom::Node)
CSS  ─Servo Stylo cascade→ Arc<ComputedValues> per node
                          ↓ projected once at css::stylo::computed_style_for → our ComputedStyle
              box construction (anon boxes, inline/block split, absolute/float out-of-flow)
                          ↓
       hand-rolled layout: block · inline (own line-breaker, egui-measured) · table · float · absolute
                          ↓  <math> / Wikipedia math <img> → hiker-math → SVG → resvg raster → texture
              display list: Vec<egui::Shape> + link rects + textures (document coords)
                          ↓
              host: ScrollArea → HtmlView::paint(painter, origin, clip)
```

The CSS cascade is **Servo Stylo** (crate `stylo`), bridged over our arena DOM via the `TElement`
/`TNode`/`TDocument` traits. Stylo's `ComputedValues` is touched in exactly one place —
`css::stylo::computed_style_for`, which projects it into our owned `ComputedStyle`; all of layout
reads only `ComputedStyle`. Flex/Grid via **Taffy** is planned but not yet wired (see
`references/STYLO_INTEGRATION.md`).

See `references/ARCHITECTURE.md` (design, distilled from litehtml + Servo) and `BUILD_PLAN.md`
(the type/API contract).

## Public API (`src/lib.rs`)

```rust
let view = HtmlView::new(html, base_url, provider /* Arc<dyn ResourceProvider> */);
view.set_theme(Theme::Light);  view.set_zoom(1.0);
let size = view.layout(ctx, width);            // cached; returns content size
view.paint(&painter, origin, clip_rect);       // origin = scroll-adjusted doc top-left
view.link_at(doc_point);                        // -> Option<String> (href)
```

`ResourceProvider::fetch(url) -> Option<(Vec<u8>, String)>` is the host's synchronous, offline
subresource resolver (CSS + images).

## Run it

From the workspace root:

```bash
cargo run -p hiker-htmlview --example viewer     # interactive: renders wiki-sample/article.html
                                                 # (the "Water" article), theme toggle + zoom,
                                                 # link hover/click, LaTeX math
cargo run -p hiker-htmlview --example snapshot   # headless: writes target/snap-{light,dark}-{top,infobox}.png
cargo test  -p hiker-htmlview                    # dom/css(stylo)/layout/table/float/paint/math/e2e
```

> The interactive/headless/e2e examples read `hiker-render/htmlview/wiki-sample/` (article HTML +
> per-article CSS/images). If that corpus is missing, those examples and the wiki-article tests
> won't run; the rest of the suite (synthetic fixtures) still does.

## Module map (`src/`)

| area | files |
|------|-------|
| DOM | `dom.rs` (html5ever TreeSink → arena `Document`, Stylo node state) |
| CSS | `css/{values,computed,ua}.rs` + `css/stylo/{mod,read,data}.rs` (Stylo bridge + `ComputedStyle` projection) |
| layout | `layout/{mod,fonts,construct,block,inline,table,float}.rs` |
| paint | `paint.rs` (display list), `lib.rs` (`HtmlView` orchestration + caching + textures + `<math>` pre-render) |
| demo | `examples/{viewer,snapshot,profile,rasterize_svg}.rs` |

## Supported
Servo Stylo cascade (the full CSS selector/specificity/media-query machinery, UA + inline +
`<style>` + external `<link>` sheets, `prefers-color-scheme`, presentational attributes); block +
inline formatting (own line-breaker, egui-measured text); floats (infoboxes/thumbnails); tables
(colspan/rowspan, auto column widths, border-collapse/separate); `position:absolute/fixed`;
`opacity`; box model; lists; links; raster **and SVG** `<img>` (via resvg); **LaTeX math**
(inline + block display equations, via `hiker-math`, incl. Wikipedia's MathML/`<img>` fallbacks);
light/dark themes; zoom.

## Known limitations
- **Flex/Grid** are not yet laid out (the Taffy seam is planned, not wired).
- `<img>` natural size isn't fed back into layout (paints into the layout-assigned rect).
- `vertical-align` in table cells is top-only; margin-collapse is simplified (adjacent siblings).
- No JS, transforms, `position:sticky`, animations (out of scope by design).
