//! Headless comparison of temporal layout stability via anchor springs.
//!
//! Builds a deterministic synthetic base graph, solves it to convergence
//! (P1). Then adds a new cluster and solves the grown graph two ways:
//!
//!   * FRESH — from a scatter seed, no anchors (`force_to_convergence`).
//!     The retained nodes are free to reshuffle into a new equilibrium.
//!   * WARM + ANCHORED — retained nodes start at their P1 positions and
//!     are tethered there by weak anchor springs
//!     (`force_to_convergence_anchored`); new nodes spawn near the
//!     centroid with no anchor and settle freely.
//!
//! All three panels are drawn with the SAME world→screen transform (the
//! union bounding box of the three layouts) so the reader can see that
//! the FRESH panel's old nodes jumped while the ANCHORED panel's old
//! nodes stayed put — matching the base panel.
//!
//! Retained nodes are blue, the newly added cluster is orange.
//!
//! Run with:
//!   cargo run -p hiker-graph --example anchor_stability
//! Output: hiker-render/graph/target/anchor-stability.png

use hiker_graph::{
    force_to_convergence, force_to_convergence_anchored, LayoutParams, Vec2,
};
use image::{Rgba, RgbaImage};

/// Weak spring constant tethering retained nodes to their prior
/// positions. Chosen empirically (see the example's self-verification):
/// strong enough that the retained ring barely moves, weak enough that
/// the new cluster still pulls itself into a sensible spot. Too high
/// (>~0.4) freezes the retained nodes so rigidly the new cluster can't
/// nudge them at all and the layout looks unbalanced; too low (<~0.02)
/// and the retained nodes drift almost as much as the fresh solve.
const ANCHOR_STIFFNESS: f32 = 0.2;

const W: u32 = 1500;
const H: u32 = 560;
const PANELS: u32 = 3;
const MARGIN: f32 = 36.0;

const COL_BG: Rgba<u8> = Rgba([0x14, 0x18, 0x1d, 0xff]);
const COL_EDGE: Rgba<u8> = Rgba([0x55, 0x5d, 0x68, 0x90]);
const COL_RETAINED: Rgba<u8> = Rgba([0x4d, 0xa3, 0xe6, 0xff]); // blue
const COL_NEW: Rgba<u8> = Rgba([0xe6, 0x88, 0x4d, 0xff]); // orange
const COL_LABEL: Rgba<u8> = Rgba([0xc6, 0xcc, 0xd5, 0xff]);
const COL_DIVIDER: Rgba<u8> = Rgba([0x2a, 0x30, 0x38, 0xff]);

fn main() {
    // ── Base graph: two small rings joined by a bridge (8 nodes). ──
    let base_n = 8usize;
    let mut base_edges: Vec<(u32, u32)> = Vec::new();
    // ring A: 0-1-2-3-0
    for i in 0..4u32 {
        base_edges.push((i, (i + 1) % 4));
    }
    // ring B: 4-5-6-7-4
    for i in 0..4u32 {
        base_edges.push((4 + i, 4 + (i + 1) % 4));
    }
    base_edges.push((3, 4)); // bridge

    let params = LayoutParams::default();
    let base_seed = circle_seed(base_n, 100.0, 0.0);
    let p1 = force_to_convergence(base_seed, &base_edges, &params, || false);

    // ── Grow: add a 5-node cluster attached to base node 0. ──
    let new_ids: Vec<u32> = (base_n as u32..base_n as u32 + 5).collect();
    let total_n = base_n + new_ids.len();
    let mut edges = base_edges.clone();
    // ring among the new cluster
    for w in 0..new_ids.len() {
        edges.push((new_ids[w], new_ids[(w + 1) % new_ids.len()]));
    }
    // a couple of chords to make it a real cluster, plus a bridge in
    edges.push((new_ids[0], new_ids[2]));
    edges.push((new_ids[1], new_ids[3]));
    edges.push((0, new_ids[0]));

    // (B) FRESH solve from a scatter seed (deterministic, rotated so it
    // doesn't coincidentally start at P1).
    let scatter = circle_seed(total_n, 130.0, 0.7);
    let fresh = force_to_convergence(scatter, &edges, &params, || false);

    // (C) WARM + ANCHORED.
    let centroid = centroid_of(&p1);
    let mut warm_seed = p1.clone();
    let mut anchors: Vec<Option<Vec2>> = p1.iter().map(|p| Some(*p)).collect();
    for (k, _) in new_ids.iter().enumerate() {
        let off = Vec2::new((k as f32 * 1.3).cos() * 6.0, (k as f32 * 1.3).sin() * 6.0);
        warm_seed.push(centroid + off);
        anchors.push(None);
    }
    let anchor_params = LayoutParams {
        anchor_stiffness: ANCHOR_STIFFNESS,
        ..LayoutParams::default()
    };
    let warm = force_to_convergence_anchored(warm_seed, &edges, &anchor_params, &anchors, || false);

    // Report drift numbers to stdout for at-a-glance verification.
    let fresh_drift = mean_disp(&fresh[..base_n], &p1);
    let warm_drift = mean_disp(&warm[..base_n], &p1);
    println!(
        "retained-node mean drift from base:  fresh = {fresh_drift:.1}  anchored = {warm_drift:.1}  (stiffness {ANCHOR_STIFFNESS})"
    );

    // ── Render 3-up with a shared transform. ──
    // Panel A draws only the base nodes/edges; B and C draw the grown
    // graph. The shared transform fits the union of all three position
    // sets so cross-panel comparison is meaningful.
    let mut all_points: Vec<Vec2> = Vec::new();
    all_points.extend_from_slice(&p1);
    all_points.extend_from_slice(&fresh);
    all_points.extend_from_slice(&warm);
    let (lo, hi) = bounds(&all_points);

    let mut img = RgbaImage::from_pixel(W, H, COL_BG);
    let panel_w = W as f32 / PANELS as f32;

    let panels = [
        Panel {
            idx: 0,
            pos: &p1,
            edges: &base_edges,
            label: "A  base (P1)",
        },
        Panel {
            idx: 1,
            pos: &fresh,
            edges: &edges,
            label: "B  fresh (reshuffled)",
        },
        Panel {
            idx: 2,
            pos: &warm,
            edges: &edges,
            label: "C  warm + anchored",
        },
    ];
    for panel in &panels {
        draw_panel(&mut img, panel, panel_w, lo, hi, base_n);
    }

    // Panel dividers.
    for p in 1..PANELS {
        let x = (p as f32 * panel_w).round() as i32;
        for y in 0..H as i32 {
            put_px(&mut img, x, y, COL_DIVIDER);
        }
    }

    let out = format!("{}/anchor-stability.png", env_target_dir());
    img.save(&out).expect("failed to write PNG");
    println!("OK -> {out}");
}

/// One panel's data: its horizontal slot, the laid-out positions, the
/// edges to draw, and a label.
struct Panel<'a> {
    idx: u32,
    pos: &'a [Vec2],
    edges: &'a [(u32, u32)],
    label: &'a str,
}

/// Draw one panel into `img`. `lo`/`hi` are the SHARED world bounds.
/// Nodes `< base_n` are retained (blue), the rest are newly added
/// (orange). Panel A's `pos` slice contains only the base nodes, so its
/// `base_n` covers all of them.
fn draw_panel(img: &mut RgbaImage, panel: &Panel<'_>, panel_w: f32, lo: Vec2, hi: Vec2, base_n: usize) {
    let span_x = (hi.x - lo.x).max(1.0);
    let span_y = (hi.y - lo.y).max(1.0);
    let avail_w = panel_w - MARGIN * 2.0;
    let avail_h = H as f32 - MARGIN * 2.0;
    let scale = (avail_w / span_x).min(avail_h / span_y);
    let center_world = (lo + hi) * 0.5;
    let panel_origin_x = panel.idx as f32 * panel_w;
    let center_screen = Vec2::new(panel_origin_x + panel_w * 0.5, H as f32 * 0.5);
    let to_screen = |w: Vec2| -> (f32, f32) {
        let s = center_screen + (w - center_world) * scale;
        (s.x, s.y)
    };

    // Edges first.
    for &(a, b) in panel.edges {
        let (a, b) = (a as usize, b as usize);
        if a >= panel.pos.len() || b >= panel.pos.len() {
            continue;
        }
        let (x1, y1) = to_screen(panel.pos[a]);
        let (x2, y2) = to_screen(panel.pos[b]);
        draw_line(img, x1, y1, x2, y2, COL_EDGE);
    }

    // Nodes.
    for (i, &p) in panel.pos.iter().enumerate() {
        let (cx, cy) = to_screen(p);
        let color = if i < base_n { COL_RETAINED } else { COL_NEW };
        fill_circle(img, cx, cy, 5.5, color);
    }

    // Panel label.
    draw_text(img, (panel_origin_x + 10.0) as i32, 10, panel.label, COL_LABEL);
}

// ── geometry helpers ───────────────────────────────────────────────

fn circle_seed(n: usize, r: f32, phase: f32) -> Vec<Vec2> {
    (0..n)
        .map(|i| {
            let a = if n == 0 {
                0.0
            } else {
                i as f32 / n as f32 * std::f32::consts::TAU + phase
            };
            Vec2::new(a.cos() * r, a.sin() * r)
        })
        .collect()
}

fn centroid_of(pos: &[Vec2]) -> Vec2 {
    if pos.is_empty() {
        return Vec2::ZERO;
    }
    let s = pos.iter().fold(Vec2::ZERO, |acc, p| acc + *p);
    s / pos.len() as f32
}

fn bounds(pos: &[Vec2]) -> (Vec2, Vec2) {
    let mut lo = Vec2::new(f32::INFINITY, f32::INFINITY);
    let mut hi = Vec2::new(f32::NEG_INFINITY, f32::NEG_INFINITY);
    for &p in pos {
        lo.x = lo.x.min(p.x);
        lo.y = lo.y.min(p.y);
        hi.x = hi.x.max(p.x);
        hi.y = hi.y.max(p.y);
    }
    (lo, hi)
}

fn mean_disp(a: &[Vec2], b: &[Vec2]) -> f32 {
    if a.is_empty() {
        return 0.0;
    }
    let total: f32 = a.iter().zip(b).map(|(p, q)| (*p - *q).length()).sum();
    total / a.len() as f32
}

fn env_target_dir() -> String {
    // examples run from the crate dir; write next to the crate's target.
    let manifest = env!("CARGO_MANIFEST_DIR");
    format!("{manifest}/target")
}

// ── pixel ops (mirrors tools/graph-snapshot) ───────────────────────

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

fn draw_line(img: &mut RgbaImage, x1: f32, y1: f32, x2: f32, y2: f32, color: Rgba<u8>) {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let steps = dx.abs().max(dy.abs()).ceil() as i32;
    if steps <= 0 {
        return;
    }
    for s in 0..=steps {
        let t = s as f32 / steps as f32;
        let x = (x1 + dx * t).round() as i32;
        let y = (y1 + dy * t).round() as i32;
        put_px(img, x, y, color);
    }
}

fn fill_circle(img: &mut RgbaImage, cx: f32, cy: f32, r: f32, color: Rgba<u8>) {
    let r2 = r * r;
    let x0 = (cx - r).floor() as i32;
    let x1 = (cx + r).ceil() as i32;
    let y0 = (cy - r).floor() as i32;
    let y1 = (cy + r).ceil() as i32;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            if dx * dx + dy * dy <= r2 {
                put_px(img, x, y, color);
            }
        }
    }
}

fn draw_text(img: &mut RgbaImage, x0: i32, y0: i32, text: &str, color: Rgba<u8>) {
    let mut lx = x0;
    for ch in text.chars() {
        if let Some(gl) = glyph(ch) {
            for (row, bits) in gl.iter().enumerate() {
                for col in 0..5 {
                    if bits & (1 << (4 - col)) != 0 {
                        put_px(img, lx + col, y0 + row as i32, color);
                    }
                }
            }
        }
        lx += 6;
    }
}

/// 5×7 bitmap font, lowercased subset used by the panel labels.
fn glyph(c: char) -> Option<[u8; 7]> {
    Some(match c {
        'a' => [0x00, 0x00, 0x0E, 0x01, 0x0F, 0x11, 0x0F],
        'b' => [0x10, 0x10, 0x1E, 0x11, 0x11, 0x11, 0x1E],
        'c' => [0x00, 0x00, 0x0E, 0x10, 0x10, 0x11, 0x0E],
        'd' => [0x01, 0x01, 0x0F, 0x11, 0x11, 0x11, 0x0F],
        'e' => [0x00, 0x00, 0x0E, 0x11, 0x1F, 0x10, 0x0E],
        'f' => [0x06, 0x09, 0x08, 0x1E, 0x08, 0x08, 0x08],
        'h' => [0x10, 0x10, 0x16, 0x19, 0x11, 0x11, 0x11],
        'l' => [0x0C, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'm' => [0x00, 0x00, 0x1A, 0x15, 0x15, 0x15, 0x15],
        'n' => [0x00, 0x00, 0x16, 0x19, 0x11, 0x11, 0x11],
        'o' => [0x00, 0x00, 0x0E, 0x11, 0x11, 0x11, 0x0E],
        'r' => [0x00, 0x00, 0x16, 0x19, 0x10, 0x10, 0x10],
        's' => [0x00, 0x00, 0x0F, 0x10, 0x0E, 0x01, 0x1E],
        't' => [0x08, 0x08, 0x1C, 0x08, 0x08, 0x09, 0x06],
        'u' => [0x00, 0x00, 0x11, 0x11, 0x11, 0x13, 0x0D],
        'w' => [0x00, 0x00, 0x11, 0x11, 0x15, 0x15, 0x0A],
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        ' ' => [0; 7],
        '(' => [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02],
        ')' => [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08],
        '+' => [0x00, 0x04, 0x04, 0x1F, 0x04, 0x04, 0x00],
        _ => return None,
    })
}
