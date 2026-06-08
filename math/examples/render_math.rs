//! Dev helper: render a LaTeX math string and write both an SVG and a PNG so it
//! can be eyeballed — self-contained in this crate (no HTML renderer needed).
//!
//! Usage:
//!   cargo run -p hiker-render --example render_math -- '\frac{dT}{dP}' [font_px] [raster_scale]
//!
//! Writes `target/render_math.svg` and `target/render_math.png`.

use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let src = args.next().expect("usage: render_math <latex> [font_px] [raster_scale]");
    let font_size_px: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(48.0);
    let scale: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(3.0);

    let opts = hiker_math::MathOptions {
        font_size_px,
        color: [0, 0, 0, 255],
        style: hiker_math::MathStyle::Display,
    };
    let r = match hiker_math::render_latex(&src, &opts) {
        Ok(r) => r,
        Err(e) => {
            println!("render_latex failed ({e:?}) for: {src}");
            return;
        }
    };

    // Write under the workspace `target/` (run cargo from the workspace root).
    let out_dir = PathBuf::from("target");
    let _ = std::fs::create_dir_all(&out_dir);
    let svg_path = out_dir.join("render_math.svg");
    std::fs::write(&svg_path, &r.svg).expect("write svg");

    // Rasterize to PNG (dev-only, via resvg) for image eyeballing.
    let tree = resvg::usvg::Tree::from_data(r.svg.as_bytes(), &resvg::usvg::Options::default())
        .expect("parse own svg");
    let size = tree.size();
    let w = ((size.width() * scale).round() as u32).max(1);
    let h = ((size.height() * scale).round() as u32).max(1);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h).expect("alloc pixmap");
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let png_path = out_dir.join("render_math.png");
    pixmap.save_png(&png_path).expect("save png");

    println!(
        "rendered {}x{} px (baseline {:.1}) -> {} , {}x{} -> {}",
        r.width_px.round(),
        r.height_px.round(),
        r.baseline_px,
        svg_path.display(),
        w,
        h,
        png_path.display()
    );
}
