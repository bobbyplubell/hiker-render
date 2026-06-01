//! Hand-drawn ("rough") look: rewrite the SVG's geometric shapes into sketchy
//! multi-stroke paths. Pure-Rust, std-only, deterministic (no RNG).
//!
//! `roughen` scans OUR well-formed SVG output and replaces each self-closing
//! `<rect>`, `<circle>`, `<ellipse>`, `<line>`, `<polygon>`, `<polyline>` with a
//! `<g>` containing hand-drawn `<path>` equivalents that preserve the shape's
//! `fill`, `fill-opacity`, `stroke`, `stroke-width`, `stroke-dasharray`.
//!
//! Determinism: jitter comes from a tiny xorshift/FNV value source SEEDED by the
//! shape's own numeric coordinates (see `Wobble`), advanced per sample. The same
//! shape always wobbles the same way, so the whole output is reproducible.
//!
//! Notes:
//! * Rectangles with `rx` (rounded corners) are treated as plain rectangles —
//!   corner rounding is ignored.
//! * The injected full-bleed background `<rect x="0" y="0" …>` is left solid /
//!   untouched (detected by `x="0"` and `y="0"`), so the canvas stays clean.

/// Rewrite shape elements in `svg` into hand-drawn `<path>` equivalents,
/// in place. Deterministic. `<text>`, existing `<path>`/`<g>`/`<marker>`/
/// `<defs>`/`<svg>` and the background rect are left untouched.
pub fn roughen(svg: &mut String) {
    let shapes = ["rect", "circle", "ellipse", "line", "polygon", "polyline"];
    *svg = roughen_scan(svg, &shapes);
}

/// UTF-8-safe single-pass rewrite: find shape spans by byte index and splice
/// replacements, copying the gaps as proper `&str` slices. Everything that
/// isn't a recognized self-closing shape element is copied through verbatim.
fn roughen_scan(svg: &str, shapes: &[&str]) -> String {
    let bytes = svg.as_bytes();
    let mut out = String::with_capacity(svg.len() * 2);
    let mut last = 0usize; // end of last copied region
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            if let Some((tag, name_end)) = read_tag_name(bytes, i + 1) {
                if shapes.contains(&tag.as_str()) {
                    if let Some(close) = find_self_close(bytes, name_end) {
                        let attr_slice = &svg[name_end..close];
                        if let Some(rep) = roughen_element(&tag, attr_slice) {
                            out.push_str(&svg[last..i]); // gap before the shape
                            out.push_str(&rep);
                            last = close + 2;
                            i = close + 2;
                            continue;
                        }
                    }
                }
            }
        }
        i += 1;
    }
    out.push_str(&svg[last..]);
    out
}

/// Read an ASCII tag name starting at `start` (just after '<'). Returns the
/// lowercased name and the index just past it (where attributes begin), or
/// `None` if this isn't a plain element start (e.g. `</`, `<!`, `<?`).
fn read_tag_name(bytes: &[u8], start: usize) -> Option<(String, usize)> {
    if start >= bytes.len() {
        return None;
    }
    let c = bytes[start];
    if !c.is_ascii_alphabetic() {
        return None;
    }
    let mut j = start;
    while j < bytes.len() {
        let b = bytes[j];
        if b.is_ascii_alphanumeric() || b == b'-' {
            j += 1;
        } else {
            break;
        }
    }
    let name = std::str::from_utf8(&bytes[start..j]).ok()?.to_ascii_lowercase();
    Some((name, j))
}

/// From `start` (within an opening tag, after the tag name), find the byte
/// index of the "/>" that self-closes this element. Returns `None` if a plain
/// '>' (non-self-closing) is hit first or end of input is reached.
fn find_self_close(bytes: &[u8], start: usize) -> Option<usize> {
    let mut j = start;
    while j < bytes.len() {
        match bytes[j] {
            b'"' => {
                // Skip a quoted attribute value.
                j += 1;
                while j < bytes.len() && bytes[j] != b'"' {
                    j += 1;
                }
                if j >= bytes.len() {
                    return None;
                }
                j += 1;
            }
            b'/' if j + 1 < bytes.len() && bytes[j + 1] == b'>' => return Some(j),
            b'>' => return None, // not self-closing
            _ => j += 1,
        }
    }
    None
}

/// Parse `name="value"` pairs out of an attribute string. Robust to ordering
/// and whitespace; values are taken verbatim between the double quotes.
fn parse_attrs(s: &str) -> Vec<(String, String)> {
    let b = s.as_bytes();
    let mut attrs = Vec::new();
    let mut i = 0usize;
    while i < b.len() {
        // Find start of a name (alphabetic).
        if !b[i].is_ascii_alphabetic() {
            i += 1;
            continue;
        }
        let ns = i;
        while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'-' || b[i] == b':') {
            i += 1;
        }
        let name = &s[ns..i];
        // Skip whitespace, expect '='.
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() || b[i] != b'=' {
            continue;
        }
        i += 1;
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() || b[i] != b'"' {
            continue;
        }
        i += 1;
        let vs = i;
        while i < b.len() && b[i] != b'"' {
            i += 1;
        }
        let val = &s[vs..i.min(b.len())];
        if i < b.len() {
            i += 1; // past closing quote
        }
        attrs.push((name.to_string(), val.to_string()));
    }
    attrs
}

fn get<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn getf(attrs: &[(String, String)], key: &str) -> Option<f64> {
    get(attrs, key).and_then(|v| v.trim().parse::<f64>().ok())
}

/// Common stroke/fill style pulled from a shape's attributes.
struct Style {
    fill: Option<String>,
    fill_opacity: Option<String>,
    stroke: Option<String>,
    stroke_width: Option<String>,
    dash: Option<String>,
}

impl Style {
    fn from(attrs: &[(String, String)]) -> Style {
        let none_filter = |v: &str| {
            let t = v.trim();
            if t.eq_ignore_ascii_case("none") || t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        };
        Style {
            fill: get(attrs, "fill").and_then(none_filter),
            fill_opacity: get(attrs, "fill-opacity").map(|s| s.to_string()),
            stroke: get(attrs, "stroke").and_then(none_filter),
            stroke_width: get(attrs, "stroke-width").map(|s| s.to_string()),
            dash: get(attrs, "stroke-dasharray").and_then(|v| {
                let t = v.trim();
                if t.eq_ignore_ascii_case("none") || t.is_empty() {
                    None
                } else {
                    Some(t.to_string())
                }
            }),
        }
    }

    fn fill_attr(&self) -> String {
        let mut s = String::new();
        if let Some(f) = &self.fill {
            s.push_str(&format!(" fill=\"{}\"", f));
        }
        if let Some(o) = &self.fill_opacity {
            s.push_str(&format!(" fill-opacity=\"{}\"", o));
        }
        s
    }

    fn stroke_attr(&self) -> String {
        let mut s = String::new();
        if let Some(st) = &self.stroke {
            s.push_str(&format!(" stroke=\"{}\"", st));
        }
        if let Some(w) = &self.stroke_width {
            s.push_str(&format!(" stroke-width=\"{}\"", w));
        }
        if let Some(d) = &self.dash {
            s.push_str(&format!(" stroke-dasharray=\"{}\"", d));
        }
        s
    }
}

/// Deterministic small-value source: xorshift64* seeded from shape geometry.
/// Produces jitter in roughly [-amp, amp].
struct Wobble {
    state: u64,
}

impl Wobble {
    fn new(seed: u64) -> Wobble {
        // Avoid a zero state (xorshift fixed point).
        Wobble {
            state: seed ^ 0x9E37_79B9_7F4A_7C15,
        }
    }

    /// Seed from a list of f64 coordinates via FNV-1a over their bit patterns.
    fn from_coords(coords: &[f64]) -> Wobble {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &c in coords {
            // Quantize so tiny float noise doesn't change the seed, but distinct
            // shapes still differ.
            let q = (c * 16.0).round() as i64 as u64;
            for byte in q.to_le_bytes() {
                h ^= byte as u64;
                h = h.wrapping_mul(0x0000_0100_0000_01B3);
            }
        }
        Wobble::new(h)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform-ish jitter in [-amp, amp].
    fn jitter(&mut self, amp: f64) -> f64 {
        // Map top 53 bits to [0,1).
        let r = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        (r * 2.0 - 1.0) * amp
    }
}

/// Format a coordinate compactly.
fn fmt(v: f64) -> String {
    let mut s = format!("{:.2}", v);
    // Trim trailing zeros / dot for compactness.
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    s
}

/// Build a wobbly path string tracing the polyline through `pts`. Each edge is
/// drawn as a quadratic curve whose control point is offset perpendicular to the
/// edge by a small jitter (1 interior wobble per edge). `closed` appends `Z`.
fn wobbly_path(pts: &[(f64, f64)], closed: bool, w: &mut Wobble, amp: f64) -> String {
    if pts.is_empty() {
        return String::new();
    }
    let mut d = String::new();
    // Start point jittered slightly.
    let (sx, sy) = pts[0];
    d.push_str(&format!("M {} {}", fmt(sx + w.jitter(amp)), fmt(sy + w.jitter(amp))));
    let n = pts.len();
    let edge_count = if closed { n } else { n - 1 };
    for e in 0..edge_count {
        let (ax, ay) = pts[e];
        let (bx, by) = pts[(e + 1) % n];
        let dx = bx - ax;
        let dy = by - ay;
        let len = (dx * dx + dy * dy).sqrt().max(1e-6);
        // Perpendicular unit vector.
        let (px, py) = (-dy / len, dx / len);
        // Control point near the edge midpoint, offset perpendicular.
        let off = w.jitter(amp);
        let mid_t = 0.5 + w.jitter(0.12); // wobble where along the edge
        let cx = ax + dx * mid_t + px * off;
        let cy = ay + dy * mid_t + py * off;
        // End point jittered a touch (kept near the true vertex).
        let ex = bx + w.jitter(amp * 0.6);
        let ey = by + w.jitter(amp * 0.6);
        d.push_str(&format!(" Q {} {} {} {}", fmt(cx), fmt(cy), fmt(ex), fmt(ey)));
    }
    if closed {
        d.push_str(" Z");
    }
    d
}

/// Emit the outline: a sketchy stroked path (drawn twice with different jitter
/// for the classic double-stroke effect). Returns the `<path …/>` elements.
fn outline_paths(pts: &[(f64, f64)], closed: bool, style: &Style, seed: &[f64]) -> String {
    if style.stroke.is_none() {
        return String::new();
    }
    let amp = 1.4;
    let mut s = String::new();
    // Two passes, seeded differently so the wobbles differ.
    for pass in 0..2u64 {
        let mut w = Wobble::from_coords(seed);
        // Advance the state per pass to decorrelate the two strokes.
        for _ in 0..(pass * 7 + 1) {
            w.next_u64();
        }
        let d = wobbly_path(pts, closed, &mut w, amp);
        s.push_str(&format!(
            "<path d=\"{}\" fill=\"none\"{}/>",
            d,
            style.stroke_attr()
        ));
    }
    s
}

/// Emit the fill: a single jittered closed path tracing the outline.
fn fill_path(pts: &[(f64, f64)], style: &Style, seed: &[f64]) -> String {
    if style.fill.is_none() {
        return String::new();
    }
    let mut w = Wobble::from_coords(seed);
    // Smaller jitter for the fill so it doesn't bleed past the outline much.
    let d = wobbly_path(pts, true, &mut w, 0.8);
    format!("<path d=\"{}\"{} stroke=\"none\"/>", d, style.fill_attr())
}

/// Approximate a circle/ellipse as an N-gon with small radial jitter.
fn ellipse_points(cx: f64, cy: f64, rx: f64, ry: f64, w: &mut Wobble) -> Vec<(f64, f64)> {
    let n = 16;
    let mut pts = Vec::with_capacity(n);
    for k in 0..n {
        let theta = (k as f64) / (n as f64) * std::f64::consts::TAU;
        // Radial jitter relative to radius, capped.
        let jr = w.jitter((rx.min(ry) * 0.06).min(2.0));
        let r_scale = 1.0 + jr / rx.max(1.0);
        pts.push((
            cx + theta.cos() * rx * r_scale,
            cy + theta.sin() * ry * r_scale,
        ));
    }
    pts
}

/// Parse a `points="x,y x,y …"` attribute into coordinate pairs.
fn parse_points(s: &str) -> Vec<(f64, f64)> {
    let mut nums = Vec::new();
    for tok in s.split(|c: char| c == ',' || c.is_whitespace()) {
        let t = tok.trim();
        if t.is_empty() {
            continue;
        }
        if let Ok(v) = t.parse::<f64>() {
            nums.push(v);
        }
    }
    nums.chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| (c[0], c[1]))
        .collect()
}

/// Roughen one shape element. `attrs_str` is the text between the tag name and
/// "/>" (i.e. the attribute list). Returns the replacement `<g>…</g>`, or `None`
/// to leave the element untouched (e.g. the background rect).
fn roughen_element(tag: &str, attrs_str: &str) -> Option<String> {
    let attrs = parse_attrs(attrs_str);
    let style = Style::from(&attrs);

    match tag {
        "rect" => {
            let x = getf(&attrs, "x")?;
            let y = getf(&attrs, "y")?;
            let wdt = getf(&attrs, "width")?;
            let hgt = getf(&attrs, "height")?;
            // Skip the injected full-bleed background rect (x=0, y=0): keep it
            // solid so the canvas stays clean.
            if x == 0.0 && y == 0.0 {
                return None;
            }
            let pts = vec![
                (x, y),
                (x + wdt, y),
                (x + wdt, y + hgt),
                (x, y + hgt),
            ];
            // NOTE: `rx` (rounded corners) intentionally ignored — treated as a
            // plain rectangle.
            let seed = [x, y, wdt, hgt];
            Some(wrap_group(&pts, true, &style, &seed))
        }
        "circle" => {
            let cx = getf(&attrs, "cx")?;
            let cy = getf(&attrs, "cy")?;
            let r = getf(&attrs, "r")?;
            let seed = [cx, cy, r];
            let mut w = Wobble::from_coords(&seed);
            let pts = ellipse_points(cx, cy, r, r, &mut w);
            Some(wrap_group(&pts, true, &style, &seed))
        }
        "ellipse" => {
            let cx = getf(&attrs, "cx")?;
            let cy = getf(&attrs, "cy")?;
            let rx = getf(&attrs, "rx")?;
            let ry = getf(&attrs, "ry")?;
            let seed = [cx, cy, rx, ry];
            let mut w = Wobble::from_coords(&seed);
            let pts = ellipse_points(cx, cy, rx, ry, &mut w);
            Some(wrap_group(&pts, true, &style, &seed))
        }
        "line" => {
            let x1 = getf(&attrs, "x1")?;
            let y1 = getf(&attrs, "y1")?;
            let x2 = getf(&attrs, "x2")?;
            let y2 = getf(&attrs, "y2")?;
            let pts = vec![(x1, y1), (x2, y2)];
            let seed = [x1, y1, x2, y2];
            // Lines have no fill; just the double-stroke outline.
            let mut g = String::from("<g>");
            g.push_str(&outline_paths(&pts, false, &style, &seed));
            g.push_str("</g>");
            Some(g)
        }
        "polygon" => {
            let pts = parse_points(get(&attrs, "points")?);
            if pts.is_empty() {
                return None;
            }
            let seed: Vec<f64> = pts.iter().flat_map(|&(a, b)| [a, b]).collect();
            Some(wrap_group(&pts, true, &style, &seed))
        }
        "polyline" => {
            let pts = parse_points(get(&attrs, "points")?);
            if pts.is_empty() {
                return None;
            }
            let seed: Vec<f64> = pts.iter().flat_map(|&(a, b)| [a, b]).collect();
            // Polyline is open; fill (if any) still closes the region.
            let mut g = String::from("<g>");
            g.push_str(&fill_path(&pts, &style, &seed));
            g.push_str(&outline_paths(&pts, false, &style, &seed));
            g.push_str("</g>");
            Some(g)
        }
        _ => None,
    }
}

/// Wrap fill + double-stroke outline of a closed shape in a `<g>`.
fn wrap_group(pts: &[(f64, f64)], closed: bool, style: &Style, seed: &[f64]) -> String {
    let mut g = String::from("<g>");
    g.push_str(&fill_path(pts, style, seed));
    g.push_str(&outline_paths(pts, closed, style, seed));
    g.push_str("</g>");
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap(inner: &str) -> String {
        format!("<svg xmlns=\"http://www.w3.org/2000/svg\">{}</svg>", inner)
    }

    #[test]
    fn rect_becomes_paths() {
        let mut svg = wrap(
            "<rect x=\"10\" y=\"20\" width=\"100\" height=\"40\" rx=\"5\" \
             fill=\"rgb(200,200,255)\" stroke=\"rgb(0,0,0)\" stroke-width=\"2\"/>",
        );
        roughen(&mut svg);
        assert!(svg.contains("<path"), "should contain a hand-drawn path");
        // The original <rect element should be gone (no background here).
        assert!(!svg.contains("<rect"), "rect should be replaced: {svg}");
        // Fill and stroke preserved.
        assert!(svg.contains("rgb(200,200,255)"), "fill preserved");
        assert!(svg.contains("rgb(0,0,0)"), "stroke preserved");
        assert!(svg.contains("stroke-width=\"2\""), "stroke-width preserved");
    }

    #[test]
    fn background_rect_skipped() {
        let mut svg = wrap(
            "<rect x=\"0\" y=\"0\" width=\"500\" height=\"300\" fill=\"rgb(255,255,255)\"/>\
             <rect x=\"10\" y=\"10\" width=\"20\" height=\"20\" fill=\"rgb(1,2,3)\"/>",
        );
        roughen(&mut svg);
        // Background rect stays solid (untouched).
        assert!(
            svg.contains("<rect x=\"0\" y=\"0\" width=\"500\" height=\"300\""),
            "background rect should remain: {svg}"
        );
        // The non-background rect is roughened.
        assert!(svg.contains("rgb(1,2,3)"));
        assert!(svg.contains("<path"));
    }

    #[test]
    fn circle_becomes_polygon_path() {
        let mut svg = wrap(
            "<circle cx=\"50\" cy=\"50\" r=\"30\" fill=\"none\" \
             stroke=\"rgb(10,20,30)\" stroke-width=\"1\"/>",
        );
        roughen(&mut svg);
        assert!(!svg.contains("<circle"), "circle replaced");
        assert!(svg.contains("<path"), "jittered polygon path emitted");
        // Multiple Q segments (N-gon) present.
        assert!(svg.matches(" Q ").count() >= 10, "should be an N-gon: {svg}");
        assert!(svg.contains("rgb(10,20,30)"));
    }

    #[test]
    fn ellipse_and_line_and_poly() {
        let mut svg = wrap(
            "<ellipse cx=\"40\" cy=\"40\" rx=\"20\" ry=\"10\" fill=\"rgb(9,9,9)\"/>\
             <line x1=\"0\" y1=\"0\" x2=\"50\" y2=\"50\" stroke=\"rgb(1,1,1)\" \
             stroke-width=\"3\" stroke-dasharray=\"4 2\"/>\
             <polygon points=\"0,0 10,0 5,8\" fill=\"rgb(2,2,2)\" stroke=\"rgb(3,3,3)\"/>\
             <polyline points=\"0,0 5,5 10,0\" fill=\"none\" stroke=\"rgb(4,4,4)\"/>",
        );
        roughen(&mut svg);
        for t in ["<ellipse", "<line", "<polygon", "<polyline"] {
            assert!(!svg.contains(t), "{t} should be replaced: {svg}");
        }
        // Dash preserved on the line outline.
        assert!(svg.contains("stroke-dasharray=\"4 2\""), "dash preserved");
        assert!(svg.contains("rgb(9,9,9)"));
        assert!(svg.contains("rgb(3,3,3)"));
    }

    #[test]
    fn text_and_path_untouched() {
        let inner = "<text x=\"5\" y=\"5\">hello</text>\
                     <path d=\"M0 0 L10 10\" stroke=\"rgb(0,0,0)\"/>";
        let mut svg = wrap(inner);
        roughen(&mut svg);
        assert!(svg.contains("<text x=\"5\" y=\"5\">hello</text>"), "text untouched");
        assert!(svg.contains("<path d=\"M0 0 L10 10\""), "existing path untouched");
    }

    #[test]
    fn no_shape_svg_unchanged() {
        let mut svg = wrap("<g><text>x</text></g>");
        let orig = svg.clone();
        roughen(&mut svg);
        assert_eq!(svg, orig, "no-shape SVG unchanged");
    }

    #[test]
    fn deterministic() {
        let base = wrap(
            "<rect x=\"3\" y=\"7\" width=\"40\" height=\"22\" fill=\"rgb(5,5,5)\" \
             stroke=\"rgb(0,0,0)\"/>\
             <circle cx=\"80\" cy=\"30\" r=\"15\" stroke=\"rgb(1,1,1)\"/>",
        );
        let mut a = base.clone();
        let mut b = base.clone();
        roughen(&mut a);
        roughen(&mut b);
        assert_eq!(a, b, "roughen must be deterministic");
    }

    #[test]
    fn still_valid_svg_envelope() {
        let mut svg = wrap("<rect x=\"1\" y=\"1\" width=\"2\" height=\"2\" fill=\"rgb(0,0,0)\"/>");
        roughen(&mut svg);
        assert!(svg.starts_with("<svg"), "starts with <svg: {svg}");
        assert!(svg.ends_with("</svg>"), "ends with </svg>: {svg}");
    }

    #[test]
    fn utf8_preserved() {
        // Non-ASCII text near a shape must survive the rewrite intact.
        let mut svg = wrap(
            "<text>café — 日本語</text>\
             <rect x=\"2\" y=\"2\" width=\"5\" height=\"5\" fill=\"rgb(0,0,0)\"/>",
        );
        roughen(&mut svg);
        assert!(svg.contains("café — 日本語"), "utf8 text preserved: {svg}");
    }
}
