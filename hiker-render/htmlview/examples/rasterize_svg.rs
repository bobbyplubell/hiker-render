//! Dev helper: rasterize an SVG file to a PNG so math/diagram output can be
//! eyeballed. Uses the same resvg backend the widget uses for `<img>` SVGs.
//!
//! Usage:
//!   cargo run -p hiker-htmlview --example rasterize_svg -- <file.svg> [scale]
//!
//! Writes `<file>.png` next to the input. `scale` (default 10) upsamples so
//! small inline math is legible.

use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().expect("usage: rasterize_svg <file.svg> [scale]"));
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
