//! Real-window scroll benchmark: drives the exact app render path (eframe +
//! ScrollArea + HtmlView::paint) with an auto-scroll, logging true per-frame
//! wall time so we can separate our CPU cost from tessellation + GPU + present.
//!
//!   # default: glow backend, vsync on (exactly like the app)
//!   CORPUS_DIR=/tmp/corpus ARTICLE=The_Beatles cargo run --release --example scroll_bench
//!   # raw cost (no vsync cap):           VSYNC=0 ...
//!   # wgpu/Vulkan instead of glow/GL:    RENDERER=wgpu ...
//!
//! Per window it auto-scrolls up/down for ~600 frames then closes, printing:
//!   - wall dt  = full frame (our update + tessellate + GPU upload + draw + present)
//!   - body cpu = time inside update() (layout cache hit + paint emit + egui build)
//! With VSYNC=1 wall dt is pinned to the refresh interval while the app keeps up;
//! with VSYNC=0 wall dt is the real cost. body << wall ⟹ cost is GPU/present, not us.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use eframe::egui;
use hiker_htmlview::{page_bg_color, HtmlView, ResourceProvider, Theme};

struct FsProvider {
    base: PathBuf,
}
impl ResourceProvider for FsProvider {
    fn fetch(&self, url: &str) -> Option<(Vec<u8>, String)> {
        let rel = url.trim_start_matches("./").trim_start_matches('/');
        let bytes = std::fs::read(self.base.join(rel)).ok()?;
        let mime = if rel.ends_with(".css") { "text/css" } else { "application/octet-stream" };
        Some((bytes, mime.to_string()))
    }
}

struct Bench {
    view: HtmlView,
    offset: f32,
    dir: f32,
    frame: usize,
    last: Option<Instant>,
    wall: Vec<f32>,
    body: Vec<f32>,
}

const LIMIT: usize = 600;
const SPEED: f32 = 40.0; // px/frame

fn pct(v: &mut [f32], q: f32) -> f32 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() as f32 * q) as usize).min(v.len() - 1)]
}

impl eframe::App for Bench {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let t_body = Instant::now();
        if let Some(prev) = self.last.take() {
            // Skip the first few frames (window/pipeline warmup).
            if self.frame > 5 {
                self.wall.push(prev.elapsed().as_secs_f32() * 1e3);
            }
        }

        let frame = egui::Frame::central_panel(&ctx.style()).fill(page_bg_color(Theme::Light));
        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
            let vp_h = ui.clip_rect().height();
            egui::ScrollArea::both()
                .vertical_scroll_offset(self.offset)
                .show(ui, |ui| {
                    let width = ui.available_width();
                    let size = self.view.layout(ui.ctx(), width);
                    let (rect, _r) = ui.allocate_exact_size(size, egui::Sense::hover());
                    let painter = ui.painter_at(rect);
                    self.view.paint(&painter, rect.min, painter.clip_rect());

                    // Advance the auto-scroll, bouncing within the content.
                    let max = (size.y - vp_h).max(0.0);
                    self.offset += self.dir * SPEED;
                    if self.offset >= max {
                        self.offset = max;
                        self.dir = -1.0;
                    } else if self.offset <= 0.0 {
                        self.offset = 0.0;
                        self.dir = 1.0;
                    }
                });
        });

        if self.frame > 5 {
            self.body.push(t_body.elapsed().as_secs_f32() * 1e3);
        }
        self.frame += 1;
        self.last = Some(Instant::now());
        ctx.request_repaint(); // drive continuous frames

        if self.frame >= LIMIT {
            let n = self.wall.len();
            let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len().max(1) as f32;
            let wmean = mean(&self.wall);
            let bmean = mean(&self.body);
            let mut w = self.wall.clone();
            println!(
                "\n=== {n} frames ===\n\
                 wall dt  : mean {:.2}ms  p50 {:.2}ms  p99 {:.2}ms  max {:.2}ms   => {:.0} fps\n\
                 body cpu : mean {:.2}ms  (our update(): layout-cache + paint emit + egui build)\n\
                 gpu+present (wall-body): ~{:.2}ms/frame",
                wmean, pct(&mut w, 0.50), pct(&mut w, 0.99), pct(&mut w, 1.0).min(*w.last().unwrap()),
                1000.0 / wmean.max(0.001),
                bmean,
                (wmean - bmean).max(0.0),
            );
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

fn main() -> eframe::Result<()> {
    let dir = std::env::var_os("CORPUS_DIR")
        .zip(std::env::var_os("ARTICLE"))
        .map(|(b, a)| PathBuf::from(b).join(a))
        .unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/wiki-sample")));
    let html = std::fs::read_to_string(dir.join("article.html")).expect("read article.html");
    let mut view = HtmlView::new(&html, Some("./"), Arc::new(FsProvider { base: dir.clone() }));
    view.set_theme(Theme::Light);

    let renderer = match std::env::var("RENDERER").as_deref() {
        Ok("wgpu") => eframe::Renderer::Wgpu,
        _ => eframe::Renderer::Glow,
    };
    let vsync = std::env::var("VSYNC").as_deref() != Ok("0");
    eprintln!("backend={renderer:?}  vsync={vsync}  article={}", dir.display());

    let options = eframe::NativeOptions {
        renderer,
        vsync,
        viewport: egui::ViewportBuilder::default().with_inner_size([1400.0, 1000.0]),
        ..Default::default()
    };
    eframe::run_native(
        "scroll_bench",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(Bench {
                view,
                offset: 0.0,
                dir: 1.0,
                frame: 0,
                last: None,
                wall: Vec::new(),
                body: Vec::new(),
            }))
        }),
    )
}
