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
            // WaveDrom data colors 2..9 (yellow/orange/.. pastels).
            series_palette: vec![
                [255, 255, 180, 255],
                [255, 224, 185, 255],
                [185, 224, 255, 255],
                [185, 255, 185, 255],
                [225, 200, 255, 255],
                [255, 200, 225, 255],
                [200, 255, 240, 255],
                [230, 230, 230, 255],
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
/// diagram family: `{signal:[…]}` → timing waveform; `{reg:[…]}` or a bare
/// `[…]` array → bitfield/register diagram.
pub fn render(src: &str, opts: &WaveDromOptions) -> Result<WaveDromRender, WaveDromError> {
    let val: serde_json::Value =
        json5::from_str(src).map_err(|e| WaveDromError::Parse(e.to_string()))?;

    if val.get("signal").is_some() {
        timing::render(&val, opts)
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
