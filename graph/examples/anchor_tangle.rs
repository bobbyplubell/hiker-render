//! Headless reproduction + visual confirmation of the temporal-stability
//! anchor-spring tangle regression on COMPLEX graphs, and the adaptive-
//! stiffness fix.
//!
//! Background: when a clustering-parameter change is *small* (membership
//! barely moves), warm-seeding retained nodes at their old positions and
//! tethering them with anchor springs keeps the layout coherent — a smooth
//! morph. But for a *big* structural re-clustering (many nodes change
//! neighbours, many add/remove), tethering retained nodes to their OLD
//! spots fights the NEW edge structure: the layout can't untangle, so it
//! ends up dramatically more knotted than a fresh solve.
//!
//! This example builds a deterministic complex graph G1 (several clusters
//! with inter-cluster edges), solves it fresh (P1), then produces a
//! substantially restructured G2 and solves G2 three ways:
//!
//!   * FRESH                — scatter seed, no anchors.
//!   * WARM + ANCHORED OLD  — warm seed + a FIXED high anchor stiffness
//!                            (the shipped behaviour). This is the tangle.
//!   * WARM + ANCHORED NEW  — warm seed + ADAPTIVE stiffness: the baseline
//!                            stiffness scaled down by change magnitude, so
//!                            a big rewrite relaxes nearly freely.
//!
//! It prints the edge-crossing counts (the tangle metric) for all three
//! and renders a 3-up comparison PNG.
//!
//! Run with:
//!   cargo run -p hiker-graph --example anchor_tangle
//! Output: hiker-render/graph/target/anchor-tangle.png

use hiker_graph::{
    edge_crossings, force_to_convergence, force_to_convergence_anchored, LayoutParams, Vec2,
};
use image::{Rgba, RgbaImage};

/// Baseline (max) anchor stiffness — the user's slider value. The OLD
/// behaviour applied this flat; the NEW behaviour scales it down by change
/// magnitude.
const BASELINE_STIFFNESS: f32 = 0.2;

const W: u32 = 1500;
const H: u32 = 560;
const PANELS: u32 = 3;
const MARGIN: f32 = 30.0;

const COL_BG: Rgba<u8> = Rgba([0x14, 0x18, 0x1d, 0xff]);
const COL_EDGE: Rgba<u8> = Rgba([0x55, 0x5d, 0x68, 0x80]);
const COL_RETAINED: Rgba<u8> = Rgba([0x4d, 0xa3, 0xe6, 0xff]); // blue
const COL_NEW: Rgba<u8> = Rgba([0xe6, 0x88, 0x4d, 0xff]); // orange
const COL_LABEL: Rgba<u8> = Rgba([0xc6, 0xcc, 0xd5, 0xff]);
const COL_DIVIDER: Rgba<u8> = Rgba([0x2a, 0x30, 0x38, 0xff]);

fn main() {
    // ── G1: a deterministic complex graph — 8 clusters of 12 nodes, each
    //    an internal ring + chords, plus inter-cluster bridges. ──
    let g1 = ComplexGraph::build(8, 12, 0);
    let params = LayoutParams::default();
    let p1 = force_to_convergence(circle_seed(g1.n, 400.0, 0.0), &g1.edges, &params, || false);
    let c_fresh1 = edge_crossings(&p1, &g1.edges);

    // ── G2: substantially restructured (different cluster wiring +
    //    a few add/remove), with stable identity for retained nodes. ──
    let g2 = ComplexGraph::build(8, 12, 1);
    let retained = g1.n.min(g2.n);

    // FRESH solve of G2 — from the SAME per-index scatter distribution the live
    // "fresh" path uses (`scatter`/`scatter_point`), so the anchored-vs-fresh
    // comparison is apples-to-apples (force layout is seed-sensitive; comparing
    // against a different seed family would measure basin luck, not tangle).
    let fresh2_seed: Vec<Vec2> = (0..g2.n).map(|i| scatter_point(i, 400.0)).collect();
    let fresh2 = force_to_convergence(fresh2_seed, &g2.edges, &params, || false);
    let c_fresh2 = edge_crossings(&fresh2, &g2.edges);

    // Warm seed + anchors mapping retained nodes (index identity here) to
    // their P1 positions; new nodes spawn near the centroid, no anchor.
    let centroid = centroid_of(&p1);
    let mut warm_seed = Vec::with_capacity(g2.n);
    let mut anchors: Vec<Option<Vec2>> = Vec::with_capacity(g2.n);
    for i in 0..g2.n {
        if i < retained {
            warm_seed.push(p1[i]);
            anchors.push(Some(p1[i]));
        } else {
            let k = (i - retained) as f32;
            warm_seed.push(centroid + Vec2::new((k * 1.3).cos() * 8.0, (k * 1.3).sin() * 8.0));
            anchors.push(None);
        }
    }

    // change_fraction: retained nodes whose neighbour set changed, plus new
    // nodes, over the total — the same quantity the live view computes.
    let change_fraction = compute_change_fraction(&g1, &g2, retained);
    let adaptive = adaptive_stiffness(BASELINE_STIFFNESS, change_fraction);

    // The NEW behaviour also RELAXES the warm seed toward a fresh scatter by the
    // same change magnitude (mirrors `build_warm_seed`'s `relax`): the warm seed
    // alone, even un-tethered, biases FA2 into the old (tangled) basin, so a big
    // change needs its retained nodes free to find the untangled equilibrium.
    let relax = (change_fraction / RELAX_FULL_AT).clamp(0.0, 1.0);
    let mut warm_seed_relaxed = warm_seed.clone();
    for (i, s) in warm_seed_relaxed.iter_mut().enumerate() {
        if i < retained && relax > 0.0 {
            let sp = scatter_point(i, 400.0);
            *s = *s + (sp - *s) * relax;
        }
    }

    // OLD behaviour: flat baseline stiffness.
    let anchored_old = force_to_convergence_anchored(
        warm_seed.clone(),
        &g2.edges,
        &LayoutParams {
            anchor_stiffness: BASELINE_STIFFNESS,
            ..LayoutParams::default()
        },
        &anchors,
        || false,
    );
    let c_old = edge_crossings(&anchored_old, &g2.edges);

    // NEW behaviour: adaptive stiffness + relaxed warm seed.
    let anchored_new = force_to_convergence_anchored(
        warm_seed_relaxed,
        &g2.edges,
        &LayoutParams {
            anchor_stiffness: adaptive,
            ..LayoutParams::default()
        },
        &anchors,
        || false,
    );
    let c_new = edge_crossings(&anchored_new, &g2.edges);

    println!("complex graph: {} nodes, {} edges", g2.n, g2.edges.len());
    println!("change_fraction = {change_fraction:.2}  ->  adaptive stiffness = {adaptive:.3} (baseline {BASELINE_STIFFNESS})");
    println!("edge crossings (tangle metric):");
    println!("  C_fresh1 (G1)                 = {c_fresh1}");
    println!("  C_fresh2 (G2 fresh)           = {c_fresh2}");
    println!("  C_anchored_old (flat {BASELINE_STIFFNESS})    = {c_old}   <- regression");
    println!("  C_anchored_new (adaptive)     = {c_new}   <- fixed");
    println!(
        "  ratio old/fresh = {:.2}x   ratio new/fresh = {:.2}x",
        c_old as f32 / c_fresh2.max(1) as f32,
        c_new as f32 / c_fresh2.max(1) as f32
    );

    // ── Render 3-up: fresh | anchored-old | anchored-new. ──
    let mut all: Vec<Vec2> = Vec::new();
    all.extend_from_slice(&fresh2);
    all.extend_from_slice(&anchored_old);
    all.extend_from_slice(&anchored_new);
    let (lo, hi) = bounds(&all);

    let mut img = RgbaImage::from_pixel(W, H, COL_BG);
    let view = PanelView {
        panel_w: W as f32 / PANELS as f32,
        lo,
        hi,
        retained,
        edges: &g2.edges,
    };
    let panels = [
        Panel {
            idx: 0,
            pos: &fresh2,
            label_chars: format!("fresh  x{c_fresh2}"),
        },
        Panel {
            idx: 1,
            pos: &anchored_old,
            label_chars: format!("anchored old  x{c_old}"),
        },
        Panel {
            idx: 2,
            pos: &anchored_new,
            label_chars: format!("anchored new  x{c_new}"),
        },
    ];
    for panel in &panels {
        draw_panel(&mut img, &view, panel);
    }
    let panel_w = view.panel_w;
    for p in 1..PANELS {
        let x = (p as f32 * panel_w).round() as i32;
        for y in 0..H as i32 {
            put_px(&mut img, x, y, COL_DIVIDER);
        }
    }

    let out = format!("{}/anchor-tangle.png", env_target_dir());
    img.save(&out).expect("failed to write PNG");
    println!("OK -> {out}");
}

/// A deterministic clustered graph: `clusters` groups of `per` nodes. Each
/// cluster is an internal ring with a couple of chords; clusters are linked
/// by inter-cluster bridges whose pattern depends on `variant`, so two
/// variants are *substantially* different wirings of the same node set.
struct ComplexGraph {
    n: usize,
    edges: Vec<(u32, u32)>,
    /// Undirected neighbour set per node, for change-fraction measurement.
    neighbours: Vec<Vec<usize>>,
}

impl ComplexGraph {
    fn build(clusters: usize, per: usize, variant: u32) -> Self {
        // variant 1 drops the last node and adds two fresh ones (add/remove).
        let n = clusters * per + if variant == 0 { 0 } else { 1 };
        let mut edges: Vec<(u32, u32)> = Vec::new();
        for c in 0..clusters {
            let base = (c * per) as u32;
            // internal ring
            for k in 0..per as u32 {
                edges.push((base + k, base + (k + 1) % per as u32));
            }
            // a couple of chords for internal density
            edges.push((base, base + (per as u32) / 2));
            edges.push((base + 1, base + (per as u32) / 2 + 1));
        }
        // Inter-cluster bridges: variant changes WHICH clusters connect and
        // WHICH nodes bridge, restructuring the macro topology.
        for c in 0..clusters {
            let a = (c * per) as u32;
            // chain to next cluster, with a variant-dependent offset target
            let next = ((c + 1) % clusters * per) as u32;
            let off = (variant * 3 + c as u32) % per as u32;
            edges.push((a + off, next + (off + variant) % per as u32));
            // long-range bridge across the ring, variant-dependent
            let across = ((c + clusters / 2) % clusters * per) as u32;
            edges.push((a + (variant + 1) % per as u32, across));
        }
        // The extra nodes in variant 1 hook into two arbitrary clusters.
        if variant != 0 {
            let extra = (clusters * per) as u32; // the one extra node
            edges.push((extra, 0));
            edges.push((extra, (per as u32) * 3 + 2));
        }

        // Drop any edge that references the removed node in variant!=0? We
        // keep n = clusters*per (+extra), so indices are valid; no removal
        // of indices is needed because we only ADD a node. The "remove"
        // dimension is simulated by the rewired bridges above.

        let mut neighbours = vec![Vec::new(); n];
        for &(x, y) in &edges {
            let (x, y) = (x as usize, y as usize);
            if x < n && y < n && x != y {
                neighbours[x].push(y);
                neighbours[y].push(x);
            }
        }
        for nb in &mut neighbours {
            nb.sort_unstable();
            nb.dedup();
        }
        Self { n, edges, neighbours }
    }
}

/// Fraction of nodes whose local structure changed between G1 and G2: a
/// retained node counts as "changed" if its neighbour set differs; new
/// nodes always count. This is the same signal `build_warm_seed` can derive
/// from retained-vs-new + neighbour comparison.
fn compute_change_fraction(g1: &ComplexGraph, g2: &ComplexGraph, retained: usize) -> f32 {
    if g2.n == 0 {
        return 0.0;
    }
    let mut changed = g2.n - retained; // all new nodes count
    for i in 0..retained {
        if g1.neighbours[i] != g2.neighbours[i] {
            changed += 1;
        }
    }
    changed as f32 / g2.n as f32
}

/// Change fraction at/above which anchoring is fully off and the warm seed is
/// fully relaxed — i.e. a re-clustering this big is treated as a fresh layout.
/// Mirrors `graph_view::RELAX_FULL_AT`.
const RELAX_FULL_AT: f32 = 0.5;

/// Adaptive anchor stiffness: scales the baseline linearly from full at 0%
/// changed down to 0 at ≥`RELAX_FULL_AT`. Big rewrites relax freely; small
/// scrubs stay coherent. (Mirrors the policy landed in the live view.)
fn adaptive_stiffness(baseline: f32, change_fraction: f32) -> f32 {
    let factor = (1.0 - change_fraction / RELAX_FULL_AT).clamp(0.0, 1.0);
    baseline * factor
}

/// Deterministic per-index scatter point, matching `graph_view::scatter_point`,
/// so the relaxed warm seed lands in the same fresh-scatter distribution.
fn scatter_point(i: usize, box_size: f32) -> Vec2 {
    let mut s = 0x9E37_79B9_7F4A_7C15u64 ^ (i as u64).wrapping_mul(0x2545_F491_4F6C_DD1D);
    let mut rng = || {
        s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((s >> 33) as u32) as f32 / (u32::MAX as f32)
    };
    Vec2::new((rng() - 0.5) * box_size, (rng() - 0.5) * box_size)
}

struct Panel<'a> {
    idx: u32,
    pos: &'a [Vec2],
    label_chars: String,
}

/// Shared draw context for every panel: the per-panel width, the SHARED
/// world bounds, the retained-node cutoff for colouring, and the edge set.
struct PanelView<'a> {
    panel_w: f32,
    lo: Vec2,
    hi: Vec2,
    retained: usize,
    edges: &'a [(u32, u32)],
}

fn draw_panel(img: &mut RgbaImage, view: &PanelView<'_>, panel: &Panel<'_>) {
    let (lo, hi, panel_w) = (view.lo, view.hi, view.panel_w);
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

    for &(a, b) in view.edges {
        let (a, b) = (a as usize, b as usize);
        if a >= panel.pos.len() || b >= panel.pos.len() {
            continue;
        }
        let (x1, y1) = to_screen(panel.pos[a]);
        let (x2, y2) = to_screen(panel.pos[b]);
        draw_line(img, x1, y1, x2, y2, COL_EDGE);
    }
    for (i, &p) in panel.pos.iter().enumerate() {
        let (cx, cy) = to_screen(p);
        let color = if i < view.retained { COL_RETAINED } else { COL_NEW };
        fill_circle(img, cx, cy, 3.5, color);
    }
    draw_text(img, (panel_origin_x + 10.0) as i32, 10, &panel.label_chars, COL_LABEL);
}

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

fn env_target_dir() -> String {
    let manifest = env!("CARGO_MANIFEST_DIR");
    format!("{manifest}/target")
}

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

fn glyph(c: char) -> Option<[u8; 7]> {
    Some(match c {
        'a' => [0x00, 0x00, 0x0E, 0x01, 0x0F, 0x11, 0x0F],
        'c' => [0x00, 0x00, 0x0E, 0x10, 0x10, 0x11, 0x0E],
        'd' => [0x01, 0x01, 0x0F, 0x11, 0x11, 0x11, 0x0F],
        'e' => [0x00, 0x00, 0x0E, 0x11, 0x1F, 0x10, 0x0E],
        'f' => [0x06, 0x09, 0x08, 0x1E, 0x08, 0x08, 0x08],
        'h' => [0x10, 0x10, 0x16, 0x19, 0x11, 0x11, 0x11],
        'l' => [0x0C, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'n' => [0x00, 0x00, 0x16, 0x19, 0x11, 0x11, 0x11],
        'o' => [0x00, 0x00, 0x0E, 0x11, 0x11, 0x11, 0x0E],
        'r' => [0x00, 0x00, 0x16, 0x19, 0x10, 0x10, 0x10],
        's' => [0x00, 0x00, 0x0F, 0x10, 0x0E, 0x01, 0x1E],
        'w' => [0x00, 0x00, 0x11, 0x11, 0x15, 0x15, 0x0A],
        'x' => [0x00, 0x00, 0x11, 0x0A, 0x04, 0x0A, 0x11],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x06, 0x08, 0x10, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        ' ' => [0; 7],
        _ => return None,
    })
}
