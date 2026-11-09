//! Software trigger. The DSO-6022BE has no trigger in hardware, so we look
//! for the trigger edge in the captured samples.

use serde::{Deserialize, Serialize};

/// Which edge starts a trigger.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Slope {
    #[default]
    Rising,
    Falling,
    Both,
}

impl Slope {
    pub fn next(self) -> Self {
        match self {
            Slope::Rising => Slope::Falling,
            Slope::Falling => Slope::Both,
            Slope::Both => Slope::Rising,
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Slope::Rising => "↑",
            Slope::Falling => "↓",
            Slope::Both => "↕",
        }
    }
}

/// Find the first trigger edge in `samples[from..to]`.
///
/// `hysteresis` is in the same unit as the samples. The signal must first
/// go `hysteresis` below the level (for a rising edge) before a crossing
/// counts. This stops noise from making false triggers.
///
/// Returns the crossing point as a fractional sample index.
pub fn find_trigger(
    samples: &[f64],
    level: f64,
    slope: Slope,
    hysteresis: f64,
    from: usize,
    to: usize,
) -> Option<f64> {
    let to = to.min(samples.len());
    if to < 2 || from >= to {
        return None;
    }
    let hyst = hysteresis.abs();
    let mut armed_rise = false;
    let mut armed_fall = false;
    // Arm on the samples before `from` too, so an edge right at `from` counts.
    let warmup = from.saturating_sub(64);
    for i in warmup..to {
        let v = samples[i];
        if i > warmup && i >= from {
            let prev = samples[i - 1];
            if armed_rise && slope != Slope::Falling && prev < level && v >= level {
                return Some(crossing(i, prev, v, level));
            }
            if armed_fall && slope != Slope::Rising && prev > level && v <= level {
                return Some(crossing(i, prev, v, level));
            }
        }
        // Arm when the signal is clearly on the far side of the level.
        // Disarm after a crossing, so the next edge needs a new full swing.
        if v < level - hyst {
            armed_rise = true;
        } else if v >= level {
            armed_rise = false;
        }
        if v > level + hyst {
            armed_fall = true;
        } else if v <= level {
            armed_fall = false;
        }
    }
    None
}

/// The trigger search on a stream: [`find_trigger`] for samples that
/// come in pieces of any size. It keeps the arm state and the last sample
/// between pieces, so an edge is found the same wherever the stream is
/// cut, also between the sample before the level and the one after it.
///
/// Edges before the hold-off index ([`EdgeScanner::hold_until`]) are not
/// reported, but they still arm and disarm the search, so the first edge
/// after the hold-off is never missed.
#[derive(Clone, Debug)]
pub struct EdgeScanner {
    pub level: f64,
    pub slope: Slope,
    /// Always positive.
    pub hysteresis: f64,
    armed_rise: bool,
    armed_fall: bool,
    prev: Option<f64>,
    /// Index of the next sample.
    next: u64,
    /// No edge before this index is reported.
    hold: f64,
}

impl EdgeScanner {
    /// A search that starts at sample index `first`.
    pub fn new(level: f64, slope: Slope, hysteresis: f64, first: u64) -> Self {
        Self {
            level,
            slope,
            hysteresis: hysteresis.abs(),
            armed_rise: false,
            armed_fall: false,
            prev: None,
            next: first,
            hold: f64::NEG_INFINITY,
        }
    }

    /// Index of the next sample the search wants.
    pub fn next(&self) -> u64 {
        self.next
    }

    /// Report no edge before index `t` (fractional).
    pub fn hold_until(&mut self, t: f64) {
        self.hold = t;
    }

    /// New level and hysteresis (the signal changed); the arm state stays.
    pub fn set_level(&mut self, level: f64, hysteresis: f64) {
        self.level = level;
        self.hysteresis = hysteresis.abs();
    }

    /// Take one sample (index [`EdgeScanner::next`]). Returns the edge it
    /// ends, as a fractional sample index.
    #[inline]
    pub fn step(&mut self, v: f64) -> Option<f64> {
        let i = self.next;
        self.next += 1;
        let level = self.level;
        let mut edge = None;
        if let Some(prev) = self.prev {
            let rise =
                self.armed_rise && self.slope != Slope::Falling && prev < level && v >= level;
            let fall = self.armed_fall && self.slope != Slope::Rising && prev > level && v <= level;
            if rise || fall {
                let t = (i - 1) as f64 + frac(prev, v, level);
                if t >= self.hold {
                    edge = Some(t);
                }
            }
        }
        // As in `find_trigger`: arm on the far side, disarm at the level.
        let hyst = self.hysteresis;
        if v < level - hyst {
            self.armed_rise = true;
        } else if v >= level {
            self.armed_rise = false;
        }
        if v > level + hyst {
            self.armed_fall = true;
        } else if v <= level {
            self.armed_fall = false;
        }
        self.prev = Some(v);
        edge
    }

    /// Which side of the level and hysteresis bands `v` is on. Two
    /// samples in one zone give the same arm state, and no edge is
    /// between them.
    #[inline]
    fn zone(&self, v: f64) -> u8 {
        let (l, h) = (self.level, self.hysteresis);
        if v < l - h {
            0
        } else if v < l {
            1
        } else if v == l {
            2
        } else if v <= l + h {
            3
        } else {
            4
        }
    }

    /// Take the `n` samples of a block with the lowest `min`, the highest
    /// `max` and the last `last`, when they can hold no edge and change no
    /// arm state: all in the zone of the sample before. Returns false (and
    /// takes nothing) otherwise; then [`EdgeScanner::step`] each sample.
    #[inline]
    pub fn skip_block(&mut self, n: usize, min: f64, max: f64, last: f64) -> bool {
        let Some(prev) = self.prev else {
            return false;
        };
        let z = self.zone(prev);
        if self.zone(min) != z || self.zone(max) != z {
            return false;
        }
        self.prev = Some(last);
        self.next += n as u64;
        true
    }

    /// Take the samples `points`, the first at index `first` (a jump
    /// from [`EdgeScanner::next`] starts the search over there). Stops at
    /// the first edge and returns it; push the rest of the piece (from
    /// [`EdgeScanner::next`]) to look for more.
    pub fn push(&mut self, points: &[f64], first: u64) -> Option<f64> {
        if first != self.next {
            self.armed_rise = false;
            self.armed_fall = false;
            self.prev = None;
            self.next = first;
        }
        for &v in points {
            if let Some(t) = self.step(v) {
                return Some(t);
            }
        }
        None
    }
}

/// Where between `prev` and `v` (0 .. 1) the line crosses `level`.
fn frac(prev: f64, v: f64, level: f64) -> f64 {
    let span = v - prev;
    if span.abs() < f64::EPSILON {
        1.0
    } else {
        ((level - prev) / span).clamp(0.0, 1.0)
    }
}

/// Fractional index where the line from `prev` (at `i - 1`) to `v` (at `i`)
/// crosses `level`.
fn crossing(i: usize, prev: f64, v: f64, level: f64) -> f64 {
    (i - 1) as f64 + frac(prev, v, level)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(n: usize, period: f64, phase: f64) -> Vec<f64> {
        (0..n)
            .map(|i| (std::f64::consts::TAU * (i as f64 / period) + phase).sin())
            .collect()
    }

    #[test]
    fn rising_edge_of_sine() {
        // Starts at the top, so the first rising zero crossing is at 3/4 period.
        let s = sine(1000, 100.0, std::f64::consts::FRAC_PI_2);
        let t = find_trigger(&s, 0.0, Slope::Rising, 0.1, 0, s.len()).unwrap();
        assert!((t - 75.0).abs() < 0.05, "t = {t}");
    }

    #[test]
    fn falling_edge_of_sine() {
        let s = sine(1000, 100.0, 0.0);
        let t = find_trigger(&s, 0.0, Slope::Falling, 0.1, 0, s.len()).unwrap();
        assert!((t - 50.0).abs() < 0.05, "t = {t}");
    }

    #[test]
    fn both_edges_takes_the_first() {
        let s = sine(1000, 100.0, std::f64::consts::FRAC_PI_2);
        let t = find_trigger(&s, 0.0, Slope::Both, 0.1, 0, s.len()).unwrap();
        assert!((t - 25.0).abs() < 0.05, "t = {t}");
    }

    #[test]
    fn level_out_of_range_gives_none() {
        let s = sine(1000, 100.0, 0.0);
        assert_eq!(find_trigger(&s, 2.0, Slope::Rising, 0.1, 0, s.len()), None);
        assert_eq!(
            find_trigger(&s, -2.0, Slope::Falling, 0.1, 0, s.len()),
            None
        );
    }

    #[test]
    fn respects_search_window() {
        let s = sine(1000, 100.0, std::f64::consts::FRAC_PI_2);
        let t = find_trigger(&s, 0.0, Slope::Rising, 0.1, 100, s.len()).unwrap();
        assert!((t - 175.0).abs() < 0.05, "t = {t}");
        assert_eq!(find_trigger(&s, 0.0, Slope::Rising, 0.1, 100, 170), None);
    }

    #[test]
    fn hysteresis_ignores_noise_at_the_level() {
        // Noise around 0 that never goes below -0.2 must not trigger.
        let mut s: Vec<f64> = (0..200)
            .map(|i| if i % 2 == 0 { 0.05 } else { -0.05 })
            .collect();
        assert_eq!(find_trigger(&s, 0.0, Slope::Rising, 0.2, 0, s.len()), None);
        // Then a real edge: down to -1, up to +1.
        s.extend([-1.0, -1.0, 1.0, 1.0]);
        let t = find_trigger(&s, 0.0, Slope::Rising, 0.2, 0, s.len()).unwrap();
        assert!((t - 201.5).abs() < 1e-9, "t = {t}");
    }

    #[test]
    fn edge_right_after_from_counts() {
        let mut s = vec![-1.0; 100];
        s.extend(vec![1.0; 100]);
        let t = find_trigger(&s, 0.0, Slope::Rising, 0.1, 100, s.len()).unwrap();
        assert!((t - 99.5).abs() < 1e-9, "t = {t}");
    }

    #[test]
    fn empty_and_tiny_inputs() {
        assert_eq!(find_trigger(&[], 0.0, Slope::Rising, 0.1, 0, 0), None);
        assert_eq!(find_trigger(&[1.0], 0.0, Slope::Rising, 0.1, 0, 1), None);
        assert_eq!(
            find_trigger(&[-1.0, 1.0], 0.0, Slope::Rising, 0.1, 5, 2),
            None
        );
    }

    /// Every edge of `s`, by the scanner on the whole of it. The first one
    /// must be the one `find_trigger` finds.
    fn all_edges(s: &[f64], level: f64, slope: Slope, hyst: f64) -> Vec<f64> {
        let mut out = Vec::new();
        let mut sc = EdgeScanner::new(level, slope, hyst, 0);
        while let Some(t) = sc.push(&s[sc.next() as usize..], sc.next()) {
            out.push(t);
        }
        // Check the first one against `find_trigger`.
        if let Some(&t) = out.first() {
            assert_eq!(find_trigger(s, level, slope, hyst, 0, s.len()), Some(t));
        }
        out
    }

    #[test]
    fn scanner_matches_find_trigger_at_every_split() {
        let s: Vec<f64> = sine(400, 37.3, 0.4)
            .iter()
            .enumerate()
            .map(|(i, v)| v + if i % 3 == 0 { 0.03 } else { -0.02 })
            .collect();
        for slope in [Slope::Rising, Slope::Falling, Slope::Both] {
            let want = find_trigger(&s, 0.1, slope, 0.2, 0, s.len()).unwrap();
            let whole = all_edges(&s, 0.1, slope, 0.2);
            assert!(whole.len() > 5);
            for k in 0..s.len() {
                let mut sc = EdgeScanner::new(0.1, slope, 0.2, 0);
                let mut got = sc.push(&s[..k], 0);
                if got.is_none() {
                    got = sc.push(&s[k..], k as u64);
                }
                assert_eq!(got, Some(want), "split at {k}");
                // All edges, cut into pieces of k + 1.
                let mut sc = EdgeScanner::new(0.1, slope, 0.2, 0);
                let mut edges = Vec::new();
                let mut at = 0;
                while at < s.len() {
                    let end = (at + k + 1).min(s.len());
                    while let Some(t) = sc.push(&s[sc.next() as usize..end], sc.next()) {
                        edges.push(t);
                    }
                    at = end;
                }
                assert_eq!(edges, whole, "pieces of {}", k + 1);
            }
        }
    }

    #[test]
    fn scanner_edge_split_between_prev_and_crossing() {
        let mut s = vec![-1.0; 10];
        s.extend(vec![1.0; 10]);
        let mut sc = EdgeScanner::new(0.0, Slope::Rising, 0.1, 100);
        // The last sample below the level ends the first piece.
        assert_eq!(sc.push(&s[..10], 100), None);
        assert_eq!(sc.push(&s[10..], 110), Some(109.5));
        assert_eq!(sc.next(), 111);
    }

    #[test]
    fn scanner_hold_off_keeps_the_arm_state() {
        // Square of period 10: rising edges at 4.5, 14.5, 24.5, ...
        let s: Vec<f64> = (0..100)
            .map(|i| if i % 10 < 5 { -1.0 } else { 1.0 })
            .collect();
        let mut sc = EdgeScanner::new(0.0, Slope::Rising, 0.1, 0);
        assert_eq!(sc.push(&s, 0), Some(4.5));
        sc.hold_until(20.0);
        // 14.5 is held off; 24.5, the first edge after it, is not missed.
        assert_eq!(sc.push(&s[sc.next() as usize..], sc.next()), Some(24.5));
        // A jump in the index starts over: the arm state is gone.
        let mut sc = EdgeScanner::new(0.0, Slope::Rising, 0.1, 0);
        sc.push(&s[..4], 0);
        assert_eq!(sc.push(&s[5..], 5), Some(14.5));
    }
}
