//! Disk-preserving Möbius transforms — the hyperbolic isometries of the Poincaré
//! disk. These are the navigation primitive: pan/recentre/fly-to in the disk are
//! all expressed as a [`Mobius`] applied to every disk point. Public so the
//! graph/canvas view layers can drive Möbius pan directly.

use crate::complex::Complex;

/// A disk-preserving Möbius transform of the form
/// `z ↦ (a·z + b) / (conj(b)·z + conj(a))`.
///
/// With `|a|² − |b|² = 1` this is an isometry of the Poincaré disk; the
/// constructors here all produce such transforms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mobius {
    pub a: Complex,
    pub b: Complex,
}

impl Mobius {
    /// The identity transform `z ↦ z`.
    pub const fn identity() -> Self {
        Self {
            a: Complex::new(1.0, 0.0),
            b: Complex::new(0.0, 0.0),
        }
    }

    /// Apply the transform to a disk point.
    pub fn apply(self, z: Complex) -> Complex {
        let numerator = self.a * z + self.b;
        let denominator = self.b.conjugate() * z + self.a.conjugate();
        numerator / denominator
    }

    /// Compose two transforms into one. The right operand acts first:
    /// `compose(first, second).apply(z) == first.apply(second.apply(z))`
    /// (matrix-product order). Hence [`from_point_pair`](Self::from_point_pair)
    /// builds `compose(translate_q, translate_p)` so `translate_p` runs first.
    pub fn compose(first: Self, second: Self) -> Self {
        Self {
            a: first.a * second.a + first.b * second.b.conjugate(),
            b: first.a * second.b + first.b * second.a.conjugate(),
        }
    }

    /// The inverse transform.
    pub fn invert(self) -> Self {
        Self {
            a: self.a.conjugate(),
            b: -self.b,
        }
    }

    /// A pure rotation about the disk centre by `angle` radians — a hyperbolic
    /// isometry that spins the whole disk without changing any distance.
    pub fn rotation(angle: f32) -> Self {
        Self {
            a: Complex::new(angle.cos(), angle.sin()),
            b: Complex::new(0.0, 0.0),
        }
    }

    /// The isometry that re-centres the disk so `p` maps to `q`: it first sends
    /// `p` to the origin, then sends the origin to `q`.
    pub fn from_point_pair(p: Complex, q: Complex) -> Self {
        let p_scale = 1.0 / (1.0 - p.abs2()).max(1e-12).sqrt();
        let translate_p = Self {
            a: Complex::new(p_scale, 0.0),
            b: (-p).scale(p_scale),
        };

        let q_scale = 1.0 / (1.0 - q.abs2()).max(1e-12).sqrt();
        let translate_q = Self {
            a: Complex::new(q_scale, 0.0),
            b: q.scale(q_scale),
        };

        Self::compose(translate_q, translate_p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Complex, b: Complex) -> bool {
        (a.re - b.re).abs() < 1e-4 && (a.im - b.im).abs() < 1e-4
    }

    #[test]
    fn identity_is_identity() {
        let z = Complex::new(0.3, -0.4);
        assert!(close(Mobius::identity().apply(z), z));
    }

    #[test]
    fn from_point_pair_maps_p_to_q() {
        let p = Complex::new(0.2, -0.1);
        let q = Complex::new(-0.4, 0.3);
        let m = Mobius::from_point_pair(p, q);
        assert!(close(m.apply(p), q));
    }

    #[test]
    fn compose_with_inverse_is_identity() {
        let t = Mobius::from_point_pair(Complex::new(0.15, 0.25), Complex::new(-0.3, 0.1));
        let round = Mobius::compose(t, t.invert());
        for &z in &[
            Complex::new(0.0, 0.0),
            Complex::new(0.5, 0.1),
            Complex::new(-0.2, -0.6),
        ] {
            assert!(close(round.apply(z), z));
        }
    }
}
