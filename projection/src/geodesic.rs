//! Geodesics of the Poincaré disk — the arcs orthogonal to the unit circle that
//! play the role of straight lines. The view layers sample these to draw curved
//! ("bowed") edges between disk-projected nodes.

use crate::complex::Complex;
use core::f32::consts::PI;

/// The circle (in the plane) whose arc between two disk points is their geodesic.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circle {
    pub center: Complex,
    pub radius: f32,
}

/// The circle orthogonal to the unit circle that passes through `a` and `b`.
///
/// Returns `None` when `a`, `b` and the origin are collinear — the geodesic is
/// then a straight diameter, not an arc.
pub fn geodesic_circle(a: Complex, b: Complex) -> Option<Circle> {
    let rhs_a = (a.abs2() + 1.0) / 2.0;
    let rhs_b = (b.abs2() + 1.0) / 2.0;
    let det = a.re * b.im - a.im * b.re;

    if det.abs() < 1e-10 {
        return None;
    }

    let center = Complex::new(
        (rhs_a * b.im - a.im * rhs_b) / det,
        (a.re * rhs_b - rhs_a * b.re) / det,
    );
    let radius = (center.abs2() - 1.0).max(0.0).sqrt();

    Some(Circle { center, radius })
}

/// Sample the geodesic between `a` and `b` as a polyline of `segments + 1`
/// points, with the first exactly `a` and the last exactly `b`.
///
/// When the geodesic is a diameter (origin-collinear) the points interpolate the
/// straight chord; otherwise they walk the minor arc of [`geodesic_circle`].
pub fn sample_geodesic(a: Complex, b: Complex, segments: u32) -> Vec<Complex> {
    let steps = segments.max(1);

    let Some(circle) = geodesic_circle(a, b) else {
        let mut points = Vec::with_capacity(steps as usize + 1);
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            points.push(a + (b - a).scale(t));
        }
        return points;
    };

    let start_angle = (a - circle.center).arg();
    let end_angle = (b - circle.center).arg();
    let delta = normalize_angle(end_angle - start_angle);

    let mut points = Vec::with_capacity(steps as usize + 1);
    for i in 0..=steps {
        let angle = start_angle + delta * (i as f32 / steps as f32);
        points.push(Complex::new(
            circle.center.re + circle.radius * angle.cos(),
            circle.center.im + circle.radius * angle.sin(),
        ));
    }
    // Pin endpoints to the exact inputs so float drift never detaches an edge.
    points[0] = a;
    points[steps as usize] = b;
    points
}

/// Wrap an angle difference into `(-π, π]`, selecting the minor arc.
fn normalize_angle(angle: f32) -> f32 {
    let mut value = angle;
    while value <= -PI {
        value += 2.0 * PI;
    }
    while value > PI {
        value -= 2.0 * PI;
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_are_exact() {
        let a = Complex::new(0.2, 0.3);
        let b = Complex::new(-0.5, 0.1);
        let pts = sample_geodesic(a, b, 16);
        assert_eq!(pts.len(), 17);
        assert_eq!(pts[0], a);
        assert_eq!(pts[16], b);
    }

    #[test]
    fn origin_collinear_pair_is_a_straight_diameter() {
        // a and b on the same ray through the origin → no geodesic circle.
        let a = Complex::new(0.1, 0.2);
        let b = Complex::new(-0.3, -0.6);
        assert!(geodesic_circle(a, b).is_none());

        let pts = sample_geodesic(a, b, 12);
        // Every sampled point must be collinear with a and b: cross product ~0.
        let dir = b - a;
        for p in &pts {
            let rel = *p - a;
            let cross = dir.re * rel.im - dir.im * rel.re;
            assert!(cross.abs() < 1e-5, "point {p:?} off the chord (cross={cross})");
        }
    }

    #[test]
    fn non_collinear_pair_bows_off_the_chord() {
        let a = Complex::new(0.5, 0.0);
        let b = Complex::new(0.0, 0.5);
        assert!(geodesic_circle(a, b).is_some());

        let pts = sample_geodesic(a, b, 16);
        let dir = b - a;
        // At least one interior point must be measurably off the straight chord.
        let max_off = pts[1..pts.len() - 1]
            .iter()
            .map(|p| {
                let rel = *p - a;
                (dir.re * rel.im - dir.im * rel.re).abs()
            })
            .fold(0.0_f32, f32::max);
        assert!(max_off > 1e-3, "arc did not bow (max offset {max_off})");
    }
}
