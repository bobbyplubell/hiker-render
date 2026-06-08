//! `hiker-diagram` — the shared seam across hiker's pure-Rust diagram engines
//! (mermaid, WaveDrom, math).
//!
//! Each engine keeps its own free `render_*` entry point and its own options
//! type; this crate adds one common trait, [`DiagramRenderer`], so a host (the
//! editor, the agent) can render *or* syntax-check any of them through one shape:
//!
//! - [`DiagramRenderer::render`] → an SVG + pixel size, or a list of
//!   [`Diagnostic`]s on failure.
//! - [`DiagramRenderer::check`] → a parse-only syntax check: an empty `Vec`
//!   means OK, a non-empty one carries the problems. This is the editor/agent
//!   seam — it does the minimum work needed to decide "does this parse?".
//!
//! Egui-agnostic and graphics-free, like the rest of `hiker-render`.

use core::ops::Range;

/// A rendered diagram: a self-contained SVG document plus its pixel size.
///
/// The per-engine render structs (`MermaidRender`, `WaveDromRender`,
/// `MathRender`) carry the same fields (math additionally carries a baseline);
/// the trait flattens them to this common shape.
#[derive(Clone, Debug, PartialEq)]
pub struct DiagramRender {
    /// A complete, self-contained SVG document.
    pub svg: String,
    /// Rendered size in CSS px.
    pub width_px: f32,
    pub height_px: f32,
}

/// How serious a [`Diagnostic`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// The source could not be rendered / does not parse.
    Error,
    /// Renders, but something is suspect.
    Warning,
    /// Informational note.
    Info,
}

/// A single problem found while rendering or checking a diagram.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// Human-readable description of the problem.
    pub message: String,
    /// Byte range in the source the problem refers to, when known. `None` is a
    /// coarse whole-source diagnostic (the engine couldn't cheaply localize it).
    pub span: Option<Range<usize>>,
    /// How serious the problem is.
    pub severity: Severity,
}

impl Diagnostic {
    /// An [`Severity::Error`] diagnostic with no span.
    pub fn error(msg: impl Into<String>) -> Self {
        Diagnostic {
            message: msg.into(),
            span: None,
            severity: Severity::Error,
        }
    }

    /// Attach a byte `span` to this diagnostic.
    #[must_use]
    pub fn with_span(mut self, span: Range<usize>) -> Self {
        self.span = Some(span);
        self
    }
}

/// A diagram engine that can render to SVG and syntax-check its source.
///
/// Implemented as a zero-sized marker type per engine (e.g. `Mermaid`,
/// `WaveDrom`, `Math`) so the trait can be used without an instance.
pub trait DiagramRenderer {
    /// The engine's own rendering options (sizes, colors, fonts, …).
    type Options;

    /// Render `src` to SVG, or return the diagnostics that explain why it
    /// couldn't be rendered.
    fn render(src: &str, opts: &Self::Options) -> Result<DiagramRender, Vec<Diagnostic>>;

    /// Parse-only syntax check: an empty `Vec` means the source is well-formed,
    /// otherwise the returned diagnostics describe the problems. The editor/agent
    /// seam — it does only the work needed to decide whether `src` parses.
    fn check(src: &str) -> Vec<Diagnostic>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_ctor_has_no_span_and_error_severity() {
        let d = Diagnostic::error("boom");
        assert_eq!(d.message, "boom");
        assert_eq!(d.span, None);
        assert_eq!(d.severity, Severity::Error);
    }

    #[test]
    fn with_span_attaches_range() {
        let d = Diagnostic::error("boom").with_span(3..7);
        assert_eq!(d.span, Some(3..7));
    }
}
