//! Stage 3: lay out the flowchart. Upstream: the [`hiker_graph`] crate.
//!
//! STUB — implemented by the layout subagent. Maps the [`FlowChart`] + measured
//! node sizes onto hiker-graph's layered (dagre) layout and reads back node
//! centers + edge routes into a [`PositionedDiagram`]. If the `hiker_graph`
//! public API is insufficient, change it there (this module is the only
//! consumer) and report the change.

use crate::MermaidOptions;
use crate::model::{FlowChart, PositionedDiagram};

/// Lay out `chart`, using `sizes[i]` as node `i`'s `(width, height)` (same order
/// as `chart.nodes`). Produces a 0-origin [`PositionedDiagram`].
pub fn layout_flowchart(
    chart: &FlowChart,
    sizes: &[(f32, f32)],
    opts: &MermaidOptions,
) -> PositionedDiagram {
    let _ = (chart, sizes, opts);
    PositionedDiagram::default() // STUB
}
