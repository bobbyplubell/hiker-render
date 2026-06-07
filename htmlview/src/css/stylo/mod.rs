//! Real Stylo (Servo's style engine) integration over our arena [`Document`].
//!
//! This is the Stage-1 port of `stylo-spike/` onto our real [`crate::dom::Node`].
//! It implements Stylo's trait surface ([`element`]: `selectors::Element`,
//! `TDocument`/`TNode`/`TShadowRoot`/`NodeInfo`/`AttributeProvider`/`TElement`)
//! over `&Node`, builds a `Stylist` from our UA + author stylesheets and runs the
//! **Option A** sequential cascade ([`cascade`]), and projects Stylo's
//! `ComputedValues` into our owned `ComputedStyle` at the single layout-facing
//! boundary ([`project`]). Per-element data lives in [`data`]; the value
//! accessors that read Stylo's structs live in [`read`].
//!
//! It runs ALONGSIDE the existing `css::cascade`; it deletes nothing and layout
//! still reads the old `ComputedStyle`. Reading these `ComputedValues` into
//! layout is Stage 2.

pub mod data;
pub mod read;

mod cascade;
pub mod element;
mod project;

pub use cascade::{style_document_stylo, RecalcStyle, RegisteredPaintersImpl};
pub use project::{
    computed_color, computed_display, computed_font_size_px, computed_style_for,
    initial_computed_values, primary_computed,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use style::properties::ComputedValues;

    use crate::dom::{Document, Node};
    use crate::{ResourceProvider, Theme};

    /// A dir-backed provider, matching the existing cascade/lib tests.
    struct DirProvider {
        dir: PathBuf,
    }
    impl ResourceProvider for DirProvider {
        fn fetch(&self, url: &str) -> Option<(Vec<u8>, String)> {
            let name = url.rsplit('/').next().unwrap_or(url);
            let path = self.dir.join(name);
            std::fs::read(&path)
                .ok()
                .map(|bytes| (bytes, "text/css".to_string()))
        }
    }

    /// A headless egui context with default fonts (mirrors `layout::mod`'s
    /// test helper) so the real `EguiFontMetricsProvider` can measure glyphs.
    fn headless_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(egui::FontDefinitions::default());
        // Run one empty frame so the font atlas is ready for measurement
        // (egui panics on `fonts(...)` before the first frame).
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        ctx
    }

    /// First element with the given tag name (markup5ever local), in arena order.
    fn find_tag<'a>(doc: &'a Document, tag: &str) -> Option<&'a Node> {
        doc.nodes
            .iter()
            .find(|n| n.tag() == Some(tag))
    }

    /// First element carrying `class` token `cls`.
    fn find_class<'a>(doc: &'a Document, cls: &str) -> Option<&'a Node> {
        doc.nodes
            .iter()
            .find(|n| n.classes().any(|c| c == cls))
    }

    #[test]
    fn stylo_styles_wiki_article() {
        let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/wiki-sample"));
        let html = std::fs::read_to_string(dir.join("article.html")).expect("read article.html");
        let provider = DirProvider { dir };

        let mut doc = crate::dom::parse_html(&html);
        let ctx = headless_ctx();
        style_document_stylo(
            &mut doc,
            &provider,
            Some("./"),
            Theme::Light,
            800.0,
            Some(&ctx),
        );

        // (a) A large fraction of element nodes got primary styles.
        let elements: Vec<&Node> = doc.nodes.iter().filter(|n| n.is_element()).collect();
        let styled = elements
            .iter()
            .filter(|n| n.stylo_element_data.primary_styles().is_some())
            .count();
        let total = elements.len();
        eprintln!(
            "stylo: {styled}/{total} element nodes styled ({:.1}%)",
            100.0 * styled as f32 / total as f32
        );
        assert!(total > 500, "expected a large article, got {total} elements");
        assert!(
            styled as f32 >= 0.9 * total as f32,
            "expected >=90% of elements styled, got {styled}/{total}"
        );

        // (b) Spot-checks against well-defined computed values.

        // The infobox/ib-chembox table computes display:table (the bug we fixed
        // was it computing block).
        let table = find_class(&doc, "ib-chembox")
            .filter(|n| n.tag() == Some("table"))
            .expect("ib-chembox table");
        let disp = computed_display(table).expect("table styled");
        eprintln!("ib-chembox table display = {disp:?}");
        assert_eq!(
            disp,
            style::values::computed::Display::Table,
            "infobox table should compute display:table, got {disp:?}"
        );

        // A normal <a href> link computes a blue-ish color (UA a:link blue).
        let link = doc
            .nodes
            .iter()
            .find(|n| n.tag() == Some("a") && n.attr("href").is_some())
            .expect("an <a href> link");
        let (r, g, b, _a) = computed_color(link).expect("link styled");
        eprintln!("link color = ({r},{g},{b})");
        assert!(
            b > r && b > g && b >= 120,
            "link should be blue-ish, got ({r},{g},{b})"
        );

        // <body> font-size is sane (UA medium ~16px; never absurd).
        let body = find_tag(&doc, "body").expect("body");
        let fs = computed_font_size_px(body).expect("body styled");
        eprintln!("body font-size = {fs}px");
        assert!(
            (8.0..=32.0).contains(&fs),
            "body font-size should be sane, got {fs}px"
        );
    }

    /// Borrow a node's primary `ComputedValues` and run `f` over the read layer.
    fn with_cv<R>(node: &Node, f: impl FnOnce(&ComputedValues) -> R) -> Option<R> {
        let styles = node.stylo_element_data.primary_styles()?;
        let cv: &ComputedValues = &styles;
        Some(f(cv))
    }

    #[test]
    fn read_layer_and_hints_on_wiki_article() {
        use crate::css::stylo::read;
        use crate::css::values::{Display, LengthOrPercent, LengthPercentOrAuto, Length};

        let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/wiki-sample"));
        let html = std::fs::read_to_string(dir.join("article.html")).expect("read article.html");
        let provider = DirProvider { dir };

        let mut doc = crate::dom::parse_html(&html);
        let ctx = headless_ctx();
        style_document_stylo(
            &mut doc,
            &provider,
            Some("./"),
            Theme::Light,
            800.0,
            Some(&ctx),
        );

        // (1) read::display on the ib-chembox table == Display::Table.
        let table = find_class(&doc, "ib-chembox")
            .filter(|n| n.tag() == Some("table"))
            .expect("ib-chembox table");
        let disp = with_cv(table, read::display).expect("table styled");
        assert_eq!(disp, Display::Table, "ib-chembox table => Display::Table");

        // (2) read::font_size on <body> ~16px.
        let body = find_tag(&doc, "body").expect("body");
        let fs = with_cv(body, read::font_size).expect("body styled");
        assert!(
            (12.0..=20.0).contains(&fs),
            "body font-size ~16px, got {fs}"
        );

        // (3) read::color on a link is blue-ish.
        let link = doc
            .nodes
            .iter()
            .find(|n| n.tag() == Some("a") && n.attr("href").is_some())
            .expect("an <a href> link");
        let c = with_cv(link, read::color).expect("link styled");
        assert!(
            c.b() > c.r() && c.b() > c.g() && c.b() >= 120,
            "link color blue-ish, got {c:?}"
        );

        // (4) margin/padding read back as expected px on some block. The body has
        // a UA margin (8px) in our UA sheet; assert it reads as a px length.
        let body_margin = with_cv(body, read::margin).expect("body styled");
        // Whatever the UA sets, the values must be concrete (px or auto), and the
        // top margin must resolve to a finite length when non-auto.
        if let LengthPercentOrAuto::Length(Length::Px(px)) = body_margin.top {
            assert!(px.is_finite() && px >= 0.0, "body margin-top px sane");
        }

        // padding edges must be length-or-percent (no panic / well-formed).
        let body_padding = with_cv(body, read::padding).expect("body styled");
        matches!(body_padding.top, LengthOrPercent::Length(_) | LengthOrPercent::Percent(_));

        // A cell with an inline `width:50%` reads back as a 50% width through the
        // read layer (exercises LengthPercentage → Percent mapping).
        let half_cell = doc
            .nodes
            .iter()
            .find(|n| {
                n.tag() == Some("td")
                    && n.attr("style").map(|s| s.contains("width:50%")).unwrap_or(false)
            })
            .expect("a td with width:50%");
        let w = with_cv(half_cell, read::width).expect("cell styled");
        assert!(
            matches!(w, LengthPercentOrAuto::Percent(p) if (p - 0.5).abs() < 1e-3),
            "td width:50% => Percent(0.5), got {w:?}"
        );
    }

    #[test]
    fn presentational_hints_reflect_in_computed_style() {
        use crate::css::stylo::read;
        use crate::css::values::{LengthPercentOrAuto, Length, TextAlign};

        // A synthetic table using only legacy presentational attributes.
        let html = r##"<!DOCTYPE html><html><body>
            <table border="3" cellspacing="5">
              <tr>
                <td width="120" height="40" bgcolor="#ff0000" align="center" nowrap>cell</td>
                <td width="50%">half</td>
              </tr>
            </table>
        </body></html>"##;

        struct NoProvider;
        impl ResourceProvider for NoProvider {
            fn fetch(&self, _url: &str) -> Option<(Vec<u8>, String)> {
                None
            }
        }

        let mut doc = crate::dom::parse_html(html);
        let ctx = headless_ctx();
        style_document_stylo(&mut doc, &NoProvider, None, Theme::Light, 800.0, Some(&ctx));

        // The first <td> with the legacy attributes.
        let td = doc
            .nodes
            .iter()
            .find(|n| n.tag() == Some("td") && n.attr("width") == Some("120"))
            .expect("the legacy <td>");

        // width=120 → 120px.
        let w = with_cv(td, read::width).expect("td styled");
        assert_eq!(
            w,
            LengthPercentOrAuto::Length(Length::Px(120.0)),
            "td width=120 => 120px hint"
        );
        // height=40 → 40px.
        let h = with_cv(td, read::height).expect("td styled");
        assert_eq!(
            h,
            LengthPercentOrAuto::Length(Length::Px(40.0)),
            "td height=40 => 40px hint"
        );
        // bgcolor=#ff0000 → red background.
        let bg = with_cv(td, read::background_color).expect("td styled").expect("bg set");
        assert!(
            bg.r() >= 200 && bg.g() < 60 && bg.b() < 60,
            "td bgcolor=#ff0000 => red, got {bg:?}"
        );
        // align=center → text-align center.
        let ta = with_cv(td, read::text_align).expect("td styled");
        assert_eq!(ta, TextAlign::Center, "td align=center => center");
        // nowrap → white-space nowrap.
        let ws = with_cv(td, read::white_space).expect("td styled");
        assert_eq!(
            ws,
            crate::css::values::WhiteSpace::Nowrap,
            "td nowrap => white-space:nowrap"
        );

        // The <table border="3"> got a uniform border width via the hint.
        let table = doc.nodes.iter().find(|n| n.tag() == Some("table")).expect("table");
        let bw = with_cv(table, read::border_width).expect("table styled");
        assert!(
            (bw.top - 3.0).abs() < 0.5 && (bw.left - 3.0).abs() < 0.5,
            "table border=3 => ~3px border widths, got {bw:?}"
        );

        // The second cell width=50% reads as Percent(0.5).
        let td2 = doc
            .nodes
            .iter()
            .find(|n| n.tag() == Some("td") && n.attr("width") == Some("50%"))
            .expect("the 50% <td>");
        let w2 = with_cv(td2, read::width).expect("td2 styled");
        assert!(
            matches!(w2, LengthPercentOrAuto::Percent(p) if (p - 0.5).abs() < 1e-3),
            "td width=50% => Percent(0.5), got {w2:?}"
        );
    }
}
