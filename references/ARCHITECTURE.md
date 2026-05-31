# hiker-htmlview — Architecture notes (distilled from references)

Distilled from a deep read of **litehtml** (C++), **Servo `components/layout`** (Rust),
and **gumbo** (HTML5 parse output), plus a current Rust-crate survey. This is the design
reference for the from-scratch egui renderer. Refs live under `references/`.

---

## 0. The shape of the thing

```
HTML bytes ──html5ever──▶ DOM (our arena)         ┐
CSS bytes  ──cssparser──▶ stylesheets (rules)     │ cascade (hand-rolled)
                                                  ▼
                              Styled DOM  (node + ComputedStyle)
                                                  │  box construction (anon boxes, inline/block split)
                                                  ▼
                              Layout pass (hand-rolled; egui Fonts measures text)
                                                  ▼
                              Display list: Vec<egui::Shape> + link rects
                                                  ▼
                  host: ScrollArea → paint(painter, origin, clip) ; link_at(point)
```

Two **conceptual** passes (construct → layout) but, for static pages with no incremental
relayout, **one set of node types** carrying both style and the computed rect. (Servo keeps
box-tree and fragment-tree as *separate* types only to cache and re-layout incrementally —
we don't need that. Keep the *pass* separation, collapse the *types*.)

---

## 1. Host-delegation boundary (the litehtml lesson — our egui paint layer)

litehtml never rasterizes glyphs or owns pixels: it calls back into a `document_container`.
That callback set IS our egui paint API. Mapping:

| litehtml `document_container`            | our egui equivalent |
|------------------------------------------|---------------------|
| `create_font(descr) -> handle + metrics` | resolve to `egui::FontId` + ascent/descent/line-height from `Fonts` |
| `text_width(text, font)`                 | `Fonts::layout_no_wrap(...).rect.width()` (cached) |
| `draw_text(text, font, color, pos)`      | `Shape::galley` / `Painter::galley` |
| `draw_solid_fill` / `draw_background`    | `Shape::rect_filled` |
| `draw_borders`                           | `Shape::rect_stroke` (4 edges if asymmetric) |
| `draw_image` / `get_image_size`          | `Shape::image` via `TextureHandle`; size from provider-decoded bytes |
| `set_clip` / `del_clip`                  | `Painter::with_clip_rect` |
| `draw_list_marker`                       | bullet = `Shape::circle_filled`; number = galley |

**Crucial difference from litehtml:** litehtml paints *immediately*, calling back mid-tree-walk.
We instead **emit a display list** (`Vec<egui::Shape>` + parallel `Vec<(Rect, href)>`), so the
host owns scroll/clip/compositing/repaint. This is the whole point of the rewrite.

Opaque `uint_ptr`/`hdc` C idioms → Rust enums + typed handles. Don't copy the C ABI.

---

## 2. The layout contract (down/up)

litehtml's clean contract, worth copying:

- **Down:** a `ContainingBlock`-style struct passes available inline width (+ whether it's
  auto/percent/definite) into `layout(node, cb) -> Fragment/size`. Thread it as a *parameter*,
  don't store it on nodes (Servo idiom — keeps layout a pure function).
- **Up:** each node returns its used size, and an intrinsic `ContentSizes { min_content,
  max_content }` for shrink-to-fit / auto-table sizing. Make `ContentSizes` a first-class type
  with `max`/`union` combinators (Servo). This is the backbone of width resolution.

Avoid litehtml's two-pass `render(second_pass=true)` hack; prefer a clean min/max-content
intrinsic pass feeding the used-width pass.

---

## 3. Formatting contexts = enum of algorithms (Servo idiom)

```rust
enum FormattingContext {
    Block(BlockContainer),     // BFC
    Inline(InlineContext),     // IFC
    Table(Table),
    Replaced(ReplacedKind),    // img, etc.
    // Flex later (nice-to-have)
}

// The key invariant, encoded in the type system:
enum BlockContainer {
    Blocks(Vec<NodeId>),       // block-level children
    Inline(InlineContext),     // XOR exactly one IFC
}
```
A block container holds **either** block children **or** a single inline context, never mixed —
anonymous-box fixup during construction enforces this. Exhaustive `match`, no `dyn`. Start with
`Block` + `Inline` + `Replaced`; add `Table` second.

Use **logical geometry** vocab (inline/block, `LogicalVec2/Rect/Sides`) even if we hardcode
LTR-horizontal `to_physical`. Keeps sizing code uniform.

---

## 4. Inline / text layout (the #1 hard part)

**Decision: we own the line-breaker; egui only measures.** (egui *measures*, we *break*.)

litehtml model to copy:
- Pre-tokenize each text node into **word boxes** and **space boxes**; images / inline-blocks
  are **atomic** boxes with a measured size. The inline layouter never splits a string.
- Represent the inline content as a **flat `Vec<InlineItem>`** with `StartInlineBox(id)` /
  `EndInlineBox` markers around runs (Servo idiom) rather than a nested tree — simpler.
- Greedy fill: `LineBox { left, right, width, items }`. `can_hold` = `left+width+item.w <= right`.
  On overflow: finish line (trim trailing space → apply text-align) and open a new line whose
  `left/right` come from the **float manager** at that y.
- Measure a run via egui `Fonts::layout(...)` → `Galley`, read `rect.width()`. **Cache
  `(text, font_id) -> width`** — words repeat heavily; this dominates cost.

**Why not hand whole paragraphs to egui to wrap:**
- We need per-word rects keyed to the owning element for **link hit-testing** and `:hover`
  underlines (`link_at`). egui-wrapped jobs lose that element→rect mapping.
- Mixed inline styles (bold/size/color, nested span padding/border, `vertical-align`) force
  per-style run splitting anyway.
- CSS **justify** distributes slack between items — our breaker does it exactly; egui has no
  CSS justify.
- Lean on egui's own wrap only for `white-space: pre` segments / single unbreakable tokens.

Text-align on a finished, trimmed line: `right` → shift x by slack; `center` → slack/2;
`justify` → distribute slack across inter-item gaps (skip if slack huge). Vertical: baseline
accumulation from font metrics (ascent/descent), `line-height`, `vertical-align`
(sub/super/middle/top/bottom). Block height = last line's bottom.

---

## 5. Floats (#3 hard part)

- Float manager = bands. Servo: `FloatBand { top, inline_start: Option, inline_end: Option }`.
  **Use a plain `Vec<FloatBand>` sorted by top** (skip Servo's persistent AA tree — that's for
  snapshotting during incremental layout we don't do).
- `get_line_left(y)` = max right edge of left floats spanning y; `get_line_right(y)` = min left
  edge of right floats. New lines query these to shrink available width and flow around floats.
- `clear` raises the next line/block top below cleared floats.
- A BFC that contains floats grows to enclose them: `height = max(content_height,
  floats_height)`.
- Float placement re-flows the current line against the new narrower width (litehtml
  `fix_line_width`).

---

## 6. Tables (#2 hard part — load-bearing for Wikipedia infoboxes)

Grid model (Servo `Vec<Vec<TableSlot>>`, litehtml `vector<vector<table_cell>>` — same idea):
```rust
enum TableSlot {
    Cell(CellId),            // origin of a (colspan,rowspan) cell
    Spanned(originCellOffset),// covered by an earlier cell; store offset back to origin
    Empty,
}
```
Each cell is its own block FC. Build the grid by walking `<tr>/<td>`, skipping slots occupied
by rowspans from above, padding rows to rectangular.

**Column width algorithm** (CSS 2.1 auto table layout — port litehtml `calc_table_width` +
`distribute_width`):
1. Per-cell **min-content / max-content** measure. Implement a single
   `measure(constraint) -> (min,max)` instead of litehtml's double-render.
2. Aggregate per column (colspan==1: column min/max = max over column). Spanning cells
   (colspan>1) distribute their excess across spanned columns **proportional to each column's
   max_width**: `add = excess * (col.max / sum_of_spanned_max)`.
3. Fit to available width: grow auto cols toward max; if slack remains distribute in 3 steps
   (auto cols → percent cols → all), share proportional to each col's slack `(max-min)`. If
   overflow, rescale percent cols, then shrink toward min.
4. Render cells at final widths; row height = max non-rowspan cell height; rowspan deficit
   added to last spanned row; distribute extra block height (percent rows → auto rows → all).

**border-collapse:** shared border = (litehtml takes the *thinner*; CSS says thickest-wins).
Wikipedia infoboxes are mostly uniform 1px, so the thinner approximation is fine for v1; do
thickest-wins only if needed. `border-separate` uses `border-spacing` between cells.

Be deliberate about integer vs float rounding + the leftover-pixel patch (litehtml mixes them).

---

## 7. Cascade & selectors (hand-rolled)

- Parse selectors + declarations with **cssparser** (battle-tested tokenizer); we own property
  interpretation. We do NOT pull in Servo `selectors` (its `Element` trait wants ~26-30 methods
  for shadow DOM/`::part`/bloom filters we never use).
- Matcher: tag / `.class` / `#id` / descendant (space) / child (`>`), W3C `(a,b,c)` specificity,
  source order tiebreak. A few hundred lines.
- Origins/order: UA stylesheet → external (`<link>` via provider) → `<style>` → inline `style=`.
  `!important` lifts above normal. Inheritance resolved explicitly into an owned `ComputedStyle`
  (do NOT copy litehtml's parent struct member-offset pointer trick).
- Lazy unit resolution: store `css_length` with units; resolve to px against font metrics + base
  at use sites.

**UA stylesheet:** port litehtml's `master_css.h` (~370 lines) nearly verbatim into a Rust
`const &str`. Covers display values for all structural/table/list tags, `head/script/style →
display:none`, default block margins, `a:link` blue+underline, table defaults
(`border-collapse:separate; border-spacing:2px`, cell padding, `[border]` attr rules),
`pre/code` monospace + `white-space:pre`, `hr`, `sub/sup`. Add `prefers-color-scheme` via
`set_theme`.

---

## 8. Paint emission = "display list" pass (Servo `display_list`)

Walk the laid-out tree and push egui shapes in **paint order**: backgrounds/borders → floats →
in-flow content → positioned (relative). Flatten Servo's stacking-context tree into a single
ordered `Vec<Shape>` built in a few ordered sub-passes — don't build the full tree. Collect
`(Rect, href)` alongside for `link_at`. Only emit shapes intersecting `clip_rect`.

---

## 9. Recommended crate stack (v1)

- **html5ever 0.38** — spec parsing; implement our own `TreeSink` into an arena DOM
  (skip `markup5ever_rcdom` — explicitly unsupported/test-only).
- **cssparser 0.37** — CSS tokenizer + selector-list/declaration parsing; we own properties.
- **hand-rolled** selector matcher + specificity + cascade + property parsing.
- **egui / epaint** — paint target AND text engine: `Fonts::layout_job → Galley` for measure,
  wrap (where wanted), paint, and `cursor_from_pos` hit-testing.
  ⚠ **Must match the host app's egui version exactly** (egui types aren't cross-version
  compatible). Confirm before scaffolding.
- **Defer:** `cosmic-text` (only if real bidi/complex-script shaping needed — egui's galley
  covers Latin), `lightningcss` (revisit if hand property-parsing gets heavy; still alpha).
- Image decode for `<img>`: `image` crate (PNG/JPEG); SVG (`Earth.svg` in samples) → `resvg`/
  `usvg` later or render a placeholder for v1.

---

## 10. Patterns to AVOID (from the references)

- litehtml's immediate `draw(hdc)` mid-walk → we emit a display list instead.
- litehtml opaque `uint_ptr` handles / `hdc` → typed Rust handles.
- litehtml inheritance via raw struct member-offset arithmetic → explicit cascade.
- litehtml `shared_ptr`/`weak_ptr` graph + `enable_shared_from_this` → arena + indices (NodeId).
- litehtml two-pass re-render hack → clean intrinsic (min/max-content) pass.
- Servo's incremental machinery (`ArcRefCell`, `cached_layout_result`, `repair_style`, rayon,
  two separate trees, persistent AA float tree) → drop entirely; render once, single-threaded.
- Servo bidi/script segmentation + HarfBuzz → egui galley absorbs it for our content.

---

## 11. Test corpus

`wiki-sample/` — real Wikipedia articles + per-article CSS and images in `wiki-sample/assets/`:
`Cat.html` (small, start here), `Photosynthesis.html`, `Lion.html`, `Mathematics.html`,
`Wikipedia.html`, `Earth.html`, `Water.html` (large, infoboxes, SVG). Each page = external
`<link>` CSS + inline `<style>` + `.mw-parser-output` body. The `ResourceProvider` is backed by
the `assets/` dir for tests. SVG present (`Earth.svg`) — needs a plan. User will eyeball-verify
rendered output.

## Reference source map
- litehtml: `document_container.h` (paint API blueprint), `render_item.{h,cpp}` (render dispatch),
  `formatting_context.{h,cpp}` (floats/line oracle), `line_box.{h,cpp}` (inline layout),
  `table.cpp` + `render_table.cpp` (`distribute_width`@~192, `calc_table_width`@~290),
  `css_properties.h`, `master_css.h` (UA sheet).
- servo: `flow/root.rs` (BoxTree), `formatting_contexts.rs` (FC enum), `flow/inline/*`
  (flat item array, line breaking), `flow/float.rs` (bands), `table/*` (slot grid),
  `fragment_tree/*`, `display_list/*`, `sizing.rs` (ContentSizes), `geom.rs` (logical geometry).
