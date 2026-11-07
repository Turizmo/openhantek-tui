//! Braille plot renderer for the scope screen.
//!
//! A cell holds 2 x 4 braille dots and one colour. Each division is a whole
//! number of cells, so the grid looks the same in every division. Traces
//! are drawn in one of the [`TraceStyle`]s: classic lines from point to
//! point, one dot thick (one vertical run of dots per dot column), bold,
//! text lines with no braille at all (`|`, `/`, `\`, `_`, `-`, `‾`), or
//! connected box-drawing lines (`─`, `│`, `╭`, `╮`, `╰`, `╯`).

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

use crate::settings::TraceStyle;
use crate::theme::Palette;

/// Draw layers, lowest first. A higher layer takes over a cell from a
/// lower one, so trace colours never mix in one cell.
pub const LAYER_CURSOR: u8 = 1;
/// The oldest old trace of the persistence. Each newer old frame has the
/// next layer, so a newer frame wins a cell over an older one.
pub const LAYER_PERSIST: u8 = 2;
/// Layers for old frames: the deepest persistence.
pub const PERSIST_LAYERS: u8 = 16;
pub const LAYER_MATH: u8 = LAYER_PERSIST + PERSIST_LAYERS;
pub const LAYER_CH2: u8 = LAYER_MATH + 1;
pub const LAYER_CH1: u8 = LAYER_MATH + 2;
/// The selected trace, drawn above all others.
pub const LAYER_TOP: u8 = LAYER_MATH + 3;

/// Layer of old frame `i` of `n` (0 = oldest): newer frames higher.
pub fn persist_layer(i: usize, n: usize) -> u8 {
    let skip = n.saturating_sub(PERSIST_LAYERS as usize);
    LAYER_PERSIST + i.saturating_sub(skip).min(PERSIST_LAYERS as usize - 1) as u8
}

fn is_persist(layer: u8) -> bool {
    (LAYER_PERSIST..LAYER_PERSIST + PERSIST_LAYERS).contains(&layer)
}

/// Divisions across and up the screen.
pub const H_DIVS: usize = 10;
pub const V_DIVS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    /// Braille dot bits (U+2800 + dots).
    pub dots: u8,
    /// Layer that owns the dots, 0 when there are none.
    pub layer: u8,
    pub color: Color,
    /// Trace character of the line style, owned by `layer` like the dots.
    pub glyph: Option<char>,
    /// Grid character, shown when the cell has no dots or glyph.
    pub text: Option<(char, Color)>,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            dots: 0,
            layer: 0,
            color: Color::Reset,
            glyph: None,
            text: None,
        }
    }
}

impl Cell {
    /// The character this cell shows, if any.
    pub fn symbol(&self) -> Option<(char, Color)> {
        if self.dots != 0 {
            char::from_u32(0x2800 + self.dots as u32).map(|c| (c, self.color))
        } else if let Some(c) = self.glyph {
            Some((c, self.color))
        } else {
            self.text
        }
    }
}

/// Sides a box character of the box style connects, as bits.
const BOX_UP: u8 = 1;
const BOX_DOWN: u8 = 2;
const BOX_LEFT: u8 = 4;
const BOX_RIGHT: u8 = 8;

/// Box characters by the sides they connect (up, down, left, right
/// bits). A cell with no sides is a point: `─`.
const BOX_CHARS: [char; 16] = [
    '─', '╵', '╷', '│', '╴', '╯', '╮', '┤', '╶', '╰', '╭', '├', '─', '┴', '┬', '┼',
];

fn box_char(sides: u8) -> char {
    BOX_CHARS[(sides & 15) as usize]
}

/// Sides of a box character, 0 for any other character.
fn box_sides(c: char) -> u8 {
    BOX_CHARS
        .iter()
        .skip(1)
        .position(|&b| b == c)
        .map_or(0, |i| i as u8 + 1)
}

/// Braille dot bit for dot column `dx` (0..2) and dot row `dy` (0..4).
fn dot_bit(dx: usize, dy: usize) -> u8 {
    const BITS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];
    BITS[dx][dy]
}

/// A plot of 10 x 8 divisions. `cx` cells per division across, `ry` cells
/// per division up. The plot is `10 * cx + 1` cells wide and `8 * ry + 1`
/// high: the extra cell holds the last grid line.
#[derive(Clone, Debug)]
pub struct Plot {
    pub cx: usize,
    pub ry: usize,
    pub cols: usize,
    pub rows: usize,
    cells: Vec<Cell>,
    /// Layers up to this one render with the DIM modifier (the older old
    /// traces in 16 colours). 0: none.
    dim_upto: u8,
}

impl Plot {
    pub fn new(cx: usize, ry: usize) -> Self {
        let cx = cx.max(1);
        let ry = ry.max(1);
        let cols = H_DIVS * cx + 1;
        let rows = V_DIVS * ry + 1;
        Self {
            cx,
            ry,
            cols,
            rows,
            cells: vec![Cell::default(); cols * rows],
            dim_upto: 0,
        }
    }

    /// Show the cells of layers up to `layer` with the DIM modifier.
    pub fn dim_upto(&mut self, layer: u8) {
        self.dim_upto = layer;
    }

    /// Dot columns and dot rows.
    pub fn dots_w(&self) -> usize {
        self.cols * 2
    }

    pub fn dots_h(&self) -> usize {
        self.rows * 4
    }

    /// Dot x (continuous) of screen x in divisions (-5 .. 5). Division
    /// lines fall on the left dot column of their grid cell.
    pub fn px(&self, x_div: f64) -> f64 {
        (x_div + H_DIVS as f64 / 2.0) * (2 * self.cx) as f64 + 0.5
    }

    /// Dot y (continuous, 0 at the top) of screen y in divisions (-4 .. 4).
    /// Division lines fall on dot row 1 of their grid cell.
    pub fn py(&self, y_div: f64) -> f64 {
        (V_DIVS as f64 / 2.0 - y_div) * (4 * self.ry) as f64 + 1.5
    }

    /// Cell column of screen x in divisions.
    pub fn col_of(&self, x_div: f64) -> usize {
        ((self.px(x_div) / 2.0).floor().max(0.0) as usize).min(self.cols - 1)
    }

    /// Cell row of screen y in divisions.
    pub fn row_of(&self, y_div: f64) -> usize {
        ((self.py(y_div) / 4.0).floor().max(0.0) as usize).min(self.rows - 1)
    }

    pub fn cell(&self, col: usize, row: usize) -> &Cell {
        &self.cells[row * self.cols + col]
    }

    /// The character at a cell, or a space.
    pub fn char_at(&self, col: usize, row: usize) -> char {
        self.cell(col, row).symbol().map_or(' ', |(c, _)| c)
    }

    /// Is the dot at dot column `x`, dot row `y` set?
    pub fn dot(&self, x: usize, y: usize) -> bool {
        if x >= self.dots_w() || y >= self.dots_h() {
            return false;
        }
        self.cell(x / 2, y / 4).dots & dot_bit(x % 2, y % 4) != 0
    }

    /// Set one dot. Out-of-range dots are ignored.
    pub fn set(&mut self, x: i64, y: i64, layer: u8, color: Color) {
        if x < 0 || y < 0 || x >= self.dots_w() as i64 || y >= self.dots_h() as i64 {
            return;
        }
        let (x, y) = (x as usize, y as usize);
        let cell = &mut self.cells[(y / 4) * self.cols + x / 2];
        let bit = dot_bit(x % 2, y % 4);
        if layer > cell.layer {
            cell.dots = bit;
            cell.glyph = None;
            cell.layer = layer;
            cell.color = color;
        } else if layer == cell.layer {
            cell.dots |= bit;
        }
    }

    /// Dots from row `y0` to row `y1` (both included) in dot column `x`.
    pub fn vrun(&mut self, x: i64, y0: i64, y1: i64, layer: u8, color: Color) {
        let (a, b) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
        let a = a.max(0);
        let b = b.min(self.dots_h() as i64 - 1);
        for y in a..=b {
            self.set(x, y, layer, color);
        }
    }

    /// Grid characters: `·` on division points, and a brighter center
    /// cross. Ticks at 1/5 division on the center lines when a division
    /// is a multiple of 5 cells, and at least 10 (so ticks have gaps).
    pub fn grid(&mut self, pal: &Palette) {
        let (cx, ry) = (self.cx, self.ry);
        let (mid_col, mid_row) = (H_DIVS / 2 * cx, V_DIVS / 2 * ry);
        let tick_x = |col: usize| cx.is_multiple_of(5) && cx >= 10 && col.is_multiple_of(cx / 5);
        let tick_y = |row: usize| ry.is_multiple_of(5) && ry >= 10 && row.is_multiple_of(ry / 5);
        for row in 0..self.rows {
            for col in 0..self.cols {
                let on_col = col % cx == 0;
                let on_row = row % ry == 0;
                let text = if row == mid_row {
                    Some(if on_col {
                        '┼'
                    } else if tick_x(col) {
                        '┴'
                    } else {
                        '╌'
                    })
                    .map(|c| (c, pal.axis))
                } else if col == mid_col {
                    Some(if on_row {
                        '┼'
                    } else if tick_y(row) {
                        '├'
                    } else {
                        '╎'
                    })
                    .map(|c| (c, pal.axis))
                } else if on_col && on_row {
                    Some(('·', pal.grid))
                } else {
                    None
                };
                self.cells[row * self.cols + col].text = text;
            }
        }
    }

    /// Draw a trace through `points` (in divisions, in time order) in
    /// `style`. With `dots`, only the points, no lines between them.
    pub fn styled(
        &mut self,
        points: &[(f64, f64)],
        style: TraceStyle,
        dots: bool,
        layer: u8,
        color: Color,
    ) {
        match (style, dots) {
            (TraceStyle::Classic, _) => self.classic(points, dots, layer, color),
            (TraceStyle::Thin, false) => self.trace(points, layer, color),
            (TraceStyle::Thin, true) => self.points(points, layer, color),
            (TraceStyle::Bold, false) => self.trace_run(points, true, layer, color),
            (TraceStyle::Bold, true) => self.points_bold(points, layer, color),
            (TraceStyle::Line, false) => self.text_line(points, layer, color),
            (TraceStyle::Line, true) => self.text_points(points, layer, color),
            (TraceStyle::Box, false) => self.box_line(points, layer, color),
            (TraceStyle::Box, true) => self.box_points(points, layer, color),
        }
    }

    /// Put a trace character in a cell. A layer at least as high as the
    /// cell's takes it over: a later trace on the same layer wins.
    pub fn put_glyph(&mut self, col: usize, row: usize, ch: char, layer: u8, color: Color) {
        if col >= self.cols || row >= self.rows {
            return;
        }
        let cell = &mut self.cells[row * self.cols + col];
        if layer >= cell.layer {
            cell.dots = 0;
            cell.glyph = Some(ch);
            cell.layer = layer;
            cell.color = color;
        }
    }

    /// Flat character for dot y `y` (continuous): `‾`, `-` or `_` for the
    /// top, middle or bottom third of its cell.
    fn flat_char(y: f64) -> char {
        let f = y / 4.0 - (y / 4.0).floor();
        if f < 1.0 / 3.0 {
            '‾'
        } else if f < 2.0 / 3.0 {
            '-'
        } else {
            '_'
        }
    }

    /// The line style: one text character per cell, no braille. Each cell
    /// column gets the low and high value of the trace in it, and the
    /// values where the trace comes in and goes out. A column inside one
    /// cell row is flat (`_`, `-`, `‾` at its height) or a slope (`/`,
    /// `\`). A column over more cell rows is a slope on gentle runs and
    /// stacked `|` on steep ones. A non-finite point breaks the line.
    pub fn text_line(&mut self, points: &[(f64, f64)], layer: u8, color: Color) {
        let w = self.cols;
        let y_max = self.dots_h() as f64 - 1e-6;
        // (low, high, first, last) dot y per cell column.
        let mut cols: Vec<Option<(f64, f64, f64, f64)>> = vec![None; w];
        let mut add = |c: i64, y: f64| {
            if c < 0 || c >= w as i64 {
                return;
            }
            let r = &mut cols[c as usize];
            *r = Some(r.map_or((y, y, y, y), |(lo, hi, first, _)| {
                (lo.min(y), hi.max(y), first, y)
            }));
        };
        let mut prev: Option<(f64, f64)> = None;
        for &(x, y) in points {
            if !x.is_finite() || !y.is_finite() {
                prev = None;
                continue;
            }
            // Cell x (continuous) and dot y.
            let p = (self.px(x) / 2.0, self.py(y).clamp(0.0, y_max));
            let col = p.0.floor() as i64;
            if let Some(a) = prev {
                let ca = a.0.floor() as i64;
                if col > ca {
                    // As in `trace_run`: the step from `a` goes to the
                    // columns after `a`, so an edge lands in one column.
                    let at = |x: f64| {
                        if p.0 - a.0 <= f64::EPSILON {
                            p.1
                        } else {
                            a.1 + (p.1 - a.1) * (x - a.0) / (p.0 - a.0)
                        }
                    };
                    let first = (ca + 1).max(0);
                    let last = col.min(w as i64 - 1);
                    for c in first..=last {
                        let x0 = if c == ca + 1 { a.0 } else { c as f64 };
                        let x1 = if c == col { p.0 } else { (c + 1) as f64 };
                        add(c, at(x0));
                        add(c, at(x1));
                    }
                }
            }
            add(col, p.1);
            prev = Some(p);
        }
        for (c, r) in cols.iter().enumerate() {
            if let Some((lo, hi, first, last)) = *r {
                self.text_column(c, lo, hi, first, last, layer, color);
            }
        }
    }

    /// Characters of one cell column of [`Plot::text_line`], from dot y
    /// `lo` (top) to `hi`, entering at `first` and leaving at `last`.
    #[allow(clippy::too_many_arguments)]
    fn text_column(
        &mut self,
        c: usize,
        lo: f64,
        hi: f64,
        first: f64,
        last: f64,
        layer: u8,
        color: Color,
    ) {
        // In cells: 4 dot rows per cell row.
        let span = (hi - lo) / 4.0;
        let rise = (first - last) / 4.0;
        let slope = if rise > 0.0 { '/' } else { '\\' };
        // Most of the column goes one way: a slope, not a peak or noise.
        let monotonic = rise.abs() >= 0.6 * span;
        // Flat, or a peak or dip that fits in about one cell: one flat
        // character, at the far end from where the line comes in and
        // goes out.
        if span < 0.6 || (!monotonic && span < 1.5) {
            let ends = (first + last) / 2.0;
            let y = if span < 0.6 {
                (lo + hi) / 2.0
            } else if ends - lo > hi - ends {
                lo
            } else {
                hi
            };
            self.put_glyph(c, (y / 4.0) as usize, Self::flat_char(y), layer, color);
            return;
        }
        let (r0, r1) = ((lo / 4.0).floor() as usize, (hi / 4.0).floor() as usize);
        let ch = if monotonic && span < 2.0 { slope } else { '|' };
        // Rows the run covers by at least a third of a cell; at least one.
        let cover = |r: usize| {
            let (a, b) = (r as f64 * 4.0, (r + 1) as f64 * 4.0);
            (hi.min(b) - lo.max(a)) / 4.0
        };
        let rows: Vec<usize> = (r0..=r1).filter(|&r| cover(r) >= 1.0 / 3.0).collect();
        if rows.is_empty() {
            let best = (r0..=r1)
                .max_by(|&a, &b| cover(a).total_cmp(&cover(b)))
                .unwrap_or(r0);
            self.put_glyph(c, best, ch, layer, color);
        }
        for r in rows {
            self.put_glyph(c, r, ch, layer, color);
        }
    }

    /// Dots mode of the line style: `'`, `·` or `.` per point, at the top,
    /// middle or bottom of its cell.
    pub fn text_points(&mut self, points: &[(f64, f64)], layer: u8, color: Color) {
        let y_max = self.dots_h() as f64 - 1e-6;
        for &(x, y) in points {
            if !x.is_finite() || !y.is_finite() {
                continue;
            }
            let (px, py) = (self.px(x), self.py(y).clamp(0.0, y_max));
            if px < 0.0 || px >= self.dots_w() as f64 {
                continue;
            }
            let ch = match Self::flat_char(py) {
                '‾' => '\'',
                '-' => '·',
                _ => '.',
            };
            self.put_glyph((px / 2.0) as usize, (py / 4.0) as usize, ch, layer, color);
        }
    }

    /// The box style: the trace as one connected line of box-drawing
    /// characters (`─`, `│`, `╭`, `╮`, `╰`, `╯`, ...), one cell row per
    /// level. The line crosses the side between two cell columns at the
    /// row of the trace there. Inside a column it runs up or down from
    /// the row it comes in at to the row it goes out at, and on to the
    /// low and high rows of the column when the trace gets to the middle
    /// of them (peaks and noise). Each cell gets the
    /// character for the sides it connects, so the line has no gaps. A
    /// non-finite point breaks the line.
    pub fn box_line(&mut self, points: &[(f64, f64)], layer: u8, color: Color) {
        let w = self.cols;
        let y_max = self.dots_h() as f64 - 1e-6;
        // Low and high dot y per cell column.
        let mut span: Vec<Option<(f64, f64)>> = vec![None; w];
        // Dot y where the line crosses the left side of column b; b = w
        // is the right side of the last column.
        let mut cross: Vec<Option<f64>> = vec![None; w + 1];
        fn add(span: &mut [Option<(f64, f64)>], c: i64, y: f64) {
            if c < 0 || c >= span.len() as i64 {
                return;
            }
            let r = &mut span[c as usize];
            *r = Some(r.map_or((y, y), |(lo, hi)| (lo.min(y), hi.max(y))));
        }
        // A line that starts or ends in the first or last column goes on
        // off the screen: it crosses the plot side at its end point.
        let end_at_side = |cross: &mut [Option<f64>], p: Option<(f64, f64)>| {
            if let Some(p) = p {
                let side = if p.0 < 1.0 { 0 } else { w };
                if (p.0 < 1.0 || p.0 >= (w - 1) as f64) && cross[side].is_none() {
                    cross[side] = Some(p.1);
                }
            }
        };
        let mut prev: Option<(f64, f64)> = None;
        for &(x, y) in points {
            if !x.is_finite() || !y.is_finite() {
                end_at_side(&mut cross, prev);
                prev = None;
                continue;
            }
            // Cell x (continuous) and dot y.
            let p = (self.px(x) / 2.0, self.py(y).clamp(0.0, y_max));
            match prev {
                None => end_at_side(&mut cross, Some(p)),
                Some(q) => {
                    let (a, b) = if q.0 <= p.0 { (q, p) } else { (p, q) };
                    let at = |x: f64| {
                        if b.0 - a.0 <= f64::EPSILON {
                            b.1
                        } else {
                            a.1 + (b.1 - a.1) * (x - a.0) / (b.0 - a.0)
                        }
                    };
                    // As in `trace_run`: the step from `a` goes to the
                    // columns after `a`, and the line leaves the column of
                    // `a` at its level, so an edge lands in one column.
                    let (ca, cb) = (a.0.floor() as i64, b.0.floor() as i64);
                    let x_at = |c: i64| if c == ca + 1 { a.0 } else { c as f64 };
                    for c in (ca + 1).max(0)..=cb.min(w as i64 - 1) {
                        add(&mut span, c, at(x_at(c)));
                        add(&mut span, c, at(if c == cb { b.0 } else { (c + 1) as f64 }));
                    }
                    for side in (ca + 1).max(0)..=cb.min(w as i64) {
                        cross[side as usize] = Some(at(x_at(side)));
                    }
                }
            }
            add(&mut span, p.0.floor() as i64, p.1);
            prev = Some(p);
        }
        end_at_side(&mut cross, prev);
        let rows = self.rows;
        let row = move |y: f64| ((y / 4.0).floor().max(0.0) as usize).min(rows - 1);
        // How far (in dots) the line on a side must get into a row to
        // reach it.
        const REACH: f64 = 1.0;
        // How far a peak or noise inside a column must get into a row to
        // reach it: to the middle of the row, where its line is.
        const PEAK: f64 = 2.0;
        // Row of the line on each side. A side that gets less than REACH
        // into another row than the sides around it (a peak that just
        // touches the row above) stays in their row, so the peak is flat.
        let mut side_row: Vec<Option<usize>> = (0..=w)
            .map(|s| {
                let y = cross[s]?;
                let r = row(y);
                let near = |o: Option<usize>| o.and_then(|o| cross[o]).map(row);
                let (a, b) = (near(s.checked_sub(1)), near((s < w).then_some(s + 1)));
                Some(match (a, b) {
                    (Some(a), Some(b)) if a == b && a.abs_diff(r) == 1 => {
                        let border = 4.0 * r.max(a) as f64;
                        if (y - border).abs() < REACH {
                            a
                        } else {
                            r
                        }
                    }
                    _ => r,
                })
            })
            .collect();
        // Low and high row of each column, from the trace in it.
        let reach: Vec<Option<(usize, usize)>> = span
            .iter()
            .map(|r| {
                r.map(|(lo, hi)| {
                    if hi - lo > 2.0 * PEAK {
                        (row(lo + PEAK), row(hi - PEAK))
                    } else {
                        let mid = row((lo + hi) / 2.0);
                        (mid, mid)
                    }
                })
            })
            .collect();
        // A peak one row above (or a dip one row below) the line on both
        // sides of its column: the line goes up on the side nearer the
        // peak and turns in the next column (`╭╮` over `╯╰`), not a loose
        // `╷` over `┴`.
        for c in 0..w {
            let (Some((top, bottom)), Some(l), Some(r)) = (reach[c], side_row[c], side_row[c + 1])
            else {
                continue;
            };
            let (Some(yl), Some(yr), Some((lo, hi))) = (cross[c], cross[c + 1], span[c]) else {
                continue;
            };
            let (low, high) = (l.min(r), l.max(r));
            if top + 1 == low && bottom <= high {
                let s = if (yl - lo).abs() < (yr - lo).abs() {
                    c
                } else {
                    c + 1
                };
                side_row[s] = Some(top);
            } else if bottom == high + 1 && top >= low {
                let s = if (yl - hi).abs() < (yr - hi).abs() {
                    c
                } else {
                    c + 1
                };
                side_row[s] = Some(bottom);
            }
        }
        for c in 0..w {
            let Some((mut top, mut bottom)) = reach[c] else {
                continue;
            };
            let (left, right) = (side_row[c], side_row[c + 1]);
            for r in [left, right].into_iter().flatten() {
                top = top.min(r);
                bottom = bottom.max(r);
            }
            for r in top..=bottom {
                let mut sides = 0;
                if r > top {
                    sides |= BOX_UP;
                }
                if r < bottom {
                    sides |= BOX_DOWN;
                }
                if left == Some(r) {
                    sides |= BOX_LEFT;
                }
                if right == Some(r) {
                    sides |= BOX_RIGHT;
                }
                self.put_box(c, r, sides, layer, color);
            }
        }
    }

    /// Put the box character for `sides` in a cell. On a cell that already
    /// has a box character of the same layer and colour, the sides of both
    /// are joined. Old traces of the persistence join each other too: the
    /// joined character takes the layer and colour of the newer one, so
    /// old traces stay connected lines instead of being cut where a newer
    /// one crosses them.
    fn put_box(&mut self, col: usize, row: usize, sides: u8, layer: u8, color: Color) {
        if col >= self.cols || row >= self.rows {
            return;
        }
        let cell = &self.cells[row * self.cols + col];
        let joins = (cell.layer == layer && cell.color == color)
            || (is_persist(layer) && is_persist(cell.layer) && layer >= cell.layer);
        let old = match cell.glyph {
            Some(g) if joins => box_sides(g),
            _ => 0,
        };
        self.put_glyph(col, row, box_char(sides | old), layer, color);
    }

    /// Dots mode of the box style: `•` per point, in the cell row of the
    /// point.
    pub fn box_points(&mut self, points: &[(f64, f64)], layer: u8, color: Color) {
        let y_max = self.dots_h() as f64 - 1e-6;
        for &(x, y) in points {
            if !x.is_finite() || !y.is_finite() {
                continue;
            }
            let (px, py) = (self.px(x), self.py(y).clamp(0.0, y_max));
            if px < 0.0 || px >= self.dots_w() as f64 {
                continue;
            }
            self.put_glyph((px / 2.0) as usize, (py / 4.0) as usize, '•', layer, color);
        }
    }

    /// The classic look: straight lines from point to point. More than two
    /// points per dot column are cut to the low and high value of each
    /// column first, low then high, so a noisy or steep trace is drawn as
    /// a zigzag that fills its column. With `dots`, only those points.
    pub fn classic(&mut self, points: &[(f64, f64)], dots: bool, layer: u8, color: Color) {
        let w = self.dots_w();
        let finite = |p: &&(f64, f64)| p.0.is_finite() && p.1.is_finite();
        let n = points.iter().filter(finite).count();
        // Dot positions; `None` breaks the line.
        let pts: Vec<Option<(i64, i64)>> = if n > 2 * w {
            let mut cols: Vec<Option<(f64, f64)>> = vec![None; w];
            for p in points.iter().filter(finite) {
                let c = self.px(p.0).floor();
                if c < 0.0 || c >= w as f64 {
                    continue;
                }
                let r = &mut cols[c as usize];
                *r = Some(r.map_or((p.1, p.1), |(lo, hi)| (lo.min(p.1), hi.max(p.1))));
            }
            cols.iter()
                .enumerate()
                .filter_map(|(c, r)| r.map(|(lo, hi)| (c as i64, lo, hi)))
                .flat_map(|(c, lo, hi)| {
                    [
                        Some((c, self.py(lo).floor() as i64)),
                        Some((c, self.py(hi).floor() as i64)),
                    ]
                })
                .collect()
        } else {
            points
                .iter()
                .map(|p| {
                    finite(&p).then(|| (self.px(p.0).floor() as i64, self.py(p.1).floor() as i64))
                })
                .collect()
        };
        let mut prev: Option<(i64, i64)> = None;
        for p in pts {
            match (p, prev) {
                (Some(b), Some(a)) if !dots => self.line(a, b, layer, color),
                (Some(b), _) => self.set(b.0, b.1, layer, color),
                _ => {}
            }
            prev = p;
        }
    }

    /// Draw a trace through `points` (in divisions, in time order).
    /// Each dot column gets one vertical run: from the last value of the
    /// column before, through all samples in this column. A non-finite
    /// point breaks the line.
    pub fn trace(&mut self, points: &[(f64, f64)], layer: u8, color: Color) {
        self.trace_run(points, false, layer, color);
    }

    /// [`Plot::trace`], two dots wide and high with `bold`.
    fn trace_run(&mut self, points: &[(f64, f64)], bold: bool, layer: u8, color: Color) {
        let w = self.dots_w();
        let y_max = self.dots_h() as f64 - 1e-6;
        let mut ranges: Vec<Option<(f64, f64)>> = vec![None; w];
        let mut add = |c: i64, lo: f64, hi: f64| {
            if c < 0 || c >= w as i64 {
                return;
            }
            let r = &mut ranges[c as usize];
            *r = Some(r.map_or((lo, hi), |(a, b)| (a.min(lo), b.max(hi))));
        };
        let mut prev: Option<(f64, f64)> = None;
        for &(x, y) in points {
            if !x.is_finite() || !y.is_finite() {
                prev = None;
                continue;
            }
            let p = (self.px(x), self.py(y).clamp(0.0, y_max));
            let col = p.0.floor() as i64;
            add(col, p.1, p.1);
            if let Some(a) = prev {
                let ca = a.0.floor() as i64;
                if col > ca {
                    // The step from `a` goes to the columns after `a`: the
                    // first of them also takes the part inside `a`'s column.
                    // So an edge between two samples lands in one column.
                    let at = |x: f64| {
                        if p.0 - a.0 <= f64::EPSILON {
                            p.1
                        } else {
                            a.1 + (p.1 - a.1) * (x - a.0) / (p.0 - a.0)
                        }
                    };
                    // Only columns on the plot: points just off the edge
                    // still draw the part that is on it.
                    let first = (ca + 1).max(0);
                    let last = col.min(w as i64 - 1);
                    for c in first..=last {
                        let x0 = if c == ca + 1 { a.0 } else { c as f64 };
                        let x1 = if c == col { p.0 } else { (c + 1) as f64 };
                        let (y0, y1) = (at(x0), at(x1));
                        add(c, y0.min(y1), y0.max(y1));
                    }
                }
            }
            prev = Some(p);
        }
        let mut last: Option<(i64, i64)> = None;
        for (c, r) in ranges.iter().enumerate() {
            let Some((lo, hi)) = *r else {
                last = None;
                continue;
            };
            let (mut a, mut b) = (lo.floor() as i64, hi.floor() as i64);
            // After a one-dot column, do not draw its row again: the line
            // stays one dot thick on slow slopes.
            if let Some((pa, pb)) = last {
                if pa == pb && b > a {
                    if a == pa {
                        a += 1;
                    } else if b == pa {
                        b -= 1;
                    }
                }
            }
            self.vrun(c as i64, a, b, layer, color);
            if bold {
                self.vrun(c as i64, a, b + 1, layer, color);
                self.vrun(c as i64 + 1, a, b + 1, layer, color);
            }
            last = Some((a, b));
        }
    }

    /// One dot per point (in divisions).
    pub fn points(&mut self, points: &[(f64, f64)], layer: u8, color: Color) {
        for &(x, y) in points {
            if x.is_finite() && y.is_finite() {
                self.set(
                    self.px(x).floor() as i64,
                    self.py(y).floor() as i64,
                    layer,
                    color,
                );
            }
        }
    }

    /// Two by two dots per point (in divisions).
    pub fn points_bold(&mut self, points: &[(f64, f64)], layer: u8, color: Color) {
        for &(x, y) in points {
            if x.is_finite() && y.is_finite() {
                let (x, y) = (self.px(x).floor() as i64, self.py(y).floor() as i64);
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    self.set(x + dx, y + dy, layer, color);
                }
            }
        }
    }

    /// Straight lines between points (in divisions), in any direction.
    /// Used for XY mode.
    pub fn polyline(&mut self, points: &[(f64, f64)], layer: u8, color: Color) {
        let mut prev: Option<(i64, i64)> = None;
        for &(x, y) in points {
            if !x.is_finite() || !y.is_finite() {
                prev = None;
                continue;
            }
            let p = (self.px(x).floor() as i64, self.py(y).floor() as i64);
            match prev {
                Some(a) => self.line(a, p, layer, color),
                None => self.set(p.0, p.1, layer, color),
            }
            prev = Some(p);
        }
    }

    /// Bresenham line between two dots.
    pub fn line(&mut self, a: (i64, i64), b: (i64, i64), layer: u8, color: Color) {
        let (mut x, mut y) = a;
        let dx = (b.0 - a.0).abs();
        let dy = -(b.1 - a.1).abs();
        let sx = if a.0 < b.0 { 1 } else { -1 };
        let sy = if a.1 < b.1 { 1 } else { -1 };
        let mut err = dx + dy;
        // A bound, in case of a huge line far off the plot.
        let limit = (dx - dy + 1).min(4 * (self.dots_w() + self.dots_h()) as i64);
        for _ in 0..limit {
            self.set(x, y, layer, color);
            if x == b.0 && y == b.1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// Dashed vertical line at screen x (divisions).
    pub fn dashed_v(&mut self, x_div: f64, layer: u8, color: Color) {
        let x = self.px(x_div).floor() as i64;
        for y in 0..self.dots_h() as i64 {
            if y % 4 < 2 {
                self.set(x, y, layer, color);
            }
        }
    }

    /// Dashed horizontal line at screen y (divisions).
    pub fn dashed_h(&mut self, y_div: f64, layer: u8, color: Color) {
        let y = self.py(y_div).floor() as i64;
        for x in 0..self.dots_w() as i64 {
            if x % 4 < 2 {
                self.set(x, y, layer, color);
            }
        }
    }

    /// The classic cursor line from `a` to `b` (in divisions): 81 points
    /// along it, two on and two off, so the dashes have the same length
    /// at every plot size.
    pub fn dashed_classic(&mut self, a: (f64, f64), b: (f64, f64), layer: u8, color: Color) {
        let steps = 80;
        for i in (0..=steps).filter(|i| i % 4 < 2) {
            let k = i as f64 / steps as f64;
            let (x, y) = (a.0 + (b.0 - a.0) * k, a.1 + (b.1 - a.1) * k);
            // The far edge maps one dot past the plot: keep it on.
            let px = (self.px(x).floor() as i64).min(self.dots_w() as i64 - 1);
            let py = (self.py(y).floor() as i64).min(self.dots_h() as i64 - 1);
            self.set(px, py, layer, color);
        }
    }

    /// Write the plot into a buffer at `area` (top-left corner). Cells with
    /// nothing in them are left as they are.
    pub fn render(&self, buf: &mut Buffer, area: Rect) {
        for row in 0..self.rows.min(area.height as usize) {
            for col in 0..self.cols.min(area.width as usize) {
                let src = self.cell(col, row);
                if let Some((c, color)) = src.symbol() {
                    let pos = (area.x + col as u16, area.y + row as u16);
                    if let Some(cell) = buf.cell_mut(pos) {
                        cell.set_char(c).set_fg(color);
                        let faint = src.layer != 0
                            && src.layer <= self.dim_upto
                            && (src.dots != 0 || src.glyph.is_some());
                        if faint {
                            cell.modifier.insert(ratatui::style::Modifier::DIM);
                        }
                    }
                }
            }
        }
    }
}

/// Space around the plot for labels and markers, in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Margins {
    pub left: u16,
    pub right: u16,
    pub top: u16,
    pub bottom: u16,
}

/// Width of the value labels on the left.
pub const LABEL_W: u16 = 7;
/// Width of the `1▸` markers between the labels and the plot.
pub const MARKER_W: u16 = 2;

impl Margins {
    /// Labels, markers, trigger marks and time labels.
    pub const FULL: Margins = Margins {
        left: LABEL_W + MARKER_W,
        right: 2,
        top: 1,
        bottom: 1,
    };
    /// Markers only, no labels above or below.
    pub const SIDES: Margins = Margins {
        left: LABEL_W + MARKER_W,
        right: 2,
        top: 0,
        bottom: 0,
    };
    pub const MARKERS: Margins = Margins {
        left: MARKER_W,
        right: 1,
        top: 0,
        bottom: 0,
    };
    pub const NONE: Margins = Margins {
        left: 0,
        right: 0,
        top: 0,
        bottom: 0,
    };
}

/// Where the plot and its margins go on the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub cx: usize,
    pub ry: usize,
    /// The plot cells.
    pub plot: Rect,
    pub margins: Margins,
}

impl Geometry {
    /// The biggest plot with whole cells per division that fits in `area`,
    /// centered. Tries smaller margins when the area is small.
    pub fn fit(area: Rect) -> Option<Geometry> {
        for m in [
            Margins::FULL,
            Margins::SIDES,
            Margins::MARKERS,
            Margins::NONE,
        ] {
            let w = area.width.saturating_sub(m.left + m.right) as usize;
            let h = area.height.saturating_sub(m.top + m.bottom) as usize;
            if w < H_DIVS + 1 || h < V_DIVS + 1 {
                continue;
            }
            let cx = (w - 1) / H_DIVS;
            let ry = (h - 1) / V_DIVS;
            let pw = (H_DIVS * cx + 1) as u16;
            let ph = (V_DIVS * ry + 1) as u16;
            let used_w = pw + m.left + m.right;
            let used_h = ph + m.top + m.bottom;
            let x = area.x + (area.width - used_w) / 2 + m.left;
            let y = area.y + (area.height - used_h) / 2 + m.top;
            return Some(Geometry {
                cx,
                ry,
                plot: Rect::new(x, y, pw, ph),
                margins: m,
            });
        }
        None
    }

    /// Rows of the plot when it is `ry` cells per division, with margins.
    pub fn height_for(ry: usize) -> u16 {
        (V_DIVS * ry + 1) as u16 + Margins::FULL.top + Margins::FULL.bottom
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ColorMode, Palette};

    const RED: Color = Color::Red;
    const BLUE: Color = Color::Blue;
    const GREEN: Color = Color::Green;

    /// Dot rows set in dot column `x`.
    fn column(p: &Plot, x: usize) -> Vec<usize> {
        (0..p.dots_h()).filter(|&y| p.dot(x, y)).collect()
    }

    /// Dot columns that have any dot.
    fn used_columns(p: &Plot) -> Vec<usize> {
        (0..p.dots_w())
            .filter(|&x| !column(p, x).is_empty())
            .collect()
    }

    fn samples(n: usize, f: impl Fn(f64) -> f64) -> Vec<(f64, f64)> {
        (0..n)
            .map(|i| {
                let x = -5.0 + 10.0 * i as f64 / (n - 1) as f64;
                (x, f(x))
            })
            .collect()
    }

    #[test]
    fn flat_line_is_one_dot_row() {
        for n in [7, 50, 1000, 20_000] {
            let mut p = Plot::new(6, 3);
            p.trace(&samples(n, |_| 1.0), LAYER_CH1, RED);
            let row = p.py(1.0).floor() as usize;
            for x in 0..p.dots_w() - 1 {
                let c = column(&p, x);
                assert!(c.is_empty() || c == vec![row], "n {n} col {x}: {c:?}");
            }
            assert!(used_columns(&p).len() >= p.dots_w() - 3, "n {n}");
        }
    }

    #[test]
    fn vertical_edge_is_one_dot_column() {
        // A square edge in the middle. With at least one sample per dot
        // column, the whole edge lands in one column.
        for n in [200, 1001, 20_000] {
            let mut p = Plot::new(7, 4);
            p.trace(
                &samples(n, |x| if x < 0.3 { -2.0 } else { 2.0 }),
                LAYER_CH1,
                RED,
            );
            let low = p.py(-2.0).floor() as usize;
            let high = p.py(2.0).floor() as usize;
            let tall: Vec<usize> = (0..p.dots_w())
                .filter(|&x| column(&p, x).len() > 1)
                .collect();
            assert_eq!(tall.len(), 1, "n {n}: tall columns {tall:?}");
            let c = column(&p, tall[0]);
            // From the high row down to just above the low row, which the
            // column before already shows.
            assert_eq!(*c.first().unwrap(), high, "n {n}");
            assert_eq!(*c.last().unwrap(), low - 1, "n {n}");
        }
    }

    #[test]
    fn sparse_edge_is_a_thin_slanted_line() {
        // Few samples: the step between two samples is a straight line.
        let mut p = Plot::new(7, 4);
        p.trace(
            &samples(33, |x| if x < 0.3 { -2.0 } else { 2.0 }),
            LAYER_CH1,
            RED,
        );
        let tall = (0..p.dots_w()).filter(|&x| column(&p, x).len() > 1).count();
        // 10 / 32 div between samples, 14 dot columns per division.
        assert!((3..=6).contains(&tall), "{tall}");
    }

    #[test]
    fn steep_sine_has_no_gaps() {
        for n in [40, 300, 5000] {
            let mut p = Plot::new(5, 2);
            p.trace(&samples(n, |x| 3.9 * (x * 4.0).sin()), LAYER_CH1, RED);
            let cols = used_columns(&p);
            // Every dot column between the first and last is used.
            assert_eq!(cols.len(), cols[cols.len() - 1] - cols[0] + 1, "n {n}");
            // Runs of neighbour columns touch (8-connected).
            for x in cols[0]..cols[cols.len() - 1] {
                let (a, b) = (column(&p, x), column(&p, x + 1));
                let (a0, a1) = (a[0] as i64, *a.last().unwrap() as i64);
                let (b0, b1) = (b[0] as i64, *b.last().unwrap() as i64);
                assert!(
                    b0 <= a1 + 1 && a0 <= b1 + 1,
                    "n {n} gap at {x}: {a:?} {b:?}"
                );
            }
        }
    }

    #[test]
    fn slow_slope_stays_thin() {
        let mut p = Plot::new(8, 4);
        p.trace(&samples(2000, |x| x * 0.3), LAYER_CH1, RED);
        for x in used_columns(&p) {
            assert!(column(&p, x).len() <= 2, "col {x}: {:?}", column(&p, x));
        }
    }

    #[test]
    fn nan_breaks_the_line() {
        let mut p = Plot::new(4, 2);
        let mut pts = samples(100, |_| 0.0);
        pts[50].1 = f64::NAN;
        p.trace(&pts, LAYER_CH1, RED);
        assert!(used_columns(&p).len() > 70);
    }

    #[test]
    fn higher_layer_owns_the_cell() {
        let mut p = Plot::new(2, 1);
        p.set(0, 0, LAYER_CH2, BLUE);
        p.set(1, 1, LAYER_CH1, RED);
        let c = p.cell(0, 0);
        assert_eq!(c.color, RED);
        assert_eq!(c.dots, 0x10);
        // A lower layer cannot draw into it.
        p.set(0, 2, LAYER_PERSIST, BLUE);
        assert_eq!(p.cell(0, 0).dots, 0x10);
        // The same layer adds dots.
        p.set(0, 3, LAYER_CH1, RED);
        assert_eq!(p.cell(0, 0).dots, 0x10 | 0x40);
    }

    #[test]
    fn dots_hide_the_grid_char() {
        let pal = Palette::new(ColorMode::TrueColor);
        let mut p = Plot::new(3, 2);
        p.grid(&pal);
        assert_eq!(p.char_at(0, 0), '·');
        p.set(0, 0, LAYER_CH1, RED);
        assert_eq!(p.char_at(0, 0), '\u{2801}');
    }

    #[test]
    fn grid_is_the_same_in_every_division() {
        let pal = Palette::new(ColorMode::TrueColor);
        for (cx, ry) in [(1, 1), (3, 2), (5, 2), (6, 3), (9, 4), (10, 5), (15, 6)] {
            let mut p = Plot::new(cx, ry);
            p.grid(&pal);
            let (mid_c, mid_r) = (5 * cx, 4 * ry);
            for row in 0..p.rows {
                for col in 0..p.cols {
                    let ch = p.char_at(col, row);
                    let want = if row == mid_r || col == mid_c {
                        ch // the center cross is checked below
                    } else if col % cx == 0 && row % ry == 0 {
                        '·'
                    } else {
                        ' '
                    };
                    assert_eq!(ch, want, "{cx}x{ry} at {col},{row}");
                }
            }
            for col in 0..p.cols {
                let ch = p.char_at(col, mid_r);
                if col % cx == 0 {
                    assert_eq!(ch, '┼');
                } else {
                    assert!(ch == '╌' || ch == '┴');
                }
            }
            for row in 0..p.rows {
                let ch = p.char_at(mid_c, row);
                if row % ry == 0 {
                    assert_eq!(ch, '┼');
                } else {
                    assert!(ch == '╎' || ch == '├');
                }
            }
            // Ticks only when they can be even.
            let ticks = (0..p.cols).filter(|&c| p.char_at(c, mid_r) == '┴').count();
            assert_eq!(ticks > 0, cx % 5 == 0 && cx >= 10, "{cx}");
        }
    }

    #[test]
    fn division_lines_map_to_their_cells() {
        let p = Plot::new(6, 3);
        for k in 0..=10 {
            let x = k as f64 - 5.0;
            assert_eq!(p.col_of(x), k * 6);
        }
        for j in 0..=8 {
            let y = 4.0 - j as f64;
            assert_eq!(p.row_of(y), j * 3);
        }
    }

    #[test]
    fn polyline_draws_any_direction() {
        let mut p = Plot::new(3, 2);
        p.polyline(&[(2.0, 2.0), (-2.0, -2.0), (2.0, -2.0)], LAYER_CH1, RED);
        let a = (p.px(2.0).floor() as usize, p.py(2.0).floor() as usize);
        let b = (p.px(-2.0).floor() as usize, p.py(-2.0).floor() as usize);
        assert!(p.dot(a.0, a.1) && p.dot(b.0, b.1));
        // The bottom line is flat.
        let row = p.py(-2.0).floor() as usize;
        for x in b.0..=a.0 {
            assert!(p.dot(x, row));
        }
    }

    #[test]
    fn dashed_cursor_lines() {
        let mut p = Plot::new(3, 2);
        p.dashed_v(0.0, LAYER_CURSOR, BLUE);
        let x = p.px(0.0).floor() as usize;
        let n = column(&p, x).len();
        assert_eq!(n, p.dots_h() / 2);
    }

    #[test]
    fn classic_fills_each_column_from_low_to_high() {
        // Many noisy samples: every column is drawn from its low to its
        // high value, so the line is thicker than the thin one.
        let noisy = samples(20_000, |x| 0.3 * (x * 997.0).sin());
        let mut c = Plot::new(6, 3);
        c.classic(&noisy, false, LAYER_CH1, RED);
        let mut t = Plot::new(6, 3);
        t.trace(&noisy, LAYER_CH1, RED);
        let dots = |p: &Plot| (0..p.dots_w()).map(|x| column(p, x).len()).sum::<usize>();
        assert!(dots(&c) >= dots(&t), "{} {}", dots(&c), dots(&t));
        let (hi, lo) = (c.py(0.3).floor() as usize, c.py(-0.3).floor() as usize);
        for x in 3..c.dots_w() - 3 {
            let col = column(&c, x);
            assert!(
                col.first() <= Some(&(hi + 1)) && col.last() >= Some(&(lo - 1)),
                "{x}: {col:?}"
            );
        }
    }

    #[test]
    fn classic_dots_and_lines() {
        // Few points: straight lines between them, or only the points.
        let pts = [(-4.0, 0.0), (0.0, 2.0), (4.0, 0.0)];
        let mut p = Plot::new(4, 2);
        p.classic(&pts, true, LAYER_CH1, RED);
        assert_eq!(used_columns(&p).len(), 3);
        let mut p = Plot::new(4, 2);
        p.classic(&pts, false, LAYER_CH1, RED);
        let cols = used_columns(&p);
        assert_eq!(cols.len(), cols[cols.len() - 1] - cols[0] + 1);
        // A NaN breaks the line.
        let mut q = Plot::new(4, 2);
        q.classic(
            &[(-4.0, 0.0), (0.0, f64::NAN), (4.0, 0.0)],
            false,
            LAYER_CH1,
            RED,
        );
        assert_eq!(used_columns(&q).len(), 2);
    }

    #[test]
    fn bold_is_two_dots_thick() {
        let mut p = Plot::new(6, 3);
        p.styled(
            &samples(500, |_| 1.0),
            TraceStyle::Bold,
            false,
            LAYER_CH1,
            RED,
        );
        let row = p.py(1.0).floor() as usize;
        for x in 2..p.dots_w() - 3 {
            assert_eq!(column(&p, x), vec![row, row + 1], "col {x}");
        }
        let mut p = Plot::new(6, 3);
        p.styled(&[(0.0, 0.0)], TraceStyle::Bold, true, LAYER_CH1, RED);
        assert_eq!(used_columns(&p).len(), 2);
    }

    /// The characters of the line style, row by row, for messages.
    fn text_rows(p: &Plot) -> String {
        (0..p.rows)
            .map(|r| (0..p.cols).map(|c| p.char_at(c, r)).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn line_plot(pts: &[(f64, f64)]) -> Plot {
        let mut p = Plot::new(6, 3);
        p.styled(pts, TraceStyle::Line, false, LAYER_CH1, RED);
        p
    }

    #[test]
    fn line_style_square_wave_has_bars_and_flat_tops() {
        for n in [200, 2000, 20_000] {
            // High in the top third of a cell, low in the bottom third.
            let p0 = Plot::new(6, 3);
            let (hi, lo) = (2.1, -2.1);
            assert_eq!(Plot::flat_char(p0.py(hi)), '‾');
            assert_eq!(Plot::flat_char(p0.py(lo)), '_');
            let p = line_plot(&samples(
                n,
                |x| if x.rem_euclid(4.0) < 2.0 { hi } else { lo },
            ));
            let text = text_rows(&p);
            assert!(!text.chars().any(|c| ('\u{2800}'..='\u{28FF}').contains(&c)));
            let (rh, rl) = (p.row_of(hi), p.row_of(lo));
            // Each edge is one column of `|` from the high row to the low row.
            let bars: Vec<usize> = (0..p.cols)
                .filter(|&c| (rh + 1..rl).all(|r| p.char_at(c, r) == '|'))
                .collect();
            assert_eq!(bars.len(), 5, "n {n}:\n{text}");
            for c in 0..p.cols {
                if bars.contains(&c) {
                    continue;
                }
                let col: String = (0..p.rows).map(|r| p.char_at(c, r)).collect();
                let ink: Vec<char> = col.chars().filter(|&ch| ch != ' ').collect();
                assert_eq!(ink.len(), 1, "n {n} col {c}:\n{text}");
                assert!(
                    p.char_at(c, rh) == '‾' || p.char_at(c, rl) == '_',
                    "n {n} col {c}:\n{text}"
                );
            }
        }
    }

    #[test]
    fn line_style_slopes() {
        // One cell row per cell column (2 div up per div across, with 6
        // cells across and 3 up per division): one slope character each.
        let ramp = |k: f64| samples(2000, move |x| if x.abs() < 1.9 { k * x } else { f64::NAN });
        let rise = line_plot(&ramp(2.0));
        let fall = line_plot(&ramp(-2.0));
        for (p, want, other) in [(&rise, '/', '\\'), (&fall, '\\', '/')] {
            let text = text_rows(p);
            let n = text.chars().filter(|&c| c == want).count();
            assert!(n >= 20, "{want}: {n}\n{text}");
            assert!(!text.contains(other) && !text.contains('|'), "{text}");
        }
        // A slow slope steps through the three heights of a cell.
        let slow = text_rows(&line_plot(&samples(2000, |x| x * 0.1)));
        for c in ['_', '-', '‾'] {
            assert!(slow.contains(c), "{c}\n{slow}");
        }
    }

    #[test]
    fn line_style_layers_and_dots() {
        let mut p = Plot::new(4, 2);
        p.styled(
            &samples(500, |_| 0.0),
            TraceStyle::Line,
            false,
            LAYER_CH2,
            BLUE,
        );
        p.styled(
            &samples(500, |_| 0.0),
            TraceStyle::Line,
            false,
            LAYER_CH1,
            RED,
        );
        let row = p.row_of(0.0);
        assert_eq!(p.char_at(5, row), '-');
        assert_eq!(p.cell(5, row).color, RED);
        // A lower layer does not take the cell back.
        p.styled(
            &samples(500, |_| 0.0),
            TraceStyle::Line,
            false,
            LAYER_PERSIST,
            BLUE,
        );
        assert_eq!(p.cell(5, row).color, RED);
        // Dots: one character per point.
        let mut d = Plot::new(4, 2);
        d.styled(
            &[(-4.0, 0.0), (0.0, 2.0)],
            TraceStyle::Line,
            true,
            LAYER_CH1,
            RED,
        );
        let text = text_rows(&d);
        assert_eq!(
            text.chars().filter(|c| !c.is_whitespace()).count(),
            2,
            "{text}"
        );
        assert_eq!(d.char_at(d.col_of(-4.0), d.row_of(0.0)), '·');
    }

    fn box_plot(pts: &[(f64, f64)]) -> Plot {
        let mut p = Plot::new(6, 3);
        p.styled(pts, TraceStyle::Box, false, LAYER_CH1, RED);
        p
    }

    /// Every side a box character connects meets the neighbour on that
    /// side, which connects back, or the side of the plot (the line goes
    /// on off the screen). So the line has no gaps and no loose ends
    /// except where it stops in a cell.
    fn assert_connected(p: &Plot) {
        let text = text_rows(p);
        let sides = |c: usize, r: usize| box_sides(p.char_at(c, r));
        for r in 0..p.rows {
            for c in 0..p.cols {
                let ch = p.char_at(c, r);
                if ch == ' ' {
                    continue;
                }
                let s = sides(c, r);
                assert!(s != 0, "({c}, {r}) {ch:?}:\n{text}");
                let ok = |bit: u8, nb: Option<(usize, usize)>, back: u8| {
                    s & bit == 0 || nb.is_some_and(|(nc, nr)| sides(nc, nr) & back != 0)
                };
                let up = r.checked_sub(1).map(|r| (c, r));
                let down = (r + 1 < p.rows).then_some((c, r + 1));
                assert!(ok(BOX_UP, up, BOX_DOWN), "({c}, {r}) up:\n{text}");
                assert!(ok(BOX_DOWN, down, BOX_UP), "({c}, {r}) down:\n{text}");
                // Left of the first column and right of the last are the
                // plot sides.
                if c > 0 {
                    assert!(
                        ok(BOX_LEFT, Some((c - 1, r)), BOX_RIGHT),
                        "({c}, {r}) left:\n{text}"
                    );
                }
                if c + 1 < p.cols {
                    assert!(
                        ok(BOX_RIGHT, Some((c + 1, r)), BOX_LEFT),
                        "({c}, {r}) right:\n{text}"
                    );
                }
            }
        }
    }

    /// Cell columns with any ink.
    fn inked_columns(p: &Plot) -> usize {
        (0..p.cols)
            .filter(|&c| (0..p.rows).any(|r| p.char_at(c, r) != ' '))
            .count()
    }

    #[test]
    fn box_chars_and_sides_match() {
        for s in 1..16 {
            assert_eq!(box_sides(box_char(s)), s);
        }
        assert_eq!(box_char(BOX_LEFT | BOX_RIGHT), '─');
        assert_eq!(box_char(BOX_UP | BOX_DOWN), '│');
        assert_eq!(box_char(BOX_RIGHT | BOX_DOWN), '╭');
        assert_eq!(box_char(BOX_LEFT | BOX_DOWN), '╮');
        assert_eq!(box_char(BOX_UP | BOX_RIGHT), '╰');
        assert_eq!(box_char(BOX_UP | BOX_LEFT), '╯');
        assert_eq!(box_sides('x'), 0);
    }

    #[test]
    fn box_flat_line_is_all_dashes() {
        for y in [0.0, 1.0, -2.3, 3.9] {
            let p = box_plot(&samples(2000, |_| y));
            let row = p.row_of(y);
            for r in 0..p.rows {
                for c in 0..p.cols {
                    let want = if r == row { '─' } else { ' ' };
                    assert_eq!(p.char_at(c, r), want, "{y}:\n{}", text_rows(&p));
                }
            }
        }
    }

    #[test]
    fn box_square_wave_has_corners_and_bars() {
        for n in [200, 2000, 20_000] {
            let (hi, lo) = (2.1, -2.1);
            let p = box_plot(&samples(
                n,
                |x| if x.rem_euclid(4.0) < 2.0 { hi } else { lo },
            ));
            let text = text_rows(&p);
            assert!(!text.chars().any(|c| ('\u{2800}'..='\u{28FF}').contains(&c)));
            assert_connected(&p);
            assert_eq!(inked_columns(&p), p.cols, "n {n}:\n{text}");
            let (rh, rl) = (p.row_of(hi), p.row_of(lo));
            let top: String = (0..p.cols).map(|c| p.char_at(c, rh)).collect();
            let bottom: String = (0..p.cols).map(|c| p.char_at(c, rl)).collect();
            // Falling edge: ─╮ on top, ╰─ below; rising: ─╯ below, ╭─ on top.
            assert!(top.contains("─╮") && top.contains("╭─"), "n {n}:\n{text}");
            assert!(
                bottom.contains("╰─") && bottom.contains("─╯"),
                "n {n}:\n{text}"
            );
            // Five edges, each one column of │ between the levels.
            let bars = (0..p.cols)
                .filter(|&c| (rh + 1..rl).all(|r| p.char_at(c, r) == '│'))
                .count();
            assert_eq!(bars, 5, "n {n}:\n{text}");
            // Only the two levels and the edges have ink.
            for r in 0..p.rows {
                let row: String = (0..p.cols).map(|c| p.char_at(c, r)).collect();
                if r < rh || r > rl {
                    assert!(row.trim().is_empty(), "n {n} row {r}:\n{text}");
                }
            }
        }
    }

    #[test]
    fn box_sine_has_round_peaks_and_dips() {
        for n in [500, 5000] {
            // Period of 4 divisions, 24 cells: flat for a few cells at the
            // peaks.
            let f = |x: f64| 3.0 * (x * std::f64::consts::FRAC_PI_2).cos();
            let p = box_plot(&samples(n, f));
            let text = text_rows(&p);
            assert_connected(&p);
            assert_eq!(inked_columns(&p), p.cols, "n {n}:\n{text}");
            let row = |r: usize| (0..p.cols).map(|c| p.char_at(c, r)).collect::<String>();
            let (top, bottom) = (row(p.row_of(3.0)), row(p.row_of(-3.0)));
            // ╭─...─╮ at each peak, ╰─...─╯ at each dip, nothing else.
            let caps = |s: &str, a: char, b: char| {
                let t = s.trim();
                t.split(' ')
                    .filter(|w| !w.is_empty())
                    .all(|w| w.starts_with(a) && w.ends_with(b) && w.chars().count() >= 3)
            };
            assert!(
                caps(&top, '╭', '╮') && top.matches('╭').count() == 3,
                "n {n}:\n{text}"
            );
            assert!(
                caps(&bottom, '╰', '╯') && bottom.matches('╰').count() == 2,
                "n {n}:\n{text}"
            );
            assert!(row(p.row_of(3.0) - 1).trim().is_empty(), "n {n}:\n{text}");
        }
    }

    #[test]
    fn box_lines_have_no_gaps() {
        type Signal = (&'static str, fn(f64) -> f64);
        let signals: [Signal; 5] = [
            ("steep sine", |x| 3.9 * (x * 4.0).sin()),
            ("slow slope", |x| x * 0.1),
            ("noise", |x| {
                (x * 1.7).sin() + 0.6 * (x * 97.0).sin() * (x * 13.0).cos()
            }),
            ("off screen", |x| 6.0 * (x * 1.3).sin()),
            ("ramp", |x| 0.8 * x),
        ];
        for (name, f) in signals {
            for n in [20, 61, 300, 3000, 30_000] {
                let p = box_plot(&samples(n, f));
                assert_connected(&p);
                assert_eq!(
                    inked_columns(&p),
                    p.cols,
                    "{name} n {n}:\n{}",
                    text_rows(&p)
                );
            }
        }
    }

    #[test]
    fn box_noise_fills_the_column() {
        // Up and down by 2 divisions many times inside each cell column.
        let p = box_plot(&samples(20_000, |x| {
            if (x * 300.0).sin() > 0.0 {
                1.0
            } else {
                -1.0
            }
        }));
        let (r0, r1) = (p.row_of(1.0), p.row_of(-1.0));
        for c in 1..p.cols - 1 {
            for r in r0 + 1..r1 - 1 {
                let s = box_sides(p.char_at(c, r));
                assert!(
                    s & BOX_UP != 0 && s & BOX_DOWN != 0,
                    "({c}, {r}):\n{}",
                    text_rows(&p)
                );
            }
        }
        assert_connected(&p);
    }

    #[test]
    fn box_nan_breaks_the_line() {
        let pts = samples(2000, |x| if x.abs() < 1.0 { f64::NAN } else { 0.5 });
        let p = box_plot(&pts);
        assert_connected(&p);
        let row = p.row_of(0.5);
        let line: String = (0..p.cols).map(|c| p.char_at(c, row)).collect();
        let parts: Vec<&str> = line.split(' ').filter(|s| !s.is_empty()).collect();
        assert_eq!(parts.len(), 2, "{line}");
        // The ends in the gap stop in their cell (`╴`, `╶`); at the
        // screen sides the line goes on (`─`).
        assert!(
            parts[0].starts_with('─') && parts[0].ends_with(['─', '╴']),
            "{line}"
        );
        assert!(
            parts[1].starts_with(['─', '╶']) && parts[1].ends_with('─'),
            "{line}"
        );
    }

    #[test]
    fn newer_old_frames_win_cells() {
        assert_eq!(persist_layer(0, 8), LAYER_PERSIST);
        assert_eq!(persist_layer(7, 8), LAYER_PERSIST + 7);
        assert_eq!(persist_layer(15, 16), LAYER_PERSIST + 15);
        assert!(persist_layer(15, 16) < LAYER_MATH);
        let flat = samples(500, |_| 0.0);
        for style in TraceStyle::ALL {
            let mut p = Plot::new(4, 2);
            p.styled(&flat, style, false, persist_layer(0, 2), BLUE);
            p.styled(&flat, style, false, persist_layer(1, 2), RED);
            let row = p.row_of(0.0);
            assert_eq!(p.cell(5, row).color, RED, "{style:?}");
            p.styled(&flat, style, false, LAYER_MATH, GREEN);
            assert_eq!(p.cell(5, row).color, GREEN, "{style:?}");
        }
    }

    #[test]
    fn dim_layers_render_dim() {
        use ratatui::style::Modifier;
        let mut p = Plot::new(4, 2);
        p.set(0, 0, LAYER_PERSIST, BLUE);
        p.set(2, 0, LAYER_PERSIST + 1, RED);
        p.dim_upto(LAYER_PERSIST);
        let area = Rect::new(0, 0, p.cols as u16, p.rows as u16);
        let mut buf = Buffer::empty(area);
        p.render(&mut buf, area);
        assert!(buf[(0, 0)].modifier.contains(Modifier::DIM));
        assert!(!buf[(1, 0)].modifier.contains(Modifier::DIM));
    }

    #[test]
    fn box_layers_and_dots() {
        let flat = samples(500, |_| 0.0);
        let mut p = Plot::new(4, 2);
        p.styled(&flat, TraceStyle::Box, false, LAYER_CH2, BLUE);
        let row = p.row_of(0.0);
        assert_eq!(p.cell(5, row).color, BLUE);
        p.styled(&flat, TraceStyle::Box, false, LAYER_CH1, RED);
        assert_eq!(p.char_at(5, row), '─');
        assert_eq!(p.cell(5, row).color, RED);
        // A lower layer does not take the cell back.
        p.styled(&flat, TraceStyle::Box, false, LAYER_PERSIST, BLUE);
        assert_eq!(p.cell(5, row).color, RED);
        // On one layer the later trace wins; with the same colour the
        // sides are joined.
        let mut q = Plot::new(4, 2);
        let step = samples(500, |x| if x < 0.0 { 0.0 } else { 1.0 });
        q.styled(&flat, TraceStyle::Box, false, LAYER_MATH, BLUE);
        q.styled(&step, TraceStyle::Box, false, LAYER_MATH, RED);
        let c = q.col_of(0.0);
        assert_eq!(q.char_at(c, row), '╯', "{}", text_rows(&q));
        q.styled(&flat, TraceStyle::Box, false, LAYER_MATH, RED);
        assert_eq!(q.char_at(c, row), '┴', "{}", text_rows(&q));
        // Old traces of different ages (layers, colours) join too, and the
        // joined cell takes the newer one.
        let mut a = Plot::new(4, 2);
        a.styled(&flat, TraceStyle::Box, false, LAYER_PERSIST, BLUE);
        a.styled(&step, TraceStyle::Box, false, LAYER_PERSIST + 1, RED);
        assert_eq!(a.char_at(c, row), '┴', "{}", text_rows(&a));
        assert_eq!(a.cell(c, row).color, RED);
        // An older one drawn later does not take the cell.
        a.styled(&flat, TraceStyle::Box, false, LAYER_PERSIST, BLUE);
        assert_eq!(a.cell(c, row).color, RED);
        // Dots: one character per point.
        let mut d = Plot::new(4, 2);
        d.styled(
            &[(-4.0, 0.0), (0.0, 2.0), (f64::NAN, 1.0)],
            TraceStyle::Box,
            true,
            LAYER_CH1,
            RED,
        );
        let text = text_rows(&d);
        assert_eq!(text.chars().filter(|&c| c == '•').count(), 2, "{text}");
        assert_eq!(d.char_at(d.col_of(-4.0), d.row_of(0.0)), '•');
    }

    #[test]
    fn classic_cursor_dashes_have_a_fixed_count() {
        for (cx, ry) in [(3, 2), (9, 4)] {
            let mut p = Plot::new(cx, ry);
            p.dashed_classic((1.0, -4.0), (1.0, 4.0), LAYER_CURSOR, BLUE);
            let x = p.px(1.0).floor() as usize;
            let n = column(&p, x).len();
            // 41 points; on small plots some land on the same dot.
            assert!(n <= 41 && n > 20, "{cx}x{ry}: {n}");
            assert_eq!(used_columns(&p), vec![x]);
        }
    }

    #[test]
    fn fit_uses_whole_cells_and_centers() {
        let g = Geometry::fit(Rect::new(0, 0, 107, 35)).unwrap();
        assert_eq!(g.margins, Margins::FULL);
        assert_eq!(g.cx, 9);
        assert_eq!(g.ry, 4);
        assert_eq!(g.plot.width, 91);
        assert_eq!(g.plot.height, 33);
        // Centered: the same space on both sides (within one cell).
        let left = g.plot.x - g.margins.left;
        let right = 107 - (g.plot.x + g.plot.width + g.margins.right);
        assert!(left.abs_diff(right) <= 1, "{left} {right}");
        // Small areas drop the margins, then give up.
        let g = Geometry::fit(Rect::new(0, 0, 30, 9)).unwrap();
        assert_eq!(g.ry, 1);
        assert!(g.margins.top == 0);
        assert!(Geometry::fit(Rect::new(0, 0, 8, 5)).is_none());
    }

    #[test]
    fn render_writes_only_used_cells() {
        let pal = Palette::new(ColorMode::TrueColor);
        let mut p = Plot::new(1, 1);
        p.grid(&pal);
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 12));
        buf[(5, 5)].set_char('X');
        p.render(&mut buf, Rect::new(2, 1, 11, 9));
        assert_eq!(buf[(2, 1)].symbol(), "·");
        assert_eq!(buf[(7, 5)].symbol(), "┼");
        assert_eq!(buf[(15, 5)].symbol(), " ");
    }
}
