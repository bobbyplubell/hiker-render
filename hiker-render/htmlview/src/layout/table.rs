//! Table layout (CSS 2.1 automatic table layout).
//!
//! Ports the load-bearing parts of litehtml's `table.cpp` / `render_table.cpp`
//! onto this crate's arena `LayoutTree`:
//!
//! 1. Grid construction from the box subtree (row groups -> rows -> cells) with
//!    colspan/rowspan resolved into a rectangular `Vec<Vec<TableSlot>>` grid
//!    using a Servo-style slot model (`Cell(origin)` / `Spanned{back}`), plus
//!    anonymous-box fixup so stray cells/rows do not crash on real markup.
//! 2. Column widths: per-cell min/max-content intrinsic sizing, per-column
//!    aggregation, distribution of spanning cells across columns proportional to
//!    column max-width, then a fit to the table's used width (grow autos toward
//!    max; distribute slack by `(max-min)`; shrink toward min on overflow).
//!    Honors cell fixed/percent width, `table { width }`, border-spacing, and
//!    border-collapse.
//! 3. Cell + row layout: each cell's content is laid out at its final column
//!    width via the normal block formatting context; row height = max cell
//!    height; rowspan deficits are pushed to the last spanned row.
//! 4. Borders: `border-collapse: separate` (border-spacing, UA default 2px) and
//!    `border-collapse: collapse` (adjacent borders overlap -> thinner edge,
//!    fine for Wikipedia's uniform 1px borders), plus the legacy `border` /
//!    `cellpadding` / `cellspacing` attributes on `<table>`.
//!
//! Vertical-align in cells is top for v1 (documented).
//!
//! Output: the table produces `LayoutBox` children (rows then cells) in the
//! arena with correct `rect`/`content_rect` (document coords) and resolved
//! padding/border edges so the paint pass draws cell backgrounds/borders. The
//! table's used border-box size is returned to the block flow, exactly like
//! `layout_block_box`.

use crate::css::computed::ComputedStyle;
use crate::css::values::{Display, LengthOrPercent, LengthPercentOrAuto};
use crate::dom::{Document, NodeData, NodeId};
use crate::geom::{Edges, Rect, Vec2};

use super::block::layout_block_box;
use super::construct::{length_px, style_for};
use super::fonts::FontCtx;
use super::{BoxKind, ContentSizes, FormattingContext, LayoutBox, LayoutTree};

/// UA default `border-spacing` for `border-collapse: separate` (2px per axis).
const DEFAULT_BORDER_SPACING: f32 = 2.0;

/// A slot in the cell grid (Servo `TableSlot` model).
#[derive(Clone, Copy, Debug)]
enum TableSlot {
    /// No cell occupies this slot.
    Empty,
    /// Origin slot of a cell; index into `Grid::cells`.
    Cell(usize),
    /// Covered by an earlier spanning cell; index into `Grid::cells`.
    Spanned(usize),
}

/// A resolved origin cell.
struct GridCell {
    /// Source DOM node (the `<td>`/`<th>`), if any.
    node: Option<NodeId>,
    /// Layout-box index of the cell's contents container, set during layout.
    box_idx: usize,
    row: usize,
    col: usize,
    colspan: usize,
    rowspan: usize,
}

/// The constructed grid.
struct Grid {
    rows: Vec<Vec<TableSlot>>,
    cells: Vec<GridCell>,
    /// One per grid row: the `<tr>` source node, if the row came from a real row.
    row_nodes: Vec<Option<NodeId>>,
    num_cols: usize,
}

/// Table-level options from CSS + legacy attributes.
struct TableOpts {
    collapse: bool,
    spacing_h: f32,
    spacing_v: f32,
    /// Explicit used width (px) from CSS `width` or the `width` attribute.
    width: Option<f32>,
    /// Legacy `border` attribute width (px), if > 0.
    legacy_border: Option<f32>,
    /// `cellpadding` attribute (px), if present.
    cellpadding: Option<f32>,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Lay out the table box at arena index `idx`. Same contract as
/// [`layout_block_box`]: position its content box from `(x, y)` given the
/// containing block content width `cb_width`, fill its `rect`/`content_rect`,
/// build child row/cell boxes, and return the table's used border-box size.
pub fn layout_table_box(
    doc: &Document,
    fonts: &FontCtx,
    tree: &mut LayoutTree,
    idx: usize,
    cb_width: f32,
    x: f32,
    y: f32,
) -> Vec2 {
    let zoom = fonts.zoom();
    let table_node = tree.boxes[idx].node;
    let style = box_style(doc, tree, idx);
    let opts = table_opts(doc, &style, table_node, zoom);

    // Resolve the table's own outer edges (percentages against cb_width).
    resolve_outer_edges(&mut tree.boxes[idx], &style, cb_width, zoom);
    let m = tree.boxes[idx].margin;
    let bp_h = tree.boxes[idx].inline_extra();
    let bp_v = tree.boxes[idx].block_extra();

    // Content-area origin (document coords).
    let b = &tree.boxes[idx];
    let content_x = x + m.left + b.border.left + b.padding.left;
    let content_y0 = y + m.top + b.border.top + b.padding.top;

    let avail_inner = (cb_width - m.horizontal() - bp_h).max(0.0);

    // Build the grid from the (already constructed) descendant boxes/DOM.
    let grid = build_grid(doc, table_node);

    let mut children: Vec<usize> = Vec::new();
    let mut content_y = content_y0;

    // --- captions above the grid (minimal block layout) ---
    let mut caption_h = 0.0;
    if let Some(node) = table_node {
        for cap_node in caption_nodes(doc, node) {
            // Build + lay out a caption block on the fly via the block path.
            if let Some(cap_idx) = super::construct::build_box(doc, cap_node, zoom, tree) {
                let used = layout_block_box(doc, fonts, tree, cap_idx, avail_inner, content_x, content_y);
                caption_h += used.y;
                content_y += used.y;
                children.push(cap_idx);
            }
        }
    }

    // --- column widths ---
    let spacing_h = if opts.collapse { 0.0 } else { opts.spacing_h };
    let spacing_v = if opts.collapse { 0.0 } else { opts.spacing_v };
    let col_widths = resolve_columns(doc, fonts, &grid, &opts, avail_inner, spacing_h);
    let num_cols = col_widths.len();
    let num_rows = grid.num_rows();

    let total_cols_w: f32 = col_widths.iter().sum();
    let grid_inner_w = if num_cols == 0 {
        0.0
    } else {
        total_cols_w + spacing_h * (num_cols as f32 + 1.0)
    };

    // Column left edges (content-box left of each column's cells).
    let mut col_x = vec![content_x; num_cols + 1];
    {
        let mut cx = content_x + spacing_h;
        for c in 0..num_cols {
            col_x[c] = cx;
            cx += col_widths[c] + spacing_h;
        }
        col_x[num_cols] = cx;
    }

    // --- lay out each origin cell at its final width; remember heights ---
    struct Laid {
        cell_pos: usize, // index into grid.cells
        box_idx: usize,
        outer_h: f32, // content + cell border + cell padding (single-row tally)
        cb: Edges<f32>,
        pb: Edges<f32>,
        cell_w: f32,
    }
    let mut laid: Vec<Laid> = Vec::new();
    let mut row_heights = vec![0.0f32; num_rows];

    for (ci, cell) in grid.cells.iter().enumerate() {
        let cell_w = cell_width(&col_widths, cell.col, cell.colspan, spacing_h);
        let (cb, pb) = cell_border_padding(doc, cell.node, &opts, avail_inner, zoom);
        let inner_w = (cell_w - cb.horizontal() - pb.horizontal()).max(0.0);

        // Build the cell's content as a block container box and lay it out at
        // (0,0); we reposition once row geometry is known.
        let box_idx = build_cell_box(doc, cell.node, zoom, tree);
        let used = layout_block_box(doc, fonts, tree, box_idx, inner_w, 0.0, 0.0);
        let content_h = used.y;
        let outer_h = content_h + cb.vertical() + pb.vertical();

        if cell.rowspan <= 1 {
            row_heights[cell.row] = row_heights[cell.row].max(outer_h);
        }
        laid.push(Laid {
            cell_pos: ci,
            box_idx,
            outer_h,
            cb,
            pb,
            cell_w,
        });
    }

    // Distribute rowspan deficits to the last spanned row.
    for l in &laid {
        let cell = &grid.cells[l.cell_pos];
        if cell.rowspan > 1 && num_rows > 0 {
            let last = (cell.row + cell.rowspan).min(num_rows).saturating_sub(1);
            let mut spanned: f32 = 0.0;
            for r in cell.row..=last {
                spanned += row_heights[r];
            }
            spanned += spacing_v * (last - cell.row) as f32;
            if l.outer_h > spanned {
                row_heights[last] += l.outer_h - spanned;
            }
        }
    }

    // Row top offsets.
    let mut row_y = vec![content_y; num_rows + 1];
    {
        let mut ry = content_y + spacing_v;
        for r in 0..num_rows {
            row_y[r] = ry;
            ry += row_heights[r] + spacing_v;
        }
        row_y[num_rows] = ry;
    }
    let grid_inner_h = if num_rows == 0 {
        0.0
    } else {
        row_y[num_rows] - content_y
    };

    // --- position cells, group into row boxes ---
    for r in 0..num_rows {
        let row_top = row_y[r];
        let row_h = row_heights[r];
        let mut row_children: Vec<usize> = Vec::new();

        for l in laid.iter() {
            let cell = &grid.cells[l.cell_pos];
            if cell.row != r {
                continue;
            }
            let last_row = (cell.row + cell.rowspan).min(num_rows).saturating_sub(1);
            let cell_left = col_x[cell.col];
            let cell_top = row_top;
            let cell_bottom = row_y[last_row] + row_heights[last_row];
            let cell_h = (cell_bottom - cell_top).max(0.0);
            let cell_w = l.cell_w;

            let content_left = cell_left + l.cb.left + l.pb.left;
            let content_top = cell_top + l.cb.top + l.pb.top;
            let content_w = (cell_w - l.cb.horizontal() - l.pb.horizontal()).max(0.0);
            let content_h = (cell_h - l.cb.vertical() - l.pb.vertical()).max(0.0);

            // Move the laid-out cell contents (built at (0,0)) into place.
            offset_box_tree(tree, l.box_idx, content_left, content_top);

            // Finalize the cell box geometry (top vertical-align: content sits
            // at the top of the cell; the cell border box spans the full cell).
            {
                let cell_box = &mut tree.boxes[l.box_idx];
                cell_box.kind = BoxKind::TableCell;
                cell_box.fc = FormattingContext::Block;
                cell_box.border = l.cb;
                cell_box.padding = l.pb;
                cell_box.margin = Edges::ZERO;
                cell_box.rect =
                    Rect::from_min_size(egui::pos2(cell_left, cell_top), egui::vec2(cell_w.max(0.0), cell_h));
                cell_box.content_rect = Rect::from_min_size(
                    egui::pos2(content_left, content_top),
                    egui::vec2(content_w, content_h),
                );
            }
            row_children.push(l.box_idx);
        }

        // Emit a row box wrapping its cells.
        let mut row_box = match grid.row_nodes.get(r).and_then(|o| *o) {
            Some(n) => LayoutBox::new(n, FormattingContext::Block, BoxKind::TableRow),
            None => LayoutBox::new_anon(FormattingContext::Block, BoxKind::TableRow),
        };
        let row_rect = Rect::from_min_size(
            egui::pos2(content_x + spacing_h, row_top),
            egui::vec2((grid_inner_w - 2.0 * spacing_h).max(0.0), row_h),
        );
        row_box.rect = row_rect;
        row_box.content_rect = row_rect;
        row_box.children = row_children;
        let row_idx = tree.boxes.len();
        tree.boxes.push(row_box);
        children.push(row_idx);
    }

    // --- finalize table box ---
    let table_content_w = grid_inner_w;
    let table_content_h = caption_h + grid_inner_h;

    tree.boxes[idx].children = children;
    let border_box_w = table_content_w + bp_h;
    let border_box_h = table_content_h + bp_v;
    {
        let tb = &mut tree.boxes[idx];
        tb.rect = Rect::from_min_size(
            egui::pos2(x + m.left, y + m.top),
            egui::vec2(border_box_w.max(0.0), border_box_h.max(0.0)),
        );
        tb.content_rect = Rect::from_min_size(
            egui::pos2(content_x, content_y0),
            egui::vec2(table_content_w.max(0.0), table_content_h.max(0.0)),
        );
    }
    let _ = avail_inner;

    Vec2::new(border_box_w, border_box_h)
}

// ---------------------------------------------------------------------------
// Grid construction
// ---------------------------------------------------------------------------

/// Build the cell grid by walking the table's DOM subtree. Row groups
/// (thead/tbody/tfoot or `display:table-row-group/header/footer`) and bare rows
/// contribute rows; cells outside any row get an anonymous row (fixup).
fn build_grid(doc: &Document, table_node: Option<NodeId>) -> Grid {
    // Collect (row_node, cell_nodes) in document order.
    let mut rows_cells: Vec<(Option<NodeId>, Vec<NodeId>)> = Vec::new();
    let mut stray: Vec<NodeId> = Vec::new();

    if let Some(table) = table_node {
        for child in element_children(doc, table) {
            match disp(doc, child) {
                Some(Display::TableRow) => {
                    flush_stray(&mut stray, &mut rows_cells);
                    rows_cells.push((Some(child), cell_children(doc, child)));
                }
                Some(Display::TableRowGroup)
                | Some(Display::TableHeaderGroup)
                | Some(Display::TableFooterGroup) => {
                    flush_stray(&mut stray, &mut rows_cells);
                    for gc in element_children(doc, child) {
                        match disp(doc, gc) {
                            Some(Display::TableRow) => {
                                rows_cells.push((Some(gc), cell_children(doc, gc)));
                            }
                            Some(Display::TableCell) => {
                                // stray cell directly in a group: anonymous row
                                rows_cells.push((None, vec![gc]));
                            }
                            _ => {}
                        }
                    }
                }
                Some(Display::TableCell) => stray.push(child),
                _ => {}
            }
        }
        flush_stray(&mut stray, &mut rows_cells);
    }

    let num_rows = rows_cells.len();
    let mut rows: Vec<Vec<TableSlot>> = vec![Vec::new(); num_rows];
    let mut cells: Vec<GridCell> = Vec::new();
    let mut row_nodes: Vec<Option<NodeId>> = Vec::with_capacity(num_rows);
    for (rn, _) in &rows_cells {
        row_nodes.push(*rn);
    }

    for (r, (_rn, cell_nodes)) in rows_cells.iter().enumerate() {
        let mut col = 0usize;
        for &cn in cell_nodes {
            // Skip slots already filled (by rowspans from rows above).
            while col < rows[r].len() && !matches!(rows[r][col], TableSlot::Empty) {
                col += 1;
            }
            let colspan = attr_usize(doc, cn, "colspan", 1).clamp(1, 1000);
            let rowspan = attr_usize(doc, cn, "rowspan", 1).clamp(1, 1000);
            let cell_idx = cells.len();
            cells.push(GridCell {
                node: Some(cn),
                box_idx: usize::MAX,
                row: r,
                col,
                colspan,
                rowspan,
            });
            for dr in 0..rowspan {
                let rr = r + dr;
                if rr >= num_rows {
                    break;
                }
                for dc in 0..colspan {
                    let cc = col + dc;
                    ensure_len(&mut rows[rr], cc + 1);
                    if matches!(rows[rr][cc], TableSlot::Empty) {
                        rows[rr][cc] = if dr == 0 && dc == 0 {
                            TableSlot::Cell(cell_idx)
                        } else {
                            TableSlot::Spanned(cell_idx)
                        };
                    }
                }
            }
            col += colspan;
        }
    }

    let num_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    for row in &mut rows {
        ensure_len(row, num_cols);
    }

    Grid {
        rows,
        cells,
        row_nodes,
        num_cols,
    }
}

impl Grid {
    fn num_rows(&self) -> usize {
        self.rows.len()
    }
}

fn flush_stray(stray: &mut Vec<NodeId>, rows: &mut Vec<(Option<NodeId>, Vec<NodeId>)>) {
    if !stray.is_empty() {
        rows.push((None, std::mem::take(stray)));
    }
}

fn ensure_len(v: &mut Vec<TableSlot>, len: usize) {
    while v.len() < len {
        v.push(TableSlot::Empty);
    }
}

/// Element children of `node` (skip text/comment).
fn element_children(doc: &Document, node: NodeId) -> Vec<NodeId> {
    doc.node(node)
        .children
        .iter()
        .copied()
        .filter(|&c| doc.node(c).is_element())
        .collect()
}

/// Cell (display:table-cell) element children of a row.
fn cell_children(doc: &Document, row: NodeId) -> Vec<NodeId> {
    element_children(doc, row)
        .into_iter()
        .filter(|&c| disp(doc, c) == Some(Display::TableCell))
        .collect()
}

/// Caption element children of a table.
fn caption_nodes(doc: &Document, table: NodeId) -> Vec<NodeId> {
    element_children(doc, table)
        .into_iter()
        .filter(|&c| disp(doc, c) == Some(Display::TableCaption))
        .collect()
}

fn disp(doc: &Document, node: NodeId) -> Option<Display> {
    if doc.node(node).is_element() {
        Some(style_for(doc, node).display)
    } else {
        None
    }
}

fn attr_usize(doc: &Document, node: NodeId, name: &str, default: usize) -> usize {
    doc.node(node)
        .attr(name)
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(default)
}

// ---------------------------------------------------------------------------
// Column width resolution
// ---------------------------------------------------------------------------

fn resolve_columns(
    doc: &Document,
    fonts: &FontCtx,
    grid: &Grid,
    opts: &TableOpts,
    avail_inner: f32,
    spacing_h: f32,
) -> Vec<f32> {
    let num_cols = grid.num_cols;
    if num_cols == 0 {
        return Vec::new();
    }
    let zoom = fonts.zoom();

    let mut col_min = vec![0.0f32; num_cols];
    let mut col_max = vec![0.0f32; num_cols];
    let mut col_pct = vec![0.0f32; num_cols];
    let mut col_has_pct = vec![false; num_cols];

    // Pass 1: single-column cells set the column min/max directly.
    for cell in &grid.cells {
        let Some(node) = cell.node else { continue };
        let (cb, pb) = cell_border_padding(doc, cell.node, opts, avail_inner, zoom);
        let extra = cb.horizontal() + pb.horizontal();
        let (cmin, cmax, pct) = cell_request(doc, fonts, node, extra, avail_inner, zoom);
        if cell.colspan == 1 {
            let c = cell.col;
            col_min[c] = col_min[c].max(cmin);
            col_max[c] = col_max[c].max(cmax);
            if let Some(p) = pct {
                if p > col_pct[c] {
                    col_pct[c] = p;
                    col_has_pct[c] = true;
                }
            }
        }
    }

    // Pass 2: distribute spanning cells across their columns proportional to
    // column max-width (litehtml distribute model).
    for cell in &grid.cells {
        if cell.colspan <= 1 {
            continue;
        }
        let Some(node) = cell.node else { continue };
        let (cb, pb) = cell_border_padding(doc, cell.node, opts, avail_inner, zoom);
        let extra = cb.horizontal() + pb.horizontal();
        let (cmin, cmax, _pct) = cell_request(doc, fonts, node, extra, avail_inner, zoom);
        let end = (cell.col + cell.colspan).min(num_cols);
        let cols: Vec<usize> = (cell.col..end).collect();
        if cols.is_empty() {
            continue;
        }
        let inner_spacing = spacing_h * (cols.len() as f32 - 1.0);
        distribute_to_cols(&mut col_min, &cols, (cmin - inner_spacing).max(0.0));
        distribute_to_cols(&mut col_max, &cols, (cmax - inner_spacing).max(0.0));
    }

    let spacing_total = spacing_h * (num_cols as f32 + 1.0);
    let sum_min: f32 = col_min.iter().sum();
    let sum_max: f32 = col_max.iter().sum();
    let avail_for_cols = (avail_inner - spacing_total).max(0.0);

    // Target column-content width (excludes border-spacing).
    let target = match opts.width {
        Some(w) => (w - spacing_total).max(sum_min),
        None => {
            if sum_max <= avail_for_cols {
                sum_max // shrink-to-fit: do not stretch unless width is set
            } else {
                avail_for_cols.max(sum_min)
            }
        }
    };

    let mut widths = col_max.clone();
    if sum_max < target {
        let extra = target - sum_max;
        distribute_slack(&mut widths, &col_min, &col_max, extra);
    } else if target < sum_max {
        let shrink = sum_max - target;
        let total_slack: f32 = (0..num_cols).map(|c| (col_max[c] - col_min[c]).max(0.0)).sum();
        if total_slack > 0.0 {
            for c in 0..num_cols {
                let slack = (col_max[c] - col_min[c]).max(0.0);
                widths[c] = (col_max[c] - shrink * (slack / total_slack)).max(col_min[c]);
            }
        } else {
            let scale = if sum_max > 0.0 { target / sum_max } else { 1.0 };
            for c in 0..num_cols {
                widths[c] = (col_max[c] * scale).max(col_min[c]);
            }
        }
    }

    // Honor explicit percent columns against the target (best effort).
    for c in 0..num_cols {
        if col_has_pct[c] {
            let want = target * col_pct[c] / 100.0;
            if want > widths[c] {
                widths[c] = want;
            }
        }
    }

    for w in &mut widths {
        if !w.is_finite() || *w < 0.0 {
            *w = 0.0;
        }
    }
    widths
}

/// Raise the given columns so they collectively reach `total`, weighting by
/// current value (max-width) — litehtml's spanning-cell distribution.
fn distribute_to_cols(col: &mut [f32], cols: &[usize], total: f32) {
    let current: f32 = cols.iter().map(|&c| col[c]).sum();
    if total <= current {
        return;
    }
    let extra = total - current;
    if current > 0.0 {
        for &c in cols {
            col[c] += extra * (col[c] / current);
        }
    } else {
        let share = extra / cols.len() as f32;
        for &c in cols {
            col[c] += share;
        }
    }
}

/// Distribute positive slack by `(max-min)` weight, falling back to equal split.
fn distribute_slack(widths: &mut [f32], col_min: &[f32], col_max: &[f32], extra: f32) {
    if extra <= 0.0 {
        return;
    }
    let n = widths.len();
    let total_slack: f32 = (0..n).map(|c| (col_max[c] - col_min[c]).max(0.0)).sum();
    if total_slack > 0.0 {
        for c in 0..n {
            let slack = (col_max[c] - col_min[c]).max(0.0);
            widths[c] += extra * (slack / total_slack);
        }
    } else if n > 0 {
        let share = extra / n as f32;
        for w in widths.iter_mut() {
            *w += share;
        }
    }
}

/// Final px width of a cell spanning `colspan` columns from `col`, including the
/// border-spacing that falls between the spanned columns (separate model).
fn cell_width(col_widths: &[f32], col: usize, colspan: usize, spacing_h: f32) -> f32 {
    let end = (col + colspan).min(col_widths.len());
    let mut w: f32 = col_widths[col..end].iter().sum();
    if end > col {
        w += spacing_h * (end - col - 1) as f32;
    }
    w
}

// ---------------------------------------------------------------------------
// Intrinsic sizing
// ---------------------------------------------------------------------------

/// (min, max, optional percent) width request for a cell, folding in its CSS
/// width (fixed/percent) plus `extra` (border+padding).
fn cell_request(
    doc: &Document,
    fonts: &FontCtx,
    node: NodeId,
    extra: f32,
    _avail: f32,
    zoom: f32,
) -> (f32, f32, Option<f32>) {
    let style = style_for(doc, node);
    let content = measure_node_children(doc, fonts, node);
    let mut min = content.min_content + extra;
    let mut max = content.max_content + extra;
    let mut pct = None;

    match style.width {
        LengthPercentOrAuto::Length(l) => {
            let w = length_px(l) * zoom + extra;
            max = w.max(content.min_content + extra);
            min = min.min(max);
        }
        LengthPercentOrAuto::Percent(p) => {
            pct = Some(p * 100.0);
        }
        LengthPercentOrAuto::Auto => {}
    }
    (min.max(0.0), max.max(min).max(0.0), pct)
}

/// Intrinsic min/max-content of a node's children (block + inline), recursing.
fn measure_node_children(doc: &Document, fonts: &FontCtx, node: NodeId) -> ContentSizes {
    let mut out = ContentSizes::ZERO;
    // Inline run accumulation: max widths sum on a single line; min = widest word.
    let mut inline_max = 0.0f32;
    let mut inline_min = 0.0f32;

    let flush = |out: &mut ContentSizes, imax: &mut f32, imin: &mut f32| {
        if *imax > 0.0 || *imin > 0.0 {
            *out = out.max(ContentSizes {
                min_content: *imin,
                max_content: *imax,
            });
        }
        *imax = 0.0;
        *imin = 0.0;
    };

    for &child in &doc.node(node).children {
        match &doc.node(child).data {
            NodeData::Text(text) => {
                let style = style_for(doc, child);
                let space_w = fonts.measure_width(" ", &style);
                let mut first = true;
                for word in text.split_whitespace() {
                    let w = fonts.measure_width(word, &style);
                    inline_min = inline_min.max(w);
                    if !first {
                        inline_max += space_w;
                    }
                    inline_max += w;
                    first = false;
                }
            }
            NodeData::Element { .. } => {
                let d = disp(doc, child).unwrap_or(Display::Inline);
                if d == Display::None {
                    continue;
                }
                let extra = horizontal_extra(doc, child, fonts.zoom());
                match d {
                    Display::Inline => {
                        // Replaced inline (img) is atomic with a rough width.
                        if doc.node(child).tag() == Some("img") {
                            let w = img_intrinsic_w(doc, child, fonts.zoom());
                            inline_min = inline_min.max(w);
                            inline_max += w;
                        } else {
                            let nested = measure_node_children(doc, fonts, child);
                            inline_min = inline_min.max(nested.min_content + extra);
                            inline_max += nested.max_content + extra;
                        }
                    }
                    Display::Table => {
                        flush(&mut out, &mut inline_max, &mut inline_min);
                        out = out.max(measure_table_intrinsic(doc, fonts, child));
                    }
                    _ => {
                        flush(&mut out, &mut inline_max, &mut inline_min);
                        let nested = measure_node_children(doc, fonts, child);
                        out = out.max(ContentSizes {
                            min_content: nested.min_content + extra,
                            max_content: nested.max_content + extra,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    flush(&mut out, &mut inline_max, &mut inline_min);
    out
}

/// Intrinsic width of a nested table: sum of its single-span columns' min/max.
fn measure_table_intrinsic(doc: &Document, fonts: &FontCtx, table: NodeId) -> ContentSizes {
    let style = style_for(doc, table);
    let opts = table_opts(doc, &style, Some(table), fonts.zoom());
    let grid = build_grid(doc, Some(table));
    let num_cols = grid.num_cols;
    if num_cols == 0 {
        return ContentSizes::ZERO;
    }
    let mut col_min = vec![0.0f32; num_cols];
    let mut col_max = vec![0.0f32; num_cols];
    for cell in &grid.cells {
        let Some(node) = cell.node else { continue };
        if cell.colspan != 1 {
            continue;
        }
        let (cb, pb) = cell_border_padding(doc, cell.node, &opts, 0.0, fonts.zoom());
        let extra = cb.horizontal() + pb.horizontal();
        let cs = measure_node_children(doc, fonts, node);
        col_min[cell.col] = col_min[cell.col].max(cs.min_content + extra);
        col_max[cell.col] = col_max[cell.col].max(cs.max_content + extra);
    }
    let spacing_h = if opts.collapse { 0.0 } else { opts.spacing_h };
    let spacing_total = spacing_h * (num_cols as f32 + 1.0);
    ContentSizes {
        min_content: col_min.iter().sum::<f32>() + spacing_total,
        max_content: col_max.iter().sum::<f32>() + spacing_total,
    }
}

fn horizontal_extra(doc: &Document, node: NodeId, zoom: f32) -> f32 {
    let style = style_for(doc, node);
    let bw = style.border_width;
    let pad = style.padding;
    let b = (bw.left + bw.right) * zoom;
    let p = lp_abs(pad.left, zoom) + lp_abs(pad.right, zoom);
    b + p
}

fn img_intrinsic_w(doc: &Document, node: NodeId, zoom: f32) -> f32 {
    // `<math>` carries its intrinsic replaced size in UNZOOMED px (already folded
    // into the projected width); otherwise use an explicit width.
    if let Some((w, _)) = doc.node(node).replaced_size {
        return w * zoom;
    }
    if let LengthPercentOrAuto::Length(l) = style_for(doc, node).width {
        return length_px(l) * zoom;
    }
    20.0 * zoom
}

// ---------------------------------------------------------------------------
// Cell content box
// ---------------------------------------------------------------------------

/// Build a block container box for a cell's contents (the cell's own
/// border/padding/margin are applied by the table; here the contents box is
/// borderless/paddingless so they don't double-count).
fn build_cell_box(doc: &Document, node: Option<NodeId>, zoom: f32, tree: &mut LayoutTree) -> usize {
    if let Some(n) = node {
        // Use the normal box builder so children get proper inline/block fixup,
        // then strip the cell's own edges (handled at the table level) and force
        // an auto width/height so layout fills the column.
        if let Some(idx) = super::construct::build_box(doc, n, zoom, tree) {
            let b = &mut tree.boxes[idx];
            b.kind = BoxKind::TableCell;
            b.fc = FormattingContext::Block;
            b.border = Edges::ZERO;
            b.padding = Edges::ZERO;
            b.margin = Edges::ZERO;
            return idx;
        }
    }
    // Fallback: empty anonymous block.
    let b = LayoutBox::new_anon(FormattingContext::Block, BoxKind::TableCell);
    let idx = tree.boxes.len();
    tree.boxes.push(b);
    idx
}

// ---------------------------------------------------------------------------
// Options / borders
// ---------------------------------------------------------------------------

fn table_opts(doc: &Document, style: &ComputedStyle, node: Option<NodeId>, zoom: f32) -> TableOpts {
    // border-collapse / border-spacing aren't in ComputedStyle; read the inline
    // `style=` attribute (the only place Wikipedia sets them per-table) plus the
    // UA default of separate/2px.
    let style_attr = node.and_then(|n| doc.node(n).attr("style")).unwrap_or("");
    let collapse = decl_value(style_attr, "border-collapse")
        .map(|v| v.trim().eq_ignore_ascii_case("collapse"))
        .unwrap_or(false);

    let (mut sp_h, mut sp_v) = (DEFAULT_BORDER_SPACING * zoom, DEFAULT_BORDER_SPACING * zoom);
    if let Some(v) = decl_value(style_attr, "border-spacing") {
        let parts: Vec<f32> = v.split_whitespace().filter_map(parse_px).collect();
        match parts.as_slice() {
            [a] => {
                sp_h = *a * zoom;
                sp_v = *a * zoom;
            }
            [a, b, ..] => {
                sp_h = *a * zoom;
                sp_v = *b * zoom;
            }
            _ => {}
        }
    }
    if let Some(cs) = node.and_then(|n| doc.node(n).attr("cellspacing")).and_then(parse_px) {
        sp_h = cs * zoom;
        sp_v = cs * zoom;
    }

    let width = match style.width {
        LengthPercentOrAuto::Length(l) => Some(length_px(l) * zoom),
        LengthPercentOrAuto::Percent(_) | LengthPercentOrAuto::Auto => node
            .and_then(|n| doc.node(n).attr("width"))
            .and_then(parse_px)
            .map(|w| w * zoom),
    };

    let legacy_border = node
        .and_then(|n| doc.node(n).attr("border"))
        .and_then(|s| s.trim().parse::<f32>().ok())
        .filter(|b| *b > 0.0);

    let cellpadding = node
        .and_then(|n| doc.node(n).attr("cellpadding"))
        .and_then(parse_px)
        .map(|p| p * zoom);

    TableOpts {
        collapse,
        spacing_h: sp_h,
        spacing_v: sp_v,
        width,
        legacy_border,
        cellpadding,
    }
}

/// Resolve a cell's effective border & padding edges (px), folding in the legacy
/// `border`/`cellpadding` attributes. In `border-collapse: collapse`, adjacent
/// borders overlap; we keep each cell's own edge for painting (the thinner-edge
/// simplification — uniform 1px reduces to a single 1px edge), and the spacing
/// contribution is removed by zeroing `border-spacing` at the call sites.
fn cell_border_padding(
    doc: &Document,
    node: Option<NodeId>,
    opts: &TableOpts,
    avail: f32,
    zoom: f32,
) -> (Edges<f32>, Edges<f32>) {
    let mut b = Edges::ZERO;
    let mut p = Edges::ZERO;
    if let Some(n) = node {
        let style = style_for(doc, n);
        b = style.border_width.map(|w| w * zoom);
        p = style.padding.map(|lp| lp_px(lp, avail, zoom));
    }
    // Legacy `border` attribute on <table> gives cells a 1px default edge.
    if opts.legacy_border.is_some() {
        if b.left == 0.0 {
            b.left = 1.0 * zoom;
        }
        if b.right == 0.0 {
            b.right = 1.0 * zoom;
        }
        if b.top == 0.0 {
            b.top = 1.0 * zoom;
        }
        if b.bottom == 0.0 {
            b.bottom = 1.0 * zoom;
        }
    }
    if let Some(cp) = opts.cellpadding {
        p = Edges::splat(cp);
    }
    (b, p)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn box_style(doc: &Document, tree: &LayoutTree, idx: usize) -> ComputedStyle {
    match tree.boxes[idx].node {
        Some(n) => style_for(doc, n),
        None => ComputedStyle::initial(),
    }
}

/// Resolve the table's own margin/padding/border edges (px) onto its box.
fn resolve_outer_edges(b: &mut LayoutBox, style: &ComputedStyle, cb_width: f32, zoom: f32) {
    b.border = style.border_width.map(|w| w * zoom);
    b.padding = style.padding.map(|lp| lp_px(lp, cb_width, zoom));
    b.margin = Edges::new(
        lpa_px(style.margin.top, cb_width, zoom),
        lpa_px(style.margin.right, cb_width, zoom),
        lpa_px(style.margin.bottom, cb_width, zoom),
        lpa_px(style.margin.left, cb_width, zoom),
    );
}

fn lp_px(lp: LengthOrPercent, base: f32, zoom: f32) -> f32 {
    match lp {
        LengthOrPercent::Length(l) => length_px(l) * zoom,
        LengthOrPercent::Percent(p) => base.max(0.0) * p,
    }
}

fn lp_abs(lp: LengthOrPercent, zoom: f32) -> f32 {
    match lp {
        LengthOrPercent::Length(l) => length_px(l) * zoom,
        LengthOrPercent::Percent(_) => 0.0,
    }
}

fn lpa_px(m: LengthPercentOrAuto, base: f32, zoom: f32) -> f32 {
    match m {
        LengthPercentOrAuto::Length(l) => length_px(l) * zoom,
        LengthPercentOrAuto::Percent(p) => base.max(0.0) * p,
        LengthPercentOrAuto::Auto => 0.0,
    }
}

/// Recursively translate a laid-out box subtree so its border-box top-left moves
/// to `(new_x, new_y)`. Mirrors block.rs's `offset_box_tree`.
fn offset_box_tree(tree: &mut LayoutTree, idx: usize, new_x: f32, new_y: f32) {
    let old = tree.boxes[idx].rect.min;
    let dx = new_x - old.x;
    let dy = new_y - old.y;
    if dx == 0.0 && dy == 0.0 {
        return;
    }
    translate(tree, idx, dx, dy);
}

fn translate(tree: &mut LayoutTree, idx: usize, dx: f32, dy: f32) {
    {
        let b = &mut tree.boxes[idx];
        b.rect = b.rect.translate(egui::vec2(dx, dy));
        b.content_rect = b.content_rect.translate(egui::vec2(dx, dy));
        for f in &mut b.inline_fragments {
            translate_fragment(f, dx, dy);
        }
    }
    let kids = tree.boxes[idx].children.clone();
    for c in kids {
        translate(tree, c, dx, dy);
    }
}

fn translate_fragment(f: &mut super::InlineFragment, dx: f32, dy: f32) {
    match f {
        super::InlineFragment::Text { pos, .. } => {
            pos.x += dx;
            pos.y += dy;
        }
        super::InlineFragment::Rect { rect, .. } => {
            *rect = rect.translate(egui::vec2(dx, dy));
        }
        super::InlineFragment::Box { .. } => {}
    }
}

/// Extract a declaration value from an inline `style=` string.
fn decl_value(style_attr: &str, prop: &str) -> Option<String> {
    for decl in style_attr.split(';') {
        let mut it = decl.splitn(2, ':');
        let name = it.next()?.trim();
        if name.eq_ignore_ascii_case(prop) {
            if let Some(val) = it.next() {
                return Some(val.trim().to_string());
            }
        }
    }
    None
}

/// Parse a px length like "12px" / "12" / "1.5px".
fn parse_px(s: &str) -> Option<f32> {
    let s = s.trim();
    let s = s.strip_suffix("px").unwrap_or(s);
    s.trim().parse::<f32>().ok()
}

// Keep the old placeholder grid type name available for any external reference.
#[allow(dead_code)]
pub type CellId = usize;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::parse_html;
    use crate::layout::{layout_document, BoxKind, LayoutTree};
    use std::path::PathBuf;

    struct DirProvider {
        root: PathBuf,
    }
    impl crate::ResourceProvider for DirProvider {
        fn fetch(&self, url: &str) -> Option<(Vec<u8>, String)> {
            let rel = url.trim_start_matches("./").trim_start_matches('/');
            let path = self.root.join(rel);
            let bytes = std::fs::read(&path).ok()?;
            let mime = if rel.ends_with(".css") {
                "text/css".to_string()
            } else {
                "application/octet-stream".to_string()
            };
            Some((bytes, mime))
        }
    }
    struct NullProvider;
    impl crate::ResourceProvider for NullProvider {
        fn fetch(&self, _url: &str) -> Option<(Vec<u8>, String)> {
            None
        }
    }

    fn headless_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(egui::FontDefinitions::default());
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        ctx
    }

    fn collect(tree: &LayoutTree, idx: usize, kind: BoxKind, out: &mut Vec<usize>) {
        if tree.boxes[idx].kind == kind {
            out.push(idx);
        }
        for &c in &tree.boxes[idx].children {
            collect(tree, c, kind, out);
        }
    }

    #[test]
    fn synthetic_table_grid_and_rects() {
        // 2 columns via a colspan header; column 0 has a rowspan.
        let html = r#"<table style="border-collapse: collapse" border="1" width="400">
          <tr><th colspan="2">Header spanning two columns</th></tr>
          <tr><td rowspan="2">Tall</td><td>r1c2</td></tr>
          <tr><td>r2c2</td></tr>
        </table>"#;
        let mut doc = parse_html(html);
        let ctx = headless_ctx();
        crate::css::stylo::style_document_stylo(&mut doc, &NullProvider, None, crate::Theme::Light, 1000.0, Some(&ctx));
        let mut fonts = FontCtx::new(ctx, 1.0);
        let (tree, _size) = layout_document(&doc, &mut fonts, 800.0, 1.0);

        let root = tree.root.expect("root");
        let mut tables = Vec::new();
        collect(&tree, root, BoxKind::Table, &mut tables);
        assert_eq!(tables.len(), 1, "exactly one table");
        let table = tables[0];

        let mut rows = Vec::new();
        collect(&tree, table, BoxKind::TableRow, &mut rows);
        assert_eq!(rows.len(), 3, "three grid rows");

        let mut cells = Vec::new();
        collect(&tree, table, BoxKind::TableCell, &mut cells);
        assert_eq!(cells.len(), 4, "four origin cells (1 header + 3 body)");

        // Table content width ~ requested 400 (collapse => no spacing).
        let tw = tree.boxes[table].content_rect.width();
        assert!((tw - 400.0).abs() <= 6.0, "table width ~400, got {tw}");

        // Finite, non-negative cell rects.
        for &c in &cells {
            let r = tree.boxes[c].content_rect;
            assert!(r.width() >= 0.0 && r.width().is_finite());
            assert!(r.height() >= 0.0 && r.height().is_finite());
        }

        // Cells within a row don't horizontally overlap (border boxes).
        for &row in &rows {
            let mut cs = Vec::new();
            collect(&tree, row, BoxKind::TableCell, &mut cs);
            let mut spans: Vec<(f32, f32)> = cs
                .iter()
                .map(|&c| {
                    let r = tree.boxes[c].rect;
                    (r.left(), r.right())
                })
                .collect();
            spans.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            for w in spans.windows(2) {
                assert!(w[0].1 <= w[1].0 + 0.5, "row cells overlap: {:?} {:?}", w[0], w[1]);
            }
        }

        // The spanning header cell ~ table width.
        let mut hc = Vec::new();
        collect(&tree, rows[0], BoxKind::TableCell, &mut hc);
        assert_eq!(hc.len(), 1, "header has one spanning cell");
        let hw = tree.boxes[hc[0]].rect.width();
        assert!((hw - tw).abs() <= 8.0, "spanning header ~ table width: {hw} vs {tw}");

        // The rowspan cell is taller than a single-row body cell.
        let rowspan_h = tree.boxes[cells.iter().copied()
            .max_by(|&a, &b| {
                tree.boxes[a].rect.height().partial_cmp(&tree.boxes[b].rect.height()).unwrap()
            })
            .unwrap()]
        .rect
        .height();
        assert!(rowspan_h > 0.0, "some cell has positive height");
    }

    #[test]
    fn article_lays_out_without_panic() {
        let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/wiki-sample"));
        let html = std::fs::read_to_string(dir.join("article.html")).expect("read article.html");
        let mut doc = parse_html(&html);
        let provider = DirProvider { root: dir.clone() };
        let ctx = headless_ctx();
        crate::css::stylo::style_document_stylo(&mut doc, &provider, Some("./"), crate::Theme::Light, 1000.0, Some(&ctx));

        let mut fonts = FontCtx::new(ctx, 1.0);
        let (tree, size) = layout_document(&doc, &mut fonts, 800.0, 1.0);

        assert!(size.x > 0.0 && size.y > 0.0 && size.y.is_finite(), "sane content size");

        let root = tree.root.expect("root");
        let mut tables = Vec::new();
        collect(&tree, root, BoxKind::Table, &mut tables);
        let mut largest = (0.0f32, 0.0f32);
        let mut largest_area = 0.0f32;
        for &t in &tables {
            let r = tree.boxes[t].content_rect;
            let area = r.width() * r.height();
            if area > largest_area {
                largest_area = area;
                largest = (r.width(), r.height());
            }
        }
        eprintln!(
            "[table-stats] laid out {} table boxes; largest = {:.0}x{:.0} px",
            tables.len(),
            largest.0,
            largest.1
        );
        assert!(!tables.is_empty(), "article contains tables");
    }
}

#[cfg(test)]
mod chembox_regression {
    //! Regression for the Water infobox (`table.infobox.ib-chembox`): Wikipedia's
    //! mobile stylesheet sets `.mw-parser-output table { display:block }` inside an
    //! `@media (max-width:639px)` block. Before width media features were
    //! evaluated, that rule applied on every viewport, demoting the table to a
    //! block so its `<td>`s stacked vertically and overflowed. With a desktop-width
    //! viewport the rule is dropped and the table lays out as a real grid.
    use super::*;
    use crate::dom::parse_html;
    use crate::layout::{layout_document, BoxKind, LayoutTree};
    use std::path::PathBuf;

    struct DirProvider {
        root: PathBuf,
    }
    impl crate::ResourceProvider for DirProvider {
        fn fetch(&self, url: &str) -> Option<(Vec<u8>, String)> {
            let rel = url.trim_start_matches("./").trim_start_matches('/');
            let bytes = std::fs::read(self.root.join(rel)).ok()?;
            let mime = if rel.ends_with(".css") {
                "text/css"
            } else {
                "application/octet-stream"
            };
            Some((bytes, mime.to_string()))
        }
    }
    fn headless_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(egui::FontDefinitions::default());
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        ctx
    }
    fn collect(tree: &LayoutTree, idx: usize, kind: BoxKind, out: &mut Vec<usize>) {
        if tree.boxes[idx].kind == kind {
            out.push(idx);
        }
        for &c in &tree.boxes[idx].children {
            collect(tree, c, kind, out);
        }
    }

    #[test]
    fn infobox_lays_out_as_grid_on_desktop_width() {
        let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/wiki-sample"));
        let html = std::fs::read_to_string(dir.join("article.html")).unwrap();
        let mut doc = parse_html(&html);
        let provider = DirProvider { root: dir.clone() };
        // Desktop-width viewport: the mobile `display:block` rule must be dropped.
        let ctx = headless_ctx();
        crate::css::stylo::style_document_stylo(&mut doc, &provider, Some("./"), crate::Theme::Light, 1000.0, Some(&ctx));
        let mut fonts = FontCtx::new(ctx, 1.0);
        let (tree, _size) = layout_document(&doc, &mut fonts, 800.0, 1.0);
        let root = tree.root.unwrap();

        // The chembox node must produce a real Table box.
        let mut tables = Vec::new();
        collect(&tree, root, BoxKind::Table, &mut tables);
        let chembox = tables
            .iter()
            .copied()
            .find(|&t| {
                tree.boxes[t]
                    .node
                    .map_or(false, |n| doc.node(n).attr("class").map_or(false, |c| c.contains("ib-chembox")))
            })
            .expect("chembox table box exists (table not demoted to block)");
        let table_right = tree.boxes[chembox].rect.right();

        // Inspect the chembox's own rows (not nested tables). Two-column rows
        // (label + value) must be laid out side-by-side, and no cell may extend
        // past the table's right edge.
        let mut two_col_rows = 0;
        for &row in &tree.boxes[chembox].children {
            if tree.boxes[row].kind != BoxKind::TableRow {
                continue;
            }
            let cells: Vec<usize> = tree.boxes[row]
                .children
                .iter()
                .copied()
                .filter(|&c| tree.boxes[c].kind == BoxKind::TableCell)
                .collect();
            // Cells in a row share a top edge and tile left-to-right.
            let mut spans: Vec<(f32, f32, f32)> = cells
                .iter()
                .map(|&c| {
                    let r = tree.boxes[c].rect;
                    (r.left(), r.right(), r.top())
                })
                .collect();
            spans.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            if spans.len() == 2 {
                two_col_rows += 1;
                // Same row => same top (side-by-side, not stacked).
                assert!(
                    (spans[0].2 - spans[1].2).abs() <= 1.0,
                    "row cells should share a top edge: {spans:?}"
                );
                // No horizontal overlap.
                assert!(spans[0].1 <= spans[1].0 + 0.5, "cells overlap: {spans:?}");
            }
            for &(_, right, _) in &spans {
                assert!(
                    right <= table_right + 1.0,
                    "cell right {right} exceeds table right {table_right}"
                );
            }
        }
        assert!(
            two_col_rows > 5,
            "expected many label/value rows, found {two_col_rows}"
        );
    }
}
