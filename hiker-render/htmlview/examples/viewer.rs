//! eframe demo shell for `hiker-htmlview`.
//!
//! Renders `wiki-sample/article.html` with a directory-backed
//! `ResourceProvider`. Top bar toggles theme and zoom; the central
//! `ScrollArea` reserves the laid-out content size and paints into it.

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui;
use hiker_htmlview::{page_bg_color, HtmlView, ResourceProvider, Theme};

/// Directory-backed resource provider rooted at `wiki-sample/`.
///
/// Resolves URLs relative to that directory: strips leading `./`, `../`, `/`,
/// and also accepts the `_assets_/...` / `_res_/...` prefixes the article uses
/// by simply joining the (cleaned) path onto the base dir.
struct FsProvider {
    base: PathBuf,
}

impl FsProvider {
    fn new() -> Self {
        FsProvider {
            base: PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/wiki-sample")),
        }
    }
}

fn mime_for(path: &str) -> &'static str {
    let p = path.to_ascii_lowercase();
    if p.ends_with(".css") {
        "text/css"
    } else if p.ends_with(".png") {
        "image/png"
    } else if p.ends_with(".jpg") || p.ends_with(".jpeg") {
        "image/jpeg"
    } else if p.ends_with(".svg") {
        "image/svg+xml"
    } else if p.ends_with(".gif") {
        "image/gif"
    } else {
        "application/octet-stream"
    }
}

/// Strip URL noise so the remainder can be joined onto the base dir.
fn clean_rel(url: &str) -> String {
    let mut s = url.trim();
    // Drop scheme-relative / absolute hosts we can't serve; keep just the path.
    if let Some(stripped) = s.strip_prefix("//") {
        // e.g. //upload.wikimedia.org/... -> take the path after the host.
        s = stripped.split_once('/').map(|(_, rest)| rest).unwrap_or("");
    }
    let mut s = s.to_string();
    loop {
        let trimmed = s
            .trim_start_matches("./")
            .trim_start_matches("../")
            .trim_start_matches('/');
        if trimmed.len() == s.len() {
            break;
        }
        s = trimmed.to_string();
    }
    s
}

impl ResourceProvider for FsProvider {
    fn fetch(&self, url: &str) -> Option<(Vec<u8>, String)> {
        let rel = clean_rel(url);
        if rel.is_empty() {
            return None;
        }
        let path = self.base.join(&rel);
        let bytes = std::fs::read(&path).ok()?;
        Some((bytes, mime_for(&rel).to_string()))
    }
}

struct ViewerApp {
    view: HtmlView,
    theme: Theme,
    zoom: f32,
}

impl ViewerApp {
    fn new() -> Self {
        let html_path = concat!(env!("CARGO_MANIFEST_DIR"), "/wiki-sample/article.html");
        let html = std::fs::read_to_string(html_path).expect("read article.html");
        let provider = Arc::new(FsProvider::new());
        let view = HtmlView::new(&html, None, provider);
        ViewerApp {
            view,
            theme: Theme::Light,
            zoom: 1.0,
        }
    }
}

impl eframe::App for ViewerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let label = match self.theme {
                    Theme::Light => "Theme: Light",
                    Theme::Dark => "Theme: Dark",
                };
                if ui.button(label).clicked() {
                    self.theme = match self.theme {
                        Theme::Light => Theme::Dark,
                        Theme::Dark => Theme::Light,
                    };
                    self.view.set_theme(self.theme);
                }
                ui.separator();
                if ui.button("Zoom -").clicked() {
                    self.zoom = (self.zoom - 0.1).clamp(0.5, 3.0);
                    self.view.set_zoom(self.zoom);
                }
                ui.label(format!("{:.0}%", self.zoom * 100.0));
                if ui.button("Zoom +").clicked() {
                    self.zoom = (self.zoom + 0.1).clamp(0.5, 3.0);
                    self.view.set_zoom(self.zoom);
                }
            });
        });

        // Clear the central panel to the themed page background so the standalone
        // app shows an opaque page (matching the renderer's base rect).
        let frame = egui::Frame::central_panel(&ctx.style()).fill(page_bg_color(self.theme));
        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
            egui::ScrollArea::both().show(ui, |ui| {
                // 1. content width = available width inside the scroll area.
                let width = ui.available_width();
                // 2. lay out at that width; returns full content size.
                let size = self.view.layout(ui.ctx(), width);
                // 3. reserve the content rect so the ScrollArea knows its extent.
                let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
                // 4. paint into a painter clipped to the allocated rect. `rect.min`
                //    is the scroll-translated top-left == document origin.
                let painter = ui.painter_at(rect);
                self.view.paint(&painter, rect.min, painter.clip_rect());

                // 5. link interaction.
                if let Some(pointer) = response.hover_pos() {
                    let doc_point = pointer - rect.min.to_vec2();
                    if self.view.is_link_at(doc_point) {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                }
                if response.clicked() {
                    if let Some(pointer) = response.interact_pointer_pos() {
                        let doc_point = pointer - rect.min.to_vec2();
                        if let Some(href) = self.view.link_at(doc_point) {
                            println!("clicked link: {href}");
                        }
                    }
                }
            });
        });
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "hiker-htmlview viewer",
        options,
        Box::new(|_cc| Ok(Box::new(ViewerApp::new()))),
    )
}
