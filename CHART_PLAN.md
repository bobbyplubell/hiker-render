# hiker-chart — CSV charting widget

Pure-Rust CSV → chart renderer, a sibling of the `hiker-render/{math,mermaid,wavedrom}`
crates. Egui-agnostic, **SVG-string out**, rasterized through the existing
resvg→texture pipeline. Lives at `hiker-render/chart/` (pkg `hiker-chart`).

## Goal

View CSV as charts in the PKM. Two surfaces:

1. **Inline charts in notes** — a fenced `chart` block in a `.md`.
2. **Standalone CSV** — open a `.csv` and render it directly (config optional).

A third consumer shares the same code: **in-app internal/debug graphs** in the
core egui UI. Since plotters is wanted there anyway, reusing it here means one
charting stack across the whole app.

## Interface: YAML config + CSV data (not a DSL)

Unlike the other render crates, the front-end is **not** a hand-rolled grammar.
Config is real YAML, deserialized straight into a `ChartConfig` struct — the
"grammar" is the struct definition. This keeps the **CSV a clean, valid CSV**
(no config baked into the data) and means zero parser to maintain.

```
ChartConfig                       // serde struct; also hand-constructible in Rust
render(&ChartConfig, &Csv) -> ChartRender { svg, w, h }
```

YAML is just *one constructor* for the config, never the interface. The three
consumers all hit one render core:

- inline note chart → deserialize YAML → `ChartConfig`
- standalone CSV → inferred default `ChartConfig` (first col = x, numeric cols = series)
- in-app graph → build `ChartConfig` in Rust

### Inline block shape

YAML on top, `---`, CSV below (the CSV half stays pristine):

```chart
type: line
x: month
y: [revenue, profit]
---
month,revenue,profit
jan,100,20
feb,140,35
```

Also support a pure-YAML block with `data: sales.csv` (no inline data). Either
way it's the same `ChartConfig`.

## Crate stays pure — host owns file I/O

The render fn takes config + CSV **text/bytes**, never a path. For `data:`
references, **hiker-core** resolves the path and reads the bytes, then passes
them in. Keeps the crate egui-agnostic and host-policy-free, and lets embedded
data, file refs, and in-app data flow through one function uniformly.

## Backend: plotters (SVG), font-kit OFF

`plotters` + `plotters-svg`, with `default-features = false` and **no `ttf`
feature** on the SVG path. Reasons:

- plotters-svg only **measures** text for layout; it emits `<text>` elements and
  leaves glyph drawing to the SVG consumer (**resvg**, with our bundled
  Liberation Sans via the `sans-serif` fontdb mapping). So the SVG path needs no
  real font backend.
- The `ttf` feature pulls **font-kit** → system fonts + native freetype/fontconfig,
  and breaks wasm. Worse, in the SVG path it creates a **measure ≠ render split**:
  plotters would measure with the system font while resvg draws with ours.
- With `ttf` off, plotters uses approximate-but-self-consistent metrics; charts
  have generous margins so it's tolerable. If it ever bites, we already own
  `font.rs` (ttf-parser Liberation Sans advances) to feed real widths.

`ttf` is gated behind this crate's optional **`bitmap-fonts`** feature, enabled
only by a future in-app **bitmap/egui** path where plotters draws glyphs itself.

## Other decisions

- **YAML lib:** `serde_yml` (maintained serde_yaml fork) — matches hiker-core
  (`../notes` already uses `serde_yml` + `toml`).
- **CSV lib:** `csv` (BurntSushi).
- **Type inference** for bare CSVs: per-column numeric vs date vs string, to
  auto-pick axes/series. plotters handles date axes (`chrono` feature) once
  dates are detected.
- **Distinct from mermaid's `xychart`:** that one is *diagram*-oriented (fixed
  categorical axes). This is for *data* — real numeric/date axes, auto-scaling,
  many series. Keep them separate.

## v1 scope

CSV → `{line, bar, scatter, area}` with linear + categorical axes, title,
legend, palette. Defer: log/date axes polish, dual axes, histograms/binning,
the long data-viz tail.

## Open / to de-risk

- **Font spike:** confirm plotters (`default-features=false`, `svg_backend`, no
  `ttf`) emits clean `<text>` that resvg renders via the fontdb mapping.
- **In-app graph path:** bitmap/egui vs SVG→resvg? Decides whether `bitmap-fonts`
  (font-kit) is ever needed.
- **plotters + dynamic config:** its generic `ChartContext` is awkward to drive
  from runtime config; plan to map categorical x → numeric indices with a label
  formatter to keep a single `f64 × f64` coordinate type.

## Status

Scaffold only: `Cargo.toml` (dep decisions captured), stub `lib.rs`, bundled
font, workspace member. No implementation yet.
