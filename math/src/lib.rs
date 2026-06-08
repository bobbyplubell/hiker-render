//! `hiker-math` — reusable, **egui-agnostic** LaTeX math typesetting.
//!
//! Callers (the HTML renderer, a markdown editor, …) hand it a math-mode LaTeX
//! string + options and get back an **SVG document plus layout metrics**, which
//! they can place inline and rasterize with whatever backend they already have
//! (e.g. resvg → texture). The crate knows nothing about HTML, CSS, or egui.
//!
//! ## Why SVG out
//!
//! SVG keeps the core free of any graphics dependency (the output is just a
//! `String`) and maximally portable. A direct draw-command / egui backend can
//! be added behind a feature later without changing the layout engine.
//!
//! ## Pipeline
//!
//! `LaTeX → [pulldown-latex parser] → event stream → [our layout] → SVG`.
//! The layout backend follows the TeXbook's Appendix G and the MathML Core
//! spec, using an OpenType **MATH**-table font for metrics. Reference
//! implementations studied (read-only, never linked): `references/microtex`
//! (C++), `references/katex` (JS).

pub mod diagram;
pub mod font;
pub mod math;

pub use diagram::Math;
pub use math::{
    check_latex, render_latex, render_latex_with_preamble, MathError, MathOptions, MathRender,
    MathStyle,
};
