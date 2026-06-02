//! Scroll-performance profiler for the ZIM/HTML viewer.
//!
//! Layout + display-list build are cached by `HtmlView`, so scrolling re-runs
//! only `HtmlView::paint()`. This harness lays a real article out once, then
//! times `paint()` across a simulated scroll sweep to find the per-frame cost.
//!
//!   cargo run --release --example scroll_profile [ARTICLE_DIR]
//!   ARTICLE=World_War_II CORPUS_DIR=/tmp/corpus cargo run --release --example scroll_profile
//!   # function-level flamegraph of the scroll sweep:
//!   cargo run --release --example scroll_profile --features profile-cpu
//!
//! It reports, per frame: `frame` = paint with the real viewport clip (cull +
//! clone + emit visible shapes); `cull` = paint with an off-screen clip (iterate
//! every shape, emit none) — the pure O(total-shapes) culling floor. If `cull`
//! is most of `frame`, the win is a spatial index over the display list.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use hiker_htmlview::{HtmlView, ResourceProvider, Theme};

struct DirProvider {
    root: PathBuf,
}
impl ResourceProvider for DirProvider {
    fn fetch(&self, url: &str) -> Option<(Vec<u8>, String)> {
        let rel = url.trim_start_matches("./").trim_start_matches('/');
        let bytes = std::fs::read(self.root.join(rel)).ok()?;
        let mime = if rel.ends_with(".css") {
            "text/css"
        } else if rel.ends_with(".png") {
            "image/png"
        } else if rel.ends_with(".jpg") || rel.ends_with(".jpeg") {
            "image/jpeg"
        } else if rel.ends_with(".svg") {
            "image/svg+xml"
        } else {
            "application/octet-stream"
        };
        Some((bytes, mime.to_string()))
    }
}

fn headless_ctx() -> egui::Context {
    let ctx = egui::Context::default();
    ctx.set_fonts(egui::FontDefinitions::default());
    let _ = ctx.run(egui::RawInput::default(), |_| {});
    ctx
}

const WIDTH: f32 = 800.0;
const VIEW_W: f32 = 820.0;
const VIEW_H: f32 = 1000.0;
const STEPS: usize = 60;

fn stats(mut v: Vec<Duration>) -> (f64, f64, f64, f64) {
    v.sort();
    let us = |d: Duration| d.as_secs_f64() * 1e6;
    let n = v.len().max(1);
    let mean = v.iter().map(|d| us(*d)).sum::<f64>() / n as f64;
    let p = |q: f64| us(v[((n as f64 * q) as usize).min(n - 1)]);
    (mean, p(0.50), p(0.99), us(*v.last().unwrap()))
}

fn main() {
    // Locate the article directory: explicit arg, or CORPUS_DIR/ARTICLE, or the
    // committed wiki-sample fixture.
    let dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .or_else(|| {
            let base = std::env::var_os("CORPUS_DIR")?;
            let art = std::env::var_os("ARTICLE")?;
            Some(PathBuf::from(base).join(art))
        })
        .unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/wiki-sample")));

    let html = match std::fs::read_to_string(dir.join("article.html")) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("could not read {}/article.html: {e}", dir.display());
            return;
        }
    };

    let ctx = headless_ctx();
    let provider: Arc<dyn ResourceProvider> = Arc::new(DirProvider { root: dir.clone() });
    let mut view = HtmlView::new(&html, Some("./"), provider);
    view.set_theme(Theme::Light);

    // Cold layout (parse already done in new()): cascade + layout + display list.
    let t = Instant::now();
    let size = view.layout(&ctx, WIDTH);
    let cold_layout = t.elapsed();
    // Warm layout: must hit the cache (this is what a scroll frame pays).
    let t = Instant::now();
    let _ = view.layout(&ctx, WIDTH);
    let warm_layout = t.elapsed();

    let n_shapes = view.shape_count();
    let height = size.y;

    println!("article : {}", dir.display());
    println!("content : {:.0}px tall, {n_shapes} display-list shapes", height);
    println!("layout  : cold {:.1}ms, cached {:.1}µs/frame",
        cold_layout.as_secs_f64() * 1e3, warm_layout.as_secs_f64() * 1e6);

    let viewport = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(VIEW_W, VIEW_H));
    let offscreen = egui::Rect::from_min_size(egui::pos2(1e6, 1e6), egui::vec2(VIEW_W, VIEW_H));
    let max_scroll = (height - VIEW_H).max(0.0);

    let mut frame_times = Vec::new();
    let mut cull_times = Vec::new();

    // Run inside a pass so we have a live Painter; time individual paint() calls
    // (tessellation at end of pass is not timed).
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let painter = ui.painter().clone();
            // Warm caches (galley shaping etc.) so the first sample isn't an outlier.
            for w in 0..3 {
                let y = max_scroll * w as f32 / 3.0;
                view.paint(&painter, egui::pos2(0.0, -y), viewport);
            }
            for i in 0..STEPS {
                let scroll = max_scroll * i as f32 / (STEPS.max(2) - 1) as f32;
                let origin = egui::pos2(0.0, -scroll);

                let t = Instant::now();
                view.paint(&painter, origin, viewport);
                frame_times.push(t.elapsed());

                let t = Instant::now();
                view.paint(&painter, origin, offscreen);
                cull_times.push(t.elapsed());
            }
        });
    });

    // Full per-frame CPU as the app actually pays it: a complete egui pass
    // (paint the visible band) PLUS tessellating the emitted shapes into GPU
    // vertices. egui re-tessellates every frame, so this is where text-heavy
    // viewports get expensive even though paint() itself is cheap.
    let mut full_times = Vec::new();
    let mut vert_counts = Vec::new();
    let input = egui::RawInput {
        screen_rect: Some(viewport),
        ..Default::default()
    };
    // warm
    for _ in 0..3 {
        let out = ctx.run(input.clone(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let p = ui.painter().with_clip_rect(viewport);
                view.paint(&p, egui::pos2(0.0, 0.0), viewport);
            });
        });
        let _ = ctx.tessellate(out.shapes, out.pixels_per_point);
    }
    for i in 0..STEPS {
        let scroll = max_scroll * i as f32 / (STEPS.max(2) - 1) as f32;
        let origin = egui::pos2(0.0, -scroll);
        let t = Instant::now();
        let out = ctx.run(input.clone(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let p = ui.painter().with_clip_rect(viewport);
                view.paint(&p, origin, viewport);
            });
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        full_times.push(t.elapsed());
        let verts: usize = prims
            .iter()
            .map(|p| match &p.primitive {
                egui::epaint::Primitive::Mesh(m) => m.vertices.len(),
                _ => 0,
            })
            .sum();
        vert_counts.push(verts);
    }

    let (fm, fp50, fp99, fmax) = stats(frame_times);
    let (cm, _cp50, _cp99, _cmax) = stats(cull_times);
    let (gm, gp50, gp99, gmax) = stats(full_times);
    let mid_verts = vert_counts.get(vert_counts.len() / 2).copied().unwrap_or(0);

    println!();
    println!("per scroll frame ({STEPS} positions, {VIEW_W:.0}x{VIEW_H:.0} viewport):");
    println!("  cull-only floor   : mean {cm:.0}µs        (iterate all {n_shapes} shapes, emit none)");
    println!("  paint() cull+emit : mean {fm:.0}µs  p50 {fp50:.0}µs  p99 {fp99:.0}µs  max {fmax:.0}µs");
    println!("    └ cull is {:.0}% of paint()", 100.0 * cm / fm.max(1.0));
    println!("  FULL pass+tessel  : mean {gm:.0}µs  p50 {gp50:.0}µs  p99 {gp99:.0}µs  max {gmax:.0}µs");
    println!("    └ tessellation makes ~{} GPU vertices/frame", mid_verts);
    println!("  implied ceiling   : paint-only {:.0} fps,  full {:.0} fps",
        1e6 / fm.max(1.0), 1e6 / gm.max(1.0));

    #[cfg(feature = "profile-cpu")]
    {
        eprintln!("\n[profile-cpu] sampling a longer scroll sweep for the flamegraph…");
        let guard = pprof::ProfilerGuardBuilder::default()
            .frequency(4000)
            .blocklist(&["libc", "libgcc", "pthread", "vdso"])
            .build()
            .expect("start pprof");
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let painter = ui.painter().clone();
                for _ in 0..400 {
                    for i in 0..STEPS {
                        let scroll = max_scroll * i as f32 / (STEPS.max(2) - 1) as f32;
                        view.paint(&painter, egui::pos2(0.0, -scroll), viewport);
                    }
                }
            });
        });
        let report = guard.report().build().expect("build report");
        let out = concat!(env!("CARGO_MANIFEST_DIR"), "/target/scroll-flamegraph.svg");
        let svg = std::fs::File::create(out).unwrap();
        report.flamegraph(svg).expect("write flamegraph");
        eprintln!("[profile-cpu] wrote target/scroll-flamegraph.svg");
    }
}
