//! Bitfield / register diagrams (`{ reg: [ … ] }` or a bare `[ … ]` array).
//!
//! Pure-Rust port of `references/wavedrom-bitfield/lib/render.js`. A register is
//! an array of field objects `{ bits: N, name?, attr?, type? }`. We lay the
//! fields out left-to-right as a strip of bit cells, with bit 0 on the *right*
//! (msb-left) by default, label the lsb/msb of each field along the top, center
//! the name in each field box, and stack `attr` lines underneath. `lanes > 1`
//! splits the register into stacked rows of `ceil(bits/lanes)` cells each.
//!
//! Geometry/defaults taken from render.js:
//!   - hspace default 800, lanes 1, fontsize 14 (`optDefaults`).
//!   - vspace default `(maxAttributes + 4) * fontsize` (so rows grow with attrs).
//!   - width  = hspace - margin.left - margin.right - 1.
//!   - height = vspace - margin.top - margin.bottom (the box height per lane).
//!   - margin: left/right 4, top 1.5*fontsize, bottom fontsize*maxAttributes + 4.
//!   - step = width / mod, mod = ceil(bits / lanes).
//!   - bit numbers sit at y = -(0.5*fontsize + 4) above the box; names at the
//!     box vertical center; attr lines below the box.
//!   - field fills use `fill-opacity:0.1` plus the `type` color when present.
//!
//! Implemented vs. the JS original:
//!   - `getLabel` binary expansion of *numeric* field names / attrs (one binary
//!     digit per bit cell, LSB→MSB), `compact` (suppress per-field bit numbers,
//!     stack lanes without inter-lane margins, draw boundary bit numbers once via
//!     `compactLabels`/`getLabelMask`, suppress typeless reserved rects), `legend`
//!     (colored squares + names), `hflip` (lane stacking direction), per-field
//!     `rotate`, and left/right diagram `label`. `vflip` is also supported.
//!   - Unnamed/typeless fields are drawn as a light hatched (diagonal) cell to
//!     read as "reserved", instead of WaveDrom's plain 0.1-opacity grey rect.
//!
//! Sparse / uneven support:
//!   - Uneven (fields of differing widths) renders proportionally from the cell
//!     step (`width / mod_`); this has always worked for contiguous registers.
//!   - Sparse: when declared fields don't cover the full span (an explicit
//!     `config.bits` total, or `lanes` forcing a wider register), the uncovered
//!     *trailing* bits are synthesized as anonymous typeless fields so they
//!     render as reserved/unused cells (hatched, with a boundary line and
//!     correct lsb/msb bit-index labels). Interior holes can't arise because
//!     WaveJSON fields tile from bit 0 with no positional gaps; an anonymous
//!     *declared* field still reserves its bits and already drew as reserved.

use std::fmt::Write as _;

use serde_json::Value;

use crate::svgutil::{escape, opacity_attr, rgb};
use crate::{WaveDromError, WaveDromOptions, WaveDromRender};

/// A field label: either a centered string, or a numeric value WaveDrom expands
/// to one binary digit per bit cell (`getLabel`, render.js line 70).
enum Label {
    /// String name — centered (with `…`-trimming) in the field box.
    Text(String),
    /// Numeric value — expanded to its `bits` binary digits, LSB→MSB.
    Num(i64),
}

/// A single parsed field of the register.
struct Field {
    bits: u32,
    /// Field name: `None` if absent, else string or numeric (binary-expanded).
    name: Option<Label>,
    /// Stacked attribute lines (each string or numeric, like `name`).
    attrs: Vec<Label>,
    /// `type` color index (WaveDrom 2..=7), if present.
    ftype: Option<i64>,
    /// Per-field `rotate` (degrees) applied to the name text, if present.
    rotate: Option<f32>,
    /// Absolute lsb / msb bit positions (filled after parse).
    lsb: u32,
    msb: u32,
}

/// Resolved layout options for a render pass (mirrors render.js `opt`).
struct Opt {
    hspace: f32,
    vspace: f32,
    lanes: u32,
    fontsize: f32,
    mod_: u32,
    vflip: bool,
    hflip: bool,
    compact: bool,
    margin_left: f32,
    margin_right: f32,
    margin_top: f32,
    margin_bottom: f32,
    offset: i64,
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    }
}

/// Read an option from top-level root then `root["config"]` (top-level wins),
/// rounded to an i64 if present.
fn opt_int(root: &Value, key: &str) -> Option<i64> {
    root.get(key)
        .or_else(|| root.get("config").and_then(|c| c.get(key)))
        .and_then(num)
        .map(|f| f.round() as i64)
}

fn opt_bool(root: &Value, key: &str) -> bool {
    root.get(key)
        .or_else(|| root.get("config").and_then(|c| c.get(key)))
        .map(|v| match v {
            Value::Bool(b) => *b,
            Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
            _ => false,
        })
        .unwrap_or(false)
}

fn opt_f32(root: &Value, key: &str) -> Option<f32> {
    root.get(key)
        .or_else(|| root.get("config").and_then(|c| c.get(key)))
        .and_then(num)
        .map(|f| f as f32)
}

/// Turn one JSON scalar into a `Label`: numbers become `Num` (binary-expanded
/// per render.js `getLabel`), everything else a centered string.
fn scalar_label(v: &Value) -> Option<Label> {
    match v {
        Value::Number(n) => n.as_f64().map(|f| Label::Num(f.round() as i64)),
        Value::String(s) => Some(Label::Text(s.clone())),
        Value::Bool(b) => Some(Label::Text(b.to_string())),
        _ => None,
    }
}

/// Flatten an `attr` value (string / number / array of those) to display lines.
/// `null`/empty array slots are kept as empty `Text` so line indices line up
/// with render.js (it preserves slot index, skipping null/undefined at draw).
fn attr_lines(v: &Value) -> Vec<Label> {
    match v {
        Value::Array(arr) => arr
            .iter()
            .map(|a| scalar_label(a).unwrap_or(Label::Text(String::new())))
            .collect(),
        Value::Null => vec![],
        other => scalar_label(other).into_iter().collect(),
    }
}

fn parse_fields(reg: &Value) -> Vec<Field> {
    let arr = match reg.as_array() {
        Some(a) => a,
        None => return vec![],
    };
    let mut lsb = 0u32;
    let mut out = Vec::with_capacity(arr.len());
    for e in arr {
        let bits = e.get("bits").and_then(num).map(|f| f.round() as i64).unwrap_or(0);
        let bits = if bits < 0 { 0 } else { bits as u32 };
        if bits == 0 {
            continue;
        }
        let name = e.get("name").and_then(scalar_label);
        let attrs = e.get("attr").map(attr_lines).unwrap_or_default();
        let ftype = e.get("type").and_then(num).map(|f| f.round() as i64);
        let rotate = e.get("rotate").and_then(num).map(|f| f as f32);
        let msb = lsb + bits - 1;
        out.push(Field { bits, name, attrs, ftype, rotate, lsb, msb });
        lsb += bits;
    }
    out
}

/// WaveDrom's `type → color` table (lib/render.js `colors`). Returns straight
/// RGBA; falls back to the palette if a host palette is preferred.
fn type_color(ftype: i64, opts: &WaveDromOptions) -> Option<[u8; 4]> {
    // Match render.js intent: map the small fixed set, but prefer the host
    // palette so the diagram tracks the app theme. type N → palette[(N-2)%len]
    // (WaveDrom's first colored type is 2).
    if ftype < 2 {
        // type 0/1 are uncolored in WaveDrom's table.
        return None;
    }
    let pal = &opts.series_palette;
    if pal.is_empty() {
        // Fallback to render.js's literal colors.
        let c = match ftype {
            2 => [0xff, 0x00, 0x00, 0xff],
            3 => [0xaa, 0xff, 0x00, 0xff],
            4 => [0x00, 0xff, 0xd5, 0xff],
            5 => [0xff, 0xbf, 0x00, 0xff],
            6 => [0x00, 0xff, 0x19, 0xff],
            7 => [0x00, 0x6a, 0xff, 0xff],
            _ => return None,
        };
        return Some(c);
    }
    let idx = ((ftype - 2).rem_euclid(pal.len() as i64)) as usize;
    Some(pal[idx])
}

pub fn render(
    reg: &Value,
    root: &Value,
    opts: &WaveDromOptions,
) -> Result<WaveDromRender, WaveDromError> {
    let mut fields = parse_fields(reg);
    if fields.is_empty() {
        return Err(WaveDromError::Empty);
    }

    // ---- resolve options (render.js optDefaults + render head) -------------
    let fontsize = opt_f32(root, "fontsize").filter(|f| *f >= 6.0).unwrap_or(opts.font_size_px);
    let hspace = opt_int(root, "hspace").filter(|h| *h >= 40).map(|h| h as f32).unwrap_or(800.0);
    let lanes = opt_int(root, "lanes").filter(|l| *l >= 1).map(|l| l as u32).unwrap_or(1);
    let vflip = opt_bool(root, "vflip");
    let hflip = opt_bool(root, "hflip");
    let compact = opt_bool(root, "compact");
    let offset = opt_int(root, "offset").unwrap_or(0);

    // legend: a `{name: typeIndex}` map drawn as colored squares + names above
    // the register (render.js `getLegendItems`, opt.legend).
    let legend: Vec<(String, i64)> = root
        .get("legend")
        .or_else(|| root.get("config").and_then(|c| c.get("legend")))
        .and_then(|v| v.as_object())
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| num(v).map(|n| (k.clone(), n.round() as i64)))
                .collect()
        })
        .unwrap_or_default();
    let has_legend = !legend.is_empty();

    // diagram label: `{left?, right?}` text drawn at the side of each lane
    // (render.js `lane`). Each side may be string / number / per-lane map.
    let label = root
        .get("label")
        .or_else(|| root.get("config").and_then(|c| c.get("label")))
        .cloned();
    let has_left_label = label.as_ref().and_then(|l| l.get("left")).is_some();
    let has_right_label = label.as_ref().and_then(|l| l.get("right")).is_some();

    let total_bits: u32 = fields.iter().map(|f| f.bits).sum();
    // Total bit span. WaveDrom tiles fields contiguously from bit 0; the span is
    // normally their sum, but `config.bits` (an explicit total) and `lanes` can
    // force a wider register. `bits` is the larger of: declared coverage, an
    // explicit `bits` override, and `lanes` rounded up to a whole multiple (so a
    // single declared field still fills out each lane).
    let explicit_bits = opt_int(root, "bits").filter(|b| *b >= 1).map(|b| b as u32);
    let bits = explicit_bits.unwrap_or(total_bits).max(total_bits).max(lanes);

    // ---- sparse: synthesize "unused" fields for any uncovered tail bits ------
    // When the declared fields don't reach `bits` (e.g. `config.bits` is wider
    // than their sum, or `lanes` forces a wider span), WaveDrom leaves the
    // trailing bit range blank. We materialize that range as anonymous,
    // typeless field(s) so they pick up the existing reserved/unused rendering
    // (hatched fill, a field boundary line, and lsb/msb bit-index labels).
    // Interior holes can't occur because fields tile from bit 0 with no
    // positional gaps in the WaveJSON; only trailing bits go uncovered.
    if total_bits < bits {
        // Split the tail at lane boundaries so each synthesized unused field
        // stays within a single lane (matching how real fields are sliced per
        // lane during draw, and keeping per-lane boundary lines/labels correct).
        let lane_w = ((bits as f32) / (lanes as f32)).ceil().max(1.0) as u32;
        let mut lsb = total_bits;
        while lsb < bits {
            // Next lane boundary above `lsb` (exclusive), clamped to `bits`.
            let next_boundary = ((lsb / lane_w) + 1) * lane_w;
            let end = next_boundary.min(bits); // exclusive
            let span = end - lsb;
            fields.push(Field {
                bits: span,
                name: None,
                attrs: vec![],
                ftype: None,
                rotate: None,
                lsb,
                msb: lsb + span - 1,
            });
            lsb = end;
        }
    }

    let max_attrs = fields.iter().map(|f| f.attrs.len()).max().unwrap_or(0) as f32;

    // vspace default: (maxAttributes + 4) * fontsize (render.js line 429).
    let vspace = opt_f32(root, "vspace").filter(|v| *v > 0.0).unwrap_or((max_attrs + 4.0) * fontsize);

    // margins (render.js render() head). Left/right grow to 0.1*hspace when a
    // side label is present so it has room.
    let margin_right = opt_f32(root, "marginRight")
        .unwrap_or(if has_right_label { (0.1 * hspace).round() } else { 4.0 });
    let margin_left = opt_f32(root, "marginLeft")
        .unwrap_or(if has_left_label { (0.1 * hspace).round() } else { 4.0 });
    let margin_top = opt_f32(root, "marginTop").unwrap_or(1.5 * fontsize);
    let margin_bottom = opt_f32(root, "marginBottom").unwrap_or(fontsize * max_attrs + 4.0);

    // mod = ceil(bits / lanes) — cells per lane.
    let mod_ = ((bits as f32) / (lanes as f32)).ceil().max(1.0) as u32;

    let opt = Opt {
        hspace,
        vspace,
        lanes,
        fontsize,
        mod_,
        vflip,
        hflip,
        compact,
        margin_left,
        margin_right,
        margin_top,
        margin_bottom,
        offset,
    };

    // Re-derive absolute lsb/msb (parse already did, but `bits` override could
    // change nothing here; positions are intrinsic to the field order).
    {
        let mut lsb = 0u32;
        for f in &mut fields {
            f.lsb = lsb;
            f.msb = lsb + f.bits - 1;
            lsb += f.bits;
        }
    }

    let width = opt.hspace; // SVG canvas width.
    let mut height = opt.vspace * opt.lanes as f32;
    // compact stacks lanes by box height only (no inter-lane top/bottom margins).
    if opt.compact {
        height -= (opt.lanes as f32 - 1.0) * (opt.margin_top + opt.margin_bottom);
    }
    // legend row reserves 12px at the top (render.js render() line 469-471).
    if has_legend {
        height += 12.0;
    }

    // ---- build the SVG -----------------------------------------------------
    let fg = opts.foreground;
    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.0}\" height=\"{h:.0}\" \
         viewBox=\"0 0 {w:.0} {h:.0}\">",
        w = width.max(1.0),
        h = height.max(1.0),
    );
    // Full-bleed background.
    if opts.background[3] > 0 {
        let _ = write!(
            svg,
            "<rect x=\"0\" y=\"0\" width=\"{w:.0}\" height=\"{h:.0}\" fill=\"{bg}\"{bo}/>",
            w = width.max(1.0),
            h = height.max(1.0),
            bg = rgb(opts.background),
            bo = opacity_attr("fill-opacity", opts.background),
        );
    }
    // A hatch pattern for unnamed/reserved fields.
    let _ = write!(
        svg,
        "<defs><pattern id=\"bf-hatch\" width=\"6\" height=\"6\" patternUnits=\"userSpaceOnUse\" \
         patternTransform=\"rotate(45)\"><line x1=\"0\" y1=\"0\" x2=\"0\" y2=\"6\" \
         stroke=\"{fg}\" stroke-width=\"1\" stroke-opacity=\"0.25\"/></pattern></defs>",
        fg = rgb(fg),
    );

    // Content group: shifted down by 12 to clear the legend row (render.js
    // translates the whole register `(0.5, legend?12.5:0.5)`).
    let content_dy = if has_legend { 12.0 } else { 0.0 };
    let _ = write!(svg, "<g transform=\"translate(0,{content_dy:.2})\">");
    for index in 0..opt.lanes {
        draw_lane(&mut svg, &fields, &opt, index, opts, label.as_ref());
    }
    // Compact mode draws the boundary bit numbers once, across the whole strip.
    if opt.compact {
        draw_compact_labels(&mut svg, &fields, &opt, has_legend, opts);
    }
    svg.push_str("</g>");

    // Legend row (colored squares + names), at the top.
    if has_legend {
        draw_legend(&mut svg, &legend, &opt, opts);
    }

    svg.push_str("</svg>");

    Ok(WaveDromRender { svg, width_px: width.max(1.0), height_px: height.max(1.0) })
}

/// Map a cell column `xm` (0..mod, measured from lsb) to its left x in px,
/// honouring `vflip` (bit-0 at left when vflip, else at right).
fn cell_x(opt: &Opt, lsbm: u32, width: f32) -> f32 {
    let step = width / opt.mod_ as f32;
    if opt.vflip {
        step * lsbm as f32
    } else {
        step * (opt.mod_ - lsbm - 1) as f32
    }
}

fn draw_lane(
    svg: &mut String,
    fields: &[Field],
    opt: &Opt,
    index: u32,
    opts: &WaveDromOptions,
    label: Option<&Value>,
) {
    let width = opt.hspace - opt.margin_left - opt.margin_right - 1.0;
    let height = opt.vspace - opt.margin_top - opt.margin_bottom;
    let step = width / opt.mod_ as f32;

    // Lane stacking: !hflip puts lane 0 (low bits) at the bottom; hflip flips so
    // lane 0 is at the top (render.js `lane`: idx = hflip ? index : lanes-index-1).
    let idx = if opt.hflip { index } else { opt.lanes - index - 1 } as f32;
    let tx = opt.margin_left;
    // compact stacks by box height (no inter-lane margins).
    let ty = if opt.compact {
        (idx * height + opt.margin_top).round()
    } else {
        (idx * opt.vspace + opt.margin_top).round()
    };

    let fg = opts.foreground;
    let fam = escape(&opts.font_family);
    let fs = opt.fontsize;

    let _ = write!(svg, "<g transform=\"translate({tx:.2},{ty:.2})\">");

    // The bit range this lane covers.
    let lane_lo = index * opt.mod_;
    let lane_hi = lane_lo + opt.mod_ - 1;

    // ---- field fill rects (drawn first, under the cage) --------------------
    for e in fields {
        // Does this field intersect this lane?
        if e.msb < lane_lo || e.lsb > lane_hi {
            continue;
        }
        let lsbm = e.lsb.max(lane_lo) - lane_lo;
        let msbm = e.msb.min(lane_hi) - lane_lo;
        let span = msbm - lsbm + 1;
        // Left edge: high-bit cell when !vflip (msb on the left), else lsb cell.
        let x = if opt.vflip { cell_x(opt, lsbm, width) } else { cell_x(opt, msbm, width) };
        let w = step * span as f32;

        let unnamed = e.name.is_none() && e.ftype.is_none();
        // render.js: in compact mode, typeless reserved fields draw no rect.
        if unnamed && opt.compact {
            continue;
        }
        let fill = e.ftype.and_then(|t| type_color(t, opts));
        if unnamed {
            // Reserved / unnamed field → hatch. (Simplification: WaveDrom uses
            // a plain 0.1-opacity grey rect; a hatch reads more clearly.)
            let _ = write!(
                svg,
                "<rect x=\"{x:.2}\" y=\"0\" width=\"{w:.2}\" height=\"{h:.2}\" \
                 fill=\"url(#bf-hatch)\"/>",
                h = height,
            );
        } else if let Some(c) = fill {
            let _ = write!(
                svg,
                "<rect x=\"{x:.2}\" y=\"0\" width=\"{w:.2}\" height=\"{h:.2}\" \
                 fill=\"{c}\" fill-opacity=\"0.4\"/>",
                h = height,
                c = rgb(c),
            );
        }
    }

    // ---- cage: outer box + per-bit / per-field vertical ticks --------------
    let _ = write!(
        svg,
        "<g fill=\"none\" stroke=\"{fg}\" stroke-width=\"1\" stroke-linecap=\"round\">",
        fg = rgb(fg),
    );
    // outer rectangle
    let _ = write!(
        svg,
        "<rect x=\"0\" y=\"0\" width=\"{width:.2}\" height=\"{height:.2}\"/>",
    );
    // vertical lines between cells: full height at field boundaries, short ticks
    // mid-cell otherwise (render.js cage `vline(height>>>3)`).
    let short = (height / 8.0).round();
    for k in 0..opt.mod_ {
        let global = lane_lo + k; // bit index within register (0-based from lsb)
        // x position of the *left* boundary of cell k (in visual order):
        // visual column j: when !vflip the high bit (mod-1) is at left.
        let xj = if opt.vflip {
            step * k as f32
        } else {
            step * (opt.mod_ - k) as f32
        };
        // Is `global` the lsb of some field (a field boundary)?
        let boundary = k == 0 || fields.iter().any(|e| e.lsb == global);
        if boundary {
            let _ = write!(svg, "<line x1=\"{xj:.2}\" y1=\"0\" x2=\"{xj:.2}\" y2=\"{height:.2}\"/>");
        } else {
            // short ticks top and bottom
            let _ = write!(svg, "<line x1=\"{xj:.2}\" y1=\"0\" x2=\"{xj:.2}\" y2=\"{short:.2}\"/>");
            let _ = write!(
                svg,
                "<line x1=\"{xj:.2}\" y1=\"{ya:.2}\" x2=\"{xj:.2}\" y2=\"{height:.2}\"/>",
                ya = height - short,
            );
        }
    }
    svg.push_str("</g>"); // end cage stroke group

    // ---- bit-index numbers along the top -----------------------------------
    // render.js places these at y = -(0.5*fontsize + 4) above the box, centered
    // on the lsb/msb cell. Suppressed in compact mode (drawn once globally by
    // `draw_compact_labels`).
    let bit_y = -(0.5 * fs + 4.0);
    for e in fields {
        if opt.compact {
            break;
        }
        if e.msb < lane_lo || e.lsb > lane_hi {
            continue;
        }
        let lsbm = e.lsb.max(lane_lo) - lane_lo;
        let msbm = e.msb.min(lane_hi) - lane_lo;
        // lsb label
        let lsb_label = e.lsb as i64 + opt.offset;
        let xc = cell_x(opt, lsbm, width) + step / 2.0;
        emit_text(svg, &lsb_label.to_string(), xc, bit_y, "middle", fs, &fam, fg);
        if lsbm != msbm {
            let msb_label = e.msb as i64 + opt.offset;
            let xc = cell_x(opt, msbm, width) + step / 2.0;
            emit_text(svg, &msb_label.to_string(), xc, bit_y, "middle", fs, &fam, fg);
        }
    }

    // ---- field names (centered in the box) ---------------------------------
    let name_y = height / 2.0;
    for e in fields {
        if e.msb < lane_lo || e.lsb > lane_hi {
            continue;
        }
        let name = match &e.name {
            Some(l) => l,
            None => continue,
        };
        let lsbm = e.lsb.max(lane_lo) - lane_lo;
        let msbm = e.msb.min(lane_hi) - lane_lo;
        let center = (lsbm + msbm) as f32 / 2.0;
        let xc = if opt.vflip {
            step * (center + 0.5)
        } else {
            step * (opt.mod_ as f32 - center - 0.5)
        };
        // A field can be clipped by a lane boundary; expand binary across the
        // bits visible in *this* lane.
        let vis = msbm - lsbm + 1;
        draw_label(svg, name, xc, name_y, step, vis, e.rotate, fs, &fam, fg);
    }

    // ---- attribute lines below the box -------------------------------------
    for e in fields {
        if e.msb < lane_lo || e.lsb > lane_hi || e.attrs.is_empty() {
            continue;
        }
        let lsbm = e.lsb.max(lane_lo) - lane_lo;
        let msbm = e.msb.min(lane_hi) - lane_lo;
        let center = (lsbm + msbm) as f32 / 2.0;
        let xc = if opt.vflip {
            step * (center + 0.5)
        } else {
            step * (opt.mod_ as f32 - center - 0.5)
        };
        // render.js attrs group baseline: height + 0.7*fontsize - 2, then each
        // line stepped by fontsize. Numeric attrs are binary-expanded too.
        let base_y = height + 0.7 * fs - 2.0 + fs * 0.5;
        let vis = msbm - lsbm + 1;
        for (i, a) in e.attrs.iter().enumerate() {
            let y = base_y + fs * i as f32;
            draw_label(svg, a, xc, y, step, vis, None, fs, &fam, fg);
        }
    }

    // ---- left / right diagram label ----------------------------------------
    // render.js `lane`: text anchored at the lane's vertical center, just
    // outside the box. `lab` is a string (used as-is), number (index+lab), or a
    // per-lane map keyed by lane index.
    if let Some(label) = label {
        let bit_lane = index; // render.js passes `index` (0 = low-bit lane)
        if let Some(side) = label.get("left") {
            let txt = side_label_text(side, bit_lane);
            emit_text(svg, &txt, -4.0, height / 2.0, "end", fs, &fam, fg);
        }
        if let Some(side) = label.get("right") {
            let txt = side_label_text(side, bit_lane);
            emit_text(svg, &txt, width + 4.0, height / 2.0, "start", fs, &fam, fg);
        }
    }

    svg.push_str("</g>"); // end lane translate group
}

/// Resolve a `label.left`/`label.right` value for lane `index` (render.js
/// `lane`): string → as-is, number → `index + n`, object → map[index] or index.
fn side_label_text(side: &Value, index: u32) -> String {
    match side {
        Value::String(s) => s.clone(),
        Value::Number(n) => (index as i64 + n.as_f64().unwrap_or(0.0).round() as i64).to_string(),
        Value::Object(_) | Value::Array(_) => side
            .get(index as usize)
            .or_else(|| side.get(index.to_string()))
            .and_then(|v| match v {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .unwrap_or_else(|| index.to_string()),
        _ => index.to_string(),
    }
}

/// Draw a field name / attr label at center `xc`, baseline `y`. Strings are
/// `…`-trimmed and centered; numbers are expanded to `bits` binary digits
/// (`getLabel`, render.js line 70). `rotate` (deg) is applied as a per-glyph
/// transform when present.
#[allow(clippy::too_many_arguments)]
fn draw_label(
    svg: &mut String,
    label: &Label,
    xc: f32,
    y: f32,
    step: f32,
    bits: u32,
    rotate: Option<f32>,
    fs: f32,
    fam: &str,
    fg: [u8; 4],
) {
    match label {
        Label::Text(s) => {
            if s.is_empty() {
                return;
            }
            let avail = step * bits as f32 - 4.0;
            let shown = trim_text(s, avail, fs);
            emit_text_rot(svg, &shown, xc, y, "middle", fs, fam, fg, rotate);
        }
        Label::Num(v) => {
            // One binary digit per bit cell, LSB at i=0. render.js positions
            // digit i at center + step*(bits/2 - i - 0.5).
            let len = bits as i32;
            for i in 0..len {
                let bit = ((*v >> i) & 1).to_string();
                let dx = xc + step * (len as f32 / 2.0 - i as f32 - 0.5);
                emit_text_rot(svg, &bit, dx, y, "middle", fs, fam, fg, rotate);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_text(
    svg: &mut String,
    s: &str,
    x: f32,
    y: f32,
    anchor: &str,
    fs: f32,
    fam: &str,
    fill: [u8; 4],
) {
    emit_text_rot(svg, s, x, y, anchor, fs, fam, fill, None);
}

/// Like `emit_text`, but applies a `rotate(<deg>)` transform about the text
/// anchor when `rotate` is `Some` (render.js per-field `e.rotate`).
#[allow(clippy::too_many_arguments)]
fn emit_text_rot(
    svg: &mut String,
    s: &str,
    x: f32,
    y: f32,
    anchor: &str,
    fs: f32,
    fam: &str,
    fill: [u8; 4],
    rotate: Option<f32>,
) {
    if s.is_empty() {
        return;
    }
    match rotate {
        Some(deg) => {
            // Wrap in a group translated to (x,y) and rotate the text about it,
            // matching render.js `text(body,x,y,rotate)` (translate + rotate).
            let _ = write!(
                svg,
                "<g transform=\"translate({x:.2},{y:.2})\">\
                 <text x=\"0\" y=\"0\" text-anchor=\"{anchor}\" \
                 dominant-baseline=\"central\" font-family=\"{fam}\" font-size=\"{fs}\" \
                 fill=\"{fill}\"{fo} transform=\"rotate({deg})\">{txt}</text></g>",
                fill = rgb(fill),
                fo = opacity_attr("fill-opacity", fill),
                txt = escape(s),
            );
        }
        None => {
            let _ = write!(
                svg,
                "<text x=\"{x:.2}\" y=\"{y:.2}\" text-anchor=\"{anchor}\" \
                 dominant-baseline=\"central\" font-family=\"{fam}\" font-size=\"{fs}\" \
                 fill=\"{fill}\"{fo}>{txt}</text>",
                fill = rgb(fill),
                fo = opacity_attr("fill-opacity", fill),
                txt = escape(s),
            );
        }
    }
}

/// Compact bit numbers: only the boundary bits of each field, drawn once across
/// the whole strip (render.js `compactLabels` + `getLabelMask`). Each field
/// contributes its first and last bit position (mod `mod_`); we draw those
/// columns' indices along the top.
fn draw_compact_labels(
    svg: &mut String,
    fields: &[Field],
    opt: &Opt,
    has_legend: bool,
    opts: &WaveDromOptions,
) {
    let width = opt.hspace - opt.margin_left - opt.margin_right - 1.0;
    let step = width / opt.mod_ as f32;
    let fs = opt.fontsize;
    let fam = escape(&opts.font_family);
    let fg = opts.foreground;

    // getLabelMask: mark `idx % mod` and `(idx+bits-1) % mod` for each field.
    let mut mask = vec![false; opt.mod_ as usize];
    let mut idx = 0u32;
    for e in fields {
        mask[(idx % opt.mod_) as usize] = true;
        idx += e.bits;
        mask[((idx - 1) % opt.mod_) as usize] = true;
    }

    // Group translate: margin.left, y = legend ? 0 : -3 (render.js).
    let gy = if has_legend { 0.0 } else { -3.0 };
    let _ = write!(svg, "<g transform=\"translate({:.2},{:.2})\">", opt.margin_left, gy);
    for i in 0..opt.mod_ {
        let col = if opt.vflip { i } else { opt.mod_ - i - 1 };
        if mask[col as usize] {
            let label = col as i64 + opt.offset;
            let x = step * (i as f32 + 0.5);
            let y = 0.5 * fs + 4.0;
            emit_text(svg, &label.to_string(), x, y, "middle", fs, &fam, fg);
        }
    }
    svg.push_str("</g>");
}

/// Legend row: colored squares + names, centered above the register
/// (render.js `getLegendItems`, lines 182-208). `legend` is `[(name, type)]`.
fn draw_legend(
    svg: &mut String,
    legend: &[(String, i64)],
    opt: &Opt,
    opts: &WaveDromOptions,
) {
    let width = opt.hspace - opt.margin_left - opt.margin_right - 1.0;
    let fs = opt.fontsize;
    let fam = escape(&opts.font_family);
    let fg = opts.foreground;
    let square_pad = 36.0;
    let name_pad = 24.0;

    // Group translate: (margin.left, -10) inside an outer that we place at the
    // top; here we anchor at the legend band (y ~ 12 reserved by render()).
    let _ = write!(svg, "<g transform=\"translate({:.2},{:.2})\">", opt.margin_left, 2.0);
    let mut x = width / 2.0 - (legend.len() as f32) / 2.0 * (square_pad + name_pad);
    for (name, ftype) in legend {
        let fill = type_color(*ftype, opts);
        let (fillc, fo) = match fill {
            // A legend swatch is a color *key*, so fill it strongly (vs. the
            // faint 0.1–0.4 tint used behind a wide field box) to stay legible.
            Some(c) => (rgb(c), "fill-opacity=\"0.85\""),
            None => ("none".to_string(), "fill-opacity=\"0\""),
        };
        let _ = write!(
            svg,
            "<rect x=\"{x:.2}\" y=\"0\" width=\"12\" height=\"12\" \
             fill=\"{fillc}\" {fo} stroke=\"{stroke}\" stroke-width=\"1.2\"/>",
            stroke = rgb(fg),
        );
        x += square_pad;
        // name baseline: 0.1*fontsize + 4 (render.js); +6 for the text() y-offset.
        emit_text(svg, name, x, 0.1 * fs + 4.0 + 6.0, "middle", fs, &fam, fg);
        x += name_pad;
    }
    svg.push_str("</g>");
}

/// Trim a string to fit `avail` px at `fontsize`, appending `…` (render.js
/// `trimText`, but using real glyph metrics).
fn trim_text(text: &str, avail: f32, fontsize: f32) -> String {
    let w = crate::font::line_width(text, fontsize);
    if w <= avail || avail <= 0.0 {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let ell = "…";
    let ell_w = crate::font::line_width(ell, fontsize);
    let mut end = chars.len();
    while end > 0 {
        let sub: String = chars[..end].iter().collect();
        if crate::font::line_width(&sub, fontsize) + ell_w <= avail {
            return format!("{sub}{ell}");
        }
        end -= 1;
    }
    ell.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_src(src: &str) -> Result<WaveDromRender, WaveDromError> {
        let val: Value = json5::from_str(src).expect("json5");
        let reg = val.get("reg").cloned().unwrap_or(val.clone());
        render(&reg, &val, &WaveDromOptions::default())
    }

    #[test]
    fn basic_register_renders() {
        let r = render_src(r#"{reg:[{bits:7,name:"opcode"},{bits:5,name:"rd"}]}"#).unwrap();
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
        assert!(r.svg.contains("opcode"), "name present");
        assert!(r.svg.contains("rd"), "name present");
        assert!(r.svg.contains("<rect"), "has rects");
        // bit numbers: lsb 0 and msb 11 should appear as text.
        assert!(r.svg.contains(">0</text>"), "bit 0 label: {}", &r.svg[..200]);
        assert!(r.svg.contains(">11</text>"), "bit 11 label");
        assert!(r.svg.starts_with("<svg"));
        assert!(r.svg.ends_with("</svg>"));
    }

    #[test]
    fn empty_reg_errs() {
        let r = render(&Value::Array(vec![]), &Value::Array(vec![]), &WaveDromOptions::default());
        assert_eq!(r, Err(WaveDromError::Empty));
    }

    #[test]
    fn zero_bit_fields_are_empty() {
        let r = render_src(r#"{reg:[{bits:0,name:"x"}]}"#);
        assert_eq!(r, Err(WaveDromError::Empty));
    }

    #[test]
    fn lanes_makes_taller() {
        let src = r#"{reg:[{bits:8,name:"a"},{bits:8,name:"b"},{bits:8,name:"c"},{bits:8,name:"d"}]"#;
        let one = render_src(&format!("{src}, lanes:1}}")).unwrap();
        let two = render_src(&format!("{src}, lanes:2}}")).unwrap();
        assert!(two.height_px > one.height_px, "lanes:2 ({}) taller than lanes:1 ({})", two.height_px, one.height_px);
    }

    #[test]
    fn type_fill_present() {
        let r = render_src(r#"{reg:[{bits:8,name:"f",type:2}]}"#).unwrap();
        assert!(r.svg.contains("fill-opacity"), "typed field filled");
    }

    #[test]
    fn unnamed_field_hatched() {
        let r = render_src(r#"{reg:[{bits:4,name:"a"},{bits:4}]}"#).unwrap();
        assert!(r.svg.contains("bf-hatch"), "reserved field hatched");
    }

    #[test]
    fn attr_lines_render() {
        let r = render_src(r#"{reg:[{bits:8,name:"a",attr:["lo","hi"]}]}"#).unwrap();
        assert!(r.svg.contains(">lo</text>"));
        assert!(r.svg.contains(">hi</text>"));
    }

    #[test]
    fn numeric_name_expands_to_binary() {
        // name 5 over 4 bits → digits 0,1,0,1 (one per cell), no centered "5".
        let r = render_src(r#"{reg:[{bits:4,name:5}]}"#).unwrap();
        let zeros = r.svg.matches(">0</text>").count();
        let ones = r.svg.matches(">1</text>").count();
        assert!(zeros >= 2, "two binary 0 digits, got {zeros}: {}", r.svg);
        assert!(ones >= 2, "two binary 1 digits, got {ones}");
        assert!(!r.svg.contains(">5</text>"), "numeric name not centered as 5");
        // String names still centered.
        let r2 = render_src(r#"{reg:[{bits:4,name:"op"}]}"#).unwrap();
        assert!(r2.svg.contains(">op</text>"));
    }

    #[test]
    fn compact_is_not_taller_and_differs() {
        let src = r#"{reg:[{bits:7,name:"opcode"},{bits:5,name:"rd"},{bits:20,name:"imm"}]"#;
        let plain = render_src(&format!("{src}}}")).unwrap();
        let compact = render_src(&format!("{src}, config:{{compact:true}}}}")).unwrap();
        assert_ne!(plain.svg, compact.svg, "compact layout differs");
        // Single lane: compact must not be taller than plain.
        assert!(
            compact.height_px <= plain.height_px,
            "compact ({}) not taller than plain ({})",
            compact.height_px,
            plain.height_px
        );
    }

    #[test]
    fn compact_multilane_shorter_per_lane() {
        // With lanes>1, compact removes inter-lane margins → shorter overall.
        let src = r#"{reg:[{bits:8,name:"a"},{bits:8,name:"b"},{bits:8,name:"c"},{bits:8,name:"d"}], lanes:2"#;
        let plain = render_src(&format!("{src}}}")).unwrap();
        let compact = render_src(&format!("{src}, compact:true}}")).unwrap();
        assert!(
            compact.height_px < plain.height_px,
            "compact 2-lane ({}) shorter than plain ({})",
            compact.height_px,
            plain.height_px
        );
    }

    #[test]
    fn legend_emits_squares_and_names() {
        let r = render_src(
            r#"{reg:[{bits:8,name:"x",type:2},{bits:8,name:"y",type:3}], legend:{rd:2,rs:3}}"#,
        )
        .unwrap();
        assert!(r.svg.contains(">rd</text>"), "legend name rd");
        assert!(r.svg.contains(">rs</text>"), "legend name rs");
        assert!(r.svg.contains("width=\"12\" height=\"12\""), "legend squares");
        // Legend reserves vertical band → taller than without.
        let plain =
            render_src(r#"{reg:[{bits:8,name:"x",type:2},{bits:8,name:"y",type:3}]}"#).unwrap();
        assert!(r.height_px > plain.height_px, "legend adds height");
    }

    #[test]
    fn hflip_differs_from_default() {
        let src = r#"{reg:[{bits:8,name:"a"},{bits:8,name:"b"},{bits:8,name:"c"},{bits:8,name:"d"}], lanes:2"#;
        let plain = render_src(&format!("{src}}}")).unwrap();
        let flip = render_src(&format!("{src}, hflip:true}}")).unwrap();
        assert_ne!(plain.svg, flip.svg, "hflip changes lane stacking");
    }

    #[test]
    fn field_rotate_emits_transform() {
        let r = render_src(r#"{reg:[{bits:8,name:"rot",rotate:90}]}"#).unwrap();
        assert!(r.svg.contains("rotate(90"), "field name carries rotate transform");
    }

    /// Count synthesized unused fields by re-running the parse + synthesis path
    /// the way `render` does, so tests can assert on field/cell counts.
    fn field_count(src: &str) -> usize {
        let val: Value = json5::from_str(src).expect("json5");
        let reg = val.get("reg").cloned().unwrap_or(val.clone());
        let mut fields = parse_fields(&reg);
        let total_bits: u32 = fields.iter().map(|f| f.bits).sum();
        let lanes = opt_int(&val, "lanes").filter(|l| *l >= 1).map(|l| l as u32).unwrap_or(1);
        let bits = opt_int(&val, "bits")
            .filter(|b| *b >= 1)
            .map(|b| b as u32)
            .unwrap_or(total_bits)
            .max(total_bits)
            .max(lanes);
        if total_bits < bits {
            let lane_w = ((bits as f32) / (lanes as f32)).ceil().max(1.0) as u32;
            let mut lsb = total_bits;
            while lsb < bits {
                let end = (((lsb / lane_w) + 1) * lane_w).min(bits);
                fields.push(Field {
                    bits: end - lsb,
                    name: None,
                    attrs: vec![],
                    ftype: None,
                    rotate: None,
                    lsb,
                    msb: end - 1,
                });
                lsb = end;
            }
        }
        fields.len()
    }

    #[test]
    fn contiguous_field_count_unchanged() {
        // Baseline contiguous reg: exactly the declared fields, no synthesis.
        assert_eq!(
            field_count(r#"{reg:[{bits:8,name:"data"},{bits:4,name:"flags"},{bits:4,name:"op"}]}"#),
            3,
        );
    }

    #[test]
    fn sparse_explicit_bits_synthesizes_unused_field() {
        // Declared fields cover 8 bits but config.bits=16 → one trailing unused
        // field (bits 8..=15) is synthesized: 2 declared + 1 synthesized = 3.
        assert_eq!(
            field_count(r#"{reg:[{bits:3,name:"a"},{bits:5,name:"b"}], config:{bits:16}}"#),
            3,
        );
        // It renders as a reserved/unused (hatched) cell with boundary labels.
        let r = render_src(r#"{reg:[{bits:3,name:"a"},{bits:5,name:"b"}], config:{bits:16}}"#)
            .unwrap();
        assert!(r.svg.contains("bf-hatch"), "trailing unused bits hatched");
        assert!(r.svg.contains(">8</text>"), "gap lsb label 8");
        assert!(r.svg.contains(">15</text>"), "gap msb label 15");
    }

    #[test]
    fn sparse_unused_split_per_lane() {
        // 10 declared bits, bits:24, lanes:2 → mod=12. Tail 10..=23 is split at
        // the lane boundary (12): unused 10..=11 and 12..=23. 2 declared + 2 = 4.
        assert_eq!(
            field_count(r#"{reg:[{bits:5,name:"a"},{bits:5,name:"b"}], config:{bits:24,lanes:2}}"#),
            4,
        );
    }

    #[test]
    fn contiguous_total_width_unchanged() {
        // Snapshot the canvas width metric for a contiguous reg (unaffected by
        // the sparse path since coverage == span).
        let r = render_src(r#"{reg:[{bits:8,name:"data"},{bits:4,name:"flags"},{bits:4,name:"op"}]}"#)
            .unwrap();
        assert_eq!(r.width_px, 800.0);
    }

    #[test]
    fn side_label_renders() {
        let r = render_src(
            r#"{reg:[{bits:8,name:"a"},{bits:8,name:"b"}], lanes:2, label:{left:"L",right:"R"}}"#,
        )
        .unwrap();
        assert!(r.svg.contains(">L</text>"), "left label");
        assert!(r.svg.contains(">R</text>"), "right label");
    }
}
