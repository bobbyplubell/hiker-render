//! User-journey diagram (self-contained: parse + draw, no graph layout).
//!
//! Mermaid `journey` syntax (the subset we support):
//! ```text
//! journey
//!     title My working day
//!     section Go to work
//!       Make tea: 5: Me
//!       Go upstairs: 3: Me
//!       Do work: 1: Me, Cat
//!     section Go home
//!       Go downstairs: 5: Me
//! ```
//! The header line is `journey`. `title <text>` sets the diagram title.
//! `section <name>` opens a section that groups the task lines that follow it.
//! A task line is `Task name: <score>: <actor1>, <actor2>` where `<score>` is an
//! integer 1..=5 (satisfaction) and the trailing colon-part is a comma list of
//! actor names (which may be empty). Blank lines and `%%` comments are ignored.
//!
//! Layout is a horizontal timeline (no graph layout). Tasks are laid left→right
//! in source order. Each section spans a colored band (header bar) across the
//! tasks it contains. A task's vertical position reflects its score (higher =
//! higher up) and the task marker is colored on a red→green ramp by score. The
//! task name sits above the marker, the numeric score below it, and each actor
//! gets a small labeled dot underneath. Consecutive tasks are joined by the
//! journey path line. The title is centered on top.
//!
//! See `references/mermaid/packages/mermaid/src/diagrams/user-journey/` for the
//! upstream renderer this mirrors (faces/actors/section bands).

use std::fmt::Write as _;

use crate::svgutil::{escape, opacity_attr, rgb, text_size};
use crate::{MermaidError, MermaidOptions, MermaidRender};

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// One task on the journey: a name, a 1..=5 satisfaction score, its actors, and
/// the section it belongs to (an index into the section list, or `None` when the
/// task appeared before any `section`).
#[derive(Clone, Debug, PartialEq)]
struct Task {
    name: String,
    score: i32,
    actors: Vec<String>,
    section: Option<usize>,
}

/// A parsed journey: optional title, the ordered section names, and the tasks.
#[derive(Clone, Debug, PartialEq)]
struct Journey {
    title: Option<String>,
    sections: Vec<String>,
    tasks: Vec<Task>,
}

// ---------------------------------------------------------------------------
// Parse
// ---------------------------------------------------------------------------

/// Parse mermaid journey source into a [`Journey`]. Returns `Err(message)` when
/// the `journey` header is missing/malformed.
fn parse_journey(src: &str) -> Result<Journey, String> {
    let mut title: Option<String> = None;
    let mut sections: Vec<String> = Vec::new();
    let mut tasks: Vec<Task> = Vec::new();
    let mut current: Option<usize> = None;
    let mut saw_header = false;

    for raw in src.lines() {
        // Strip `%%` comments and surrounding whitespace.
        let line = raw.split("%%").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }

        if !saw_header {
            let rest = line
                .strip_prefix("journey")
                .filter(|r| r.is_empty() || r.starts_with(char::is_whitespace))
                .ok_or_else(|| format!("expected 'journey' header, got: {line:?}"))?;
            saw_header = true;
            // A `journey title ...` tail is tolerated.
            let rest = rest.trim();
            if let Some(t) = rest.strip_prefix("title") {
                if t.is_empty() || t.starts_with(char::is_whitespace) {
                    let t = t.trim();
                    if !t.is_empty() {
                        title = Some(t.to_string());
                    }
                }
            }
            continue;
        }

        if let Some(t) = strip_keyword(line, "title") {
            if !t.is_empty() {
                title = Some(t.to_string());
            }
            continue;
        }
        if let Some(name) = strip_keyword(line, "section") {
            sections.push(name.to_string());
            current = Some(sections.len() - 1);
            continue;
        }

        // Otherwise a task line: `Task name: <score>: <actors>`.
        if let Some(task) = parse_task_line(line, current) {
            tasks.push(task);
        }
        // Unrecognized lines are silently skipped (forgiving, like mermaid).
    }

    if !saw_header {
        return Err("empty input / no 'journey' header".to_string());
    }
    Ok(Journey { title, sections, tasks })
}

/// If `line` begins with `kw` followed by whitespace (or is exactly `kw`),
/// return the trimmed remainder; otherwise `None`.
fn strip_keyword<'a>(line: &'a str, kw: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(kw)?;
    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        Some(rest.trim())
    } else {
        None
    }
}

/// Parse a task line `Name: <score>: <actor1>, <actor2>`. The actor part (and
/// its leading colon) is optional. Returns `None` if there is no score field.
fn parse_task_line(line: &str, section: Option<usize>) -> Option<Task> {
    // Split into at most 3 colon-separated pieces: name, score, actors.
    let first = line.find(':')?;
    let name = line[..first].trim().to_string();
    if name.is_empty() {
        return None;
    }
    let after = &line[first + 1..];
    // Score is up to the next colon (or the whole remainder).
    let (score_str, actors_str) = match after.find(':') {
        Some(c) => (after[..c].trim(), after[c + 1..].trim()),
        None => (after.trim(), ""),
    };
    let score: i32 = score_str.parse().ok()?;
    let actors: Vec<String> = actors_str
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    Some(Task { name, score: score.clamp(1, 5), actors, section })
}

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------

/// Section band colors, cycled across sections (mermaid's pastel rotation).
const SECTION_PALETTE: [[u8; 3]; 8] = [
    [0xCD, 0xE4, 0x98],
    [0xA9, 0xD1, 0x8E],
    [0x86, 0xBE, 0x83],
    [0xD6, 0xDC, 0xFF],
    [0xC8, 0xC8, 0xFF],
    [0xFF, 0xE0, 0xB2],
    [0xFF, 0xCC, 0xBC],
    [0xB2, 0xEB, 0xF2],
];

/// The section band color for section index `i` (cycling).
fn section_color(i: usize) -> [u8; 3] {
    SECTION_PALETTE[i % SECTION_PALETTE.len()]
}

/// A task marker color from a red→green ramp keyed by score 1..=5.
/// 1 = red (dissatisfied), 3 = amber, 5 = green (delighted).
fn score_color(score: i32) -> [u8; 3] {
    match score.clamp(1, 5) {
        1 => [0xE5, 0x39, 0x35], // red
        2 => [0xFB, 0x8C, 0x00], // orange
        3 => [0xFD, 0xD8, 0x35], // amber
        4 => [0x7C, 0xB3, 0x42], // light green
        _ => [0x43, 0xA0, 0x47], // green
    }
}

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------

const MARGIN: f32 = 30.0;
/// Horizontal width allotted to each task column, px.
const TASK_W: f32 = 150.0;
/// Task marker radius, px.
const MARKER_R: f32 = 18.0;
/// Height of a section header band, px.
const SECTION_BAND_H: f32 = 26.0;
/// Vertical span over which the score (1..=5) maps to marker center height.
const SCORE_SPAN_H: f32 = 120.0;
/// Actor dot radius, px.
const ACTOR_R: f32 = 7.0;
/// Stroke width for the journey path and markers, px.
const STROKE_W: f32 = 2.0;

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Render mermaid journey source to an SVG document.
pub fn render_journey(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let journey = parse_journey(src).map_err(MermaidError::Parse)?;
    if journey.tasks.is_empty() {
        return Err(MermaidError::Empty);
    }

    let fs = opts.font_size_px;
    let title_fs = fs * 1.5;
    let n = journey.tasks.len();

    // Max actor count across tasks → how much vertical room the actor rows need.
    let max_actors = journey.tasks.iter().map(|t| t.actors.len()).max().unwrap_or(0);

    // Vertical bands (top→bottom): title, section header, task-name labels,
    // the score/marker plot, score numbers, then actor rows.
    let title_band = if journey.title.is_some() { title_fs + MARGIN * 0.5 } else { 0.0 };
    let band_top = title_band + MARGIN;
    let section_y = band_top; // section header band top
    let name_y = section_y + SECTION_BAND_H + MARGIN * 0.5; // task-name label baseline area
    let plot_top = name_y + fs * 1.4; // top of the marker plot region
    let plot_h = SCORE_SPAN_H + 2.0 * MARKER_R;
    let score_y = plot_top + plot_h + fs * 0.4; // numeric score baseline
    let actors_top = score_y + fs * 0.8; // first actor row center
    let actor_row_h = ACTOR_R * 2.0 + 6.0;

    // Room on the left for a "Satisfaction 1–5" axis so the vertical meaning of
    // the markers is explicit.
    let axis_w = fs * 2.4;
    let plot_left = MARGIN + axis_w;
    let width = plot_left + TASK_W * n as f32 + MARGIN;
    let height = actors_top + actor_row_h * max_actors as f32 + MARGIN;

    // Center X of each task column.
    let task_cx = |i: usize| plot_left + TASK_W * (i as f32 + 0.5);
    // Marker center Y from score: score 5 high (small y), score 1 low (large y).
    let marker_cy = |score: i32| {
        let frac = (score.clamp(1, 5) - 1) as f32 / 4.0; // 0..1, 1 at score 5
        plot_top + MARKER_R + (1.0 - frac) * SCORE_SPAN_H
    };

    let mut svg = String::new();
    let w = (width.ceil() + 1.0).max(1.0);
    let h = (height.ceil() + 1.0).max(1.0);
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"0 0 {w} {h}\">"
    );

    // Title centered on top.
    if let Some(t) = &journey.title {
        let cx = w / 2.0;
        let ty = title_band / 2.0;
        let _ = write!(
            svg,
            "<text x=\"{cx:.2}\" y=\"{ty:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
             font-family=\"{family}\" font-size=\"{title_fs}\" font-weight=\"bold\" fill=\"{fill}\">{txt}</text>",
            family = escape(&opts.font_family),
            fill = rgb(opts.text_color),
            txt = escape(t),
        );
    }

    // Left "Satisfaction" axis: a rotated label plus a happy/unhappy hint at the
    // top/bottom of the score plot, so the marker height reads as a 1–5 score.
    {
        let top = marker_cy(5);
        let bottom = marker_cy(1);
        let label_x = MARGIN + fs * 0.55;
        let mid_y = (top + bottom) / 2.0;
        let _ = write!(
            svg,
            "<text x=\"{label_x:.2}\" y=\"{mid_y:.2}\" text-anchor=\"middle\" \
             dominant-baseline=\"central\" transform=\"rotate(-90 {label_x:.2} {mid_y:.2})\" \
             font-family=\"{family}\" font-size=\"{fs}\" fill=\"{fill}\">Satisfaction</text>",
            family = escape(&opts.font_family),
            fill = rgb(opts.text_color),
        );
        // "5" near the top, "1" near the bottom of the score span.
        let tick_x = MARGIN + fs * 1.4;
        for (val, ty) in [(5, top), (1, bottom)] {
            let _ = write!(
                svg,
                "<text x=\"{tick_x:.2}\" y=\"{ty:.2}\" text-anchor=\"middle\" \
                 dominant-baseline=\"central\" font-family=\"{family}\" font-size=\"{fs}\" \
                 fill=\"{fill}\">{val}</text>",
                family = escape(&opts.font_family),
                fill = rgb(opts.text_color),
            );
        }
    }

    // Section header bands: each section spans from its first to its last task.
    // Compute the contiguous [first,last] task index range for each section.
    for (si, name) in journey.sections.iter().enumerate() {
        let indices: Vec<usize> = journey
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.section == Some(si))
            .map(|(i, _)| i)
            .collect();
        if indices.is_empty() {
            continue;
        }
        let first = *indices.first().unwrap();
        let last = *indices.last().unwrap();
        let x0 = plot_left + TASK_W * first as f32 + 2.0;
        let x1 = plot_left + TASK_W * (last as f32 + 1.0) - 2.0;
        let bw = (x1 - x0).max(1.0);
        let [r, g, b] = section_color(si);
        let _ = write!(
            svg,
            "<rect x=\"{x0:.2}\" y=\"{section_y:.2}\" width=\"{bw:.2}\" height=\"{SECTION_BAND_H}\" \
             rx=\"4\" ry=\"4\" fill=\"rgb({r},{g},{b})\" stroke=\"none\"/>",
        );
        let bcx = (x0 + x1) / 2.0;
        let bcy = section_y + SECTION_BAND_H / 2.0;
        let _ = write!(
            svg,
            "<text x=\"{bcx:.2}\" y=\"{bcy:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
             font-family=\"{family}\" font-size=\"{fs}\" font-weight=\"bold\" fill=\"{fill}\">{txt}</text>",
            family = escape(&opts.font_family),
            fill = rgb(opts.text_color),
            txt = escape(name),
        );
    }

    // The journey path: a polyline through consecutive task markers.
    if n >= 2 {
        let mut pts = String::new();
        for (i, t) in journey.tasks.iter().enumerate() {
            let _ = write!(pts, "{:.2},{:.2} ", task_cx(i), marker_cy(t.score));
        }
        let _ = write!(
            svg,
            "<polyline points=\"{p}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"{STROKE_W}\"{op}/>",
            p = pts.trim_end(),
            stroke = rgb(opts.edge_stroke),
            op = opacity_attr("stroke-opacity", opts.edge_stroke),
        );
    }

    // Each task: name label, marker, score number, actor dots.
    for (i, t) in journey.tasks.iter().enumerate() {
        let cx = task_cx(i);
        let cy = marker_cy(t.score);

        // Task name above the plot region (so it doesn't collide with markers).
        let _ = write!(
            svg,
            "<text x=\"{cx:.2}\" y=\"{name_y:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
             font-family=\"{family}\" font-size=\"{fs}\" fill=\"{fill}\">{txt}</text>",
            family = escape(&opts.font_family),
            fill = rgb(opts.text_color),
            txt = escape(&t.name),
        );

        // Marker: a circle colored by score.
        let [r, g, b] = score_color(t.score);
        let _ = write!(
            svg,
            "<circle cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{MARKER_R}\" fill=\"rgb({r},{g},{b})\" \
             stroke=\"{stroke}\" stroke-width=\"{STROKE_W}\"/>",
            stroke = rgb(opts.node_stroke),
        );

        // Numeric score below the plot region.
        let _ = write!(
            svg,
            "<text x=\"{cx:.2}\" y=\"{score_y:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
             font-family=\"{family}\" font-size=\"{fs}\" font-weight=\"bold\" fill=\"{fill}\">{score}</text>",
            family = escape(&opts.font_family),
            fill = rgb(opts.text_color),
            score = t.score,
        );

        // Actor rows: a small dot + initials, then the full actor name to the
        // right. One row per actor, stacked downward.
        for (ai, actor) in t.actors.iter().enumerate() {
            let ay = actors_top + actor_row_h * ai as f32;
            // Actor dot color cycles through the section palette for variety.
            let [ar, ag, ab] = section_color(ai + 3);
            let dot_x = cx - text_size(actor, fs).0 / 2.0 - ACTOR_R;
            let _ = write!(
                svg,
                "<circle cx=\"{dot_x:.2}\" cy=\"{ay:.2}\" r=\"{ACTOR_R}\" fill=\"rgb({ar},{ag},{ab})\" \
                 stroke=\"{stroke}\" stroke-width=\"1\"/>",
                stroke = rgb(opts.node_stroke),
            );
            let label_x = dot_x + ACTOR_R + 3.0;
            let _ = write!(
                svg,
                "<text x=\"{label_x:.2}\" y=\"{ay:.2}\" text-anchor=\"start\" dominant-baseline=\"central\" \
                 font-family=\"{family}\" font-size=\"{small_fs:.2}\" fill=\"{fill}\">{txt}</text>",
                family = escape(&opts.font_family),
                small_fs = fs * 0.85,
                fill = rgb(opts.text_color),
                txt = escape(actor),
            );
        }
    }

    svg.push_str("</svg>");

    Ok(MermaidRender { svg, width_px: w, height_px: h })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"journey
    title My working day
    section Go to work
      Make tea: 5: Me
      Go upstairs: 3: Me
      Do work: 1: Me, Cat
    section Go home
      Go downstairs: 5: Me
      Sit down: 3: Me
"#;

    #[test]
    fn parses_title_sections_and_tasks() {
        let j = parse_journey(SAMPLE).expect("parse");
        assert_eq!(j.title.as_deref(), Some("My working day"));
        assert_eq!(j.sections, vec!["Go to work", "Go home"]);
        assert_eq!(j.tasks.len(), 5);
        assert_eq!(j.tasks[0].name, "Make tea");
        assert_eq!(j.tasks[0].score, 5);
        assert_eq!(j.tasks[0].actors, vec!["Me"]);
        assert_eq!(j.tasks[0].section, Some(0));
    }

    #[test]
    fn parses_multiple_actors() {
        let j = parse_journey(SAMPLE).expect("parse");
        let do_work = j.tasks.iter().find(|t| t.name == "Do work").unwrap();
        assert_eq!(do_work.score, 1);
        assert_eq!(do_work.actors, vec!["Me", "Cat"]);
        assert_eq!(do_work.section, Some(0));
    }

    #[test]
    fn task_assigned_to_current_section() {
        let j = parse_journey(SAMPLE).expect("parse");
        let sit = j.tasks.iter().find(|t| t.name == "Sit down").unwrap();
        assert_eq!(sit.section, Some(1));
    }

    #[test]
    fn empty_actor_list_ok() {
        let j = parse_journey("journey\nsection S\n  Walk: 4:\n").expect("parse");
        assert_eq!(j.tasks.len(), 1);
        assert!(j.tasks[0].actors.is_empty());
        assert_eq!(j.tasks[0].score, 4);
    }

    #[test]
    fn score_clamped_to_1_5() {
        let j = parse_journey("journey\n  Big: 9: A\n  Small: 0: B\n").expect("parse");
        assert_eq!(j.tasks[0].score, 5);
        assert_eq!(j.tasks[1].score, 1);
    }

    #[test]
    fn task_before_section_has_no_section() {
        let j = parse_journey("journey\n  Loose: 3: A\nsection S\n  Tight: 2: B\n").expect("parse");
        assert_eq!(j.tasks[0].section, None);
        assert_eq!(j.tasks[1].section, Some(0));
    }

    #[test]
    fn ignores_comments_and_blanks() {
        let src = "journey\n%% a comment\n\nsection S %% inline\n  T: 3: A\n";
        let j = parse_journey(src).expect("parse");
        assert_eq!(j.sections, vec!["S"]);
        assert_eq!(j.tasks.len(), 1);
    }

    #[test]
    fn render_well_formed_svg() {
        let r = render_journey(SAMPLE, &MermaidOptions::default()).expect("render");
        assert!(r.svg.starts_with("<svg"));
        assert!(r.svg.trim_end().ends_with("</svg>"));
        assert!(r.svg.contains("viewBox="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn render_has_section_bands() {
        let r = render_journey(SAMPLE, &MermaidOptions::default()).expect("render");
        // Two section bands, drawn as rects.
        assert!(r.svg.matches("<rect").count() >= 2, "expected >=2 section bands");
        assert!(r.svg.contains("Go to work"));
        assert!(r.svg.contains("Go home"));
    }

    #[test]
    fn render_one_marker_per_task() {
        let r = render_journey(SAMPLE, &MermaidOptions::default()).expect("render");
        // 5 task markers; actor dots are additional circles, so count is >= 5.
        // Each task has at least one marker circle; verify the markers exist by
        // counting circles >= tasks.
        let circles = r.svg.matches("<circle").count();
        assert!(circles >= 5, "expected >=5 circles (markers), got {circles}");
    }

    #[test]
    fn render_shows_scores_and_actors() {
        let r = render_journey(SAMPLE, &MermaidOptions::default()).expect("render");
        // Task names present.
        assert!(r.svg.contains("Make tea"));
        // Actor label present.
        assert!(r.svg.contains("Cat"));
        // A journey path line connecting tasks.
        assert!(r.svg.contains("<polyline"));
    }

    #[test]
    fn score_reflected_in_vertical_position() {
        // Two tasks, score 5 then score 1: the high-score marker must sit higher
        // (smaller cy) than the low-score one.
        let src = "journey\nsection S\n  High: 5: A\n  Low: 1: A\n";
        let r = render_journey(src, &MermaidOptions::default()).expect("render");
        // Pull the cy of each circle marker in document order. The first two
        // circles are the High then Low markers (path is a polyline, not circle).
        let cys: Vec<f32> = r
            .svg
            .match_indices("<circle")
            .filter_map(|(idx, _)| {
                let seg = &r.svg[idx..];
                let cyi = seg.find("cy=\"")? + 4;
                let end = seg[cyi..].find('"')? + cyi;
                seg[cyi..end].parse::<f32>().ok()
            })
            .collect();
        assert!(cys.len() >= 2);
        assert!(cys[0] < cys[1], "score-5 marker should be higher than score-1");
    }

    #[test]
    fn xml_escapes_text() {
        let src = "journey\ntitle A & B <x>\nsection S & T\n  Do <it>: 3: M&e\n";
        let r = render_journey(src, &MermaidOptions::default()).expect("render");
        assert!(r.svg.contains("A &amp; B &lt;x&gt;"));
        assert!(r.svg.contains("S &amp; T"));
        assert!(r.svg.contains("Do &lt;it&gt;"));
        assert!(r.svg.contains("M&amp;e"));
        assert!(!r.svg.contains("A & B"));
    }

    #[test]
    fn empty_input_errors() {
        match render_journey("", &MermaidOptions::default()) {
            Err(MermaidError::Parse(_)) => {}
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn header_only_is_empty() {
        let r = render_journey("journey\ntitle Nothing\n", &MermaidOptions::default());
        assert!(matches!(r, Err(MermaidError::Empty)));
    }

    #[test]
    fn missing_header_errors() {
        let r = render_journey("graph TD\nA-->B\n", &MermaidOptions::default());
        assert!(matches!(r, Err(MermaidError::Parse(_))));
    }

    #[test]
    fn deterministic_output() {
        let opts = MermaidOptions::default();
        let a = render_journey(SAMPLE, &opts).expect("a");
        let b = render_journey(SAMPLE, &opts).expect("b");
        assert_eq!(a.svg, b.svg);
        assert_eq!(a.width_px, b.width_px);
        assert_eq!(a.height_px, b.height_px);
    }
}
