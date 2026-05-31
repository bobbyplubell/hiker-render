//! Stage 4: draw a [`PositionedDiagram`] → self-contained SVG. Upstream: the
//! SVG-out / resvg conventions shared with the `hiker-render` math engine.
//!
//! STUB — implemented by the render subagent. Emits node shapes, edge polylines
//! (with arrowhead markers), and `<text>` labels into one `<svg>` document.

use crate::MermaidOptions;
use crate::model::PositionedDiagram;

/// Emit a complete, self-contained SVG document for `diagram`.
pub fn draw_svg(diagram: &PositionedDiagram, opts: &MermaidOptions) -> String {
    let _ = (diagram, opts);
    String::new() // STUB
}
