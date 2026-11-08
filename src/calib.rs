//! The app's own calibration helpers. Their values go into the config
//! file (`[calibration]`), never into the scope's EEPROM.
//!
//! Zero calibration: with the probes grounded, the app steps through
//! every gain and both ADC rate banks (below 30 MS/s with both channels,
//! and CH1 alone at 30 MS/s), and measures the ADC count of 0 V of each.

use std::time::{Duration, Instant};

use crate::acquire::Block;
use crate::device::protocol::{GAIN_STEPS, GAIN_VDIV, MAX_ADC_DEFAULT};
use crate::scope::time_index;
use crate::settings::{CalibrationSettings, Coupling, Settings, TriggerMode, VOLT_DIVS};
use crate::si::fmt_si;

/// Blocks thrown away after each change (the input settles).
const SKIP_BLOCKS: usize = 1;
/// Blocks averaged per step.
const MEASURE_BLOCKS: usize = 3;
/// Most noise (standard deviation, in ADC counts) a grounded input has.
const NOISE_MAX: f64 = 2.5;
/// Farthest a zero may be from the middle of the ADC (128), in counts.
const ZERO_FAR: f64 = 48.0;
/// A step with no block for this long fails.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(5);

/// One step of the zero calibration: a gain, and the ADC rate bank.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZeroStep {
    pub gain: usize,
    /// CH1 alone at 30 MS/s (the high-speed bank); else both channels at
    /// 16 MS/s.
    pub fast: bool,
}

/// What a block did to the zero calibration.
#[derive(Clone, Debug, PartialEq)]
pub enum ZeroOutcome {
    /// Not done with this step yet (or the block was for other settings).
    Busy,
    /// The next step starts: use [`ZeroCal::settings`].
    Next,
    /// All steps done: the new calibration.
    Done(CalibrationSettings),
    Failed(String),
}

/// The running zero calibration.
#[derive(Clone, Debug)]
pub struct ZeroCal {
    /// The user's settings, put back at the end.
    pub saved: Settings,
    /// The user's run / stop.
    pub was_running: bool,
    steps: Vec<ZeroStep>,
    at: usize,
    blocks: usize,
    sum: [f64; 2],
    n: [usize; 2],
    result: CalibrationSettings,
    /// Biggest change against the zero in use: counts and where.
    pub biggest: (f64, String),
    since: Instant,
}

impl ZeroCal {
    pub fn new(saved: Settings, was_running: bool) -> Self {
        let steps = [false, true]
            .iter()
            .flat_map(|&fast| (0..GAIN_STEPS).map(move |gain| ZeroStep { gain, fast }))
            .collect();
        let mut result = saved.calibration.clone();
        result.clear_zero();
        Self {
            saved,
            was_running,
            steps,
            at: 0,
            blocks: 0,
            sum: [0.0; 2],
            n: [0; 2],
            result,
            biggest: (0.0, String::new()),
            since: Instant::now(),
        }
    }

    pub fn step(&self) -> ZeroStep {
        self.steps[self.at.min(self.steps.len() - 1)]
    }

    /// The scope settings of the step now: the gain on CH1 (and CH2),
    /// the ADC at 16 MS/s with both channels or CH1 alone at 30 MS/s,
    /// blocks at 1 ms/div, no math, no averaging.
    pub fn settings(&self) -> Settings {
        let st = self.step();
        let mut s = self.saved.clone();
        let idx = VOLT_DIVS
            .iter()
            .position(|v| (v - GAIN_VDIV[st.gain]).abs() < 1e-9)
            .unwrap_or(0);
        for (c, ch) in s.channels.iter_mut().enumerate() {
            ch.enabled = c == 0 || !st.fast;
            ch.volt_idx = idx;
            ch.volts_fine = 1.0;
            ch.coupling = Coupling::Dc;
        }
        s.math.enabled = false;
        s.display.xy = false;
        s.display.averaging = 1;
        s.trigger.source = 0;
        s.trigger.mode = TriggerMode::Auto;
        s.time_idx = time_index(1e-3);
        s.time_fine = 1.0;
        s.acquisition.max_adc_rate = MAX_ADC_DEFAULT;
        s
    }

    /// Text for the message line, like
    /// `Zero calibration 3/16: CH1 + CH2, 100 mV/div, 16 MS/s (Esc stops)`.
    pub fn progress(&self) -> String {
        let st = self.step();
        format!(
            "Zero calibration {}/{}: {}, {}/div at the BNC (Esc stops)",
            self.at + 1,
            self.steps.len(),
            if st.fast {
                "CH1 alone, 30 MS/s"
            } else {
                "CH1 + CH2, 16 MS/s"
            },
            fmt_si(GAIN_VDIV[st.gain], "V")
        )
    }

    /// True when the step got no block for too long.
    pub fn timed_out(&self) -> bool {
        self.since.elapsed() > STEP_TIMEOUT
    }

    /// Take a block of the current settings.
    pub fn block(&mut self, b: &Block) -> ZeroOutcome {
        let st = self.step();
        let chans = if st.fast { 1 } else { 2 };
        let fits = b.cfg.high_speed() == st.fast
            && b.cfg.channels as usize == chans
            && b.cfg.gains[..chans].iter().all(|g| *g == st.gain)
            && b.data[..chans].iter().all(|d| d.len() > 10);
        if !fits {
            return ZeroOutcome::Busy;
        }
        self.since = Instant::now();
        self.blocks += 1;
        if self.blocks <= SKIP_BLOCKS {
            return ZeroOutcome::Busy;
        }
        let names = ["CH1", "CH2"];
        for (c, name) in names.iter().enumerate().take(chans) {
            let (lo, hi) = b.range[c];
            if hi <= lo {
                return ZeroOutcome::Failed("no calibration for the input range".into());
            }
            if b.clipped[c] {
                return ZeroOutcome::Failed(format!(
                    "{} is out of range: ground the probe tip (or the BNC input) and try again",
                    name
                ));
            }
            // Volts at the BNC to ADC counts: the range is counts 0..255.
            let k = 255.0 / (hi - lo);
            let d = &b.data[c];
            let mean = d.iter().sum::<f64>() / d.len() as f64;
            let var = d.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / d.len() as f64;
            let noise = var.sqrt() * k;
            let count = (mean - lo) * k;
            if noise > NOISE_MAX {
                return ZeroOutcome::Failed(format!(
                    "{} has a signal ({noise:.1} counts of noise): ground the probe tip and try again",
                    name
                ));
            }
            if (count - 128.0).abs() > ZERO_FAR {
                return ZeroOutcome::Failed(format!(
                    "{} reads {count:.0} counts, far from the middle: is the input grounded?",
                    name
                ));
            }
            // The zero in use now is at 0 V.
            let change = count - (0.0 - lo) * k;
            if change.abs() > self.biggest.0.abs() {
                self.biggest = (
                    change,
                    format!(
                        "{} {}/div{}",
                        name,
                        fmt_si(GAIN_VDIV[st.gain], "V"),
                        if st.fast { " at 30 MS/s" } else { "" }
                    ),
                );
            }
            self.sum[c] += count;
            self.n[c] += 1;
        }
        if self.blocks < SKIP_BLOCKS + MEASURE_BLOCKS {
            return ZeroOutcome::Busy;
        }
        let zero = |c: usize| (self.sum[c] / self.n[c].max(1) as f64 * 1000.0).round() / 1000.0;
        let r = &mut self.result;
        if st.fast {
            r.zero_ch1_30m[st.gain] = zero(0);
        } else {
            r.zero_ch1[st.gain] = zero(0);
            r.zero_ch2[st.gain] = zero(1);
        }
        self.at += 1;
        self.blocks = 0;
        self.sum = [0.0; 2];
        self.n = [0; 2];
        if self.at >= self.steps.len() {
            ZeroOutcome::Done(self.result.clone())
        } else {
            ZeroOutcome::Next
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::protocol::{choose_rate, Calibration, HwConfig};

    /// A block of `step` with flat inputs at ADC counts `zero`, read with
    /// the calibration `cal`, plus a little noise.
    fn block(st: ZeroStep, zero: [f64; 2], cal: &Calibration) -> Block {
        let chans = if st.fast { 1 } else { 2 };
        let cfg = HwConfig {
            rate: choose_rate(10e-3, chans, MAX_ADC_DEFAULT),
            channels: chans,
            gains: [st.gain; 2],
            dc: [true; 2],
            cal_freq: 1000,
            roll: false,
        };
        let hs = cfg.high_speed();
        let data = [0, 1].map(|c| {
            if c >= chans as usize {
                return Vec::new();
            }
            (0..1000)
                .map(|i| cal.volts(zero[c] + [-0.3, 0.3][i % 2], st.gain, c, hs))
                .collect()
        });
        Block {
            gen: 1,
            cfg,
            data,
            min: Default::default(),
            max: Default::default(),
            clipped: [false; 2],
            range: [0, 1].map(|c| cal.range(st.gain, c, hs)),
            rate: cfg.rate.rate,
            roll: false,
            trigger: None,
        }
    }

    #[test]
    fn steps_every_gain_and_bank() {
        let mut s = Settings::default();
        s.channels[1].enabled = false;
        s.calibration.gain = [1.1, 1.0];
        let mut z = ZeroCal::new(s.clone(), true);
        let mut cal = Calibration::default();
        // The 30 MS/s bank reads 3.5 counts high on this scope.
        cal.offset[1] = [[132.0, 129.0]; GAIN_STEPS];
        cal.offset[0] = [[132.0, 129.0]; GAIN_STEPS];
        let true_zero = |st: ZeroStep| {
            [
                132.0 + if st.fast { 3.5 } else { 0.0 },
                129.0 + st.gain as f64 * 0.1,
            ]
        };
        let mut steps = 0;
        let result = loop {
            let st = z.step();
            let set = z.settings();
            assert_eq!(set.channels[1].enabled, !st.fast);
            assert_eq!(VOLT_DIVS[set.channels[0].volt_idx], GAIN_VDIV[st.gain]);
            assert!(
                z.progress().contains(&format!("{}/16", steps + 1)),
                "{}",
                z.progress()
            );
            // A block of other settings does not count.
            let other = ZeroStep {
                gain: (st.gain + 1) % GAIN_STEPS,
                fast: st.fast,
            };
            assert_eq!(
                z.block(&block(other, true_zero(other), &cal)),
                ZeroOutcome::Busy
            );
            let out = loop {
                match z.block(&block(st, true_zero(st), &cal)) {
                    ZeroOutcome::Busy => continue,
                    o => break o,
                }
            };
            steps += 1;
            match out {
                ZeroOutcome::Next => continue,
                ZeroOutcome::Done(r) => break r,
                o => panic!("{o:?}"),
            }
        };
        assert_eq!(steps, 16);
        for g in 0..GAIN_STEPS {
            assert!((result.zero_ch1[g] - 132.0).abs() < 1e-6);
            assert!((result.zero_ch1_30m[g] - 135.5).abs() < 1e-6);
            assert!((result.zero_ch2[g] - (129.0 + g as f64 * 0.1)).abs() < 1e-6);
        }
        // The gain correction stays; the user's settings are kept.
        assert_eq!(result.gain, [1.1, 1.0]);
        assert_eq!(z.saved, s);
        assert!((z.biggest.0 - 3.5).abs() < 1e-6, "{:?}", z.biggest);
        assert!(z.biggest.1.contains("30 MS/s"), "{:?}", z.biggest);
    }

    #[test]
    fn a_signal_or_an_open_input_fails() {
        let mut z = ZeroCal::new(Settings::default(), true);
        let cal = Calibration::default();
        let st = z.step();
        let mut b = block(st, [128.0, 128.0], &cal);
        assert_eq!(z.block(&b), ZeroOutcome::Busy);
        // A 1 V square on CH2.
        for (i, v) in b.data[1].iter_mut().enumerate() {
            *v = if i % 100 < 50 { 0.05 } else { 0.0 };
        }
        match z.block(&b) {
            ZeroOutcome::Failed(m) => assert!(m.contains("CH2 has a signal"), "{m}"),
            o => panic!("{o:?}"),
        }
        let mut z = ZeroCal::new(Settings::default(), true);
        let b = block(st, [128.0, 128.0], &cal);
        let mut c = b.clone();
        c.clipped[0] = true;
        z.block(&b);
        assert!(matches!(z.block(&c), ZeroOutcome::Failed(m) if m.contains("CH1 is out of range")));
    }
}
