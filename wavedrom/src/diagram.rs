//! [`DiagramRenderer`] implementation for the WaveDrom engine.
//!
//! Wraps the free [`crate::render`] / [`crate::check`] entry points in the shared
//! [`hiker_diagram`] trait so a host can render or syntax-check WaveDrom through
//! the same seam as mermaid / math. A JSON5 syntax error carries a byte span
//! (from json5's line/column); the other errors are coarse (span `None`).

use hiker_diagram::{Diagnostic, DiagramRender, DiagramRenderer};

use crate::{WaveDromError, WaveDromOptions, WaveDromRender};

/// Marker type implementing [`DiagramRenderer`] for WaveDrom diagrams.
pub struct WaveDrom;

/// Map a [`WaveDromError`] to the shared diagnostic shape, preserving a parse
/// span when one was located.
fn diagnostics(err: WaveDromError) -> Vec<Diagnostic> {
    let diag = match err {
        WaveDromError::Parse(msg, Some(span)) => Diagnostic::error(msg).with_span(span),
        WaveDromError::Parse(msg, None) => Diagnostic::error(msg),
        WaveDromError::Empty => Diagnostic::error("empty diagram (no signals / fields)"),
        WaveDromError::Unsupported(msg) => Diagnostic::error(msg),
    };
    vec![diag]
}

impl DiagramRenderer for WaveDrom {
    type Options = WaveDromOptions;

    fn render(src: &str, opts: &WaveDromOptions) -> Result<DiagramRender, Vec<Diagnostic>> {
        match crate::render(src, opts) {
            Ok(WaveDromRender {
                svg,
                width_px,
                height_px,
            }) => Ok(DiagramRender {
                svg,
                width_px,
                height_px,
            }),
            Err(e) => Err(diagnostics(e)),
        }
    }

    fn check(src: &str) -> Vec<Diagnostic> {
        match crate::check(src, &WaveDromOptions::default()) {
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
    fn check_accepts_valid_wavejson() {
        // A minimal but renderable timing diagram.
        assert!(WaveDrom::check(r#"{signal:[{name:"clk",wave:"p..."}]}"#).is_empty());
    }

    #[test]
    fn check_rejects_broken_json() {
        // Unterminated object — a JSON5 syntax error.
        let diags = WaveDrom::check("{signal:");
        assert!(!diags.is_empty(), "broken json should produce diagnostics");
        let d = &diags[0];
        assert_eq!(d.severity, Severity::Error);
        assert!(!d.message.is_empty(), "diagnostic message must be non-empty");
    }

    #[test]
    fn check_rejects_unsupported_form() {
        // Valid JSON5, but no `signal`/`reg`/array top-level form.
        let diags = WaveDrom::check(r#"{foo:1}"#);
        assert!(!diags.is_empty(), "unsupported form should produce diagnostics");
        assert_eq!(diags[0].severity, Severity::Error);
    }

    #[test]
    fn render_roundtrips_through_trait() {
        let r = WaveDrom::render(
            r#"{signal:[{name:"clk",wave:"p..."}]}"#,
            &WaveDromOptions::default(),
        )
        .expect("renders");
        assert!(r.svg.starts_with("<svg"), "is an svg document");
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }
}
