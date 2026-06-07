//! `hiker-wavedrom` — pure-Rust [WaveDrom](https://wavedrom.com) renderer → SVG.
//!
//! Egui-agnostic, SVG-string out (same contract as the `hiker-render` math /
//! mermaid engines), so callers rasterize with their existing resvg→texture
//! pipeline. Renders the two WaveDrom diagram families from **WaveJSON**:
//!
//! - **Timing waveforms** — `{ signal: [ … ] }`: clock/signal lanes drawn from
//!   `wave:` strings, with data buses, gaps, groups, marks, and edge arrows.
//! - **Bitfield / register diagrams** — `{ reg: [ … ] }` or a bare `[ … ]`
//!   array of fields: a register laid out by bit position with names + attrs.
//!
//! WaveJSON is JSON5 (unquoted keys, single quotes, comments, trailing commas);
//! we parse it with the `json5` crate into a [`serde_json::Value`] and each
//! renderer walks that value directly.

pub mod assign;
pub mod bitfield;
pub mod font;
pub mod svgutil;
pub mod timing;

/// Rendering inputs (sizes, colors, fonts). Defaults approximate WaveDrom's
/// default skin.
#[derive(Clone, Debug)]
pub struct WaveDromOptions {
    /// Label font size in CSS px.
    pub font_size_px: f32,
    /// SVG `font-family` for `<text>` (and assumed by text measurement).
    pub font_family: String,
    /// Foreground (lines / text) color, straight RGBA.
    pub foreground: [u8; 4],
    /// Canvas background, straight RGBA (painted when alpha > 0).
    pub background: [u8; 4],
    /// Categorical palette for data buses / field fills (WaveDrom's `2..9`).
    pub series_palette: Vec<[u8; 4]>,
}

impl Default for WaveDromOptions {
    fn default() -> Self {
        WaveDromOptions {
            font_size_px: 14.0,
            font_family: "Liberation Sans".to_string(),
            foreground: [0, 0, 0, 255],
            background: [255, 255, 255, 255],
            // WaveDrom data-bus colors for wave codes 2..9, mirroring the
            // default skin classes s7..s14 in references/wavedrom/skins/default.js
            // (verified against wavedrom.js): code 2 is WHITE, not yellow.
            //   s7  code 2 #ffffff   s11 code 6 #ccfdfe
            //   s8  code 3 #ffffb4   s12 code 7 #cdfdc5
            //   s9  code 4 #ffe0b9   s13 code 8 #f0c1fb
            //   s10 code 5 #b9e0ff   s14 code 9 #f5c2c0
            series_palette: vec![
                [0xff, 0xff, 0xff, 255], // s7  code 2
                [0xff, 0xff, 0xb4, 255], // s8  code 3
                [0xff, 0xe0, 0xb9, 255], // s9  code 4
                [0xb9, 0xe0, 0xff, 255], // s10 code 5
                [0xcc, 0xfd, 0xfe, 255], // s11 code 6
                [0xcd, 0xfd, 0xc5, 255], // s12 code 7
                [0xf0, 0xc1, 0xfb, 255], // s13 code 8
                [0xf5, 0xc2, 0xc0, 255], // s14 code 9
            ],
        }
    }
}

/// A rendered diagram: a self-contained SVG document plus its pixel size.
#[derive(Clone, Debug, PartialEq)]
pub struct WaveDromRender {
    pub svg: String,
    pub width_px: f32,
    pub height_px: f32,
}

/// Errors from [`render`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WaveDromError {
    /// The source could not be parsed as WaveJSON (JSON5 syntax error).
    Parse(String),
    /// Parsed OK but there was nothing to draw (no signals / fields).
    Empty,
    /// A recognized-but-unsupported shape, or an unrecognized top-level form.
    Unsupported(String),
}

/// Render WaveDrom **WaveJSON** source to an SVG document, auto-detecting the
/// diagram family: `{signal:[…]}` → timing waveform; `{assign:[…]}` → logic-gate
/// circuit schematic; `{reg:[…]}` or a bare `[…]` array → bitfield/register
/// diagram.
pub fn render(src: &str, opts: &WaveDromOptions) -> Result<WaveDromRender, WaveDromError> {
    let val: serde_json::Value =
        json5::from_str(src).map_err(|e| WaveDromError::Parse(e.to_string()))?;

    if val.get("signal").is_some() {
        timing::render(&val, opts)
    } else if let Some(asg) = val.get("assign") {
        assign::render(asg, opts)
    } else if let Some(reg) = val.get("reg") {
        bitfield::render(reg, &val, opts)
    } else if val.is_array() {
        bitfield::render(&val, &val, opts)
    } else {
        Err(WaveDromError::Unsupported(
            "WaveJSON must have a `signal` or `reg` key, or be a bitfield array".to_string(),
        ))
    }
}
