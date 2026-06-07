//! Windowed gallery / playground for `hiker-wavedrom`.
//!
//! A native egui/eframe window showing built-in WaveDrom diagrams (timing
//! waveforms + bitfield/register) rendered by our renderer. Pick an example
//! from the left list to load its WaveJSON into a live editor; the central view
//! re-renders the SVG (via `hiker_wavedrom::render`), rasterizes it with resvg,
//! and uploads it as an egui texture whenever the editor text (or zoom) changes.
//!
//! Run it (needs a display — will not work headless):
//!   cargo run -p hiker-wavedrom --example gallery
//!
//! Compile-check only (no display required):
//!   cargo build -p hiker-wavedrom --example gallery

use eframe::egui;
use hiker_wavedrom::{WaveDromOptions, render};

/// A built-in example: display name, family group, and WaveJSON source.
struct Example {
    name: &'static str,
    group: Group,
    src: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    Timing,
    Bitfield,
}

impl Group {
    fn label(self) -> &'static str {
        match self {
            Group::Timing => "Timing waveforms",
            Group::Bitfield => "Bitfield / register",
        }
    }
}

/// The built-in example set — covers both families and the major features.
fn examples() -> Vec<Example> {
    vec![
        // ── Timing ──────────────────────────────────────────────────────────
        Example {
            name: "Clock & signals",
            group: Group::Timing,
            src: r#"{ "signal": [
    { "name": "clk",  "wave": "P......" },
    { "name": "req",  "wave": "0.1..0." },
    { "name": "bus",  "wave": "x.34.5x", "data": ["addr", "data", "ok"] },
    { "name": "ack",  "wave": "0...1.0" }
]}"#,
        },
        Example {
            name: "Data bus & gaps",
            group: Group::Timing,
            src: r#"{ "signal": [
    { "name": "clk", "wave": "n......" },
    { "name": "dat", "wave": "x.3.|.x", "data": ["valid"] },
    { "name": "ack", "wave": "0..1|0." }
  ],
  "head": { "text": "fig 1: handshake", "tick": 0 },
  "foot": { "tock": 0 }
}"#,
        },
        Example {
            name: "Groups & spacer",
            group: Group::Timing,
            src: r#"{ "signal": [
    { "name": "clk", "wave": "p......" },
    {},
    ["Request",
      { "name": "ena", "wave": "0.1...0" },
      { "name": "addr", "wave": "x.3...x", "data": ["A0"] }
    ],
    ["Reply",
      { "name": "rdy", "wave": "0...1.0" },
      { "name": "data", "wave": "x...4.x", "data": ["D0"] }
    ]
]}"#,
        },
        Example {
            name: "Edges & arcs",
            group: Group::Timing,
            src: r#"{ "signal": [
    { "name": "A", "wave": "01........0", "node": ".a........b" },
    { "name": "B", "wave": "0...1...0..", "node": "....c...d.." }
  ],
  "edge": ["a~c setup", "c~d hold", "a<->b period"]
}"#,
        },
        Example {
            name: "All wave chars",
            group: Group::Timing,
            src: r#"{ "signal": [
    { "name": "clk",  "wave": "pPnN" },
    { "name": "lvls", "wave": "01xz" },
    { "name": "pull", "wave": "du.." },
    { "name": "data", "wave": "2345", "data": ["a", "b", "c", "d"] }
]}"#,
        },
        Example {
            name: "Sub-cycles",
            group: Group::Timing,
            src: r#"{ "signal": [
    { "name": "clk", "wave": "p<....>p" },
    { "name": "d",   "wave": "x<2.3.>x", "data": ["a", "b"] }
]}"#,
        },
        Example {
            name: "Skin: dark",
            group: Group::Timing,
            src: r#"{ "signal": [
    { "name": "clk", "wave": "P......" },
    { "name": "bus", "wave": "x.34.5x", "data": ["a", "b", "c"] },
    { "name": "en",  "wave": "0.1..0." }
  ],
  "config": { "skin": "dark" }
}"#,
        },
        Example {
            name: "Skin: narrow",
            group: Group::Timing,
            src: r#"{ "signal": [
    { "name": "clk",  "wave": "p..........", "period": 2 },
    { "name": "data", "wave": "x.3.4.5.6.x", "data": ["a", "b", "c", "d"] }
  ],
  "config": { "skin": "narrow" }
}"#,
        },
        // ── Bitfield / register ─────────────────────────────────────────────
        Example {
            name: "RISC-V R-type",
            group: Group::Bitfield,
            src: r#"{ "reg": [
    { "bits": 7, "name": "opcode" },
    { "bits": 5, "name": "rd", "attr": "dst" },
    { "bits": 3, "name": "funct3" },
    { "bits": 5, "name": "rs1" },
    { "bits": 5, "name": "rs2" },
    { "bits": 7, "name": "funct7" }
]}"#,
        },
        Example {
            name: "Typed + legend",
            group: Group::Bitfield,
            src: r#"{ "reg": [
    { "bits": 8, "name": "data",  "type": 2 },
    { "bits": 8, "name": "addr",  "type": 3 },
    { "bits": 8, "name": "flags", "type": 4 },
    { "bits": 8, "name": "crc",   "type": 5 }
  ],
  "legend": { "data": 2, "addr": 3, "flags": 4, "crc": 5 }
}"#,
        },
        Example {
            name: "Compact (32-bit)",
            group: Group::Bitfield,
            src: r#"{ "reg": [
    { "bits": 7, "name": "opcode" },
    { "bits": 5, "name": "rd" },
    { "bits": 3, "name": "f3" },
    { "bits": 5, "name": "rs1" },
    { "bits": 5, "name": "rs2" },
    { "bits": 7, "name": "f7" }
  ],
  "config": { "compact": true }
}"#,
        },
        Example {
            name: "Binary + attrs",
            group: Group::Bitfield,
            src: r#"{ "reg": [
    { "bits": 4, "name": 5, "attr": "const" },
    { "bits": 4, "name": "mode" },
    { "bits": 8, "name": "value", "attr": ["lo", "byte"] }
]}"#,
        },
    ]
}

/// A successfully rasterized diagram: the GPU texture plus its pixel size.
struct Rendered {
    texture: egui::TextureHandle,
    diagram_w: f32,
    diagram_h: f32,
}

struct GalleryApp {
    examples: Vec<Example>,
    /// Index of the currently-selected built-in example (for list highlight).
    selected: Option<usize>,
    /// Live editor contents — the source actually rendered.
    source: String,
    /// Last (source, scale) we rasterized for, so we only re-render on change.
    last_key: Option<(String, u32)>,
    /// Current render result, or the error message to show in red.
    current: Result<Rendered, String>,
    /// Zoom: display scale of the diagram in the view.
    zoom: f32,
    /// Rasterization supersampling factor (for crispness), tied to zoom.
    raster_scale: f32,
    /// Show a checkered background behind the diagram (helps dark-skin/transparent).
    checkered: bool,
}

impl Default for GalleryApp {
    fn default() -> Self {
        let examples = examples();
        let source = examples[0].src.to_string();
        GalleryApp {
            examples,
            selected: Some(0),
            source,
            last_key: None,
            current: Err("not yet rendered".to_string()),
            zoom: 1.5,
            raster_scale: 2.0,
            checkered: false,
        }
    }
}

impl GalleryApp {
    /// Re-render the current `source` to a texture if the source or raster scale
    /// changed since the last render. Cheap no-op when nothing changed.
    fn ensure_rendered(&mut self, ctx: &egui::Context) {
        let scale = self.raster_scale.max(self.zoom).max(1.0);
        let scale_key = (scale * 100.0).round() as u32;
        let key = (self.source.clone(), scale_key);
        if self.last_key.as_ref() == Some(&key) {
            return;
        }
        self.last_key = Some(key);
        let opts = WaveDromOptions::default();
        self.current = render_to_texture(ctx, &self.source, &opts, scale);
    }
}

impl eframe::App for GalleryApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Left: example list grouped by family.
        egui::SidePanel::left("examples")
            .resizable(true)
            .default_width(190.0)
            .show(ctx, |ui| {
                ui.heading("WaveDrom");
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for group in [Group::Timing, Group::Bitfield] {
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(group.label()).strong().size(13.0));
                        for (i, ex) in self.examples.iter().enumerate() {
                            if ex.group != group {
                                continue;
                            }
                            let selected = self.selected == Some(i);
                            if ui.selectable_label(selected, ex.name).clicked() {
                                self.selected = Some(i);
                                self.source = ex.src.to_string();
                            }
                        }
                    }
                });
            });

        // Right: live editor.
        egui::SidePanel::right("editor")
            .resizable(true)
            .default_width(330.0)
            .show(ctx, |ui| {
                ui.label("WaveJSON (editable):");
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let edit = egui::TextEdit::multiline(&mut self.source)
                        .code_editor()
                        .desired_width(f32::INFINITY)
                        .desired_rows(30);
                    if ui.add(edit).changed() {
                        self.selected = None;
                    }
                });
            });

        // Center: the rendered diagram (or error).
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Zoom");
                ui.add(egui::Slider::new(&mut self.zoom, 0.5..=4.0).fixed_decimals(1));
                ui.separator();
                ui.checkbox(&mut self.checkered, "Checkered bg");
                if let Ok(r) = &self.current {
                    ui.separator();
                    ui.label(format!("{:.0} × {:.0} px", r.diagram_w, r.diagram_h));
                }
            });
            ui.separator();

            self.ensure_rendered(ctx);
            match &self.current {
                Ok(r) => {
                    let display = egui::vec2(r.diagram_w * self.zoom, r.diagram_h * self.zoom);
                    egui::ScrollArea::both().show(ui, |ui| {
                        let (rect, _) = ui.allocate_exact_size(display, egui::Sense::hover());
                        if self.checkered {
                            paint_checkered(ui, rect);
                        } else {
                            ui.painter().rect_filled(rect, 0.0, egui::Color32::WHITE);
                        }
                        let img = egui::Image::new(egui::load::SizedTexture::new(
                            r.texture.id(),
                            display,
                        ));
                        img.paint_at(ui, rect);
                    });
                }
                Err(msg) => {
                    ui.colored_label(egui::Color32::RED, format!("Render error: {msg}"));
                }
            }
        });
    }
}

/// Paint a light checkerboard over `rect` (so a dark/transparent diagram reads).
fn paint_checkered(ui: &egui::Ui, rect: egui::Rect) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, egui::Color32::from_gray(245));
    let cell: f32 = 12.0;
    let dark = egui::Color32::from_gray(225);
    let mut y = rect.top();
    let mut row = 0;
    while y < rect.bottom() {
        let mut x = rect.left();
        let mut col = 0;
        while x < rect.right() {
            if (row + col) % 2 == 0 {
                let cell_rect = egui::Rect::from_min_size(
                    egui::pos2(x, y),
                    egui::vec2(cell.min(rect.right() - x), cell.min(rect.bottom() - y)),
                );
                painter.rect_filled(cell_rect, 0.0, dark);
            }
            x += cell;
            col += 1;
        }
        y += cell;
        row += 1;
    }
}

/// Render WaveJSON `src` → SVG → rasterized → egui texture. `scale` is the resvg
/// supersampling factor. On failure, returns the error message string.
fn render_to_texture(
    ctx: &egui::Context,
    src: &str,
    opts: &WaveDromOptions,
    scale: f32,
) -> Result<Rendered, String> {
    let r = render(src, opts).map_err(|e| format!("{e:?}"))?;
    let color_image = rasterize(&r.svg, r.width_px, r.height_px, scale)?;
    let texture = ctx.load_texture("diagram", color_image, egui::TextureOptions::LINEAR);
    Ok(Rendered {
        texture,
        diagram_w: r.width_px,
        diagram_h: r.height_px,
    })
}

fn rasterize(svg: &str, w: f32, h: f32, scale: f32) -> Result<egui::ColorImage, String> {
    use resvg::tiny_skia::{Pixmap, Transform};
    use resvg::usvg::{Options, Tree};

    let mut opt = Options::default();
    {
        let db = opt.fontdb_mut();
        db.load_system_fonts();
        db.load_font_data(hiker_wavedrom::font::FONT_BYTES.to_vec());
        db.set_sans_serif_family("Liberation Sans");
    }

    let tree = Tree::from_str(svg, &opt).map_err(|e| format!("svg parse: {e}"))?;

    let pw = ((w * scale).ceil() as u32).max(1);
    let ph = ((h * scale).ceil() as u32).max(1);
    let mut pixmap = Pixmap::new(pw, ph).ok_or_else(|| format!("pixmap alloc {pw}×{ph}"))?;
    resvg::render(&tree, Transform::from_scale(scale, scale), &mut pixmap.as_mut());

    Ok(egui::ColorImage::from_rgba_premultiplied(
        [pw as usize, ph as usize],
        pixmap.data(),
    ))
}

fn main() -> eframe::Result {
    eframe::run_native(
        "hiker-wavedrom gallery",
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::new(GalleryApp::default()))),
    )
}
