//! Minimal 2-D float vector used by the graph layout engines.
//!
//! This crate is **egui-agnostic**, so we carry our own `Vec2` rather than
//! depending on `eframe::egui::Vec2`. The API mirrors the subset of egui's
//! vector that the force / tree layouts rely on: the four arithmetic ops,
//! their assignment variants, scalar multiply/divide, and length helpers,
//! plus the obvious tuple/array conversions so callers can hand positions in
//! and out without ceremony.

use std::ops::{Add, AddAssign, Div, Mul, Sub, SubAssign};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

    #[inline]
    pub const fn new(x: f32, y: f32) -> Self {
        Vec2 { x, y }
    }

    #[inline]
    pub fn length(&self) -> f32 {
        self.length_sq().sqrt()
    }

    #[inline]
    pub fn length_sq(&self) -> f32 {
        self.x * self.x + self.y * self.y
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    #[inline]
    fn add(self, rhs: Vec2) -> Vec2 {
        Vec2::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub for Vec2 {
    type Output = Vec2;
    #[inline]
    fn sub(self, rhs: Vec2) -> Vec2 {
        Vec2::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl Mul<f32> for Vec2 {
    type Output = Vec2;
    #[inline]
    fn mul(self, rhs: f32) -> Vec2 {
        Vec2::new(self.x * rhs, self.y * rhs)
    }
}

impl Div<f32> for Vec2 {
    type Output = Vec2;
    #[inline]
    fn div(self, rhs: f32) -> Vec2 {
        Vec2::new(self.x / rhs, self.y / rhs)
    }
}

impl AddAssign for Vec2 {
    #[inline]
    fn add_assign(&mut self, rhs: Vec2) {
        self.x += rhs.x;
        self.y += rhs.y;
    }
}

impl SubAssign for Vec2 {
    #[inline]
    fn sub_assign(&mut self, rhs: Vec2) {
        self.x -= rhs.x;
        self.y -= rhs.y;
    }
}

impl From<(f32, f32)> for Vec2 {
    #[inline]
    fn from((x, y): (f32, f32)) -> Self {
        Vec2::new(x, y)
    }
}

impl From<[f32; 2]> for Vec2 {
    #[inline]
    fn from(a: [f32; 2]) -> Self {
        Vec2::new(a[0], a[1])
    }
}

impl From<Vec2> for [f32; 2] {
    #[inline]
    fn from(v: Vec2) -> [f32; 2] {
        [v.x, v.y]
    }
}

impl From<Vec2> for (f32, f32) {
    #[inline]
    fn from(v: Vec2) -> (f32, f32) {
        (v.x, v.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_ops() {
        let a = Vec2::new(1.0, 2.0);
        let b = Vec2::new(3.0, 4.0);
        assert_eq!(a + b, Vec2::new(4.0, 6.0));
        assert_eq!(b - a, Vec2::new(2.0, 2.0));
        assert_eq!(a * 2.0, Vec2::new(2.0, 4.0));
        assert_eq!(b / 2.0, Vec2::new(1.5, 2.0));
    }

    #[test]
    fn assign_ops() {
        let mut v = Vec2::new(1.0, 1.0);
        v += Vec2::new(2.0, 3.0);
        assert_eq!(v, Vec2::new(3.0, 4.0));
        v -= Vec2::new(1.0, 2.0);
        assert_eq!(v, Vec2::new(2.0, 2.0));
    }

    #[test]
    fn lengths() {
        let v = Vec2::new(3.0, 4.0);
        assert_eq!(v.length_sq(), 25.0);
        assert_eq!(v.length(), 5.0);
        assert_eq!(Vec2::ZERO.length(), 0.0);
    }

    #[test]
    fn conversions_round_trip() {
        let v = Vec2::new(7.0, -2.0);
        let t: (f32, f32) = v.into();
        assert_eq!(t, (7.0, -2.0));
        assert_eq!(Vec2::from(t), v);

        let a: [f32; 2] = v.into();
        assert_eq!(a, [7.0, -2.0]);
        assert_eq!(Vec2::from(a), v);
    }
}
