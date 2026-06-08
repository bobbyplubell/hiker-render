//! A 2-D point doubling as a complex number — the shared currency of the
//! disk/Möbius/geodesic math. Single-precision (`f32`) to match the render
//! pipeline; conversions to/from the plain `[f32; 2]` / `(f32, f32)` pairs that
//! callers use for their own `Vec2`-like types are provided so no caller has to
//! depend on this crate's concrete type.

use core::ops::{Add, Div, Mul, Neg, Sub};

/// A point in the plane, interpreted as a complex number `re + i·im`.
///
/// Used both as a raw 2-D vector (world-relative offsets) and as a disk point
/// inside the unit circle by the Poincaré code.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Complex {
    pub re: f32,
    pub im: f32,
}

impl Complex {
    /// The complex number `re + i·im`.
    pub const fn new(re: f32, im: f32) -> Self {
        Self { re, im }
    }

    /// The origin `0 + 0i`.
    pub const ORIGIN: Self = Self { re: 0.0, im: 0.0 };

    /// Complex conjugate `re - i·im`.
    pub fn conjugate(self) -> Self {
        Self::new(self.re, -self.im)
    }

    /// Uniform scale by a real factor.
    pub fn scale(self, by: f32) -> Self {
        Self::new(self.re * by, self.im * by)
    }

    /// Squared magnitude `re² + im²` (cheaper than [`abs`](Self::abs)).
    pub fn abs2(self) -> f32 {
        self.re * self.re + self.im * self.im
    }

    /// Magnitude `√(re² + im²)`.
    pub fn abs(self) -> f32 {
        self.abs2().sqrt()
    }

    /// Argument `atan2(im, re)` — the angle from the positive real axis.
    pub fn arg(self) -> f32 {
        self.im.atan2(self.re)
    }
}

impl Add for Complex {
    type Output = Self;
    /// Component-wise sum.
    fn add(self, other: Self) -> Self {
        Self::new(self.re + other.re, self.im + other.im)
    }
}

impl Sub for Complex {
    type Output = Self;
    /// Component-wise difference `self - other`.
    fn sub(self, other: Self) -> Self {
        Self::new(self.re - other.re, self.im - other.im)
    }
}

impl Mul for Complex {
    type Output = Self;
    /// Complex product.
    fn mul(self, other: Self) -> Self {
        Self::new(
            self.re * other.re - self.im * other.im,
            self.re * other.im + self.im * other.re,
        )
    }
}

impl Div for Complex {
    type Output = Self;
    /// Complex quotient `self / other`.
    fn div(self, other: Self) -> Self {
        let denom = other.re * other.re + other.im * other.im;
        Self::new(
            (self.re * other.re + self.im * other.im) / denom,
            (self.im * other.re - self.re * other.im) / denom,
        )
    }
}

impl Neg for Complex {
    type Output = Self;
    /// Additive inverse `-self`.
    fn neg(self) -> Self {
        Self::new(-self.re, -self.im)
    }
}

impl From<[f32; 2]> for Complex {
    fn from(v: [f32; 2]) -> Self {
        Self::new(v[0], v[1])
    }
}

impl From<(f32, f32)> for Complex {
    fn from(v: (f32, f32)) -> Self {
        Self::new(v.0, v.1)
    }
}

impl From<Complex> for [f32; 2] {
    fn from(c: Complex) -> Self {
        [c.re, c.im]
    }
}

impl From<Complex> for (f32, f32) {
    fn from(c: Complex) -> Self {
        (c.re, c.im)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_matches_complex_algebra() {
        let a = Complex::new(2.0, 3.0);
        let b = Complex::new(-1.0, 4.0);
        assert_eq!(a + b, Complex::new(1.0, 7.0));
        assert_eq!(a - b, Complex::new(3.0, -1.0));
        // (2+3i)(-1+4i) = -2 + 8i - 3i + 12i² = -14 + 5i
        assert_eq!(a * b, Complex::new(-14.0, 5.0));
        assert_eq!(-a, Complex::new(-2.0, -3.0));
        assert_eq!(a.conjugate(), Complex::new(2.0, -3.0));
        assert_eq!(a.abs2(), 13.0);
    }

    #[test]
    fn div_is_inverse_of_mul() {
        let a = Complex::new(2.0, 3.0);
        let b = Complex::new(-1.0, 4.0);
        let q = (a * b) / b;
        assert!((q.re - a.re).abs() < 1e-5 && (q.im - a.im).abs() < 1e-5);
    }

    #[test]
    fn conversions_round_trip() {
        let c = Complex::new(1.5, -2.5);
        let arr: [f32; 2] = c.into();
        let tup: (f32, f32) = c.into();
        assert_eq!(Complex::from(arr), c);
        assert_eq!(Complex::from(tup), c);
    }
}
