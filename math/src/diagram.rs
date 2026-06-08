//! [`DiagramRenderer`] implementation for the math engine.
//!
//! Wraps the free [`render_latex`] / [`check_latex`] entry points in the shared
//! [`hiker_diagram`] trait so a host can render or syntax-check math through the
//! same seam as mermaid / WaveDrom. The rendered [`MathRender::baseline_px`] is
//! dropped here — the common [`DiagramRender`] is geometry-only; callers that
//! need the baseline keep using [`render_latex`] directly.

use hiker_diagram::{Diagnostic, DiagramRender, DiagramRenderer};

use crate::math::{check_latex, render_latex, MathError, MathOptions, MathRender};

/// Marker type implementing [`DiagramRenderer`] for LaTeX math.
pub struct Math;

/// Map a [`MathError`] to the shared diagnostic shape.
fn diagnostics(err: MathError) -> Vec<Diagnostic> {
    let msg = match err {
        MathError::Parse(s) => s,
        MathError::Empty => "empty math expression".to_string(),
    };
    vec![Diagnostic::error(msg)]
}

impl DiagramRenderer for Math {
    type Options = MathOptions;

    fn render(src: &str, opts: &MathOptions) -> Result<DiagramRender, Vec<Diagnostic>> {
        match render_latex(src, opts) {
            Ok(MathRender {
                svg,
                width_px,
                height_px,
                baseline_px: _,
            }) => Ok(DiagramRender {
                svg,
                width_px,
                height_px,
            }),
            Err(e) => Err(diagnostics(e)),
        }
    }

    fn check(src: &str) -> Vec<Diagnostic> {
        match check_latex(src) {
            Ok(()) => Vec::new(),
            Err(e) => diagnostics(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hiker_diagram::Severity;

    #[test]
    fn check_accepts_valid_math() {
        assert!(Math::check("x^2").is_empty());
        assert!(Math::check(r"\frac{1}{2}").is_empty());
    }

    #[test]
    fn check_rejects_broken_math() {
        // An unterminated group: `\frac{` has no closing brace / second arg.
        let diags = Math::check(r"\frac{");
        assert!(!diags.is_empty(), "broken math should produce diagnostics");
        let d = &diags[0];
        assert_eq!(d.severity, Severity::Error);
        assert!(!d.message.is_empty(), "diagnostic message must be non-empty");
    }

    #[test]
    fn check_rejects_empty() {
        let diags = Math::check("   ");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Error);
    }

    #[test]
    fn render_roundtrips_through_trait() {
        let r = Math::render("x^2", &MathOptions::default()).expect("renders");
        assert!(r.svg.contains("<svg") || r.svg.contains("<path"));
        assert!(!r.svg.is_empty());
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }
}
