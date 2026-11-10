//! Scope logic: turns sample blocks into the frame on screen. Pure code,
//! so it is easy to test: no terminal and no USB here.

use std::collections::VecDeque;
use std::ops::Range;

use crate::acquire::Block;
use crate::device::protocol::{choose_rate, gain_index, hw_channels, nominal_range, HwConfig};
use crate::dsp::measure::{measure, Measurements};
use crate::dsp::trigger::find_trigger;
use crate::settings::*;
use crate::stream::pipeline::{self, Averaging, ProcSettings, Published, Roll, MARGIN};

/// Index of the math trace in [`Frame::traces`].
pub const MATH: usize = 2;

/// What is on the screen now.
#[derive(Clone, Debug, Default)]
pub struct Frame {
    /// CH1, CH2 and MATH, at the probe tip. `None` when not available.
    /// Each sample is the mean of the raw samples behind it (and of the
    /// averaged captures).
    pub traces: [Option<Vec<f64>>; 3],
    /// The peak band of each trace: the lowest and highest raw sample
    /// behind each sample, over the averaged captures too. The same
    /// length as the trace.
    pub peak: [Option<(Vec<f64>, Vec<f64>)>; 3],
    /// Time between samples, in seconds.
    pub dt: f64,
    /// Time of sample 0, relative to the trigger point, in seconds.
    pub t0: f64,
    pub triggered: bool,
    pub roll: bool,
    pub clipped: [bool; 2],
    /// Output samples per second.
    pub rate: f64,
    /// Measurements of CH1, CH2 and MATH over a longer record than the
    /// screen (from the stream's ring), when the stream made them. They
    /// see whole periods the screen may not hold.
    pub long: Option<[Option<Measurements>; 3]>,
}

impl Frame {
    /// Screen position (in divisions, -5 .. 5) of the trigger point.
    pub fn x_ref(&self, s: &Settings) -> f64 {
        if self.roll {
            H_DIVS / 2.0
        } else {
            -H_DIVS / 2.0 + s.trigger.position * H_DIVS
        }
    }

    /// Screen x (in divisions) of sample `i`.
    pub fn x(&self, i: f64, s: &Settings) -> f64 {
        self.x_ref(s) + (self.t0 + i * self.dt) / s.time_per_div()
    }

    /// Time (relative to the trigger) at screen x.
    pub fn time_at(&self, x: f64, s: &Settings) -> f64 {
        (x - self.x_ref(s)) * s.time_per_div()
    }

    /// Time shift of trace `t` on screen, in seconds: the channel's
    /// deskew. The math trace is not moved.
    pub fn deskew(t: usize, s: &Settings) -> f64 {
        if t < MATH {
            s.channels[t].deskew
        } else {
            0.0
        }
    }

    /// Screen x (in divisions) where trace `t` draws its sample `i`: the
    /// sample's own x, moved by the trace's deskew.
    pub fn trace_x(&self, t: usize, i: f64, s: &Settings) -> f64 {
        self.x(i, s) + Self::deskew(t, s) / s.time_per_div()
    }

    /// Indexes of the samples inside the screen.
    pub fn visible(&self, s: &Settings, len: usize) -> Range<usize> {
        self.visible_shifted(s, len, 0.0)
    }

    /// Indexes of the samples that trace `t` draws inside the screen,
    /// with its deskew. Measurements use [`Frame::visible`]: real time.
    pub fn trace_visible(&self, t: usize, s: &Settings, len: usize) -> Range<usize> {
        self.visible_shifted(s, len, Self::deskew(t, s))
    }

    /// Indexes of the samples inside the screen when they are drawn
    /// `shift` seconds later.
    fn visible_shifted(&self, s: &Settings, len: usize, shift: f64) -> Range<usize> {
        if self.dt <= 0.0 || len == 0 {
            return 0..0;
        }
        let half = H_DIVS / 2.0;
        let tdiv = s.time_per_div();
        // x(i) = x_ref + (t0 + shift + i dt) / tdiv
        //   =>  i = ((x - x_ref) tdiv - t0 - shift) / dt
        let idx = |x: f64| ((x - self.x_ref(s)) * tdiv - self.t0 - shift) / self.dt;
        let a = idx(-half).ceil().max(0.0) as usize;
        let b = (idx(half).floor() + 1.0).max(0.0) as usize;
        a.min(len)..b.min(len)
    }

    /// The samples of trace `t` on screen.
    pub fn visible_samples(&self, t: usize, s: &Settings) -> Option<&[f64]> {
        let tr = self.traces[t].as_ref()?;
        let r = self.visible(s, tr.len());
        Some(&tr[r])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrigState {
    /// Running and triggered.
    Triggered,
    /// Running, no trigger found: showing an untriggered trace.
    Auto,
    /// Normal or single mode, waiting for a trigger.
    Waiting,
    /// Roll mode: no trigger.
    Roll,
    Stopped,
}

impl TrigState {
    pub fn name(self) -> &'static str {
        match self {
            TrigState::Triggered => "Trig'd",
            TrigState::Auto => "Auto",
            TrigState::Waiting => "Wait",
            TrigState::Roll => "Roll",
            TrigState::Stopped => "Stop",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum AutosetStep {
    /// Look at 1 ms/div with the biggest range.
    Fast,
    /// Nothing found: look slower for low frequencies.
    Slow,
    /// Measure again at the volts/div and time/div the coarse pass picked:
    /// the biggest range is coarse and its zero can be off by several ADC
    /// steps, so the offset and trigger level come from this pass. `n`:
    /// refine passes done; `freq`: the frequency found so far.
    Refine { n: u8, freq: Option<f64> },
}

/// Refine passes at most: a volts/div that keeps changing still ends.
const AUTOSET_REFINES: u8 = 3;

pub struct Scope {
    pub settings: Settings,
    pub running: bool,
    pub frame: Option<Frame>,
    /// Old frames for persistence, newest last.
    pub history: VecDeque<Frame>,
    pub state: TrigState,
    /// Generation of the hardware settings now in use.
    pub gen: u64,
    /// Generation of the processing settings now in use.
    pub pgen: u64,
    /// The processing settings last sent.
    proc: Option<ProcSettings>,
    /// Counts the arms of the single trigger.
    arm: u64,
    pub hw: Option<HwConfig>,
    /// The average and peak band over the last triggers.
    avg: Averaging,
    /// Roll mode: the newest screen of the stream.
    roll: Roll,
    last_block: Option<Block>,
    autoset: Option<AutosetStep>,
    /// Notices for the message line (autoset results and such).
    pub notices: Vec<String>,
    /// Input range at the BNC per channel, from the last block, with the
    /// gain index it was measured at.
    adc_range: [Option<(usize, (f64, f64))>; 2],
    /// Mean that AC coupling took away, per channel, in the last block.
    ac_mean: [f64; 2],
    /// Triggers in the average of the last frame.
    pub averaged: usize,
}

pub use crate::stream::pipeline::TRIGGER_INSIDE;

impl Scope {
    pub fn new(mut settings: Settings) -> Self {
        settings.sanitize();
        Self {
            settings,
            running: true,
            frame: None,
            history: VecDeque::new(),
            state: TrigState::Auto,
            gen: 0,
            pgen: 0,
            proc: None,
            arm: 0,
            hw: None,
            avg: Averaging::default(),
            roll: Roll::default(),
            last_block: None,
            autoset: None,
            notices: Vec::new(),
            adc_range: [None; 2],
            ac_mean: [0.0; 2],
            averaged: 0,
        }
    }

    /// Voltage range (at the probe tip, low to high) the trigger source
    /// can measure at its gain: the ADC range from the calibration, with
    /// probe, invert and AC coupling. Without a block yet, the nominal
    /// calibration.
    pub fn trigger_range(&self) -> (f64, f64) {
        let s = &self.settings;
        let src = s.trigger_source();
        let ch = &s.channels[src];
        let gain = gain_index(VOLT_DIVS[ch.volt_idx.min(VOLT_DIVS.len() - 1)]);
        let (lo, hi) = match self.adc_range[src] {
            Some((g, r)) if g == gain => r,
            _ => {
                let mut cfg = self.hw_config();
                cfg.gains[src] = gain;
                nominal_range(&cfg)[src]
            }
        };
        let mean = if ch.coupling == Coupling::Ac {
            self.ac_mean[src]
        } else {
            0.0
        };
        let k = ch.probe as f64 * if ch.invert { -1.0 } else { 1.0 };
        let (a, b) = ((lo - mean) * k, (hi - mean) * k);
        (a.min(b), a.max(b))
    }

    /// The trigger level the trigger search uses, when it is not the
    /// user's level: a level outside the input range is never crossed, so
    /// the search uses a level 5 % of the range inside the nearest edge.
    /// `None` when the user's level is inside the range.
    pub fn effective_trigger_level(&self) -> Option<f64> {
        let level = self.settings.trigger.level;
        let (lo, hi) = self.trigger_range();
        let inside = TRIGGER_INSIDE * (hi - lo);
        if level > hi {
            Some(hi - inside)
        } else if level < lo {
            Some(lo + inside)
        } else {
            None
        }
    }

    /// Hardware settings for the current scope settings.
    pub fn hw_config(&self) -> HwConfig {
        let s = &self.settings;
        let roll = s.roll();
        let need_ch2 = s.channels[1].enabled
            || s.math.enabled
            || s.display.xy
            || (!roll && s.trigger_source() == 1);
        let max = s.acquisition.max_adc_rate;
        let channels = hw_channels(need_ch2, max);
        let rate = choose_rate(s.coarse_screen_time(), channels, max);
        let gains = [0, 1].map(|c| gain_index(VOLT_DIVS[s.channels[c].volt_idx]));
        let dc = [0, 1].map(|c| s.channels[c].coupling == Coupling::Dc);
        HwConfig {
            rate,
            channels,
            gains,
            dc,
            cal_freq: s.cal_freq,
            roll,
        }
    }

    /// Call after any settings change. When the hardware must change, this
    /// makes a new generation and returns what to send.
    pub fn sync_hw(&mut self) -> Option<(u64, HwConfig)> {
        let cfg = self.hw_config();
        if self.hw == Some(cfg) {
            return None;
        }
        let roll_changed = self
            .hw
            .is_none_or(|h| h.roll != cfg.roll || h.rate != cfg.rate);
        self.gen += 1;
        self.hw = Some(cfg);
        self.reset_average();
        self.history.clear();
        if roll_changed {
            self.clear_roll();
        }
        Some((self.gen, cfg))
    }

    /// Call after any settings change. When the processing settings
    /// changed, this makes a new processing generation and returns what to
    /// send.
    pub fn sync_proc(&mut self) -> Option<(u64, ProcSettings)> {
        let p = ProcSettings::of(&self.settings, self.arm);
        if self.proc.as_ref() == Some(&p) {
            return None;
        }
        self.pgen += 1;
        self.proc = Some(p.clone());
        Some((self.pgen, p))
    }

    /// Take the newest frame of the stream processing. Returns true when
    /// the screen changed.
    pub fn take(&mut self, p: Published) -> bool {
        if p.gen != self.gen || p.pgen != self.pgen || !self.running {
            return false;
        }
        let hw = self.hw;
        for c in 0..2 {
            if let Some(h) = hw {
                self.adc_range[c] = Some((h.gains[c], p.range[c]));
            }
            self.ac_mean[c] = p.ac_mean[c];
        }
        if let Some(step) = self.autoset {
            if let Some(b) = &p.snapshot {
                self.autoset_step(step, b);
            }
            self.last_block = p.snapshot;
            return false;
        }
        if p.snapshot.is_some() {
            self.last_block = p.snapshot;
        }
        let new = p.frame.is_some();
        if let Some(f) = p.frame {
            if f.roll {
                self.frame = Some(f);
            } else {
                self.set_frame(f);
            }
        }
        self.state = p.state;
        self.averaged = p.averaged;
        if p.single_done {
            self.running = false;
            self.state = TrigState::Stopped;
        }
        new || p.single_done
    }

    /// Start the capture average (and its peak band) over.
    fn reset_average(&mut self) {
        self.avg.reset();
    }

    fn clear_roll(&mut self) {
        self.roll.clear();
    }

    /// Take a new block from the acquisition thread. Returns true when the
    /// screen changed.
    pub fn push(&mut self, block: Block) -> bool {
        if block.gen != self.gen || !self.running {
            return false;
        }
        if let Some(step) = self.autoset {
            self.autoset_step(step, &block);
            self.last_block = Some(block);
            return false;
        }
        let changed = self.process(&block);
        self.last_block = Some(block);
        changed
    }

    /// Show the last block again with new display settings (used while
    /// stopped).
    pub fn reprocess(&mut self) {
        if let Some(block) = self.last_block.take() {
            let running = self.running;
            let state = self.state;
            self.running = true;
            self.reset_average();
            if !block.roll {
                self.process(&block);
            }
            self.running = running;
            self.state = if running { self.state } else { state };
            self.last_block = Some(block);
        }
    }

    pub fn set_running(&mut self, run: bool) {
        self.running = run;
        if run {
            if self.state == TrigState::Stopped {
                self.state = TrigState::Waiting;
            }
        } else {
            self.state = TrigState::Stopped;
        }
    }

    /// Arm the single trigger: capture until the next trigger, then stop.
    pub fn arm_single(&mut self) {
        self.settings.trigger.mode = TriggerMode::Single;
        self.running = true;
        self.state = TrigState::Waiting;
        self.arm += 1;
    }

    /// Channel samples at the probe tip: probe, invert, AC coupling.
    fn condition(&self, block: &Block, c: usize) -> Option<Vec<f64>> {
        let data = &block.data[c];
        if data.is_empty() {
            return None;
        }
        let ch = &self.settings.channels[c];
        let k = ch.probe as f64 * if ch.invert { -1.0 } else { 1.0 };
        let mean = ac_mean(block, ch, c);
        Some(data.iter().map(|v| (v - mean) * k).collect())
    }

    /// The trigger in a block: a fractional sample index.
    fn find_trigger(&self, block: &Block, p: &ProcSettings) -> Option<f64> {
        let s = &self.settings;
        let (_, pre, post) = p.window(block.rate);
        let src = s.trigger_source();
        // The source channel, else the one that was sampled.
        let src_c = if block.data[src].is_empty() {
            1 - src
        } else {
            src
        };
        let v = self.condition(block, src_c)?;
        let vdiv = s.channels[src].volts_per_div();
        let level = self.effective_trigger_level().unwrap_or(s.trigger.level);
        let probe = s.channels[src_c].probe as f64;
        let (lo, hi) = block.range[src_c];
        let lsb = (hi - lo).abs() / 255.0 * probe;
        find_trigger(
            &v,
            level,
            s.trigger.slope,
            hysteresis(vdiv, &v, lsb),
            pre + MARGIN,
            v.len().saturating_sub(post + MARGIN),
        )
    }

    fn process(&mut self, block: &Block) -> bool {
        for c in 0..2 {
            if !block.data[c].is_empty() {
                self.adc_range[c] = Some((block.cfg.gains[c], block.range[c]));
                self.ac_mean[c] = ac_mean(block, &self.settings.channels[c], c);
            }
        }
        let p = ProcSettings::of(&self.settings, 0);
        if block.roll {
            self.roll.push(block, p.screen_time);
            self.frame = Some(self.roll.frame(&p));
            self.state = TrigState::Roll;
            return true;
        }
        // A block without a trigger: Auto shows it untriggered (the
        // stream processing waits for its timeout before it sends one).
        let trig = block.trigger.or_else(|| self.find_trigger(block, &p));
        if trig.is_none() && p.mode != TriggerMode::Auto {
            self.state = TrigState::Waiting;
            return false;
        }
        let ac = [0, 1].map(|c| ac_mean(block, &self.settings.channels[c], c));
        let cut = pipeline::cut(block, &p, ac, trig);
        let depth = p.averaging;
        let shown = if !cut.triggered {
            // An untriggered frame is not averaged and leaves the
            // average as it is.
            cut
        } else if depth > 1 {
            self.avg.add(&cut, depth, p.peak);
            self.avg.value(&cut, depth)
        } else {
            self.avg.reset();
            cut
        };
        let triggered = shown.triggered;
        let frame = pipeline::frame(
            &p,
            shown.traces,
            shown.peak,
            block.rate,
            shown.t0,
            triggered,
            false,
            block.clipped,
        );
        self.set_frame(frame);
        self.state = if triggered {
            TrigState::Triggered
        } else {
            TrigState::Auto
        };
        if triggered && self.settings.trigger.mode == TriggerMode::Single {
            self.running = false;
            self.state = TrigState::Stopped;
        }
        true
    }

    fn set_frame(&mut self, frame: Frame) {
        let depth = self.settings.display.persistence;
        if let Some(old) = self.frame.take() {
            if depth > 0 && !old.roll {
                self.history.push_back(old);
            }
        }
        while self.history.len() > depth {
            self.history.pop_front();
        }
        self.frame = Some(frame);
    }

    /// Measurements of trace `t` on screen. Timing values need a signal of
    /// at least a quarter division: smaller ones are mostly noise.
    pub fn measure(&self, t: usize) -> Option<Measurements> {
        let f = self.frame.as_ref()?;
        let long = f.long.as_ref().and_then(|l| l[t].clone());
        let mut m = match long {
            Some(m) => m,
            None => {
                let v = f.visible_samples(t, &self.settings)?;
                if v.len() < 2 {
                    return None;
                }
                measure(v, f.dt)
            }
        };
        let per_div = if t == MATH {
            self.settings.math.units_per_div()
        } else {
            self.settings.channels[t].volts_per_div()
        };
        if m.pk_pk < 0.25 * per_div {
            m.freq = None;
            m.period = None;
            m.duty = None;
            m.pos_width = None;
            m.neg_width = None;
            m.rise = None;
            m.fall = None;
        }
        Some(m)
    }

    /// Start autoset: find good volts/div, time/div and trigger settings.
    pub fn start_autoset(&mut self) {
        let s = &mut self.settings;
        for ch in &mut s.channels {
            ch.volt_idx = VOLT_DIVS.len() - 2; // 5 V/div: the biggest range
            ch.volts_fine = 1.0;
        }
        s.time_fine = 1.0;
        s.time_idx = time_index(1e-3);
        s.trigger.mode = TriggerMode::Auto;
        self.running = true;
        self.autoset = Some(AutosetStep::Fast);
        self.notices.push("Autoset...".into());
    }

    pub fn autoset_running(&self) -> bool {
        self.autoset.is_some()
    }

    fn autoset_step(&mut self, step: AutosetStep, block: &Block) {
        let dt = 1.0 / block.rate;
        let enabled: Vec<usize> = (0..2)
            .filter(|&c| self.settings.channels[c].enabled && !block.data[c].is_empty())
            .collect();
        let found: Vec<(usize, Measurements)> = enabled
            .iter()
            .filter_map(|&c| self.condition(block, c).map(|v| (c, measure(&v, dt))))
            .collect();
        // The channel with the biggest signal sets the time base and trigger.
        let main = found
            .iter()
            .max_by(|a, b| a.1.pk_pk.total_cmp(&b.1.pk_pk))
            .cloned();
        let freq = main.as_ref().and_then(|(_, m)| m.freq);
        if freq.is_none() && step == AutosetStep::Fast {
            // Maybe a low frequency: look at 50 ms/div.
            self.settings.time_idx = time_index(50e-3);
            self.autoset = Some(AutosetStep::Slow);
            return;
        }
        let n = found.len().max(1) as f64;
        // Each channel gets its own band of the screen.
        let band = V_DIVS / n;
        let mut volts_changed = false;
        for (i, (c, m)) in found.iter().enumerate() {
            let ch = &mut self.settings.channels[*c];
            let probe = ch.probe as f64;
            let pp = m.pk_pk.max(1e-3);
            let old = (ch.volt_idx, ch.volts_fine);
            ch.volt_idx = VOLT_DIVS
                .iter()
                .position(|v| pp <= v * probe * band * 0.8)
                .unwrap_or(VOLT_DIVS.len() - 1);
            volts_changed |= old != (ch.volt_idx, 1.0);
            let vdiv = ch.volts_per_div();
            let center = V_DIVS / 2.0 - band * (i as f64 + 0.5);
            ch.offset = (center - (m.max + m.min) / 2.0 / vdiv).clamp(-V_DIVS / 2.0, V_DIVS / 2.0);
        }
        if let Some((c, m)) = &main {
            let t = &mut self.settings.trigger;
            t.source = *c;
            t.level = (m.max + m.min) / 2.0;
            t.position = 0.5;
            t.slope = crate::dsp::trigger::Slope::Rising;
        }
        match step {
            AutosetStep::Refine { n, freq: f0 } => {
                // The time base stays: the coarse pass picked it.
                let freq = freq.or(f0);
                if volts_changed && n + 1 < AUTOSET_REFINES {
                    self.autoset = Some(AutosetStep::Refine { n: n + 1, freq });
                    return;
                }
                match freq {
                    Some(f) => self
                        .notices
                        .push(format!("Autoset: {}", crate::si::fmt_si(f, "Hz"))),
                    None => self
                        .notices
                        .push("Autoset: no periodic signal found".into()),
                }
            }
            _ => {
                // About 3 periods on the screen.
                self.settings.time_idx = time_index(freq.map_or(1e-3, |f| 3.0 / f / H_DIVS));
                self.autoset = Some(AutosetStep::Refine { n: 0, freq });
                return;
            }
        }
        self.autoset = None;
    }
}

/// Trigger hysteresis: 0.1 div, but at most a quarter of the signal's
/// pk-pk (at least 2 ADC steps of `lsb`), so a big volts/div still
/// triggers on a small signal.
fn hysteresis(vdiv: f64, samples: &[f64], lsb: f64) -> f64 {
    let (lo, hi) = samples
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &v| (a.min(v), b.max(v)));
    let pp = if hi >= lo { hi - lo } else { 0.0 };
    (0.1 * vdiv).min((0.25 * pp).max(2.0 * lsb))
}

/// The mean AC coupling takes away from channel `c` of a block (0 for DC
/// and in roll mode).
fn ac_mean(block: &Block, ch: &ChannelSettings, c: usize) -> f64 {
    let data = &block.data[c];
    if ch.coupling == Coupling::Ac && !block.roll && !data.is_empty() {
        data.iter().sum::<f64>() / data.len() as f64
    } else {
        0.0
    }
}

/// Index of the smallest time/div step that is at least `t`.
pub fn time_index(t: f64) -> usize {
    TIME_DIVS
        .iter()
        .position(|&v| v >= t * 0.999)
        .unwrap_or(TIME_DIVS.len() - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::math::MathMode;
    use crate::dsp::trigger::Slope;
    use crate::stream::pipeline::{shift, Shift};

    fn sine_block(gen: u64, rate: f64, freq: f64, amp: f64, phase: f64) -> Block {
        let n = 20_000;
        let w = |a: f64, ph: f64| -> Vec<f64> {
            (0..n)
                .map(|i| a * (std::f64::consts::TAU * freq * i as f64 / rate + ph).sin())
                .collect()
        };
        let mut cfg = Scope::new(Settings::default()).hw_config();
        cfg.rate.rate = rate;
        Block {
            gen,
            cfg,
            data: [w(amp, phase), w(amp / 2.0, phase)],
            min: Default::default(),
            max: Default::default(),
            clipped: [false; 2],
            range: nominal_range(&cfg),
            rate,
            roll: false,
            trigger: None,
        }
    }

    fn scope() -> Scope {
        let mut s = Scope::new(Settings::default());
        s.sync_hw();
        s
    }

    #[test]
    fn hw_config_picks_channels_and_gains() {
        let mut s = Scope::new(Settings::default());
        let cfg = s.hw_config();
        assert_eq!(cfg.channels, 2);
        assert_eq!(cfg.rate.rate, 1e6); // 1 ms/div
        assert_eq!(cfg.gains, [5, 5]); // 1 V/div
        s.settings.channels[1].enabled = false;
        assert_eq!(s.hw_config().channels, 1);
        s.settings.math.enabled = true;
        assert_eq!(s.hw_config().channels, 2);
        s.settings.math.enabled = false;
        s.settings.trigger.source = 1;
        // Source CH2 while CH2 is off: the trigger follows CH1, so CH1
        // alone still gets 1-channel mode.
        assert_eq!(s.settings.trigger_source(), 0);
        assert_eq!(s.hw_config().channels, 1);
        // Probe x10 at 10 V/div is 1 V/div at the BNC.
        s.settings.channels[0].probe = 10;
        assert_eq!(s.settings.channels[0].volts_per_div(), 10.0);
        assert_eq!(s.hw_config().gains[0], 5);
        // Slow time bases are roll mode; CH1 alone streams at 30 MS/s too.
        s.settings.trigger.source = 0;
        s.settings.time_idx = time_index(1.0);
        let cfg = s.hw_config();
        assert!(cfg.roll);
        assert_eq!((cfg.channels, cfg.rate.raw, cfg.rate.rate), (1, 30e6, 1e3));
        // The max ADC rate caps every mode; below 1 MS/s with both channels.
        s.settings.acquisition.max_adc_rate = 500e3;
        let cfg = s.hw_config();
        assert_eq!((cfg.channels, cfg.rate.raw, cfg.rate.id), (2, 500e3, 150));
        s.settings.time_idx = time_index(1e-6);
        assert_eq!(s.hw_config().rate.rate, 500e3);
    }

    #[test]
    fn sync_hw_only_on_change() {
        let mut s = scope();
        assert_eq!(s.gen, 1);
        assert!(s.sync_hw().is_none());
        s.settings.channels[0].offset = 1.0; // display only
        assert!(s.sync_hw().is_none());
        s.settings.time_idx += 1;
        let (gen, _) = s.sync_hw().unwrap();
        assert_eq!(gen, 2);
    }

    #[test]
    fn trigger_puts_the_edge_at_the_trigger_position() {
        let mut s = scope();
        // 1 ms/div at 1 MS/s: 10000 samples on screen.
        let changed = s.push(sine_block(1, 1e6, 1000.0, 2.0, 1.0));
        assert!(changed);
        assert_eq!(s.state, TrigState::Triggered);
        let f = s.frame.as_ref().unwrap();
        // Sample at the trigger point is at the level (0 V), rising.
        let tr = f.traces[0].as_ref().unwrap();
        let i = -f.t0 / f.dt;
        let v = tr[i.floor() as usize];
        let v2 = tr[i.ceil() as usize];
        assert!(v <= 0.0 && v2 >= 0.0, "{v} {v2}");
        // The trigger point is at screen center.
        assert!((f.x(i, &s.settings)).abs() < 1e-9);
        // The visible part is one screen.
        let r = f.visible(&s.settings, tr.len());
        assert!((r.len() as i64 - 10_001).abs() <= 1, "{r:?}");
    }

    #[test]
    fn deskew_moves_only_the_drawing_of_its_channel() {
        let mut s = scope();
        s.push(sine_block(1, 1e6, 1000.0, 2.0, 1.0));
        let f = s.frame.clone().unwrap();
        let len = f.traces[0].as_ref().unwrap().len();
        let i = -f.t0 / f.dt;
        let r0 = f.trace_visible(0, &s.settings, len);
        // 1 ms/div: +2 ms moves CH1 two divisions right.
        s.settings.channels[0].deskew = 2e-3;
        let st = &s.settings;
        assert!((f.trace_x(0, i, st) - 2.0).abs() < 1e-9);
        assert!(f.trace_x(1, i, st).abs() < 1e-9);
        assert!(f.trace_x(MATH, i, st).abs() < 1e-9);
        // The drawn range ends 2000 samples (2 ms at 1 MS/s) earlier. It
        // starts at the record's first sample: the screen's left part
        // has no samples and stays empty.
        let r = f.trace_visible(0, st, len);
        assert!((r0.end as i64 - r.end as i64 - 2000).abs() <= 1, "{r0:?} {r:?}");
        assert_eq!(r.start, 0);
        assert!(f.trace_x(0, 0.0, st) > -H_DIVS / 2.0 + 1.5);
        // Measurements keep the real time: the same range as before.
        assert_eq!(f.visible(st, len), r0);
        assert_eq!(f.trace_visible(1, st, len), r0);
        // A shift past the record leaves nothing to draw.
        s.settings.channels[0].deskew = 1.0;
        assert!(f.trace_visible(0, &s.settings, len).is_empty());
        s.settings.channels[0].deskew = -1.0;
        assert!(f.trace_visible(0, &s.settings, len).is_empty());
    }

    #[test]
    fn falling_slope_and_position() {
        let mut s = scope();
        s.settings.trigger.slope = Slope::Falling;
        s.settings.trigger.position = 0.1;
        s.settings.trigger.level = 0.5;
        s.push(sine_block(1, 1e6, 1000.0, 2.0, 0.0));
        let f = s.frame.as_ref().unwrap();
        let tr = f.traces[0].as_ref().unwrap();
        let i = (-f.t0 / f.dt).floor() as usize;
        assert!(tr[i] >= 0.5 && tr[i + 1] <= 0.5);
        assert!((f.x(-f.t0 / f.dt, &s.settings) + 4.0).abs() < 1e-9);
    }

    #[test]
    fn normal_mode_waits_and_auto_mode_free_runs() {
        let mut s = scope();
        s.settings.trigger.level = 5.0; // above the signal
        s.settings.trigger.mode = TriggerMode::Normal;
        assert!(!s.push(sine_block(1, 1e6, 1000.0, 1.0, 0.0)));
        assert_eq!(s.state, TrigState::Waiting);
        assert!(s.frame.is_none());
        s.settings.trigger.mode = TriggerMode::Auto;
        assert!(s.push(sine_block(1, 1e6, 1000.0, 1.0, 0.0)));
        assert_eq!(s.state, TrigState::Auto);
        assert!(!s.frame.as_ref().unwrap().triggered);
    }

    #[test]
    fn single_stops_after_one_trigger() {
        let mut s = scope();
        s.arm_single();
        assert!(s.push(sine_block(1, 1e6, 1000.0, 1.0, 0.0)));
        assert!(!s.running);
        assert_eq!(s.state, TrigState::Stopped);
        // Further blocks are ignored while stopped.
        assert!(!s.push(sine_block(1, 1e6, 1000.0, 1.0, 0.0)));
    }

    #[test]
    fn stale_generation_is_dropped() {
        let mut s = scope();
        assert!(!s.push(sine_block(0, 1e6, 1000.0, 1.0, 0.0)));
        assert!(s.frame.is_none());
    }

    #[test]
    fn probe_invert_and_ac() {
        let mut s = scope();
        s.settings.channels[0].probe = 10;
        s.settings.channels[1].invert = true;
        s.settings.channels[1].coupling = Coupling::Ac;
        let mut b = sine_block(1, 1e6, 1000.0, 1.0, 0.0);
        for v in &mut b.data[1] {
            *v += 3.0; // DC part that AC coupling removes
        }
        s.push(b);
        let m1 = s.measure(0).unwrap();
        assert!((m1.pk_pk - 20.0).abs() < 0.05, "{}", m1.pk_pk);
        let m2 = s.measure(1).unwrap();
        assert!(m2.mean.abs() < 0.01, "{}", m2.mean);
        assert!((m2.pk_pk - 1.0).abs() < 0.01);
    }

    #[test]
    fn math_channel() {
        let mut s = scope();
        s.settings.math.enabled = true;
        s.settings.math.mode = MathMode::Sub;
        s.sync_hw();
        s.push(sine_block(s.gen, 1e6, 1000.0, 2.0, 0.0));
        let f = s.frame.as_ref().unwrap();
        let (a, b, m) = (
            f.traces[0].as_ref().unwrap(),
            f.traces[1].as_ref().unwrap(),
            f.traces[MATH].as_ref().unwrap(),
        );
        for i in (0..m.len()).step_by(997) {
            assert!((m[i] - (a[i] - b[i])).abs() < 1e-12);
        }
    }

    /// RMS error of CH2 against the clean sine (amplitude 0.5).
    fn ch2_error(s: &Scope) -> f64 {
        let f = s.frame.as_ref().unwrap();
        let tr = f.traces[1].as_ref().unwrap();
        let sum: f64 = tr
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let t = f.t0 + i as f64 * f.dt;
                (v - 0.5 * (std::f64::consts::TAU * 1000.0 * t).sin()).powi(2)
            })
            .sum();
        (sum / tr.len() as f64).sqrt()
    }

    #[test]
    fn averaging_lowers_noise() {
        // CH1 is clean and triggers; CH2 has noise of +-0.2 V.
        let mut rng = 1u64;
        let mut noisy = |gen| {
            let mut b = sine_block(gen, 1e6, 1000.0, 1.0, 0.0);
            for v in &mut b.data[1] {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                *v += ((rng >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * 0.4;
            }
            b
        };
        let mut s = scope();
        s.push(noisy(1));
        let single = ch2_error(&s);
        s.settings.display.averaging = 16;
        for _ in 0..16 {
            s.push(noisy(1));
        }
        let averaged = ch2_error(&s);
        // Uniform noise of +-0.2 V has 0.115 V RMS; 16 averages make it 4x less.
        assert!(single > 0.1, "single {single}");
        assert!(averaged < 0.04, "averaged {averaged}");
    }

    /// A block whose CH1 is 0 V on average with raw extremes of ±(1 +
    /// `k`) V, CH2 the sine.
    fn peaky_block(gen: u64, k: f64) -> Block {
        let mut b = sine_block(gen, 1e6, 1000.0, 1.0, 0.0);
        let n = b.data[0].len();
        b.min = [
            b.data[0].iter().map(|v| v - 1.0 - k).collect(),
            b.data[1].clone(),
        ];
        b.max = [
            b.data[0].iter().map(|v| v + 1.0 + k).collect(),
            b.data[1].clone(),
        ];
        assert_eq!(b.min[0].len(), n);
        b
    }

    #[test]
    fn peak_band_from_the_raw_extremes() {
        let mut s = scope();
        s.push(peaky_block(1, 0.0));
        let f = s.frame.as_ref().unwrap();
        let tr = f.traces[0].as_ref().unwrap();
        let (lo, hi) = f.peak[0].as_ref().unwrap();
        assert_eq!((lo.len(), hi.len()), (tr.len(), tr.len()));
        for i in 0..tr.len() {
            assert!((hi[i] - tr[i] - 1.0).abs() < 1e-9 && (tr[i] - lo[i] - 1.0).abs() < 1e-9);
        }
        // Without min and max (one raw sample per point) the band is the
        // trace.
        let (lo2, hi2) = f.peak[1].as_ref().unwrap();
        assert_eq!(lo2, f.traces[1].as_ref().unwrap());
        assert_eq!(hi2, f.traces[1].as_ref().unwrap());
        // Invert and probe: high and low swap and scale.
        s.settings.channels[0].invert = true;
        s.settings.channels[0].probe = 10;
        s.push(peaky_block(1, 0.0));
        let f = s.frame.as_ref().unwrap();
        let tr = f.traces[0].as_ref().unwrap();
        let (lo, hi) = f.peak[0].as_ref().unwrap();
        assert!((hi[100] - tr[100] - 10.0).abs() < 1e-9, "{} {}", hi[100], tr[100]);
        assert!((tr[100] - lo[100] - 10.0).abs() < 1e-9);
        // The math band holds every value: CH1 - CH2.
        s.settings.channels[0].invert = false;
        s.settings.channels[0].probe = 1;
        s.settings.math.enabled = true;
        s.settings.math.mode = MathMode::Sub;
        s.sync_hw();
        s.push(peaky_block(s.gen, 0.0));
        let f = s.frame.as_ref().unwrap();
        let (lo, hi) = f.peak[MATH].as_ref().unwrap();
        let m = f.traces[MATH].as_ref().unwrap();
        assert!((hi[50] - m[50] - 1.0).abs() < 1e-9 && (m[50] - lo[50] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn averaged_peak_keeps_the_extremes_of_the_last_captures() {
        let mut s = scope();
        s.settings.display.averaging = 4;
        // The band is averaged only when it is drawn.
        s.settings.display.trace_mode = TraceMode::Peak;
        // One capture with a big spike, then small ones.
        s.push(peaky_block(1, 2.0));
        for _ in 0..3 {
            s.push(peaky_block(1, 0.0));
        }
        let band = |s: &Scope| {
            let f = s.frame.as_ref().unwrap();
            let (lo, hi) = f.peak[0].as_ref().unwrap();
            let tr = f.traces[0].as_ref().unwrap();
            (hi[200] - tr[200], tr[200] - lo[200])
        };
        // The spike capture is one of the last 4: its ±3 V stay, not
        // averaged down. (Aligning the captures to the trigger widens
        // the band by up to one sample of slope: under 0.01 V here.)
        let (up, down) = band(&s);
        assert!((up - 3.0).abs() < 0.01 && (down - 3.0).abs() < 0.01, "{up} {down}");
        // A fifth capture: the spike drops out of the window.
        s.push(peaky_block(1, 0.0));
        let (up, _) = band(&s);
        assert!((up - 1.0).abs() < 0.01, "{up}");
        // A settings change resets it with the average.
        s.push(peaky_block(1, 2.0));
        s.settings.time_idx -= 1;
        let (gen, _) = s.sync_hw().unwrap();
        let mut b = peaky_block(gen, 0.0);
        b.cfg = s.hw.unwrap();
        s.push(b);
        let (up, _) = band(&s);
        assert!((up - 1.0).abs() < 0.01, "{up}");
    }

    #[test]
    fn roll_mode_keeps_the_peak_band() {
        let mut s = Scope::new(Settings::default());
        s.settings.time_idx = time_index(500e-3);
        let (gen, cfg) = s.sync_hw().unwrap();
        let piece = Block {
            gen,
            cfg,
            data: [vec![1.0; 100], vec![0.0; 100]],
            min: [vec![0.5; 100], vec![0.0; 100]],
            max: [vec![2.0; 100], vec![0.0; 100]],
            clipped: [false; 2],
            range: nominal_range(&cfg),
            rate: cfg.rate.rate,
            roll: true,
            trigger: None,
        };
        s.push(piece);
        let f = s.frame.as_ref().unwrap();
        let (lo, hi) = f.peak[0].as_ref().unwrap();
        assert_eq!((lo.len(), lo[0], hi[99]), (100, 0.5, 2.0));
    }

    #[test]
    fn persistence_keeps_old_frames() {
        let mut s = scope();
        s.settings.display.persistence = 3;
        for _ in 0..6 {
            s.push(sine_block(1, 1e6, 1000.0, 1.0, 0.0));
        }
        assert_eq!(s.history.len(), 3);
    }

    /// A sine of `amp` volts cut at the input range of 20 mV/div, like
    /// the ADC does, with that range in the block.
    fn clipped_block(gen: u64, cfg: HwConfig, amp: f64) -> Block {
        let range = nominal_range(&cfg);
        let rate = cfg.rate.rate;
        let (lo, hi) = range[0];
        let w: Vec<f64> = (0..20_000)
            .map(|i| (amp * (std::f64::consts::TAU * 1000.0 * i as f64 / rate).sin()).clamp(lo, hi))
            .collect();
        Block {
            gen,
            cfg,
            data: [w.clone(), w],
            min: Default::default(),
            max: Default::default(),
            clipped: [true; 2],
            range,
            rate,
            roll: false,
            trigger: None,
        }
    }

    #[test]
    fn trigger_level_outside_the_range_uses_one_inside() {
        let mut s = Scope::new(Settings::default());
        s.settings.channels[0].volt_idx = 0; // 20 mV/div: about ±0.5 V
        s.settings.trigger.level = 1.0;
        s.settings.trigger.mode = TriggerMode::Normal;
        let (gen, cfg) = s.sync_hw().unwrap();
        assert_eq!(cfg.gains[0], 0);
        let (lo, hi) = s.trigger_range();
        assert!((hi - 127.0 / 250.0).abs() < 1e-9, "{hi}");
        assert!((lo + 128.0 / 250.0).abs() < 1e-9, "{lo}");
        let used = s.effective_trigger_level().expect("out of range");
        assert!((used - (hi - 0.05 * (hi - lo))).abs() < 1e-9, "{used}");
        // The clipped sine never reaches 1 V, but it triggers.
        assert!(s.push(clipped_block(gen, cfg, 2.0)));
        assert_eq!(s.state, TrigState::Triggered);
        // The user's level stays.
        assert_eq!(s.settings.trigger.level, 1.0);
        // Below the range: 5 % above the low edge.
        s.settings.trigger.level = -3.0;
        let used = s.effective_trigger_level().unwrap();
        assert!((used - (lo + 0.05 * (hi - lo))).abs() < 1e-9);
        // The probe and invert move the range.
        s.settings.channels[0].probe = 10;
        s.settings.channels[0].invert = true;
        let (lo10, hi10) = s.trigger_range();
        assert!((hi10 + lo * 10.0).abs() < 1e-9 && (lo10 + hi * 10.0).abs() < 1e-9);
        // Back at 1 V/div the real level is used again.
        s.settings.channels[0].probe = 1;
        s.settings.channels[0].invert = false;
        s.settings.channels[0].volt_idx = 5;
        s.settings.trigger.level = 1.0;
        assert_eq!(s.effective_trigger_level(), None);
    }

    #[test]
    fn trigger_level_inside_the_range_is_unchanged() {
        let mut s = scope();
        s.settings.trigger.level = 0.3;
        assert_eq!(s.effective_trigger_level(), None);
        s.push(sine_block(1, 1e6, 1000.0, 1.0, 0.0));
        let f = s.frame.as_ref().unwrap();
        assert!(f.triggered);
        // The trace crosses the user's level at the trigger point.
        let tr = f.traces[0].as_ref().unwrap();
        let i = (-f.t0 / f.dt).floor() as usize;
        assert!(tr[i] <= 0.3 && tr[i + 1] >= 0.3, "{} {}", tr[i], tr[i + 1]);
    }

    #[test]
    fn roll_mode_appends_and_scrolls() {
        let mut s = Scope::new(Settings::default());
        s.settings.time_idx = time_index(500e-3);
        let (gen, cfg) = s.sync_hw().unwrap();
        assert!(cfg.roll);
        let rate = cfg.rate.rate;
        let piece = |k: usize| Block {
            gen,
            cfg,
            data: [vec![k as f64; 1000], vec![0.0; 1000]],
            min: [vec![k as f64 - 0.5; 1000], vec![0.0; 1000]],
            max: [vec![k as f64 + 0.5; 1000], vec![0.0; 1000]],
            clipped: [false; 2],
            range: nominal_range(&cfg),
            rate,
            roll: true,
            trigger: None,
        };
        for k in 0..20 {
            s.push(piece(k));
        }
        let f = s.frame.as_ref().unwrap();
        let tr = f.traces[0].as_ref().unwrap();
        // 1 s screen at 10 kS/s: 10001 samples kept.
        assert_eq!(tr.len(), 10_001);
        assert_eq!(*tr.last().unwrap(), 19.0);
        // Newest sample at the right edge.
        assert!((f.x((tr.len() - 1) as f64, &s.settings) - 5.0).abs() < 1e-9);
        assert_eq!(s.state, TrigState::Roll);
    }

    #[test]
    fn reprocess_while_stopped() {
        let mut s = scope();
        s.push(sine_block(1, 1e6, 1000.0, 1.0, 0.0));
        s.set_running(false);
        s.settings.channels[0].invert = true;
        s.reprocess();
        assert_eq!(s.state, TrigState::Stopped);
        let m = s.measure(0).unwrap();
        assert!((m.pk_pk - 2.0).abs() < 0.01);
        assert!(s.frame.as_ref().unwrap().triggered);
    }

    #[test]
    fn autoset_finds_scale_and_time_base() {
        let mut s = scope();
        s.settings.channels[1].enabled = false;
        s.start_autoset();
        s.sync_hw();
        let rate = s.hw.unwrap().rate.rate;
        let gen = s.gen;
        // 2 kHz, 3 Vpp.
        let mut b = sine_block(gen, rate, 2000.0, 1.5, 0.0);
        b.data[1].clear();
        s.push(b);
        // The refine pass measures again at the picked settings.
        assert!(s.autoset_running());
        s.sync_hw();
        let rate = s.hw.unwrap().rate.rate;
        let mut b = sine_block(s.gen, rate, 2000.0, 1.5, 0.0);
        b.data[1].clear();
        s.push(b);
        assert!(!s.autoset_running());
        let st = &s.settings;
        // 3 Vpp in 8 divs at 80 %: 0.5 V/div.
        assert_eq!(st.channels[0].volts_per_div(), 0.5);
        // 3 periods of 500 us on 10 divs: 150 us/div -> 200 us/div.
        assert_eq!(st.time_per_div(), 200e-6);
        assert_eq!(st.trigger.source, 0);
        assert!(st.trigger.level.abs() < 0.05);
        assert!(s.notices.iter().any(|n| n.contains("2 kHz")));
    }

    #[test]
    fn autoset_looks_slower_for_low_frequencies() {
        let mut s = scope();
        s.start_autoset();
        s.sync_hw();
        let rate = s.hw.unwrap().rate.rate;
        // 20 Hz: no full period in a 20 ms block at 1 MS/s.
        s.push(sine_block(s.gen, rate, 20.0, 1.0, 0.0));
        assert!(s.autoset_running());
        assert_eq!(s.settings.time_per_div(), 50e-3);
        s.sync_hw();
        let rate = s.hw.unwrap().rate.rate;
        s.push(sine_block(s.gen, rate, 20.0, 1.0, 0.0));
        assert!(s.autoset_running());
        s.sync_hw();
        let rate = s.hw.unwrap().rate.rate;
        s.push(sine_block(s.gen, rate, 20.0, 1.0, 0.0));
        assert!(!s.autoset_running());
        assert!(s.notices.iter().any(|n| n.contains("20 Hz")));
    }

    #[test]
    fn tiny_signals_have_no_timing() {
        let mut s = scope();
        // 0.2 Vpp at 1 V/div is under a quarter division.
        s.push(sine_block(1, 1e6, 1000.0, 0.1, 0.0));
        let m = s.measure(0).unwrap();
        assert!((m.pk_pk - 0.2).abs() < 0.01);
        assert_eq!(m.freq, None);
        s.settings.channels[0].volt_idx = 0; // 20 mV/div
        s.reprocess();
        assert!(s.measure(0).unwrap().freq.is_some());
    }

    /// A frame of the stream processing around `b`, as the app gets it.
    fn published(s: &Scope, b: Block) -> Published {
        Published {
            gen: s.gen,
            pgen: s.pgen,
            frame: None,
            state: TrigState::Waiting,
            range: b.range,
            snapshot: Some(b),
            triggers: 0,
            averaged: 0,
            trigger_at: None,
            points: 0,
            single_done: false,
            ac_mean: [0.0; 2],
        }
    }

    #[test]
    fn autoset_takes_level_and_offset_from_the_picked_range() {
        let mut s = scope();
        s.settings.channels[1].enabled = false;
        s.settings.trigger.level = 2.42;
        s.settings.channels[0].offset = -4.0;
        s.start_autoset();
        // A 0 .. 2 V square at 1 kHz; the 5 V/div range reads 1 V high
        // (its zero is off), the others are right.
        let block = |s: &mut Scope| {
            s.sync_hw();
            s.sync_proc();
            let rate = s.hw.unwrap().rate.rate;
            let err = if s.settings.channels[0].volts_per_div() >= 5.0 {
                1.0
            } else {
                0.0
            };
            let mut b = sine_block(s.gen, rate, 1000.0, 1.0, 0.0);
            b.data[0] = b.data[0]
                .iter()
                .map(|v| if *v >= 0.0 { 2.0 } else { 0.0 } + err)
                .collect();
            b.data[1].clear();
            published(s, b)
        };
        let p = block(&mut s);
        s.take(p);
        assert!(s.autoset_running(), "the coarse pass is not the end");
        // A frame of the old settings does not count.
        let mut stale = block(&mut s);
        stale.gen -= 1;
        s.take(stale);
        assert!(s.autoset_running());
        for _ in 0..AUTOSET_REFINES {
            if !s.autoset_running() {
                break;
            }
            let p = block(&mut s);
            s.take(p);
        }
        assert!(!s.autoset_running());
        let st = &s.settings;
        assert!(
            (st.trigger.level - 1.0).abs() < 0.05,
            "level {}",
            st.trigger.level
        );
        // 2 Vpp in 8 divs at 80 %: 0.5 V/div, the middle (1 V) on the centre.
        assert_eq!(st.channels[0].volts_per_div(), 0.5);
        assert!(
            (st.channels[0].offset + 2.0).abs() < 0.1,
            "offset {}",
            st.channels[0].offset
        );
        assert!(s.notices.iter().any(|n| n.contains("1 kHz")));
    }

    #[test]
    fn time_index_rounds_up() {
        assert_eq!(TIME_DIVS[time_index(1e-3)], 1e-3);
        assert_eq!(TIME_DIVS[time_index(1.5e-3)], 2e-3);
        assert_eq!(time_index(1e9), TIME_DIVS.len() - 1);
        assert_eq!(time_index(0.0), 0);
    }

    /// The 1 kHz calibration wave at the BNC of a x10 probe (0 .. 0.2 V)
    /// with RC edges of 1.5 µs (10 - 90 %), as consecutive blocks of
    /// 20000 samples with 1 LSB of noise, quantized to the ADC steps.
    struct CalWave {
        k: u64,
        seed: u64,
    }

    impl CalWave {
        fn new() -> Self {
            Self {
                k: 0,
                seed: 0x1234_5678_9abc_def1,
            }
        }

        fn noise(&mut self) -> f64 {
            let mut u = || {
                self.seed ^= self.seed << 13;
                self.seed ^= self.seed >> 7;
                self.seed ^= self.seed << 17;
                ((self.seed >> 11) as f64 / (1u64 << 53) as f64).max(1e-300)
            };
            let (a, b) = (u(), u());
            (-2.0 * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos()
        }

        /// The next block for scope `s`, the wave on channel `on`.
        fn block(&mut self, s: &Scope, on: usize) -> Block {
            let cfg = s.hw.unwrap();
            let rate = cfg.rate.rate;
            let range = nominal_range(&cfg);
            let f = 1000.0 * (1.0 + 37e-6);
            let tau = 1.5e-6 / 2.2;
            let mut data: [Vec<f64>; 2] = Default::default();
            for c in 0..cfg.channels as usize {
                let lsb = (range[c].1 - range[c].0) / 255.0;
                data[c] = (0..20_000u64)
                    .map(|i| {
                        let t = (self.k + i) as f64 / rate;
                        let p = (t * f).fract() / f;
                        let half = 0.5 / f;
                        let v = if c != on {
                            0.0
                        } else if p < half {
                            0.2 * (1.0 - (-p / tau).exp())
                        } else {
                            0.2 * (-(p - half) / tau).exp()
                        };
                        ((v / lsb + self.noise()).round() * lsb).clamp(range[c].0, range[c].1)
                    })
                    .collect();
            }
            self.k += 20_000;
            Block {
                gen: s.gen,
                cfg,
                data,
                min: Default::default(),
                max: Default::default(),
                clipped: [false; 2],
                range,
                rate,
                roll: false,
                trigger: None,
            }
        }
    }

    /// 50 ns/div, Auto, rising at 1.16 V, x10 probes, CH1 / CH2 on as
    /// given, trigger source `src`.
    fn cal_scope(on: [bool; 2], src: usize, volt_idx: [usize; 2], avg: usize) -> Scope {
        let mut st = Settings {
            time_idx: 1,
            ..Default::default()
        };
        st.trigger.mode = TriggerMode::Auto;
        st.trigger.source = src;
        st.trigger.level = 1.16;
        st.trigger.position = 0.5;
        for c in 0..2 {
            st.channels[c].enabled = on[c];
            st.channels[c].probe = 10;
            st.channels[c].volt_idx = volt_idx[c];
            st.channels[c].offset = -2.3;
        }
        st.display.averaging = avg;
        st.display.interpolation = Interpolation::Sinc;
        let mut s = Scope::new(st);
        s.sync_hw();
        s
    }

    /// Push `n` cal wave blocks; return the frames shown, how many of
    /// them were triggered, and the standard deviation and largest size
    /// (ns) of where the drawn trace (sin(x)/x, 400 dots) crosses the
    /// level, relative to the trigger point.
    fn cal_run(s: &mut Scope, n: usize) -> (usize, usize, f64, f64) {
        let on = s.settings.trigger_source();
        let mut wave = CalWave::new();
        let (mut shown, mut trig) = (0, 0);
        let mut xs = Vec::new();
        for _ in 0..n {
            let b = wave.block(s, on);
            if !s.push(b) {
                continue;
            }
            shown += 1;
            let f = s.frame.as_ref().unwrap();
            if !f.triggered {
                continue;
            }
            trig += 1;
            let pts = crate::ui::trace_points(f, on, &s.settings, 400, false);
            let ch = &s.settings.channels[on];
            let yl = s.settings.trigger.level / ch.volts_per_div() + ch.offset;
            let x = pts
                .windows(2)
                .filter(|w| w[0].1 < yl && w[1].1 >= yl)
                .map(|w| w[0].0 + (yl - w[0].1) / (w[1].1 - w[0].1) * (w[1].0 - w[0].0))
                .min_by(|a, b| a.abs().total_cmp(&b.abs()))
                .expect("a crossing");
            xs.push(x * s.settings.time_per_div() * 1e9);
        }
        let m = xs.iter().sum::<f64>() / xs.len().max(1) as f64;
        let sd = (xs.iter().map(|x| (x - m).powi(2)).sum::<f64>() / xs.len().max(1) as f64).sqrt();
        let big = xs.iter().fold(0.0f64, |a, x| a.max(x.abs()));
        (shown, trig, sd, big)
    }

    #[test]
    fn cal_wave_ch2_alone_holds_still_with_averaging() {
        // 2-channel mode at 16 MS/s: 8 samples on the screen. Before the
        // captures were aligned, averaging 64 moved the edge by up to
        // ±0.5 sample (sd 17 ns, 0.33 div).
        for avg in [1, 64] {
            let mut s = cal_scope([false, true], 1, [7, 1], avg);
            assert_eq!(s.hw.unwrap().rate.rate, 16e6);
            let (shown, trig, sd, big) = cal_run(&mut s, 300);
            assert_eq!((shown, trig), (300, 300));
            assert!(sd < 10.0 && big < 60.0, "avg {avg}: sd {sd} ns, max {big} ns");
            if avg == 64 {
                assert!(sd < 3.0, "sd {sd} ns");
            }
        }
    }

    #[test]
    fn cal_wave_ch1_alone_at_30_ms_keeps_the_trigger() {
        // CH1 alone at 30 MS/s: a block (0.67 ms) is shorter than the
        // 1 ms period, so a third of them have no edge. In Normal mode
        // those wait, and the average goes on. (The stream processing
        // has no blocks: it finds every edge, see the pipeline tests.)
        // The trigger source is CH2 (off): it follows CH1, in 1-channel
        // mode.
        for avg in [1, 64] {
            let mut s = cal_scope([true, false], 1, [1, 1], avg);
            s.settings.trigger.mode = TriggerMode::Normal;
            let hw = s.hw.unwrap();
            assert_eq!((hw.channels, hw.rate.rate), (1, 30e6));
            let (shown, trig, sd, big) = cal_run(&mut s, 300);
            assert_eq!(shown, trig, "untriggered frames shown");
            assert!(trig > 150, "{trig}");
            assert!(sd < 10.0 && big < 60.0, "avg {avg}: sd {sd} ns, max {big} ns");
            if avg == 64 {
                assert!(s.avg.count() > 32, "{}", s.avg.count());
            }
        }
    }

    #[test]
    fn big_volts_per_div_still_triggers() {
        // 5 V/div at the BNC (50 V/div with x10): 0.1 div of hysteresis
        // (5 V) is more than the 2 V signal, so it never armed.
        let mut s = cal_scope([true, false], 0, [7, 1], 1);
        let (shown, trig, _, _) = cal_run(&mut s, 30);
        assert!(shown > 0 && trig == shown, "{trig} of {shown}");
        // The cap: 0.1 div normally, at most a quarter of the pk-pk.
        let v = [0.0, 2.0];
        assert_eq!(hysteresis(0.5, &v, 0.04), 0.05);
        assert_eq!(hysteresis(50.0, &v, 0.4), 0.8);
        assert_eq!(hysteresis(50.0, &v, 0.04), 0.5);
    }

    #[test]
    fn untriggered_blocks_leave_the_average_alone() {
        let mut s = scope();
        s.settings.display.averaging = 4;
        // 20000 samples at 1 MS/s: 20 ms per block.
        assert!(s.push(sine_block(1, 1e6, 1000.0, 2.0, 1.0)));
        assert!(s.push(sine_block(1, 1e6, 1000.0, 2.0, 1.0)));
        assert_eq!(s.avg.count(), 2);
        let flat = |s: &Scope| {
            let mut b = sine_block(s.gen, 1e6, 1000.0, 0.0, 0.0);
            b.cfg = s.hw.unwrap();
            b
        };
        // A block without a trigger is shown untriggered at once (the
        // stream processing does the Auto timeout).
        assert!(s.push(flat(&s)));
        assert!(!s.frame.as_ref().unwrap().triggered);
        assert_eq!(s.state, TrigState::Auto);
        assert!(s.push(flat(&s)));
        // The untriggered frames left the average alone.
        assert_eq!(s.avg.count(), 2);
        assert!(s.push(sine_block(1, 1e6, 1000.0, 2.0, 1.0)));
        assert_eq!(s.avg.count(), 3);
        assert!(s.frame.as_ref().unwrap().triggered);
    }

    #[test]
    fn shift_moves_by_a_fraction_of_a_sample() {
        let v = [0.0, 1.0, 3.0, 2.0];
        assert_eq!(shift(&v, 0, 4, 0.0, Shift::Line), v.to_vec());
        assert_eq!(shift(&v, 1, 4, 0.5, Shift::Line), vec![2.0, 2.5, 2.0]);
        assert_eq!(shift(&v, 0, 3, 0.25, Shift::Min), vec![0.0, 1.0, 2.0]);
        assert_eq!(shift(&v, 0, 9, 0.25, Shift::Max), vec![1.0, 3.0, 3.0, 2.0]);
    }
}
