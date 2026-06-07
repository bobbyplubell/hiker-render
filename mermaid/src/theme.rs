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
    /// mermaid's `base` — a plain light theme meant to be customized via
    /// `themeVariables`. Used as-is it looks close to `default`.
    Base,
}

impl MermaidTheme {
    /// Parse a theme name (case-insensitive), as used in `theme: <name>` /
    /// `"theme": "<name>"`. Unknown names fall back to `Default` returning
    /// `None` so callers keep whatever they had.
    pub fn from_name(name: &str) -> Option<MermaidTheme> {
        match name.trim().trim_matches(['"', '\'']).to_ascii_lowercase().as_str() {
            "default" => Some(MermaidTheme::Default),
            "dark" => Some(MermaidTheme::Dark),
            "forest" => Some(MermaidTheme::Forest),
            "neutral" => Some(MermaidTheme::Neutral),
            "base" => Some(MermaidTheme::Base),
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
                // mermaid's default clusterBkg (#ffffde) with a soft olive border.
                cluster_fill: [255, 255, 222, 255],
                cluster_stroke: [170, 170, 51, 255],
                edge_stroke: [51, 51, 51, 255],
                text_color: [51, 51, 51, 255],
                series: SERIES_DEFAULT,
            },
            MermaidTheme::Dark => Palette {
                background: [30, 30, 38, 255],
                node_fill: [54, 58, 79, 255],
                node_stroke: [156, 156, 219, 255],
                // Dark theme: a darker cluster fill with a lighter border.
                cluster_fill: [38, 42, 58, 255],
                cluster_stroke: [140, 140, 170, 255],
                edge_stroke: [200, 200, 212, 255],
                text_color: [233, 233, 238, 255],
                series: SERIES_DARK,
            },
            MermaidTheme::Forest => Palette {
                background: [243, 255, 243, 255],
                node_fill: [205, 228, 152, 255],
                node_stroke: [19, 84, 12, 255],
                // A pale green wash with a muted green border.
                cluster_fill: [225, 240, 200, 255],
                cluster_stroke: [99, 138, 70, 255],
                edge_stroke: [51, 51, 51, 255],
                text_color: [34, 51, 28, 255],
                series: SERIES_FOREST,
            },
            MermaidTheme::Neutral => Palette {
                background: [255, 255, 255, 255],
                node_fill: [238, 238, 238, 255],
                node_stroke: [136, 136, 136, 255],
                // A near-white grey wash with a mid-grey border.
                cluster_fill: [245, 245, 245, 255],
                cluster_stroke: [170, 170, 170, 255],
                edge_stroke: [51, 51, 51, 255],
                text_color: [51, 51, 51, 255],
                series: SERIES_NEUTRAL,
            },
            // `base` is mermaid's customizable light theme: a light lavender
            // fill (#ECECFF) with a mid-grey border and dark lines/text, close
            // to mermaid's own base defaults.
            MermaidTheme::Base => Palette {
                background: [255, 255, 255, 255],
                node_fill: [236, 236, 255, 255],
                node_stroke: [153, 153, 153, 255],
                // Same neutral light cluster wash as mermaid's base.
                cluster_fill: [255, 255, 222, 255],
                cluster_stroke: [170, 170, 51, 255],
                edge_stroke: [51, 51, 51, 255],
                text_color: [51, 51, 51, 255],
                series: SERIES_DEFAULT,
            },
        }
    }
}

/// The resolved core colors of a theme.
pub(crate) struct Palette {
    pub background: [u8; 4],
    pub node_fill: [u8; 4],
    pub node_stroke: [u8; 4],
    pub cluster_fill: [u8; 4],
    pub cluster_stroke: [u8; 4],
    pub edge_stroke: [u8; 4],
    pub text_color: [u8; 4],
    pub series: [[u8; 4]; 8],
}

/// Apply a theme's core colors onto `opts` (keeping its fonts/sizes/spacing).
pub(crate) fn apply(opts: &mut MermaidOptions, theme: MermaidTheme) {
    let p = theme.palette();
    opts.background = p.background;
    // Edge-label backing defaults to the canvas color (mermaid uses a near-bg
    // `edgeLabelBackground`); a transparent canvas leaves it transparent so no
    // box is painted. The host can override it with an opaque surface color.
    opts.edge_label_bg = p.background;
    opts.node_fill = p.node_fill;
    opts.node_stroke = p.node_stroke;
    opts.cluster_fill = p.cluster_fill;
    opts.cluster_stroke = p.cluster_stroke;
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

/// Per-field color overrides parsed from a `themeVariables` map (front-matter
/// `config:` block or `%%{init}%%` directive). Each is `Some` only when the
/// matching mermaid variable was present and parsed to a color.
///
/// Mapping of mermaid `themeVariables` names → [`MermaidOptions`] fields:
///
/// | mermaid variable          | field                                |
/// |---------------------------|--------------------------------------|
/// | `background`              | `background`                         |
/// | `primaryColor`            | `node_fill`                          |
/// | `primaryBorderColor`      | `node_stroke`                        |
/// | `primaryTextColor`        | `text_color` (wins over `textColor`) |
/// | `textColor`               | `text_color`                         |
/// | `lineColor`               | `edge_stroke`                        |
/// | `edgeLabelBackground`     | `edge_label_bg`                      |
/// | `clusterBkg`              | `cluster_fill`                       |
/// | `clusterBorder`           | `cluster_stroke`                     |
/// | `pie1`..`pie12`           | `series_palette[N-1]` (1-based)      |
/// | `cScale0`..`cScale11`     | `series_palette[N]` (0-based)        |
/// | `secondaryColor`          | `series_palette[1]` (approx.; pie2 wins) |
/// | `tertiaryColor`           | `series_palette[2]` (approx.; pie3 wins), and `cluster_fill` (fallback; explicit `clusterBkg` wins) |
///
/// Unknown variable names and unparseable color values are ignored. The
/// distinct `text_color` and `primary_text_color` slots preserve precedence:
/// `text_color` is applied first, then `primary_text_color` (the more specific
/// one) overrides it when both are present.
///
/// `secondaryColor`/`tertiaryColor` are an approximation: mermaid derives a
/// family of node-group fills from them, but we model only the categorical
/// `series_palette`, so they map onto series indices 1 and 2. An explicit
/// `pie2`/`cScale1` (resp. `pie3`/`cScale2`) wins over `secondaryColor`
/// (resp. `tertiaryColor`).
///
/// `tertiaryColor` *additionally* feeds `cluster_fill` as a fallback (mermaid
/// derives `clusterBkg` from `tertiaryColor`): an explicit `clusterBkg` wins,
/// but when only `tertiaryColor` is given it sets the cluster fill too. Both
/// derivations (series[2] and cluster_fill) coexist.
///
/// Series overrides are held in `series` as `Some(color)` per 0-based index;
/// `pieN` maps to index `N-1`, `cScaleN` to index `N`.
#[derive(Default)]
pub(crate) struct ThemeVars {
    pub background: Option<[u8; 4]>,
    pub node_fill: Option<[u8; 4]>,
    pub node_stroke: Option<[u8; 4]>,
    pub text_color: Option<[u8; 4]>,
    pub primary_text_color: Option<[u8; 4]>,
    pub edge_stroke: Option<[u8; 4]>,
    pub edge_label_bg: Option<[u8; 4]>,
    pub cluster_fill: Option<[u8; 4]>,
    pub cluster_stroke: Option<[u8; 4]>,
    /// Per-index categorical-palette overrides (0-based). Set explicitly by
    /// `pieN`/`cScaleN`.
    pub series: [Option<[u8; 4]>; 12],
    /// Approximate overrides from `secondaryColor`/`tertiaryColor`, applied to
    /// series indices 1 and 2 only when not already set by an explicit
    /// `pieN`/`cScaleN`.
    pub secondary_color: Option<[u8; 4]>,
    pub tertiary_color: Option<[u8; 4]>,
}

impl ThemeVars {
    /// True when at least one override was parsed.
    pub fn any(&self) -> bool {
        self.background.is_some()
            || self.node_fill.is_some()
            || self.node_stroke.is_some()
            || self.text_color.is_some()
            || self.primary_text_color.is_some()
            || self.edge_stroke.is_some()
            || self.edge_label_bg.is_some()
            || self.cluster_fill.is_some()
            || self.cluster_stroke.is_some()
            || self.secondary_color.is_some()
            || self.tertiary_color.is_some()
            || self.series.iter().any(Option::is_some)
    }

    /// Apply the present overrides onto `opts`. Call this *after* theme/look
    /// selection so `themeVariables` win over the chosen/base theme.
    pub fn apply_to(&self, opts: &mut MermaidOptions) {
        if let Some(c) = self.background {
            opts.background = c;
        }
        if let Some(c) = self.node_fill {
            opts.node_fill = c;
        }
        if let Some(c) = self.node_stroke {
            opts.node_stroke = c;
        }
        // `textColor` first, then the more specific `primaryTextColor`.
        if let Some(c) = self.text_color {
            opts.text_color = c;
        }
        if let Some(c) = self.primary_text_color {
            opts.text_color = c;
        }
        if let Some(c) = self.edge_stroke {
            opts.edge_stroke = c;
        }
        if let Some(c) = self.edge_label_bg {
            opts.edge_label_bg = c;
        }

        // Cluster colors. `clusterBkg` is the explicit fill; when absent,
        // `tertiaryColor` feeds the cluster fill as a fallback (mermaid derives
        // clusterBkg from tertiaryColor). An explicit `clusterBkg` wins.
        if let Some(c) = self.cluster_fill.or(self.tertiary_color) {
            opts.cluster_fill = c;
        }
        if let Some(c) = self.cluster_stroke {
            opts.cluster_stroke = c;
        }

        // Categorical-palette overrides. Explicit `pieN`/`cScaleN` first, then
        // `secondaryColor`/`tertiaryColor` fill indices 1/2 only if those
        // weren't already set explicitly.
        let mut effective = self.series;
        if effective[1].is_none() {
            effective[1] = self.secondary_color;
        }
        if effective[2].is_none() {
            effective[2] = self.tertiary_color;
        }
        for (i, slot) in effective.iter().enumerate() {
            if let Some(c) = *slot {
                set_series(&mut opts.series_palette, i, c);
            }
        }
    }

    /// Set the slot for mermaid variable `name` from a raw color string.
    /// Accepts mermaid's camelCase and its all-lowercase form. Unknown names
    /// and unparseable colors are silently ignored.
    fn set(&mut self, name: &str, raw: &str) {
        let raw = raw.trim().trim_matches(['"', '\'']);
        let Some(color) = crate::parse::directives::parse_color(raw) else {
            return;
        };
        match name {
            "background" => self.background = Some(color),
            "primaryColor" | "primarycolor" => self.node_fill = Some(color),
            "primaryBorderColor" | "primarybordercolor" => self.node_stroke = Some(color),
            "primaryTextColor" | "primarytextcolor" => self.primary_text_color = Some(color),
            "textColor" | "textcolor" => self.text_color = Some(color),
            "lineColor" | "linecolor" => self.edge_stroke = Some(color),
            "edgeLabelBackground" | "edgelabelbackground" => self.edge_label_bg = Some(color),
            "clusterBkg" | "clusterbkg" => self.cluster_fill = Some(color),
            "clusterBorder" | "clusterborder" => self.cluster_stroke = Some(color),
            "secondaryColor" | "secondarycolor" => self.secondary_color = Some(color),
            "tertiaryColor" | "tertiarycolor" => self.tertiary_color = Some(color),
            // `pieN` (1-based) and `cScaleN` (0-based) override the categorical
            // palette. Parse the trailing integer and route to a 0-based index.
            _ => {
                let lower = name.to_ascii_lowercase();
                if let Some(idx) = lower
                    .strip_prefix("pie")
                    .and_then(|n| n.parse::<usize>().ok())
                    .and_then(|n| n.checked_sub(1))
                {
                    if idx < self.series.len() {
                        self.series[idx] = Some(color);
                    }
                } else if let Some(idx) =
                    lower.strip_prefix("cscale").and_then(|n| n.parse::<usize>().ok())
                {
                    if idx < self.series.len() {
                        self.series[idx] = Some(color);
                    }
                }
            }
        }
    }
}

/// Set `palette[idx]` to `color`, growing the Vec if needed. Gap entries
/// added while growing clone the previous last color (or `color` itself when
/// the palette is empty) so the categorical palette stays fully populated.
fn set_series(palette: &mut Vec<[u8; 4]>, idx: usize, color: [u8; 4]) {
    while palette.len() <= idx {
        let fill = palette.last().copied().unwrap_or(color);
        palette.push(fill);
    }
    palette[idx] = color;
}

/// Config extracted from a diagram's frontmatter / `%%{init}%%` directive.
#[derive(Default)]
pub(crate) struct Config {
    pub theme: Option<MermaidTheme>,
    pub look: Option<crate::Look>,
    pub font_family: Option<String>,
    pub font_size: Option<f32>,
    pub theme_vars: ThemeVars,
}

/// Strip a leading `---` frontmatter block and any `%%{init: …}%%` directives
/// from `src`, returning `(cleaned_source, config)`.
///
/// Extracts `theme`, `look`, `fontFamily`, and `fontSize` from frontmatter
/// (`key: value`) and from an init directive (`"key": "value"`), plus a
/// nested `themeVariables` color-override map from either syntax (see
/// [`ThemeVars`]). The cleaned source is what the diagram parsers see (they
/// don't understand frontmatter).
pub(crate) fn preprocess(src: &str) -> (String, Config) {
    let mut cfg = Config::default();
    let mut body = src;

    let apply = |cfg: &mut Config, key: &str, val: &str| {
        let val = val.trim().trim_matches(['"', '\'']);
        match key {
            "theme" => {
                if let Some(t) = MermaidTheme::from_name(val) {
                    cfg.theme = Some(t);
                }
            }
            "look" => {
                cfg.look = match val.to_ascii_lowercase().as_str() {
                    "handdrawn" | "hand-drawn" | "rough" | "sketch" => Some(crate::Look::HandDrawn),
                    "classic" | "default" | "neat" => Some(crate::Look::Classic),
                    _ => cfg.look,
                };
            }
            "fontFamily" | "fontfamily" => {
                if !val.is_empty() {
                    cfg.font_family = Some(val.to_string());
                }
            }
            "fontSize" | "fontsize" => {
                if let Ok(n) = val.trim_end_matches("px").trim().parse::<f32>() {
                    if n > 0.0 {
                        cfg.font_size = Some(n);
                    }
                }
            }
            _ => {}
        }
    };

    // Leading YAML frontmatter: `---` … `---`.
    let trimmed = src.trim_start_matches([' ', '\t', '\n', '\r']);
    if trimmed.starts_with("---") {
        let after = &trimmed[3..];
        if let Some(end) = after.find("\n---") {
            let block = &after[..end];
            // Indentation (leading-space count) of the `themeVariables:` line,
            // or `None` while we're not inside such a nested block. Lines more
            // indented than this are theme variables; an equal/lesser indent
            // (or end of block) closes it.
            let mut in_vars: Option<usize> = None;
            for line in block.lines() {
                let indent = line.len() - line.trim_start().len();
                let l = line.trim();
                if l.is_empty() {
                    continue;
                }
                if let Some(base) = in_vars {
                    if indent > base {
                        if let Some((k, v)) = l.split_once(':') {
                            cfg.theme_vars.set(k.trim(), v);
                        }
                        continue;
                    }
                    in_vars = None; // dedented back out of the nested block
                }
                if let Some((k, v)) = l.split_once(':') {
                    let key = k.trim();
                    if (key == "themeVariables" || key == "themevariables")
                        && v.trim().is_empty()
                    {
                        in_vars = Some(indent);
                        continue;
                    }
                    apply(&mut cfg, key, v);
                }
            }
            let rest = &after[end + 4..];
            let rest = rest.strip_prefix(|c| c == '\n' || c == '\r').unwrap_or(rest);
            body = rest;
        }
    }

    // `%%{init: { … }}%%` anywhere — extract config keys, then strip the line.
    let mut out_lines: Vec<&str> = Vec::new();
    for line in body.lines() {
        let t = line.trim_start();
        if t.starts_with("%%{") && t.contains("init") {
            for key in ["theme", "look", "fontFamily", "fontSize"] {
                if let Some(v) = extract_init_value(t, key) {
                    apply(&mut cfg, key, &v);
                }
            }
            if let Some(obj) = extract_init_object(t, "themeVariables") {
                parse_var_object(&obj, &mut cfg.theme_vars);
            }
            continue; // drop the directive line
        }
        out_lines.push(line);
    }

    (out_lines.join("\n"), cfg)
}

/// Pull a `"<key>": "<value>"` value out of an `%%{init: { … }}%%` line.
fn extract_init_value(line: &str, key: &str) -> Option<String> {
    let idx = line.find(key)?;
    let after = &line[idx + key.len()..];
    // skip optional closing quote of the key, `:`, spaces, opening quote
    let after = after.trim_start_matches(['"', '\'', ' ', ':']);
    let end = after.find(['"', '\'', ',', '}']).unwrap_or(after.len());
    let val = after[..end].trim();
    if val.is_empty() { None } else { Some(val.to_string()) }
}

/// Extract the brace-matched `{ … }` object that follows `<key>` in an init
/// directive line (e.g. the value of `"themeVariables"`), without the braces.
/// Returns `None` if the key or a balanced object isn't found.
fn extract_init_object(line: &str, key: &str) -> Option<String> {
    let idx = line.find(key)?;
    let open = line[idx + key.len()..].find('{')? + idx + key.len();
    let bytes = line.as_bytes();
    let mut depth = 0usize;
    for (i, &b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(line[open + 1..i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// Parse `"key": "value"` pairs out of a (brace-stripped) JSON-ish object body
/// into `vars`. Values may be quoted or bare; commas separate pairs.
fn parse_var_object(obj: &str, vars: &mut ThemeVars) {
    for pair in obj.split(',') {
        if let Some((k, v)) = pair.split_once(':') {
            let key = k.trim().trim_matches(['"', '\'']);
            if !key.is_empty() {
                vars.set(key, v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_theme_names() {
        assert_eq!(MermaidTheme::from_name("dark"), Some(MermaidTheme::Dark));
        assert_eq!(MermaidTheme::from_name("  Forest "), Some(MermaidTheme::Forest));
        assert_eq!(MermaidTheme::from_name("\"neutral\""), Some(MermaidTheme::Neutral));
        assert_eq!(MermaidTheme::from_name("base"), Some(MermaidTheme::Base));
        assert_eq!(MermaidTheme::from_name("nope"), None);
    }

    #[test]
    fn base_theme_resolves_and_applies() {
        let (_body, cfg) = preprocess("---\nconfig:\n  theme: base\n---\ngraph TD\n A-->B");
        assert_eq!(cfg.theme, Some(MermaidTheme::Base));
        let mut opts = MermaidOptions::default();
        apply(&mut opts, MermaidTheme::Base);
        let p = MermaidTheme::Base.palette();
        assert_eq!(opts.node_fill, p.node_fill);
        assert_eq!(opts.node_stroke, p.node_stroke);
        assert_eq!(opts.text_color, p.text_color);
    }

    #[test]
    fn base_plus_theme_variables_override() {
        let src = "---\nconfig:\n  theme: base\n  themeVariables:\n    primaryColor: \"#ff0000\"\n---\ngraph TD\n  A --> B";
        let (_body, cfg) = preprocess(src);
        let mut opts = MermaidOptions::default();
        apply(&mut opts, cfg.theme.unwrap());
        assert_eq!(opts.node_fill, MermaidTheme::Base.palette().node_fill);
        cfg.theme_vars.apply_to(&mut opts);
        assert_eq!(opts.node_fill, [255, 0, 0, 255]);
    }

    #[test]
    fn pie_overrides_series_frontmatter() {
        let src = "---\nconfig:\n  theme: base\n  themeVariables:\n    pie1: \"#ff0000\"\n    pie2: \"#00ff00\"\n---\npie\n \"A\" : 1";
        let (_body, cfg) = preprocess(src);
        let mut opts = MermaidOptions::default();
        apply(&mut opts, cfg.theme.unwrap());
        cfg.theme_vars.apply_to(&mut opts);
        assert_eq!(opts.series_palette[0], [255, 0, 0, 255]);
        assert_eq!(opts.series_palette[1], [0, 255, 0, 255]);
    }

    #[test]
    fn pie_overrides_series_init() {
        let src = "%%{init: {\"theme\":\"base\", \"themeVariables\": {\"pie1\":\"#ff0000\",\"pie2\":\"#0f0\"}}}%%\npie\n \"A\" : 1";
        let (_body, cfg) = preprocess(src);
        let mut opts = MermaidOptions::default();
        apply(&mut opts, cfg.theme.unwrap());
        cfg.theme_vars.apply_to(&mut opts);
        assert_eq!(opts.series_palette[0], [255, 0, 0, 255]);
        assert_eq!(opts.series_palette[1], [0, 255, 0, 255]);
    }

    #[test]
    fn cscale_overrides_series_zero_based() {
        let src = "---\nconfig:\n  theme: base\n  themeVariables:\n    cScale0: \"#ff0000\"\n---\npie\n \"A\" : 1";
        let (_body, cfg) = preprocess(src);
        let mut opts = MermaidOptions::default();
        apply(&mut opts, cfg.theme.unwrap());
        cfg.theme_vars.apply_to(&mut opts);
        assert_eq!(opts.series_palette[0], [255, 0, 0, 255]);
    }

    #[test]
    fn secondary_color_maps_to_series_one() {
        let src = "---\nconfig:\n  theme: base\n  themeVariables:\n    secondaryColor: \"#00ff00\"\n---\npie\n \"A\" : 1";
        let (_body, cfg) = preprocess(src);
        let mut opts = MermaidOptions::default();
        apply(&mut opts, cfg.theme.unwrap());
        cfg.theme_vars.apply_to(&mut opts);
        assert_eq!(opts.series_palette[1], [0, 255, 0, 255]);
    }

    #[test]
    fn explicit_pie2_wins_over_secondary_color() {
        let src = "---\nconfig:\n  theme: base\n  themeVariables:\n    secondaryColor: \"#00ff00\"\n    pie2: \"#0000ff\"\n---\npie\n \"A\" : 1";
        let (_body, cfg) = preprocess(src);
        let mut opts = MermaidOptions::default();
        apply(&mut opts, cfg.theme.unwrap());
        cfg.theme_vars.apply_to(&mut opts);
        assert_eq!(opts.series_palette[1], [0, 0, 255, 255]);
    }

    #[test]
    fn series_override_beyond_len_grows_vec() {
        // Start from a short palette, override index 10 → no panic, Vec grows.
        let mut vars = ThemeVars::default();
        vars.set("pie11", "#ff0000"); // pie11 → index 10
        let mut opts = MermaidOptions::default();
        opts.series_palette = vec![[1, 1, 1, 255], [2, 2, 2, 255]];
        vars.apply_to(&mut opts);
        assert_eq!(opts.series_palette.len(), 11);
        assert_eq!(opts.series_palette[10], [255, 0, 0, 255]);
        // Pre-existing entries are untouched.
        assert_eq!(opts.series_palette[0], [1, 1, 1, 255]);
    }

    #[test]
    fn frontmatter_theme_and_strip() {
        let (body, cfg) = preprocess("---\nconfig:\n  theme: dark\n---\ngraph TD\n A-->B");
        assert_eq!(cfg.theme, Some(MermaidTheme::Dark));
        assert!(body.starts_with("graph TD"), "frontmatter stripped: {body:?}");
    }

    #[test]
    fn init_directive_theme() {
        let (body, cfg) = preprocess("%%{init: {'theme': 'forest'}}%%\npie\n \"A\" : 1");
        assert_eq!(cfg.theme, Some(MermaidTheme::Forest));
        assert!(body.trim_start().starts_with("pie"));
    }

    #[test]
    fn no_directive() {
        let (body, cfg) = preprocess("graph TD\n A-->B");
        assert_eq!(cfg.theme, None);
        assert_eq!(body, "graph TD\n A-->B");
    }

    #[test]
    fn frontmatter_theme_variables() {
        let src = "---\nconfig:\n  theme: base\n  themeVariables:\n    primaryColor: \"#ff0000\"\n    lineColor: '#00ff00'\n    textColor: \"#333\"\n---\ngraph TD\n  A --> B";
        let (body, cfg) = preprocess(src);
        let v = &cfg.theme_vars;
        assert_eq!(v.node_fill, Some([255, 0, 0, 255]));
        assert_eq!(v.edge_stroke, Some([0, 255, 0, 255]));
        assert_eq!(v.text_color, Some([51, 51, 51, 255]));
        assert!(body.starts_with("graph TD"), "frontmatter stripped: {body:?}");
    }

    #[test]
    fn init_directive_theme_variables() {
        let src = "%%{init: {\"theme\":\"base\", \"themeVariables\": {\"primaryColor\":\"#ff0000\",\"lineColor\":\"#0f0\"}}}%%\ngraph TD\n  A --> B";
        let (body, cfg) = preprocess(src);
        let v = &cfg.theme_vars;
        assert_eq!(v.node_fill, Some([255, 0, 0, 255]));
        assert_eq!(v.edge_stroke, Some([0, 255, 0, 255]));
        assert!(body.trim_start().starts_with("graph TD"));
    }

    #[test]
    fn theme_variables_override_selected_theme() {
        // `dark` theme then a primaryColor override → node_fill is red, not dark's.
        let src = "---\nconfig:\n  theme: dark\n  themeVariables:\n    primaryColor: \"#ff0000\"\n---\ngraph TD\n  A --> B";
        let (_body, cfg) = preprocess(src);
        let mut opts = MermaidOptions::default();
        if let Some(t) = cfg.theme {
            apply(&mut opts, t);
        }
        assert_eq!(opts.node_fill, MermaidTheme::Dark.palette().node_fill);
        cfg.theme_vars.apply_to(&mut opts);
        assert_eq!(opts.node_fill, [255, 0, 0, 255]);
    }

    #[test]
    fn primary_text_color_wins_over_text_color() {
        let src = "---\nconfig:\n  themeVariables:\n    textColor: \"#111111\"\n    primaryTextColor: \"#222222\"\n---\ngraph TD\n  A --> B";
        let (_body, cfg) = preprocess(src);
        let mut opts = MermaidOptions::default();
        cfg.theme_vars.apply_to(&mut opts);
        assert_eq!(opts.text_color, [34, 34, 34, 255]);
    }

    #[test]
    fn unknown_var_and_bad_color_ignored() {
        let src = "---\nconfig:\n  themeVariables:\n    bogusVariable: \"#ff0000\"\n    primaryColor: \"not-a-color\"\n    lineColor: \"#00ff00\"\n---\ngraph TD\n  A --> B";
        let (_body, cfg) = preprocess(src);
        let v = &cfg.theme_vars;
        assert_eq!(v.node_fill, None, "unparseable color skipped");
        assert_eq!(v.edge_stroke, Some([0, 255, 0, 255]));
        assert!(!v.background.is_some() && !v.node_stroke.is_some());
    }

    #[test]
    fn theme_applies_cluster_colors() {
        // Each theme sets cluster_fill / cluster_stroke from its palette.
        let mut opts = MermaidOptions::default();
        apply(&mut opts, MermaidTheme::Dark);
        let p = MermaidTheme::Dark.palette();
        assert_eq!(opts.cluster_fill, p.cluster_fill);
        assert_eq!(opts.cluster_stroke, p.cluster_stroke);
        // Dark's cluster fill is darker than its node fill's brightness is not a
        // strict invariant, but it must differ from the default theme's.
        let mut def = MermaidOptions::default();
        apply(&mut def, MermaidTheme::Default);
        assert_ne!(opts.cluster_fill, def.cluster_fill);
    }

    #[test]
    fn cluster_bkg_and_border_override() {
        let src = "---\nconfig:\n  themeVariables:\n    clusterBkg: \"#ffe0b2\"\n    clusterBorder: \"#e65100\"\n---\ngraph TD\n  A --> B";
        let (_body, cfg) = preprocess(src);
        let mut opts = MermaidOptions::default();
        cfg.theme_vars.apply_to(&mut opts);
        assert_eq!(opts.cluster_fill, [255, 224, 178, 255]);
        assert_eq!(opts.cluster_stroke, [230, 81, 0, 255]);
    }

    #[test]
    fn tertiary_color_falls_back_to_cluster_fill() {
        // tertiaryColor alone feeds cluster_fill (and series[2]).
        let src = "---\nconfig:\n  themeVariables:\n    tertiaryColor: \"#00ff00\"\n---\ngraph TD\n  A --> B";
        let (_body, cfg) = preprocess(src);
        let mut opts = MermaidOptions::default();
        cfg.theme_vars.apply_to(&mut opts);
        assert_eq!(opts.cluster_fill, [0, 255, 0, 255]);
        assert_eq!(opts.series_palette[2], [0, 255, 0, 255]);
    }

    #[test]
    fn explicit_cluster_bkg_wins_over_tertiary() {
        // When both are given, clusterBkg wins for cluster_fill; tertiaryColor
        // still drives series[2].
        let src = "---\nconfig:\n  themeVariables:\n    tertiaryColor: \"#00ff00\"\n    clusterBkg: \"#0000ff\"\n---\ngraph TD\n  A --> B";
        let (_body, cfg) = preprocess(src);
        let mut opts = MermaidOptions::default();
        cfg.theme_vars.apply_to(&mut opts);
        assert_eq!(opts.cluster_fill, [0, 0, 255, 255]);
        assert_eq!(opts.series_palette[2], [0, 255, 0, 255]);
    }

    #[test]
    fn theme_vars_block_ends_on_dedent() {
        // A sibling key after the nested block must not be swallowed as a var.
        let src = "---\nconfig:\n  themeVariables:\n    primaryColor: \"#ff0000\"\n  theme: dark\n---\ngraph TD\n  A --> B";
        let (_body, cfg) = preprocess(src);
        assert_eq!(cfg.theme, Some(MermaidTheme::Dark));
        assert_eq!(cfg.theme_vars.node_fill, Some([255, 0, 0, 255]));
    }
}
