//! Windowed gallery / playground for `hiker-mermaid`.
//!
//! A native egui/eframe window that shows built-in mermaid diagrams rendered by
//! our renderer. Pick an example from the left list to load its source into a
//! live editor; the central view re-renders the SVG (via `hiker_mermaid::render`),
//! rasterizes it with resvg, and uploads it as an egui texture whenever the
//! editor text (or zoom) changes.
//!
//! Run it (needs a display — will not work headless):
//!   cargo run -p hiker-mermaid --example gallery
//!
//! Compile-check only (no display required):
//!   cargo build -p hiker-mermaid --example gallery

use eframe::egui;
use hiker_mermaid::{MermaidError, MermaidOptions, MermaidRender, render};

/// A built-in example: display name, diagram-type group, and mermaid source.
struct Example {
    name: &'static str,
    group: Group,
    src: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    Flowchart,
    Pie,
    Sequence,
}

impl Group {
    fn label(self) -> &'static str {
        match self {
            Group::Flowchart => "Flowchart",
            Group::Pie => "Pie",
            Group::Sequence => "Sequence",
        }
    }
}

/// The built-in example set — covers all three diagram types.
fn examples() -> Vec<Example> {
    vec![
        Example {
            name: "Decision",
            group: Group::Flowchart,
            src: "graph TD; A[Start]-->B{OK?}; B-->|yes|C(Done); B-->|no|A",
        },
        Example {
            name: "Pipeline LR",
            group: Group::Flowchart,
            src: "graph LR; A([Input]) --> B{Valid?}; B -->|yes| C[Process]; B -->|no| D[Reject]; C --> E((End)); D --> E",
        },
        Example {
            name: "Chain",
            group: Group::Flowchart,
            src: "graph TD; A[Start] --> B[Load config]; B --> C[Run]; C --> D[Done]",
        },
        Example {
            name: "Pets",
            group: Group::Pie,
            src: "pie showData title Pet ownership\n    \"Dogs\" : 386\n    \"Cats\" : 85\n    \"Rats\" : 15",
        },
        Example {
            name: "Languages",
            group: Group::Pie,
            src: "pie title Favorite languages\n    \"Rust\" : 55\n    \"Python\" : 30\n    \"Go\" : 15",
        },
        Example {
            name: "Greeting",
            group: Group::Sequence,
            src: "sequenceDiagram\n    participant A as Alice\n    participant B as Bob\n    A->>B: Hello Bob\n    B-->>A: Hi Alice\n    A->>B: How are you?\n    B->>B: thinking\n    B-->>A: Great!",
        },
    ]
}

/// A successfully rasterized diagram: the GPU texture plus the source diagram's
/// pixel size (the SVG `width_px`×`height_px`, before zoom).
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
    /// Render options (renderer defaults; light theme).
    opts: MermaidOptions,

    /// Last (source, scale) we rasterized for, so we only re-render on change.
    last_key: Option<(String, u32)>,
    /// Current render result, or the error message to show in red.
    current: Result<Rendered, String>,

    /// Zoom: display scale of the diagram in the view.
    zoom: f32,
    /// Rasterization supersampling factor (for crispness), tied to zoom.
    raster_scale: f32,
    /// Show a checkered background behind the (possibly transparent) diagram.
    checkered: bool,
}

impl Default for GalleryApp {
    fn default() -> Self {
        let examples = examples();
        // Start on the first example.
        let source = examples[0].src.to_string();
        GalleryApp {
            examples,
            selected: Some(0),
            source,
            opts: MermaidOptions::default(),
            last_key: None,
            current: Err("not yet rendered".to_string()),
            zoom: 1.0,
            raster_scale: 2.0,
            checkered: false,
        }
    }
}

impl GalleryApp {
    /// Re-render the current `source` to a texture if the source or raster scale
    /// changed since the last render. Cheap no-op when nothing changed.
    fn ensure_rendered(&mut self, ctx: &egui::Context) {
        // Rasterize at max(raster_scale, zoom) so zooming in stays crisp without
        // re-rasterizing on every tiny zoom tick (we key on the integer scale).
        let scale = self.raster_scale.max(self.zoom).max(1.0);
        let scale_key = (scale * 100.0).round() as u32;
        let key = (self.source.clone(), scale_key);
        if self.last_key.as_ref() == Some(&key) {
            return;
        }
        self.last_key = Some(key);
        self.current = render_to_texture(ctx, &self.source, &self.opts, scale);
    }
}

impl eframe::App for GalleryApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Left: selectable example list, grouped by diagram type.
        egui::SidePanel::left("examples")
            .resizable(true)
            .default_width(190.0)
            .show(ctx, |ui| {
                ui.heading("Examples");
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for group in [Group::Flowchart, Group::Pie, Group::Sequence] {
                        ui.label(egui::RichText::new(group.label()).strong());
                        for (i, ex) in self.examples.iter().enumerate() {
                            if ex.group != group {
                                continue;
                            }
                            if ui
                                .selectable_label(self.selected == Some(i), ex.name)
                                .clicked()
                            {
                                self.selected = Some(i);
                                self.source = ex.src.to_string();
                            }
                        }
                        ui.add_space(6.0);
                    }
                });
            });

        // Top of the central area: the live, editable source.
        egui::TopBottomPanel::top("editor")
            .resizable(true)
            .default_height(180.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Mermaid source");
                    ui.separator();
                    ui.checkbox(&mut self.checkered, "Checkered bg");
                    ui.separator();
                    ui.label("Zoom");
                    ui.add(egui::Slider::new(&mut self.zoom, 0.25..=4.0).step_by(0.05));
                });
                egui::ScrollArea::vertical()
                    .id_salt("editor_scroll")
                    .show(ui, |ui| {
                        let edit = egui::TextEdit::multiline(&mut self.source)
                            .font(egui::TextStyle::Monospace)
                            .desired_rows(8)
                            .desired_width(f32::INFINITY)
                            .code_editor();
                        if ui.add(edit).changed() {
                            // Typing detaches from the selected built-in.
                            let matches = self
                                .selected
                                .and_then(|i| self.examples.get(i))
                                .map(|ex| ex.src == self.source)
                                .unwrap_or(false);
                            if !matches {
                                self.selected = None;
                            }
                        }
                    });
            });

        // Bottom of the central area: the rendered diagram (or error).
        egui::CentralPanel::default().show(ctx, |ui| {
            // Re-render lazily if the source/scale changed.
            self.ensure_rendered(ctx);

            match &self.current {
                Ok(r) => {
                    ui.horizontal(|ui| {
                        ui.label(format!(
                            "Diagram: {:.0} × {:.0} px",
                            r.diagram_w, r.diagram_h
                        ));
                        ui.separator();
                        ui.label(format!("(shown at {:.0}%)", self.zoom * 100.0));
                    });
                    ui.separator();

                    let display = egui::vec2(r.diagram_w * self.zoom, r.diagram_h * self.zoom);
                    egui::ScrollArea::both().show(ui, |ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(display, egui::Sense::hover());
                        if self.checkered {
                            paint_checkered(ui, rect);
                        } else {
                            ui.painter().rect_filled(
                                rect,
                                0.0,
                                egui::Color32::WHITE,
                            );
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

/// Paint a light checkerboard over `rect` (so a transparent diagram reads).
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

/// Render mermaid `src` → SVG → rasterized → egui texture. `scale` is the resvg
/// supersampling factor. On render or rasterization failure, returns the error
/// message string (to be shown in red).
fn render_to_texture(
    ctx: &egui::Context,
    src: &str,
    opts: &MermaidOptions,
    scale: f32,
) -> Result<Rendered, String> {
    let MermaidRender {
        svg,
        width_px,
        height_px,
    } = render(src, opts).map_err(fmt_err)?;

    let color_image = rasterize(&svg, width_px, height_px, scale)?;
    let texture = ctx.load_texture("diagram", color_image, egui::TextureOptions::LINEAR);
    Ok(Rendered {
        texture,
        diagram_w: width_px,
        diagram_h: height_px,
    })
}

fn fmt_err(e: MermaidError) -> String {
    match e {
        MermaidError::Parse(s) => format!("parse: {s}"),
        MermaidError::Empty => "empty diagram (no nodes)".to_string(),
    }
}

/// Rasterize an SVG string into an `egui::ColorImage` via resvg, at `scale`×.
///
/// resvg/tiny_skia pixmaps are **premultiplied** RGBA, and egui 0.32 has a
/// matching `ColorImage::from_rgba_premultiplied`, so we feed the pixmap bytes
/// straight in (no un-premultiply step).
fn rasterize(svg: &str, w: f32, h: f32, scale: f32) -> Result<egui::ColorImage, String> {
    use resvg::tiny_skia::{Pixmap, Transform};
    use resvg::usvg::{Options, Tree};

    let mut opt = Options::default();
    {
        let db = opt.fontdb_mut();
        db.load_system_fonts();
        // fontdb maps the generic `sans-serif` to "Arial" by default, which is
        // absent on Linux, so `<text font-family="sans-serif">` would resolve to
        // nothing. Point the generics at fonts that are actually installed.
        db.set_sans_serif_family("Liberation Sans");
        db.set_serif_family("Liberation Serif");
        db.set_monospace_family("Liberation Mono");
    }

    let tree = Tree::from_str(svg, &opt).map_err(|e| format!("svg parse: {e}"))?;

    let pw = ((w * scale).ceil() as u32).max(1);
    let ph = ((h * scale).ceil() as u32).max(1);
    let mut pixmap = Pixmap::new(pw, ph).ok_or_else(|| format!("pixmap alloc {pw}×{ph}"))?;
    resvg::render(
        &tree,
        Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    Ok(egui::ColorImage::from_rgba_premultiplied(
        [pw as usize, ph as usize],
        pixmap.data(),
    ))
}

fn main() -> eframe::Result {
    eframe::run_native(
        "hiker-mermaid gallery",
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::new(GalleryApp::default()))),
    )
}
