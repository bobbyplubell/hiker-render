//! Digital timing waveforms (`{ signal: [ … ] }`).
//!
//! Port of the `references/wavedrom/lib` timing pipeline:
//! `rec` (flatten the signal tree into lanes + groups) → `parseWaveLane`
//! (wave string → half-bricks, see [`wavelane`]) → `renderWaveLane` /
//! `renderMarks` / `renderGaps` / `renderGroups` / `renderArcs` → SVG.
//!
//! Geometry follows WaveDrom's default skin/`lane.js`: half-brick width
//! `xs = 20` (×hscale via repetition), lane pitch `yo = 30`, lane height
//! `ys = 20`, the signal name in a right-aligned left column ending at `tgo`.

#[path = "bricks.rs"]
mod bricks;
mod wavelane;

use std::fmt::Write as _;

use serde_json::Value;

use crate::svgutil::{opacity_attr, rgb, text};
use crate::{WaveDromError, WaveDromOptions, WaveDromRender};

// ── Skin/lane constants (lane.js + default skin socket). ────────────────────
const XS: f32 = 20.0; // half-brick width
const YO: f32 = 30.0; // lane pitch (top of lane i = i*yo)
const YS: f32 = 20.0; // brick (lane) height
const Y0: f32 = 5.0; // first lane's top gap
const YM: f32 = 15.0; // text baseline within a lane (central-ish)
const TGO: f32 = -10.0; // name-column right edge, relative to lanes origin
const XLABEL: f32 = 6.0; // data-label x nudge

/// A flattened signal lane plus its left-column indent (px, from `rec`).
struct Lane {
    sig: Value,
    indent: f32,
}

/// A group bracket spanning lanes `[y, y+height)` with an optional label.
struct Group {
    x: f32,
    y: usize,
    height: usize,
    name: Option<String>,
}

/// Walker state for [`rec`].
struct RecState {
    x: f32,
    y: usize,
    lanes: Vec<Lane>,
    groups: Vec<Group>,
    xx: f32,
    name: Option<String>,
}

/// Port of `rec.js`: flatten the (possibly nested) `signal` array into a flat
/// list of lanes (each carrying its indent `x`) and group brackets.
fn rec(items: &[Value], mut st: RecState) -> RecState {
    let mut delta_x = 10.0;
    let mut name = None;
    if let Some(first) = items.first().filter(|f| f.is_string() || f.is_number()) {
        name = first.as_str().map(|s| s.to_string()).or_else(|| Some(first.to_string()));
        delta_x = 25.0;
    }
    st.x += delta_x;
    for item in items {
        if item.is_array() {
            let old_y = st.y;
            let arr = item.as_array().unwrap();
            st = rec(arr, st);
            st.groups.push(Group {
                x: st.xx,
                y: old_y,
                height: st.y - old_y,
                name: st.name.take(),
            });
        } else if item.is_object() {
            // Signal object (or `{}` spacer — still a blank lane).
            st.lanes.push(Lane { sig: item.clone(), indent: st.x });
            st.y += 1;
        }
        // (strings/numbers other than the leading label are ignored, as in JS)
    }
    st.xx = st.x;
    st.x -= delta_x;
    st.name = name;
    st
}

/// Resolved config (hscale + head/foot ticks/text).
struct Config {
    hscale: usize,
    yh0: f32, // head tick band height
    yh1: f32, // head text band height
    yf0: f32, // foot tick band height
    yf1: f32, // foot text band height
}

fn parse_config(root: &Value) -> Config {
    let mut hscale = 1usize;
    if let Some(h) = root.get("config").and_then(|c| c.get("hscale")).and_then(|v| v.as_f64()) {
        let h = h.round();
        if h > 0.0 {
            hscale = (h.min(100.0)) as usize;
        }
    }
    let head = root.get("head");
    let foot = root.get("foot");
    let has_tickish = |o: Option<&Value>| {
        o.map(|h| {
            h.get("tick").map(|v| !v.is_null()).unwrap_or(false)
                || h.get("tock").map(|v| !v.is_null()).unwrap_or(false)
        })
        .unwrap_or(false)
    };
    let has_text = |o: Option<&Value>| {
        o.and_then(|h| h.get("text")).map(|v| !v.is_null()).unwrap_or(false)
    };
    Config {
        hscale,
        yh0: if has_tickish(head) { 20.0 } else { 0.0 },
        yh1: if has_text(head) { 46.0 } else { 0.0 },
        yf0: if has_tickish(foot) { 20.0 } else { 0.0 },
        yf1: if has_text(foot) { 46.0 } else { 0.0 },
    }
}

/// Resolved `config.skin`. Skins differ by brick width (the socket `width:` in
/// each skin file: default 20, narrow 10, narrower 5, narrowerer 2.5) and by
/// colors (dark = dark bg + light fg; lowkey = muted grey). We model the width
/// purely as an x-scale `sx = xs/20` applied to each lane's bricks and to the
/// canvas/marks geometry — proportional narrowing, an acceptable match to the
/// narrow skins (which ship hand-redrawn-narrower glyphs). Colors override the
/// host `foreground`/`background` and the grid/text marks. Unknown skin →
/// default.
#[derive(Clone, Copy)]
struct Skin {
    /// Effective half-brick width in px (default `XS` = 20).
    xs: f32,
    /// Foreground (signal/text) color override, or `None` to keep host fg.
    fg: Option<[u8; 4]>,
    /// Background override, or `None` to keep host background.
    bg: Option<[u8; 4]>,
    /// Gridline / tick-number color.
    grid: [u8; 4],
    /// Edge/group/info-label color.
    info: [u8; 4],
}

impl Skin {
    /// x-scale relative to the default 20px half-brick.
    fn sx(&self) -> f32 {
        self.xs / XS
    }
}

/// Map `config.skin` (a string) to a [`Skin`]. The `xs` values are the socket
/// `width:` read from `references/wavedrom/skins/{name}.js`.
fn parse_skin(root: &Value) -> Skin {
    let default_grid = [136, 136, 136, 255]; // `#888` gridlines
    let default_info = [0, 65, 196, 255]; // `.info` blue
    let name = root
        .get("config")
        .and_then(|c| c.get("skin"))
        .and_then(|v| v.as_str())
        .unwrap_or("default");
    match name {
        "narrow" => Skin { xs: 10.0, fg: None, bg: None, grid: default_grid, info: default_info },
        "narrower" => Skin { xs: 5.0, fg: None, bg: None, grid: default_grid, info: default_info },
        "narrowerer" => {
            Skin { xs: 2.5, fg: None, bg: None, grid: default_grid, info: default_info }
        }
        "dark" => Skin {
            // Dark background + light (#fff) strokes/text; skins/dark.js uses
            // `.s1{stroke:#fff}`. Grid/info lightened to stay visible.
            xs: XS,
            fg: Some([255, 255, 255, 255]),
            bg: Some([40, 40, 40, 255]),
            grid: [160, 160, 160, 255],
            info: [120, 170, 255, 255],
        },
        "lowkey" => Skin {
            // Muted grey strokes/grid/text (skins/lowkey.js `.s1{stroke:#606060}`).
            xs: XS,
            fg: Some([96, 96, 96, 255]),
            bg: None,
            grid: [200, 200, 200, 255],
            info: [96, 96, 96, 255],
        },
        // "default" and any unknown skin.
        _ => Skin { xs: XS, fg: None, bg: None, grid: default_grid, info: default_info },
    }
}

/// Render a timing-waveform diagram from a `{ signal: [ … ] }` WaveJSON object.
pub fn render(root: &Value, opts: &WaveDromOptions) -> Result<WaveDromRender, WaveDromError> {
    let signal = root
        .get("signal")
        .and_then(|v| v.as_array())
        .ok_or_else(|| WaveDromError::Unsupported("`signal` must be an array".into()))?;

    if signal.is_empty() {
        return Err(WaveDromError::Empty);
    }

    let cfg = parse_config(root);
    let hscale = cfg.hscale;

    // Flatten the tree.
    let st = rec(
        signal,
        RecState {
            x: 0.0,
            y: 0,
            lanes: Vec::new(),
            groups: Vec::new(),
            xx: 0.0,
            name: None,
        },
    );
    let RecState { lanes, mut groups, xx: root_xx, name: root_name, .. } = st;

    if lanes.is_empty() {
        return Err(WaveDromError::Empty);
    }

    // The top-level `signal` array can itself carry a leading-string group
    // label (e.g. `signal:["grp", {…}]`). `rec` returns that as `st.name` but
    // doesn't push it (render-signal.js ignores the root name); WaveDrom users
    // expect it bracketed, so add it as a group spanning all lanes.
    if let Some(name) = root_name {
        groups.push(Group { x: root_xx, y: 0, height: lanes.len(), name: Some(name) });
    }

    // Skin: brick-width x-scale + color overrides (dark/lowkey).
    let skin = parse_skin(root);
    let sx = skin.sx();
    let background = skin.bg.unwrap_or(opts.background);
    let fg = skin.fg.unwrap_or(opts.foreground);
    let info = skin.info; // group/edge labels
    let muted = skin.grid; // tick numbers / gridlines

    // ── Per-lane: parse waves, gather max width, name-column width. ──────────
    struct LaneData {
        bricks: Vec<String>,
        data: Vec<String>,
        markers: Vec<f32>,
        num_unseen_markers: usize,
        name: String,
        indent: f32,
        phase_bricks: usize,
        period: usize,
        wave: String,
        node: Option<String>,
    }

    let mut laned: Vec<LaneData> = Vec::new();
    let mut xmax_bricks = 0usize; // max half-brick count across lanes
    let mut name_col = 0.0f32; // widest (name width + indent)

    for ln in &lanes {
        let sig = &ln.sig;
        let name = sig.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let period =
            sig.get("period").and_then(|v| v.as_f64()).map(|p| p.max(1.0).round() as usize).unwrap_or(1);
        let phase = sig.get("phase").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let phase_bricks = (phase * 2.0).max(0.0).round() as usize;
        let extra = (period * hscale).saturating_sub(1);

        let wave = sig.get("wave").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let parsed = wavelane::parse_wave_lane(&wave, extra, period, phase_bricks);
        let markers = wavelane::find_lane_markers(&parsed.bricks);

        // `data` may be an array or a whitespace-separated string; slice past
        // markers hidden by phase.
        let mut data: Vec<String> = match sig.get("data") {
            Some(Value::Array(a)) => {
                a.iter().map(|v| v.as_str().map(|s| s.to_string()).unwrap_or_else(|| v.to_string())).collect()
            }
            Some(Value::String(s)) => s.split_whitespace().map(|t| t.to_string()).collect(),
            _ => Vec::new(),
        };
        if parsed.num_unseen_markers <= data.len() {
            data.drain(0..parsed.num_unseen_markers);
        }

        xmax_bricks = xmax_bricks.max(parsed.bricks.len());
        let nw = crate::font::line_width(&name, 11.0) + ln.indent;
        name_col = name_col.max(nw);

        let node = sig.get("node").and_then(|v| v.as_str()).map(|s| s.to_string());

        laned.push(LaneData {
            bricks: parsed.bricks,
            data,
            markers,
            num_unseen_markers: parsed.num_unseen_markers,
            name,
            indent: ln.indent,
            phase_bricks,
            period,
            wave,
            node,
        });
    }

    // Left column width (xg): round the widest name up to an xs grid, like
    // render-signal.js (`ceil((xmax - tgo)/xs)*xs`).
    let xg = ((name_col - TGO) / XS).ceil().max(1.0) * XS;

    let xmax = xmax_bricks as f32; // total half-bricks → drawing width units
    let lane_count = laned.len();

    // ── Overall canvas size (insert-svg-template.js). ───────────────────────
    let head_off = cfg.yh0 + cfg.yh1;
    let foot_off = cfg.yf0 + cfg.yf1;
    let content_h = lane_count as f32 * YO;
    // The brick area (everything right of the name column) narrows by `sx`.
    let width = xg + XS * (xmax + 1.0) * sx;
    let height = content_h + head_off + foot_off;

    // ── Emit SVG. ───────────────────────────────────────────────────────────
    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" \
         xmlns:xlink=\"http://www.w3.org/1999/xlink\" \
         width=\"{w:.0}\" height=\"{h:.0}\" viewBox=\"0 0 {w:.0} {h:.0}\">",
        w = width,
        h = height,
    );
    // Full-bleed background (skin may override, e.g. dark).
    if background[3] > 0 {
        let _ = write!(
            svg,
            "<rect x=\"0\" y=\"0\" width=\"{w:.0}\" height=\"{h:.0}\" fill=\"{c}\"{o}/>",
            w = width,
            h = height,
            c = rgb(background),
            o = opacity_attr("fill-opacity", background),
        );
    }

    // Marks live in the lanes coordinate space; the whole lane area is shifted
    // right by `xg` and down by the head offset.
    let lanes_ox = xg + 0.5;
    let lanes_oy = head_off + 0.5;
    let _ = write!(svg, "<g transform=\"translate({lanes_ox:.2},{lanes_oy:.2})\">");

    // ── Marks: gridlines + head/foot ticks + head/foot text. ────────────────
    render_marks(&mut svg, root, &cfg, xmax, lane_count, sx, fg, muted);

    // ── Lanes: name + bricks + data labels. ─────────────────────────────────
    for (j, ld) in laned.iter().enumerate() {
        let ly = Y0 + j as f32 * YO;
        let _ = write!(svg, "<g transform=\"translate(0,{ly:.2})\">");

        // Name (right-anchored at tgo, foreground).
        if !ld.name.is_empty() {
            text(&mut svg, &ld.name, TGO, YM, "end", 11.0, &opts.font_family, fg, None);
        }

        // Bricks. WaveDrom shifts the draw group by a fractional `xoffset` for
        // phase; we already dropped whole half-bricks, so the residual is the
        // fractional part. (`tmp0[1] = phase + xmin/2`.)
        let phase = ld.phase_bricks as f32 / 2.0;
        let xoffset = if phase > 0.0 { phase.ceil() * 2.0 - 2.0 * phase } else { -2.0 * phase };
        let draw_ox = xoffset * XS * sx;
        // The brick glyphs are unit (20px) cells; narrow skins x-scale them by
        // `sx` (proportional narrowing — see `Skin`). Data labels/gaps live in
        // this same scaled space.
        let _ = write!(svg, "<g transform=\"translate({draw_ox:.2},0) scale({sx:.4},1)\">");
        for (i, brick) in ld.bricks.iter().enumerate() {
            let data_fill = bricks::data_color_index(brick)
                .map(|idx| bricks::palette_color(idx, &opts.series_palette));
            bricks::emit_brick(&mut svg, brick, i as f32 * XS, 0.0, fg, data_fill);
        }
        // Data labels at marker centers (drawn in unscaled-x space so the glyph
        // shapes stay round, but positioned at the scaled marker centers).
        svg.push_str("</g>"); // draw (scaled) group
        if !ld.data.is_empty() {
            let label_ox = draw_ox;
            let _ = write!(svg, "<g transform=\"translate({label_ox:.2},0)\">");
            for (i, label) in ld.markers.iter().zip(ld.data.iter()) {
                if label.is_empty() {
                    continue;
                }
                let lx = (i * XS + XLABEL) * sx;
                text(&mut svg, label, lx, YM, "middle", 11.0, &opts.font_family, fg, None);
            }
            svg.push_str("</g>");
        }

        // Gaps (`|`): drawn over the lane at each break position.
        let gaps = wavelane::gap_positions(&ld.wave, ld.period, hscale, ld.phase_bricks);
        for gx in gaps {
            bricks::emit_gap(&mut svg, gx * XS * sx, 0.0, fg);
        }

        svg.push_str("</g>"); // lane group
        let _ = ld.indent; // indent already folded into name_col; name stays at tgo
        let _ = ld.num_unseen_markers;
    }

    // ── Groups: left brackets + rotated labels (render-groups.js). ──────────
    render_groups(&mut svg, &groups, &cfg, info, &opts.font_family);

    // ── Arcs/edges (render-arcs.js). ────────────────────────────────────────
    let node_lanes: Vec<NodeLane> = laned
        .iter()
        .map(|l| NodeLane {
            node: l.node.clone(),
            period: l.period,
            phase_bricks: l.phase_bricks,
        })
        .collect();
    render_arcs(&mut svg, root, &node_lanes, hscale, sx, info, &opts.font_family);

    svg.push_str("</g>"); // lanes translate
    svg.push_str("</svg>");

    Ok(WaveDromRender { svg, width_px: width, height_px: height })
}

/// Minimal projection of lane data needed by the arc renderer.
struct NodeLane {
    node: Option<String>,
    period: usize,
    phase_bricks: usize,
}

// ── Marks ───────────────────────────────────────────────────────────────────
fn render_marks(
    svg: &mut String,
    root: &Value,
    cfg: &Config,
    xmax: f32,
    lane_count: usize,
    sx: f32,
    fg: [u8; 4],
    muted: [u8; 4],
) {
    let mstep = 2.0 * cfg.hscale as f32; // half-bricks per cycle
    let mmstep = mstep * XS * sx; // px per cycle (narrowed by skin)
    let marks = xmax / mstep; // number of cycles (fractional)
    let gy = lane_count as f32 * YO;

    let marks_off = root
        .get("config")
        .and_then(|c| c.get("marks"))
        .and_then(|v| v.as_bool())
        .map(|b| !b)
        .unwrap_or(false);

    if !marks_off {
        let n = marks.floor() as i32 + 1;
        for i in 0..n {
            let x = i as f32 * mmstep;
            let _ = write!(
                svg,
                "<line x1=\"{x:.2}\" y1=\"0\" x2=\"{x:.2}\" y2=\"{gy:.2}\" \
                 stroke=\"{grid}\" stroke-width=\"0.5\" stroke-dasharray=\"1,3\"/>",
                grid = rgb(muted),
            );
        }
    }

    let _ = fg;
    // Head/foot text (centered over the wave area).
    let mid = xmax * XS * sx / 2.0;
    if let Some(t) = root.get("head").and_then(|h| h.get("text")).and_then(|v| v.as_str()) {
        let y = if cfg.yh0 > 0.0 { -33.0 } else { -13.0 };
        text(svg, t, mid, y, "middle", 16.0, "Liberation Sans", [0, 0, 0, 255], Some("bold"));
    }
    if let Some(t) = root.get("foot").and_then(|h| h.get("text")).and_then(|v| v.as_str()) {
        let y = gy + if cfg.yf0 > 0.0 { 45.0 } else { 25.0 };
        text(svg, t, mid, y, "middle", 16.0, "Liberation Sans", [0, 0, 0, 255], Some("bold"));
    }

    // Tick/tock numbers along head/foot.
    let cycles = marks.floor() as i32;
    ticktock(svg, root, "head", "tick", 0.0, mmstep, -5.0, cycles + 1, muted);
    ticktock(svg, root, "head", "tock", mmstep / 2.0, mmstep, -5.0, cycles, muted);
    ticktock(svg, root, "foot", "tick", 0.0, mmstep, gy + 15.0, cycles + 1, muted);
    ticktock(svg, root, "foot", "tock", mmstep / 2.0, mmstep, gy + 15.0, cycles, muted);
}

/// Port of `ticktock`: number the cycles. Supports numeric offset, a single
/// string offset, `[offset, step]`, or an explicit whitespace list.
#[allow(clippy::too_many_arguments)]
fn ticktock(
    svg: &mut String,
    root: &Value,
    anchor: &str,
    which: &str,
    x: f32,
    dx: f32,
    y: f32,
    len: i32,
    muted: [u8; 4],
) {
    if len <= 0 {
        return;
    }
    let val = match root.get(anchor).and_then(|h| h.get(which)) {
        Some(v) => v,
        None => return,
    };

    // Build the label list `L`.
    let labels: Vec<String> = if let Some(num) = val.as_f64() {
        (0..len).map(|i| format!("{}", i as f64 + num)).collect()
    } else if let Some(b) = val.as_bool() {
        let off = if b { 1.0 } else { 0.0 };
        (0..len).map(|i| format!("{}", i as f64 + off)).collect()
    } else if let Some(s) = val.as_str() {
        let toks: Vec<&str> = s.split_whitespace().collect();
        if toks.is_empty() {
            return;
        } else if toks.len() == 1 {
            match toks[0].parse::<f64>() {
                Ok(off) => (0..len).map(|i| format!("{}", i as f64 + off)).collect(),
                Err(_) => toks.iter().map(|t| t.to_string()).collect(),
            }
        } else if toks.len() == 2 {
            match (toks[0].parse::<f64>(), toks[1].parse::<f64>()) {
                (Ok(off0), Ok(step)) => {
                    let dp = toks[1].split('.').nth(1).map(|f| f.len()).unwrap_or(0);
                    let off = step * off0;
                    (0..len).map(|i| format!("{:.*}", dp, step * i as f64 + off)).collect()
                }
                _ => toks.iter().map(|t| t.to_string()).collect(),
            }
        } else {
            toks.iter().map(|t| t.to_string()).collect()
        }
    } else if let Some(arr) = val.as_array() {
        arr.iter().map(|v| v.as_str().map(|s| s.to_string()).unwrap_or_else(|| v.to_string())).collect()
    } else {
        return;
    };

    for i in 0..len {
        if let Some(label) = labels.get(i as usize) {
            let lx = i as f32 * dx + x;
            text(svg, label, lx, y, "middle", 11.0, "Liberation Sans", muted, None);
        }
    }
}

// ── Groups ───────────────────────────────────────────────────────────────────
fn render_groups(svg: &mut String, groups: &[Group], cfg: &Config, info: [u8; 4], family: &str) {
    let head_off = cfg.yh0 + cfg.yh1;
    for g in groups {
        let x = g.x + 0.5;
        let y0 = g.y as f32 * YO + 3.5 + head_off;
        let h = g.height as f32 * YO - 16.0;
        let _ = write!(
            svg,
            "<path d=\"m {x:.2},{y0:.2} c -3,0 -5,2 -5,5 l 0,{h:.2} c 0,3 2,5 5,5\" \
             fill=\"none\" stroke=\"#0041c4\" stroke-width=\"1\"/>",
        );
        if let Some(name) = &g.name {
            let lx = g.x - 10.0;
            let ly = YO * (g.y as f32 + g.height as f32 / 2.0) + head_off;
            let _ = write!(svg, "<g transform=\"translate({lx:.2},{ly:.2})\"><g transform=\"rotate(270)\">");
            text(svg, name, 0.0, 0.0, "middle", 11.0, family, info, None);
            svg.push_str("</g></g>");
        }
    }
}

// ── Arcs / edges ──────────────────────────────────────────────────────────────
#[derive(Clone, Copy)]
struct Pt {
    x: f32,
    y: f32,
}

fn render_arcs(
    svg: &mut String,
    root: &Value,
    nodes: &[NodeLane],
    hscale: usize,
    sx: f32,
    info: [u8; 4],
    family: &str,
) {
    use std::collections::HashMap;
    // Collect node-id → point.
    let mut events: HashMap<char, Pt> = HashMap::new();
    for (i, nl) in nodes.iter().enumerate() {
        let node = match &nl.node {
            Some(n) => n,
            None => continue,
        };
        let period = nl.period.max(1);
        let phase = nl.phase_bricks as f32;
        // `pos` advances per char incl. `.`, so it is not a plain enumerate.
        let mut pos = 0i64;
        #[allow(clippy::explicit_counter_loop)]
        for ch in node.chars() {
            if ch != '.' {
                // Node x lives in the same `sx`-narrowed brick space.
                let x = (XS * (2.0 * pos as f32 * period as f32 * hscale as f32 - phase) + XLABEL) * sx;
                let y = i as f32 * YO + Y0 + YS * 0.5;
                events.entry(ch).or_insert(Pt { x, y });
            }
            pos += 1;
        }
    }

    let edges = match root.get("edge").and_then(|v| v.as_array()) {
        Some(e) => e,
        None => {
            // Still draw the standalone lowercase node markers.
            draw_node_markers(svg, &events, info, family);
            return;
        }
    };

    for edge in edges {
        let s = match edge.as_str() {
            Some(s) => s,
            None => continue,
        };
        let words: Vec<&str> = s.split_whitespace().collect();
        if words.is_empty() {
            continue;
        }
        let token = words[0];
        let label = s[token.len()..].trim_start().to_string();
        let mut tchars = token.chars();
        let from_id = match tchars.next() {
            Some(c) => c,
            None => continue,
        };
        let to_id = match token.chars().last() {
            Some(c) => c,
            None => continue,
        };
        // shape = middle of token
        let shape: String = if token.len() >= 2 {
            token[from_id.len_utf8()..token.len() - to_id.len_utf8()].to_string()
        } else {
            String::new()
        };

        let (from, to) = match (events.get(&from_id), events.get(&to_id)) {
            (Some(f), Some(t)) => (*f, *t),
            _ => continue,
        };
        draw_arc(svg, &shape, from, to, &label, info, family);
    }

    draw_node_markers(svg, &events, info, family);
}

/// Standalone lowercase node ids get a small boxed label (render-arcs.js tail).
fn draw_node_markers(
    svg: &mut String,
    events: &std::collections::HashMap<char, Pt>,
    info: [u8; 4],
    family: &str,
) {
    let _ = info;
    for (k, p) in events {
        if k.is_lowercase() && p.x > 0.0 {
            render_label(svg, p.x, p.y, &k.to_string(), family);
        }
    }
}

/// Faithful port of `arcShape` (arc-shape.js) covering the full shape set:
/// `-  ~  -~  ~-  -|  |-  -|-  ->  ~>  -~>  ~->  -|>  |->  -|->  <->  <~>  <-~>
///  <-|>  <-|->  +`. Each shape decides its path `d` (or, when arc-shape.js
/// leaves `d` undefined — the straight/orthogonal arrowed forms — a straight
/// `M from to` fallback, per render-arcs.js), its label x (`lx`), the
/// arrowhead/arrowtail markers, and the stroke color. The `<…>`/`<-…>` forms
/// are double-ended (both `marker-start` and `marker-end`). Unknown → red line.
fn draw_arc(svg: &mut String, shape: &str, from: Pt, to: Pt, label: &str, info: [u8; 4], family: &str) {
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let mut lx = (from.x + to.x) / 2.0;
    let ly = (from.y + to.y) / 2.0;
    let has_label = !label.is_empty();

    // Straight-line fallback used when arc-shape.js leaves `d` undefined.
    let straight =
        || format!("M {x0:.2},{y0:.2} {x1:.2},{y1:.2}", x0 = from.x, y0 = from.y, x1 = to.x, y1 = to.y);
    // The three cubic-spline variants, parameterized by control-point biases.
    let spline = |c1x: f32, c2x: f32| {
        format!(
            "M {x0:.2},{y0:.2} c {c1:.2},0 {c2:.2},{dy:.2} {dx:.2},{dy:.2}",
            x0 = from.x,
            y0 = from.y,
            c1 = c1x,
            c2 = c2x,
        )
    };
    // Orthogonal "step" paths.
    let step_hv = || format!("m {x0:.2},{y0:.2} {dx:.2},0 0,{dy:.2}", x0 = from.x, y0 = from.y);
    let step_vh = || format!("m {x0:.2},{y0:.2} 0,{dy:.2} {dx:.2},0", x0 = from.x, y0 = from.y);
    let step_hvh =
        || format!("m {x0:.2},{y0:.2} {hx:.2},0 0,{dy:.2} {hx:.2},0", x0 = from.x, y0 = from.y, hx = dx / 2.0);

    // Defaults match arc-shape.js / render-arcs.js: blue, no markers.
    let mut stroke = "#0041c4";
    let mut arrow_end = false;
    let mut arrow_start = false;

    let d: String = match shape {
        "-" => straight(),
        "~" => spline(0.7 * dx, 0.3 * dx),
        "-~" => {
            if has_label {
                lx = from.x + dx * 0.75;
            }
            spline(0.7 * dx, dx)
        }
        "~-" => {
            if has_label {
                lx = from.x + dx * 0.25;
            }
            spline(0.0, 0.3 * dx)
        }
        "-|" => {
            if has_label {
                lx = to.x;
            }
            step_hv()
        }
        "|-" => {
            if has_label {
                lx = from.x;
            }
            step_vh()
        }
        "-|-" => step_hvh(),
        "->" => {
            arrow_end = true;
            straight()
        }
        "~>" => {
            arrow_end = true;
            spline(0.7 * dx, 0.3 * dx)
        }
        "-~>" => {
            arrow_end = true;
            if has_label {
                lx = from.x + dx * 0.75;
            }
            spline(0.7 * dx, dx)
        }
        "~->" => {
            arrow_end = true;
            if has_label {
                lx = from.x + dx * 0.25;
            }
            spline(0.0, 0.3 * dx)
        }
        "-|>" => {
            arrow_end = true;
            if has_label {
                lx = to.x;
            }
            step_hv()
        }
        "|->" => {
            arrow_end = true;
            if has_label {
                lx = from.x;
            }
            step_vh()
        }
        "-|->" => {
            arrow_end = true;
            step_hvh()
        }
        "<->" => {
            arrow_end = true;
            arrow_start = true;
            straight()
        }
        "<~>" => {
            arrow_end = true;
            arrow_start = true;
            spline(0.7 * dx, 0.3 * dx)
        }
        "<-~>" => {
            arrow_end = true;
            arrow_start = true;
            if has_label {
                lx = from.x + dx * 0.75;
            }
            spline(0.7 * dx, dx)
        }
        "<-|>" => {
            arrow_end = true;
            arrow_start = true;
            if has_label {
                lx = to.x;
            }
            step_hv()
        }
        "<-|->" => {
            arrow_end = true;
            arrow_start = true;
            step_hvh()
        }
        "+" => {
            // Blue straight line (arc-shape.js uses a tee-marked `#00F` style).
            stroke = "#0000ff";
            straight()
        }
        _ => {
            // Unknown shape → straight red (WaveDrom default).
            stroke = "#ff0000";
            straight()
        }
    };

    // Define the arrow markers once per document (idempotent: resvg dedups by id).
    if (arrow_end || arrow_start) && !svg.contains("id=\"arrowhead\"") {
        let _ = write!(
            svg,
            "<defs>\
             <marker id=\"arrowhead\" markerWidth=\"11\" markerHeight=\"8\" refX=\"9\" refY=\"4\" \
             orient=\"auto\"><path d=\"M0,0 11,4 0,8z\" fill=\"#0041c4\"/></marker>\
             <marker id=\"arrowtail\" markerWidth=\"11\" markerHeight=\"8\" refX=\"2\" refY=\"4\" \
             orient=\"auto\"><path d=\"M11,0 0,4 11,8z\" fill=\"#0041c4\"/></marker>\
             </defs>",
        );
    }

    let marker_end = if arrow_end { " marker-end=\"url(#arrowhead)\"" } else { "" };
    let marker_start = if arrow_start { " marker-start=\"url(#arrowtail)\"" } else { "" };

    let _ = write!(
        svg,
        "<path d=\"{d}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"1\"{marker_start}{marker_end}/>",
    );

    if has_label {
        render_label(svg, lx, ly, label, family);
    }
    let _ = info;
}

/// Port of `renderLabel`: a white box behind a small centered label.
fn render_label(svg: &mut String, x: f32, y: f32, label: &str, family: &str) {
    let fs = 11.0;
    let w = crate::font::line_width(label, fs) + 2.0;
    let _ = write!(
        svg,
        "<g transform=\"translate({x:.2},{y:.2})\">\
         <rect x=\"{rx:.2}\" y=\"{ry:.2}\" width=\"{w:.2}\" height=\"{fs}\" fill=\"#ffffff\"/>",
        rx = -(w / 2.0),
        ry = -(fs / 2.0),
    );
    text(svg, label, 0.0, 0.0, "middle", fs, family, [0, 0, 0, 255], None);
    svg.push_str("</g>");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Value {
        json5::from_str(src).expect("valid json5")
    }

    #[test]
    fn clock_renders() {
        let v = parse(r#"{signal:[{name:"clk",wave:"p..."}]}"#);
        let r = render(&v, &WaveDromOptions::default()).expect("ok");
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
        assert!(r.svg.contains(">clk<"), "name should appear: {}", &r.svg[..200.min(r.svg.len())]);
        assert!(r.svg.contains("<path"), "clock should draw paths");
        assert!(r.svg.starts_with("<svg") && r.svg.ends_with("</svg>"));
    }

    #[test]
    fn data_lane_labels_and_fills() {
        let v = parse(r#"{signal:[{name:"bus",wave:"x.34.5x",data:["a","b","c"]}]}"#);
        let r = render(&v, &WaveDromOptions::default()).expect("ok");
        for lbl in ["a", "b", "c"] {
            assert!(r.svg.contains(&format!(">{lbl}<")), "missing data label {lbl}");
        }
        // Palette color 3 → second palette entry (255,224,185).
        assert!(r.svg.contains("rgb(255,224,185)"), "expected a data fill color");
    }

    #[test]
    fn group_label_renders() {
        let v = parse(r#"{signal:["grp",{name:"a",wave:"01"}]}"#);
        let r = render(&v, &WaveDromOptions::default()).expect("ok");
        assert!(r.svg.contains(">grp<"), "group label should appear");
        assert!(r.svg.contains("rotate(270)"), "group label should be rotated");
        assert!(r.svg.contains("stroke=\"#0041c4\""), "group bracket should be drawn");
    }

    #[test]
    fn empty_is_error() {
        let v = parse(r#"{signal:[]}"#);
        assert_eq!(render(&v, &WaveDromOptions::default()), Err(WaveDromError::Empty));
    }

    #[test]
    fn spacer_only_is_empty_drawing_but_renders() {
        // A lone spacer lane is still a lane (not Empty), should not panic.
        let v = parse(r#"{signal:[{}]}"#);
        let r = render(&v, &WaveDromOptions::default()).expect("ok");
        assert!(r.height_px > 0.0);
    }

    #[test]
    fn phase_and_period_handled() {
        let v = parse(r#"{signal:[{name:"a",wave:"p..",period:2,phase:0.5}]}"#);
        let r = render(&v, &WaveDromOptions::default()).expect("ok");
        assert!(r.width_px > 0.0);
        // Wider period → wider canvas than period 1.
        let v1 = parse(r#"{signal:[{name:"a",wave:"p.."}]}"#);
        let r1 = render(&v1, &WaveDromOptions::default()).expect("ok");
        assert!(r.width_px > r1.width_px, "period should widen the diagram");
    }

    #[test]
    fn gap_and_edges_no_panic() {
        let v = parse(
            r#"{signal:[
                {name:"a",wave:"01.zx",node:".a..b"},
                {name:"b",wave:"0.1.0|"}
            ], edge:["a~b lbl","a-b"]}"#,
        );
        let r = render(&v, &WaveDromOptions::default()).expect("ok");
        assert!(r.svg.contains("lbl"), "edge label should appear");
    }

    #[test]
    fn head_foot_ticks_and_text() {
        let v = parse(
            r#"{signal:[{name:"clk",wave:"p..."}],
               head:{text:"Title",tick:0}, foot:{text:"Foot",tock:0}}"#,
        );
        let r = render(&v, &WaveDromOptions::default()).expect("ok");
        assert!(r.svg.contains(">Title<"));
        assert!(r.svg.contains(">Foot<"));
    }

    #[test]
    fn double_arrow_emits_two_markers() {
        // `<->` is double-ended: both marker-start (arrowtail) and marker-end.
        let v = parse(
            r#"{signal:[
                {name:"A",wave:"01.0",node:".a.b"},
                {name:"B",wave:"0.10",node:"..c."}
            ], edge:["a<->b two"]}"#,
        );
        let r = render(&v, &WaveDromOptions::default()).expect("ok");
        assert!(r.svg.contains("marker-end=\"url(#arrowhead)\""), "<-> needs an arrowhead");
        assert!(r.svg.contains("marker-start=\"url(#arrowtail)\""), "<-> needs an arrowtail");
        // The arrowtail marker must be defined in defs.
        assert!(r.svg.contains("id=\"arrowtail\""), "arrowtail marker must be defined");
        assert!(r.svg.contains(">two<"), "edge label should appear");
    }

    #[test]
    fn single_arrow_emits_one_marker() {
        // `->` is single-ended: marker-end only, no marker-start.
        let v = parse(
            r#"{signal:[
                {name:"A",wave:"01.0",node:".a.b"},
                {name:"B",wave:"0.10",node:"..c."}
            ], edge:["a->b one"]}"#,
        );
        let r = render(&v, &WaveDromOptions::default()).expect("ok");
        assert!(r.svg.contains("marker-end=\"url(#arrowhead)\""), "-> needs an arrowhead");
        assert!(!r.svg.contains("marker-start="), "-> must not have a start marker");
    }

    #[test]
    fn skin_narrow_is_narrower_than_default() {
        let dfl = parse(r#"{signal:[{name:"clk",wave:"p..."}]}"#);
        let nar = parse(r#"{signal:[{name:"clk",wave:"p..."}],config:{skin:"narrow"}}"#);
        let rd = render(&dfl, &WaveDromOptions::default()).expect("ok");
        let rn = render(&nar, &WaveDromOptions::default()).expect("ok");
        assert!(rn.width_px < rd.width_px, "narrow ({}) should be < default ({})", rn.width_px, rd.width_px);
    }

    #[test]
    fn skin_dark_changes_background() {
        let v = parse(r#"{signal:[{name:"clk",wave:"p..."}],config:{skin:"dark"}}"#);
        let r = render(&v, &WaveDromOptions::default()).expect("ok");
        // Dark skin overrides the default white background with a dark rect.
        assert!(r.svg.contains("rgb(40,40,40)"), "dark skin should set a dark background");
        assert!(!r.svg.contains("rgb(255,255,255)") || r.svg.contains("rgb(40,40,40)"));
        // Light foreground strokes.
        assert!(r.svg.contains("rgb(255,255,255)"), "dark skin should use light fg strokes");
    }

    #[test]
    fn skin_unknown_falls_back_to_default() {
        let dfl = parse(r#"{signal:[{name:"clk",wave:"p..."}]}"#);
        let unk = parse(r#"{signal:[{name:"clk",wave:"p..."}],config:{skin:"bogus"}}"#);
        let rd = render(&dfl, &WaveDromOptions::default()).expect("ok");
        let ru = render(&unk, &WaveDromOptions::default()).expect("ok");
        assert_eq!(rd.width_px, ru.width_px, "unknown skin should match default width");
    }
}
