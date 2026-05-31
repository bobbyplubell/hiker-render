//! `hiker-render` — reusable, **egui-agnostic** renderers.
//!
//! Today this hosts LaTeX **math** typesetting; mermaid diagram rendering is
//! planned to live here too. The crate deliberately knows nothing about HTML,
//! CSS, or egui: callers (the `hiker-htmlview` widget, hiker-core's markdown
//! editor) hand it a source string + options and get back an **SVG document
//! plus layout metrics**, which they can place inline and rasterize with
//! whatever backend they already have (e.g. resvg → texture).
//!
//! ## Why SVG out
//!
//! SVG keeps the core free of any graphics dependency (the output is just a
//! `String`) and maximally portable. A direct draw-command / egui backend can
//! be added behind a feature later without changing the layout engine.
//!
//! ## Math pipeline
//!
//! `LaTeX → [pulldown-latex parser] → event stream → [our layout] → SVG`.
//! The layout backend follows the TeXbook's Appendix G and the MathML Core
//! spec, using an OpenType **MATH**-table font for metrics. Reference
//! implementations studied (read-only, never linked): `references/microtex`
//! (C++), `references/katex` (JS).

pub mod font;
pub mod math;

pub use math::{render_latex, MathOptions, MathRender, MathStyle};
