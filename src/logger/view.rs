//! State of the log view: which part of the log is on screen, the zoom,
//! the time cursors. No drawing here (that is in `ui.rs`).

use std::path::Path;

use anyhow::Result;

use super::pyramid::{Aggs, Pyramid, BASE};
use super::reader::LogFile;

/// Steps per dot column the zoom keys go through (1-2-5).
pub fn zoom_steps() -> impl Iterator<Item = u64> {
    (0..13).flat_map(|e| [1u64, 2, 5].map(|m| m * 10u64.pow(e)))
}

/// Steps summed up per tick while the summary is built.
pub const SUMMARY_BUDGET: u64 = 4 << 20;
/// Save the summary next to the log when it took at least this many
/// steps to build.
pub const SAVE_SUMMARY_FROM: u64 = 256 * BASE;

/// What a key does in the log view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    /// Scroll by this many divisions (negative: back in time).
    Divs(i64),
    /// Scroll by whole screens.
    Pages(i64),
    /// The start of the log.
    Home,
    /// The newest data, and follow it.
    End,
    ZoomIn,
    ZoomOut,
    /// The whole log on the screen.
    Fit,
    /// One step per dot column.
    Native,
}

/// Left edge, zoom, width, end and summary progress of drawn columns.
type CacheKey = (u64, u64, usize, u64, u64);

pub struct LogView {
    pub file: LogFile,
    pub pyr: Pyramid,
    /// Steps per dot column; `None`: the whole log fits.
    pub zoom: Option<u64>,
    /// Step at the left edge, when not following.
    pub start: u64,
    /// Keep the newest data at the right edge.
    pub follow: bool,
    pub cursors: bool,
    /// Cursors A and B, in steps.
    pub cur: [u64; 2],
    /// Dot columns of the last drawing (for the keys).
    pub width: std::cell::Cell<usize>,
    /// The log is being recorded now.
    pub live: bool,
    summed_here: u64,
    saved: bool,
    cache: std::cell::RefCell<Option<(CacheKey, Vec<Aggs>)>>,
}

impl LogView {
    pub fn open(path: &Path, live: bool) -> Result<LogView> {
        let file = LogFile::open(path)?;
        let pyr = Pyramid::load(&file).unwrap_or_default();
        Ok(LogView {
            file,
            pyr,
            zoom: None,
            start: 0,
            follow: true,
            cursors: false,
            cur: [0, 0],
            width: std::cell::Cell::new(200),
            live,
            summed_here: 0,
            saved: false,
            cache: Default::default(),
        })
    }

    /// Read new data and sum up more of the log. True when the screen
    /// changes.
    pub fn tick(&mut self) -> Result<bool> {
        let mut changed = self.file.refresh()?;
        let before = self.pyr.covered;
        changed |= self.pyr.update(&self.file, SUMMARY_BUDGET)?;
        self.summed_here += self.pyr.covered - before;
        if !self.live
            && !self.saved
            && self.pyr.complete(&self.file)
            && self.summed_here >= SAVE_SUMMARY_FROM
        {
            // A cache only; a read-only folder is fine.
            let _ = self.pyr.save(&self.file);
            self.saved = true;
        }
        Ok(changed)
    }

    /// Part of the log summed up, 0 to 1.
    pub fn summary_progress(&self) -> f64 {
        let end = self.file.end() / BASE * BASE;
        if end == 0 {
            1.0
        } else {
            (self.pyr.covered as f64 / end as f64).min(1.0)
        }
    }

    pub fn end(&self) -> u64 {
        self.file.end()
    }

    /// Steps per dot column for a plot `w` dot columns wide.
    pub fn z(&self, w: usize) -> u64 {
        match self.zoom {
            Some(z) => z,
            None => self.end().div_ceil(w.max(1) as u64).max(1),
        }
    }

    /// Step at the left edge, for a plot `w` dot columns wide.
    pub fn left(&self, w: usize) -> u64 {
        let z = self.z(w);
        let span = z * w as u64;
        let s = if self.zoom.is_none() {
            0
        } else if self.follow {
            (self.end().div_ceil(z) * z).saturating_sub(span)
        } else {
            self.start.min(self.end().saturating_sub(z))
        };
        s / z * z
    }

    /// The columns to draw: `w` columns from [`LogView::left`].
    pub fn columns(&self, w: usize) -> Result<Vec<Aggs>> {
        let z = self.z(w);
        let start = self.left(w);
        let key = (start, z, w, self.end(), self.pyr.covered);
        if let Some((k, cols)) = &*self.cache.borrow() {
            if *k == key {
                return Ok(cols.clone());
            }
        }
        let cols = self.pyr.columns(&self.file, start, z, w)?;
        *self.cache.borrow_mut() = Some((key, cols.clone()));
        Ok(cols)
    }

    /// Scroll or zoom. `w` is the plot width in dot columns, `div` the
    /// dot columns per division.
    pub fn go(&mut self, m: Move, w: usize, div: usize) {
        let z = self.z(w);
        let left = self.left(w);
        let span = z * w as u64;
        let end = self.end();
        let set_start = |v: &mut LogView, s: i64| {
            v.start = s.clamp(0, end as i64) as u64;
            v.follow = v.start + span >= end;
        };
        match m {
            Move::Divs(n) => {
                self.zoom.get_or_insert(z);
                set_start(self, left as i64 + n * (div as u64 * z) as i64);
            }
            Move::Pages(n) => {
                self.zoom.get_or_insert(z);
                set_start(self, left as i64 + n * span as i64);
            }
            Move::Home => {
                self.zoom.get_or_insert(z);
                self.start = 0;
                self.follow = false;
            }
            Move::End => self.follow = true,
            Move::Fit => {
                self.zoom = None;
                self.follow = true;
            }
            Move::ZoomIn | Move::ZoomOut | Move::Native => {
                let new = match m {
                    Move::ZoomIn => zoom_steps().filter(|s| *s < z).last().unwrap_or(1),
                    Move::ZoomOut => zoom_steps().find(|s| *s > z).unwrap_or(z),
                    _ => 1,
                };
                // Keep the middle of the screen; the right edge when
                // following, the left edge at the start of the log.
                let mid = if left == 0 && !self.follow {
                    0
                } else {
                    left + span / 2
                };
                if new as u128 * w as u128 >= end as u128 && m == Move::ZoomOut {
                    self.zoom = None;
                    self.follow = true;
                    return;
                }
                self.zoom = Some(new);
                if !self.follow {
                    let half = if mid == 0 { 0 } else { new * w as u64 / 2 };
                    self.start = mid.saturating_sub(half);
                    self.follow = self.start + new * w as u64 >= end;
                }
            }
        }
    }

    /// Cursors on (at a quarter and three quarters of the screen) or off.
    pub fn toggle_cursors(&mut self, w: usize) {
        self.cursors = !self.cursors;
        if self.cursors {
            let z = self.z(w);
            let left = self.left(w);
            let span = (z * w as u64).min(self.end().saturating_sub(left));
            self.cur = [left + span / 4, left + span * 3 / 4];
        }
    }

    /// Move cursor `i` by `cols` dot columns.
    /// Move cursor `i` by `tenths` tenths of a dot column: 10 is one dot
    /// column, 1 is a tenth of one (at least one log step).
    pub fn move_cursor(&mut self, i: usize, tenths: i64, w: usize) {
        if !self.cursors {
            self.toggle_cursors(w);
        }
        let z = self.z(w) as i64;
        let steps = if tenths.abs() >= 10 {
            tenths / 10 * z
        } else {
            ((tenths.abs() * z) / 10).max(1) * tenths.signum()
        };
        let v = self.cur[i] as i64 + steps;
        self.cur[i] = v.clamp(0, self.end() as i64) as u64;
    }

    /// The cursor range, in steps, low to high.
    pub fn cursor_range(&self) -> std::ops::Range<u64> {
        let [a, b] = self.cur;
        a.min(b)..a.max(b)
    }

    /// Min, max and mean between the cursors.
    pub fn cursor_aggs(&self) -> Result<Aggs> {
        self.pyr.agg(&self.file, self.cursor_range())
    }
}

#[cfg(test)]
mod tests {
    use super::super::format::test_header;
    use super::super::reader::tests::write_log;
    use super::*;

    fn view(n: u64) -> (LogView, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("oht-view-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("v.ohlog");
        let h = test_header([true, false], 1e-3);
        let v: Vec<f32> = (0..n)
            .flat_map(|i| [i as f32, i as f32 + 1.0, i as f32])
            .collect();
        write_log(&path, &h, &[(0, v, false)]);
        let mut lv = LogView::open(&path, false).unwrap();
        while lv.tick().unwrap() {}
        (lv, dir)
    }

    #[test]
    fn zoom_scroll_and_follow() {
        let (mut v, dir) = view(10_000);
        let w = 100;
        // The whole log fits.
        assert_eq!(v.z(w), 100);
        assert_eq!(v.left(w), 0);
        v.go(Move::ZoomIn, w, 10);
        assert_eq!(v.zoom, Some(50));
        assert!(v.follow);
        assert_eq!(v.left(w), 5_000);
        v.go(Move::Divs(-2), w, 10);
        assert!(!v.follow);
        assert_eq!(v.left(w), 4_000);
        v.go(Move::Divs(1), w, 10);
        v.go(Move::Native, w, 10);
        assert_eq!(v.z(w), 1);
        // Zooming keeps the middle of the screen.
        assert_eq!(v.left(w), 6_950);
        v.go(Move::Pages(1), w, 10);
        assert_eq!(v.left(w), 7_050);
        // At the start it keeps the start.
        v.go(Move::Home, w, 10);
        v.go(Move::ZoomOut, w, 10);
        assert_eq!((v.left(w), v.z(w)), (0, 2));
        v.go(Move::End, w, 10);
        assert_eq!(v.left(w), 9_800);
        v.go(Move::ZoomOut, w, 10);
        assert_eq!(v.z(w), 5);
        for _ in 0..10 {
            v.go(Move::ZoomOut, w, 10);
        }
        assert_eq!(v.zoom, None);
        let cols = v.columns(w).unwrap();
        assert_eq!(cols.len(), w);
        assert_eq!(cols[3][0].min, 300.0);
        assert_eq!(cols[3][0].max, 400.0);
        v.toggle_cursors(w);
        assert_eq!(v.cur, [2_500, 7_500]);
        v.move_cursor(1, -100, w);
        assert_eq!(v.cur[1], 6_500);
        // Fine: a tenth of a dot column.
        v.move_cursor(1, -1, w);
        assert_eq!(v.cur[1], 6_500 - v.z(w) / 10);
        v.move_cursor(1, 1, w);
        assert_eq!(v.cur[1], 6_500);
        let a = v.cursor_aggs().unwrap();
        assert_eq!((a[0].min, a[0].max, a[0].n), (2_500.0, 6_500.0, 4_000));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
