//! Standalone integration showcase for `hiker-projection` on top of egui.
//!
//! This file is a self-contained *template*: it shows how to drop the
//! `hiker-projection` lens into your own egui app, and depends only on
//! `hiker-projection` + `eframe`/`egui` — not on hiker. The recipe is:
//!
//!   * subtract a focus, then call `forward(world − focus, cfg)` per point to get
//!     the "lensed" coordinate (the kernel's job);
//!   * own the per-surface affine yourself (here: an auto-fit that maps the
//!     lensed bounding box into the viewport) — the kernel never sees pixels;
//!   * size each node by `magnification(lensed, cfg)` so the focus area is
//!     magnified and the rim shrinks;
//!   * draw edges straight under Affine/Fisheye, or as a `sample_geodesic(...)`
//!     polyline under Poincaré so they bow like hyperbolic lines;
//!   * for Poincaré, draw the unit-disk boundary (radius 1 through the affine)
//!     and fade alpha toward the rim by `magnification` (≈ 1 − |z|²).
//!
//! Every left-panel control is wired to a real `ProjectionConfig` field (nothing
//! cosmetic). The focus follows the cursor while hovering the canvas — pan the
//! lens around and watch the lattice warp under it — and falls back to the
//! layout centroid otherwise.
//!
//! Run: `cargo run -p hiker-projection --example demo`

#[path = "common/mod.rs"]
mod common;

use common::{centroid, edge_polyline, fit, fit_points, lattice, lens_nodes, node_radius, rim_alpha};
use eframe::egui;
use hiker_projection::{Complex, ProjectionConfig, ProjectionKind};

/// A label for each lens mode, including the user-facing "Off" for Affine.
fn mode_label(kind: ProjectionKind) -> &'static str {
    match kind {
        ProjectionKind::Affine => "Off (Affine)",
        ProjectionKind::Fisheye => "Fisheye",
        ProjectionKind::Poincare => "Poincaré",
    }
}

/// egui colours for the demo.
const EDGE: egui::Color32 = egui::Color32::from_rgb(0x6c, 0x9b, 0xd6);
const NODE: egui::Color32 = egui::Color32::from_rgb(0xe6, 0xc4, 0x4d);
const BOUNDARY: egui::Color32 = egui::Color32::from_rgb(0x6c, 0xa0, 0x6c);
const BASE_RADIUS: f32 = 9.0;
const MARGIN: f32 = 40.0;

struct DemoApp {
    cfg: ProjectionConfig,
    /// Whether to draw the Poincaré unit-disk boundary circle.
    show_boundary: bool,
}

impl Default for DemoApp {
    fn default() -> Self {
        Self {
            cfg: ProjectionConfig {
                kind: ProjectionKind::Poincare,
                strength: 1.0,
                size_falloff: 1.0,
                geodesic_segments: 24,
            },
            show_boundary: true,
        }
    }
}

/// Convert our `Complex` screen point to an egui `Pos2`.
fn pos2(c: Complex) -> egui::Pos2 {
    egui::pos2(c.re, c.im)
}

/// Apply an alpha multiplier to an egui colour.
fn faded(color: egui::Color32, alpha: f32) -> egui::Color32 {
    let a = (255.0 * alpha.clamp(0.0, 1.0)).round() as u8;
    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a)
}

impl eframe::App for DemoApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::SidePanel::left("controls")
            .resizable(false)
            .default_width(220.0)
            .show(ctx, |ui| {
                ui.heading("hiker-projection");
                ui.label("Live lens over a 7×7 lattice.");
                ui.separator();

                ui.label("Mode");
                for kind in [
                    ProjectionKind::Affine,
                    ProjectionKind::Fisheye,
                    ProjectionKind::Poincare,
                ] {
                    ui.radio_value(&mut self.cfg.kind, kind, mode_label(kind));
                }
                ui.separator();

                ui.add(
                    egui::Slider::new(&mut self.cfg.strength, 0.1..=3.0).text("strength (k)"),
                );
                ui.add(
                    egui::Slider::new(&mut self.cfg.size_falloff, 0.0..=1.0).text("size_falloff"),
                );
                ui.add(
                    egui::Slider::new(&mut self.cfg.geodesic_segments, 2..=64)
                        .text("geodesic_segments"),
                );
                ui.separator();

                ui.checkbox(&mut self.show_boundary, "Show disk boundary (Poincaré)");
                ui.separator();
                ui.label("Hover the canvas: the focus follows the cursor.");
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            let painter = ui.painter();
            let rect = ui.max_rect();
            let cfg = self.cfg;

            let graph = lattice();

            // Focus follows the cursor when hovering, else the layout centroid.
            // The cursor is in *screen* space; convert it back into world space
            // so the lens centres exactly under the pointer. We do a cheap
            // first-pass fit on the centroid to derive that inverse mapping.
            let default_focus = centroid(&graph.nodes);
            let focus = pointer_world(ui, rect, &graph.nodes, default_focus, cfg)
                .unwrap_or(default_focus);

            // 1. lens every node relative to the focus.
            let lensed = lens_nodes(&graph.nodes, focus, cfg);
            // 2. own the affine: auto-fit the lensed extent into this viewport.
            let fit_pts = fit_points(&lensed, cfg);
            let affine = fit_into_rect(&fit_pts, rect);

            // 5 (Poincaré): the unit-disk boundary, mapped through the affine.
            if cfg.kind == ProjectionKind::Poincare && self.show_boundary {
                let center = pos2(affine.to_screen(Complex::ORIGIN));
                painter.circle_stroke(
                    center,
                    affine.scale, // disk radius 1.0 → `scale` px.
                    egui::Stroke::new(1.5, BOUNDARY),
                );
            }

            // 4. edges: straight (Affine/Fisheye) or geodesic polyline (Poincaré).
            for &(a, b) in &graph.edges {
                let pts: Vec<egui::Pos2> = edge_polyline(lensed[a], lensed[b], cfg, &affine)
                    .into_iter()
                    .map(pos2)
                    .collect();
                let alpha = rim_alpha(lensed[a], cfg).min(rim_alpha(lensed[b], cfg));
                painter.add(egui::Shape::line(
                    pts,
                    egui::Stroke::new(1.5, faded(EDGE, alpha)),
                ));
            }

            // 3. nodes: radius by magnification, alpha faded toward the rim.
            for &l in &lensed {
                let screen = pos2(affine.to_screen(l));
                let r = node_radius(l, BASE_RADIUS, cfg);
                painter.circle_filled(screen, r, faded(NODE, rim_alpha(l, cfg)));
            }

            painter.text(
                rect.left_top() + egui::vec2(8.0, 8.0),
                egui::Align2::LEFT_TOP,
                mode_label(cfg.kind),
                egui::FontId::proportional(16.0),
                egui::Color32::from_gray(0xc6),
            );
        });

        // Keep repainting so the cursor-driven focus stays live.
        ctx.request_repaint();
    }
}

/// Build the auto-fit affine for an egui `Rect` (rather than a bare size). Same
/// math as `common::fit` but anchored at the rect's centre.
fn fit_into_rect(lensed: &[Complex], rect: egui::Rect) -> common::Fit {
    let mut f = fit(lensed, rect.width(), rect.height(), MARGIN);
    // `fit` centres on (w/2, h/2); shift into the rect's actual position.
    f.screen_center = Complex::new(rect.center().x, rect.center().y);
    f
}

/// Recover the world-space point under the cursor so the lens can centre there.
///
/// We can't invert the auto-fit before we know the focus (the fit depends on the
/// lens, which depends on the focus). So we approximate: do a one-shot fit using
/// the centroid as focus, invert *that* affine + lens to map the cursor screen
/// point back to world. Good enough for an interactive focus that the user is
/// steering by eye.
fn pointer_world(
    ui: &egui::Ui,
    rect: egui::Rect,
    nodes: &[Complex],
    default_focus: Complex,
    cfg: ProjectionConfig,
) -> Option<Complex> {
    let pointer = ui.ctx().pointer_hover_pos()?;
    if !rect.contains(pointer) {
        return None;
    }
    // Fit once around the centroid focus.
    let lensed = lens_nodes(nodes, default_focus, cfg);
    let fit_pts = fit_points(&lensed, cfg);
    let affine = fit_into_rect(&fit_pts, rect);
    // Invert the affine: screen → lensed (y flipped, uniform scale).
    let lensed_pt = Complex::new(
        affine.lensed_center.re + (pointer.x - affine.screen_center.re) / affine.scale,
        affine.lensed_center.im - (pointer.y - affine.screen_center.im) / affine.scale,
    );
    // Invert the lens: lensed → world-relative-focus, then add the focus back.
    Some(hiker_projection::inverse(lensed_pt, cfg) + default_focus)
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([960.0, 680.0]),
        ..Default::default()
    };
    eframe::run_native(
        "hiker-projection demo",
        options,
        Box::new(|_cc| Ok(Box::<DemoApp>::default())),
    )
}
