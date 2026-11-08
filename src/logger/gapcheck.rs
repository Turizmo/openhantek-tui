//! Finds samples the scope lost, by checking the stream against the clock.
//!
//! When the scope's FIFO overflows, time passes but its data never comes,
//! and nothing in the data says so. So the logger counts the samples that
//! came and compares them with the time since the first piece: the lag
//! (clock time minus sample time) stays about the same while the stream is
//! whole, and steps up by the lost time when samples are lost.
//!
//! The lag jitters by about one transfer, so a loss counts only when the
//! lag stays more than [`GapCheck::threshold`] above its lowest value for
//! a whole [`GapCheck::window`]. The lowest value may creep up by
//! [`DRIFT`], because the scope's crystal and the PC's clock do not run
//! at exactly the same speed. Pieces carry the time they came off the bus,
//! so a busy consumer does not look like lost samples. Samples the host
//! dropped itself are known exactly and do not need the clock.

use std::collections::VecDeque;
use std::time::Instant;

/// How fast the lag may grow without a loss: the clocks may differ by
/// this much (500 ppm). Quartz crystals are within about 50 ppm, but a
/// scope in a VM measured 150 ppm against the guest's clock. A scope
/// that loses packets loses far more (0.5 % at 48 MB/s).
pub const DRIFT: f64 = 500e-6;

#[derive(Clone, Debug)]
pub struct GapCheck {
    /// Samples per second.
    rate: f64,
    /// Lag above the lowest one that means a loss, in seconds.
    pub threshold: f64,
    /// How long the lag must stay that high, in seconds.
    pub window: f64,
    /// When the first piece came.
    t0: Option<Instant>,
    /// Samples since then, lost ones included (the first piece not).
    got: u64,
    /// The lowest lag (seconds) and when it was seen (seconds after t0).
    floor: Option<(f64, f64)>,
    /// Recent (time, lag) pairs, the oldest first.
    recent: VecDeque<(f64, f64)>,
}

impl GapCheck {
    /// For a stream of `rate` samples per second that comes in transfers
    /// of `transfer_secs` seconds of data each.
    pub fn new(rate: f64, transfer_secs: f64) -> Self {
        Self {
            rate,
            threshold: 0.02 + 2.0 * transfer_secs,
            window: 0.1 + 4.0 * transfer_secs,
            t0: None,
            got: 0,
            floor: None,
            recent: VecDeque::new(),
        }
    }

    /// The lowest lag now, with the drift allowance since it was seen.
    fn floor_at(&self, t: f64) -> Option<f64> {
        self.floor.map(|(lag, at)| lag + DRIFT * (t - at).max(0.0))
    }

    /// A piece of `n` samples came off the bus at `at`. `lost` samples
    /// were dropped just before it, as the host knows (exact). `restart`:
    /// the stream started again after an error, so there is a gap of
    /// whatever length the clock says. Returns the samples to mark as lost
    /// before this piece, or `None` when the stream is whole.
    pub fn piece(&mut self, at: Instant, n: u64, lost: u64, restart: bool) -> Option<u64> {
        let Some(t0) = self.t0 else {
            // The first piece sets the time: its last sample is at t0.
            self.t0 = Some(at);
            return (lost > 0).then_some(lost);
        };
        let t = at.saturating_duration_since(t0).as_secs_f64();
        self.got += lost + n;
        let lag = t - self.got as f64 / self.rate;
        let mut gap = (lost > 0).then_some(lost);
        let floor = self.floor_at(t);
        if restart {
            // Count the clock time as lost (at least nothing).
            let extra = floor.map_or(0.0, |f| (lag - f).max(0.0));
            let more = (extra * self.rate).round() as u64;
            self.lose(more);
            self.recent.clear();
            return Some(gap.unwrap_or(0) + more);
        }
        match floor {
            Some(f) if lag >= f => {}
            _ => self.floor = Some((lag, t)),
        }
        self.recent.push_back((t, lag));
        while self.recent.len() > 2 && self.recent[1].0 <= t - self.window {
            self.recent.pop_front();
        }
        let full = self.recent.front().is_some_and(|r| r.0 <= t - self.window);
        let floor = self.floor_at(t).unwrap_or(lag);
        let low = self
            .recent
            .iter()
            .map(|r| r.1)
            .fold(f64::INFINITY, f64::min);
        if full && low - floor > self.threshold {
            let more = ((low - floor) * self.rate).round() as u64;
            self.lose(more);
            gap = Some(gap.unwrap_or(0) + more);
        }
        gap
    }

    /// Count `n` samples that did not come.
    fn lose(&mut self, n: u64) {
        self.got += n;
        let d = n as f64 / self.rate;
        for r in &mut self.recent {
            r.1 -= d;
        }
    }

    /// Samples counted so far (the first piece not), lost ones included.
    pub fn samples(&self) -> u64 {
        self.got
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Pieces of `per` samples at `rate`, arriving on time plus `jitter(i)`.
    #[allow(clippy::too_many_arguments)]
    fn feed(
        g: &mut GapCheck,
        t0: Instant,
        rate: f64,
        per: u64,
        pieces: std::ops::Range<u64>,
        skew: f64,
        mut jitter: impl FnMut(u64) -> f64,
        mut drop_at: impl FnMut(u64) -> u64,
    ) -> Vec<(u64, u64)> {
        let mut out = Vec::new();
        let mut made = pieces.start * per;
        for i in pieces {
            made += drop_at(i);
            made += per;
            let secs = made as f64 / rate * (1.0 + skew) + jitter(i);
            let at = t0 + Duration::from_secs_f64(secs.max(0.0));
            if let Some(n) = g.piece(at, per, 0, false) {
                out.push((i, n));
            }
        }
        out
    }

    #[test]
    fn whole_stream_with_jitter_and_drift_has_no_gaps() {
        // 16 MS/s, transfers of 128 k samples (8 ms), 10 min.
        let rate = 16e6;
        let per = 131_072;
        let mut g = GapCheck::new(rate, per as f64 / rate);
        let t0 = Instant::now();
        let mut x = 1u64;
        let jitter = |_| {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
            // Up to 15 ms late, and one long stall that the queue rides out.
            (x >> 40) as f64 / (1u64 << 24) as f64 * 0.015
        };
        let n = (600.0 * rate / per as f64) as u64;
        let gaps = feed(&mut g, t0, rate, per, 0..n, 100e-6, jitter, |_| 0);
        assert!(gaps.is_empty(), "{gaps:?}");
    }

    #[test]
    fn a_stall_that_catches_up_is_no_gap() {
        let rate = 1e6;
        let per = 10_000; // 10 ms
        let mut g = GapCheck::new(rate, 0.01);
        let t0 = Instant::now();
        // Pieces 100..110 all arrive late, at the time of piece 110.
        let late = |i: u64| {
            if (100..110).contains(&i) {
                (110 - i) as f64 * 0.01
            } else {
                0.0
            }
        };
        let gaps = feed(&mut g, t0, rate, per, 0..300, 0.0, late, |_| 0);
        assert!(gaps.is_empty(), "{gaps:?}");
    }

    #[test]
    fn lost_samples_are_found_and_counted() {
        let rate = 1e6;
        let per = 10_000;
        let mut g = GapCheck::new(rate, 0.01);
        let t0 = Instant::now();
        // 50 ms of samples (5 % of a second) do not come before piece 200.
        let gaps = feed(
            &mut g,
            t0,
            rate,
            per,
            0..400,
            0.0,
            |_| 0.0,
            |i| {
                if i == 200 {
                    50_000
                } else {
                    0
                }
            },
        );
        assert_eq!(gaps.len(), 1, "{gaps:?}");
        let (at, n) = gaps[0];
        assert!((200..=220).contains(&at), "found at piece {at}");
        assert!((49_000..=51_000).contains(&n), "{n}");
        // The count goes on as if the lost samples had come.
        assert_eq!(g.samples(), 399 * per + n);
    }

    #[test]
    fn host_drops_and_restarts_are_always_gaps() {
        let rate = 1e5;
        let mut g = GapCheck::new(rate, 0.01);
        let t0 = Instant::now();
        let at = |s: f64| t0 + Duration::from_secs_f64(s);
        assert_eq!(g.piece(at(0.01), 1000, 0, false), None);
        assert_eq!(g.piece(at(0.02), 1000, 0, false), None);
        // The host dropped 512 samples: exact, at once.
        assert_eq!(g.piece(at(0.03 + 0.00512), 1000, 512, false), Some(512));
        // A restart after 0.5 s without data.
        let n = g.piece(at(0.54), 1000, 0, true).unwrap();
        assert!((49_000..51_000).contains(&n), "{n}");
        assert_eq!(g.piece(at(0.55), 1000, 0, false), None);
    }
}
