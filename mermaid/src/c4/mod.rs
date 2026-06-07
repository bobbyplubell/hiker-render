//! `c4` diagram (`C4Context` / `C4Container` / `C4Component` / `C4Dynamic` /
//! `C4Deployment`).
//!
//! Self-contained: parse → build a `hiker_graph` layered (dagre) graph → lay
//! out → draw one SVG document. Like flowchart/state/requirement, each C4
//! element becomes a node and each relationship a directed edge.
//!
//! ## Supported subset
//!
//! Header (any of): `C4Context`, `C4Container`, `C4Component`, `C4Dynamic`,
//! `C4Deployment`.
//!
//! **Elements** (comma-separated, quoted args):
//! * `Person(id, "label", "desc"?)`, `Person_Ext(...)`.
//! * `System(id, "label", "desc"?)`, `System_Ext(...)`, `SystemDb(...)`,
//!   `SystemDb_Ext(...)`, `SystemQueue(...)`, `SystemQueue_Ext(...)`.
//! * `Container(id, "label", "tech"?, "desc"?)`, `ContainerDb(...)`,
//!   `ContainerQueue(...)` plus their `_Ext` variants.
//! * `Component(id, "label", "tech"?, "desc"?)`, `ComponentDb(...)`,
//!   `ComponentQueue(...)` plus their `_Ext` variants.
//!
//! The first arg is the id; the remaining quoted args are the label, the
//! (container/component-only) technology, and the description. Each element is
//! mapped to a [`ElemKind`] (Person / System / Container / Component) and an
//! `external` flag.
//!
//! **Relationships**: `Rel(from, to, "label", "tech"?)` plus the directional
//! variants `Rel_U`/`Rel_D`/`Rel_L`/`Rel_R` (a.k.a. `Rel_Up`/`Rel_Down`/…),
//! `Rel_Back`, and `BiRel(from, to, "label", "tech"?)`. `BiRel` adds the reverse
//! edge as well. The direction suffix is parsed but does not steer the layout
//! (rankdir stays Tb).
//!
//! **Boundaries**: `System_Boundary(id, "label") { … }`,
//! `Enterprise_Boundary(...)`, `Container_Boundary(...)`, and bare
//! `Boundary(...)`. Each becomes a dagre cluster (container node) enclosing the
//! elements declared inside its `{ … }` block; nesting is supported. After
//! layout the boundary's bounding rectangle is drawn as a dashed, faintly
//! filled rectangle with a `«System»` / `«Enterprise»` / `«Container»` type
//! label and the boundary name at the top-left, drawn behind the element boxes
//! (outermost boundaries first). Inner elements/relationships are still parsed
//! and laid out as usual.
//!
//! **Skipped** (noted): `UpdateElementStyle`, `UpdateRelStyle`,
//! `UpdateLayoutConfig`, tags, sprites/icons, `RelIndex`, and deployment-node
//! nesting (`Deployment_Node`/`Node`/`Node_L`/`Node_R` are treated as plain
//! container-like boxes via their id/label, not nested).
//!
//! Split across submodules by stage: [`model`] (parsed types), [`parse`] (text →
//! model), [`render`] (sizing + layout + SVG drawing).

mod model;
mod parse;
mod render;

use crate::{MermaidError, MermaidOptions, MermaidRender};

/// Render a mermaid `c4` diagram to SVG.
pub fn render_c4(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    render::render(src, opts)
}

#[cfg(test)]
mod tests {
    use super::model::{BoundaryKind, ElemKind};
    use super::parse::parse;
    use super::render::{EXTERNAL_FILL, PERSON_FILL};
    use super::render_c4;
    use crate::svgutil::rgb;
    use crate::{MermaidError, MermaidOptions};

    fn opts() -> MermaidOptions {
        MermaidOptions::default()
    }
    #[test]
    fn parse_person_system_rel() {
        let src = "C4Context\n\
            Person(u, \"User\", \"a user\")\n\
            System(s, \"Sys\", \"the system\")\n\
            Rel(u, s, \"uses\")";
        let d = parse(src).unwrap();
        assert_eq!(d.elements.len(), 2);

        let u = &d.elements[0];
        assert_eq!(u.id, "u");
        assert_eq!(u.label, "User");
        assert_eq!(u.kind, ElemKind::Person);
        assert!(!u.external);
        assert_eq!(u.descr, "a user");

        let s = &d.elements[1];
        assert_eq!(s.id, "s");
        assert_eq!(s.label, "Sys");
        assert_eq!(s.kind, ElemKind::System);
        assert_eq!(s.descr, "the system");

        assert_eq!(d.relationships.len(), 1);
        assert_eq!(d.relationships[0].from, "u");
        assert_eq!(d.relationships[0].to, "s");
        assert_eq!(d.relationships[0].label, "uses");
    }

    #[test]
    fn parse_external_variants() {
        let src = "C4Context\n\
            Person_Ext(pe, \"Ext Person\")\n\
            System_Ext(se, \"Ext Sys\")";
        let d = parse(src).unwrap();
        assert_eq!(d.elements.len(), 2);
        assert!(d.elements[0].external && d.elements[0].kind == ElemKind::Person);
        assert!(d.elements[1].external && d.elements[1].kind == ElemKind::System);
    }

    #[test]
    fn parse_container_component_with_tech() {
        let src = "C4Container\n\
            Container(c, \"Web App\", \"Rust\", \"serves pages\")\n\
            Component(cmp, \"Handler\", \"axum\", \"routes\")\n\
            ContainerDb(db, \"DB\", \"Postgres\", \"stores data\")";
        let d = parse(src).unwrap();
        assert_eq!(d.elements.len(), 3);

        let c = &d.elements[0];
        assert_eq!(c.kind, ElemKind::Container);
        assert_eq!(c.tech, "Rust");
        assert_eq!(c.descr, "serves pages");

        let cmp = &d.elements[1];
        assert_eq!(cmp.kind, ElemKind::Component);
        assert_eq!(cmp.tech, "axum");

        let db = &d.elements[2];
        assert_eq!(db.kind, ElemKind::Container);
        assert_eq!(db.tech, "Postgres");
    }

    #[test]
    fn quoted_args_with_commas_are_safe() {
        let src = "C4Context\n\
            System(s, \"A, B and C\", \"desc, with comma\")";
        let d = parse(src).unwrap();
        assert_eq!(d.elements.len(), 1);
        assert_eq!(d.elements[0].label, "A, B and C");
        assert_eq!(d.elements[0].descr, "desc, with comma");
    }

    #[test]
    fn rel_with_tech() {
        let src = "C4Context\n\
            System(a, \"A\")\n\
            System(b, \"B\")\n\
            Rel(a, b, \"calls\", \"HTTPS\")";
        let d = parse(src).unwrap();
        assert_eq!(d.relationships.len(), 1);
        assert_eq!(d.relationships[0].label, "calls");
        assert_eq!(d.relationships[0].tech, "HTTPS");
    }

    #[test]
    fn directional_rel_variants() {
        for kw in ["Rel_U", "Rel_D", "Rel_L", "Rel_R", "Rel_Up", "Rel_Down"] {
            let src = format!(
                "C4Context\nSystem(a,\"A\")\nSystem(b,\"B\")\n{kw}(a, b, \"x\")"
            );
            let d = parse(&src).unwrap();
            assert_eq!(d.relationships.len(), 1, "kw {kw}");
            assert_eq!(d.relationships[0].from, "a");
            assert_eq!(d.relationships[0].to, "b");
        }
    }

    #[test]
    fn birel_adds_reverse_edge() {
        let src = "C4Context\n\
            System(a, \"A\")\n\
            System(b, \"B\")\n\
            BiRel(a, b, \"talks\")";
        let d = parse(src).unwrap();
        assert_eq!(d.relationships.len(), 2);
        // One a→b and one b→a, both labeled "talks".
        let mut pairs: Vec<(String, String)> = d
            .relationships
            .iter()
            .map(|r| (r.from.clone(), r.to.clone()))
            .collect();
        pairs.sort();
        assert_eq!(
            pairs,
            vec![
                ("a".to_string(), "b".to_string()),
                ("b".to_string(), "a".to_string())
            ]
        );
        assert!(d.relationships.iter().all(|r| r.label == "talks"));
    }

    #[test]
    fn boundary_inner_elements_parsed_and_grouped() {
        let src = "C4Context\n\
            System_Boundary(b1, \"Boundary\") {\n\
            Person(u, \"User\")\n\
            System(s, \"Sys\")\n\
            }\n\
            Rel(u, s, \"uses\")";
        let d = parse(src).unwrap();
        // Inner elements present and still parsed.
        assert_eq!(d.elements.len(), 2);
        assert_eq!(d.elements[0].id, "u");
        assert_eq!(d.elements[1].id, "s");
        assert_eq!(d.relationships.len(), 1);
        // The boundary is captured with its two members.
        assert_eq!(d.boundaries.len(), 1);
        let b = &d.boundaries[0];
        assert_eq!(b.id, "b1");
        assert_eq!(b.label, "Boundary");
        assert_eq!(b.kind, BoundaryKind::System);
        assert_eq!(b.parent, None);
        assert_eq!(b.member_elems, vec!["u".to_string(), "s".to_string()]);
    }

    #[test]
    fn boundary_members_from_spec_example() {
        let src = "C4Container\n\
            System_Boundary(b1, \"My System\") {\n\
            Container(c1,\"Web\",\"\",\"\")\n\
            Container(c2,\"DB\",\"\",\"\")\n\
            }";
        let d = parse(src).unwrap();
        assert_eq!(d.boundaries.len(), 1);
        assert_eq!(d.boundaries[0].id, "b1");
        assert_eq!(d.boundaries[0].label, "My System");
        assert_eq!(
            d.boundaries[0].member_elems,
            vec!["c1".to_string(), "c2".to_string()]
        );
        assert_eq!(d.elements.len(), 2);
    }

    #[test]
    fn nested_boundaries_parsed() {
        let src = "C4Container\n\
            Enterprise_Boundary(e1, \"Ent\") {\n\
            System_Boundary(s1, \"Sys\") {\n\
            Container(c1, \"Web\")\n\
            }\n\
            Container(c2, \"Edge\")\n\
            }";
        let d = parse(src).unwrap();
        assert_eq!(d.boundaries.len(), 2);
        let e = &d.boundaries[0];
        let s = &d.boundaries[1];
        assert_eq!(e.kind, BoundaryKind::Enterprise);
        assert_eq!(e.parent, None);
        assert_eq!(e.member_elems, vec!["c2".to_string()]);
        assert_eq!(s.kind, BoundaryKind::System);
        assert_eq!(s.parent, Some(0));
        assert_eq!(s.member_elems, vec!["c1".to_string()]);
    }

    #[test]
    fn boundary_rect_encloses_member_centers() {
        let src = "C4Container\n\
            System_Boundary(b1, \"My System\") {\n\
            Container(c1, \"Web\")\n\
            Container(c2, \"DB\")\n\
            }\n\
            Container(c3, \"Outside\")\n\
            Rel(c1, c2, \"x\")";
        let r = render_c4(src, &opts()).unwrap();
        // A dashed boundary rect must be present.
        assert!(
            r.svg.contains("stroke-dasharray=\"7,7\""),
            "dashed boundary rect missing: {}",
            r.svg
        );
        // And the boundary label + type.
        assert!(r.svg.contains(">My System<"));
        assert!(r.svg.contains("«System»"));
    }

    #[test]
    fn nested_boundary_render_has_two_dashed_rects() {
        let src = "C4Container\n\
            Enterprise_Boundary(e1, \"Ent\") {\n\
            System_Boundary(s1, \"Sys\") {\n\
            Container(c1, \"Web\")\n\
            }\n\
            }";
        let r = render_c4(src, &opts()).unwrap();
        assert_eq!(
            r.svg.matches("stroke-dasharray=\"7,7\"").count(),
            2,
            "expected two dashed boundary rects: {}",
            r.svg
        );
        assert!(r.svg.contains("«Enterprise»"));
        assert!(r.svg.contains("«System»"));
    }

    #[test]
    fn boundary_drawn_before_element_boxes() {
        let src = "C4Container\n\
            System_Boundary(b1, \"My System\") {\n\
            Container(c1, \"Web\")\n\
            }";
        let r = render_c4(src, &opts()).unwrap();
        let dash = r.svg.find("stroke-dasharray=\"7,7\"").unwrap();
        // The element box is a rounded rect with rx="3"; the boundary uses
        // rx="2.5". The dashed boundary rect must precede the first element box.
        let elem_box = r.svg.find("rx=\"3\" ry=\"3\"").unwrap();
        assert!(dash < elem_box, "boundary not drawn before element box");
    }

    #[test]
    fn no_boundary_diagram_unchanged() {
        // A diagram with no boundaries must contain no dashed boundary rects and
        // render identically through the no-boundary path.
        let src = "C4Context\n\
            Person(u, \"User\")\n\
            System(s, \"Sys\")\n\
            Rel(u, s, \"uses\")";
        let r = render_c4(src, &opts()).unwrap();
        let d = parse(src).unwrap();
        assert!(d.boundaries.is_empty());
        assert!(!r.svg.contains("stroke-dasharray"));
    }

    #[test]
    fn all_c4_headers_accepted() {
        for h in [
            "C4Context",
            "C4Container",
            "C4Component",
            "C4Dynamic",
            "C4Deployment",
        ] {
            let src = format!("{h}\nSystem(s, \"S\")");
            let d = parse(&src).unwrap();
            assert_eq!(d.elements.len(), 1, "header {h}");
        }
    }

    #[test]
    fn bad_header_errors() {
        assert!(parse("graph TD\nA --> B").is_err());
    }

    #[test]
    fn no_header_errors() {
        assert!(parse("\n\n").is_err());
    }

    #[test]
    fn label_falls_back_to_id() {
        let src = "C4Context\nSystem(s)";
        let d = parse(src).unwrap();
        assert_eq!(d.elements[0].label, "s");
    }

    #[test]
    fn type_label_formatting() {
        assert_eq!(ElemKind::Person.type_label(false, ""), "[Person]");
        assert_eq!(
            ElemKind::Person.type_label(true, ""),
            "[External Person]"
        );
        assert_eq!(
            ElemKind::Container.type_label(false, "Rust"),
            "[Container: Rust]"
        );
        assert_eq!(ElemKind::System.type_label(false, ""), "[Software System]");
        assert_eq!(
            ElemKind::System.type_label(true, ""),
            "[External System]"
        );
    }

    #[test]
    fn render_wellformed_svg() {
        let src = "C4Context\n\
            Person(u, \"User\", \"a user\")\n\
            System(s, \"Sys\", \"the system\")\n\
            Rel(u, s, \"uses\")";
        let r = render_c4(src, &opts()).unwrap();
        assert!(r.svg.starts_with("<svg"));
        assert!(r.svg.trim_end().ends_with("</svg>"));
        assert!(r.svg.contains("viewBox="));
        assert!(r.svg.contains("xmlns="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn render_box_per_element_with_label_and_type() {
        let src = "C4Container\n\
            Person(u, \"User\")\n\
            Container(c, \"App\", \"Rust\", \"the app\")";
        let r = render_c4(src, &opts()).unwrap();
        // Two boxes.
        assert!(r.svg.matches("<rect").count() >= 2);
        // Labels + type lines present.
        assert!(r.svg.contains(">User<"));
        assert!(r.svg.contains(">App<"));
        assert!(r.svg.contains("[Person]"));
        assert!(r.svg.contains("[Container: Rust]"));
    }

    #[test]
    fn render_relationship_polyline_arrow_and_label() {
        let src = "C4Context\n\
            System(a, \"A\")\n\
            System(b, \"B\")\n\
            Rel(a, b, \"calls\")";
        let r = render_c4(src, &opts()).unwrap();
        // One edge polyline (fill="none" path) + one arrowhead path.
        assert_eq!(r.svg.matches("<path d=").count(), 2);
        assert!(r.svg.contains(">calls<"));
    }

    #[test]
    fn render_rel_label_includes_tech() {
        let src = "C4Context\n\
            System(a, \"A\")\n\
            System(b, \"B\")\n\
            Rel(a, b, \"calls\", \"HTTPS\")";
        let r = render_c4(src, &opts()).unwrap();
        assert!(r.svg.contains("calls [HTTPS]"));
    }

    #[test]
    fn person_vs_external_distinct() {
        let src = "C4Context\n\
            Person(p, \"P\")\n\
            System_Ext(e, \"E\")\n\
            System(s, \"S\")";
        let r = render_c4(src, &opts()).unwrap();
        // Person uses the person fill, external uses the grey fill, normal
        // system uses the default node fill — all three distinct.
        let person = rgb(PERSON_FILL);
        let external = rgb(EXTERNAL_FILL);
        let normal = rgb(opts().node_fill);
        assert!(r.svg.contains(&format!("fill=\"{person}\"")));
        assert!(r.svg.contains(&format!("fill=\"{external}\"")));
        assert!(r.svg.contains(&format!("fill=\"{normal}\"")));
        // Person has a head circle.
        assert!(r.svg.contains("<circle"));
    }

    #[test]
    fn xml_escapes_text() {
        let src = "C4Context\n\
            System(s, \"A & B < C\", \"x > y\")";
        let r = render_c4(src, &opts()).unwrap();
        assert!(r.svg.contains("A &amp; B &lt; C"));
        assert!(r.svg.contains("x &gt; y"));
        assert!(!r.svg.contains("A & B"));
    }

    #[test]
    fn element_name_renders_inline_math() {
        // An element name containing `$…$` renders the embedded math group
        // rather than a plain bold `<text>` with the raw dollars.
        let src = "C4Context\nSystem(s, \"Energy $x^2$\")";
        let r = render_c4(src, &opts()).unwrap();
        assert!(r.svg.contains("<g transform"), "expected math group: {}", r.svg);
        assert!(r.svg.contains("<path"), "expected math path: {}", r.svg);
    }

    #[test]
    fn relationship_label_renders_bold_markdown() {
        // A `**bold**` relationship label renders a bold run, not literal `**`.
        let src = "C4Context\n\
            System(a, \"A\")\n\
            System(b, \"B\")\n\
            Rel(a, b, \"**calls**\")";
        let r = render_c4(src, &opts()).unwrap();
        assert!(r.svg.contains("font-weight=\"bold\""), "expected bold run: {}", r.svg);
        assert!(!r.svg.contains("**calls**"), "raw markdown leaked: {}", r.svg);
    }

    #[test]
    fn plain_element_name_keeps_bold_text() {
        // A plain (non-rich) name must still render as a bold <text> — unchanged.
        let src = "C4Context\nSystem(s, \"Plain\")";
        let r = render_c4(src, &opts()).unwrap();
        assert!(r.svg.contains(">Plain<"));
        assert!(r.svg.contains("font-weight=\"bold\""), "plain name lost bold: {}", r.svg);
    }

    #[test]
    fn empty_diagram_errors() {
        assert_eq!(
            render_c4("C4Context\n", &opts()),
            Err(MermaidError::Empty)
        );
    }

    #[test]
    fn deterministic() {
        let src = "C4Context\n\
            Person(u, \"User\", \"a user\")\n\
            System(s, \"Sys\")\n\
            Rel(u, s, \"uses\")\n\
            Rel(s, u, \"replies\")";
        let x = render_c4(src, &opts()).unwrap();
        let y = render_c4(src, &opts()).unwrap();
        assert_eq!(x, y);
    }

    #[test]
    fn bidirectional_labels_separated() {
        // Two relationships between the same pair → distinct label anchors.
        let src = "C4Context\n\
            System(a, \"A\")\n\
            System(b, \"B\")\n\
            Rel(a, b, \"up\")\n\
            Rel(b, a, \"down\")";
        let r = render_c4(src, &opts()).unwrap();
        assert!(r.svg.contains(">up<"));
        assert!(r.svg.contains(">down<"));
        // The two label backgrounds must sit at distinct anchors. Dagre now
        // reserves the labels in the rank gap and orders them apart (here in x),
        // so check separation in either axis rather than y specifically.
        let pts = label_rect_centers(&r.svg);
        assert_eq!(pts.len(), 2, "two label backgrounds");
        let (dx, dy) = ((pts[0].0 - pts[1].0).abs(), (pts[0].1 - pts[1].1).abs());
        assert!(dx > 1.0 || dy > 1.0, "labels separated: {pts:?}");
    }

    /// Parse the `(x+w/2, y+h/2)` centers of each white label-background rect.
    fn label_rect_centers(svg: &str) -> Vec<(f32, f32)> {
        svg.match_indices("fill=\"rgb(255,255,255)\"")
            .filter_map(|(i, _)| {
                let rect_start = svg[..i].rfind("<rect")?;
                let seg = &svg[rect_start..i];
                let attr = |name: &str| -> Option<f32> {
                    let s = seg.find(name)? + name.len();
                    let rest = &seg[s..];
                    let end = rest.find('"')?;
                    rest[..end].parse::<f32>().ok()
                };
                Some((
                    attr(" x=\"")? + attr(" width=\"")? / 2.0,
                    attr(" y=\"")? + attr(" height=\"")? / 2.0,
                ))
            })
            .collect()
    }
}

