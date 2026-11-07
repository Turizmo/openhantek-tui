//! Spectrum analysis: window functions and a radix-2 FFT.

use serde::{Deserialize, Serialize};

/// Window function applied before the FFT.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Window {
    Rectangular,
    #[default]
    Hann,
    Hamming,
    Blackman,
    FlatTop,
}

impl Window {
    pub const ALL: [Window; 5] = [
        Window::Rectangular,
        Window::Hann,
        Window::Hamming,
        Window::Blackman,
        Window::FlatTop,
    ];

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|w| *w == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub fn name(self) -> &'static str {
        match self {
            Window::Rectangular => "Rect",
            Window::Hann => "Hann",
            Window::Hamming => "Hamming",
            Window::Blackman => "Blackman",
            Window::FlatTop => "Flat top",
        }
    }

    /// Window weight at sample `i` of `n`.
    pub fn weight(self, i: usize, n: usize) -> f64 {
        if n < 2 {
            return 1.0;
        }
        let x = std::f64::consts::TAU * i as f64 / (n - 1) as f64;
        match self {
            Window::Rectangular => 1.0,
            Window::Hann => 0.5 - 0.5 * x.cos(),
            Window::Hamming => 0.54 - 0.46 * x.cos(),
            Window::Blackman => 0.42 - 0.5 * x.cos() + 0.08 * (2.0 * x).cos(),
            Window::FlatTop => {
                0.215_578_95 - 0.416_631_58 * x.cos() + 0.277_263_158 * (2.0 * x).cos()
                    - 0.083_578_947 * (3.0 * x).cos()
                    + 0.006_947_368 * (4.0 * x).cos()
            }
        }
    }
}

/// Amplitude spectrum of a trace.
#[derive(Clone, Debug, Default)]
pub struct Spectrum {
    /// Frequency step between bins, in Hz.
    pub df: f64,
    /// RMS amplitude of each bin, in volts. Bin 0 is DC.
    pub rms: Vec<f64>,
}

impl Spectrum {
    pub fn freq(&self, bin: usize) -> f64 {
        bin as f64 * self.df
    }

    /// Level of a bin in dBV (0 dBV = 1 V RMS).
    pub fn dbv(&self, bin: usize) -> f64 {
        20.0 * self.rms[bin].max(1e-12).log10()
    }

    /// Strongest bin, DC not counted. Frequency is refined between bins
    /// with a parabola through the three highest points.
    pub fn peak(&self) -> Option<(f64, f64)> {
        if self.rms.len() < 3 {
            return None;
        }
        let (k, _) = self
            .rms
            .iter()
            .enumerate()
            .skip(1)
            .max_by(|a, b| a.1.total_cmp(b.1))?;
        let mut f = self.freq(k);
        if k + 1 < self.rms.len() {
            let (a, b, c) = (self.dbv(k - 1), self.dbv(k), self.dbv(k + 1));
            let denom = a - 2.0 * b + c;
            if denom.abs() > 1e-12 {
                let p = (0.5 * (a - c) / denom).clamp(-0.5, 0.5);
                f = (k as f64 + p) * self.df;
            }
        }
        Some((f, self.rms[k]))
    }
}

/// Spectrum of `samples` (in volts, `dt` seconds apart). The input is cut
/// to the largest power of two that fits, at most `max_len` samples.
pub fn spectrum(samples: &[f64], dt: f64, window: Window, max_len: usize) -> Spectrum {
    let mut n = 1usize;
    while n * 2 <= samples.len().min(max_len) {
        n *= 2;
    }
    if n < 4 || dt <= 0.0 {
        return Spectrum::default();
    }
    let input = &samples[..n];
    let weights: Vec<f64> = (0..n).map(|i| window.weight(i, n)).collect();
    let gain: f64 = weights.iter().sum();
    let mut re: Vec<f64> = input.iter().zip(&weights).map(|(v, w)| v * w).collect();
    let mut im = vec![0.0; n];
    fft(&mut re, &mut im);
    let half = n / 2;
    let rms = (0..=half)
        .map(|k| {
            let mag = (re[k] * re[k] + im[k] * im[k]).sqrt() / gain;
            if k == 0 || k == half {
                mag
            } else {
                // Peak amplitude is 2 * mag; RMS is that over sqrt(2).
                mag * std::f64::consts::SQRT_2
            }
        })
        .collect();
    Spectrum {
        df: 1.0 / (n as f64 * dt),
        rms,
    }
}

/// In-place iterative radix-2 FFT. The length must be a power of two.
pub fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    assert_eq!(n, im.len());
    assert!(n.is_power_of_two());
    // Bit reversal.
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -std::f64::consts::TAU / len as f64;
        let (w_im, w_re) = ang.sin_cos();
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                let a = start + k;
                let b = a + len / 2;
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let next = cr * w_re - ci * w_im;
                ci = cr * w_im + ci * w_re;
                cr = next;
            }
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_of_impulse_is_flat() {
        let mut re = vec![0.0; 8];
        let mut im = vec![0.0; 8];
        re[0] = 1.0;
        fft(&mut re, &mut im);
        for k in 0..8 {
            assert!((re[k] - 1.0).abs() < 1e-12 && im[k].abs() < 1e-12);
        }
    }

    #[test]
    fn fft_matches_slow_dft() {
        let n = 64;
        let x: Vec<f64> = (0..n).map(|i| ((i * 7 % 13) as f64).sin()).collect();
        let mut re = x.clone();
        let mut im = vec![0.0; n];
        fft(&mut re, &mut im);
        for k in 0..n {
            let (mut sr, mut si) = (0.0, 0.0);
            for (i, v) in x.iter().enumerate() {
                let a = -std::f64::consts::TAU * (k * i) as f64 / n as f64;
                sr += v * a.cos();
                si += v * a.sin();
            }
            assert!((re[k] - sr).abs() < 1e-9 && (im[k] - si).abs() < 1e-9);
        }
    }

    #[test]
    fn sine_peak_frequency_and_level() {
        let dt = 1e-6;
        let freq = 12_345.0;
        let amp = 2.0;
        let s: Vec<f64> = (0..10_000)
            .map(|i| amp * (std::f64::consts::TAU * freq * i as f64 * dt).sin())
            .collect();
        for w in Window::ALL {
            let sp = spectrum(&s, dt, w, 1 << 16);
            assert_eq!(sp.rms.len(), 8192 / 2 + 1);
            let (f, rms) = sp.peak().unwrap();
            assert!((f - freq).abs() < sp.df * 0.6, "{w:?}: f = {f}");
            // Flat top is accurate in level; others lose up to ~1.5 dB.
            let want = amp / 2f64.sqrt();
            let tol = if w == Window::FlatTop { 0.01 } else { 0.2 };
            assert!((rms - want).abs() / want < tol, "{w:?}: rms = {rms}");
        }
    }

    #[test]
    fn dc_bin() {
        let s = vec![0.7; 1024];
        let sp = spectrum(&s, 1e-3, Window::Rectangular, 4096);
        assert!((sp.rms[0] - 0.7).abs() < 1e-9);
        assert!(sp.rms[1..].iter().all(|v| *v < 1e-9));
    }

    #[test]
    fn too_short_gives_empty() {
        let sp = spectrum(&[1.0, 2.0], 1e-3, Window::Hann, 4096);
        assert!(sp.rms.is_empty());
        assert_eq!(sp.peak(), None);
    }

    #[test]
    fn window_cycle() {
        let mut w = Window::Rectangular;
        for _ in 0..Window::ALL.len() {
            w = w.next();
        }
        assert_eq!(w, Window::Rectangular);
    }
}
