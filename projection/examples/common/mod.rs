//! Shared fixture + pipeline glue for the `hiker-projection` examples.
//!
//! This module is `#[path]`-included by both `snapshot.rs` (headless PNG) and
//! `demo.rs` (interactive eframe) so the two render the *same* hardcoded graph
//! through the *same* projection→fit math — the comparison between them is then
//! honest. It depends only on `hiker_projection`, nothing else.
//!
//! The kernel ([`hiker_projection::forward`] / [`magnification`] /
//! [`sample_geodesic`]) is the middle step; the per-surface affine (auto-fit)
//! lives *here*, in the caller, exactly as the crate's pipeline doc describes.

#![allow(dead_code)] // each example uses a subset of these helpers.

use hiker_projection::{
    Complex, ProjectionConfig, ProjectionKind, forward, magnification, sample_geodesic,
};

/// A node-and-edge mesh in world space.
pub struct Graph {
    /// World-space node positions.
    pub nodes: Vec<Complex>,
    /// Edges as `(from, to)` indices into `nodes`.
    pub edges: Vec<(usize, usize)>,
}

/// Side length of the lattice (7×7 nodes).
pub const GRID: usize = 7;
/// World spacing between adjacent lattice nodes.
pub const SPACING: f32 = 1.0;

/// Build the shared 7×7 lattice mesh, centred on the origin so world coords run
/// `-3.0..=3.0`. Edges connect each node to its right and down neighbour, so the
/// warp of a regular grid is plainly visible.
pub fn lattice() -> Graph {
    let mut nodes = Vec::with_capacity(GRID * GRID);
    let half = (GRID as f32 - 1.0) * 0.5; // 3.0 for a 7-wide grid.
    for row in 0..GRID {
        for col in 0..GRID {
            let x = col as f32 * SPACING - half;
            let y = row as f32 * SPACING - half;
            nodes.push(Complex::new(x, y));
        }
    }

    let idx = |row: usize, col: usize| row * GRID + col;
    let mut edges = Vec::new();
    for row in 0..GRID {
        for col in 0..GRID {
            if col + 1 < GRID {
                edges.push((idx(row, col), idx(row, col + 1))); // right neighbour
            }
            if row + 1 < GRID {
                edges.push((idx(row, col), idx(row + 1, col))); // down neighbour
            }
        }
    }

    Graph { nodes, edges }
}

/// The layout centroid — the projection focus. For this origin-centred lattice
/// it is the origin, but compute it honestly so the helper is reusable.
pub fn centroid(nodes: &[Complex]) -> Complex {
    if nodes.is_empty() {
        return Complex::ORIGIN;
    }
    let mut sum = Complex::ORIGIN;
    for &n in nodes {
        sum = sum + n;
    }
    sum.scale(1.0 / nodes.len() as f32)
}

/// The per-surface affine: a uniform scale `s` plus a translation that maps the
/// centre of the lensed bounding box to the viewport centre. This is the
/// caller-owned step; the kernel never sees screen coordinates.
#[derive(Clone, Copy)]
pub struct Fit {
    /// Lensed-space → screen scale.
    pub scale: f32,
    /// Centre of the lensed bounding box.
    pub lensed_center: Complex,
    /// Viewport centre in screen space.
    pub screen_center: Complex,
}

impl Fit {
    /// Map a lensed-space point through the affine to a screen point.
    /// Screen `y` grows downward, so the lensed `im` axis is flipped.
    pub fn to_screen(self, lensed: Complex) -> Complex {
        let dx = (lensed.re - self.lensed_center.re) * self.scale;
        let dy = (lensed.im - self.lensed_center.im) * self.scale;
        Complex::new(self.screen_center.re + dx, self.screen_center.im - dy)
    }
}

/// Compute the auto-fit affine for a viewport of `width`×`height` with a margin.
///
/// `lensed` is every node already pushed through [`forward`]. For Poincaré the
/// boundary should be included so the disk fits; pass the relevant points (the
/// snapshot/demo pass the lensed nodes and, for Poincaré, also `±1` corners via
/// [`bbox`] of an augmented list — here we just fit the given points).
pub fn fit(lensed: &[Complex], width: f32, height: f32, margin: f32) -> Fit {
    let (lo, hi) = bbox(lensed);
    let span_x = (hi.re - lo.re).max(1e-4);
    let span_y = (hi.im - lo.im).max(1e-4);
    let avail_w = (width - margin * 2.0).max(1.0);
    let avail_h = (height - margin * 2.0).max(1.0);
    let scale = (avail_w / span_x).min(avail_h / span_y);
    Fit {
        scale,
        lensed_center: Complex::new((lo.re + hi.re) * 0.5, (lo.im + hi.im) * 0.5),
        screen_center: Complex::new(width * 0.5, height * 0.5),
    }
}

/// Axis-aligned bounding box of a point set as `(min, max)`.
pub fn bbox(points: &[Complex]) -> (Complex, Complex) {
    let mut lo = Complex::new(f32::INFINITY, f32::INFINITY);
    let mut hi = Complex::new(f32::NEG_INFINITY, f32::NEG_INFINITY);
    for &p in points {
        lo.re = lo.re.min(p.re);
        lo.im = lo.im.min(p.im);
        hi.re = hi.re.max(p.re);
        hi.im = hi.im.max(p.im);
    }
    (lo, hi)
}

/// Push every world node through the lens relative to `focus`.
pub fn lens_nodes(nodes: &[Complex], focus: Complex, cfg: ProjectionConfig) -> Vec<Complex> {
    nodes
        .iter()
        .map(|&w| forward(w - focus, cfg))
        .collect()
}

/// Points to include in the auto-fit. For Poincaré we add the disk's `±1`
/// extent so the boundary circle is always inside the viewport; otherwise just
/// the lensed nodes.
pub fn fit_points(lensed: &[Complex], cfg: ProjectionConfig) -> Vec<Complex> {
    let mut pts = lensed.to_vec();
    if cfg.kind == ProjectionKind::Poincare {
        pts.push(Complex::new(-1.0, -1.0));
        pts.push(Complex::new(1.0, 1.0));
    }
    pts
}

/// Geometry for one edge, ready to draw: a screen-space polyline.
///
/// Only **Poincaré** edges follow geodesics — the arcs orthogonal to the unit
/// circle that play the role of straight lines in the disk, so they bow. Affine
/// and Fisheye edges are straight in lensed space: Affine because there is no
/// lens, Fisheye because its bulge already lives in the node remap (geodesic
/// arcs are a disk/hyperbolic primitive and would draw spurious spider-web bows
/// on the non-disk fisheye plane). For straight edges a 2-point line suffices.
pub fn edge_polyline(
    lensed_a: Complex,
    lensed_b: Complex,
    cfg: ProjectionConfig,
    affine: &Fit,
) -> Vec<Complex> {
    if cfg.kind == ProjectionKind::Poincare {
        sample_geodesic(lensed_a, lensed_b, cfg.geodesic_segments)
            .into_iter()
            .map(|p| affine.to_screen(p))
            .collect()
    } else {
        vec![affine.to_screen(lensed_a), affine.to_screen(lensed_b)]
    }
}

/// Screen radius of a node: `base` scaled by the local magnification, so centre
/// nodes are big and rim nodes shrink.
pub fn node_radius(lensed: Complex, base: f32, cfg: ProjectionConfig) -> f32 {
    (base * magnification(lensed, cfg)).max(0.5)
}

/// Per-point alpha multiplier (0..=1). For Poincaré this fades node/edge alpha
/// toward the rim via `magnification` (≈ 1 − |z|²); other modes stay opaque.
pub fn rim_alpha(lensed: Complex, cfg: ProjectionConfig) -> f32 {
    if cfg.kind == ProjectionKind::Poincare {
        magnification(lensed, cfg).clamp(0.0, 1.0)
    } else {
        1.0
    }
}
