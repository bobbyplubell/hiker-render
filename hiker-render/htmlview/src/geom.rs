//! Geometry helpers.
//!
//! We lean on `egui::{Vec2, Pos2, Rect}` for the bulk of geometry; this module
//! adds the small extras the layout/paint code needs (currently just `Edges`).

pub use egui::{Pos2, Rect, Vec2};

/// Four-sided box quantity (margin / padding / border width, etc.).
///
/// Generic over the stored type so it can hold resolved pixel lengths
/// (`Edges<f32>`), colors (`Edges<Color>`), border styles, and so on.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Edges<T> {
    pub top: T,
    pub right: T,
    pub bottom: T,
    pub left: T,
}

impl<T> Edges<T> {
    pub const fn new(top: T, right: T, bottom: T, left: T) -> Self {
        Self {
            top,
            right,
            bottom,
            left,
        }
    }

    /// All four sides set to the same value.
    pub fn splat(v: T) -> Self
    where
        T: Clone,
    {
        Self {
            top: v.clone(),
            right: v.clone(),
            bottom: v.clone(),
            left: v,
        }
    }

    /// Apply a function to each side, producing a new `Edges`.
    pub fn map<U>(self, mut f: impl FnMut(T) -> U) -> Edges<U> {
        Edges {
            top: f(self.top),
            right: f(self.right),
            bottom: f(self.bottom),
            left: f(self.left),
        }
    }
}

impl Edges<f32> {
    pub const ZERO: Edges<f32> = Edges {
        top: 0.0,
        right: 0.0,
        bottom: 0.0,
        left: 0.0,
    };

    /// Total horizontal extent (`left + right`).
    pub fn horizontal(&self) -> f32 {
        self.left + self.right
    }

    /// Total vertical extent (`top + bottom`).
    pub fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}
