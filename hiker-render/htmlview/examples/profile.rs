//! Profiling harness for the render pipeline (parse -> Stylo style -> layout) on
//! a real Wikipedia article. CPU and heap profilers are feature-gated so they
//! never touch normal builds:
//!
//!   cargo run --release --example profile --features profile-cpu   [iters]
//!   cargo run --release --example profile --features profile-heap
//!
//! `profile-cpu` (pprof) writes `target/profile-flamegraph.svg` and prints the
//! hottest functions by self-time. `profile-heap` (dhat) writes
//! `target/dhat-heap.json` (open at https://nnethercote.github.io/dh_view/).
//! With no feature it's a plain smoke runner.

use std::path::PathBuf;

use hiker_htmlview::layout::fonts::FontCtx;
use hiker_htmlview::{dom, layout, ResourceProvider, Theme};

#[cfg(feature = "profile-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

/// Directory-backed resource provider over `wiki-sample/`.
struct DirProvider {
    root: PathBuf,
}
impl ResourceProvider for DirProvider {
    fn fetch(&self, url: &str) -> Option<(Vec<u8>, String)> {
        let rel = url.trim_start_matches("./").trim_start_matches('/');
        let bytes = std::fs::read(self.root.join(rel)).ok()?;
        let mime = if rel.ends_with(".css") {
            "text/css"
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

/// One full pipeline pass: parse HTML, run the Stylo style pass, lay out at
/// 800px. Returns (#dom nodes, #layout boxes) so the work can't be optimized
/// away. The Stylo pass needs an `egui::Context` for real font metrics; we pass
/// the one backing `fonts`.
fn run_once(html: &str, provider: &DirProvider, fonts: &mut FontCtx) -> (usize, usize) {
    let mut doc = dom::parse_html(html);
    hiker_htmlview::css::stylo::style_document_stylo(
        &mut doc,
        provider,
        Some("./"),
        Theme::Light,
        1000.0,
        Some(fonts.ctx()),
    );
    let (tree, _size) = layout::layout_document(&doc, fonts, 800.0, 1.0);
    (doc.nodes.len(), tree.boxes.len())
}

/// Full "clicked a new page" cost: everything `HtmlView::layout` does — parse,
/// cascade, layout, image/SVG texture decode, and display-list build. `set_html`
/// forces a cold rebuild each call (drops all caches), matching a navigation.
fn run_full(view: &mut hiker_htmlview::HtmlView, ctx: &egui::Context, html: &str) -> usize {
    view.set_html(html);
    let size = view.layout(ctx, 800.0);
    size.x as usize
}

fn main() {
    let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/wiki-sample"));
    let html = std::fs::read_to_string(dir.join("article.html")).expect("read article.html");
    let provider = DirProvider { root: dir.clone() };
    let ctx = headless_ctx();
    let mut fonts = FontCtx::new(ctx.clone(), 1.0);

    // Args: [iters] [mode]   where mode = "full" profiles the whole HtmlView path
    // (incl. texture decode + display list), default profiles parse+style+layout.
    let iters: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(40);
    let full = std::env::args().nth(2).as_deref() == Some("full");

    // A full-path view (its own Arc provider).
    let arc_provider: std::sync::Arc<dyn ResourceProvider> =
        std::sync::Arc::new(DirProvider { root: dir });
    let mut view = hiker_htmlview::HtmlView::new(&html, Some("./"), arc_provider);

    // Warm once so font atlases / lazy statics don't dominate the first sample.
    let (nodes, boxes) = run_once(&html, &provider, &mut fonts);
    let _ = run_full(&mut view, &ctx, &html);
    eprintln!(
        "article: {nodes} DOM nodes, {boxes} layout boxes; mode={}; running {iters} iters",
        if full { "FULL (incl. textures+displaylist)" } else { "parse+style+layout" }
    );

    #[cfg(feature = "profile-heap")]
    {
        // Heap profile: a single pass is enough to attribute allocations.
        let _profiler = dhat::Profiler::builder()
            .file_name(concat!(env!("CARGO_MANIFEST_DIR"), "/target/dhat-heap.json"))
            .build();
        let _ = run_once(&html, &provider, &mut fonts);
        eprintln!("wrote target/dhat-heap.json (view at https://nnethercote.github.io/dh_view/)");
    }

    #[cfg(feature = "profile-cpu")]
    {
        let guard = pprof::ProfilerGuardBuilder::default()
            .frequency(2000)
            .blocklist(&["libc", "libgcc", "pthread", "vdso"])
            .build()
            .expect("start pprof");

        for _ in 0..iters {
            if full {
                let _ = run_full(&mut view, &ctx, &html);
            } else {
                let _ = run_once(&html, &provider, &mut fonts);
            }
        }

        let report = guard.report().build().expect("build report");
        print_hot_functions(&report, 25);

        let svg = std::fs::File::create(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/target/profile-flamegraph.svg"
        ))
        .unwrap();
        report.flamegraph(svg).expect("write flamegraph");
        eprintln!("wrote target/profile-flamegraph.svg");
    }

    #[cfg(not(any(feature = "profile-cpu", feature = "profile-heap")))]
    {
        for _ in 0..iters {
            let _ = run_once(&html, &provider, &mut fonts);
        }
        eprintln!("ran {iters} iters (no profiler feature enabled)");
    }
}

/// Aggregate pprof samples by the leaf frame's function (self-time) and by every
/// frame in the stack (inclusive), and print the top `n` of each. This is the
/// readable companion to the SVG flamegraph.
#[cfg(feature = "profile-cpu")]
fn print_hot_functions(report: &pprof::Report, n: usize) {
    use std::collections::HashMap;

    let mut self_time: HashMap<String, isize> = HashMap::new();
    let mut inclusive: HashMap<String, isize> = HashMap::new();
    let mut total: isize = 0;

    for (frames, count) in &report.data {
        total += *count;
        // Leaf = first symbol of the first (innermost) frame.
        if let Some(leaf) = frames.frames.first().and_then(|f| f.first()) {
            *self_time.entry(short(&leaf.name())).or_default() += *count;
        }
        // Inclusive: each distinct function appearing in the stack once.
        let mut seen = std::collections::HashSet::new();
        for frame in &frames.frames {
            for sym in frame {
                let name = short(&sym.name());
                if seen.insert(name.clone()) {
                    *inclusive.entry(name).or_default() += *count;
                }
            }
        }
    }

    let pct = |c: isize| 100.0 * c as f64 / total.max(1) as f64;
    let mut top = |label: &str, map: &HashMap<String, isize>| {
        let mut v: Vec<_> = map.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1));
        eprintln!("\n=== top {n} by {label} ({total} samples) ===");
        for (name, count) in v.into_iter().take(n) {
            eprintln!("  {:6.1}%  {:>7}  {}", pct(*count), count, name);
        }
    };
    top("SELF time", &self_time);
    top("INCLUSIVE time", &inclusive);
}

/// Trim a fully-qualified symbol to something readable: drop the crate-path
/// prefix noise but keep the last two `::` segments.
#[cfg(feature = "profile-cpu")]
fn short(name: &str) -> String {
    // Strip generic args and hash suffixes crudely.
    let name = name.split('<').next().unwrap_or(name);
    let parts: Vec<&str> = name.split("::").collect();
    if parts.len() >= 2 {
        let tail = &parts[parts.len().saturating_sub(2)..];
        tail.join("::")
    } else {
        name.to_string()
    }
}
