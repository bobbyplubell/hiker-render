//! `classDiagram` — UML class diagrams.
//!
//! Self-contained `parse → size → layout → draw` for the mermaid class diagram
//! subset. Classes become multi-compartment boxes (name / attributes / methods)
//! laid out with the [`hiker_graph`] layered (dagre) engine; relationships become
//! routed polylines with the appropriate UML end marker (inheritance triangle,
//! association/dependency arrow, aggregation/composition diamond), dashed for
//! `..` (dependency / realization) lines.
//!
//! Supported extras: generics `~T~` (e.g. `List~int~` renders `List<int>`,
//! but relationships match the base `List` id), annotations / stereotypes
//! `<<interface>>` (rendered «interface» in italics above the class name, both
//! the in-body and standalone forms), and `note`s (`note for Class "text"` and
//! floating `note "text"`).
//!
//! Skipped (noted, not parsed specially): namespaces and cardinality/
//! multiplicity labels.
//!
//! The pipeline is split across submodules so each stage evolves on its own:
//! [`model`] (parsed types), [`parse`] (text → model), [`layout`] (boxes +
//! routed relationships), [`render`] (model + layout → SVG).

pub mod model;

mod layout;
mod parse;
mod render;

use crate::{HitRegion, MermaidError, MermaidOptions, MermaidRender};

/// Render a mermaid `classDiagram` to SVG.
pub fn render_class(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    Ok(render::render(src, opts)?.0)
}

/// Like [`render_class`], but also returns one [`HitRegion`] per class box (its
/// drawn rect plus any `click`/`link`/`callback` data), in SVG-px coords. Used
/// by `render_with_regions` to make class diagrams interactive.
pub fn render_class_with_regions(
    src: &str,
    opts: &MermaidOptions,
) -> Result<(MermaidRender, Vec<HitRegion>), MermaidError> {
    render::render(src, opts)
}

#[cfg(test)]
mod tests {
    use super::model::{ClassDiagram, RelMarker};
    use super::parse::{parse, parse_member, split_generic};
    use super::{render_class, render_class_with_regions};
    use crate::model::ElemStyle;
    use crate::svgutil::rgb;
    use crate::{MermaidError, MermaidOptions};

    fn opts() -> MermaidOptions {
        MermaidOptions::default()
    }
    // ---- parse ----

    #[test]
    fn parse_block_class() {
        let src = "classDiagram\nclass Animal {\n+int age\n+String name\n+void eat()\n}\n";
        let d = parse(src).unwrap();
        assert_eq!(d.classes.len(), 1);
        let c = &d.classes[0];
        assert_eq!(c.name, "Animal");
        assert_eq!(c.attributes.len(), 2);
        assert_eq!(c.methods.len(), 1);
        assert_eq!(c.attributes[0].text, "+int age");
        assert_eq!(c.methods[0].text, "+void eat()");
    }

    #[test]
    fn parse_member_lines() {
        let src = "classDiagram\nAnimal : +int age\nAnimal : +void eat()\n";
        let d = parse(src).unwrap();
        assert_eq!(d.classes.len(), 1);
        let c = &d.classes[0];
        assert_eq!(c.name, "Animal");
        assert_eq!(c.attributes.len(), 1);
        assert_eq!(c.methods.len(), 1);
    }

    #[test]
    fn attribute_vs_method_classification() {
        assert!(!parse_member("+int age").is_method);
        assert!(parse_member("+void eat()").is_method);
        assert!(parse_member("-doStuff(int x)").is_method);
        assert!(!parse_member("#name").is_method);
    }

    #[test]
    fn visibility_sigils_preserved() {
        let src = "classDiagram\nFoo : +pub\nFoo : -priv\nFoo : #prot\nFoo : ~pkg\n";
        let d = parse(src).unwrap();
        let texts: Vec<&str> = d.classes[0].attributes.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(texts, vec!["+pub", "-priv", "#prot", "~pkg"]);
    }

    #[test]
    fn auto_create_classes() {
        let src = "classDiagram\nAnimal <|-- Dog\n";
        let d = parse(src).unwrap();
        assert_eq!(d.classes.len(), 2);
        assert!(d.classes.iter().any(|c| c.name == "Animal"));
        assert!(d.classes.iter().any(|c| c.name == "Dog"));
        assert_eq!(d.relations.len(), 1);
    }

    #[test]
    fn relationship_kinds_and_markers() {
        let cases = [
            ("classDiagram\nAnimal <|-- Dog\n", RelMarker::Triangle, false, false),
            ("classDiagram\nA --> B\n", RelMarker::Arrow, true, false),
            ("classDiagram\nA -- B\n", RelMarker::None, true, false),
            ("classDiagram\nA o-- B\n", RelMarker::DiamondHollow, false, false),
            ("classDiagram\nA *-- B\n", RelMarker::DiamondFilled, false, false),
            ("classDiagram\nA ..> B\n", RelMarker::Arrow, true, true),
            ("classDiagram\nA ..|> B\n", RelMarker::Triangle, true, true),
        ];
        for (src, marker, at_to, dashed) in cases {
            let d = parse(src).unwrap();
            assert_eq!(d.relations.len(), 1, "src={src}");
            let r = &d.relations[0];
            assert_eq!(r.marker, marker, "marker src={src}");
            assert_eq!(r.marker_at_to, at_to, "at_to src={src}");
            assert_eq!(r.dashed, dashed, "dashed src={src}");
        }
    }

    #[test]
    fn relationship_label() {
        let src = "classDiagram\nA --> B : uses\n";
        let d = parse(src).unwrap();
        assert_eq!(d.relations[0].label.as_deref(), Some("uses"));
    }

    #[test]
    fn dashed_detection() {
        let d = parse("classDiagram\nA ..> B\n").unwrap();
        assert!(d.relations[0].dashed);
        let d2 = parse("classDiagram\nA --> B\n").unwrap();
        assert!(!d2.relations[0].dashed);
    }

    #[test]
    fn bad_header_errors() {
        assert!(parse("flowchart TD\nA-->B").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn empty_diagram_renders_err_empty() {
        // Header but no classes.
        let r = render_class("classDiagram\n", &opts());
        assert_eq!(r, Err(MermaidError::Empty));
    }

    #[test]
    fn bad_header_render_err_parse() {
        let r = render_class("nonsense\nA-->B", &opts());
        assert!(matches!(r, Err(MermaidError::Parse(_))));
    }

    // ---- render ----

    fn sample() -> &'static str {
        "classDiagram\n\
         class Animal {\n+int age\n+String name\n+void eat()\n}\n\
         class Dog {\n+bark()\n}\n\
         Animal <|-- Dog\n"
    }

    #[test]
    fn renders_svg_envelope() {
        let r = render_class(sample(), &opts()).unwrap();
        assert!(r.svg.starts_with("<svg"));
        assert!(r.svg.trim_end().ends_with("</svg>"));
        assert!(r.svg.contains("viewBox="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn one_box_per_class_with_compartments() {
        let r = render_class(sample(), &opts()).unwrap();
        // 2 classes → 2 outer <rect> (label backgrounds are only for edge labels;
        // none here).
        assert_eq!(r.svg.matches("<rect").count(), 2);
        // Each class has >= 2 separator lines → >= 4 total.
        assert!(r.svg.matches("<line").count() >= 4);
    }

    #[test]
    fn member_text_present() {
        let r = render_class(sample(), &opts()).unwrap();
        assert!(r.svg.contains("+int age"));
        assert!(r.svg.contains("+void eat()"));
        assert!(r.svg.contains("+bark()"));
        assert!(r.svg.contains(">Animal<"));
        assert!(r.svg.contains(">Dog<"));
    }

    #[test]
    fn one_polyline_per_relationship() {
        let r = render_class(sample(), &opts()).unwrap();
        // The relationship line is a <path fill="none">.
        assert_eq!(r.svg.matches("fill=\"none\"").count() >= 1, true);
        // Inheritance → hollow triangle polygon present.
        assert!(r.svg.contains("<polygon"));
    }

    #[test]
    fn dashed_relationship_drawn_dashed() {
        let src = "classDiagram\nclass A\nclass B\nA ..|> B\n";
        let r = render_class(src, &opts()).unwrap();
        assert!(r.svg.contains("stroke-dasharray"));
    }

    #[test]
    fn markers_per_kind() {
        // arrow → polyline (open), diamond → polygon, triangle → polygon.
        let arrow = render_class("classDiagram\nA --> B\n", &opts()).unwrap();
        // edge path + arrow polyline both have fill="none". Arrow is a polyline.
        assert!(arrow.svg.contains("<polyline"));

        let comp = render_class("classDiagram\nA *-- B\n", &opts()).unwrap();
        assert!(comp.svg.contains("<polygon"));
        // filled diamond uses the edge-stroke color as fill (not white).
        assert!(comp.svg.contains(&rgb(opts().edge_stroke)));
    }

    #[test]
    fn relationship_label_rendered() {
        let r = render_class("classDiagram\nA --> B : uses\n", &opts()).unwrap();
        assert!(r.svg.contains(">uses<"));
    }

    #[test]
    fn bidirectional_relationship_labels_separated() {
        // A→B and B→A both labeled: labels must land at distinct anchors.
        let src = "classDiagram\nA --> B : up\nB --> A : down\n";
        let r = render_class(src, &opts()).unwrap();
        assert!(r.svg.contains(">up<"));
        assert!(r.svg.contains(">down<"));

        // Read the (x, y) anchor of each label's <text> element. The two must
        // differ in at least one coordinate (the route is vertical here so the
        // perpendicular nudge moves x).
        fn label_xy(svg: &str, text: &str) -> (f32, f32) {
            let needle = format!(">{text}<");
            let at = svg.find(&needle).expect("label text present");
            let tag_start = svg[..at].rfind("<text").expect("text tag");
            let tag = &svg[tag_start..at];
            let attr = |name: &str| {
                let k = tag.find(name).expect("attr") + name.len();
                let end = tag[k..].find('"').unwrap() + k;
                tag[k..end].parse::<f32>().unwrap()
            };
            (attr("x=\""), attr("y=\""))
        }
        let up = label_xy(&r.svg, "up");
        let down = label_xy(&r.svg, "down");
        assert!(
            (up.0 - down.0).abs() > 1.0 || (up.1 - down.1).abs() > 1.0,
            "bidirectional labels overlap: up={up:?}, down={down:?}"
        );
    }

    #[test]
    fn xml_escapes_member_text() {
        let src = "classDiagram\nFoo : +Map<K,V> data\n";
        let r = render_class(src, &opts()).unwrap();
        assert!(r.svg.contains("+Map&lt;K,V&gt; data"));
        assert!(!r.svg.contains("+Map<K,V>"));
    }

    #[test]
    fn empty_compartments_still_have_separators() {
        // A class with no members still gets 3 compartments / 2 separators.
        let r = render_class("classDiagram\nclass Lonely\n", &opts()).unwrap();
        assert_eq!(r.svg.matches("<line").count(), 2);
        assert_eq!(r.svg.matches("<rect").count(), 1);
    }

    #[test]
    fn deterministic() {
        let a = render_class(sample(), &opts()).unwrap();
        let b = render_class(sample(), &opts()).unwrap();
        assert_eq!(a, b);
    }

    // ---- styling directives ----

    fn style_of<'a>(d: &'a ClassDiagram, name: &str) -> &'a ElemStyle {
        &d.classes.iter().find(|c| c.name == name).expect("class").style
    }

    #[test]
    fn classdef_and_class_apply() {
        let src = "classDiagram\nclass Animal\nclass Dog\nclassDef hot fill:#f00\nclass Animal hot\n";
        let d = parse(src).unwrap();
        assert_eq!(style_of(&d, "Animal").fill, Some([255, 0, 0, 255]));
        // Dog untouched.
        assert_eq!(style_of(&d, "Dog").fill, None);
    }

    #[test]
    fn classdef_defined_after_class_resolves() {
        // Two-pass: the `class` assignment references `hot` before its classDef
        // appears later in the source.
        let src = "classDiagram\nclass Animal\nclass Animal hot\nclassDef hot fill:#0f0\n";
        let d = parse(src).unwrap();
        assert_eq!(style_of(&d, "Animal").fill, Some([0, 255, 0, 255]));
    }

    #[test]
    fn triple_colon_shorthand() {
        let src = "classDiagram\nAnimal:::hot <|-- Dog\nclassDef hot fill:#f00\n";
        let d = parse(src).unwrap();
        assert_eq!(style_of(&d, "Animal").fill, Some([255, 0, 0, 255]));
        // Relationship still recorded with the bare name.
        assert_eq!(d.relations.len(), 1);
        assert_eq!(d.relations[0].from, "Animal");
        assert_eq!(d.relations[0].to, "Dog");
    }

    #[test]
    fn triple_colon_bare_class() {
        let src = "classDiagram\nclass Animal\nAnimal:::hot\nclassDef hot fill:#00f\n";
        let d = parse(src).unwrap();
        assert_eq!(style_of(&d, "Animal").fill, Some([0, 0, 255, 255]));
    }

    #[test]
    fn cssclass_directive() {
        let src = "classDiagram\nclass Animal\nclassDef hot fill:#f00\ncssClass \"Animal\" hot\n";
        let d = parse(src).unwrap();
        assert_eq!(style_of(&d, "Animal").fill, Some([255, 0, 0, 255]));
    }

    #[test]
    fn style_directive_direct_and_overrides_class() {
        let src = "classDiagram\nclass Dog\nstyle Dog fill:#0f0\n";
        let d = parse(src).unwrap();
        assert_eq!(style_of(&d, "Dog").fill, Some([0, 255, 0, 255]));

        // Inline `style` wins over `class`.
        let src2 = "classDiagram\nclass Animal\nclassDef hot fill:#f00\nclass Animal hot\nstyle Animal fill:#00f\n";
        let d2 = parse(src2).unwrap();
        assert_eq!(style_of(&d2, "Animal").fill, Some([0, 0, 255, 255]));
    }

    #[test]
    fn style_override_in_rendered_svg() {
        // Animal's box rect should carry the override fill color.
        let src = "classDiagram\nclass Animal\nclass Dog\nclassDef hot fill:#f00\nclass Animal hot\n";
        let r = render_class(src, &opts()).unwrap();
        assert!(r.svg.contains(&rgb([255, 0, 0, 255])), "override fill present: {}", r.svg);
        // Default node fill still appears (Dog uses it).
        assert!(r.svg.contains(&rgb(opts().node_fill)));
    }

    #[test]
    fn unstyled_classes_unchanged() {
        // No directives → every class style is default.
        let d = parse(sample()).unwrap();
        for c in &d.classes {
            assert_eq!(c.style, ElemStyle::default());
        }
    }

    #[test]
    fn all_relationship_kinds_render() {
        let src = "classDiagram\n\
            A <|-- B\nA --> C\nA -- D\nA o-- E\nA *-- F\nA ..> G\nA ..|> H\n";
        let r = render_class(src, &opts()).unwrap();
        // 8 classes A..H.
        assert_eq!(r.svg.matches("<rect").count(), 8);
        assert!(r.svg.starts_with("<svg"));
    }

    // ---- generics ----

    #[test]
    fn split_generic_forms() {
        assert_eq!(split_generic("List~int~"), ("List".into(), "List<int>".into()));
        assert_eq!(split_generic("Map~K, V~"), ("Map".into(), "Map<K, V>".into()));
        assert_eq!(split_generic("Plain"), ("Plain".into(), "Plain".into()));
    }

    #[test]
    fn generic_class_id_vs_display() {
        let d = parse("classDiagram\nclass List~int~\n").unwrap();
        assert_eq!(d.classes.len(), 1);
        assert_eq!(d.classes[0].name, "List");
        assert_eq!(d.classes[0].display_name, "List<int>");
    }

    #[test]
    fn generic_class_renders_angle_brackets() {
        let r = render_class("classDiagram\nclass List~int~\n", &opts()).unwrap();
        // Display name rendered as List<int> (XML-escaped).
        assert!(r.svg.contains("List&lt;int&gt;"));
    }

    #[test]
    fn generic_relationship_links_base_class() {
        // `List~int~ --> Item` must link the `List` class id, not `List~int~`.
        let d = parse("classDiagram\nList~int~ --> Item\n").unwrap();
        assert_eq!(d.relations.len(), 1);
        assert_eq!(d.relations[0].from, "List");
        assert_eq!(d.relations[0].to, "Item");
        assert!(d.classes.iter().any(|c| c.name == "List"));
        assert!(d.classes.iter().any(|c| c.name == "Item"));
    }

    #[test]
    fn generic_definition_plus_relationship_shares_class() {
        // A `class List~int~` definition gives the display; the relationship
        // (matched on the base id `List`) links the same class.
        let src = "classDiagram\nclass List~int~\nList~int~ --> Item\n";
        let d = parse(src).unwrap();
        let list = d.classes.iter().find(|c| c.name == "List").unwrap();
        assert_eq!(list.display_name, "List<int>");
        assert_eq!(d.relations[0].from, "List");
        // Only one List class (definition + relationship endpoint merged).
        assert_eq!(d.classes.iter().filter(|c| c.name == "List").count(), 1);
    }

    #[test]
    fn generic_member_renders_angles() {
        let d = parse("classDiagram\nclass Box {\n+List~int~ items\n}\n").unwrap();
        assert_eq!(d.classes[0].attributes[0].text, "+List<int> items");
        let r = render_class("classDiagram\nclass Box {\n+List~int~ items\n}\n", &opts()).unwrap();
        assert!(r.svg.contains("+List&lt;int&gt; items"));
    }

    #[test]
    fn generic_with_space_is_definition_not_directive() {
        let d = parse("classDiagram\nclass Map~K, V~\n").unwrap();
        assert_eq!(d.classes.len(), 1);
        assert_eq!(d.classes[0].name, "Map");
        assert_eq!(d.classes[0].display_name, "Map<K, V>");
    }

    // ---- annotations / stereotypes ----

    #[test]
    fn annotation_in_body() {
        let src = "classDiagram\nclass Shape {\n<<interface>>\n+area() float\n}\n";
        let d = parse(src).unwrap();
        let shape = d.classes.iter().find(|c| c.name == "Shape").unwrap();
        assert_eq!(shape.annotation.as_deref(), Some("interface"));
        // The annotation line is NOT a member.
        assert_eq!(shape.methods.len(), 1);
        assert_eq!(shape.attributes.len(), 0);
    }

    #[test]
    fn annotation_standalone() {
        let d = parse("classDiagram\n<<interface>> Shape\n").unwrap();
        let shape = d.classes.iter().find(|c| c.name == "Shape").unwrap();
        assert_eq!(shape.annotation.as_deref(), Some("interface"));
    }

    #[test]
    fn annotation_renders_guillemets_above_name() {
        let r = render_class("classDiagram\n<<interface>> Shape\n", &opts()).unwrap();
        assert!(r.svg.contains("«interface»"));
        assert!(r.svg.contains("font-style=\"italic\""));
        assert!(r.svg.contains(">Shape<"));
    }

    #[test]
    fn annotation_in_body_renders() {
        let src = "classDiagram\nclass Shape {\n<<interface>>\n+area() float\n}\n";
        let r = render_class(src, &opts()).unwrap();
        assert!(r.svg.contains("«interface»"));
        assert!(r.svg.contains("+area() float"));
    }

    // ---- notes ----

    #[test]
    fn note_for_class_parsed() {
        let d = parse("classDiagram\nclass Shape\nnote for Shape \"important\"\n").unwrap();
        assert_eq!(d.notes.len(), 1);
        assert_eq!(d.notes[0].text, "important");
        assert_eq!(d.notes[0].for_class.as_deref(), Some("Shape"));
    }

    #[test]
    fn note_for_class_renders() {
        let src = "classDiagram\nclass Shape\nnote for Shape \"important\"\n";
        let r = render_class(src, &opts()).unwrap();
        assert!(r.svg.contains(">important<"));
        // Note body is drawn as a path (folded-corner rectangle).
        assert!(r.svg.contains("#fff5ad"));
    }

    #[test]
    fn floating_note_parsed_and_renders() {
        let src = "classDiagram\nclass A\nnote \"floating text\"\n";
        let d = parse(src).unwrap();
        assert_eq!(d.notes.len(), 1);
        assert!(d.notes[0].for_class.is_none());
        let r = render_class(src, &opts()).unwrap();
        assert!(r.svg.contains(">floating text<"));
    }

    #[test]
    fn note_xml_escaped() {
        let src = "classDiagram\nclass A\nnote for A \"a < b & c\"\n";
        let r = render_class(src, &opts()).unwrap();
        assert!(r.svg.contains("a &lt; b &amp; c"));
    }

    #[test]
    fn simple_diagrams_unchanged_no_extras() {
        // A diagram with no generics/annotations/notes: display_name == name,
        // no annotation, no notes.
        let d = parse(sample()).unwrap();
        assert!(d.notes.is_empty());
        for c in &d.classes {
            assert_eq!(c.display_name, c.name);
            assert!(c.annotation.is_none());
        }
    }

    // ---- click / interaction ----

    #[test]
    fn click_sets_link_and_tooltip() {
        let src = "classDiagram\nclass Animal\nclick Animal \"https://x\" \"tip\"\n";
        let d = parse(src).unwrap();
        let a = d.classes.iter().find(|c| c.name == "Animal").unwrap();
        assert_eq!(a.link.as_deref(), Some("https://x"));
        assert_eq!(a.tooltip.as_deref(), Some("tip"));
        assert!(a.callback.is_none());
    }

    #[test]
    fn click_call_and_unknown_id() {
        // `call name(args)` → callback; unknown id is skipped, not fabricated.
        let src = "classDiagram\nclass Animal\nclick Animal call doThing() \"hi\"\nclick Ghost \"https://y\"\n";
        let d = parse(src).unwrap();
        assert_eq!(d.classes.len(), 1);
        let a = &d.classes[0];
        assert_eq!(a.callback.as_deref(), Some("doThing"));
        assert_eq!(a.tooltip.as_deref(), Some("hi"));
    }

    #[test]
    fn regions_carry_click_data() {
        let src = "classDiagram\nclass Animal\nclass Dog\nAnimal <|-- Dog\nclick Animal \"https://x\" \"tip\"\n";
        let (render, regions) = render_class_with_regions(src, &opts()).unwrap();
        assert_eq!(regions.len(), 2);
        let a = regions.iter().find(|r| r.id == "Animal").unwrap();
        assert_eq!(a.link.as_deref(), Some("https://x"));
        assert_eq!(a.tooltip.as_deref(), Some("tip"));
        assert!(a.w > 0.0 && a.h > 0.0);
        assert!(a.x >= 0.0 && a.y >= 0.0);
        assert!(a.x + a.w <= render.width_px + 1.0);
        assert!(a.y + a.h <= render.height_px + 1.0);
        // Dog has no click.
        let dog = regions.iter().find(|r| r.id == "Dog").unwrap();
        assert!(dog.link.is_none() && dog.callback.is_none() && dog.tooltip.is_none());
    }

    #[test]
    fn regions_without_click_and_svg_unchanged() {
        let src = "classDiagram\nclass Animal\nclass Dog\nAnimal <|-- Dog\n";
        let plain = render_class(src, &opts()).unwrap();
        let (with_regions, regions) = render_class_with_regions(src, &opts()).unwrap();
        // Regions per node, all click-free.
        assert_eq!(regions.len(), 2);
        assert!(regions.iter().all(|r| r.link.is_none()
            && r.callback.is_none()
            && r.tooltip.is_none()
            && r.w > 0.0
            && r.h > 0.0));
        // `render` output byte-identical.
        assert_eq!(plain.svg, with_regions.svg);
        assert_eq!(plain.width_px, with_regions.width_px);
        assert_eq!(plain.height_px, with_regions.height_px);
    }
}
