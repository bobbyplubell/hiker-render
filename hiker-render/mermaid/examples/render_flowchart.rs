//! Eyeball example: render a mermaid flowchart to SVG + PNG.
//!
//! Usage:
//!   cargo run -p hiker-mermaid --example render_flowchart            # default sample
//!   cargo run -p hiker-mermaid --example render_flowchart -- chain   # a built-in sample
//!   cargo run -p hiker-mermaid --example render_flowchart -- decision
//!   cargo run -p hiker-mermaid --example render_flowchart -- lr
//!   cargo run -p hiker-mermaid --example render_flowchart -- 'graph TD; A-->B'  # inline src
//!
//! Writes `target/mermaid-<name>.svg` and rasterizes it to
//! `target/mermaid-<name>.png` via resvg (system fonts loaded so `<text>`
//! renders). When given inline source, `<name>` is `custom`.

use hiker_mermaid::{MermaidOptions, render};

/// A built-in sample: (name, mermaid source). Covers all diagram types; rendered
/// via the auto-detecting `render()` dispatcher.
fn samples() -> Vec<(&'static str, &'static str)> {
    vec![
        // A vertical chain.
        (
            "chain",
            "graph TD; A[Start] --> B[Load config]; B --> C[Run]; C --> D[Done]",
        ),
        // An if/decision diamond with labeled branches.
        (
            "decision",
            "graph TD; A[Start]-->B{OK?}; B-->|yes|C(Done); B-->|no|A",
        ),
        // A left-to-right graph with a few shapes.
        (
            "lr",
            "graph LR; A([Input]) --> B{Valid?}; B -->|yes| C[Process]; B -->|no| D[Reject]; C --> E((End)); D --> E",
        ),
        // A pie chart.
        (
            "pie",
            "pie showData title Pet ownership\n    \"Dogs\" : 386\n    \"Cats\" : 85\n    \"Rats\" : 15",
        ),
        // A sequence diagram.
        (
            "sequence",
            "sequenceDiagram\n    participant A as Alice\n    participant B as Bob\n    A->>B: Hello Bob\n    B-->>A: Hi Alice\n    A->>B: How are you?\n    B->>B: thinking\n    B-->>A: Great!",
        ),
    ]
}

fn main() {
    let arg = std::env::args().nth(1);
    let opts = MermaidOptions::default();

    // Decide which (name, src) pairs to render.
    let jobs: Vec<(String, String)> = match arg {
        None => samples()
            .into_iter()
            .map(|(n, s)| (n.to_string(), s.to_string()))
            .collect(),
        Some(a) => {
            if let Some((n, s)) = samples().iter().find(|(n, _)| *n == a) {
                vec![(n.to_string(), s.to_string())]
            } else {
                // Treat the arg as inline mermaid source.
                vec![("custom".to_string(), a)]
            }
        }
    };

    for (name, src) in jobs {
        match render(&src, &opts) {
            Ok(render) => {
                let (svg_path, png_path) = write_outputs(&name, &render.svg, render.width_px, render.height_px);
                let svg_sz = std::fs::metadata(&svg_path).map(|m| m.len()).unwrap_or(0);
                let png_sz = std::fs::metadata(&png_path).map(|m| m.len()).unwrap_or(0);
                println!(
                    "{name}: {}x{} px\n  {svg_path} ({svg_sz} bytes)\n  {png_path} ({png_sz} bytes)",
                    render.width_px.round() as i64,
                    render.height_px.round() as i64,
                );
            }
            Err(e) => {
                eprintln!("{name}: render failed: {e:?}");
            }
        }
    }
}

/// Write the SVG to `target/mermaid-<name>.svg` and a rasterized PNG next to it.
/// Returns the two paths.
fn write_outputs(name: &str, svg: &str, w: f32, h: f32) -> (String, String) {
    // `target/` exists when run via cargo (CARGO_TARGET_DIR or default).
    let target = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".to_string());
    std::fs::create_dir_all(&target).expect("create target dir");

    let svg_path = format!("{target}/mermaid-{name}.svg");
    let png_path = format!("{target}/mermaid-{name}.png");

    std::fs::write(&svg_path, svg).expect("write svg");
    rasterize(svg, w, h, &png_path);

    (svg_path, png_path)
}

/// Rasterize the SVG string to a PNG via resvg, loading system fonts so the
/// `<text>` labels render.
fn rasterize(svg: &str, w: f32, h: f32, png_path: &str) {
    use resvg::tiny_skia::{Pixmap, Transform};
    use resvg::usvg::{Options, Tree};

    let mut opt = Options::default();
    {
        let db = opt.fontdb_mut();
        db.load_system_fonts();
        // fontdb defaults the generic `sans-serif` family to "Arial", which is
        // absent on Linux, so `<text font-family="sans-serif">` would resolve to
        // nothing. Point the generics at fonts that are actually installed.
        db.set_sans_serif_family("Liberation Sans");
        db.set_serif_family("Liberation Serif");
        db.set_monospace_family("Liberation Mono");
    }

    let tree = match Tree::from_str(svg, &opt) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("  (svg parse failed, skipping png: {e})");
            return;
        }
    };

    let pw = (w.ceil() as u32).max(1);
    let ph = (h.ceil() as u32).max(1);
    let mut pixmap = match Pixmap::new(pw, ph) {
        Some(p) => p,
        None => {
            eprintln!("  (pixmap alloc failed for {pw}x{ph})");
            return;
        }
    };
    resvg::render(&tree, Transform::identity(), &mut pixmap.as_mut());
    if let Err(e) = pixmap.save_png(png_path) {
        eprintln!("  (png save failed: {e})");
    }
}
