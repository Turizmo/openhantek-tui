//! Automatic measurements on one trace: voltages and timing.

/// All measurements of one trace. Timing values are `None` when the trace
/// has too few edges to measure them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Measurements {
    pub max: f64,
    pub min: f64,
    pub pk_pk: f64,
    pub mean: f64,
    /// True RMS, DC included.
    pub rms: f64,
    /// RMS with the DC part removed (standard deviation).
    pub ac_rms: f64,
    /// Most common high level (for square waves), else the max.
    pub top: f64,
    /// Most common low level (for square waves), else the min.
    pub base: f64,
    /// top - base.
    pub amplitude: f64,
    pub freq: Option<f64>,
    pub period: Option<f64>,
    /// Positive duty cycle, 0..1.
    pub duty: Option<f64>,
    pub pos_width: Option<f64>,
    pub neg_width: Option<f64>,
    /// 10 % to 90 % rise time.
    pub rise: Option<f64>,
    /// 90 % to 10 % fall time.
    pub fall: Option<f64>,
}

/// Measure a trace. `dt` is the time between samples, in seconds.
pub fn measure(samples: &[f64], dt: f64) -> Measurements {
    let n = samples.len();
    if n == 0 {
        return Measurements::default();
    }
    let min = samples.iter().copied().fold(f64::INFINITY, f64::min);
    let max = samples.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let mean = samples.iter().sum::<f64>() / n as f64;
    let mean_sq = samples.iter().map(|v| v * v).sum::<f64>() / n as f64;
    let rms = mean_sq.sqrt();
    let ac_rms = (mean_sq - mean * mean).max(0.0).sqrt();
    let (top, base) = top_base(samples, min, max);

    let mut m = Measurements {
        max,
        min,
        pk_pk: max - min,
        mean,
        rms,
        ac_rms,
        top,
        base,
        amplitude: top - base,
        ..Default::default()
    };

    let edges = find_edges(samples, min, max);
    let period_from =
        |e: &[f64]| (e.len() >= 2).then(|| (e[e.len() - 1] - e[0]) / (e.len() - 1) as f64);
    // Mean period from rising and from falling edges, in samples.
    let periods: Vec<f64> = [period_from(&edges.rising), period_from(&edges.falling)]
        .into_iter()
        .flatten()
        .collect();
    if periods.is_empty() {
        return m;
    }
    let period = periods.iter().sum::<f64>() / periods.len() as f64;
    if period <= 0.0 {
        return m;
    }
    m.period = Some(period * dt);
    m.freq = Some(1.0 / (period * dt));

    // Pulse widths: from each rising edge to the next falling edge. Both
    // lists are in order, so one pass finds them (noise has many edges).
    let mut widths = Vec::new();
    let mut j = 0;
    for &r in &edges.rising {
        while edges.falling.get(j).is_some_and(|&f| f <= r) {
            j += 1;
        }
        if let Some(&f) = edges.falling.get(j) {
            if f - r < period * 1.5 {
                widths.push(f - r);
            }
        }
    }
    if !widths.is_empty() {
        let w = widths.iter().sum::<f64>() / widths.len() as f64;
        m.pos_width = Some(w * dt);
        m.neg_width = Some((period - w).max(0.0) * dt);
        m.duty = Some((w / period).clamp(0.0, 1.0));
    }

    let lo = base + 0.1 * (top - base);
    let hi = base + 0.9 * (top - base);
    let reach = (period / 2.0).ceil() as usize + 1;
    m.rise = mean_of(
        edges
            .rising
            .iter()
            .filter_map(|&e| transition(samples, e, lo, hi, reach)),
    )
    .map(|s| s * dt);
    m.fall = mean_of(
        edges
            .falling
            .iter()
            .filter_map(|&e| transition(samples, e, hi, lo, reach)),
    )
    .map(|s| s * dt);
    m
}

/// Mean 10 % to 90 % rise time and 90 % to 10 % fall time, in samples, of
/// the edges in `samples` between the levels `(top, base)`: those of a
/// longer record, so a short piece with only an edge or two works, or
/// with None the top and base of `samples`. An edge cut off by an end of
/// `samples` does not count.
pub fn edge_times(samples: &[f64], levels: Option<(f64, f64)>) -> (Option<f64>, Option<f64>) {
    let (top, base) = levels.unwrap_or_else(|| {
        let min = samples.iter().copied().fold(f64::INFINITY, f64::min);
        let max = samples.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        top_base(samples, min, max)
    });
    let amp = top - base;
    if amp.is_nan() || amp <= 0.0 || samples.len() < 2 {
        return (None, None);
    }
    let (lo, hi) = (base + 0.1 * amp, base + 0.9 * amp);
    let edges = find_edges(samples, base, top);
    let reach = samples.len();
    let rise = mean_of(
        edges
            .rising
            .iter()
            .filter_map(|&e| transition(samples, e, lo, hi, reach)),
    );
    let fall = mean_of(
        edges
            .falling
            .iter()
            .filter_map(|&e| transition(samples, e, hi, lo, reach)),
    );
    (rise, fall)
}

/// Rising and falling edges of `samples` through the middle of `base` and
/// `top` (with a band of 10 % of the swing against noise), as fractional
/// sample indexes, in order.
pub fn edges(samples: &[f64], top: f64, base: f64) -> (Vec<f64>, Vec<f64>) {
    let e = find_edges(samples, base, top);
    (e.rising, e.falling)
}

/// Top and base of a histogram of ADC counts (`hist[count]`), as counts,
/// like [`top_base`]: the biggest peak (with its neighbours) in the top
/// and in the bottom 40 % of the range, when it holds 10 % of the samples,
/// else the highest and lowest count. None for an empty histogram.
pub fn top_base_counts(hist: &[u64; 256]) -> Option<(usize, usize)> {
    let lo = hist.iter().position(|&n| n > 0)?;
    let hi = hist.iter().rposition(|&n| n > 0)?;
    let n: u64 = hist.iter().sum();
    let span = hi - lo;
    let peak = |r: std::ops::Range<usize>| {
        let bin = r.clone().max_by_key(|&b| hist[b])?;
        let a = bin.saturating_sub(1).max(r.start);
        let b = (bin + 1).min(r.end - 1);
        let count: u64 = hist[a..=b].iter().sum();
        (count * 10 >= n).then_some(bin)
    };
    let top = peak(lo + (span * 6).div_ceil(10)..hi + 1).unwrap_or(hi);
    let base = peak(lo..lo + span * 4 / 10 + 1).unwrap_or(lo);
    Some((top, base))
}

/// Find top and base levels with a histogram. A square wave has two big
/// peaks in the histogram: those are top and base. A sine has none, so we
/// fall back to max and min.
fn top_base(samples: &[f64], min: f64, max: f64) -> (f64, f64) {
    const BINS: usize = 128;
    let span = max - min;
    if span <= 0.0 {
        return (max, min);
    }
    let mut hist = [0usize; BINS];
    for &v in samples {
        // + 0.5 and a cut: `round` is a slow library call.
        let b = (((v - min) / span) * (BINS - 1) as f64 + 0.5) as usize;
        hist[b.min(BINS - 1)] += 1;
    }
    let n = samples.len();
    // A level counts when its peak (with neighbours) holds 10 % of the samples.
    let peak = |range: std::ops::Range<usize>| {
        let (bin, _) = range
            .clone()
            .map(|b| (b, hist[b]))
            .max_by_key(|&(_, c)| c)
            .unwrap_or((0, 0));
        let lo = bin.saturating_sub(1).max(range.start);
        let hi = (bin + 1).min(range.end - 1);
        let count: usize = hist[lo..=hi].iter().sum();
        let value = min + bin as f64 / (BINS - 1) as f64 * span;
        (count * 10 >= n).then_some(value)
    };
    let top = peak(BINS * 6 / 10..BINS).unwrap_or(max);
    let base = peak(0..BINS * 4 / 10).unwrap_or(min);
    (top, base)
}

struct Edges {
    rising: Vec<f64>,
    falling: Vec<f64>,
}

/// Find edges through the middle level. A band of 10 % of the swing on each
/// side stops noise from making extra edges. Edge positions are fractional
/// sample indexes of the middle crossing.
fn find_edges(samples: &[f64], min: f64, max: f64) -> Edges {
    let mid = (max + min) / 2.0;
    let band = (max - min) * 0.1;
    let mut edges = Edges {
        rising: Vec::new(),
        falling: Vec::new(),
    };
    if max - min <= 0.0 {
        return edges;
    }
    let mut high: Option<bool> = None;
    let (mut cand_rise, mut cand_fall) = (None, None);
    for i in 1..samples.len() {
        let (prev, v) = (samples[i - 1], samples[i]);
        if prev < mid && v >= mid {
            cand_rise = Some(cross(i, prev, v, mid));
        } else if prev >= mid && v < mid {
            cand_fall = Some(cross(i, prev, v, mid));
        }
        if v > mid + band && high != Some(true) {
            if high.is_some() {
                edges.rising.extend(cand_rise);
            }
            high = Some(true);
        } else if v < mid - band && high != Some(false) {
            if high.is_some() {
                edges.falling.extend(cand_fall);
            }
            high = Some(false);
        }
    }
    edges
}

fn cross(i: usize, prev: f64, v: f64, level: f64) -> f64 {
    let span = v - prev;
    let frac = if span.abs() < f64::EPSILON {
        0.0
    } else {
        (level - prev) / span
    };
    (i - 1) as f64 + frac
}

/// Time in samples for the signal to go from `from` to `to` around the edge
/// at `edge`. Works for both rising (from < to) and falling edges. It
/// looks at most `reach` samples to each side (half a period: an edge
/// takes no longer, and noise with many edges stays fast).
fn transition(samples: &[f64], edge: f64, from: f64, to: f64, reach: usize) -> Option<f64> {
    let rising = to > from;
    let past = |v: f64, level: f64| if rising { v >= level } else { v <= level };
    let e = edge.floor() as usize;
    // Walk back to the last sample before the `from` level.
    let mut i = e.min(samples.len() - 1);
    let stop = e.saturating_sub(reach);
    while i > stop && past(samples[i], from) {
        i -= 1;
    }
    if past(samples[i], from) || i + 1 >= samples.len() {
        return None;
    }
    let t_from = cross(i + 1, samples[i], samples[i + 1], from);
    // Walk forward to the first sample past the `to` level.
    let mut j = e + 1;
    let end = samples.len().min(e + 1 + reach);
    while j < end && !past(samples[j], to) {
        j += 1;
    }
    if j >= end || j == 0 {
        return None;
    }
    let t_to = cross(j, samples[j - 1], samples[j], to);
    (t_to >= t_from).then_some(t_to - t_from)
}

fn mean_of(values: impl Iterator<Item = f64>) -> Option<f64> {
    let (sum, count) = values.fold((0.0, 0usize), |(s, c), v| (s + v, c + 1));
    (count > 0).then(|| sum / count as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f64 = 1e-6;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1e-12)
    }

    fn sine(freq: f64, amp: f64, dc: f64, n: usize) -> Vec<f64> {
        (0..n)
            .map(|i| dc + amp * (std::f64::consts::TAU * freq * i as f64 * DT).sin())
            .collect()
    }

    fn square(freq: f64, duty: f64, low: f64, high: f64, n: usize) -> Vec<f64> {
        (0..n)
            .map(|i| {
                if (i as f64 * DT * freq).rem_euclid(1.0) < duty {
                    high
                } else {
                    low
                }
            })
            .collect()
    }

    #[test]
    fn sine_voltages_and_frequency() {
        let s = sine(1000.0, 1.5, 0.5, 10_000);
        let m = measure(&s, DT);
        assert!(close(m.max, 2.0, 1e-3));
        assert!(close(m.min, -1.0, 1e-3));
        assert!(close(m.pk_pk, 3.0, 1e-3));
        assert!(close(m.mean, 0.5, 1e-3));
        assert!(close(m.ac_rms, 1.5 / 2f64.sqrt(), 1e-3));
        assert!(close(m.rms, (0.25 + 1.125f64).sqrt(), 1e-3));
        assert!(close(m.freq.unwrap(), 1000.0, 1e-4), "{:?}", m.freq);
        assert!(close(m.period.unwrap(), 1e-3, 1e-4));
        assert!(close(m.duty.unwrap(), 0.5, 1e-2));
    }

    #[test]
    fn square_duty_and_levels() {
        let s = square(2500.0, 0.25, 0.0, 3.3, 10_000);
        let m = measure(&s, DT);
        assert!(close(m.top, 3.3, 1e-2), "top {}", m.top);
        assert!(m.base.abs() < 0.03, "base {}", m.base);
        assert!(close(m.amplitude, 3.3, 1e-2));
        assert!(close(m.freq.unwrap(), 2500.0, 1e-3));
        assert!(close(m.duty.unwrap(), 0.25, 1e-2), "{:?}", m.duty);
        assert!(close(m.pos_width.unwrap(), 100e-6, 1e-2));
        assert!(close(m.neg_width.unwrap(), 300e-6, 1e-2));
    }

    #[test]
    fn rise_time_of_a_ramp_edge() {
        // Low for 100 samples, a 50 sample linear ramp, high for 100: repeat.
        let mut s = Vec::new();
        for _ in 0..5 {
            s.extend(vec![0.0; 100]);
            s.extend((0..50).map(|i| i as f64 / 50.0));
            s.extend(vec![1.0; 100]);
            s.extend((0..50).map(|i| 1.0 - i as f64 / 50.0));
        }
        let m = measure(&s, DT);
        // 10 % to 90 % of a 50 sample ramp is 40 samples.
        assert!(close(m.rise.unwrap(), 40e-6, 0.03), "{:?}", m.rise);
        assert!(close(m.fall.unwrap(), 40e-6, 0.03), "{:?}", m.fall);
        assert!(close(m.period.unwrap(), 300e-6, 1e-3));
    }

    #[test]
    fn noise_does_not_make_extra_edges() {
        let mut noise = 12345u64;
        let s: Vec<f64> = square(1000.0, 0.5, -1.0, 1.0, 20_000)
            .into_iter()
            .map(|v| {
                noise ^= noise << 13;
                noise ^= noise >> 7;
                noise ^= noise << 17;
                v + ((noise >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * 0.3
            })
            .collect();
        let m = measure(&s, DT);
        assert!(close(m.freq.unwrap(), 1000.0, 1e-3), "{:?}", m.freq);
        assert!(close(m.duty.unwrap(), 0.5, 2e-2));
    }

    #[test]
    fn flat_and_empty_traces() {
        let m = measure(&[1.0; 100], DT);
        assert_eq!(m.pk_pk, 0.0);
        assert_eq!(m.freq, None);
        assert_eq!(m.mean, 1.0);
        let m = measure(&[], DT);
        assert_eq!(m, Measurements::default());
    }

    #[test]
    fn one_edge_gives_no_frequency() {
        let mut s = vec![0.0; 100];
        s.extend(vec![1.0; 100]);
        let m = measure(&s, DT);
        assert_eq!(m.freq, None);
        assert_eq!(m.pk_pk, 1.0);
    }
}
