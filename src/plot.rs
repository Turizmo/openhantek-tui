//! Braille plot renderer for the scope screen.
//!
//! A cell holds 2 x 4 braille dots and one colour. Each division is a whole
//! number of cells, so the grid looks the same in every division. Traces
//! are drawn one dot thick: each dot column gets one vertical run of dots.

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

use crate::theme::Palette;

/// Draw layers, lowest first. A higher layer takes over a cell from a
/// lower one, so trace colours never mix in one cell.
pub const LAYER_CURSOR: u8 = 1;
pub const LAYER_PERSIST: u8 = 2;
pub const LAYER_MATH: u8 = 3;
pub const LAYER_CH2: u8 = 4;
pub const LAYER_CH1: u8 = 5;
/// The selected trace, drawn above all others.
pub const LAYER_TOP: u8 = 6;

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
    /// Grid character, shown when the cell has no dots.
    pub text: Option<(char, Color)>,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            dots: 0,
            layer: 0,
            color: Color::Reset,
            text: None,
        }
    }
}

impl Cell {
    /// The character this cell shows, if any.
    pub fn symbol(&self) -> Option<(char, Color)> {
        if self.dots != 0 {
            char::from_u32(0x2800 + self.dots as u32).map(|c| (c, self.color))
        } else {
            self.text
        }
    }
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
        }
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

    /// Draw a trace through `points` (in divisions, in time order).
    /// Each dot column gets one vertical run: from the last value of the
    /// column before, through all samples in this column. A non-finite
    /// point breaks the line.
    pub fn trace(&mut self, points: &[(f64, f64)], layer: u8, color: Color) {
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

    /// Write the plot into a buffer at `area` (top-left corner). Cells with
    /// nothing in them are left as they are.
    pub fn render(&self, buf: &mut Buffer, area: Rect) {
        for row in 0..self.rows.min(area.height as usize) {
            for col in 0..self.cols.min(area.width as usize) {
                if let Some((c, color)) = self.cell(col, row).symbol() {
                    let pos = (area.x + col as u16, area.y + row as u16);
                    if let Some(cell) = buf.cell_mut(pos) {
                        cell.set_char(c).set_fg(color);
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
