# hiker-htmlview

A from-scratch, pure-Rust HTML/CSS renderer for **static, offline** pages (Wikipedia / ZIM /
web-archive) that **emits egui paint primitives** — the host app owns scrolling, clipping, and
input. No JavaScript, no network, no GPU device of its own, no taffy/Blitz. Built to replace
Blitz.

Targets **egui 0.32** (matches the `notes` host app).

## Pipeline

```
HTML ─html5ever(custom TreeSink)→ arena DOM
CSS  ─cssparser + hand-rolled selectors/cascade→ ComputedStyle per node
                                  ↓ box construction (anon boxes, inline/block split)
              hand-rolled layout: block · inline(line-breaker) · table · float
                                  ↓
              display list: Vec<egui::Shape> + link rects (document coords)
                                  ↓
              host: ScrollArea → HtmlView::paint(painter, origin, clip)
```

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

```bash
cargo run --example viewer       # interactive: renders wiki-sample/article.html (the "Water"
                                 # article), theme toggle + zoom, link hover/click
cargo run --example snapshot     # headless: writes target/snap-{light,dark}-{top,infobox}.png
cargo test                       # 29 tests (dom/css/layout/table/float/paint/e2e)
```

## Module map (`src/`)

| area | files |
|------|-------|
| DOM | `dom.rs` (html5ever TreeSink → arena `Document`) |
| CSS | `css/{values,computed,selector,parser,cascade,ua}.rs` |
| layout | `layout/{mod,fonts,construct,block,inline,table,float}.rs` |
| paint | `paint.rs` (display list), `lib.rs` (`HtmlView` orchestration + caching + images) |
| demo | `examples/{viewer,snapshot}.rs` |

## Supported (v1)
Block + inline formatting (own line-breaker, egui-measured text), floats (infoboxes/thumbnails),
tables (colspan/rowspan, auto column widths, border-collapse/separate), the cascade (tag/class/id
/descendant/child/attribute selectors, specificity, inline/`<style>`/external `<link>` sheets,
`prefers-color-scheme`), box model, lists, links, raster `<img>`, light/dark themes, zoom.

## Known v1 limitations
- SVG and missing images render as placeholder boxes (no SVG rasterizer yet).
- `<img>` natural size isn't fed back into layout (paints into the layout-assigned rect).
- `vertical-align` in table cells is top-only; margin-collapse is simplified (adjacent siblings).
- No JS, `position:absolute/fixed/sticky`, transforms, grid, animations (out of scope by design).
