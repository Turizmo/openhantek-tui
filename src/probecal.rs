//! Probe helper: the scope's calibration output (a 2 Vpp square wave)
//! on a probe, for the probe trimmer and a gain check.
//!
//! The helper sets a time/div and volts/div that fit the square wave,
//! measures how far the top (and the bottom) is from flat just after the
//! edges, and the amplitude against the known 2 V. The gain correction
//! goes into the config file, never into the scope's EEPROM.

use crate::dsp::measure::measure;
use crate::dsp::trigger::Slope;
use crate::scope::{time_index, Frame};
use crate::settings::{
    Coupling, Settings, TraceMode, TriggerMode, GAIN_CORRECTION, H_DIVS, VOLT_DIVS,
};
use crate::si::fmt_si;

/// Peak to peak volts of the calibration output.
pub const CAL_AMPLITUDE: f64 = 2.0;
/// Calibration output frequencies the helper offers, in Hz.
pub const PROBE_FREQS: [u32; 8] = [500, 600, 660, 800, 1_000, 1_200, 1_600, 2_000];
/// A top within this many percent of flat is good.
pub const FLAT: f64 = 1.0;
/// Frames to wait after a change before measuring (the average fills).
const SETTLE_FRAMES: usize = 4;
/// Volts/div at the probe tip: 2 V on 4 divisions.
const TIP_VOLTS_DIV: f64 = 0.5;

/// What the helper measured on the square wave.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProbeReading {
    /// How far the level just after the edges is from the level before
    /// the next edge, in percent of the amplitude. Positive: the edges
    /// overshoot (over-compensated); negative: they are rounded
    /// (under-compensated).
    pub edge: f64,
    /// Flat top minus flat bottom, in volts at the probe tip.
    pub amplitude: f64,
    pub freq: Option<f64>,
    /// Half periods it was measured on.
    pub halves: usize,
}

impl ProbeReading {
    /// Amplitude error against the 2 V, in percent.
    pub fn gain_error(&self) -> f64 {
        (self.amplitude / CAL_AMPLITUDE - 1.0) * 100.0
    }

    /// The hint for the trimmer.
    pub fn hint(&self) -> String {
        if self.edge.abs() < FLAT {
            "Flat top: the probe is compensated.".into()
        } else if self.edge > 0.0 {
            "Over-compensated: the edges overshoot. Turn the probe trimmer until the top is flat."
                .into()
        } else {
            "Under-compensated: the edges are rounded. Turn the probe trimmer the other way until the top is flat."
                .into()
        }
    }
}

/// Measure the square wave in `v` (volts, sample time `dt`). `None` when
/// there is no square wave of at least a quarter of the 2 V.
pub fn analyse(v: &[f64], dt: f64) -> Option<ProbeReading> {
    if v.len() < 100 {
        return None;
    }
    let m = measure(v, dt);
    let (base, amp) = (m.base, m.amplitude);
    if amp.is_nan() || amp <= CAL_AMPLITUDE / 4.0 {
        return None;
    }
    // Edges with hysteresis: up past 60 %, down past 40 %.
    let (hi, lo) = (base + 0.6 * amp, base + 0.4 * amp);
    let mut edges: Vec<(usize, bool)> = Vec::new();
    let mut high = v[0] > base + 0.5 * amp;
    for (i, &x) in v.iter().enumerate() {
        if !high && x > hi {
            high = true;
            edges.push((i, true));
        } else if high && x < lo {
            high = false;
            edges.push((i, false));
        }
    }
    let mean = |a: usize, b: usize| v[a..b].iter().sum::<f64>() / (b - a).max(1) as f64;
    let (mut err, mut halves) = (0.0, 0);
    let (mut tops, mut bottoms) = (Vec::new(), Vec::new());
    for w in edges.windows(2) {
        let ((i, up), (j, _)) = (w[0], w[1]);
        let len = j - i;
        if len < 20 {
            continue;
        }
        let at = |f: f64| i + (len as f64 * f) as usize;
        let early = mean(at(0.05), at(0.25));
        let late = mean(at(0.7), at(0.95));
        if up {
            err += early - late;
            tops.push(late);
        } else {
            err += late - early;
            bottoms.push(late);
        }
        halves += 1;
    }
    if tops.is_empty() || bottoms.is_empty() {
        return None;
    }
    let avg = |x: &[f64]| x.iter().sum::<f64>() / x.len() as f64;
    let amplitude = avg(&tops) - avg(&bottoms);
    Some(ProbeReading {
        edge: err / halves as f64 / amplitude * 100.0,
        amplitude,
        freq: m.freq,
        halves,
    })
}

/// The open probe helper.
#[derive(Clone, Debug)]
pub struct ProbeCal {
    /// The user's settings, put back when the helper closes.
    pub saved: Settings,
    pub was_running: bool,
    /// The channel with the probe on the calibration pin.
    pub channel: usize,
    pub freq: u32,
    pub reading: Option<ProbeReading>,
    settle: usize,
}

impl ProbeCal {
    pub fn new(saved: Settings, was_running: bool, channel: usize) -> Self {
        let freq = if PROBE_FREQS.contains(&saved.cal_freq) {
            saved.cal_freq
        } else {
            1_000
        };
        Self {
            saved,
            was_running,
            channel: channel.min(1),
            freq,
            reading: None,
            settle: SETTLE_FRAMES,
        }
    }

    /// The scope settings for the helper: the channel alone, 0.5 V/div
    /// at the tip with 0 V two divisions down, two periods on the screen,
    /// a rising trigger at 1 V, an average of 8 captures.
    pub fn settings(&self) -> Settings {
        let c = self.channel;
        let mut s = self.saved.clone();
        for (i, ch) in s.channels.iter_mut().enumerate() {
            ch.enabled = i == c;
        }
        let ch = &mut s.channels[c];
        let bnc = TIP_VOLTS_DIV / ch.probe as f64;
        ch.volt_idx = VOLT_DIVS
            .iter()
            .position(|v| *v >= bnc * 0.999)
            .unwrap_or(0);
        ch.volts_fine = 1.0;
        ch.offset = -2.0;
        ch.coupling = Coupling::Dc;
        ch.invert = false;
        s.math.enabled = false;
        s.display.xy = false;
        s.display.spectrum = false;
        s.display.averaging = 8;
        s.display.persistence = 0;
        s.display.trace_mode = TraceMode::Average;
        s.selected = c;
        s.cal_freq = self.freq;
        s.time_idx = time_index(2.0 / self.freq as f64 / H_DIVS);
        s.time_fine = 1.0;
        let t = &mut s.trigger;
        t.source = c;
        t.slope = Slope::Rising;
        t.mode = TriggerMode::Auto;
        t.level = CAL_AMPLITUDE / 2.0;
        t.position = 0.1;
        s
    }

    /// Next (or previous) calibration frequency.
    pub fn step_freq(&mut self, up: bool) {
        let i = PROBE_FREQS
            .iter()
            .position(|f| *f == self.freq)
            .unwrap_or(4);
        let j = if up {
            (i + 1).min(PROBE_FREQS.len() - 1)
        } else {
            i.saturating_sub(1)
        };
        self.freq = PROBE_FREQS[j];
        self.changed();
    }

    /// Settings changed: wait for new frames before measuring.
    pub fn changed(&mut self) {
        self.reading = None;
        self.settle = SETTLE_FRAMES;
    }

    /// A new frame on screen.
    pub fn frame(&mut self, f: &Frame) {
        if self.settle > 0 {
            self.settle -= 1;
            return;
        }
        self.reading = f.traces[self.channel]
            .as_ref()
            .and_then(|v| analyse(v, f.dt));
    }

    /// The gain correction that makes the amplitude 2 V, from the
    /// correction in use now. An error when it is too far off.
    pub fn new_gain(&self, now: f64) -> Result<f64, String> {
        let Some(r) = &self.reading else {
            return Err("no square wave measured yet: probe tip on the calibration pin?".into());
        };
        let g = now * CAL_AMPLITUDE / r.amplitude;
        if (GAIN_CORRECTION.0..=GAIN_CORRECTION.1).contains(&g) {
            Ok(g)
        } else {
            Err(format!(
                "the amplitude is {} ({:+.0} %): is the probe switch the same as the probe setting (p)?",
                fmt_si(r.amplitude, "V"),
                r.gain_error()
            ))
        }
    }

    /// The lines of the helper box: what it measured, the hint, the gain.
    pub fn lines(&self, gain: f64) -> Vec<String> {
        let ch = format!("CH{}", self.channel + 1);
        let probe = self.saved.channels[self.channel].probe;
        let mut out = vec![format!(
            "{ch} x{probe} on the calibration pin · {} square wave, 2 Vpp",
            fmt_si(self.freq as f64, "Hz")
        )];
        match &self.reading {
            None => {
                out.push(
                    "Measuring... (probe tip on the calibration pin, ground clip on its ground)"
                        .into(),
                );
                out.push(String::new());
            }
            Some(r) => {
                let word = if r.edge.abs() < FLAT {
                    "flat"
                } else if r.edge > 0.0 {
                    "overshoot"
                } else {
                    "undershoot"
                };
                out.push(format!("Edge {word} {:+.1} %. {}", r.edge, r.hint()));
                out.push(format!(
                    "Amplitude {} ({:+.2} % from 2 V)",
                    fmt_si(r.amplitude, "V"),
                    r.gain_error()
                ));
            }
        }
        out.push(if gain == 1.0 {
            format!("{ch} gain correction: none")
        } else {
            format!(
                "{ch} gain correction: x{gain:.4} ({:+.2} %)",
                (gain - 1.0) * 100.0
            )
        });
        out
    }
}

/// The key line of the helper box.
pub const PROBE_KEYS: &str =
    "Tab channel · ← → g G frequency · p probe x1/x10 · Enter save gain · Del clear gain · Esc close";

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2 Vpp square of `periods` periods with an exponential error
    /// after each edge of `edge` (fraction of the amplitude; positive
    /// overshoots), and a gain `k`.
    fn square(periods: usize, edge: f64, k: f64) -> Vec<f64> {
        let half = 500;
        (0..periods * 2 * half)
            .map(|i| {
                let up = (i / half) % 2 == 0;
                let t = (i % half) as f64 / half as f64;
                let e = edge * (-t * 8.0).exp();
                k * if up { 2.0 + 2.0 * e } else { -2.0 * e }
            })
            .collect()
    }

    #[test]
    fn edge_error_sign_and_size() {
        let dt = 1e-6;
        let flat = analyse(&square(4, 0.0, 1.0), dt).unwrap();
        assert!(flat.edge.abs() < 0.1, "{flat:?}");
        assert!((flat.amplitude - 2.0).abs() < 1e-6);
        assert!(flat.halves >= 6, "{flat:?}");
        assert!(flat.hint().contains("compensated"));
        assert!((flat.freq.unwrap() - 1000.0).abs() < 1.0);
        let over = analyse(&square(4, 0.1, 1.0), dt).unwrap();
        assert!(over.edge > 3.0, "{over:?}");
        assert!(over.hint().starts_with("Over-compensated"));
        let under = analyse(&square(4, -0.1, 1.0), dt).unwrap();
        assert!(under.edge < -3.0, "{under:?}");
        assert!(under.hint().contains("other way"));
        // Gain: 2 % low.
        let low = analyse(&square(4, 0.0, 0.98), dt).unwrap();
        assert!((low.gain_error() + 2.0).abs() < 0.01, "{low:?}");
        // No square wave.
        assert!(analyse(&vec![0.0; 1000], dt).is_none());
        assert!(analyse(&square(4, 0.0, 0.1), dt).is_none());
    }

    #[test]
    fn settings_fit_the_square_wave_and_gain_correction() {
        let mut s = Settings::default();
        s.channels[1].probe = 10;
        s.cal_freq = 50;
        let mut p = ProbeCal::new(s.clone(), true, 1);
        assert_eq!(p.freq, 1_000);
        let t = p.settings();
        assert!(!t.channels[0].enabled && t.channels[1].enabled);
        // 0.5 V/div at the tip: 50 mV/div at the BNC with x10.
        assert_eq!(t.channels[1].volts_per_div(), 0.5);
        assert_eq!(t.time_per_div(), 200e-6);
        assert_eq!((t.cal_freq, t.trigger.source), (1_000, 1));
        p.step_freq(false);
        assert_eq!(p.settings().cal_freq, 800);
        assert_eq!(p.settings().time_per_div(), 500e-6);
        for _ in 0..20 {
            p.step_freq(true);
        }
        assert_eq!(p.freq, 2_000);
        assert_eq!(p.settings().time_per_div(), 100e-6);
        // The gain from a reading: 1.96 V with x1.01 in use.
        assert!(p.new_gain(1.0).is_err());
        p.reading = analyse(&square(4, 0.0, 0.98), 1e-6);
        let g = p.new_gain(1.01).unwrap();
        assert!((g - 1.01 / 0.98).abs() < 1e-6, "{g}");
        // A x10 probe set to x1: far off.
        p.reading = analyse(&square(4, 0.0, 0.1 * 4.0), 1e-6);
        assert!(p.new_gain(1.0).unwrap_err().contains("probe switch"));
        let lines = p.lines(1.0);
        assert!(lines[0].starts_with("CH2 x10"), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("Amplitude")), "{lines:?}");
    }
}
