//! Stage 2: measure a node's box size from its label + shape. Upstream: text
//! metrics (heuristic for v1).
//!
//! STUB — implemented by the render subagent (paired with `draw`, since the
//! measured size must match the drawn text + padding).

use crate::MermaidOptions;
use crate::model::NodeShape;

/// Return the `(width, height)` in CSS px for a node box that comfortably holds
/// `label` (with `opts` padding) given its `shape`. Diamonds/circles need extra
/// room than a rect for the same label.
pub fn measure_node(label: &str, shape: NodeShape, opts: &MermaidOptions) -> (f32, f32) {
    let _ = (label, shape, opts);
    (0.0, 0.0) // STUB
}
