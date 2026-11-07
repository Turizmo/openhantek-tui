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

/// Fractional index where the line from `prev` (at `i - 1`) to `v` (at `i`)
/// crosses `level`.
fn crossing(i: usize, prev: f64, v: f64, level: f64) -> f64 {
    let span = v - prev;
    let frac = if span.abs() < f64::EPSILON {
        1.0
    } else {
        ((level - prev) / span).clamp(0.0, 1.0)
    };
    (i - 1) as f64 + frac
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
}
