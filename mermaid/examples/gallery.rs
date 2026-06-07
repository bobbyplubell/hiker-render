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
use hiker_mermaid::{
    HitRegion, Look, MermaidError, MermaidOptions, MermaidRender, MermaidTheme, render_with_regions,
};

/// Display name for a theme.
fn theme_name(t: MermaidTheme) -> &'static str {
    match t {
        MermaidTheme::Default => "Default",
        MermaidTheme::Dark => "Dark",
        MermaidTheme::Forest => "Forest",
        MermaidTheme::Neutral => "Neutral",
        MermaidTheme::Base => "Base",
    }
}

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
    State,
    Er,
    Class,
    Mindmap,
    Gantt,
    Journey,
    Quadrant,
    Requirement,
    GitGraph,
    XyChart,
    Radar,
    Timeline,
    Kanban,
    Sankey,
    Treemap,
    C4,
    Packet,
    Block,
    Venn,
    Architecture,
    Cynefin,
    EventModeling,
    Info,
    Ishikawa,
    Railroad,
    TreeView,
    Wardley,
}

impl Group {
    fn label(self) -> &'static str {
        match self {
            Group::Flowchart => "Flowchart",
            Group::Pie => "Pie",
            Group::Sequence => "Sequence",
            Group::State => "State",
            Group::Er => "ER",
            Group::Class => "Class",
            Group::Mindmap => "Mindmap",
            Group::Gantt => "Gantt",
            Group::Journey => "Journey",
            Group::Quadrant => "Quadrant",
            Group::Requirement => "Requirement",
            Group::GitGraph => "Git",
            Group::XyChart => "XY Chart",
            Group::Radar => "Radar",
            Group::Timeline => "Timeline",
            Group::Kanban => "Kanban",
            Group::Sankey => "Sankey",
            Group::Treemap => "Treemap",
            Group::C4 => "C4",
            Group::Packet => "Packet",
            Group::Block => "Block",
            Group::Venn => "Venn",
            Group::Architecture => "Architecture",
            Group::Cynefin => "Cynefin",
            Group::EventModeling => "Event Modeling",
            Group::Info => "Info",
            Group::Ishikawa => "Ishikawa",
            Group::Railroad => "Railroad",
            Group::TreeView => "Tree View",
            Group::Wardley => "Wardley",
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
            name: "Styled (classDef)",
            group: Group::Flowchart,
            src: "graph TD\n    A[Start]:::hot --> B{Check}\n    B -->|ok| C[Process]:::cool\n    B -->|fail| D[Reject]\n    style D fill:#fdd,stroke:#c00,stroke-width:3px\n    classDef hot fill:#ffb3b3,stroke:#c00,stroke-width:3px\n    classDef cool fill:#b3d9ff,stroke:#06c,stroke-width:2px",
        },
        Example {
            name: "Interactive (click)",
            group: Group::Flowchart,
            src: "graph TD\n    A[Homepage] --> B[Docs]\n    A --> C[Login]\n    C --> D[Dashboard]\n    click A \"https://example.com\" \"Open the homepage\"\n    click B \"https://example.com/docs\" \"Read the docs\"\n    click D call openDashboard() \"Run a callback\"",
        },
        Example {
            name: "Subgraphs",
            group: Group::Flowchart,
            src: "flowchart TD\n    subgraph one [Frontend]\n        A[UI] --> B[Router]\n    end\n    subgraph two [Backend]\n        C[API] --> D[Database]\n    end\n    B --> C",
        },
        Example {
            name: "Pets",
            group: Group::Pie,
            src: "pie showData title Pet ownership\n    \"Dogs\" : 386\n    \"Cats\" : 85\n    \"Rats\" : 15",
        },
        Example {
            name: "Greeting",
            group: Group::Sequence,
            src: "sequenceDiagram\n    participant A as Alice\n    participant B as Bob\n    A->>B: Hello Bob\n    B-->>A: Hi Alice\n    A->>B: How are you?\n    B->>B: thinking\n    B-->>A: Great!",
        },
        Example {
            name: "Loops & alt",
            group: Group::Sequence,
            src: "sequenceDiagram\n    participant A as Alice\n    participant B as Bob\n    A->>+B: Request\n    Note over A,B: handshake\n    loop every minute\n        B-->>A: heartbeat\n    end\n    alt success\n        B->>A: data\n    else failure\n        B->>A: error\n    end\n    B-->>-A: done",
        },
        Example {
            name: "Lifecycle",
            group: Group::State,
            src: "stateDiagram-v2\n    [*] --> Idle\n    Idle --> Running : start\n    Running --> Idle : stop\n    Running --> [*] : exit",
        },
        Example {
            name: "Orders",
            group: Group::Er,
            src: "erDiagram\n    CUSTOMER ||--o{ ORDER : places\n    ORDER ||--|{ LINE_ITEM : contains\n    CUSTOMER }|..|{ ADDRESS : uses",
        },
        Example {
            name: "Animals",
            group: Group::Class,
            src: "classDiagram\n    Animal <|-- Dog\n    Animal <|-- Cat\n    class Animal {\n      +int age\n      +String name\n      +eat() void\n    }\n    class Dog {\n      +bark() void\n    }",
        },
        Example {
            name: "Topics",
            group: Group::Mindmap,
            src: "mindmap\n  root((mermaid))\n    Origins\n      Long history\n    Uses\n      Docs\n      Diagrams\n    Tools\n      Editor",
        },
        Example {
            name: "Project",
            group: Group::Gantt,
            src: "gantt\n    title Project\n    dateFormat YYYY-MM-DD\n    section Design\n    Spec :done, a1, 2024-01-01, 5d\n    Mockups :active, a2, after a1, 4d\n    section Build\n    Code :a3, after a2, 8d\n    Launch :milestone, m1, after a3, 0d",
        },
        Example {
            name: "Online shopping",
            group: Group::Journey,
            src: "journey
    title Online shopping experience
    section Browse
      Visit store: 5: Customer
      Search product: 3: Customer
    section Buy
      Add to cart: 4: Customer
      Checkout: 2: Customer
    section After
      Track order: 3: Customer
      Receive item: 5: Customer",
        },
        Example {
            name: "Reach vs Effort",
            group: Group::Quadrant,
            src: "quadrantChart\n    title Reach vs Effort\n    x-axis Low Effort --> High Effort\n    y-axis Low Reach --> High Reach\n    quadrant-1 Do now\n    quadrant-2 Plan\n    quadrant-3 Skip\n    quadrant-4 Maybe\n    Campaign A: [0.3, 0.6]\n    Campaign B: [0.45, 0.23]\n    Campaign C: [0.57, 0.69]",
        },
        Example {
            name: "Satisfies",
            group: Group::Requirement,
            src: "requirementDiagram\n    requirement test_req {\n      id: 1\n      text: the system shall work\n      risk: high\n      verifymethod: test\n    }\n    element test_entity {\n      type: simulation\n    }\n    test_entity - satisfies -> test_req",
        },
        Example {
            name: "Branch & merge",
            group: Group::GitGraph,
            src: "gitGraph\n    commit id: \"init\"\n    branch dev\n    checkout dev\n    commit\n    commit tag: \"v1\"\n    checkout main\n    merge dev\n    commit",
        },
        Example {
            name: "Revenue",
            group: Group::XyChart,
            src: "xychart-beta\n    title Monthly revenue\n    x-axis [Jan, Feb, Mar, Apr, May]\n    y-axis Revenue 0 --> 100\n    bar [30, 55, 40, 80, 65]\n    line [20, 45, 50, 70, 60]",
        },
        Example {
            name: "Skills",
            group: Group::Radar,
            src: "radar-beta\n    title Skills\n    axis a[\"Speed\"], b[\"Power\"], c[\"Range\"], d[\"Defense\"], e[\"Magic\"]\n    curve hero{ 80, 60, 70, 50, 90 }\n    curve rival{ 50, 90, 40, 80, 30 }\n    max 100",
        },
        Example {
            name: "History of the web",
            group: Group::Timeline,
            src: "timeline\n    title History of the web\n    section Early\n    1990 : Tim invents the web\n    1993 : Mosaic browser\n    section Growth\n    1995 : JavaScript : PHP\n    2004 : Web 2.0",
        },
        Example {
            name: "Board",
            group: Group::Kanban,
            src: "kanban\n    Todo\n      Write spec\n      Draft API\n    In Progress\n      Build parser\n    Done\n      Set up repo\n      CI pipeline",
        },
        Example {
            name: "Energy",
            group: Group::Sankey,
            src: "sankey-beta\nCoal,Electricity,25\nGas,Electricity,15\nElectricity,Homes,20\nElectricity,Industry,20\nGas,Heating,10",
        },
        Example {
            name: "Storage",
            group: Group::Treemap,
            src: "treemap-beta\ntitle Storage\n\"Media\"\n    \"Photos\": 40\n    \"Video\": 80\n\"Docs\"\n    \"Work\": 30\n    \"Personal\": 15\n\"Apps\": 25",
        },
        Example {
            name: "Banking",
            group: Group::C4,
            src: "C4Context\n    Person(user, \"Customer\", \"A bank customer\")\n    System(bank, \"Online Banking\", \"Lets customers view accounts\")\n    System_Ext(email, \"Email System\", \"Sends emails\")\n    Rel(user, bank, \"Uses\")\n    Rel(bank, email, \"Sends mail via\")",
        },
        Example {
            name: "TCP header",
            group: Group::Packet,
            src: "packet-beta\ntitle TCP header\n0-15: \"Source Port\"\n16-31: \"Destination Port\"\n32-63: \"Sequence Number\"\n64-95: \"Acknowledgment Number\"",
        },
        Example {
            name: "Services",
            group: Group::Block,
            src: "block-beta\n    columns 3\n    a[\"Frontend\"] b[\"API\"] c[\"Database\"]\n    a --> b\n    b --> c",
        },
        Example {
            name: "Hobbies",
            group: Group::Venn,
            src: "venn\n    title Hobbies\n    set \"Music\": Alice, Bob, Carol\n    set \"Sports\": Bob, Carol, Dave\n    set \"Art\": Carol, Eve",
        },
        Example {
            name: "API",
            group: Group::Architecture,
            src: "architecture-beta\n    group api(cloud)[API]\n    service db(database)[Database] in api\n    service server(server)[Server] in api\n    db:L -- R:server",
        },
        Example {
            name: "Cynefin",
            group: Group::Cynefin,
            src: "cynefin-beta\n    title Cynefin\n    complex\n        \"New product\"\n    complicated\n        \"Scaling up\"\n    clear\n        \"Run payroll\"\n    chaotic\n        \"Site outage\"",
        },
        Example {
            name: "Order flow",
            group: Group::EventModeling,
            src: "eventmodeling\n    tf 1 ui Order.Form\n    tf 2 cmd Order.Place\n    tf 3 evt Order.Placed\n    tf 4 rmo Order.List",
        },
        Example {
            name: "About",
            group: Group::Info,
            src: "info",
        },
        Example {
            name: "Defects",
            group: Group::Ishikawa,
            src: "ishikawa\n    Defects\n        Machine\n            Wear\n        Method\n            Unclear steps\n        Material\n            Bad supplier",
        },
        Example {
            name: "EBNF",
            group: Group::Railroad,
            src: "railroad-ebnf\n    expr = term { \"+\" term } ;\n    term = \"a\" | \"b\" | ( expr ) ;",
        },
        Example {
            name: "Project files",
            group: Group::TreeView,
            src: "treeView-beta\n    src\n      main.rs\n      lib.rs\n    tests\n      smoke.rs",
        },
        Example {
            name: "Tea Shop",
            group: Group::Wardley,
            src: "wardley-beta\n    title Tea Shop\n    component Customer [0.9, 0.5]\n    component Cup of Tea [0.7, 0.6]\n    component Kettle [0.3, 0.8]\n    Customer -> Cup of Tea\n    Cup of Tea -> Kettle",
        },
    ]
}

/// A successfully rasterized diagram: the GPU texture plus the source diagram's
/// pixel size (the SVG `width_px`×`height_px`, before zoom).
struct Rendered {
    texture: egui::TextureHandle,
    diagram_w: f32,
    diagram_h: f32,
    /// Interactive hit regions (flowchart node `click`/tooltip data), in diagram px.
    regions: Vec<HitRegion>,
}

struct GalleryApp {
    examples: Vec<Example>,
    /// Index of the currently-selected built-in example (for list highlight).
    selected: Option<usize>,
    /// Live editor contents — the source actually rendered.
    source: String,
    /// Selected color theme.
    theme: MermaidTheme,
    /// Hand-drawn (sketchy) look.
    hand_drawn: bool,

    /// Last (source, scale, theme) we rasterized for, so we only re-render on change.
    last_key: Option<(String, u32, MermaidTheme, bool)>,
    /// Current render result, or the error message to show in red.
    current: Result<Rendered, String>,

    /// Zoom: display scale of the diagram in the view.
    zoom: f32,
    /// Rasterization supersampling factor (for crispness), tied to zoom.
    raster_scale: f32,
    /// Show a checkered background behind the (possibly transparent) diagram.
    checkered: bool,
    /// Last interaction (a node was clicked) — shown in a status line.
    last_click: Option<String>,
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
            theme: MermaidTheme::Default,
            hand_drawn: false,
            last_key: None,
            current: Err("not yet rendered".to_string()),
            zoom: 1.0,
            raster_scale: 2.0,
            checkered: false,
            last_click: None,
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
        let key = (self.source.clone(), scale_key, self.theme, self.hand_drawn);
        if self.last_key.as_ref() == Some(&key) {
            return;
        }
        self.last_key = Some(key);
        let mut opts = MermaidOptions::theme(self.theme);
        if self.hand_drawn {
            opts.look = Look::HandDrawn;
        }
        self.current = render_to_texture(ctx, &self.source, &opts, scale);
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
                    // All groups present in the example set, in first-appearance
                    // order — so every diagram type shows up automatically.
                    let mut groups: Vec<Group> = Vec::new();
                    for ex in &self.examples {
                        if !groups.contains(&ex.group) {
                            groups.push(ex.group);
                        }
                    }
                    for group in groups {
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
                    ui.label("Theme");
                    egui::ComboBox::from_id_salt("theme")
                        .selected_text(theme_name(self.theme))
                        .show_ui(ui, |ui| {
                            for t in [
                                MermaidTheme::Default,
                                MermaidTheme::Dark,
                                MermaidTheme::Forest,
                                MermaidTheme::Neutral,
                            ] {
                                ui.selectable_value(&mut self.theme, t, theme_name(t));
                            }
                        });
                    ui.checkbox(&mut self.hand_drawn, "Hand-drawn");
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
                        if !r.regions.is_empty() {
                            ui.separator();
                            ui.label(format!("{} interactive region(s)", r.regions.len()));
                        }
                        if let Some(c) = &self.last_click {
                            ui.separator();
                            ui.colored_label(egui::Color32::from_rgb(0, 110, 200), c);
                        }
                    });
                    ui.separator();

                    let zoom = self.zoom;
                    let display = egui::vec2(r.diagram_w * zoom, r.diagram_h * zoom);
                    let mut click_msg = None;
                    egui::ScrollArea::both().show(ui, |ui| {
                        // Sense clicks so node `click` regions are interactive.
                        let (rect, response) =
                            ui.allocate_exact_size(display, egui::Sense::click());
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

                        // Hit-test the pointer (mapped into diagram px) against the
                        // regions — this is how an egui host makes a static SVG
                        // diagram interactive (hover highlight/tooltip + click).
                        if let Some(p) = response.hover_pos() {
                            let d = (p - rect.min) / zoom; // screen → diagram coords
                            if let Some(reg) = r.regions.iter().find(|reg| {
                                d.x >= reg.x
                                    && d.x <= reg.x + reg.w
                                    && d.y >= reg.y
                                    && d.y <= reg.y + reg.h
                            }) {
                                // Highlight the region.
                                let hr = egui::Rect::from_min_size(
                                    rect.min + egui::vec2(reg.x, reg.y) * zoom,
                                    egui::vec2(reg.w, reg.h) * zoom,
                                );
                                ui.painter().rect_filled(
                                    hr,
                                    3.0,
                                    egui::Color32::from_rgba_unmultiplied(0, 120, 215, 38),
                                );
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                paint_tooltip(ui, p, &region_label(reg));
                                if response.clicked() {
                                    if let Some(url) = &reg.link {
                                        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                                    }
                                    click_msg = Some(format!("clicked: {}", region_label(reg)));
                                }
                            }
                        }
                    });
                    if click_msg.is_some() {
                        self.last_click = click_msg;
                    }
                }
                Err(msg) => {
                    ui.colored_label(egui::Color32::RED, format!("Render error: {msg}"));
                }
            }
        });
    }
}

/// A one-line description of a hit region for the hover tooltip / status line.
fn region_label(reg: &HitRegion) -> String {
    let mut s = reg.id.clone();
    if let Some(t) = &reg.tooltip {
        s = format!("{s} — {t}");
    }
    if let Some(url) = &reg.link {
        s = format!("{s}  →  {url}");
    } else if let Some(cb) = &reg.callback {
        s = format!("{s}  ⇒  {cb}()");
    }
    s
}

/// Paint a small dark tooltip box with `text` just past the pointer `p`.
fn paint_tooltip(ui: &egui::Ui, p: egui::Pos2, text: &str) {
    let painter = ui.painter();
    let galley =
        painter.layout_no_wrap(text.to_owned(), egui::FontId::proportional(13.0), egui::Color32::WHITE);
    let pad = egui::vec2(6.0, 4.0);
    let origin = p + egui::vec2(14.0, 14.0);
    let box_rect = egui::Rect::from_min_size(origin, galley.size() + pad * 2.0);
    painter.rect_filled(box_rect, 4.0, egui::Color32::from_black_alpha(225));
    painter.galley(origin + pad, galley, egui::Color32::WHITE);
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
    let (
        MermaidRender {
            svg,
            width_px,
            height_px,
        },
        regions,
    ) = render_with_regions(src, opts).map_err(fmt_err)?;

    let color_image = rasterize(&svg, width_px, height_px, scale)?;
    let texture = ctx.load_texture("diagram", color_image, egui::TextureOptions::LINEAR);
    Ok(Rendered {
        texture,
        diagram_w: width_px,
        diagram_h: height_px,
        regions,
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
        // Load the exact font the renderer measured with, so glyph
        // widths match the laid-out boxes even without it installed.
        db.load_font_data(hiker_mermaid::font::FONT_BYTES.to_vec());
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
