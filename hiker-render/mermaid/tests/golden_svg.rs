//! Golden-SVG snapshot corpus — a behavior-preserving safety net.
//!
//! Captures the CURRENT SVG output of [`hiker_mermaid::render`] for a broad set
//! of mermaid inputs as committed golden `.svg` files under `tests/golden/`.
//! A later refactor that splits the big per-diagram modules can be verified
//! byte-for-byte by `cargo test -p hiker-mermaid`.
//!
//! Usage:
//! - `cargo test -p hiker-mermaid golden` — assert each case matches its golden.
//! - `UPDATE_GOLDENS=1 cargo test -p hiker-mermaid golden` — (re)write goldens.
//!
//! Zero extra deps: std + the crate only. Rendering uses
//! `MermaidOptions::default()` so output is theme-stable.
//!
//! Cases that currently `Err` or `panic` in the engine are intentionally NOT in
//! the corpus below — they are pre-existing issues, listed in the test report,
//! and out of scope for the snapshot net.

use hiker_mermaid::{render, MermaidOptions};
use std::path::PathBuf;

/// One snapshot case: a stable file-safe `name` and the mermaid `src`.
struct Case {
    name: &'static str,
    src: &'static str,
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn update_mode() -> bool {
    std::env::var("UPDATE_GOLDENS").map(|v| v != "0" && !v.is_empty()).unwrap_or(false)
}

/// Render a case's source to SVG with default options.
fn render_svg(src: &str) -> String {
    let opts = MermaidOptions::default();
    render(src, &opts)
        .unwrap_or_else(|e| panic!("render failed (expected to succeed): {e:?}"))
        .svg
}

#[test]
fn determinism() {
    // Every corpus input must render identically across two independent calls —
    // no random ids, timestamps, or HashMap-iteration-order leakage into SVG.
    let opts = MermaidOptions::default();
    for case in CASES {
        let a = render(case.src, &opts).expect("render a").svg;
        let b = render(case.src, &opts).expect("render b").svg;
        assert_eq!(a, b, "non-deterministic SVG for case {:?}", case.name);
    }
}

#[test]
fn golden() {
    let dir = golden_dir();
    let update = update_mode();
    if update {
        std::fs::create_dir_all(&dir).expect("create golden dir");
    }

    // Guard: case names must be unique (one golden file each).
    let mut seen = std::collections::HashSet::new();
    for c in CASES {
        assert!(seen.insert(c.name), "duplicate case name {:?}", c.name);
    }

    let mut mismatches = Vec::new();
    for case in CASES {
        let svg = render_svg(case.src);
        let path = dir.join(format!("{}.svg", case.name));

        if update {
            std::fs::write(&path, &svg)
                .unwrap_or_else(|e| panic!("write golden {path:?}: {e}"));
            continue;
        }

        let expected = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                mismatches.push(format!("{}: missing golden ({e}) — run UPDATE_GOLDENS=1", case.name));
                continue;
            }
        };
        if expected != svg {
            mismatches.push(format!(
                "{}: SVG differs from golden ({} vs {} bytes)",
                case.name,
                svg.len(),
                expected.len()
            ));
        }
    }

    if update {
        eprintln!("updated {} golden files in {}", CASES.len(), dir.display());
        return;
    }
    assert!(
        mismatches.is_empty(),
        "{} golden mismatch(es):\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

/// The corpus. Multiple feature-exercising cases per supported diagram type.
const CASES: &[Case] = &[
    // ---------------------------------------------------------------- flowchart
    Case { name: "flowchart_td_basic", src: "graph TD; A[Start]-->B{OK?}; B-->|yes|C(Done); B-->|no|A" },
    Case { name: "flowchart_lr_basic", src: "graph LR\n A --> B --> C --> D" },
    Case { name: "flowchart_bt", src: "flowchart BT\n A --> B\n B --> C" },
    Case { name: "flowchart_rl", src: "flowchart RL\n A --> B --> C" },
    Case { name: "flowchart_shapes", src: "flowchart TD\n A[Rect] --> B(Round)\n A --> C{Diamond}\n A --> D((Circle))\n A --> E([Stadium])\n A --> F[[Subroutine]]\n A --> G[(Database)]" },
    Case { name: "flowchart_edge_kinds", src: "flowchart LR\n A --> B\n B --- C\n C -.-> D\n D ==> E\n E -.- F" },
    Case { name: "flowchart_edge_labels", src: "flowchart TD\n A -->|go| B\n B ---|plain| C\n C -.->|maybe| D\n D ==>|thick| E" },
    Case { name: "flowchart_subgraphs", src: "flowchart TD\n    subgraph one [Frontend]\n        A[UI] --> B[Router]\n    end\n    subgraph two [Backend]\n        C[API] --> D[Database]\n    end\n    B --> C" },
    Case { name: "flowchart_classdef_style", src: "graph TD\n    A[Start]:::hot --> B{Check}\n    B -->|ok| C[Process]:::cool\n    B -->|fail| D[Reject]\n    style D fill:#fdd,stroke:#c00,stroke-width:3px\n    classDef hot fill:#ffb3b3,stroke:#c00,stroke-width:3px\n    classDef cool fill:#b3d9ff,stroke:#06c,stroke-width:2px" },
    Case { name: "flowchart_click", src: "graph TD\n    A[Homepage] --> B[Docs]\n    A --> C[Login]\n    C --> D[Dashboard]\n    click A \"https://example.com\" \"Open the homepage\"\n    click B \"https://example.com/docs\" \"Read the docs\"" },
    Case { name: "flowchart_chained", src: "flowchart LR\n A --> B & C --> D" },

    // ----------------------------------------------------------------- sequence
    Case { name: "sequence_basic", src: "sequenceDiagram\n    participant A as Alice\n    participant B as Bob\n    A->>B: Hello Bob\n    B-->>A: Hi Alice\n    A->>B: How are you?\n    B->>B: thinking\n    B-->>A: Great!" },
    Case { name: "sequence_actors", src: "sequenceDiagram\n    actor Alice\n    actor Bob\n    Alice->>Bob: Hi\n    Bob-->>Alice: Hello" },
    Case { name: "sequence_activations", src: "sequenceDiagram\n    A->>+B: Request\n    B-->>-A: Response\n    A->>+B: Again\n    activate B\n    B-->>-A: One\n    B-->>A: Two\n    deactivate B" },
    Case { name: "sequence_loop_alt_opt", src: "sequenceDiagram\n    participant A as Alice\n    participant B as Bob\n    A->>+B: Request\n    Note over A,B: handshake\n    loop every minute\n        B-->>A: heartbeat\n    end\n    alt success\n        B->>A: data\n    else failure\n        B->>A: error\n    end\n    opt extra\n        B->>A: bonus\n    end\n    B-->>-A: done" },
    Case { name: "sequence_par", src: "sequenceDiagram\n    A->>B: start\n    par to B\n        A->>B: one\n    and to C\n        A->>C: two\n    end\n    B-->>A: done" },
    Case { name: "sequence_notes", src: "sequenceDiagram\n    participant A\n    participant B\n    Note left of A: left note\n    Note right of B: right note\n    Note over A,B: over both\n    A->>B: msg" },
    Case { name: "sequence_autonumber", src: "sequenceDiagram\n    autonumber\n    A->>B: first\n    B->>A: second\n    A->>B: third" },
    Case { name: "sequence_autonumber_step", src: "sequenceDiagram\n    autonumber 10 5\n    A->>B: first\n    B->>A: second" },
    Case { name: "sequence_rect", src: "sequenceDiagram\n    A->>B: before\n    rect rgb(200,220,255)\n        B->>A: inside\n        A->>B: inside2\n    end\n    A->>B: after" },
    Case { name: "sequence_critical", src: "sequenceDiagram\n    critical connect\n        A->>B: connect\n    option timeout\n        A->>A: retry\n    end" },

    // -------------------------------------------------------------------- class
    Case { name: "class_inheritance", src: "classDiagram\n    Animal <|-- Dog\n    Animal <|-- Cat\n    class Animal {\n      +int age\n      +String name\n      +eat() void\n    }\n    class Dog {\n      +bark() void\n    }" },
    Case { name: "class_relations", src: "classDiagram\n    A <|-- B\n    C *-- D\n    E o-- F\n    G <.. H\n    I ..|> J\n    K --> L" },
    Case { name: "class_generics", src: "classDiagram\n    class List~T~ {\n      +add(T item) void\n      +get(int i) T\n    }\n    class Map~K, V~" },
    Case { name: "class_visibility", src: "classDiagram\n    class Account {\n      +String owner\n      -double balance\n      #int id\n      ~String note\n      +deposit(double amt) void\n    }" },
    Case { name: "class_annotations", src: "classDiagram\n    class Shape {\n      <<interface>>\n      +area() double\n    }\n    class Circle\n    Shape <|.. Circle" },
    Case { name: "class_cardinality", src: "classDiagram\n    Customer \"1\" --> \"*\" Order : places\n    Order \"*\" --> \"1..*\" Item" },
    Case { name: "class_namespace", src: "classDiagram\n    class Animal\n    class Dog\n    Animal <|-- Dog" },

    // -------------------------------------------------------------------- state
    Case { name: "state_basic", src: "stateDiagram-v2\n    [*] --> Idle\n    Idle --> Running : start\n    Running --> Idle : stop\n    Running --> [*] : exit" },
    Case { name: "state_v1", src: "stateDiagram\n    [*] --> Active\n    Active --> [*]" },
    Case { name: "state_composite", src: "stateDiagram-v2\n    [*] --> First\n    state First {\n        [*] --> second\n        second --> [*]\n    }\n    First --> [*]" },
    Case { name: "state_fork_join", src: "stateDiagram-v2\n    state fork_state <<fork>>\n    [*] --> fork_state\n    fork_state --> State2\n    fork_state --> State3\n    state join_state <<join>>\n    State2 --> join_state\n    State3 --> join_state\n    join_state --> [*]" },
    Case { name: "state_choice", src: "stateDiagram-v2\n    state if_state <<choice>>\n    [*] --> if_state\n    if_state --> A : yes\n    if_state --> B : no" },
    Case { name: "state_notes", src: "stateDiagram-v2\n    s1 --> s2\n    note right of s1 : a note\n    note left of s2 : another" },

    // ----------------------------------------------------------------------- er
    Case { name: "er_basic", src: "erDiagram\n    CUSTOMER ||--o{ ORDER : places\n    ORDER ||--|{ LINE_ITEM : contains\n    CUSTOMER }|..|{ ADDRESS : uses" },
    Case { name: "er_attributes", src: "erDiagram\n    CUSTOMER {\n      string name\n      string email\n      int age\n    }\n    ORDER {\n      int id\n      date created\n    }\n    CUSTOMER ||--o{ ORDER : places" },
    Case { name: "er_keys", src: "erDiagram\n    CUSTOMER {\n      int id PK\n      string email UK\n      int company_id FK\n    }" },
    Case { name: "er_cardinalities", src: "erDiagram\n    A ||--|| B : one_to_one\n    C ||--o{ D : one_to_many\n    E }o--o{ F : many_to_many\n    G }|--|{ H : required" },

    // -------------------------------------------------------------------- gantt
    Case { name: "gantt_basic", src: "gantt\n    title Project\n    dateFormat YYYY-MM-DD\n    section Design\n    Spec :done, a1, 2024-01-01, 5d\n    Mockups :active, a2, after a1, 4d\n    section Build\n    Code :a3, after a2, 8d\n    Launch :milestone, m1, after a3, 0d" },
    Case { name: "gantt_dependencies", src: "gantt\n    dateFormat YYYY-MM-DD\n    section A\n    Task1 :t1, 2024-02-01, 3d\n    Task2 :t2, after t1, 2d\n    Task3 :t3, after t2, 4d" },
    Case { name: "gantt_milestone_crit", src: "gantt\n    title Release\n    dateFormat YYYY-MM-DD\n    section Work\n    Build :crit, b1, 2024-03-01, 5d\n    Ship :milestone, m1, after b1, 0d" },

    // ------------------------------------------------------------------ gitgraph
    Case { name: "gitgraph_basic", src: "gitGraph\n    commit id: \"init\"\n    branch dev\n    checkout dev\n    commit\n    commit tag: \"v1\"\n    checkout main\n    merge dev\n    commit" },
    Case { name: "gitgraph_multi_branch", src: "gitGraph\n    commit\n    branch feature\n    checkout feature\n    commit\n    checkout main\n    branch hotfix\n    commit\n    checkout main\n    merge hotfix\n    merge feature" },
    Case { name: "gitgraph_cherry", src: "gitGraph\n    commit id: \"a\"\n    branch dev\n    commit id: \"b\"\n    checkout main\n    cherry-pick id: \"b\"" },

    // ------------------------------------------------------------------- journey
    Case { name: "journey_basic", src: "journey\n    title Online shopping experience\n    section Browse\n      Visit store: 5: Customer\n      Search product: 3: Customer\n    section Buy\n      Add to cart: 4: Customer\n      Checkout: 2: Customer\n    section After\n      Track order: 3: Customer\n      Receive item: 5: Customer" },
    Case { name: "journey_multi_actor", src: "journey\n    title Team workflow\n    section Plan\n      Write spec: 4: Alice, Bob\n      Review: 3: Bob\n    section Do\n      Implement: 5: Alice" },

    // ------------------------------------------------------------------- mindmap
    Case { name: "mindmap_basic", src: "mindmap\n  root((mermaid))\n    Origins\n      Long history\n    Uses\n      Docs\n      Diagrams\n    Tools\n      Editor" },
    Case { name: "mindmap_shapes", src: "mindmap\n  root((center))\n    A[square]\n    B(round)\n    C))cloud((\n    D{{hexagon}}" },

    // ----------------------------------------------------------------------- pie
    Case { name: "pie_basic", src: "pie showData title Pet ownership\n    \"Dogs\" : 386\n    \"Cats\" : 85\n    \"Rats\" : 15" },
    Case { name: "pie_no_title", src: "pie\n    \"A\" : 10\n    \"B\" : 20\n    \"C\" : 30" },
    Case { name: "pie_many_slices", src: "pie title Languages\n    \"Rust\" : 40\n    \"Go\" : 25\n    \"Python\" : 20\n    \"JS\" : 10\n    \"C\" : 5" },

    // ------------------------------------------------------------------ quadrant
    Case { name: "quadrant_basic", src: "quadrantChart\n    title Reach vs Effort\n    x-axis Low Effort --> High Effort\n    y-axis Low Reach --> High Reach\n    quadrant-1 Do now\n    quadrant-2 Plan\n    quadrant-3 Skip\n    quadrant-4 Maybe\n    Campaign A: [0.3, 0.6]\n    Campaign B: [0.45, 0.23]\n    Campaign C: [0.57, 0.69]" },
    Case { name: "quadrant_minimal", src: "quadrantChart\n    x-axis Low --> High\n    y-axis Low --> High\n    Point A: [0.2, 0.8]\n    Point B: [0.7, 0.3]" },

    // --------------------------------------------------------------- requirement
    Case { name: "requirement_basic", src: "requirementDiagram\n    requirement test_req {\n      id: 1\n      text: the system shall work\n      risk: high\n      verifymethod: test\n    }\n    element test_entity {\n      type: simulation\n    }\n    test_entity - satisfies -> test_req" },
    Case { name: "requirement_types", src: "requirementDiagram\n    functionalRequirement fr {\n      id: 2\n      text: do the thing\n      risk: low\n      verifymethod: inspection\n    }\n    element e {\n      type: word doc\n    }\n    e - traces -> fr" },

    // -------------------------------------------------------------------- xychart
    Case { name: "xychart_bar_line", src: "xychart-beta\n    title Monthly revenue\n    x-axis [Jan, Feb, Mar, Apr, May]\n    y-axis Revenue 0 --> 100\n    bar [30, 55, 40, 80, 65]\n    line [20, 45, 50, 70, 60]" },
    Case { name: "xychart_bar_only", src: "xychart-beta\n    x-axis [A, B, C]\n    y-axis 0 --> 50\n    bar [10, 30, 20]" },

    // --------------------------------------------------------------------- radar
    Case { name: "radar_basic", src: "radar-beta\n    title Skills\n    axis a[\"Speed\"], b[\"Power\"], c[\"Range\"], d[\"Defense\"], e[\"Magic\"]\n    curve hero{ 80, 60, 70, 50, 90 }\n    curve rival{ 50, 90, 40, 80, 30 }\n    max 100" },
    Case { name: "radar_single", src: "radar-beta\n    axis a[\"A\"], b[\"B\"], c[\"C\"]\n    curve only{ 30, 60, 90 }\n    max 100" },

    // ------------------------------------------------------------------ timeline
    Case { name: "timeline_basic", src: "timeline\n    title History of the web\n    section Early\n    1990 : Tim invents the web\n    1993 : Mosaic browser\n    section Growth\n    1995 : JavaScript : PHP\n    2004 : Web 2.0" },
    Case { name: "timeline_no_sections", src: "timeline\n    title Milestones\n    2020 : Founded\n    2021 : First release\n    2022 : v2" },

    // --------------------------------------------------------------------- kanban
    Case { name: "kanban_basic", src: "kanban\n    Todo\n      Write spec\n      Draft API\n    In Progress\n      Build parser\n    Done\n      Set up repo\n      CI pipeline" },
    Case { name: "kanban_two_col", src: "kanban\n    Backlog\n      Idea 1\n      Idea 2\n    Active\n      Doing it" },

    // --------------------------------------------------------------------- sankey
    Case { name: "sankey_basic", src: "sankey-beta\nCoal,Electricity,25\nGas,Electricity,15\nElectricity,Homes,20\nElectricity,Industry,20\nGas,Heating,10" },
    Case { name: "sankey_chain", src: "sankey-beta\nA,B,10\nB,C,10\nC,D,10" },

    // -------------------------------------------------------------------- treemap
    Case { name: "treemap_basic", src: "treemap-beta\ntitle Storage\n\"Media\"\n    \"Photos\": 40\n    \"Video\": 80\n\"Docs\"\n    \"Work\": 30\n    \"Personal\": 15\n\"Apps\": 25" },
    Case { name: "treemap_flat", src: "treemap-beta\n\"A\": 10\n\"B\": 20\n\"C\": 30" },

    // ------------------------------------------------------------------------ c4
    Case { name: "c4_context", src: "C4Context\n    Person(user, \"Customer\", \"A bank customer\")\n    System(bank, \"Online Banking\", \"Lets customers view accounts\")\n    System_Ext(email, \"Email System\", \"Sends emails\")\n    Rel(user, bank, \"Uses\")\n    Rel(bank, email, \"Sends mail via\")" },
    Case { name: "c4_container", src: "C4Container\n    Person(user, \"User\")\n    Container(web, \"Web App\", \"React\")\n    Container(api, \"API\", \"Rust\")\n    Rel(user, web, \"Uses\")\n    Rel(web, api, \"Calls\")" },

    // -------------------------------------------------------------------- packet
    Case { name: "packet_basic", src: "packet-beta\ntitle TCP header\n0-15: \"Source Port\"\n16-31: \"Destination Port\"\n32-63: \"Sequence Number\"\n64-95: \"Acknowledgment Number\"" },
    Case { name: "packet_small", src: "packet-beta\n0-3: \"Ver\"\n4-7: \"IHL\"\n8-15: \"ToS\"" },

    // --------------------------------------------------------------------- block
    Case { name: "block_basic", src: "block-beta\n    columns 3\n    a[\"Frontend\"] b[\"API\"] c[\"Database\"]\n    a --> b\n    b --> c" },
    Case { name: "block_columns", src: "block-beta\n    columns 2\n    a b\n    c d" },

    // ---------------------------------------------------------------------- venn
    Case { name: "venn_basic", src: "venn\n    title Hobbies\n    set \"Music\": Alice, Bob, Carol\n    set \"Sports\": Bob, Carol, Dave\n    set \"Art\": Carol, Eve" },
    Case { name: "venn_two_sets", src: "venn\n    set \"A\": x, y, z\n    set \"B\": y, z, w" },

    // -------------------------------------------------------------- architecture
    Case { name: "architecture_basic", src: "architecture-beta\n    group api(cloud)[API]\n    service db(database)[Database] in api\n    service server(server)[Server] in api\n    db:L -- R:server" },
    Case { name: "architecture_groups", src: "architecture-beta\n    group a(cloud)[Cloud]\n    group b(server)[On Prem]\n    service s1(server)[S1] in a\n    service s2(disk)[S2] in b\n    s1:R -- L:s2" },

    // ------------------------------------------------------------------- cynefin
    Case { name: "cynefin_basic", src: "cynefin-beta\n    title Cynefin\n    complex\n        \"New product\"\n    complicated\n        \"Scaling up\"\n    clear\n        \"Run payroll\"\n    chaotic\n        \"Site outage\"" },

    // ------------------------------------------------------------ eventmodeling
    Case { name: "eventmodeling_basic", src: "eventmodeling\n    tf 1 ui Order.Form\n    tf 2 cmd Order.Place\n    tf 3 evt Order.Placed\n    tf 4 rmo Order.List" },

    // ----------------------------------------------------------------------- info
    Case { name: "info_basic", src: "info" },

    // ------------------------------------------------------------------- ishikawa
    Case { name: "ishikawa_basic", src: "ishikawa\n    Defects\n        Machine\n            Wear\n        Method\n            Unclear steps\n        Material\n            Bad supplier" },
    Case { name: "ishikawa_fishbone_kw", src: "fishbone\n    Problem\n        People\n            Training\n        Process\n            Steps" },

    // ------------------------------------------------------------------- railroad
    Case { name: "railroad_ebnf", src: "railroad-ebnf\n    expr = term { \"+\" term } ;\n    term = \"a\" | \"b\" | ( expr ) ;" },

    // ------------------------------------------------------------------- treeview
    Case { name: "treeview_basic", src: "treeView-beta\n    src\n      main.rs\n      lib.rs\n    tests\n      smoke.rs" },

    // -------------------------------------------------------------------- wardley
    Case { name: "wardley_basic", src: "wardley-beta\n    title Tea Shop\n    component Customer [0.9, 0.5]\n    component Cup of Tea [0.7, 0.6]\n    component Kettle [0.3, 0.8]\n    Customer -> Cup of Tea\n    Cup of Tea -> Kettle" },

    // --------------------------------------------------------- config/frontmatter
    Case { name: "config_init_theme", src: "%%{init: {\"theme\": \"dark\"}}%%\ngraph TD\n A --> B" },
    Case { name: "config_frontmatter_title", src: "---\ntitle: My Chart\n---\ngraph LR\n A --> B" },
];
