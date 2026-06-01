//! Themes: named color palettes that feed every diagram's core colors, plus
//! helpers to select a theme from a diagram's frontmatter / `%%{init}%%`
//! directive (the way mermaid does).
//!
//! A theme sets the cross-cutting colors on [`MermaidOptions`](crate::MermaidOptions)
//! — `background`, node fill/stroke, edge/line color, text color, and a
//! categorical `series_palette` (for pie slices, chart bars, sankey nodes, …).
//! Per-diagram local accents (e.g. gantt status colors) are not themed yet.

use crate::MermaidOptions;

/// A built-in mermaid-style theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MermaidTheme {
    /// Light lavender — mermaid's `default`.
    #[default]
    Default,
    /// Dark background, light text/lines.
    Dark,
    /// Greens — mermaid's `forest`.
    Forest,
    /// Greys — mermaid's `neutral`.
    Neutral,
}

impl MermaidTheme {
    /// Parse a theme name (case-insensitive), as used in `theme: <name>` /
    /// `"theme": "<name>"`. Unknown names (incl. `base`) fall back to `Default`
    /// returning `None` so callers keep whatever they had.
    pub fn from_name(name: &str) -> Option<MermaidTheme> {
        match name.trim().trim_matches(['"', '\'']).to_ascii_lowercase().as_str() {
            "default" => Some(MermaidTheme::Default),
            "dark" => Some(MermaidTheme::Dark),
            "forest" => Some(MermaidTheme::Forest),
            "neutral" => Some(MermaidTheme::Neutral),
            _ => None,
        }
    }

    /// The core colors for this theme.
    pub(crate) fn palette(self) -> Palette {
        match self {
            MermaidTheme::Default => Palette {
                background: [255, 255, 255, 255],
                node_fill: [236, 236, 255, 255],
                node_stroke: [147, 112, 219, 255],
                edge_stroke: [51, 51, 51, 255],
                text_color: [51, 51, 51, 255],
                series: SERIES_DEFAULT,
            },
            MermaidTheme::Dark => Palette {
                background: [30, 30, 38, 255],
                node_fill: [54, 58, 79, 255],
                node_stroke: [156, 156, 219, 255],
                edge_stroke: [200, 200, 212, 255],
                text_color: [233, 233, 238, 255],
                series: SERIES_DARK,
            },
            MermaidTheme::Forest => Palette {
                background: [243, 255, 243, 255],
                node_fill: [205, 228, 152, 255],
                node_stroke: [19, 84, 12, 255],
                edge_stroke: [51, 51, 51, 255],
                text_color: [34, 51, 28, 255],
                series: SERIES_FOREST,
            },
            MermaidTheme::Neutral => Palette {
                background: [255, 255, 255, 255],
                node_fill: [238, 238, 238, 255],
                node_stroke: [136, 136, 136, 255],
                edge_stroke: [51, 51, 51, 255],
                text_color: [51, 51, 51, 255],
                series: SERIES_NEUTRAL,
            },
        }
    }
}

/// The resolved core colors of a theme.
pub(crate) struct Palette {
    pub background: [u8; 4],
    pub node_fill: [u8; 4],
    pub node_stroke: [u8; 4],
    pub edge_stroke: [u8; 4],
    pub text_color: [u8; 4],
    pub series: [[u8; 4]; 8],
}

/// Apply a theme's core colors onto `opts` (keeping its fonts/sizes/spacing).
pub(crate) fn apply(opts: &mut MermaidOptions, theme: MermaidTheme) {
    let p = theme.palette();
    opts.background = p.background;
    opts.node_fill = p.node_fill;
    opts.node_stroke = p.node_stroke;
    opts.edge_stroke = p.edge_stroke;
    opts.text_color = p.text_color;
    opts.series_palette = p.series.to_vec();
}

const SERIES_DEFAULT: [[u8; 4]; 8] = [
    [129, 134, 214, 255],
    [255, 213, 128, 255],
    [110, 198, 167, 255],
    [232, 122, 122, 255],
    [120, 175, 220, 255],
    [200, 150, 220, 255],
    [240, 180, 120, 255],
    [150, 200, 150, 255],
];
const SERIES_DARK: [[u8; 4]; 8] = [
    [129, 140, 248, 255],
    [251, 211, 141, 255],
    [110, 231, 183, 255],
    [248, 113, 113, 255],
    [96, 165, 250, 255],
    [196, 153, 247, 255],
    [251, 191, 116, 255],
    [134, 239, 172, 255],
];
const SERIES_FOREST: [[u8; 4]; 8] = [
    [108, 168, 96, 255],
    [180, 200, 120, 255],
    [80, 140, 80, 255],
    [200, 180, 90, 255],
    [120, 160, 110, 255],
    [150, 190, 120, 255],
    [90, 130, 70, 255],
    [170, 200, 130, 255],
];
const SERIES_NEUTRAL: [[u8; 4]; 8] = [
    [170, 170, 170, 255],
    [120, 120, 120, 255],
    [200, 200, 200, 255],
    [90, 90, 90, 255],
    [150, 150, 150, 255],
    [220, 220, 220, 255],
    [70, 70, 70, 255],
    [190, 190, 190, 255],
];

/// Strip a leading `---` frontmatter block and any `%%{init: …}%%` directives
/// from `src`, returning `(cleaned_source, theme_if_specified)`.
///
/// Looks for `theme: <name>` inside the frontmatter and `"theme": "<name>"`
/// (single or double quotes) inside an init directive. The cleaned source is
/// what the diagram parsers see (they don't understand frontmatter).
pub(crate) fn preprocess(src: &str) -> (String, Option<MermaidTheme>) {
    let mut theme: Option<MermaidTheme> = None;
    let mut body = src;

    // Leading YAML frontmatter: `---` … `---`.
    let trimmed = src.trim_start_matches([' ', '\t', '\n', '\r']);
    if trimmed.starts_with("---") {
        let after = &trimmed[3..];
        if let Some(end) = after.find("\n---") {
            let block = &after[..end];
            for line in block.lines() {
                if let Some(rest) = line.trim().strip_prefix("theme:") {
                    if let Some(t) = MermaidTheme::from_name(rest) {
                        theme = Some(t);
                    }
                }
            }
            // Body is everything after the closing `---` line.
            let rest = &after[end + 4..];
            let rest = rest.strip_prefix(|c| c == '\n' || c == '\r').unwrap_or(rest);
            body = rest;
        }
    }

    // `%%{init: {... "theme": "dark" ...}}%%` anywhere — extract the theme, then
    // strip the directive lines (the diagram parsers treat `%%`-lines as
    // comments, but we drop them cleanly so the header detection isn't fooled).
    let mut out_lines: Vec<&str> = Vec::new();
    for line in body.lines() {
        let t = line.trim_start();
        if t.starts_with("%%{") && t.contains("init") {
            if let Some(name) = extract_init_theme(t) {
                if let Some(th) = MermaidTheme::from_name(&name) {
                    theme = Some(th);
                }
            }
            continue; // drop the directive line
        }
        out_lines.push(line);
    }

    (out_lines.join("\n"), theme)
}

/// Pull the `theme` value out of an `%%{init: { ... "theme": "x" ... }}%%` line.
fn extract_init_theme(line: &str) -> Option<String> {
    let idx = line.find("theme")?;
    let after = &line[idx + 5..];
    // skip optional closing quote of the key, `:`, spaces, opening quote
    let after = after.trim_start_matches(['"', '\'', ' ', ':']);
    // value runs until the next quote / comma / brace / space
    let end = after.find(['"', '\'', ',', '}', ' ']).unwrap_or(after.len());
    let val = after[..end].trim();
    if val.is_empty() { None } else { Some(val.to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_theme_names() {
        assert_eq!(MermaidTheme::from_name("dark"), Some(MermaidTheme::Dark));
        assert_eq!(MermaidTheme::from_name("  Forest "), Some(MermaidTheme::Forest));
        assert_eq!(MermaidTheme::from_name("\"neutral\""), Some(MermaidTheme::Neutral));
        assert_eq!(MermaidTheme::from_name("base"), None);
    }

    #[test]
    fn frontmatter_theme_and_strip() {
        let (body, theme) = preprocess("---\nconfig:\n  theme: dark\n---\ngraph TD\n A-->B");
        assert_eq!(theme, Some(MermaidTheme::Dark));
        assert!(body.starts_with("graph TD"), "frontmatter stripped: {body:?}");
    }

    #[test]
    fn init_directive_theme() {
        let (body, theme) = preprocess("%%{init: {'theme': 'forest'}}%%\npie\n \"A\" : 1");
        assert_eq!(theme, Some(MermaidTheme::Forest));
        assert!(body.trim_start().starts_with("pie"));
    }

    #[test]
    fn no_directive() {
        let (body, theme) = preprocess("graph TD\n A-->B");
        assert_eq!(theme, None);
        assert_eq!(body, "graph TD\n A-->B");
    }
}
