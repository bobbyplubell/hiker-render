//! [`DiagramRenderer`] implementation for the mermaid engine.
//!
//! Wraps the free [`crate::render`] / [`crate::check`] entry points in the shared
//! [`hiker_diagram`] trait so a host can render or syntax-check mermaid through
//! the same seam as WaveDrom / math.

use hiker_diagram::{Diagnostic, DiagramRender, DiagramRenderer};

use crate::{MermaidError, MermaidOptions, MermaidRender};

/// Marker type implementing [`DiagramRenderer`] for mermaid diagrams.
pub struct Mermaid;

/// Map a [`MermaidError`] to the shared diagnostic shape.
fn diagnostics(err: MermaidError) -> Vec<Diagnostic> {
    let msg = match err {
        MermaidError::Parse(s) => s,
        MermaidError::Empty => "empty diagram (no nodes to render)".to_string(),
    };
    vec![Diagnostic::error(msg)]
}

impl DiagramRenderer for Mermaid {
    type Options = MermaidOptions;

    fn render(src: &str, opts: &MermaidOptions) -> Result<DiagramRender, Vec<Diagnostic>> {
        match crate::render(src, opts) {
            Ok(MermaidRender {
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
        match crate::check(src) {
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
    fn check_accepts_valid_diagrams() {
        // A flowchart and a pie chart both parse cleanly.
        assert!(Mermaid::check("graph TD\nA-->B").is_empty());
        assert!(Mermaid::check("pie\n\"A\" : 10").is_empty());
    }

    #[test]
    fn check_rejects_malformed_diagram_body() {
        // The flowchart grammar is intentionally lenient (it never errors), so we
        // exercise a strict diagram type: a malformed pie data line is a genuine
        // syntax error the seam must surface.
        let diags = Mermaid::check("pie title\n: notanumber");
        assert!(!diags.is_empty(), "malformed pie body should produce diagnostics");
        let d = &diags[0];
        assert_eq!(d.severity, Severity::Error);
        assert!(!d.message.is_empty(), "diagnostic message must be non-empty");
    }

    #[test]
    fn check_rejects_unknown_diagram_type() {
        let diags = Mermaid::check("notADiagram foo\nbar");
        assert!(!diags.is_empty(), "unknown type should produce diagnostics");
        assert_eq!(diags[0].severity, Severity::Error);
        assert!(!diags[0].message.is_empty());
    }

    #[test]
    fn render_roundtrips_through_trait() {
        let r = Mermaid::render("graph TD\nA-->B", &MermaidOptions::default()).expect("renders");
        assert!(r.svg.starts_with("<svg"), "is an svg document");
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }
}
