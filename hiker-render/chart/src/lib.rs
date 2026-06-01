//! `hiker-chart` — pure-Rust CSV charting widget → SVG.
//!
//! Egui-agnostic, SVG-string out (same contract as the `hiker-render`
//! math / mermaid / wavedrom engines), so callers rasterize with their existing
//! resvg→texture pipeline. The interface is **YAML config + CSV data**, not a
//! hand-rolled DSL: config deserializes straight into a `ChartConfig` struct
//! (the same struct in-app graphs build directly), and the data stays a clean,
//! valid CSV.
//!
//! See `CHART_PLAN.md` at the repo root for the design.
//!
//! Scaffold only — no implementation yet.
