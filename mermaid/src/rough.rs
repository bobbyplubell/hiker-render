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
    let shapes = ["rect", "circle", "ellipse", "line", "polygon", "polyline", "path"];
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
/// split into two quadratic sub-curves, each bowed perpendicular to the edge by
/// an independent jitter (roughjs-style "bowing") so even straight segments read
/// as hand-drawn. `closed` appends `Z` and connects the last point to the first.
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
        // Two bowed sub-curves per edge: control points near 1/4 and 3/4 along
        // the edge, each offset perpendicular by an independent jitter. The
        // amplitude scales with edge length (capped) so long edges bow more.
        let edge_amp = amp * (1.0 + (len / 200.0).min(1.0));
        // First half: midpoint of [a, mid].
        let mx = ax + dx * 0.5;
        let my = ay + dy * 0.5;
        let off1 = w.jitter(edge_amp);
        let t1 = 0.25 + w.jitter(0.08);
        let c1x = ax + dx * t1 + px * off1;
        let c1y = ay + dy * t1 + py * off1;
        d.push_str(&format!(
            " Q {} {} {} {}",
            fmt(c1x),
            fmt(c1y),
            fmt(mx + w.jitter(amp * 0.5)),
            fmt(my + w.jitter(amp * 0.5))
        ));
        // Second half: midpoint of [mid, b]; end near the true vertex (with a
        // touch of overshoot for closed shapes, giving the sketchy "past the
        // corner" look).
        let off2 = w.jitter(edge_amp);
        let t2 = 0.75 + w.jitter(0.08);
        let c2x = ax + dx * t2 + px * off2;
        let c2y = ay + dy * t2 + py * off2;
        let end_amp = if closed { amp } else { amp * 0.6 };
        let ex = bx + w.jitter(end_amp);
        let ey = by + w.jitter(end_amp);
        d.push_str(&format!(" Q {} {} {} {}", fmt(c2x), fmt(c2y), fmt(ex), fmt(ey)));
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
    let amp = 2.2;
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
    let d = wobbly_path(pts, true, &mut w, 1.2);
    format!("<path d=\"{}\"{} stroke=\"none\"/>", d, style.fill_attr())
}

/// Approximate a circle/ellipse as an N-gon with small radial jitter.
fn ellipse_points(cx: f64, cy: f64, rx: f64, ry: f64, w: &mut Wobble) -> Vec<(f64, f64)> {
    let n = 16;
    let mut pts = Vec::with_capacity(n);
    for k in 0..n {
        let theta = (k as f64) / (n as f64) * std::f64::consts::TAU;
        // Radial jitter relative to radius, capped.
        let jr = w.jitter((rx.min(ry) * 0.12).min(3.5));
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

/// Flatten an SVG path `d` string into one or more polylines (sub-paths) plus a
/// `closed` flag per sub-path. Supports the absolute and relative commands our
/// renderers actually emit: M/m, L/l, H/h, V/v, C/c, Q/q, Z/z. Curves are
/// sampled into short line segments. Unknown commands abort parsing (returns
/// what was collected so far) so we degrade gracefully rather than corrupt.
fn flatten_path(d: &str) -> Vec<(Vec<(f64, f64)>, bool)> {
    // Tokenize into commands (alpha) and numbers.
    let mut nums: Vec<f64> = Vec::new();
    let mut cmds: Vec<(char, usize)> = Vec::new(); // (command, index into nums where its args start)
    let b = d.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i] as char;
        if c.is_ascii_alphabetic() {
            cmds.push((c, nums.len()));
            i += 1;
        } else if c == '-' || c == '+' || c == '.' || c.is_ascii_digit() {
            // Parse a number (handles leading sign, decimal, exponent).
            let start = i;
            i += 1;
            while i < b.len() {
                let ch = b[i] as char;
                if ch.is_ascii_digit() || ch == '.' {
                    i += 1;
                } else if (ch == 'e' || ch == 'E')
                    && i + 1 < b.len()
                    && (b[i + 1] == b'-' || b[i + 1] == b'+' || (b[i + 1] as char).is_ascii_digit())
                {
                    i += 2;
                } else {
                    break;
                }
            }
            if let Ok(v) = d[start..i].parse::<f64>() {
                nums.push(v);
            }
        } else {
            i += 1; // whitespace / comma
        }
    }

    let mut subpaths: Vec<(Vec<(f64, f64)>, bool)> = Vec::new();
    let mut cur: Vec<(f64, f64)> = Vec::new();
    let mut cur_closed = false;
    let (mut x, mut y) = (0.0f64, 0.0f64);
    let (mut start_x, mut start_y) = (0.0f64, 0.0f64);

    let flush = |subpaths: &mut Vec<(Vec<(f64, f64)>, bool)>,
                 cur: &mut Vec<(f64, f64)>,
                 closed: &mut bool| {
        if cur.len() >= 2 {
            subpaths.push((std::mem::take(cur), *closed));
        } else {
            cur.clear();
        }
        *closed = false;
    };

    // Sample a cubic Bezier into line segments.
    let sample_cubic = |p0: (f64, f64), p1: (f64, f64), p2: (f64, f64), p3: (f64, f64), out: &mut Vec<(f64, f64)>| {
        let steps = 4;
        for s in 1..=steps {
            let t = s as f64 / steps as f64;
            let mt = 1.0 - t;
            let px = mt * mt * mt * p0.0
                + 3.0 * mt * mt * t * p1.0
                + 3.0 * mt * t * t * p2.0
                + t * t * t * p3.0;
            let py = mt * mt * mt * p0.1
                + 3.0 * mt * mt * t * p1.1
                + 3.0 * mt * t * t * p2.1
                + t * t * t * p3.1;
            out.push((px, py));
        }
    };
    let sample_quad = |p0: (f64, f64), p1: (f64, f64), p2: (f64, f64), out: &mut Vec<(f64, f64)>| {
        let steps = 3;
        for s in 1..=steps {
            let t = s as f64 / steps as f64;
            let mt = 1.0 - t;
            let px = mt * mt * p0.0 + 2.0 * mt * t * p1.0 + t * t * p2.0;
            let py = mt * mt * p0.1 + 2.0 * mt * t * p1.1 + t * t * p2.1;
            out.push((px, py));
        }
    };

    for ci in 0..cmds.len() {
        let (cmd, arg_start) = cmds[ci];
        let arg_end = if ci + 1 < cmds.len() { cmds[ci + 1].1 } else { nums.len() };
        let args = &nums[arg_start..arg_end];
        let rel = cmd.is_ascii_lowercase();
        match cmd.to_ascii_uppercase() {
            'M' => {
                // First pair is moveto; subsequent pairs are implicit linetos.
                let mut k = 0;
                let mut first = true;
                while k + 1 < args.len() {
                    let (mut nx, mut ny) = (args[k], args[k + 1]);
                    if rel {
                        nx += x;
                        ny += y;
                    }
                    if first {
                        flush(&mut subpaths, &mut cur, &mut cur_closed);
                        x = nx;
                        y = ny;
                        start_x = x;
                        start_y = y;
                        cur.push((x, y));
                        first = false;
                    } else {
                        x = nx;
                        y = ny;
                        cur.push((x, y));
                    }
                    k += 2;
                }
            }
            'L' => {
                let mut k = 0;
                while k + 1 < args.len() {
                    let (mut nx, mut ny) = (args[k], args[k + 1]);
                    if rel {
                        nx += x;
                        ny += y;
                    }
                    x = nx;
                    y = ny;
                    cur.push((x, y));
                    k += 2;
                }
            }
            'H' => {
                for &a in args {
                    x = if rel { x + a } else { a };
                    cur.push((x, y));
                }
            }
            'V' => {
                for &a in args {
                    y = if rel { y + a } else { a };
                    cur.push((x, y));
                }
            }
            'C' => {
                let mut k = 0;
                while k + 5 < args.len() {
                    let (mut c1x, mut c1y) = (args[k], args[k + 1]);
                    let (mut c2x, mut c2y) = (args[k + 2], args[k + 3]);
                    let (mut ex, mut ey) = (args[k + 4], args[k + 5]);
                    if rel {
                        c1x += x;
                        c1y += y;
                        c2x += x;
                        c2y += y;
                        ex += x;
                        ey += y;
                    }
                    sample_cubic((x, y), (c1x, c1y), (c2x, c2y), (ex, ey), &mut cur);
                    x = ex;
                    y = ey;
                    k += 6;
                }
            }
            'Q' => {
                let mut k = 0;
                while k + 3 < args.len() {
                    let (mut c1x, mut c1y) = (args[k], args[k + 1]);
                    let (mut ex, mut ey) = (args[k + 2], args[k + 3]);
                    if rel {
                        c1x += x;
                        c1y += y;
                        ex += x;
                        ey += y;
                    }
                    sample_quad((x, y), (c1x, c1y), (ex, ey), &mut cur);
                    x = ex;
                    y = ey;
                    k += 4;
                }
            }
            'Z' => {
                cur_closed = true;
                x = start_x;
                y = start_y;
                flush(&mut subpaths, &mut cur, &mut cur_closed);
            }
            _ => {
                // Unsupported command (e.g. arcs/smooth curves): bail out, keeping
                // what we have. The caller falls back to leaving the path as-is.
                return Vec::new();
            }
        }
    }
    flush(&mut subpaths, &mut cur, &mut cur_closed);
    subpaths
}

/// Pull through attributes a roughened `<path>` must keep verbatim on its stroke
/// output: arrow markers and any vector-effect. Returns a string like
/// ` marker-end="url(#arrow)"`.
fn passthrough_attrs(attrs: &[(String, String)]) -> String {
    let mut s = String::new();
    for key in ["marker-start", "marker-end", "marker-mid", "vector-effect"] {
        if let Some(v) = get(attrs, key) {
            s.push_str(&format!(" {}=\"{}\"", key, v));
        }
    }
    s
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
            // Skip the injected full-bleed background rect: it sits at (0,0) and
            // has a fill but NO stroke. A real shape at the origin (e.g. a class
            // box that happens to be laid out at 0,0) still has a stroke, so it
            // is roughened. Checking stroke avoids treating such shapes as the
            // background.
            if x == 0.0 && y == 0.0 && style.stroke.is_none() {
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
        "path" => {
            // Only roughen stroked paths (edges, stadium/cylinder node outlines).
            // Fill-only paths with no stroke are arrowhead markers and similar
            // glyphs inside <defs>; leave those crisp.
            style.stroke.as_ref()?;
            let subpaths = flatten_path(get(&attrs, "d")?);
            if subpaths.is_empty() {
                return None; // unparseable / unsupported commands: leave as-is
            }
            let passthrough = passthrough_attrs(&attrs);
            let mut g = String::from("<g>");
            for (pts, closed) in &subpaths {
                if pts.len() < 2 {
                    continue;
                }
                let seed: Vec<f64> = pts.iter().flat_map(|&(a, b)| [a, b]).collect();
                if *closed {
                    g.push_str(&fill_path(pts, &style, &seed));
                }
                // Emit the sketchy outline, preserving markers/vector-effect.
                if style.stroke.is_some() {
                    // Paths are already flattened into many short segments, so a
                    // smaller per-segment amplitude keeps edges hand-drawn
                    // without turning into noise.
                    let amp = 1.3;
                    for pass in 0..2u64 {
                        let mut w = Wobble::from_coords(&seed);
                        for _ in 0..(pass * 7 + 1) {
                            w.next_u64();
                        }
                        let d = wobbly_path(pts, *closed, &mut w, amp);
                        // Markers (arrowheads) only on the second/final stroke so
                        // we don't draw two overlapping arrowheads.
                        let marks = if pass == 1 { passthrough.as_str() } else { "" };
                        g.push_str(&format!(
                            "<path d=\"{}\" fill=\"none\"{}{}/>",
                            d,
                            style.stroke_attr(),
                            marks
                        ));
                    }
                }
            }
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
    fn stroked_origin_rect_is_roughened() {
        // A real shape laid out at (0,0) WITH a stroke (e.g. a class box) must be
        // roughened — only the stroke-less full-bleed background is skipped.
        let mut svg = wrap(
            "<rect x=\"0\" y=\"0\" width=\"120\" height=\"80\" fill=\"rgb(236,236,255)\" \
             stroke=\"rgb(147,112,219)\" stroke-width=\"1.5\"/>",
        );
        roughen(&mut svg);
        assert!(!svg.contains("<rect"), "stroked origin rect should be replaced: {svg}");
        assert!(svg.contains("<path"), "roughened into paths");
        assert!(svg.contains("rgb(147,112,219)"), "stroke preserved");
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
    fn text_untouched_and_fill_only_path_untouched() {
        // Text and a fill-only path (e.g. an arrowhead marker glyph: no stroke)
        // must be left crisp; only stroked paths get roughened.
        let inner = "<text x=\"5\" y=\"5\">hello</text>\
                     <path d=\"M0,0 L9,4 L0,8 Z\" fill=\"rgb(51,51,51)\"/>";
        let mut svg = wrap(inner);
        roughen(&mut svg);
        assert!(svg.contains("<text x=\"5\" y=\"5\">hello</text>"), "text untouched");
        assert!(
            svg.contains("<path d=\"M0,0 L9,4 L0,8 Z\" fill=\"rgb(51,51,51)\"/>"),
            "fill-only (marker) path untouched: {svg}"
        );
    }

    #[test]
    fn stroked_path_becomes_sketchy() {
        // An edge-style stroked path is flattened and re-drawn as wobbly strokes.
        let inner = "<path d=\"M0,0 C10,0 20,10 30,10\" fill=\"none\" \
                     stroke=\"rgb(51,51,51)\" stroke-width=\"1.5\" marker-end=\"url(#arrow)\"/>";
        let mut svg = wrap(inner);
        roughen(&mut svg);
        // Original cubic path is gone, replaced by Q-curve strokes.
        assert!(!svg.contains("C10,0 20,10 30,10"), "original cubic replaced: {svg}");
        assert!(svg.contains(" Q "), "wobbly quadratic strokes emitted: {svg}");
        // Double-stroke: two outline paths.
        assert_eq!(svg.matches("<path").count(), 2, "two stroke passes: {svg}");
        // Marker preserved exactly once (only on the final pass).
        assert_eq!(
            svg.matches("marker-end=\"url(#arrow)\"").count(),
            1,
            "exactly one arrowhead: {svg}"
        );
        assert!(svg.contains("stroke=\"rgb(51,51,51)\""), "stroke color preserved");
    }

    #[test]
    fn stroked_path_deterministic() {
        let inner = "<path d=\"M5,5 L40,20 L20,50 Z\" fill=\"rgb(9,9,9)\" \
                     stroke=\"rgb(0,0,0)\" stroke-width=\"2\"/>";
        let mut a = wrap(inner);
        let mut b = wrap(inner);
        roughen(&mut a);
        roughen(&mut b);
        assert_eq!(a, b, "path roughening must be deterministic");
        // Closed path (Z) also produced a fill path.
        assert!(a.matches("<path").count() >= 3, "fill + two strokes: {a}");
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
