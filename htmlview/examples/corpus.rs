//! Headless PNG corpus snapshotter for ZIM/Wikipedia layout regression review.
//!
//! Renders the TOP region of every article in a corpus directory so the
//! renderer's output can be eyeballed (and diffed) without a display. Each
//! immediate subdirectory of the corpus root is expected to hold an
//! `article.html` plus its `style-*.css` (the exact layout `zxr --extract`
//! produces).
//!
//!   CORPUS_DIR=/tmp/corpus cargo run --example corpus
//!
//! Defaults to `<manifest>/corpus` if `CORPUS_DIR` is unset. Output PNGs land in
//! `<manifest>/target/corpus/<slug>.png`. If wgpu cannot initialize (no
//! GPU/software backend) the example prints a clear message and exits 0.

use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
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
const VIEW_W: f32 = 820.0;
const VIEW_H: f32 = 2400.0;

/// Render the top `VIEW_H` px of `dir/article.html` at `WIDTH` content width
/// into a wgpu-backed harness, saving to `out_path`. Returns pixel dims or a
/// human-readable error.
fn render_article(dir: &Path, theme: Theme, out_path: &Path) -> Result<(u32, u32), String> {
    let html = std::fs::read_to_string(dir.join("article.html"))
        .map_err(|e| format!("read article.html: {e}"))?;
    let provider = Arc::new(FsProvider {
        base: dir.to_path_buf(),
    });

    let mut view = HtmlView::new(&html, None, provider);
    view.set_theme(theme);

    let renderer = std::panic::catch_unwind(AssertUnwindSafe(|| {
        egui_kittest::wgpu::WgpuTestRenderer::new()
    }))
    .map_err(|_| "wgpu backend failed to initialize (no GPU/software device)".to_string())?;

    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::Vec2::new(VIEW_W, VIEW_H))
        .renderer(renderer)
        .build_ui(move |ui| {
            let _size = view.layout(ui.ctx(), WIDTH);
            let scroll_y: f32 = std::env::var("SCROLL_Y").ok().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            let (rect, _resp) =
                ui.allocate_exact_size(egui::Vec2::new(VIEW_W, VIEW_H), egui::Sense::hover());
            let painter = ui.painter_at(rect);
            let origin = egui::pos2(rect.min.x, rect.min.y - scroll_y);
            view.paint(&painter, origin, painter.clip_rect());
        });

    harness.run();

    match std::panic::catch_unwind(AssertUnwindSafe(|| harness.render())) {
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
    if std::env::var_os("LIBGL_ALWAYS_SOFTWARE").is_none() {
        std::env::set_var("LIBGL_ALWAYS_SOFTWARE", "1");
    }

    let corpus_dir = std::env::var_os("CORPUS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/corpus")));

    let out_dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target/corpus"));
    let _ = std::fs::create_dir_all(&out_dir);

    // Collect article subdirs (those containing article.html), sorted by name.
    let mut articles: Vec<(String, PathBuf)> = match std::fs::read_dir(&corpus_dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.join("article.html").is_file())
            .map(|p| (p.file_name().unwrap().to_string_lossy().into_owned(), p))
            .collect(),
        Err(e) => {
            println!("could not read corpus dir {}: {e}", corpus_dir.display());
            return;
        }
    };
    articles.sort_by(|a, b| a.0.cmp(&b.0));

    if articles.is_empty() {
        println!("no articles (subdirs with article.html) under {}", corpus_dir.display());
        return;
    }

    println!("rendering {} articles from {}\n", articles.len(), corpus_dir.display());
    let mut any_ok = false;
    let mut first_err: Option<String> = None;
    for (slug, dir) in &articles {
        let out = out_dir.join(format!("{slug}.png"));
        match render_article(dir, Theme::Light, &out) {
            Ok((w, h)) => {
                any_ok = true;
                println!("OK   {slug:32} {w}x{h} -> target/corpus/{slug}.png");
            }
            Err(e) => {
                println!("FAIL {slug:32} {e}");
                first_err.get_or_insert(e);
            }
        }
    }

    if !any_ok {
        println!("\nHeadless corpus could not render: {}", first_err.unwrap_or_default());
        println!("This environment appears to lack a usable GPU/software (Vulkan/GL) backend.");
    }
}
