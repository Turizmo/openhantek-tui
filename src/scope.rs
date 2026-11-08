//! Scope logic: turns sample blocks into the frame on screen. Pure code,
//! so it is easy to test: no terminal and no USB here.

use std::collections::VecDeque;
use std::ops::Range;

use crate::acquire::Block;
use crate::device::protocol::{choose_rate, gain_index, hw_channels, nominal_range, HwConfig};
use crate::dsp::math::combine;
use crate::dsp::measure::{measure, Measurements};
use crate::dsp::trigger::find_trigger;
use crate::dsp::{Averager, PeakHold};
use crate::settings::*;

/// Index of the math trace in [`Frame::traces`].
pub const MATH: usize = 2;

/// Extra samples kept on each side of the screen, for interpolation.
const MARGIN: usize = 8;

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

    /// Indexes of the samples inside the screen.
    pub fn visible(&self, s: &Settings, len: usize) -> Range<usize> {
        if self.dt <= 0.0 || len == 0 {
            return 0..0;
        }
        let half = H_DIVS / 2.0;
        let tdiv = s.time_per_div();
        // x(i) = x_ref + (t0 + i dt) / tdiv  =>  i = ((x - x_ref) tdiv - t0) / dt
        let idx = |x: f64| ((x - self.x_ref(s)) * tdiv - self.t0) / self.dt;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AutosetStep {
    /// Look at 1 ms/div with the biggest range.
    Fast,
    /// Nothing found: look slower for low frequencies.
    Slow,
}

pub struct Scope {
    pub settings: Settings,
    pub running: bool,
    pub frame: Option<Frame>,
    /// Old frames for persistence, newest last.
    pub history: VecDeque<Frame>,
    pub state: TrigState,
    /// Generation of the hardware settings now in use.
    pub gen: u64,
    pub hw: Option<HwConfig>,
    averagers: [Averager; 3],
    /// The peak band over the averaged captures, reset with `averagers`.
    peaks: [PeakHold; 3],
    /// Roll mode: mean, min and max per channel.
    roll: [VecDeque<f64>; 2],
    roll_min: [VecDeque<f64>; 2],
    roll_max: [VecDeque<f64>; 2],
    roll_rate: f64,
    roll_clipped: [bool; 2],
    last_block: Option<Block>,
    autoset: Option<AutosetStep>,
    /// Notices for the message line (autoset results and such).
    pub notices: Vec<String>,
    /// Input range at the BNC per channel, from the last block, with the
    /// gain index it was measured at.
    adc_range: [Option<(usize, (f64, f64))>; 2],
    /// Mean that AC coupling took away, per channel, in the last block.
    ac_mean: [f64; 2],
}

/// How far inside the input range the trigger level goes when the
/// user's level is outside it: 5 % of the range.
pub const TRIGGER_INSIDE: f64 = 0.05;

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
            hw: None,
            averagers: Default::default(),
            peaks: Default::default(),
            roll: Default::default(),
            roll_min: Default::default(),
            roll_max: Default::default(),
            roll_rate: 0.0,
            roll_clipped: [false; 2],
            last_block: None,
            autoset: None,
            notices: Vec::new(),
            adc_range: [None; 2],
            ac_mean: [0.0; 2],
        }
    }

    /// Voltage range (at the probe tip, low to high) the trigger source
    /// can measure at its gain: the ADC range from the calibration, with
    /// probe, invert and AC coupling. Without a block yet, the nominal
    /// calibration.
    pub fn trigger_range(&self) -> (f64, f64) {
        let s = &self.settings;
        let src = s.trigger.source.min(1);
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
            || (!roll && s.trigger.source == 1);
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

    /// Start the capture average (and its peak band) over.
    fn reset_average(&mut self) {
        for a in &mut self.averagers {
            a.reset();
        }
        for p in &mut self.peaks {
            p.reset();
        }
    }

    fn clear_roll(&mut self) {
        for r in self
            .roll
            .iter_mut()
            .chain(&mut self.roll_min)
            .chain(&mut self.roll_max)
        {
            r.clear();
        }
        self.roll_clipped = [false; 2];
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

    /// The peak band of a channel at the probe tip, like
    /// [`Scope::condition`]: low and high (an inverted channel swaps
    /// them). A block without min and max has a band as thin as the trace.
    fn condition_peak(&self, block: &Block, c: usize) -> Option<(Vec<f64>, Vec<f64>)> {
        let data = &block.data[c];
        if data.is_empty() {
            return None;
        }
        let (min, max) = if block.min[c].len() == data.len() && block.max[c].len() == data.len()
        {
            (&block.min[c], &block.max[c])
        } else {
            (data, data)
        };
        let ch = &self.settings.channels[c];
        let k = ch.probe as f64 * if ch.invert { -1.0 } else { 1.0 };
        let mean = ac_mean(block, ch, c);
        let lo: Vec<f64> = min.iter().map(|v| (v - mean) * k).collect();
        let hi: Vec<f64> = max.iter().map(|v| (v - mean) * k).collect();
        Some(if k < 0.0 { (hi, lo) } else { (lo, hi) })
    }

    fn process(&mut self, block: &Block) -> bool {
        for c in 0..2 {
            if !block.data[c].is_empty() {
                self.adc_range[c] = Some((block.cfg.gains[c], block.range[c]));
                self.ac_mean[c] = ac_mean(block, &self.settings.channels[c], c);
            }
        }
        let ch = [self.condition(block, 0), self.condition(block, 1)];
        let pk = [self.condition_peak(block, 0), self.condition_peak(block, 1)];
        if block.roll {
            return self.process_roll(block, ch, pk);
        }
        let s = &self.settings;
        let rate = block.rate;
        let dt = 1.0 / rate;
        let d = ((s.screen_time() * rate).round() as usize).max(2);
        let pre = (s.trigger.position * d as f64).round() as usize;
        let post = d - pre;
        let src = s.trigger.source.min(1);
        let src_samples = ch[src].as_deref().or(ch[1 - src].as_deref());
        let len = src_samples.map_or(0, |v| v.len());
        let vdiv = s.channels[src].volts_per_div();
        let level = self.effective_trigger_level().unwrap_or(s.trigger.level);
        let trig = src_samples.and_then(|v| {
            find_trigger(
                v,
                level,
                s.trigger.slope,
                0.1 * vdiv,
                pre + MARGIN,
                len.saturating_sub(post + MARGIN),
            )
        });
        let (start, t0, triggered) = match trig {
            Some(t) => {
                let start = (t.floor() as usize).saturating_sub(pre + MARGIN);
                (start, (start as f64 - t) * dt, true)
            }
            None if s.trigger.mode == TriggerMode::Auto => {
                // No trigger: show the start of the block as if it triggered there.
                (0, -((pre + MARGIN) as f64) * dt, false)
            }
            None => {
                self.state = TrigState::Waiting;
                return false;
            }
        };
        let end = start + d + 2 * MARGIN + 1;
        let cut = |v: &Option<Vec<f64>>| -> Option<Vec<f64>> {
            v.as_ref()
                .map(|v| v[start.min(v.len())..end.min(v.len())].to_vec())
        };
        let mut traces = [cut(&ch[0]), cut(&ch[1]), None];
        let cut_pk = |p: &Option<(Vec<f64>, Vec<f64>)>| {
            p.as_ref().map(|(a, b)| {
                let r = |v: &Vec<f64>| v[start.min(v.len())..end.min(v.len())].to_vec();
                (r(a), r(b))
            })
        };
        let mut peak = [cut_pk(&pk[0]), cut_pk(&pk[1]), None];
        let depth = s.display.averaging;
        for i in 0..2 {
            if let Some(t) = &mut traces[i] {
                if triggered && depth > 1 {
                    *t = self.averagers[i].push(t, depth);
                    if let Some((lo, hi)) = &mut peak[i] {
                        (*lo, *hi) = self.peaks[i].push(lo, hi, depth);
                    }
                } else {
                    self.averagers[i].reset();
                    self.peaks[i].reset();
                }
            }
        }
        traces[MATH] = self.math_trace(&traces);
        peak[MATH] = self.math_peak(&traces, &peak);
        let frame = Frame {
            traces,
            peak,
            dt,
            t0,
            triggered,
            roll: false,
            clipped: block.clipped,
            rate,
        };
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

    fn math_trace(&self, traces: &[Option<Vec<f64>>; 3]) -> Option<Vec<f64>> {
        let m = &self.settings.math;
        if !m.enabled {
            return None;
        }
        let empty = Vec::new();
        let a = traces[0].as_ref().unwrap_or(&empty);
        let b = traces[1].as_ref().unwrap_or(&empty);
        if (m.mode.uses_ch1() && a.is_empty()) || (m.mode.uses_ch2() && b.is_empty()) {
            return None;
        }
        Some(combine(m.mode, a, b))
    }

    /// The peak band of the math trace from the bands of CH1 and CH2, by
    /// interval arithmetic (a band that holds every value the function
    /// can take there). Division gets no band (the line only).
    fn math_peak(
        &self,
        traces: &[Option<Vec<f64>>; 3],
        peak: &[Option<(Vec<f64>, Vec<f64>)>; 3],
    ) -> Option<(Vec<f64>, Vec<f64>)> {
        use crate::dsp::math::MathMode::*;
        let m = &self.settings.math;
        let line = traces[MATH].as_ref()?;
        let empty = (Vec::new(), Vec::new());
        let (al, ah) = peak[0].as_ref().unwrap_or(&empty);
        let (bl, bh) = peak[1].as_ref().unwrap_or(&empty);
        let n = line.len();
        let get = |v: &Vec<f64>, i: usize| v.get(i).copied().unwrap_or(f64::NAN);
        let band = |f: &dyn Fn(f64, f64, f64, f64) -> (f64, f64)| {
            let (lo, hi): (Vec<f64>, Vec<f64>) = (0..n)
                .map(|i| f(get(al, i), get(ah, i), get(bl, i), get(bh, i)))
                .unzip();
            Some((lo, hi))
        };
        match m.mode {
            Add => band(&|al, ah, bl, bh| (al + bl, ah + bh)),
            Sub => band(&|al, ah, bl, bh| (al - bh, ah - bl)),
            SubRev => band(&|al, ah, bl, bh| (bl - ah, bh - al)),
            Avg => band(&|al, ah, bl, bh| ((al + bl) / 2.0, (ah + bh) / 2.0)),
            NegCh1 => band(&|al, ah, _, _| (-ah, -al)),
            NegCh2 => band(&|_, _, bl, bh| (-bh, -bl)),
            Mul => band(&|al, ah, bl, bh| {
                let p = [al * bl, al * bh, ah * bl, ah * bh];
                (
                    p.iter().copied().fold(f64::INFINITY, f64::min),
                    p.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                )
            }),
            _ => Some((line.clone(), line.clone())),
        }
    }

    fn process_roll(
        &mut self,
        block: &Block,
        ch: [Option<Vec<f64>>; 2],
        pk: [Option<(Vec<f64>, Vec<f64>)>; 2],
    ) -> bool {
        let rate = block.rate;
        if (rate - self.roll_rate).abs() > 1e-9 {
            self.clear_roll();
            self.roll_rate = rate;
        }
        let keep = (self.settings.screen_time() * rate).ceil() as usize + 1;
        for (c, data) in ch.iter().enumerate() {
            if let (Some(d), Some((lo, hi))) = (data, &pk[c]) {
                for (buf, v) in [
                    (&mut self.roll[c], d),
                    (&mut self.roll_min[c], lo),
                    (&mut self.roll_max[c], hi),
                ] {
                    buf.extend(v.iter().copied());
                    while buf.len() > keep {
                        buf.pop_front();
                    }
                }
            }
            self.roll_clipped[c] |= block.clipped[c];
        }
        let all = |b: &VecDeque<f64>| b.iter().copied().collect::<Vec<f64>>();
        let [a, b] = [0, 1].map(|c| (!self.roll[c].is_empty()).then(|| all(&self.roll[c])));
        let n = a.as_ref().or(b.as_ref()).map_or(0, |v| v.len());
        let mut full: [Option<Vec<f64>>; 3] = [a, b, None];
        full[MATH] = self.math_trace(&full);
        let band = |c: usize| {
            (!self.roll[c].is_empty()).then(|| (all(&self.roll_min[c]), all(&self.roll_max[c])))
        };
        let mut peak = [band(0), band(1), None];
        peak[MATH] = self.math_peak(&full, &peak);
        let dt = 1.0 / rate;
        let frame = Frame {
            traces: full,
            peak,
            dt,
            t0: -(n.saturating_sub(1) as f64) * dt,
            triggered: false,
            roll: true,
            clipped: self.roll_clipped,
            rate,
        };
        self.frame = Some(frame);
        self.state = TrigState::Roll;
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
        let v = f.visible_samples(t, &self.settings)?;
        if v.len() < 2 {
            return None;
        }
        let mut m = measure(v, f.dt);
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
        for (i, (c, m)) in found.iter().enumerate() {
            let ch = &mut self.settings.channels[*c];
            let probe = ch.probe as f64;
            let pp = m.pk_pk.max(1e-3);
            ch.volt_idx = VOLT_DIVS
                .iter()
                .position(|v| pp <= v * probe * band * 0.8)
                .unwrap_or(VOLT_DIVS.len() - 1);
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
        match freq {
            Some(f) => {
                // About 3 periods on the screen.
                self.settings.time_idx = time_index(3.0 / f / H_DIVS);
                self.notices
                    .push(format!("Autoset: {}", crate::si::fmt_si(f, "Hz")));
            }
            None => {
                self.settings.time_idx = time_index(1e-3);
                self.notices
                    .push("Autoset: no periodic signal found".into());
            }
        }
        self.autoset = None;
    }
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
        assert_eq!(s.hw_config().channels, 2);
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
        s.settings.time_idx = time_index(100e-3);
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

    #[test]
    fn time_index_rounds_up() {
        assert_eq!(TIME_DIVS[time_index(1e-3)], 1e-3);
        assert_eq!(TIME_DIVS[time_index(1.5e-3)], 2e-3);
        assert_eq!(time_index(1e9), TIME_DIVS.len() - 1);
        assert_eq!(time_index(0.0), 0);
    }
}
