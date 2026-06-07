//! Dev helper: rasterize an SVG file to PNG for eyeballing — self-contained in
//! this crate (dev-only `resvg`), so it works even when other workspace crates
//! don't build.
//!
//! Usage:
//!   cargo run -p hiker-render --example rasterize -- <file.svg> [scale]
//!
//! Writes `<file>.png` next to the input.

use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().expect("usage: rasterize <file.svg> [scale]"));
    let scale: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(10.0);

    let data = std::fs::read(&path).expect("read svg");
    let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default())
        .expect("parse svg");
    let size = tree.size();
    let w = ((size.width() * scale).round() as u32).max(1);
    let h = ((size.height() * scale).round() as u32).max(1);

    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h).expect("alloc pixmap");
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let out = path.with_extension("png");
    pixmap.save_png(&out).expect("save png");
    println!("{} -> {w}x{h} {}", path.display(), out.display());
}
