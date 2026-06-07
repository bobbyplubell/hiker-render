//! Reproduce the app's ZIM scroll loop headlessly to check whether scrolling
//! thrashes the layout cache.
//!
//! Mirrors `app/src/panels/zim.rs`: a `ScrollArea::both()` with
//! `auto_shrink([false,false])`, laying the article out at `ui.available_width()`
//! every frame, then feeding scroll-wheel input frame by frame. If
//! `layout_runs()` climbs while scrolling, the (exact-float) width key is
//! jittering and each frame pays the full ~100ms layout — the scroll lag.
//!
//!   cargo run --release --example scroll_repro [ARTICLE_DIR]

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui;
use hiker_htmlview::{HtmlView, ResourceProvider, Theme};

struct DirProvider {
    root: PathBuf,
}
impl ResourceProvider for DirProvider {
    fn fetch(&self, url: &str) -> Option<(Vec<u8>, String)> {
        let rel = url.trim_start_matches("./").trim_start_matches('/');
        let bytes = std::fs::read(self.root.join(rel)).ok()?;
        let mime = if rel.ends_with(".css") { "text/css" } else { "application/octet-stream" };
        Some((bytes, mime.to_string()))
    }
}

fn main() {
    let dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .or_else(|| {
            Some(PathBuf::from(std::env::var_os("CORPUS_DIR")?).join(std::env::var_os("ARTICLE")?))
        })
        .unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/wiki-sample")));
    let html = std::fs::read_to_string(dir.join("article.html")).expect("read article.html");

    let ctx = egui::Context::default();
    ctx.set_fonts(egui::FontDefinitions::default());
    let provider: Arc<dyn ResourceProvider> = Arc::new(DirProvider { root: dir.clone() });
    let mut view = HtmlView::new(&html, Some("./"), provider);
    view.set_theme(Theme::Light);

    // Simulate a 1200x900 window. Each frame: optional scroll delta, then the
    // exact app body (ScrollArea::both + available_width + layout + paint).
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1200.0, 900.0));
    let mut widths: Vec<f32> = Vec::new();

    let run_frame = |view: &mut HtmlView, ctx: &egui::Context, scroll: f32, widths: &mut Vec<f32>| {
        let input = egui::RawInput {
            screen_rect: Some(screen),
            events: if scroll != 0.0 {
                vec![egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -scroll),
                    modifiers: egui::Modifiers::default(),
                }]
            } else {
                vec![]
            },
            ..Default::default()
        };
        ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let width = ui.available_width();
                        widths.push(width);
                        let size = view.layout(ui.ctx(), width);
                        let (rect, _r) = ui.allocate_exact_size(size, egui::Sense::click());
                        let painter = ui.painter_at(rect);
                        view.paint(&painter, rect.min, painter.clip_rect());
                    });
            });
        });
    };

    // Settle a few frames (scrollbar state stabilizes), then record the baseline.
    for _ in 0..4 {
        run_frame(&mut view, &ctx, 0.0, &mut widths);
    }
    let base_runs = view.layout_runs();
    let settle_w = *widths.last().unwrap();
    println!("after settle: layout_runs={base_runs}, width={settle_w}");

    // Now scroll for 120 frames (like dragging the wheel), recording width each frame.
    widths.clear();
    for _ in 0..120 {
        run_frame(&mut view, &ctx, 80.0, &mut widths);
    }
    let scroll_runs = view.layout_runs() - base_runs;

    // Width stability across the scroll.
    let wmin = widths.iter().cloned().fold(f32::INFINITY, f32::min);
    let wmax = widths.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let distinct: std::collections::BTreeSet<u32> =
        widths.iter().map(|w| w.to_bits()).collect();

    println!("\n=== 120 scroll frames ===");
    println!("layout pipeline re-ran : {scroll_runs} times  (0 = healthy; ~120 = thrashing → lag)");
    println!("width passed to layout : min {wmin}  max {wmax}  ({} distinct float values)", distinct.len());
    if scroll_runs > 1 {
        println!("\n>>> CONFIRMED: scrolling re-runs layout (~{:.0}ms each). That is the lag.", 100.0);
        println!("    Distinct widths seen: {:?}",
            distinct.iter().take(8).map(|b| f32::from_bits(*b)).collect::<Vec<_>>());
    } else {
        println!("\n>>> Layout is stable during scroll; lag (if any) is elsewhere.");
    }
}
