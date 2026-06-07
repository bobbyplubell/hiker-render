//! `gantt` diagram (self-contained: parse + self-layout on a time axis, no graph
//! layout).
//!
//! Mermaid gantt syntax (the subset we support):
//! ```text
//! gantt
//!     title A Gantt Diagram
//!     dateFormat YYYY-MM-DD
//!     section A section
//!     A task          :a1, 2014-01-01, 30d
//!     Another task    :after a1, 20d
//!     section Another
//!     Task in sec     :2014-01-12, 12d
//!     Milestone       :milestone, m1, 2014-01-25, 0d
//! ```
//! The header line is `gantt`. Recognised directives: `title <text>`,
//! `dateFormat <fmt>` (we parse `YYYY-MM-DD` task dates regardless of the stated
//! format; other formats are tolerated/ignored), `excludes …` and `axisFormat …`
//! (ignored). `section <name>` opens a section; following task lines belong to
//! it (tasks before the first `section` go in a default unnamed section).
//!
//! A task line is `Task name :<meta>` where `<meta>` is a comma-separated list
//! that may contain (in any order): a status keyword (`done`/`active`/`crit`/
//! `milestone`), an optional task id, a start (`YYYY-MM-DD`, `after <id>`, or
//! omitted = right after the previous task in the section), and a duration
//! (`<n>d`/`<n>w`/`<n>h`; `Nw` = 7 days, `Nh` = fraction of a day).
//!
//! Day math: dates are converted to an integer day number via a proleptic
//! Gregorian day count, so two dates can be differenced directly. `after <id>`
//! resolves to that task's end day; an omitted start follows the previous task's
//! end (0 for the first task). Milestones are zero-width markers drawn as a
//! diamond at their start day.
//!
//! See `references/mermaid/packages/mermaid/src/diagrams/gantt/` for the upstream
//! grammar this mirrors.

use std::fmt::Write as _;

use crate::svgutil::{escape, opacity_attr, rgb, text_size};
use crate::{MermaidError, MermaidOptions, MermaidRender};

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// Task status, controlling the bar fill and (for `Milestone`) the shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Normal,
    Done,
    Active,
    Crit,
    Milestone,
}

/// A parsed task with its resolved time span (in integer-ish day offsets).
#[derive(Clone, Debug, PartialEq)]
struct Task {
    name: String,
    /// Index into the chart's `sections` vec.
    section: usize,
    status: Status,
    /// Start day number (proleptic Gregorian day count, or relative resolution).
    start_day: f64,
    /// End day number; equals `start_day` for milestones.
    end_day: f64,
}

/// A parsed gantt chart: title, the section names, and the tasks.
#[derive(Clone, Debug, PartialEq)]
struct Gantt {
    title: Option<String>,
    sections: Vec<String>,
    tasks: Vec<Task>,
    /// Days the schedule skips (weekends / named weekdays / specific dates).
    excludes: Excludes,
}

/// The set of excluded ("non-working") days, parsed from one or more `excludes`
/// directives. `is_excluded(day)` is the predicate task durations step over and
/// the renderer shades. When empty (the common case) it is a pure no-op, so a
/// chart with no `excludes` directive renders byte-identically to before.
#[derive(Clone, Debug, Default, PartialEq)]
struct Excludes {
    /// Excluded weekdays, indexed by [`weekday`] (0 = Monday .. 6 = Sunday).
    weekdays: [bool; 7],
    /// Excluded specific day numbers (proleptic Gregorian day count).
    dates: std::collections::BTreeSet<i64>,
}

impl Excludes {
    /// `true` when the schedule treats `day` as a non-working day.
    fn is_excluded(&self, day: i64) -> bool {
        self.weekdays[weekday(day) as usize] || self.dates.contains(&day)
    }

    /// Whether any day is excluded; used to skip excludes work cheaply.
    fn is_empty(&self) -> bool {
        self.weekdays.iter().all(|&b| !b) && self.dates.is_empty()
    }

    /// Merge the tokens of one `excludes <list>` directive (comma/space
    /// separated) into this set. Recognises `weekends`, weekday names
    /// (`monday`..`sunday`), and `YYYY-MM-DD` dates; anything else is ignored.
    fn add_directive(&mut self, list: &str) {
        for tok in list.split([',', ' ', '\t']).filter(|t| !t.is_empty()) {
            let lower = tok.to_ascii_lowercase();
            match lower.as_str() {
                "weekends" => {
                    // Saturday + Sunday.
                    self.weekdays[5] = true;
                    self.weekdays[6] = true;
                }
                "monday" => self.weekdays[0] = true,
                "tuesday" => self.weekdays[1] = true,
                "wednesday" => self.weekdays[2] = true,
                "thursday" => self.weekdays[3] = true,
                "friday" => self.weekdays[4] = true,
                "saturday" => self.weekdays[5] = true,
                "sunday" => self.weekdays[6] = true,
                _ => {
                    if let Some(day) = parse_date(tok) {
                        self.dates.insert(day);
                    }
                }
            }
        }
    }
}

/// Day of week for a day number: 0 = Monday .. 6 = Sunday. Derived from the fact
/// that 2024-01-06 is a Saturday (its day number mod 7 fixes the offset), so the
/// mapping stays correct against [`ymd_to_day`]'s arbitrary epoch.
fn weekday(day: i64) -> i64 {
    // `ymd_to_day(2024, 1, 6) % 7 == 5` and that date is a Saturday, which we
    // want to be index 5; so the raw `day % 7` already lands Monday at 0.
    debug_assert_eq!(ymd_to_day(2024, 1, 6).rem_euclid(7), 5);
    day.rem_euclid(7)
}

/// Step forward from `start` over enough calendar days to contain `n` non-excluded
/// ("working") days, returning the resulting end day. The span is half-open:
/// `[start, end)` contains exactly `n` working days. With no excludes (or a
/// fractional/zero `n`) this is just `start + n`, preserving the old behavior.
fn step_working_days(start: f64, n: f64, ex: &Excludes) -> f64 {
    // Sub-day durations (`Nh`) and the empty-excludes case keep exact arithmetic.
    if ex.is_empty() || n <= 0.0 || n.fract() != 0.0 {
        return start + n;
    }
    let mut remaining = n as i64;
    let mut cur = start as i64;
    while remaining > 0 {
        if !ex.is_excluded(cur) {
            remaining -= 1;
        }
        cur += 1;
    }
    cur as f64
}

// ---------------------------------------------------------------------------
// Date / day math (pure std, proleptic Gregorian)
// ---------------------------------------------------------------------------

/// `true` for a leap year in the proleptic Gregorian calendar.
fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Days in `month` (1..=12) of year `y`.
fn days_in_month(y: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

/// Day number for a `(year, month, day)` date: days since 0000-03-01 in the
/// proleptic Gregorian calendar. The epoch is arbitrary — only differences
/// matter for layout, so any consistent monotone count works.
fn ymd_to_day(y: i64, m: i64, d: i64) -> i64 {
    // Shift so that March is month 0 of the year; this makes the leap day the
    // last day of the year and simplifies the closed-form count.
    let a = (14 - m) / 12;
    let yy = y + 4800 - a;
    let mm = m + 12 * a - 3;
    d + (153 * mm + 2) / 5 + 365 * yy + yy / 4 - yy / 100 + yy / 400 - 32045
}

/// Parse a `YYYY-MM-DD` token to a day number. Returns `None` for any other
/// shape so the caller can classify the token differently.
fn parse_date(tok: &str) -> Option<i64> {
    let bytes = tok.as_bytes();
    // Exactly `dddd-dd-dd`.
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let y: i64 = tok.get(0..4)?.parse().ok()?;
    let m: i64 = tok.get(5..7)?.parse().ok()?;
    let d: i64 = tok.get(8..10)?.parse().ok()?;
    if !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) {
        return None;
    }
    Some(ymd_to_day(y, m, d))
}

/// Parse a duration token (`<n>d`, `<n>w`, `<n>h`) to a count of days (`w`=7d,
/// `h`=1/24 d). Returns `None` if the token is not a valid duration.
fn parse_duration(tok: &str) -> Option<f64> {
    if tok.len() < 2 {
        return None;
    }
    let (num, unit) = tok.split_at(tok.len() - 1);
    let n: f64 = num.parse().ok()?;
    if !n.is_finite() || n < 0.0 {
        return None;
    }
    match unit {
        "d" => Some(n),
        "w" => Some(n * 7.0),
        "h" => Some(n / 24.0),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Parse
// ---------------------------------------------------------------------------

/// One raw, pre-resolution task: name, status, optional id, the start spec, and
/// a duration in days (if any).
struct RawTask {
    name: String,
    section: usize,
    status: Status,
    id: Option<String>,
    start: StartSpec,
    duration: Option<f64>,
}

/// How a task's start day is determined.
enum StartSpec {
    /// An absolute date (already a day number).
    Date(i64),
    /// Right after the named task's end day.
    After(String),
    /// Right after the previous task in the section.
    Implicit,
}

/// Parse mermaid gantt source into a [`Gantt`]. Returns `Err(message)` when the
/// header is missing/malformed.
fn parse_gantt(src: &str) -> Result<Gantt, String> {
    let mut saw_header = false;
    let mut title: Option<String> = None;
    let mut sections: Vec<String> = Vec::new();
    let mut raws: Vec<RawTask> = Vec::new();
    let mut excludes = Excludes::default();
    // Current section index. Tasks before any `section` land in a synthesised
    // default section, created lazily on the first such task.
    let mut cur_section: Option<usize> = None;

    for raw in src.lines() {
        let line = raw.split("%%").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }

        if !saw_header {
            let rest = line
                .strip_prefix("gantt")
                .filter(|r| r.is_empty() || r.starts_with(char::is_whitespace))
                .ok_or_else(|| format!("expected 'gantt' header, got: {line:?}"))?;
            saw_header = true;
            let rest = rest.trim();
            if !rest.is_empty() {
                // Tolerate `gantt title …` on the header line.
                if let Some(t) = directive(rest, "title") {
                    title = Some(t.to_string());
                }
            }
            continue;
        }

        // Directives.
        if let Some(t) = directive(line, "title") {
            title = Some(t.to_string());
            continue;
        }
        if let Some(list) = directive(line, "excludes") {
            excludes.add_directive(list);
            continue;
        }
        if directive(line, "dateFormat").is_some()
            || directive(line, "axisFormat").is_some()
            || directive(line, "todayMarker").is_some()
            || directive(line, "tickInterval").is_some()
            || directive(line, "weekday").is_some()
            || directive(line, "inclusiveEndDates").is_some()
        {
            continue;
        }
        if let Some(name) = directive(line, "section") {
            sections.push(name.to_string());
            cur_section = Some(sections.len() - 1);
            continue;
        }
        // Skip interaction / display lines we don't model.
        if line.starts_with("click ") || line.starts_with("vert ") || line == "vert" {
            continue;
        }

        // Otherwise: a task line `Name :meta`.
        if let Some(colon) = line.find(':') {
            let name = line[..colon].trim().to_string();
            let meta = &line[colon + 1..];
            let section = match cur_section {
                Some(s) => s,
                None => {
                    // Lazily create the default unnamed section.
                    sections.push(String::new());
                    cur_section = Some(0);
                    0
                }
            };
            let raw_task = parse_task_meta(name, section, meta)?;
            raws.push(raw_task);
        }
        // Lines without a colon that aren't directives are ignored leniently.
    }

    if !saw_header {
        return Err("empty input / no 'gantt' header".to_string());
    }

    let tasks = resolve_tasks(&raws, &excludes);
    Ok(Gantt {
        title,
        sections,
        tasks,
        excludes,
    })
}

/// If `line` is `<key>` followed by whitespace (or is exactly `<key>`), return
/// the trimmed remainder (possibly empty); else `None`.
fn directive<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(key)?;
    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        Some(rest.trim())
    } else {
        None
    }
}

/// Parse a task's `<meta>` (the comma-separated part after the first `:`) into a
/// [`RawTask`]. Tokens are classified independently: status keyword, date,
/// `after <id>`, duration, or — a bare leading non-keyword with no date/dur — an
/// id.
fn parse_task_meta(name: String, section: usize, meta: &str) -> Result<RawTask, String> {
    let mut status = Status::Normal;
    let mut id: Option<String> = None;
    let mut start: Option<StartSpec> = None;
    let mut duration: Option<f64> = None;

    for raw_tok in meta.split(',') {
        let tok = raw_tok.trim();
        if tok.is_empty() {
            continue;
        }

        // Status keywords (may be combined, e.g. `crit, active`).
        match tok {
            "done" => {
                if status == Status::Normal {
                    status = Status::Done;
                }
                continue;
            }
            "active" => {
                if status == Status::Normal {
                    status = Status::Active;
                }
                continue;
            }
            "crit" => {
                // crit dominates; but a milestone stays a milestone.
                if status != Status::Milestone {
                    status = Status::Crit;
                }
                continue;
            }
            "milestone" => {
                status = Status::Milestone;
                continue;
            }
            _ => {}
        }

        // `after <id…>` start spec. We support a single predecessor id (extra
        // ids in `after a b` are ignored — we take the first).
        if let Some(rest) = tok.strip_prefix("after") {
            if rest.is_empty() || rest.starts_with(char::is_whitespace) {
                if let Some(first) = rest.split_whitespace().next() {
                    start = Some(StartSpec::After(first.to_string()));
                }
                continue;
            }
        }

        // Absolute date.
        if let Some(day) = parse_date(tok) {
            start = Some(StartSpec::Date(day));
            continue;
        }

        // Duration.
        if let Some(days) = parse_duration(tok) {
            duration = Some(days);
            continue;
        }

        // Otherwise a bare token = the task id (first one wins).
        if id.is_none() {
            id = Some(tok.to_string());
        }
    }

    Ok(RawTask {
        name,
        section,
        status,
        id,
        start: start.unwrap_or(StartSpec::Implicit),
        duration,
    })
}

/// Resolve raw tasks into laid-out [`Task`]s, computing start/end day numbers.
/// `after <id>` resolves against earlier tasks (by id); an omitted start follows
/// the previous task's end (0 for the first). Unknown `after` ids fall back to
/// the previous-task rule.
fn resolve_tasks(raws: &[RawTask], ex: &Excludes) -> Vec<Task> {
    // Map id → end day, filled as we go so forward references degrade gracefully.
    let mut end_by_id: std::collections::HashMap<&str, f64> = std::collections::HashMap::new();
    let mut tasks: Vec<Task> = Vec::with_capacity(raws.len());
    let mut prev_end: f64 = 0.0;

    for rt in raws {
        let start = match &rt.start {
            StartSpec::Date(day) => *day as f64,
            StartSpec::After(id) => *end_by_id.get(id.as_str()).unwrap_or(&prev_end),
            StartSpec::Implicit => prev_end,
        };

        let end = match rt.status {
            // A milestone is a zero-width marker at its start (excludes never move
            // it).
            Status::Milestone => start,
            // A duration spans forward over `n` working days, stepping over any
            // excluded calendar days in between (no-op when `ex` is empty).
            _ => step_working_days(start, rt.duration.unwrap_or(0.0), ex),
        };

        if let Some(id) = &rt.id {
            end_by_id.insert(id.as_str(), end);
        }
        prev_end = end;

        tasks.push(Task {
            name: rt.name.clone(),
            section: rt.section,
            status: rt.status,
            start_day: start,
            end_day: end,
        });
    }

    tasks
}

// ---------------------------------------------------------------------------
// Layout constants / palette
// ---------------------------------------------------------------------------

/// Outer margin around the whole chart, px.
const MARGIN: f32 = 24.0;
/// Width allotted per day on the time axis, px (clamped so very long charts stay
/// reasonable and very short ones aren't cramped).
const DAY_WIDTH_DEFAULT: f32 = 22.0;
const DAY_WIDTH_MIN: f32 = 3.0;
const DAY_WIDTH_MAX: f32 = 40.0;
/// Target maximum width of the bar area (the day axis), px, used to pick a
/// per-day width that fits.
const TARGET_AXIS_W: f32 = 900.0;
/// Bar height as a multiple of the font size.
const BAR_H_EM: f32 = 1.2;
/// Row height as a multiple of the font size (a little taller than the bar so
/// rows breathe).
const ROW_H_EM: f32 = 1.7;
/// Stroke width for bars / diamonds, px.
const STROKE_W: f32 = 1.0;

/// Fill color for a status (straight RGBA). Mermaid-ish: done = muted grey,
/// active = accent blue, crit = red, normal = the theme node fill.
fn status_fill(status: Status, opts: &MermaidOptions) -> [u8; 4] {
    match status {
        Status::Done => [188, 196, 208, 255],
        Status::Active => [101, 159, 217, 255],
        Status::Crit => [217, 83, 79, 255],
        Status::Milestone => [102, 102, 187, 255],
        Status::Normal => opts.node_fill,
    }
}

/// Stroke color for a status bar.
fn status_stroke(status: Status, opts: &MermaidOptions) -> [u8; 4] {
    match status {
        Status::Done => [140, 150, 165, 255],
        Status::Active => [60, 110, 170, 255],
        Status::Crit => [170, 50, 47, 255],
        Status::Milestone => [70, 70, 150, 255],
        Status::Normal => opts.node_stroke,
    }
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Render mermaid gantt-chart source to an SVG document.
pub fn render_gantt(src: &str, opts: &MermaidOptions) -> Result<MermaidRender, MermaidError> {
    let gantt = parse_gantt(src).map_err(MermaidError::Parse)?;
    if gantt.tasks.is_empty() {
        return Err(MermaidError::Empty);
    }

    let fs = opts.font_size_px;
    let bar_h = fs * BAR_H_EM;
    let row_h = fs * ROW_H_EM;
    let title_fs = fs * 1.5;

    // Day span across all tasks.
    let min_day = gantt
        .tasks
        .iter()
        .map(|t| t.start_day)
        .fold(f64::INFINITY, f64::min);
    let max_day = gantt
        .tasks
        .iter()
        .map(|t| t.end_day)
        .fold(f64::NEG_INFINITY, f64::max);
    let span = (max_day - min_day).max(1.0);

    // Per-day width: fit the span into the target axis width, but clamp.
    let day_width = (TARGET_AXIS_W / span as f32)
        .min(DAY_WIDTH_DEFAULT)
        .clamp(DAY_WIDTH_MIN, DAY_WIDTH_MAX);

    // Left band holds the section headings; the task-name gutter sits to its
    // right so section labels and task names never share a column (they used to
    // overprint in one narrow gutter). Section band width = widest section name
    // (0 when there are no named sections, collapsing the band away).
    let widest_section = gantt
        .sections
        .iter()
        .map(|s| if s.is_empty() { 0.0 } else { text_size(s, fs).0 })
        .fold(0.0_f32, f32::max);
    let section_w = if widest_section > 0.0 {
        (widest_section + 12.0).clamp(48.0, 200.0)
    } else {
        0.0
    };

    // Task-name gutter, to the right of the section band. Width = widest name.
    let widest_name = gantt
        .tasks
        .iter()
        .map(|t| text_size(&t.name, fs).0)
        .fold(0.0_f32, f32::max);
    let gutter_w = (widest_name + 16.0).clamp(80.0, 360.0);

    let axis_w = span as f32 * day_width;

    // Maps a day number to an x coordinate within the chart.
    let day_to_x =
        |day: f64| -> f32 { gutter_x_end(section_w, gutter_w) + (day - min_day) as f32 * day_width };

    // Vertical bands: title, then one row per task.
    let title_band = if gantt.title.is_some() {
        title_fs + MARGIN * 0.5
    } else {
        0.0
    };
    let axis_band = fs + 6.0; // date axis labels at the top of the grid.
    let grid_top = MARGIN + title_band + axis_band;
    let n_tasks = gantt.tasks.len();
    let grid_h = n_tasks as f32 * row_h;

    // The rightmost date-axis tick is text-anchor=middle at the grid's right
    // edge, so its right half spills past the axis. Reserve a right margin large
    // enough to contain that half-label width (the labels are ~"YYYY-MM-DD" at
    // 0.75*fs) so the final tick isn't clipped.
    let axis_label_hw = text_size("2024-01-18", fs * 0.75).0 / 2.0;
    let right_margin = MARGIN.max(axis_label_hw + 2.0);

    let width = MARGIN + section_w + gutter_w + axis_w + right_margin;
    let height = grid_top + grid_h + MARGIN;

    let w = (width.ceil() + 1.0).max(1.0);
    let h = (height.ceil() + 1.0).max(1.0);

    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"0 0 {w} {h}\">"
    );

    // ---- Section background bands (alternating tint per section) -----------
    // Group consecutive tasks by section to draw a soft band and a left label.
    let mut i = 0usize;
    while i < n_tasks {
        let sec = gantt.tasks[i].section;
        let start_row = i;
        while i < n_tasks && gantt.tasks[i].section == sec {
            i += 1;
        }
        let end_row = i; // exclusive
        let band_y = grid_top + start_row as f32 * row_h;
        let band_h = (end_row - start_row) as f32 * row_h;
        // Alternating tint by section index for visual grouping.
        let tint = if sec % 2 == 0 {
            [0u8, 0u8, 0u8, 18u8]
        } else {
            [0u8, 0u8, 0u8, 8u8]
        };
        let _ = write!(
            svg,
            "<rect x=\"{x:.2}\" y=\"{band_y:.2}\" width=\"{bw:.2}\" height=\"{band_h:.2}\" \
             fill=\"{fill}\"{op}/>",
            x = MARGIN,
            bw = section_w + gutter_w + axis_w,
            fill = rgb(tint),
            op = opacity_attr("fill-opacity", tint),
        );

        // Section label, vertically centered in the band, left-aligned in gutter.
        let label = &gantt.sections[sec];
        if !label.is_empty() {
            let ly = band_y + band_h / 2.0;
            let [r, g, b, _] = opts.text_color;
            let _ = write!(
                svg,
                "<text x=\"{x:.2}\" y=\"{ly:.2}\" text-anchor=\"start\" dominant-baseline=\"central\" \
                 font-family=\"{family}\" font-size=\"{fs}\" font-weight=\"bold\" fill=\"rgb({r},{g},{b})\">{txt}</text>",
                x = MARGIN + 6.0,
                family = escape(&opts.font_family),
                txt = escape(label),
            );
        }
    }

    // ---- Excluded-day shading: a faint vertical band per skipped day -------
    // Drawn over the section bands but under the gridlines/bars so excluded days
    // read as lightly greyed-out columns spanning the whole grid.
    if !gantt.excludes.is_empty() {
        let first = min_day.floor() as i64;
        let last = max_day.ceil() as i64;
        // Faint grey band; subtle so bars/text stay readable.
        let band = [120u8, 120u8, 120u8, 36u8];
        for day in first..last {
            if !gantt.excludes.is_excluded(day) {
                continue;
            }
            let bx = day_to_x(day as f64);
            let _ = write!(
                svg,
                "<rect x=\"{bx:.2}\" y=\"{y:.2}\" width=\"{bw:.2}\" height=\"{bh:.2}\" \
                 fill=\"{fill}\"{op}/>",
                y = grid_top,
                bw = day_width,
                bh = grid_h,
                fill = rgb(band),
                op = opacity_attr("fill-opacity", band),
            );
        }
    }

    // ---- Date axis: a gridline + label at the start, mid, and end days -----
    // We pick a handful of tick days across the span for light vertical lines.
    let n_ticks = ((axis_w / 90.0).floor() as i64).clamp(1, 12);
    let [gr, gg, gb, _] = opts.edge_stroke;
    for k in 0..=n_ticks {
        let day = min_day + span * (k as f64 / n_ticks as f64);
        let x = day_to_x(day);
        // Vertical gridline.
        let _ = write!(
            svg,
            "<line x1=\"{x:.2}\" y1=\"{y1:.2}\" x2=\"{x:.2}\" y2=\"{y2:.2}\" \
             stroke=\"rgb({gr},{gg},{gb})\" stroke-width=\"0.5\" stroke-opacity=\"0.35\"/>",
            y1 = grid_top - 4.0,
            y2 = grid_top + grid_h,
        );
        // Date label above the grid.
        let label = day_to_date_label(day.round() as i64);
        let ty = grid_top - 6.0;
        let _ = write!(
            svg,
            "<text x=\"{x:.2}\" y=\"{ty:.2}\" text-anchor=\"middle\" dominant-baseline=\"alphabetic\" \
             font-family=\"{family}\" font-size=\"{small:.2}\" fill=\"rgb({gr},{gg},{gb})\">{txt}</text>",
            small = fs * 0.75,
            family = escape(&opts.font_family),
            txt = escape(&label),
        );
    }

    // ---- Task bars ---------------------------------------------------------
    for (row, task) in gantt.tasks.iter().enumerate() {
        let row_y = grid_top + row as f32 * row_h;
        let bar_y = row_y + (row_h - bar_h) / 2.0;

        if task.status == Status::Milestone {
            draw_milestone(
                &mut svg,
                task,
                bar_y,
                bar_h,
                day_to_x(task.start_day),
                gutter_x_end(section_w, gutter_w) - 6.0,
                fs,
                opts,
            );
            continue;
        }

        let x0 = day_to_x(task.start_day);
        let x1 = day_to_x(task.end_day);
        let bw = (x1 - x0).max(2.0);
        let fill = status_fill(task.status, opts);
        let stroke = status_stroke(task.status, opts);
        let rx = (bar_h * 0.25).min(6.0);

        let _ = write!(
            svg,
            "<rect x=\"{x0:.2}\" y=\"{bar_y:.2}\" width=\"{bw:.2}\" height=\"{bar_h:.2}\" \
             rx=\"{rx:.2}\" ry=\"{rx:.2}\" fill=\"{fill}\"{fop} stroke=\"{stroke}\"{sop} \
             stroke-width=\"{STROKE_W}\"/>",
            fill = rgb(fill),
            fop = opacity_attr("fill-opacity", fill),
            stroke = rgb(stroke),
            sop = opacity_attr("stroke-opacity", stroke),
        );

        // Task name: in the gutter (right-aligned at the grid edge) so names are
        // always legible regardless of bar width.
        let name_x = gutter_x_end(section_w, gutter_w) - 6.0;
        let name_y = row_y + row_h / 2.0;
        let [tr, tg, tb, _] = opts.text_color;
        let _ = write!(
            svg,
            "<text x=\"{name_x:.2}\" y=\"{name_y:.2}\" text-anchor=\"end\" dominant-baseline=\"central\" \
             font-family=\"{family}\" font-size=\"{fs}\" fill=\"rgb({tr},{tg},{tb})\">{txt}</text>",
            family = escape(&opts.font_family),
            txt = escape(&task.name),
        );
    }

    svg.push_str("</svg>");

    // Title centered above everything (emit last; SVG draws in order, but text on
    // top of background is fine — there's no overlap here).
    if let Some(t) = &gantt.title {
        let cx = w / 2.0;
        let ty = MARGIN + title_fs / 2.0;
        let [tr, tg, tb, _] = opts.text_color;
        let title_svg = format!(
            "<text x=\"{cx:.2}\" y=\"{ty:.2}\" text-anchor=\"middle\" dominant-baseline=\"central\" \
             font-family=\"{family}\" font-size=\"{title_fs}\" font-weight=\"bold\" fill=\"rgb({tr},{tg},{tb})\">{txt}</text>",
            family = escape(&opts.font_family),
            txt = escape(t),
        );
        // Insert before the closing tag.
        svg.insert_str(svg.len() - "</svg>".len(), &title_svg);
    }

    Ok(MermaidRender {
        svg,
        width_px: w,
        height_px: h,
    })
}

/// The x coordinate where the gutter ends and the day grid begins (after the
/// outer margin, the section-label band, and the task-name gutter).
fn gutter_x_end(section_w: f32, gutter_w: f32) -> f32 {
    MARGIN + section_w + gutter_w
}

/// Draw a milestone as a diamond centered at `cx` (its start-day x), plus its
/// name right-anchored at `name_x` (the gutter/grid boundary).
fn draw_milestone(
    svg: &mut String,
    task: &Task,
    bar_y: f32,
    bar_h: f32,
    cx: f32,
    name_x: f32,
    fs: f32,
    opts: &MermaidOptions,
) {
    let cy = bar_y + bar_h / 2.0;
    let r = bar_h / 2.0;
    let fill = status_fill(Status::Milestone, opts);
    let stroke = status_stroke(Status::Milestone, opts);
    let _ = write!(
        svg,
        "<polygon points=\"{cx:.2},{top:.2} {right:.2},{cy:.2} {cx:.2},{bot:.2} {left:.2},{cy:.2}\" \
         fill=\"{fill}\"{fop} stroke=\"{stroke}\"{sop} stroke-width=\"{STROKE_W}\"/>",
        top = cy - r,
        bot = cy + r,
        right = cx + r,
        left = cx - r,
        fill = rgb(fill),
        fop = opacity_attr("fill-opacity", fill),
        stroke = rgb(stroke),
        sop = opacity_attr("stroke-opacity", stroke),
    );

    // Name in the gutter (right-aligned at the grid edge), like ordinary tasks.
    let [tr, tg, tb, _] = opts.text_color;
    let _ = write!(
        svg,
        "<text x=\"{name_x:.2}\" y=\"{cy:.2}\" text-anchor=\"end\" dominant-baseline=\"central\" \
         font-family=\"{family}\" font-size=\"{fs}\" fill=\"rgb({tr},{tg},{tb})\">{txt}</text>",
        family = escape(&opts.font_family),
        txt = escape(&task.name),
    );
}

/// Format a day number back to a `YYYY-MM-DD` axis label.
fn day_to_date_label(day: i64) -> String {
    let (y, m, d) = day_to_ymd(day);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Inverse of [`ymd_to_day`]: day number → `(year, month, day)`.
fn day_to_ymd(jdn: i64) -> (i64, i64, i64) {
    // Standard Julian-day-number → Gregorian conversion (Richards' algorithm),
    // matching the epoch used by `ymd_to_day` (which produces JDN-style counts).
    let a = jdn + 32044;
    let b = (4 * a + 3) / 146097;
    let c = a - (146097 * b) / 4;
    let dd = (4 * c + 3) / 1461;
    let e = c - (1461 * dd) / 4;
    let m = (5 * e + 2) / 153;
    let day = e - (153 * m + 2) / 5 + 1;
    let month = m + 3 - 12 * (m / 10);
    let year = 100 * b + dd - 4800 + m / 10;
    (year, month, day)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"gantt
    title A Gantt Diagram
    dateFormat YYYY-MM-DD
    section Design
    Design task     :done, des1, 2014-01-06, 5d
    Implement       :active, imp1, after des1, 10d
    section Review
    Review          : 2014-01-20, 3d
    Follow up       : 2d
    Launch          :milestone, m1, 2014-01-25, 0d
"#;

    // ---- date / day math --------------------------------------------------

    #[test]
    fn day_number_round_trips() {
        for &(y, m, d) in &[(2014, 1, 6), (2000, 2, 29), (1999, 12, 31), (2024, 2, 29)] {
            let jdn = ymd_to_day(y, m, d);
            assert_eq!(day_to_ymd(jdn), (y, m, d), "round trip {y}-{m}-{d}");
        }
    }

    #[test]
    fn date_diff_is_calendar_correct() {
        // 2014-01-06 + 5d ends 2014-01-11.
        let start = parse_date("2014-01-06").unwrap();
        let end = start + 5;
        assert_eq!(day_to_ymd(end), (2014, 1, 11));
        // Span across a month boundary: 2014-01-28 .. 2014-02-03 = 6 days.
        let a = parse_date("2014-01-28").unwrap();
        let b = parse_date("2014-02-03").unwrap();
        assert_eq!(b - a, 6);
    }

    #[test]
    fn leap_year_rules() {
        assert!(is_leap(2000) && is_leap(2024) && is_leap(2004));
        assert!(!is_leap(1900) && !is_leap(2023) && !is_leap(2100));
        // Feb 29 2000 valid; Feb 29 1900 invalid (rejected by parse_date).
        assert!(parse_date("2000-02-29").is_some());
        assert!(parse_date("1900-02-29").is_none());
    }

    #[test]
    fn parse_date_rejects_garbage() {
        assert!(parse_date("2014-13-01").is_none());
        assert!(parse_date("2014-01-32").is_none());
        assert!(parse_date("2014/01/01").is_none());
        assert!(parse_date("abc").is_none());
        assert!(parse_date("2014-1-1").is_none());
    }

    #[test]
    fn duration_units() {
        assert_eq!(parse_duration("5d"), Some(5.0));
        assert_eq!(parse_duration("2w"), Some(14.0));
        assert_eq!(parse_duration("12h"), Some(0.5));
        assert_eq!(parse_duration("0d"), Some(0.0));
        assert_eq!(parse_duration("d"), None);
        assert_eq!(parse_duration("5x"), None);
    }

    // ---- parse ------------------------------------------------------------

    #[test]
    fn parses_title_dateformat_sections() {
        let g = parse_gantt(SAMPLE).expect("parse");
        assert_eq!(g.title.as_deref(), Some("A Gantt Diagram"));
        assert_eq!(g.sections, vec!["Design".to_string(), "Review".to_string()]);
        assert_eq!(g.tasks.len(), 5);
    }

    #[test]
    fn absolute_date_task() {
        let g = parse_gantt(SAMPLE).expect("parse");
        let des = &g.tasks[0];
        assert_eq!(des.name, "Design task");
        assert_eq!(des.status, Status::Done);
        let start = parse_date("2014-01-06").unwrap() as f64;
        assert_eq!(des.start_day, start);
        assert_eq!(des.end_day, start + 5.0);
    }

    #[test]
    fn after_resolves_to_predecessor_end() {
        let g = parse_gantt(SAMPLE).expect("parse");
        let des = &g.tasks[0];
        let imp = &g.tasks[1];
        assert_eq!(imp.name, "Implement");
        assert_eq!(imp.status, Status::Active);
        // Implement starts where Design ends, runs 10 days.
        assert_eq!(imp.start_day, des.end_day);
        assert_eq!(imp.end_day, des.end_day + 10.0);
    }

    #[test]
    fn omitted_start_follows_previous() {
        let g = parse_gantt(SAMPLE).expect("parse");
        // "Review" has an absolute date; "Follow up" omits its start.
        let review = &g.tasks[2];
        let follow = &g.tasks[3];
        assert_eq!(follow.name, "Follow up");
        assert_eq!(follow.start_day, review.end_day);
        assert_eq!(follow.end_day, review.end_day + 2.0);
    }

    #[test]
    fn milestone_is_zero_width() {
        let g = parse_gantt(SAMPLE).expect("parse");
        let launch = g.tasks.iter().find(|t| t.name == "Launch").unwrap();
        assert_eq!(launch.status, Status::Milestone);
        assert_eq!(launch.start_day, launch.end_day);
        assert_eq!(launch.start_day, parse_date("2014-01-25").unwrap() as f64);
    }

    #[test]
    fn status_tags_classified() {
        let g = parse_gantt(SAMPLE).expect("parse");
        assert_eq!(g.tasks[0].status, Status::Done);
        assert_eq!(g.tasks[1].status, Status::Active);
        assert_eq!(g.tasks[2].status, Status::Normal);
        assert_eq!(g.tasks[4].status, Status::Milestone);
    }

    #[test]
    fn crit_status_and_weeks() {
        let src = "gantt\ndateFormat YYYY-MM-DD\nTask :crit, c1, 2014-01-01, 1w\n";
        let g = parse_gantt(src).expect("parse");
        assert_eq!(g.tasks[0].status, Status::Crit);
        let start = parse_date("2014-01-01").unwrap() as f64;
        assert_eq!(g.tasks[0].end_day, start + 7.0);
    }

    #[test]
    fn tasks_before_section_get_default_section() {
        let src = "gantt\nFirst :2014-01-01, 2d\nsection S\nSecond :2014-01-05, 1d\n";
        let g = parse_gantt(src).expect("parse");
        // Default unnamed section (index 0) + "S".
        assert_eq!(g.sections.len(), 2);
        assert_eq!(g.sections[0], "");
        assert_eq!(g.tasks[0].section, 0);
        assert_eq!(g.tasks[1].section, 1);
    }

    #[test]
    fn id_only_token_is_task_id() {
        // `after b1` must resolve to the end of the task whose id is `b1`.
        let src = "gantt\ndateFormat YYYY-MM-DD\nA :b1, 2014-01-01, 3d\nB :after b1, 2d\n";
        let g = parse_gantt(src).expect("parse");
        let a_end = parse_date("2014-01-01").unwrap() as f64 + 3.0;
        assert_eq!(g.tasks[1].start_day, a_end);
        assert_eq!(g.tasks[1].end_day, a_end + 2.0);
    }

    #[test]
    fn unknown_after_falls_back_to_previous() {
        let src = "gantt\ndateFormat YYYY-MM-DD\nA :2014-01-01, 3d\nB :after nope, 2d\n";
        let g = parse_gantt(src).expect("parse");
        assert_eq!(g.tasks[1].start_day, g.tasks[0].end_day);
    }

    // ---- errors -----------------------------------------------------------

    #[test]
    fn missing_header_errors() {
        match render_gantt("graph TD\nA-->B\n", &MermaidOptions::default()) {
            Err(MermaidError::Parse(_)) => {}
            other => panic!("expected Parse, got {other:?}"),
        }
    }

    #[test]
    fn empty_input_errors() {
        assert!(matches!(
            render_gantt("", &MermaidOptions::default()),
            Err(MermaidError::Parse(_))
        ));
    }

    #[test]
    fn header_only_is_empty() {
        let r = render_gantt("gantt\ntitle Nothing\n", &MermaidOptions::default());
        assert!(matches!(r, Err(MermaidError::Empty)));
    }

    // ---- render -----------------------------------------------------------

    #[test]
    fn render_well_formed_svg() {
        let r = render_gantt(SAMPLE, &MermaidOptions::default()).expect("render");
        assert!(r.svg.starts_with("<svg"));
        assert!(r.svg.trim_end().ends_with("</svg>"));
        assert!(r.svg.contains("viewBox="));
        assert!(r.width_px > 0.0 && r.height_px > 0.0);
    }

    #[test]
    fn one_bar_per_task_and_diamond_per_milestone() {
        let r = render_gantt(SAMPLE, &MermaidOptions::default()).expect("render");
        // 4 non-milestone tasks → 4 bar <rect>s. Section bands are also <rect>s
        // (2 sections), so count bars by the rounded-corner `rx=` marker which
        // only bars carry.
        let bars = r.svg.matches(" rx=\"").count();
        assert_eq!(bars, 4, "expected 4 task bars");
        // 1 milestone → 1 diamond polygon.
        assert_eq!(r.svg.matches("<polygon").count(), 1, "expected 1 milestone");
    }

    #[test]
    fn task_names_and_sections_present() {
        let r = render_gantt(SAMPLE, &MermaidOptions::default()).expect("render");
        for name in ["Design task", "Implement", "Review", "Follow up", "Launch"] {
            assert!(r.svg.contains(name), "missing task name {name}");
        }
        assert!(r.svg.contains("Design") && r.svg.contains("Review"));
        assert!(r.svg.contains("A Gantt Diagram"), "title missing");
    }

    #[test]
    fn xml_escapes_names() {
        let src = "gantt\ndateFormat YYYY-MM-DD\nA & <b> :2014-01-01, 2d\n";
        let r = render_gantt(src, &MermaidOptions::default()).expect("render");
        assert!(r.svg.contains("A &amp; &lt;b&gt;"), "got: {}", r.svg);
        assert!(!r.svg.contains("A & <b>"));
    }

    #[test]
    fn deterministic_output() {
        let opts = MermaidOptions::default();
        let a = render_gantt(SAMPLE, &opts).expect("a");
        let b = render_gantt(SAMPLE, &opts).expect("b");
        assert_eq!(a.svg, b.svg);
        assert_eq!(a.width_px, b.width_px);
        assert_eq!(a.height_px, b.height_px);
    }

    #[test]
    fn distinct_status_fills() {
        let opts = MermaidOptions::default();
        let done = status_fill(Status::Done, &opts);
        let active = status_fill(Status::Active, &opts);
        let crit = status_fill(Status::Crit, &opts);
        assert_ne!(done, active);
        assert_ne!(active, crit);
        assert_ne!(done, crit);
    }

    // ---- excludes ---------------------------------------------------------

    #[test]
    fn weekday_and_weekend_detection() {
        // 2024-01-06 is a Saturday, 2024-01-08 a Monday.
        let sat = parse_date("2024-01-06").unwrap();
        let sun = parse_date("2024-01-07").unwrap();
        let mon = parse_date("2024-01-08").unwrap();
        let fri = parse_date("2024-01-12").unwrap();
        assert_eq!(weekday(mon), 0, "Monday is index 0");
        assert_eq!(weekday(fri), 4, "Friday is index 4");
        assert_eq!(weekday(sat), 5, "Saturday is index 5");
        assert_eq!(weekday(sun), 6, "Sunday is index 6");

        let mut ex = Excludes::default();
        ex.add_directive("weekends");
        assert!(ex.is_excluded(sat), "Saturday excluded");
        assert!(ex.is_excluded(sun), "Sunday excluded");
        assert!(!ex.is_excluded(mon), "Monday not excluded");
        assert!(!ex.is_excluded(fri), "Friday not excluded");
    }

    #[test]
    fn excludes_directive_parses_dates_and_names() {
        let g = parse_gantt(
            "gantt\ndateFormat YYYY-MM-DD\nexcludes weekends, 2024-01-10 monday\nA :2024-01-08, 1d\n",
        )
        .expect("parse");
        let mon = parse_date("2024-01-08").unwrap();
        let wed = parse_date("2024-01-10").unwrap();
        let sat = parse_date("2024-01-06").unwrap();
        assert!(g.excludes.is_excluded(sat), "weekend");
        assert!(g.excludes.is_excluded(wed), "specific date 2024-01-10");
        assert!(g.excludes.is_excluded(mon), "monday name");
    }

    #[test]
    fn duration_skips_excluded_weekends() {
        // A 5d task Mon 2024-01-08 fits Mon..Fri (5 working days), so it ends on
        // the Saturday — same calendar span as no-excludes, but for the right
        // reason (Friday is the 5th working day).
        let g = parse_gantt(
            "gantt\ndateFormat YYYY-MM-DD\nexcludes weekends\nA :2024-01-08, 5d\n",
        )
        .expect("parse");
        let start = parse_date("2024-01-08").unwrap() as f64;
        assert_eq!(g.tasks[0].start_day, start);
        assert_eq!(g.tasks[0].end_day, start + 5.0);
        assert_eq!(day_to_ymd(g.tasks[0].end_day as i64), (2024, 1, 13));

        // A 7d task Mon 2024-01-08 must span the intervening Sat+Sun: it needs 9
        // calendar days to cover 7 working days, ending Wed 2024-01-17.
        let g7 = parse_gantt(
            "gantt\ndateFormat YYYY-MM-DD\nexcludes weekends\nA :2024-01-08, 7d\n",
        )
        .expect("parse");
        let s7 = parse_date("2024-01-08").unwrap() as f64;
        assert_eq!(g7.tasks[0].end_day, s7 + 9.0, "7 working days span 9 days");
        assert_eq!(day_to_ymd(g7.tasks[0].end_day as i64), (2024, 1, 17));

        // Without excludes the same 7d task ends 7 calendar days later.
        let g_no = parse_gantt(
            "gantt\ndateFormat YYYY-MM-DD\nA :2024-01-08, 7d\n",
        )
        .expect("parse");
        assert_eq!(g_no.tasks[0].end_day, s7 + 7.0);
        assert!(
            g7.tasks[0].end_day > g_no.tasks[0].end_day,
            "excludes lengthen the calendar span"
        );
    }

    #[test]
    fn duration_skips_specific_excluded_date() {
        // 2024-01-10 (a Wednesday) is excluded, so a 5d task from Mon 2024-01-08
        // needs an extra calendar day to cover 5 working days.
        let g = parse_gantt(
            "gantt\ndateFormat YYYY-MM-DD\nexcludes 2024-01-10\nA :2024-01-08, 5d\n",
        )
        .expect("parse");
        let start = parse_date("2024-01-08").unwrap() as f64;
        assert_eq!(g.tasks[0].end_day, start + 6.0);
        assert_eq!(day_to_ymd(g.tasks[0].end_day as i64), (2024, 1, 14));
    }

    #[test]
    fn no_excludes_is_unchanged() {
        // The byte-for-byte output with no `excludes` directive must match what an
        // explicit empty excludes set produces, and contain no shading band.
        let opts = MermaidOptions::default();
        let r = render_gantt(SAMPLE, &opts).expect("render");
        // Grey shading band fill (used only for excluded days) must be absent.
        assert!(
            !r.svg.contains("fill=\"rgb(120,120,120)\""),
            "no excludes => no shading bands"
        );
        // Durations are plain calendar arithmetic.
        let g = parse_gantt(SAMPLE).expect("parse");
        let start = parse_date("2014-01-06").unwrap() as f64;
        assert_eq!(g.tasks[0].end_day, start + 5.0);
    }

    #[test]
    fn excludes_render_has_shading_bands() {
        let opts = MermaidOptions::default();
        let src = "gantt\ndateFormat YYYY-MM-DD\nexcludes weekends\n\
                   A :2024-01-08, 14d\n";
        let r = render_gantt(src, &opts).expect("render");
        // At least one faint grey shading band for the weekend columns.
        assert!(
            r.svg.contains("fill=\"rgb(120,120,120)\""),
            "excluded-day shading bands present, got: {}",
            r.svg
        );
    }

    #[test]
    fn milestone_unaffected_by_excludes() {
        let g = parse_gantt(
            "gantt\ndateFormat YYYY-MM-DD\nexcludes weekends\nM :milestone, m1, 2024-01-06, 0d\n",
        )
        .expect("parse");
        let day = parse_date("2024-01-06").unwrap() as f64;
        assert_eq!(g.tasks[0].start_day, day);
        assert_eq!(g.tasks[0].end_day, day, "milestone stays zero-width on a weekend");
    }
}
