//! Eyeball example: render WaveJSON to SVG + PNG.
//!
//! Usage:
//!   cargo run -p hiker-wavedrom --example render_wavedrom -- reg
//!   cargo run -p hiker-wavedrom --example render_wavedrom -- signal
//!   cargo run -p hiker-wavedrom --example render_wavedrom -- '{signal:[{name:"clk",wave:"p..."}]}'
//!
//! Writes `target/wavedrom-<name>.svg` and rasterizes it to PNG via resvg.

use hiker_wavedrom::{WaveDromOptions, render};

fn samples() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "reg",
            r#"{reg:[
                {bits:7, name:'opcode'},
                {bits:5, name:'rd', attr:'dst'},
                {bits:3, name:'funct3'},
                {bits:5, name:'rs1'},
                {bits:5, name:'rs2'},
                {bits:7, name:'funct7'}
            ]}"#,
        ),
        (
            "signal",
            r#"{signal:[
                {name:'clk', wave:'p.....'},
                {name:'bus', wave:'x.34.5x', data:['a','b','c']},
                {name:'req', wave:'0.1..0.'}
            ]}"#,
        ),
    ]
}

fn main() {
    let arg = std::env::args().nth(1);
    let opts = WaveDromOptions::default();

    let jobs: Vec<(String, String)> = match arg.as_deref() {
        None => samples().into_iter().map(|(n, s)| (n.to_string(), s.to_string())).collect(),
        Some(a) if a.starts_with('{') || a.starts_with('[') => {
            vec![("custom".to_string(), a.to_string())]
        }
        Some(name) => match samples().into_iter().find(|(n, _)| *n == name) {
            Some((n, s)) => vec![(n.to_string(), s.to_string())],
            None => {
                eprintln!("unknown sample '{name}'");
                return;
            }
        },
    };

    for (name, src) in jobs {
        match render(&src, &opts) {
            Ok(r) => {
                let svg_path = format!("target/wavedrom-{name}.svg");
                std::fs::write(&svg_path, &r.svg).ok();
                println!("{name}: {:.0}x{:.0} px", r.width_px, r.height_px);
                rasterize(&r.svg, &format!("target/wavedrom-{name}.png"));
                println!("  {svg_path}");
            }
            Err(e) => eprintln!("{name}: render error: {e:?}"),
        }
    }
}

fn rasterize(svg: &str, png_path: &str) {
    use resvg::tiny_skia::{Pixmap, Transform};
    use resvg::usvg;

    let mut opt = usvg::Options::default();
    {
        let db = opt.fontdb_mut();
        db.load_font_data(hiker_wavedrom::font::FONT_BYTES.to_vec());
        db.set_sans_serif_family("Liberation Sans");
    }
    let tree = match usvg::Tree::from_str(svg, &opt) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("  (svg parse failed, skipping png: {e})");
            return;
        }
    };
    let size = tree.size();
    let (pw, ph) = (size.width().ceil() as u32, size.height().ceil() as u32);
    let mut pixmap = match Pixmap::new(pw.max(1), ph.max(1)) {
        Some(p) => p,
        None => return,
    };
    resvg::render(&tree, Transform::identity(), &mut pixmap.as_mut());
    if pixmap.save_png(png_path).is_ok() {
        println!("  {png_path}");
    }
}
