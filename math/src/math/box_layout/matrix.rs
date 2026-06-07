//! Matrix / array / cases / aligned layout — the TeX *array* algorithm, split out
//! of the layout engine as an [`Ctx`] impl-continuation. Lays a grid of cells in
//! rows and columns, vertically centered on the math axis, with column/row rules
//! from an `array` spec and a self-drawn left brace for `cases`.

use super::{
    axis_px, delim, layout_list, Align, Box, BoxKind, Child, Ctx, MathList, MatrixKind, Style,
};

impl Ctx<'_> {
    /// Lay out a matrix/array/cases/aligned environment (the TeX *array* algorithm,
    /// cf. KaTeX `buildHTML` `makeArray` and microTeX's matrix atom).
    ///
    /// 1. **Cells.** Each cell lays out as its own [`MathList`] at the cell style:
    ///    `Plain` matrices keep the surrounding `style` (Display stays Display);
    ///    `cases`/`aligned` cells render in [`Style::Text`]. Empty/missing cells are
    ///    treated as zero-size.
    /// 2. **Column widths** = the max cell width in each column; a cell is placed in
    ///    its column by `col_align` (`Center`: `(colw−cellw)/2`, `Left`: 0,
    ///    `Right`: `colw−cellw`).
    /// 3. **Row metrics**: each row's height/depth is the max over its cells. Rows
    ///    are stacked on baselines a fixed `arraystretch · em` apart, but never
    ///    closer than a `jot` of clearance between one row's depth and the next
    ///    row's height (`baseline ≥ prevDepth + jot + thisHeight`).
    /// 4. **Columns** are separated by `arraycolsep` on each side (`≈ 0.5 em`); for
    ///    `aligned` the right|left column pair touches (gap 0) so the `&` boundary —
    ///    typically a relation — lines up across rows.
    /// 5. The whole grid is **vertically centered on the math axis**: the row-stack's
    ///    own center is shifted to the axis, so the array sits like a tall delimiter
    ///    (and the enclosing `\left…\right` of `pmatrix`/… sizes to it). For `cases`
    ///    a large left brace, grown to the grid height via [`delim::sized_delim`], is
    ///    prepended; there is no right delimiter.
    pub(crate) fn layout_matrix(
        &self,
        rows: &[Vec<MathList>],
        col_align: &[Align],
        kind: MatrixKind,
        col_seps: &[u8],
        row_lines: &[u8],
        style: Style,
    ) -> Option<Box> {
        let ctx = self;
        if rows.is_empty() {
            return None;
        }

        // Cell render style: matrices follow the surrounding style; cases/aligned
        // bodies are text style (TeX `\textstyle`); `\substack` content is one step
        // smaller (script size) than its surroundings.
        let cell_style = match kind {
            MatrixKind::Plain => style,
            MatrixKind::Cases | MatrixKind::Aligned => Style::Text,
            MatrixKind::Substack => style.smaller(),
        };

        let n_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        if n_cols == 0 {
            return None;
        }

        // Lay out every cell; an empty cell becomes a zero-size empty hbox.
        let empty = || Box {
            width: 0.0,
            height: 0.0,
            depth: 0.0,
            kind: BoxKind::Hbox { children: Vec::new() },
        };
        let mut cells: Vec<Vec<Box>> = Vec::with_capacity(rows.len());
        let mut col_w = vec![0.0f32; n_cols];
        let mut row_h = vec![0.0f32; rows.len()];
        let mut row_d = vec![0.0f32; rows.len()];
        for (r, row) in rows.iter().enumerate() {
            let mut boxes = Vec::with_capacity(n_cols);
            for c in 0..n_cols {
                let b = row
                    .get(c)
                    .and_then(|cell| layout_list(ctx, cell, cell_style, /* cramped */ false))
                    .unwrap_or_else(empty);
                col_w[c] = col_w[c].max(b.width);
                row_h[r] = row_h[r].max(b.height);
                row_d[r] = row_d[r].max(b.depth);
                boxes.push(b);
            }
            cells.push(boxes);
        }

        // Gaps and stretch (px at the base em).
        let arraycolsep = 0.5 * ctx.base_em; // half-gap on each side of a column
        // `\arraystretch` scales the nominal inter-row baseline distance; substack
        // (a script-size stack) is unaffected.
        let arraystretch = match kind {
            MatrixKind::Substack => 1.0,
            _ => ctx.arraystretch,
        };
        let baseline_skip = arraystretch * ctx.base_em; // nominal baseline distance
        let jot = 0.25 * ctx.base_em; // min clearance between adjacent rows

        // Alignment of column `c`, extended past `col_align` with the kind default.
        let align_of = |c: usize| -> Align {
            col_align.get(c).copied().unwrap_or(match kind {
                MatrixKind::Aligned => {
                    if c % 2 == 0 {
                        Align::Right
                    } else {
                        Align::Left
                    }
                }
                MatrixKind::Cases => Align::Left,
                MatrixKind::Plain | MatrixKind::Substack => Align::Center,
            })
        };

        // Vertical-rule geometry (px at the base em). A `|` is a thin rule preceded
        // and followed by a small gap; `||` stacks two rules a hair apart. The rule
        // thickness mirrors the fraction-bar default (≈ 0.04 em).
        let rule_thickness = 0.04 * ctx.base_em;
        let sep_at = |slot: usize| col_seps.get(slot).copied().unwrap_or(0);
        let rule_gap = 0.5 * arraycolsep; // gap on each side of a rule run
        let double_gap = 0.06 * ctx.base_em; // space between the two rules of `||`

        // Column x-offsets: a half-`arraycolsep` of inter-column space on each side,
        // i.e. a full `arraycolsep` between adjacent columns — except the `aligned`
        // right|left pair (even→odd) which touches so the `&` boundary lines up. Any
        // vertical `|` rules from the column spec insert their own width (gap + rule)
        // at the appropriate slot; we record each rule's x for drawing below.
        let mut col_x = vec![0.0f32; n_cols];
        // `(x_of_first_rule, count)` for each non-empty separator slot.
        let mut vrules: Vec<(f32, u8)> = Vec::new();
        let mut x = 0.0f32;
        // Place any left-edge rules before the first column.
        let push_vrule = |vrules: &mut Vec<(f32, u8)>, x: &mut f32, n: u8| {
            if n > 0 {
                *x += rule_gap;
                vrules.push((*x, n));
                *x += n as f32 * rule_thickness + (n.saturating_sub(1)) as f32 * double_gap;
                *x += rule_gap;
            }
        };
        push_vrule(&mut vrules, &mut x, sep_at(0));
        for c in 0..n_cols {
            col_x[c] = x;
            x += col_w[c];
            if c + 1 < n_cols {
                // Inter-column space: a separator (if any) replaces the plain gap;
                // otherwise the usual `arraycolsep` (suppressed for an `aligned` pair).
                let n = sep_at(c + 1);
                if n > 0 {
                    push_vrule(&mut vrules, &mut x, n);
                } else {
                    let touching = matches!(kind, MatrixKind::Aligned) && c % 2 == 0;
                    if !touching {
                        x += arraycolsep;
                    }
                }
            }
        }
        // Right-edge rules after the last column.
        push_vrule(&mut vrules, &mut x, sep_at(n_cols));
        let grid_w = x;

        // Row baselines, top-down, starting at 0 (we recenter on the axis after).
        let mut row_y = vec![0.0f32; rows.len()];
        let mut y = 0.0f32;
        for r in 0..rows.len() {
            if r > 0 {
                let gap = (row_d[r - 1] + jot + row_h[r]).max(baseline_skip);
                y += gap;
            }
            row_y[r] = y;
        }
        // Extent of the row stack about the *first* row's baseline (y=0 .. last).
        let stack_top = row_h[0]; // above first baseline
        let stack_bottom = row_y[rows.len() - 1] + row_d[rows.len() - 1]; // below first baseline

        // Center the stack on the math axis: the stack's vertical midpoint should
        // sit at the axis (above the baseline by `axis`). The midpoint currently sits
        // at `(−stack_top + stack_bottom)/2` (downward-positive). Shift every row so
        // that midpoint maps to `−axis` (above baseline).
        let axis = axis_px(ctx);
        let mid = (-stack_top + stack_bottom) / 2.0;
        let shift = -axis - mid; // add to each row_y (downward dy)

        // Assemble the grid as an Hbox of cells placed by (col_x, row baseline).
        let mut children: Vec<Child> = Vec::new();
        for (r, boxes) in cells.into_iter().enumerate() {
            let dy = row_y[r] + shift;
            for (c, b) in boxes.into_iter().enumerate() {
                let pad = match align_of(c) {
                    Align::Left => 0.0,
                    Align::Center => (col_w[c] - b.width) / 2.0,
                    Align::Right => col_w[c] - b.width,
                };
                children.push(Child { dx: col_x[c] + pad, dy, b });
            }
        }

        // height = extent above baseline = stack_top − shift (shift pushes rows down);
        // depth  = extent below baseline = stack_bottom + shift.
        let height = stack_top - shift;
        let depth = stack_bottom + shift;

        // Vertical `|` rules: each spans the full grid body (top → bottom). A `Rule`
        // is width×thickness extending `thickness` up from its child baseline, so a
        // thin (`width = rule_thickness`), tall (`thickness = body_height`) rule
        // placed with its baseline at the grid bottom (`dy = depth`) fills the body.
        let body_height = height + depth;
        if body_height > 0.0 {
            let rcolor = ctx.cur_color.get();
            for &(rx, n) in &vrules {
                for k in 0..n {
                    let dx = rx + k as f32 * (rule_thickness + double_gap);
                    children.push(Child {
                        dx,
                        dy: depth,
                        b: Box {
                            width: rule_thickness,
                            height: body_height,
                            depth: 0.0,
                            kind: BoxKind::Rule {
                                width: rule_thickness,
                                thickness: body_height,
                                color: rcolor,
                            },
                        },
                    });
                }
            }
            // Horizontal `\hline` rules at row boundaries. Boundary `b` sits above
            // row `b` (b in 0..rows): the top edge for b=0, midway between adjacent
            // rows otherwise; boundary `rows.len()` is the bottom edge.
            let boundary_dy = |b: usize| -> f32 {
                if b == 0 {
                    -height
                } else if b >= rows.len() {
                    depth
                } else {
                    let above = row_y[b - 1] + shift + row_d[b - 1];
                    let below = row_y[b] + shift - row_h[b];
                    (above + below) / 2.0
                }
            };
            for (b, &n) in row_lines.iter().enumerate() {
                for k in 0..n {
                    // Stack the rules of `\hline\hline` a hair apart.
                    let dy = boundary_dy(b) + k as f32 * (rule_thickness + double_gap);
                    children.push(Child {
                        dx: 0.0,
                        // `Rule` extends `thickness` upward from the child baseline, so
                        // offset down by `rule_thickness` to center the line on `dy`.
                        dy: dy + rule_thickness / 2.0,
                        b: Box {
                            width: grid_w,
                            height: rule_thickness,
                            depth: 0.0,
                            kind: BoxKind::Rule { width: grid_w, thickness: rule_thickness, color: rcolor },
                        },
                    });
                }
            }
        }

        let grid = Box {
            width: grid_w,
            height: height.max(0.0),
            depth: depth.max(0.0),
            kind: BoxKind::Hbox { children },
        };

        // `cases`: prepend a large left brace sized to the grid, no right delim.
        if matches!(kind, MatrixKind::Cases) {
            let target = (grid.height - axis).max(grid.depth + axis).max(0.0) * 2.0;
            let brace = delim::sized_delim(ctx.face, '{', target, axis, ctx.base_em, ctx.cur_color.get());
            let gap = 0.16 * ctx.base_em; // nib-to-content space (TeX ~ \nulldelimiterspace-ish)
            let mut kids: Vec<Child> = Vec::new();
            let mut pen = 0.0f32;
            let mut h = grid.height;
            let mut d = grid.depth;
            if let Some(b) = brace {
                h = h.max(b.height);
                d = d.max(b.depth);
                let w = b.width;
                kids.push(Child { dx: pen, dy: 0.0, b });
                pen += w + gap;
            }
            let gw = grid.width;
            kids.push(Child { dx: pen, dy: 0.0, b: grid });
            pen += gw;
            return Some(Box {
                width: pen,
                height: h,
                depth: d,
                kind: BoxKind::Hbox { children: kids },
            });
        }

        Some(grid)
    }
}
