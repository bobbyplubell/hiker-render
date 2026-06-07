//! Golden-snapshot corpus for the laid-out box tree.
//!
//! This is a SAFETY NET captured before refactoring `css/stylo/mod.rs` and the
//! `layout/*` files. It does NOT snapshot egui paint primitives directly (those
//! lack a stable, human-diffable Debug surface). Instead it snapshots the
//! DETERMINISTIC, meaningful output: the laid-out box tree — every generated
//! box's tag, kind, formatting context, border-box rect, content-box rect, the
//! box-model edges (margin/padding/border), and a handful of key computed style
//! fields — walked in document order and formatted as stable text. Text content
//! is captured via the inline fragments each IFC produces (galley text + line
//! position), which is the layout-level record of text wrapping / line breaking.
//!
//! Determinism: every float is rounded to 0.01 px before printing (kills
//! cross-platform float jitter); the tree is walked in deterministic document
//! order (the box arena's `children` vectors, root-first); no HashMap iteration
//! is printed. Rendering the same input twice yields byte-identical output (see
//! the `determinism` test below).
//!
//! Goldens live in `tests/golden/<name>.txt`. To (re)capture them, run:
//!
//!   UPDATE_GOLDENS=1 cargo test -p hiker-htmlview golden
//!
//! then re-run without the env var to confirm green.

use std::fmt::Write as _;
use std::path::PathBuf;

use hiker_htmlview::css::computed::ComputedStyle;
use hiker_htmlview::css::values::{Display, FontStyle};
use hiker_htmlview::dom::{parse_html, Document, NodeData, NodeId};
use hiker_htmlview::layout::construct::style_for;
use hiker_htmlview::layout::fonts::FontCtx;
use hiker_htmlview::layout::{
    layout_document, BoxKind, FormattingContext, InlineFragment, LayoutBox, LayoutTree,
};
use hiker_htmlview::{ResourceProvider, Theme};

// --- resource provider ------------------------------------------------------

/// Resolves nothing — the corpus is self-contained (inline/embedded CSS, no
/// external images or stylesheets). Inline `<img>` cases assert layout of an
/// element that fails to load, which is itself part of the behavior under test.
struct NullProvider;

impl ResourceProvider for NullProvider {
    fn fetch(&self, _url: &str) -> Option<(Vec<u8>, String)> {
        None
    }
}

// --- headless egui context for text measurement -----------------------------

fn headless_ctx() -> egui::Context {
    let ctx = egui::Context::default();
    ctx.set_fonts(egui::FontDefinitions::default());
    let _ = ctx.run(egui::RawInput::default(), |_| {});
    ctx
}

const CONTENT_WIDTH: f32 = 800.0;
const VIEWPORT_WIDTH: f32 = 1000.0;

/// Parse + style + lay out `html` at the standard corpus width, returning the
/// document and its layout tree. Determinism note: a fresh context/font ctx is
/// built per call, so there is no cross-case state.
fn lay_out(html: &str) -> (Document, LayoutTree) {
    let mut doc = parse_html(html);
    let provider = NullProvider;
    let ctx = headless_ctx();
    hiker_htmlview::css::stylo::style_document_stylo(
        &mut doc,
        &provider,
        None,
        Theme::Light,
        VIEWPORT_WIDTH,
        Some(&ctx),
    );
    let mut fonts = FontCtx::new(ctx, 1.0);
    let (tree, _content) = layout_document(&doc, &mut fonts, CONTENT_WIDTH, 1.0);
    (doc, tree)
}

// --- stable formatting ------------------------------------------------------

/// Round to 0.01 px and normalize -0.0 -> 0.0 so the text dump is identical
/// across platforms regardless of float-formatting quirks.
fn r(v: f32) -> f32 {
    let x = (v * 100.0).round() / 100.0;
    if x == 0.0 {
        0.0
    } else {
        x
    }
}

fn fmt_rect(rect: hiker_htmlview::geom::Rect) -> String {
    format!(
        "x={} y={} w={} h={}",
        r(rect.left()),
        r(rect.top()),
        r(rect.width()),
        r(rect.height())
    )
}

fn fmt_edges(e: hiker_htmlview::geom::Edges<f32>) -> String {
    format!(
        "[{} {} {} {}]",
        r(e.top),
        r(e.right),
        r(e.bottom),
        r(e.left)
    )
}

fn kind_str(k: BoxKind) -> &'static str {
    match k {
        BoxKind::Block => "Block",
        BoxKind::Inline => "Inline",
        BoxKind::InlineBlock => "InlineBlock",
        BoxKind::Replaced => "Replaced",
        BoxKind::Table => "Table",
        BoxKind::TableRow => "TableRow",
        BoxKind::TableRowGroup => "TableRowGroup",
        BoxKind::TableCell => "TableCell",
        BoxKind::Anonymous => "Anonymous",
    }
}

fn fc_str(fc: FormattingContext) -> &'static str {
    match fc {
        FormattingContext::Block => "Block",
        FormattingContext::Inline => "Inline",
        FormattingContext::Table => "Table",
        FormattingContext::Replaced => "Replaced",
    }
}

/// A short, document-order label for the box's source DOM node.
fn node_label(doc: &Document, node: Option<NodeId>) -> String {
    match node {
        None => "(anon)".to_string(),
        Some(n) => {
            let node = doc.node(n);
            match &node.data {
                NodeData::Element { name, .. } => {
                    let mut s = name.clone();
                    if let Some(id) = node.attr("id") {
                        let _ = write!(s, "#{id}");
                    }
                    if let Some(class) = node.attr("class") {
                        // Classes are space-separated; print them dot-joined,
                        // already in source order (deterministic).
                        for c in class.split_whitespace() {
                            let _ = write!(s, ".{c}");
                        }
                    }
                    s
                }
                NodeData::Text(_) => "#text".to_string(),
                NodeData::Document => "#document".to_string(),
                NodeData::Comment(_) => "#comment".to_string(),
                NodeData::Doctype => "#doctype".to_string(),
            }
        }
    }
}

/// Key computed-style fields worth snapshotting. Only emitted for boxes backed
/// by a DOM element (anonymous boxes have no style). We deliberately print a
/// small, high-signal subset (display, color, font, alignment, whitespace,
/// background) rather than the whole struct, to keep goldens readable and to
/// avoid over-coupling to fields irrelevant to layout regressions.
fn style_line(doc: &Document, node: Option<NodeId>) -> Option<String> {
    let n = node?;
    if !doc.node(n).is_element() {
        return None;
    }
    let s: ComputedStyle = style_for(doc, n);
    let mut parts: Vec<String> = Vec::new();

    parts.push(format!("display={:?}", s.display));
    // Skip the rare-but-noisy fields unless they differ from the initial value.
    if s.display != Display::None {
        parts.push(format!(
            "color=#{:02x}{:02x}{:02x}{:02x}",
            s.color.r(),
            s.color.g(),
            s.color.b(),
            s.color.a()
        ));
        if let Some(bg) = s.background_color {
            parts.push(format!(
                "bg=#{:02x}{:02x}{:02x}{:02x}",
                bg.r(),
                bg.g(),
                bg.b(),
                bg.a()
            ));
        }
        parts.push(format!("font-size={}", r(s.font_size)));
        parts.push(format!("weight={}", s.font_weight.0));
        if s.font_style != FontStyle::Normal {
            parts.push(format!("style={:?}", s.font_style));
        }
        if let Some(lh) = s.line_height {
            parts.push(format!("line-height={}", r(lh)));
        }
        parts.push(format!("text-align={:?}", s.text_align));
        if s.text_decoration_underline {
            parts.push("underline".to_string());
        }
        parts.push(format!("white-space={:?}", s.white_space));
    }
    Some(parts.join(" "))
}

/// Inline-fragment summary for a box that establishes an IFC. Captures wrapping
/// / line-breaking: each text fragment is one shaped run placed on a line, so
/// the count and y-positions encode where lines broke.
fn fragments_text(b: &LayoutBox) -> Vec<String> {
    let mut out = Vec::new();
    for f in &b.inline_fragments {
        match f {
            InlineFragment::Text {
                galley,
                pos,
                color,
                underline,
                ..
            } => {
                let mut line = format!(
                    "text @({},{}) {:?}",
                    r(pos.x),
                    r(pos.y),
                    galley.text()
                );
                let _ = write!(
                    line,
                    " color=#{:02x}{:02x}{:02x}{:02x}",
                    color.r(),
                    color.g(),
                    color.b(),
                    color.a()
                );
                if *underline {
                    line.push_str(" underline");
                }
                out.push(line);
            }
            InlineFragment::Box { box_idx, .. } => {
                out.push(format!("inline-box -> #{box_idx}"));
            }
            InlineFragment::Rect { rect, color, .. } => {
                out.push(format!(
                    "rect {} color=#{:02x}{:02x}{:02x}{:02x}",
                    fmt_rect(*rect),
                    color.r(),
                    color.g(),
                    color.b(),
                    color.a()
                ));
            }
        }
    }
    out
}

/// Recursively dump one box subtree in document order.
fn dump_box(doc: &Document, tree: &LayoutTree, idx: usize, depth: usize, out: &mut String) {
    let b = &tree.boxes[idx];
    let indent = "  ".repeat(depth);

    let mut header = format!(
        "{indent}[{}] {} {}/{} rect:{{{}}}",
        idx,
        node_label(doc, b.node),
        kind_str(b.kind),
        fc_str(b.fc),
        fmt_rect(b.rect),
    );
    if b.is_br {
        header.push_str(" br");
    }
    out.push_str(&header);
    out.push('\n');

    // content-box + box-model edges, only when they carry signal.
    let mut geo = format!("{indent}  content:{{{}}}", fmt_rect(b.content_rect));
    let m = b.margin;
    let p = b.padding;
    let bd = b.border;
    let any = |e: hiker_htmlview::geom::Edges<f32>| {
        e.top != 0.0 || e.right != 0.0 || e.bottom != 0.0 || e.left != 0.0
    };
    if any(m) {
        let _ = write!(geo, " margin:{}", fmt_edges(m));
    }
    if any(p) {
        let _ = write!(geo, " padding:{}", fmt_edges(p));
    }
    if any(bd) {
        let _ = write!(geo, " border:{}", fmt_edges(bd));
    }
    out.push_str(&geo);
    out.push('\n');

    if let Some(style) = style_line(doc, b.node) {
        out.push_str(&format!("{indent}  style: {style}\n"));
    }

    for frag in fragments_text(b) {
        out.push_str(&format!("{indent}  {frag}\n"));
    }

    for &c in &b.children {
        dump_box(doc, tree, c, depth + 1, out);
    }
}

/// Full deterministic text dump of a layout tree.
fn dump_tree(doc: &Document, tree: &LayoutTree) -> String {
    let mut out = String::new();
    match tree.root {
        Some(root) => dump_box(doc, tree, root, 0, &mut out),
        None => out.push_str("(no root)\n"),
    }
    out
}

fn render_dump(html: &str) -> String {
    let (doc, tree) = lay_out(html);
    dump_tree(&doc, &tree)
}

// --- golden compare ---------------------------------------------------------

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden")).join(format!("{name}.txt"))
}

fn check_golden(name: &str, html: &str) {
    let actual = render_dump(html);
    let path = golden_path(name);

    if std::env::var_os("UPDATE_GOLDENS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).expect("create golden dir");
        std::fs::write(&path, &actual).unwrap_or_else(|e| panic!("write golden {name}: {e}"));
        return;
    }

    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "missing golden for {name} ({e}); run UPDATE_GOLDENS=1 cargo test -p hiker-htmlview golden"
        )
    });

    if actual != expected {
        // Produce a compact first-divergence report.
        let mut report = String::new();
        for (i, (a, e)) in actual.lines().zip(expected.lines()).enumerate() {
            if a != e {
                let _ = write!(
                    report,
                    "\nfirst diff at line {}:\n  expected: {e}\n  actual:   {a}",
                    i + 1
                );
                break;
            }
        }
        if report.is_empty() {
            let _ = write!(
                report,
                "\nlength differs (expected {} lines, actual {} lines)",
                expected.lines().count(),
                actual.lines().count()
            );
        }
        panic!(
            "golden mismatch for {name}.{report}\nrun UPDATE_GOLDENS=1 cargo test -p hiker-htmlview golden to refresh"
        );
    }
}

// --- the corpus -------------------------------------------------------------

/// Every case: (name, html). HTML is self-contained with inline/embedded CSS.
fn corpus() -> Vec<(&'static str, &'static str)> {
    vec![
        // ===== block flow + nesting =====
        ("block_single", r#"<div>hello</div>"#),
        (
            "block_siblings",
            r#"<div>one</div><div>two</div><div>three</div>"#,
        ),
        (
            "block_nested",
            r#"<div><div><div>deep</div></div></div>"#,
        ),
        (
            "block_mixed_nesting",
            r#"<div>a<div>b<div>c</div>d</div>e</div>"#,
        ),
        (
            "block_explicit_width",
            r#"<div style="width:300px">narrow</div>"#,
        ),
        (
            "block_explicit_height",
            r#"<div style="height:120px">tall</div>"#,
        ),
        (
            "block_width_auto_fills",
            r#"<div><div>child fills parent width</div></div>"#,
        ),
        (
            "block_percent_width",
            r#"<div style="width:400px"><div style="width:50%">half</div></div>"#,
        ),

        // ===== margins / padding / borders (box model) =====
        (
            "box_margin_all",
            r#"<div style="margin:20px">margined</div>"#,
        ),
        (
            "box_margin_sides",
            r#"<div style="margin:10px 20px 30px 40px">tblr</div>"#,
        ),
        (
            "box_padding_all",
            r#"<div style="padding:15px">padded</div>"#,
        ),
        (
            "box_border_all",
            r#"<div style="border:5px solid black">bordered</div>"#,
        ),
        (
            "box_border_sides",
            r#"<div style="border-top:2px solid red; border-left:8px solid blue">sides</div>"#,
        ),
        (
            "box_margin_padding_border",
            r#"<div style="margin:10px; padding:20px; border:3px solid black; width:200px">all three</div>"#,
        ),
        (
            "box_sizing_border_box",
            r#"<div style="box-sizing:border-box; width:200px; padding:20px; border:5px solid black">border-box</div>"#,
        ),
        (
            "box_margin_collapse_siblings",
            r#"<div style="margin-bottom:30px">a</div><div style="margin-top:20px">b</div>"#,
        ),
        (
            "box_auto_margin_center",
            r#"<div style="width:200px; margin:0 auto">centered</div>"#,
        ),

        // ===== inline flow + text wrapping / line breaking =====
        (
            "inline_short_text",
            r#"<p>short line</p>"#,
        ),
        (
            "inline_wraps",
            r#"<p style="width:200px">the quick brown fox jumps over the lazy dog again and again</p>"#,
        ),
        (
            "inline_spans",
            r#"<p>plain <span>spanned</span> plain again</p>"#,
        ),
        (
            "inline_nested_spans",
            r#"<p>a <span>b <span>c</span> d</span> e</p>"#,
        ),
        (
            "inline_br",
            r#"<p>line one<br>line two<br>line three</p>"#,
        ),
        (
            "inline_bold_italic",
            r#"<p>normal <b>bold</b> <i>italic</i> <b><i>both</i></b></p>"#,
        ),
        (
            "inline_anchor_underline",
            r#"<p>see <a href="x">this link</a> here</p>"#,
        ),
        (
            "inline_mixed_wrap",
            r#"<p style="width:160px">words <b>bold words</b> and <i>italic words</i> wrapping</p>"#,
        ),

        // ===== whitespace handling =====
        (
            "ws_normal_collapse",
            "<p>a    b\n\n  c     d</p>",
        ),
        (
            "ws_pre",
            "<pre>line 1\n   indented\nline 3</pre>",
        ),
        (
            "ws_pre_with_spaces",
            "<pre>col1    col2    col3</pre>",
        ),
        (
            "ws_nowrap",
            r#"<p style="white-space:nowrap; width:80px">this should not wrap at all</p>"#,
        ),

        // ===== headings / paragraphs =====
        (
            "heading_h1",
            r#"<h1>Heading One</h1>"#,
        ),
        (
            "heading_levels",
            r#"<h1>H1</h1><h2>H2</h2><h3>H3</h3><h4>H4</h4><h5>H5</h5><h6>H6</h6>"#,
        ),
        (
            "heading_with_paragraph",
            r#"<h2>Title</h2><p>Body paragraph follows the heading.</p>"#,
        ),
        (
            "paragraph_pair",
            r#"<p>First paragraph.</p><p>Second paragraph.</p>"#,
        ),

        // ===== font styling =====
        (
            "font_size_px",
            r#"<p style="font-size:24px">big text</p>"#,
        ),
        (
            "font_size_em",
            r#"<div style="font-size:20px"><span style="font-size:1.5em">scaled</span></div>"#,
        ),
        (
            "font_weight",
            r#"<p style="font-weight:700">bold weight</p><p style="font-weight:300">light weight</p>"#,
        ),
        (
            "font_color",
            r#"<p style="color:#ff0000">red</p><p style="color:rgb(0,128,0)">green</p>"#,
        ),
        (
            "font_family_serif",
            r#"<p style="font-family:serif">serif text</p><p style="font-family:monospace">mono text</p>"#,
        ),
        (
            "font_line_height",
            r#"<p style="line-height:40px; width:120px">tall lines wrap across rows here</p>"#,
        ),

        // ===== backgrounds =====
        (
            "bg_color_block",
            r#"<div style="background-color:#eeeeee; padding:10px">on grey</div>"#,
        ),
        (
            "bg_nested",
            r#"<div style="background:#cccccc; padding:20px"><div style="background:#888888">inner</div></div>"#,
        ),

        // ===== text alignment =====
        (
            "align_center",
            r#"<p style="text-align:center; width:300px">centered text</p>"#,
        ),
        (
            "align_right",
            r#"<p style="text-align:right; width:300px">right text</p>"#,
        ),
        (
            "align_justify",
            r#"<p style="text-align:justify; width:200px">justified text that should spread across the available width</p>"#,
        ),

        // ===== lists =====
        (
            "list_ul",
            r#"<ul><li>first</li><li>second</li><li>third</li></ul>"#,
        ),
        (
            "list_ol",
            r#"<ol><li>alpha</li><li>beta</li><li>gamma</li></ol>"#,
        ),
        (
            "list_nested",
            r#"<ul><li>a<ul><li>a1</li><li>a2</li></ul></li><li>b</li></ul>"#,
        ),
        (
            "list_ol_nested",
            r#"<ol><li>one<ol><li>one-a</li><li>one-b</li></ol></li><li>two</li></ol>"#,
        ),
        (
            "list_mixed",
            r#"<ul><li>bullet<ol><li>num1</li><li>num2</li></ol></li></ul>"#,
        ),

        // ===== tables =====
        (
            "table_basic",
            r#"<table><tr><td>a</td><td>b</td></tr><tr><td>c</td><td>d</td></tr></table>"#,
        ),
        (
            "table_with_header",
            r#"<table><thead><tr><th>H1</th><th>H2</th></tr></thead><tbody><tr><td>1</td><td>2</td></tr></tbody></table>"#,
        ),
        (
            "table_borders",
            r#"<table style="border-collapse:collapse"><tr><td style="border:1px solid black">x</td><td style="border:1px solid black">y</td></tr></table>"#,
        ),
        (
            "table_colspan",
            r#"<table border="1"><tr><td colspan="2">wide</td></tr><tr><td>a</td><td>b</td></tr></table>"#,
        ),
        (
            "table_rowspan",
            r#"<table border="1"><tr><td rowspan="2">tall</td><td>top</td></tr><tr><td>bottom</td></tr></table>"#,
        ),
        (
            "table_widths",
            r#"<table style="width:400px"><tr><td style="width:100px">narrow</td><td>wide remainder</td></tr></table>"#,
        ),
        (
            "table_cell_align",
            r#"<table border="1"><tr><td style="text-align:right">r</td><td style="text-align:center">c</td></tr></table>"#,
        ),
        (
            "table_padding",
            r#"<table><tr><td style="padding:10px">padded cell</td></tr></table>"#,
        ),
        (
            "table_caption",
            r#"<table><caption>The Caption</caption><tr><td>cell</td></tr></table>"#,
        ),
        (
            "table_multi_row_col",
            r#"<table border="1"><tr><td>r1c1</td><td>r1c2</td><td>r1c3</td></tr><tr><td>r2c1</td><td>r2c2</td><td>r2c3</td></tr></table>"#,
        ),

        // ===== inline images (sizing) =====
        (
            "img_explicit_size",
            r#"<p>before <img src="missing.png" width="64" height="48"> after</p>"#,
        ),
        (
            "img_block",
            r#"<div><img src="missing.png" width="200" height="100"></div>"#,
        ),
        (
            "img_no_dims",
            r#"<p>text <img src="missing.png" alt="alt text"> more</p>"#,
        ),

        // ===== nested mixed content =====
        (
            "mixed_article_section",
            r#"<div style="width:400px"><h2>Section</h2><p>A paragraph with <b>bold</b> and <a href="x">a link</a>.</p><ul><li>point one</li><li>point two</li></ul></div>"#,
        ),
        (
            "mixed_blockquote",
            r#"<blockquote style="margin-left:40px"><p>Quoted text here.</p></blockquote>"#,
        ),
        (
            "mixed_inline_block",
            r#"<div><span style="display:inline-block; width:80px; height:40px; background:#ddd">A</span><span style="display:inline-block; width:80px; height:40px; background:#bbb">B</span></div>"#,
        ),
        (
            "mixed_div_in_p_context",
            r#"<div style="width:300px"><p>Lead paragraph.</p><div style="background:#eee; padding:8px">A boxed note inside.</div><p>Trailing paragraph.</p></div>"#,
        ),

        // ===== width/height/auto edge cases =====
        (
            "size_auto_block",
            r#"<div>auto width auto height</div>"#,
        ),
        (
            "size_max_width",
            r#"<div style="max-width:150px">content limited by max-width even though available is wider</div>"#,
        ),
        (
            "size_min_height",
            r#"<div style="min-height:200px">tall by min-height</div>"#,
        ),

        // ===== realistic page fragments =====
        (
            "page_article",
            r#"<article style="width:600px">
                <h1>The Title of the Article</h1>
                <p class="lead">This is the lead paragraph that introduces the article with a bit of <b>emphasis</b> and a <a href="/wiki/Topic">wikilink</a>.</p>
                <h2>First Section</h2>
                <p>Some explanatory prose that runs long enough to wrap onto more than a single line at this content width, demonstrating inline flow.</p>
                <ul>
                    <li>A bulleted item.</li>
                    <li>Another item, slightly longer than the first.</li>
                </ul>
                <h2>Data</h2>
                <table border="1">
                    <thead><tr><th>Name</th><th>Value</th></tr></thead>
                    <tbody>
                        <tr><td>Alpha</td><td>1</td></tr>
                        <tr><td>Beta</td><td>2</td></tr>
                    </tbody>
                </table>
            </article>"#,
        ),
        (
            "page_card",
            r#"<div style="width:320px; border:1px solid #ccc; padding:16px; background:#fafafa">
                <h3 style="margin-top:0">Card Heading</h3>
                <p>Card body copy describing the thing in a couple of sentences that will wrap.</p>
                <p style="text-align:right"><a href="x">Read more</a></p>
            </div>"#,
        ),
        (
            "page_two_column_table",
            r#"<table style="width:500px">
                <tr>
                    <td style="width:30%; background:#eef; padding:8px"><h4>Sidebar</h4><ul><li>nav one</li><li>nav two</li></ul></td>
                    <td style="padding:8px"><h2>Main</h2><p>Main column content that flows to fill the remaining width of the table.</p></td>
                </tr>
            </table>"#,
        ),
    ]
}

// --- one #[test] per case via a macro so failures name the case -------------

macro_rules! golden_tests {
    ($($name:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                let cases = corpus();
                let stringy = stringify!($name);
                let (_, html) = cases
                    .iter()
                    .find(|(n, _)| *n == stringy)
                    .unwrap_or_else(|| panic!("case {stringy} not in corpus()"));
                check_golden(stringy, html);
            }
        )*
    };
}

golden_tests!(
    block_single,
    block_siblings,
    block_nested,
    block_mixed_nesting,
    block_explicit_width,
    block_explicit_height,
    block_width_auto_fills,
    block_percent_width,
    box_margin_all,
    box_margin_sides,
    box_padding_all,
    box_border_all,
    box_border_sides,
    box_margin_padding_border,
    box_sizing_border_box,
    box_margin_collapse_siblings,
    box_auto_margin_center,
    inline_short_text,
    inline_wraps,
    inline_spans,
    inline_nested_spans,
    inline_br,
    inline_bold_italic,
    inline_anchor_underline,
    inline_mixed_wrap,
    ws_normal_collapse,
    ws_pre,
    ws_pre_with_spaces,
    ws_nowrap,
    heading_h1,
    heading_levels,
    heading_with_paragraph,
    paragraph_pair,
    font_size_px,
    font_size_em,
    font_weight,
    font_color,
    font_family_serif,
    font_line_height,
    bg_color_block,
    bg_nested,
    align_center,
    align_right,
    align_justify,
    list_ul,
    list_ol,
    list_nested,
    list_ol_nested,
    list_mixed,
    table_basic,
    table_with_header,
    table_borders,
    table_colspan,
    table_rowspan,
    table_widths,
    table_cell_align,
    table_padding,
    table_caption,
    table_multi_row_col,
    img_explicit_size,
    img_block,
    img_no_dims,
    mixed_article_section,
    mixed_blockquote,
    mixed_inline_block,
    mixed_div_in_p_context,
    size_auto_block,
    size_max_width,
    size_min_height,
    page_article,
    page_card,
    page_two_column_table,
);

// --- determinism guard ------------------------------------------------------

/// Rendering the same input twice must produce byte-identical dumps. If this
/// fails, the dump has nondeterminism (float jitter / unstable iteration) that
/// must be stabilized before goldens are trustworthy.
#[test]
fn determinism() {
    for (name, html) in corpus() {
        let a = render_dump(html);
        let b = render_dump(html);
        assert_eq!(a, b, "nondeterministic dump for case {name}");
    }
}
