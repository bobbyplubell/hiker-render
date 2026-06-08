//! Tangle metrics for a laid-out graph — deterministic, geometry-only
//! measures of how "knotted" a node placement is under a given edge set.
//!
//! These exist to catch layout-quality regressions automatically (in
//! tests and headless examples) that are otherwise only visible to the
//! eye. The temporal-stability anchor springs in [`crate::force`] keep a
//! layout coherent across small clustering-parameter scrubs, but tethering
//! every retained node to its old spot fights a *big* structural rewrite —
//! the layout can't untangle. A numeric tangle measure turns "this looks
//! knotted" into a test assertion.
//!
//! Two measures, cheapest first:
//!
//! * [`total_edge_length`] — sum of edge lengths. A cheap proxy: a tangled
//!   layout drags edges across each other and tends to be longer, but a
//!   merely *spread out* layout is also longer, so it's not a pure tangle
//!   signal.
//! * [`edge_crossings`] — number of pairs of edges whose open segments
//!   properly cross. This is the truer "tangle" signal: a planar drawing
//!   scores 0, and crossings rise sharply when edges are forced over one
//!   another. `O(E²)`; fine for the graph sizes we lay out (≤ a few
//!   thousand edges in tests).

use crate::vec2::Vec2;

/// Sum of the Euclidean lengths of every edge under `positions`. Edges
/// referencing an out-of-range or self index contribute nothing. Cheap
/// proxy for tangle (see module docs); pair it with [`edge_crossings`] for
/// the real signal.
#[must_use]
pub fn total_edge_length(positions: &[Vec2], edges: &[(u32, u32)]) -> f32 {
    let mut total = 0.0;
    for &(a, b) in edges {
        let (a, b) = (a as usize, b as usize);
        if a >= positions.len() || b >= positions.len() || a == b {
            continue;
        }
        total += (positions[a] - positions[b]).length();
    }
    total
}

/// Count pairs of edges whose segments **properly intersect** under
/// `positions`: they cross at an interior point of both segments.
///
/// Edges that merely share an endpoint do not count (adjacent edges in a
/// graph always meet at their shared node — that is not a tangle), and
/// collinear overlaps are likewise excluded (no single proper crossing
/// point). Degenerate (zero-length / out-of-range / self) edges are
/// skipped. Pure and deterministic: identical inputs yield an identical
/// count. `O(E²)`.
#[must_use]
pub fn edge_crossings(positions: &[Vec2], edges: &[(u32, u32)]) -> usize {
    // Materialise valid edges as endpoint-index pairs once, skipping
    // anything degenerate, so the O(E²) double loop stays clean.
    let mut valid: Vec<(usize, usize)> = Vec::with_capacity(edges.len());
    for &(a, b) in edges {
        let (a, b) = (a as usize, b as usize);
        if a >= positions.len() || b >= positions.len() || a == b {
            continue;
        }
        valid.push((a, b));
    }

    let mut count = 0usize;
    for i in 0..valid.len() {
        let (a1, a2) = valid[i];
        let p1 = positions[a1];
        let p2 = positions[a2];
        for &(b1, b2) in valid.iter().skip(i + 1) {
            // Edges sharing a node meet at it by construction — not a
            // crossing. (Covers the common case of fan-out from a hub.)
            if a1 == b1 || a1 == b2 || a2 == b1 || a2 == b2 {
                continue;
            }
            if segments_properly_intersect(p1, p2, positions[b1], positions[b2]) {
                count += 1;
            }
        }
    }
    count
}

/// 2-D cross product of `(b - a)` and `(c - a)`. Sign tells orientation of
/// the turn a→b→c: positive = counter-clockwise, negative = clockwise,
/// zero = collinear.
#[inline]
fn orient(a: Vec2, b: Vec2, c: Vec2) -> f32 {
    let ab = b - a;
    let ac = c - a;
    ab.x * ac.y - ab.y * ac.x
}

/// Do segments `p1p2` and `p3p4` **properly** cross — i.e. intersect at a
/// single point interior to both? Endpoint-touching and collinear overlaps
/// return `false` (they have no single interior crossing point).
fn segments_properly_intersect(p1: Vec2, p2: Vec2, p3: Vec2, p4: Vec2) -> bool {
    let d1 = orient(p3, p4, p1);
    let d2 = orient(p3, p4, p2);
    let d3 = orient(p1, p2, p3);
    let d4 = orient(p1, p2, p4);
    // Strictly opposite sides on both tests ⇒ the segments straddle each
    // other ⇒ a proper interior crossing. Any zero means an endpoint lies
    // on the other segment (touching / collinear) — excluded.
    (d1 > 0.0) != (d2 > 0.0)
        && (d1 != 0.0 && d2 != 0.0)
        && (d3 > 0.0) != (d4 > 0.0)
        && (d3 != 0.0 && d4 != 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An X: two diagonals of a unit square cross once at the centre.
    #[test]
    fn tangle_metric_single_crossing() {
        let positions = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(0.0, 1.0),
            Vec2::new(1.0, 0.0),
        ];
        let edges = [(0, 1), (2, 3)];
        assert_eq!(edge_crossings(&positions, &edges), 1);
    }

    /// A planar config (the square's four sides) has zero crossings.
    #[test]
    fn tangle_metric_planar_is_zero() {
        let positions = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(0.0, 1.0),
        ];
        let edges = [(0, 1), (1, 2), (2, 3), (3, 0)];
        assert_eq!(edge_crossings(&positions, &edges), 0);
    }

    /// Edges that only share an endpoint do not count as a crossing.
    #[test]
    fn tangle_metric_shared_endpoint_not_a_crossing() {
        let positions = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(0.0, 1.0),
        ];
        // Two edges meeting at node 0.
        let edges = [(0, 1), (0, 2)];
        assert_eq!(edge_crossings(&positions, &edges), 0);
    }

    /// A grid of crossings: three horizontal lines vs three vertical lines
    /// produces 3×3 = 9 proper crossings.
    #[test]
    fn tangle_metric_grid_of_crossings() {
        let mut positions = Vec::new();
        // verticals at x = 0,1,2 spanning y = -1..3
        for x in 0..3 {
            positions.push(Vec2::new(x as f32, -1.0));
            positions.push(Vec2::new(x as f32, 3.0));
        }
        // horizontals at y = 0,1,2 spanning x = -1..3
        for y in 0..3 {
            positions.push(Vec2::new(-1.0, y as f32));
            positions.push(Vec2::new(3.0, y as f32));
        }
        let mut edges = Vec::new();
        for k in 0..3u32 {
            edges.push((k * 2, k * 2 + 1)); // verticals
        }
        for k in 0..3u32 {
            edges.push((6 + k * 2, 6 + k * 2 + 1)); // horizontals
        }
        assert_eq!(edge_crossings(&positions, &edges), 9);
    }

    /// Collinear overlapping segments are not a proper crossing.
    #[test]
    fn tangle_metric_collinear_overlap_not_crossing() {
        let positions = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(3.0, 0.0),
        ];
        let edges = [(0, 1), (2, 3)];
        assert_eq!(edge_crossings(&positions, &edges), 0);
    }

    #[test]
    fn total_edge_length_sums() {
        let positions = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(3.0, 4.0), // length 5
            Vec2::new(3.0, 0.0), // (0)->(2) length 3
        ];
        let edges = [(0, 1), (0, 2)];
        let len = total_edge_length(&positions, &edges);
        assert!((len - 8.0).abs() < 1e-5, "got {len}");
    }
}
