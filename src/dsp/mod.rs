//! Signal processing. Everything here is pure code with no I/O, so it is
//! easy to test.

pub mod fft;
pub mod math;
pub mod measure;
pub mod trigger;

/// Band-limited (sin(x)/x) interpolation, with a Lanczos window.
/// Makes `factor` points per input sample. Use it when a screen has few
/// samples, so the trace looks like the real analog signal.
pub fn sinc_interpolate(samples: &[f64], factor: usize) -> Vec<f64> {
    const A: isize = 6;
    if factor <= 1 || samples.len() < 2 {
        return samples.to_vec();
    }
    let n = samples.len();
    let out_len = (n - 1) * factor + 1;
    let mut out = Vec::with_capacity(out_len);
    for j in 0..out_len {
        let x = j as f64 / factor as f64;
        let base = x.floor() as isize;
        if j % factor == 0 {
            out.push(samples[base as usize]);
            continue;
        }
        let mut sum = 0.0;
        let mut weight = 0.0;
        for i in (base - A + 1)..=(base + A) {
            let idx = i.clamp(0, n as isize - 1) as usize;
            let w = lanczos(x - i as f64, A as f64);
            sum += samples[idx] * w;
            weight += w;
        }
        out.push(if weight.abs() > 1e-12 {
            sum / weight
        } else {
            sum
        });
    }
    out
}

fn lanczos(x: f64, a: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else if x.abs() >= a {
        0.0
    } else {
        let px = std::f64::consts::PI * x;
        a * px.sin() * (px / a).sin() / (px * px)
    }
}

/// Running average of many captures, sample by sample. Noise goes down by
/// the square root of the count.
#[derive(Clone, Debug, Default)]
pub struct Averager {
    sum: Vec<f64>,
    count: usize,
}

impl Averager {
    pub fn reset(&mut self) {
        self.sum.clear();
        self.count = 0;
    }

    /// Add one capture and return the average. With `depth` captures in the
    /// average, it acts like an exponential moving average after that.
    pub fn push(&mut self, samples: &[f64], depth: usize) -> Vec<f64> {
        if depth <= 1 {
            self.reset();
            return samples.to_vec();
        }
        if self.sum.len() != samples.len() {
            self.sum = samples.to_vec();
            self.count = 1;
            return samples.to_vec();
        }
        self.count = (self.count + 1).min(depth);
        let k = 1.0 / self.count as f64;
        for (a, v) in self.sum.iter_mut().zip(samples) {
            *a += (v - *a) * k;
        }
        self.sum.clone()
    }

    pub fn count(&self) -> usize {
        self.count
    }
}

/// The peak band over the last captures of the average: for each point
/// the lowest of their minima and the highest of their maxima. A true
/// sliding window over the last `depth` captures (never an average of
/// minima or maxima), so the band holds the absolute extremes. Reset it
/// with the [`Averager`].
#[derive(Clone, Debug, Default)]
pub struct PeakHold {
    /// Min and max of the last captures, oldest first.
    window: std::collections::VecDeque<(Vec<f64>, Vec<f64>)>,
}

impl PeakHold {
    pub fn reset(&mut self) {
        self.window.clear();
    }

    /// Add the min and max of one capture; return the min and max over
    /// the last `depth` captures. A new length starts over.
    pub fn push(&mut self, min: &[f64], max: &[f64], depth: usize) -> (Vec<f64>, Vec<f64>) {
        if depth <= 1 {
            self.reset();
            return (min.to_vec(), max.to_vec());
        }
        if self.window.front().is_some_and(|w| w.0.len() != min.len()) {
            self.reset();
        }
        while self.window.len() >= depth {
            self.window.pop_front();
        }
        self.window.push_back((min.to_vec(), max.to_vec()));
        let mut lo = min.to_vec();
        let mut hi = max.to_vec();
        for (a, b) in &self.window {
            for (l, v) in lo.iter_mut().zip(a) {
                *l = l.min(*v);
            }
            for (h, v) in hi.iter_mut().zip(b) {
                *h = h.max(*v);
            }
        }
        (lo, hi)
    }

    /// Captures in the window.
    pub fn count(&self) -> usize {
        self.window.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sinc_keeps_input_points() {
        let s = [0.0, 1.0, 0.0, -1.0, 0.0];
        let out = sinc_interpolate(&s, 4);
        assert_eq!(out.len(), 17);
        for (i, v) in s.iter().enumerate() {
            assert!((out[i * 4] - v).abs() < 1e-12);
        }
    }

    #[test]
    fn sinc_rebuilds_a_sampled_sine() {
        // 5 samples per period is enough for sin(x)/x, not for straight lines.
        let period = 5.0;
        let s: Vec<f64> = (0..64)
            .map(|i| (std::f64::consts::TAU * i as f64 / period).sin())
            .collect();
        let out = sinc_interpolate(&s, 8);
        // Check the middle part, away from the edges.
        for (j, v) in out.iter().enumerate().skip(16 * 8).take(32 * 8) {
            let want = (std::f64::consts::TAU * (j as f64 / 8.0) / period).sin();
            assert!((v - want).abs() < 0.05, "j {j}: {v} vs {want}");
        }
    }

    #[test]
    fn factor_one_is_a_copy() {
        assert_eq!(sinc_interpolate(&[1.0, 2.0], 1), vec![1.0, 2.0]);
    }

    #[test]
    fn averager_converges_and_resets() {
        let mut avg = Averager::default();
        assert_eq!(avg.push(&[1.0, 1.0], 4), vec![1.0, 1.0]);
        assert_eq!(avg.push(&[3.0, 3.0], 4), vec![2.0, 2.0]);
        assert_eq!(avg.count(), 2);
        // New length starts over.
        assert_eq!(avg.push(&[5.0], 4), vec![5.0]);
        assert_eq!(avg.count(), 1);
        // Depth 1 means no averaging.
        assert_eq!(avg.push(&[7.0], 1), vec![7.0]);
        assert_eq!(avg.count(), 0);
    }

    #[test]
    fn peak_hold_is_a_sliding_window_of_extremes() {
        let mut p = PeakHold::default();
        assert_eq!(p.push(&[0.0], &[1.0], 3), (vec![0.0], vec![1.0]));
        assert_eq!(p.push(&[-5.0], &[9.0], 3), (vec![-5.0], vec![9.0]));
        assert_eq!(p.push(&[0.0], &[1.0], 3), (vec![-5.0], vec![9.0]));
        // The -5 / 9 capture is still one of the last 3, then it drops out.
        assert_eq!(p.push(&[0.0], &[1.0], 3), (vec![-5.0], vec![9.0]));
        assert_eq!(p.push(&[0.0], &[1.0], 3), (vec![0.0], vec![1.0]));
        assert_eq!(p.count(), 3);
        // Never an average: a single wide capture is kept whole.
        assert_eq!(p.push(&[-1.0, 0.0], &[2.0, 3.0], 3).0, vec![-1.0, 0.0]);
        assert_eq!(p.count(), 1);
        assert_eq!(p.push(&[1.0], &[2.0], 1), (vec![1.0], vec![2.0]));
        assert_eq!(p.count(), 0);
    }
}
