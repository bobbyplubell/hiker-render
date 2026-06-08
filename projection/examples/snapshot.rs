//! Headless, deterministic PNG snapshot of the `hiker-projection` lens.
//!
//! Renders one shared 7×7 lattice mesh (see `common`) through the three
//! projection modes — Affine / Fisheye / Poincaré — to three PNGs plus a 3-up
//! comparison strip, using pure CPU pixel rasterization (the `image` crate,
//! no egui, no GPU) so it runs in a headless CI box. The rasterization style
//! (RgbaImage + alpha-blended `put_px` + Bresenham-ish line loop + filled-circle
//! loop) mirrors `tools/graph-snapshot`.
//!
//! Run: `cargo run -p hiker-projection --example snapshot`

#[path = "common/mod.rs"]
mod common;

use std::path::{Path, PathBuf};

use common::{
    Fit, centroid, edge_polyline, fit, fit_points, lattice, lens_nodes, node_radius, rim_alpha,
};
use hiker_projection::{Complex, ProjectionConfig, ProjectionKind};
use image::{Rgba, RgbaImage};

const WIDTH: u32 = 600;
const HEIGHT: u32 = 600;
const MARGIN: f32 = 50.0;
const BG: Rgba<u8> = Rgba([0x14, 0x18, 0x1d, 0xff]);
const EDGE: Rgba<u8> = Rgba([0x6c, 0x9b, 0xd6, 0xff]);
const NODE: Rgba<u8> = Rgba([0xe6, 0xc4, 0x4d, 0xff]);
const BOUNDARY: Rgba<u8> = Rgba([0x55, 0x88, 0x55, 0xff]);
const BASE_RADIUS: f32 = 9.0;

// ── Minimal pixel ops (mirrors graph-snapshot) ──────────────────────────────

/// Alpha-blend `color` over the existing pixel at `(x, y)`; clip out-of-bounds.
fn put_px(img: &mut RgbaImage, x: i32, y: i32, color: Rgba<u8>) {
    if x < 0 || y < 0 || x as u32 >= img.width() || y as u32 >= img.height() {
        return;
    }
    let dst = img.get_pixel_mut(x as u32, y as u32);
    let a = color[3] as f32 / 255.0;
    let inv = 1.0 - a;
    for c in 0..3 {
        dst[c] = ((color[c] as f32) * a + (dst[c] as f32) * inv) as u8;
    }
    dst[3] = 0xff;
}

/// Draw a straight segment between two screen points by stepping one pixel at a
/// time along the longer axis.
fn line(img: &mut RgbaImage, a: Complex, b: Complex, color: Rgba<u8>) {
    let dx = b.re - a.re;
    let dy = b.im - a.im;
    let steps = dx.abs().max(dy.abs()).ceil() as i32;
    if steps <= 0 {
        put_px(img, a.re.round() as i32, a.im.round() as i32, color);
        return;
    }
    for s in 0..=steps {
        let t = s as f32 / steps as f32;
        let x = (a.re + dx * t).round() as i32;
        let y = (a.im + dy * t).round() as i32;
        put_px(img, x, y, color);
    }
}

/// Draw a polyline (a sequence of screen points) as connected segments.
fn polyline(img: &mut RgbaImage, points: &[Complex], color: Rgba<u8>) {
    for pair in points.windows(2) {
        line(img, pair[0], pair[1], color);
    }
}

/// Draw a filled disk of radius `r` centred at `c`.
fn disk(img: &mut RgbaImage, c: Complex, r: f32, color: Rgba<u8>) {
    let r2 = r * r;
    let x0 = (c.re - r).floor() as i32;
    let x1 = (c.re + r).ceil() as i32;
    let y0 = (c.im - r).floor() as i32;
    let y1 = (c.im + r).ceil() as i32;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = x as f32 + 0.5 - c.re;
            let dy = y as f32 + 0.5 - c.im;
            if dx * dx + dy * dy <= r2 {
                put_px(img, x, y, color);
            }
        }
    }
}

/// Draw a 1px-thick circle outline (for the Poincaré disk boundary).
fn circle_outline(img: &mut RgbaImage, c: Complex, r: f32, color: Rgba<u8>) {
    let steps = ((2.0 * std::f32::consts::PI * r).ceil() as i32).max(64);
    let mut prev: Option<Complex> = None;
    for i in 0..=steps {
        let t = i as f32 / steps as f32 * std::f32::consts::TAU;
        let p = Complex::new(c.re + r * t.cos(), c.im + r * t.sin());
        if let Some(q) = prev {
            line(img, q, p, color);
        }
        prev = Some(p);
    }
}

/// Apply an alpha multiplier to a colour.
fn faded(color: Rgba<u8>, alpha: f32) -> Rgba<u8> {
    let a = (color[3] as f32 * alpha.clamp(0.0, 1.0)).round() as u8;
    Rgba([color[0], color[1], color[2], a])
}

// ── Rendering ───────────────────────────────────────────────────────────────

/// Render the shared lattice in one projection mode into a fresh image.
fn render(cfg: ProjectionConfig) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(WIDTH, HEIGHT, BG);
    let graph = lattice();
    let focus = centroid(&graph.nodes);

    // 1. lens every world node relative to the focus.
    let lensed = lens_nodes(&graph.nodes, focus, cfg);
    // 2. auto-fit affine over the lensed extent (+ disk for Poincaré).
    let fit_pts = fit_points(&lensed, cfg);
    let affine = fit(&fit_pts, WIDTH as f32, HEIGHT as f32, MARGIN);

    // 5 (Poincaré only): the unit-disk boundary, mapped through the affine.
    if cfg.kind == ProjectionKind::Poincare {
        draw_boundary(&mut img, &affine);
    }

    // 4. edges: straight (Affine) or geodesic polyline (Fisheye/Poincaré).
    for &(a, b) in &graph.edges {
        let pts = edge_polyline(lensed[a], lensed[b], cfg, &affine);
        // Fade by the dimmer endpoint so rim edges fade out (Poincaré).
        let alpha = rim_alpha(lensed[a], cfg).min(rim_alpha(lensed[b], cfg));
        polyline(&mut img, &pts, faded(EDGE, alpha));
    }

    // 3. nodes: radius scaled by magnification, alpha faded toward the rim.
    for &l in &lensed {
        let screen = affine.to_screen(l);
        let r = node_radius(l, BASE_RADIUS, cfg);
        disk(&mut img, screen, r, faded(NODE, rim_alpha(l, cfg)));
    }

    img
}

/// Draw the unit-disk boundary circle: map disk radius `1.0` through the affine.
/// The affine is a uniform scale, so the screen radius is `1.0 * scale`.
fn draw_boundary(img: &mut RgbaImage, affine: &Fit) {
    let center = affine.to_screen(Complex::ORIGIN);
    let screen_r = affine.scale; // radius 1.0 in lensed space → `scale` px.
    circle_outline(img, center, screen_r, BOUNDARY);
}

/// Blit `src` into `dst` at the given top-left offset (opaque copy).
fn blit(dst: &mut RgbaImage, src: &RgbaImage, off_x: u32) {
    for y in 0..src.height() {
        for x in 0..src.width() {
            dst.put_pixel(off_x + x, y, *src.get_pixel(x, y));
        }
    }
}

fn out_dir() -> PathBuf {
    let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target"));
    std::fs::create_dir_all(&dir).expect("create target dir");
    dir
}

fn save(img: &RgbaImage, path: &Path, label: &str) {
    img.save(path).expect("write PNG");
    println!("OK {label} -> {}", path.display());
}

fn main() {
    let modes = [
        (ProjectionKind::Affine, "proj-affine.png", "affine"),
        (ProjectionKind::Fisheye, "proj-fisheye.png", "fisheye"),
        (ProjectionKind::Poincare, "proj-poincare.png", "poincare"),
    ];

    let dir = out_dir();
    let mut rendered = Vec::new();
    for (kind, file, label) in modes {
        let cfg = ProjectionConfig {
            kind,
            strength: 0.45,
            size_falloff: 1.0,
            geodesic_segments: 24,
        };
        let img = render(cfg);
        save(&img, &dir.join(file), label);
        rendered.push(img);
    }

    // The money shot: 3-up strip with 1px dividers between panels.
    let mut strip = RgbaImage::from_pixel(WIDTH * 3 + 2, HEIGHT, BG);
    blit(&mut strip, &rendered[0], 0);
    blit(&mut strip, &rendered[1], WIDTH + 1);
    blit(&mut strip, &rendered[2], WIDTH * 2 + 2);
    let divider = Rgba([0x40, 0x46, 0x4e, 0xff]);
    for y in 0..HEIGHT as i32 {
        put_px(&mut strip, WIDTH as i32, y, divider);
        put_px(&mut strip, (WIDTH * 2 + 1) as i32, y, divider);
    }
    save(&strip, &dir.join("proj-compare.png"), "compare");
}
