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
        // A state diagram.
        (
            "state",
            "stateDiagram-v2\n    [*] --> Idle\n    Idle --> Running : start\n    Running --> Idle : stop\n    Running --> [*] : exit",
        ),
        // An ER diagram.
        (
            "er",
            "erDiagram\n    CUSTOMER ||--o{ ORDER : places\n    ORDER ||--|{ LINE_ITEM : contains\n    CUSTOMER }|..|{ ADDRESS : uses",
        ),
        // A class diagram.
        (
            "class",
            "classDiagram\n    Animal <|-- Dog\n    Animal <|-- Cat\n    class Animal {\n      +int age\n      +String name\n      +eat() void\n    }\n    class Dog {\n      +bark() void\n    }",
        ),
        // A mindmap.
        (
            "mindmap",
            "mindmap\n  root((mermaid))\n    Origins\n      Long history\n    Uses\n      Docs\n      Diagrams\n    Tools\n      Editor",
        ),
        // A gantt chart.
        (
            "gantt",
            "gantt\n    title Project\n    dateFormat YYYY-MM-DD\n    section Design\n    Spec :done, a1, 2024-01-01, 5d\n    Mockups :active, a2, after a1, 4d\n    section Build\n    Code :a3, after a2, 8d\n    Launch :milestone, m1, after a3, 0d",
        ),
        // A user journey.
        (
            "journey",
            "journey
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
        ),
        // A quadrant chart.
        (
            "quadrant",
            "quadrantChart\n    title Reach vs Effort\n    x-axis Low Effort --> High Effort\n    y-axis Low Reach --> High Reach\n    quadrant-1 Do now\n    quadrant-2 Plan\n    quadrant-3 Skip\n    quadrant-4 Maybe\n    Campaign A: [0.3, 0.6]\n    Campaign B: [0.45, 0.23]\n    Campaign C: [0.57, 0.69]",
        ),
        // A requirement diagram.
        (
            "requirement",
            "requirementDiagram\n    requirement test_req {\n      id: 1\n      text: the system shall work\n      risk: high\n      verifymethod: test\n    }\n    element test_entity {\n      type: simulation\n    }\n    test_entity - satisfies -> test_req",
        ),
        // A git graph.
        (
            "gitgraph",
            "gitGraph\n    commit id: \"init\"\n    branch dev\n    checkout dev\n    commit\n    commit tag: \"v1\"\n    checkout main\n    merge dev\n    commit",
        ),
        // An xy chart.
        (
            "xychart",
            "xychart-beta\n    title Monthly revenue\n    x-axis [Jan, Feb, Mar, Apr, May]\n    y-axis Revenue 0 --> 100\n    bar [30, 55, 40, 80, 65]\n    line [20, 45, 50, 70, 60]",
        ),
        // A radar chart.
        (
            "radar",
            "radar-beta\n    title Skills\n    axis a[\"Speed\"], b[\"Power\"], c[\"Range\"], d[\"Defense\"], e[\"Magic\"]\n    curve hero{ 80, 60, 70, 50, 90 }\n    curve rival{ 50, 90, 40, 80, 30 }\n    max 100",
        ),
        // A timeline.
        (
            "timeline",
            "timeline\n    title History of the web\n    section Early\n    1990 : Tim invents the web\n    1993 : Mosaic browser\n    section Growth\n    1995 : JavaScript : PHP\n    2004 : Web 2.0",
        ),
        // A kanban board.
        (
            "kanban",
            "kanban\n    Todo\n      Write spec\n      Draft API\n    In Progress\n      Build parser\n    Done\n      Set up repo\n      CI pipeline",
        ),
        // A sankey flow.
        (
            "sankey",
            "sankey-beta\nCoal,Electricity,25\nGas,Electricity,15\nElectricity,Homes,20\nElectricity,Industry,20\nGas,Heating,10",
        ),
        // A treemap.
        (
            "treemap",
            "treemap-beta\ntitle Storage\n\"Media\"\n    \"Photos\": 40\n    \"Video\": 80\n\"Docs\"\n    \"Work\": 30\n    \"Personal\": 15\n\"Apps\": 25",
        ),
        // A C4 context diagram.
        (
            "c4",
            "C4Context\n    Person(user, \"Customer\", \"A bank customer\")\n    System(bank, \"Online Banking\", \"Lets customers view accounts\")\n    System_Ext(email, \"Email System\", \"Sends emails\")\n    Rel(user, bank, \"Uses\")\n    Rel(bank, email, \"Sends mail via\")",
        ),
        // A packet diagram.
        (
            "packet",
            "packet-beta\ntitle TCP header\n0-15: \"Source Port\"\n16-31: \"Destination Port\"\n32-63: \"Sequence Number\"\n64-95: \"Acknowledgment Number\"",
        ),
        // A block diagram.
        (
            "block",
            "block-beta\n    columns 3\n    a[\"Frontend\"] b[\"API\"] c[\"Database\"]\n    a --> b\n    b --> c",
        ),
        // A venn diagram.
        (
            "venn",
            "venn\n    title Hobbies\n    set \"Music\": Alice, Bob, Carol\n    set \"Sports\": Bob, Carol, Dave\n    set \"Art\": Carol, Eve",
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
