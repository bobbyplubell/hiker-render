//! Egui-free math kernel for hyperbolic/fisheye projection.
//!
//! This crate is the swappable **middle** step of the render pipeline:
//!
//! ```text
//!   world − focus  ──▶  [ projection lens ]  ──▶  per-surface affine  ──▶  screen
//!                       (this crate)             (caller's job)
//! ```
//!
//! It owns only the *lens* math — the radial remap from a focus-relative world
//! offset to a "lensed" coordinate — plus the disk primitives the view layers
//! consume directly: [`Complex`], disk-preserving [`Mobius`] transforms, and
//! geodesic sampling. It does **no** affine/screen mapping, no rendering, no
//! egui, and has zero dependencies.
//!
//! The seam is three closed-form functions parameterised by [`ProjectionConfig`]:
//! [`forward`] (world→lens), [`inverse`] (lens→world, exact inverse of
//! `forward`), and [`magnification`] (local linear scale at a lensed point).

mod complex;
mod geodesic;
mod mobius;

pub use complex::Complex;
pub use geodesic::{Circle, geodesic_circle, sample_geodesic};
pub use mobius::Mobius;

/// Clamp helper: a strictly-inside-disk point. Non-finite → origin; already
/// inside → unchanged; otherwise scaled onto `boundary_radius`. The default
/// boundary sits a hair inside `1.0` so the result is always a legal Möbius
/// argument.
pub fn clamp_inside_disk(z: Complex, boundary_radius: f32) -> Complex {
    if !z.re.is_finite() || !z.im.is_finite() {
        return Complex::ORIGIN;
    }
    let r2 = z.abs2();
    if r2 < 1.0 {
        return z;
    }
    z.scale(boundary_radius / r2.sqrt())
}

/// Default boundary radius for [`clamp_inside_disk`].
pub const DEFAULT_BOUNDARY_RADIUS: f32 = 0.999_999;

/// Clamp helper: keep a point within `√max_r2` of the origin, leaving interior
/// points untouched. Used to keep layouts off the rim where the lens blows up.
pub fn clamp_disk(z: Complex, max_r2: f32) -> Complex {
    if z.abs2() <= max_r2 {
        return z;
    }
    z.scale(max_r2.sqrt() / z.abs())
}

/// Which lens the projection applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ProjectionKind {
    /// No lens — `forward`/`inverse` are the identity, `magnification` is `1`.
    #[default]
    Affine,
    /// Radial `tanh` compression that keeps direction; the whole plane warps
    /// toward the focus but is not confined to the unit disk.
    Fisheye,
    /// Poincaré-disk lens: the focus-relative plane is compressed into `|z| < 1`
    /// and magnification follows the conformal factor `(1 − |z|²)`.
    Poincare,
}

/// Math-relevant configuration for the projection seam.
///
/// UI-only parameters (minimap, fly-to, etc.) live app-side and are deliberately
/// not represented here.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectionConfig {
    /// Which lens to apply.
    pub kind: ProjectionKind,
    /// `k`: how aggressively distance maps to lensed radius (the `tanh(k·r)`
    /// rate / disk-spread). Larger `k` packs more world into the centre.
    pub strength: f32,
    /// Blend between uniform size (`0`) and the full conformal size response
    /// (`1`) returned by [`magnification`].
    pub size_falloff: f32,
    /// How many segments [`sample_geodesic`] uses for geodesic edges.
    pub geodesic_segments: u32,
}

impl Default for ProjectionConfig {
    fn default() -> Self {
        Self {
            kind: ProjectionKind::Affine,
            strength: 1.0,
            size_falloff: 1.0,
            geodesic_segments: 24,
        }
    }
}

/// Smallest radius below which a point is treated as the centre, avoiding a
/// divide-by-zero when re-deriving a direction.
const RADIAL_EPSILON: f32 = 1e-9;

/// Largest lensed radius the disk inverse will trust before clamping, so
/// `atanh` never hits its `±1` singularity.
const MAX_LENSED_RADIUS: f32 = 0.999_999;

/// Apply the lens: map a focus-relative world offset to its lensed coordinate
/// (pre-affine). The focus has already been subtracted by the caller, so the
/// lens is centred on the origin.
///
/// - [`Affine`](ProjectionKind::Affine): identity.
/// - [`Fisheye`](ProjectionKind::Fisheye) / [`Poincare`](ProjectionKind::Poincare):
///   radial remap `r' = tanh(strength · r)`, direction preserved. For Poincaré
///   the result lies in the open unit disk; Möbius re-centring (a separate
///   navigation concern) is applied by callers on top of this.
pub fn forward(world_rel_focus: Complex, cfg: ProjectionConfig) -> Complex {
    match cfg.kind {
        ProjectionKind::Affine => world_rel_focus,
        ProjectionKind::Fisheye | ProjectionKind::Poincare => {
            radial_remap(world_rel_focus, |r| (cfg.strength * r).tanh())
        }
    }
}

/// Closed-form inverse of [`forward`]: recover the focus-relative world offset
/// from a lensed coordinate. Satisfies `inverse(forward(z)) ≈ z`.
///
/// - [`Affine`](ProjectionKind::Affine): identity.
/// - [`Fisheye`](ProjectionKind::Fisheye) / [`Poincare`](ProjectionKind::Poincare):
///   `r = atanh(r') / strength` (the lensed radius is clamped just inside `1`).
pub fn inverse(lensed: Complex, cfg: ProjectionConfig) -> Complex {
    match cfg.kind {
        ProjectionKind::Affine => lensed,
        ProjectionKind::Fisheye | ProjectionKind::Poincare => {
            radial_remap(lensed, |r| {
                let clamped = r.min(MAX_LENSED_RADIUS);
                atanh(clamped) / cfg.strength
            })
        }
    }
}

/// Local linear scale factor at a lensed point, blended by `size_falloff`.
///
/// The raw factor is `1` at the centre and decreases monotonically toward the
/// rim: Poincaré uses the conformal factor `(1 − |z|²)`; Fisheye uses the
/// (normalised) radial derivative of its remap, also `(1 − r'²)`; Affine is `1`.
/// The blend `1 + size_falloff·(raw − 1)` gives uniform `1` at `size_falloff = 0`
/// and the full factor at `size_falloff = 1`.
pub fn magnification(lensed: Complex, cfg: ProjectionConfig) -> f32 {
    let raw = match cfg.kind {
        ProjectionKind::Affine => 1.0,
        // d/dr tanh(k·r) = k·(1 − tanh²(k·r)) = k·(1 − r'²); normalising by its
        // centre value k leaves (1 − r'²), matching the conformal factor.
        ProjectionKind::Fisheye | ProjectionKind::Poincare => (1.0 - lensed.abs2()).max(0.0),
    };
    1.0 + cfg.size_falloff * (raw - 1.0)
}

/// Apply a scalar radius remap while preserving direction.
fn radial_remap(z: Complex, remap: impl Fn(f32) -> f32) -> Complex {
    let r = z.abs();
    if r < RADIAL_EPSILON {
        return Complex::ORIGIN;
    }
    let new_r = remap(r);
    z.scale(new_r / r)
}

/// `atanh(x)` — std `f32` has no inverse hyperbolic tangent, so hand-roll it.
fn atanh(x: f32) -> f32 {
    0.5 * ((1.0 + x) / (1.0 - x)).ln()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic spread of test points (no rng).
    fn grid() -> Vec<Complex> {
        let mut out = Vec::new();
        for i in -4..=4 {
            for j in -4..=4 {
                out.push(Complex::new(i as f32 * 0.5, j as f32 * 0.7));
            }
        }
        out
    }

    #[test]
    fn forward_inverse_round_trip() {
        for kind in [ProjectionKind::Fisheye, ProjectionKind::Poincare] {
            let cfg = ProjectionConfig {
                kind,
                strength: 0.8,
                ..Default::default()
            };
            for z in grid() {
                let back = inverse(forward(z, cfg), cfg);
                assert!(
                    (back.re - z.re).abs() < 1e-3 && (back.im - z.im).abs() < 1e-3,
                    "{kind:?}: {z:?} -> {back:?}"
                );
            }
        }
    }

    #[test]
    fn affine_is_identity() {
        let cfg = ProjectionConfig::default();
        let z = Complex::new(3.0, -2.0);
        assert_eq!(forward(z, cfg), z);
        assert_eq!(inverse(z, cfg), z);
        assert_eq!(magnification(z, cfg), 1.0);
    }

    #[test]
    fn magnification_is_monotonic_non_increasing_to_rim() {
        for kind in [ProjectionKind::Fisheye, ProjectionKind::Poincare] {
            let cfg = ProjectionConfig {
                kind,
                strength: 1.0,
                size_falloff: 1.0,
                ..Default::default()
            };
            // Walk a ray from centre toward the rim in lensed space.
            let dir = Complex::new(0.6, 0.8); // unit length
            let mut prev = f32::INFINITY;
            for i in 0..=50 {
                let r = i as f32 / 50.0 * 0.99;
                let m = magnification(dir.scale(r), cfg);
                assert!(m <= prev + 1e-6, "{kind:?}: rose at r={r}: {m} > {prev}");
                prev = m;
            }
        }
    }

    #[test]
    fn size_falloff_blends_to_uniform() {
        let lensed = Complex::new(0.5, 0.3);
        let cfg = ProjectionConfig {
            kind: ProjectionKind::Poincare,
            size_falloff: 0.0,
            ..Default::default()
        };
        assert!((magnification(lensed, cfg) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn clamps_keep_points_legal() {
        // Non-finite -> origin.
        assert_eq!(
            clamp_inside_disk(Complex::new(f32::NAN, 0.0), DEFAULT_BOUNDARY_RADIUS),
            Complex::ORIGIN
        );
        // Outside the disk -> pulled onto the boundary.
        let outside = clamp_inside_disk(Complex::new(2.0, 0.0), DEFAULT_BOUNDARY_RADIUS);
        assert!(outside.abs() < 1.0);
        // clamp_disk leaves interior points alone, pulls outer ones in.
        let inside = Complex::new(0.1, 0.1);
        assert_eq!(clamp_disk(inside, 0.93), inside);
        assert!(clamp_disk(Complex::new(2.0, 0.0), 0.93).abs2() <= 0.93 + 1e-6);
    }
}
