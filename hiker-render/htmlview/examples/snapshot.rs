//! Headless PNG snapshot of `wiki-sample/article.html` via `egui_kittest`'s
//! wgpu backend, so the renderer's output can be inspected as an image without
//! a display.
//!
//! Renders two views: the TOP of the document, and a region scrolled down so a
//! table/infobox is visible. If wgpu cannot initialize (no Vulkan/GL software
//! backend in this headless environment) the example prints a clear message and
//! exits 0 rather than failing the build.

use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui;
use hiker_htmlview::{HtmlView, ResourceProvider, Theme};

// --- directory-backed provider (same resolution rules as the viewer) ---

struct FsProvider {
    base: PathBuf,
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

fn clean_rel(url: &str) -> String {
    let mut s = url.trim();
    if let Some(stripped) = s.strip_prefix("//") {
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
        let bytes = std::fs::read(self.base.join(&rel)).ok()?;
        Some((bytes, mime_for(&rel).to_string()))
    }
}

const WIDTH: f32 = 800.0;
const VIEW_W: f32 = 800.0;
const VIEW_H: f32 = 1200.0;

/// Render the article in `theme`, scrolled down by `scroll_y` pixels, into a
/// wgpu-backed harness, saving the result to `out_path`. Returns the PNG pixel
/// dimensions on success, or a human-readable error string on failure.
fn render_view(theme: Theme, scroll_y: f32, out_path: &PathBuf) -> Result<(u32, u32), String> {
    let base = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/wiki-sample"));
    let html = std::fs::read_to_string(base.join("article.html"))
        .map_err(|e| format!("read article.html: {e}"))?;
    let provider = Arc::new(FsProvider { base });

    let mut view = HtmlView::new(&html, None, provider);
    view.set_theme(theme);

    // Building the wgpu renderer initializes a device; on a machine with no
    // usable (even software) backend `WgpuTestRenderer::new` panics. Trap that
    // first, so the rest of the pipeline only runs once we have a device.
    let renderer = std::panic::catch_unwind(AssertUnwindSafe(|| {
        egui_kittest::wgpu::WgpuTestRenderer::new()
    }))
    .map_err(|_| "wgpu backend failed to initialize (no GPU/software device)".to_string())?;

    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::Vec2::new(VIEW_W, VIEW_H))
        .renderer(renderer)
        .build_ui(move |ui| {
            let _size = view.layout(ui.ctx(), WIDTH);
            // Reserve the whole viewport and paint the document shifted up by
            // `scroll_y` so that document row `scroll_y` sits at the top edge.
            let (rect, _resp) =
                ui.allocate_exact_size(egui::Vec2::new(VIEW_W, VIEW_H), egui::Sense::hover());
            let painter = ui.painter_at(rect);
            let origin = egui::pos2(rect.min.x, rect.min.y - scroll_y);
            view.paint(&painter, origin, painter.clip_rect());
        });

    harness.run();

    let render = std::panic::catch_unwind(AssertUnwindSafe(|| harness.render()));
    match render {
        Ok(Ok(image)) => {
            let (w, h) = (image.width(), image.height());
            image.save(out_path).map_err(|e| format!("save png: {e}"))?;
            Ok((w, h))
        }
        Ok(Err(e)) => Err(format!("wgpu render failed: {e}")),
        Err(_) => Err("wgpu render panicked".into()),
    }
}

fn main() {
    // Encourage software rendering in headless environments.
    if std::env::var_os("LIBGL_ALWAYS_SOFTWARE").is_none() {
        std::env::set_var("LIBGL_ALWAYS_SOFTWARE", "1");
    }

    let target = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target"));
    let _ = std::fs::create_dir_all(&target);

    // The right-hand chemical infobox table sits below the lead; this y brings it
    // into view at 800px content width.
    const INFOBOX_Y: f32 = 600.0;

    let jobs = [
        (
            "light, top of document",
            Theme::Light,
            0.0_f32,
            target.join("snap-light-top.png"),
        ),
        (
            "light, scrolled to infobox",
            Theme::Light,
            INFOBOX_Y,
            target.join("snap-light-infobox.png"),
        ),
        (
            "dark, top of document",
            Theme::Dark,
            0.0,
            target.join("snap-dark-top.png"),
        ),
        (
            "dark, scrolled to infobox",
            Theme::Dark,
            INFOBOX_Y,
            target.join("snap-dark-infobox.png"),
        ),
    ];

    let mut any_ok = false;
    let mut first_err: Option<String> = None;
    for (label, theme, scroll_y, path) in &jobs {
        match render_view(*theme, *scroll_y, path) {
            Ok((w, h)) => {
                any_ok = true;
                println!(
                    "OK  [{label}] -> {} ({w}x{h})",
                    std::fs::canonicalize(path)
                        .unwrap_or_else(|_| path.clone())
                        .display()
                );
            }
            Err(e) => {
                println!("SKIP [{label}]: {e}");
                first_err.get_or_insert(e);
            }
        }
    }

    if !any_ok {
        println!();
        println!(
            "Headless snapshot could not render: {}",
            first_err.unwrap_or_default()
        );
        println!("This environment appears to lack a usable GPU/software (Vulkan/GL) backend.");
        println!("Run the demo manually instead:  cargo run --example viewer");
        // Do NOT fail the build.
    }
}
