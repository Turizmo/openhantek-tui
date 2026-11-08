//! The stream processing: raw ADC bytes from the [`Ring`] in, finished
//! frames out. Pure code (no USB, no threads), so the tests can feed it
//! any stream.
//!
//! The ring holds one continuous stream. A decimator turns it into
//! points at the output rate (mean, min and max of `oversample` raw
//! samples; point `p` is raw frames `p * os .. (p + 1) * os`). The
//! [`EdgeScanner`] looks for the trigger in the points of the source
//! channel, in ADC counts. Each trigger cuts a capture around it out of
//! the ring (decimated again, both channels), which goes into the average
//! and the peak band. The trigger rearms at the end of the capture; the
//! scanner keeps its arm state meanwhile, so the first edge after that is
//! never missed. Nothing is dropped because the screen is slow: the
//! newest frame simply replaces one the screen did not take yet.
//!
//! The frame building here (cut, shift, conditioning, math, roll) is also
//! what [`crate::scope::Scope`] uses for a single block.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::ring::Ring;
use crate::acquire::Block;
use crate::device::hantek::SKIP_SAMPLES;
use crate::device::protocol::{AppCal, Calibration, Decimator, HwConfig};
use crate::dsp::math::{combine, MathMode};
use crate::dsp::trigger::{EdgeScanner, Slope};
use crate::dsp::{Averager, PeakHold};
use crate::scope::{Frame, TrigState, MATH};
use crate::settings::{Coupling, Settings, TriggerMode};

/// Extra samples kept on each side of the screen, for interpolation.
pub const MARGIN: usize = 8;

/// Shortest time between two published frames: at most 60 per second.
pub const FRAME_TIME: Duration = Duration::from_nanos(16_666_667);

/// Raw frames decimated in one go, between two looks at the captures.
const CHUNK_FRAMES: u64 = 1 << 18;

/// How far inside the input range the trigger level goes when the
/// user's level is outside it: 5 % of the range.
pub const TRIGGER_INSIDE: f64 = 0.05;

/// The settings the processing uses (not the hardware): a change resets
/// the trigger search and the average, not the USB stream.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcSettings {
    /// Seconds across the screen (with the fine time/div).
    pub screen_time: f64,
    /// Trigger position on the screen, 0 .. 1.
    pub position: f64,
    /// Trigger level at the probe tip.
    pub level: f64,
    pub slope: Slope,
    pub mode: TriggerMode,
    /// The trigger source after the fallback to an enabled channel.
    pub source: usize,
    /// Triggers in the average.
    pub averaging: usize,
    pub probe: [u32; 2],
    pub invert: [bool; 2],
    pub ac: [bool; 2],
    /// Volts/div at the probe tip (for the hysteresis).
    pub vdiv: [f64; 2],
    /// The math trace, when it is on.
    pub math: Option<MathMode>,
    /// The app's calibration.
    pub cal: AppCal,
    /// Counts the single trigger arms: a new value arms it again.
    pub arm: u64,
}

impl ProcSettings {
    pub fn of(s: &Settings, arm: u64) -> Self {
        Self {
            screen_time: s.screen_time(),
            position: s.trigger.position,
            level: s.trigger.level,
            slope: s.trigger.slope,
            mode: s.trigger.mode,
            source: s.trigger_source(),
            averaging: s.display.averaging,
            probe: [0, 1].map(|c| s.channels[c].probe),
            invert: [0, 1].map(|c| s.channels[c].invert),
            ac: [0, 1].map(|c| s.channels[c].coupling == Coupling::Ac),
            vdiv: [0, 1].map(|c| s.channels[c].volts_per_div()),
            math: s.math.enabled.then_some(s.math.mode),
            cal: s.calibration.app_cal(),
            arm,
        }
    }

    /// Probe factor of channel `c`, negative when inverted.
    pub fn k(&self, c: usize) -> f64 {
        self.probe[c] as f64 * if self.invert[c] { -1.0 } else { 1.0 }
    }

    /// Samples on the screen, before and after the trigger, at `rate`.
    pub fn window(&self, rate: f64) -> (usize, usize, usize) {
        let d = ((self.screen_time * rate).round() as usize).max(2);
        let pre = (self.position * d as f64).round() as usize;
        (d, pre, d - pre)
    }
}

/// Samples of a capture: the screen, a margin on each side and one more.
pub fn capture_len(d: usize) -> usize {
    d + 2 * MARGIN + 1
}

#[derive(Clone, Copy)]
pub enum Shift {
    /// Straight line between the two neighbours.
    Line,
    /// The lower of the two neighbours (a band minimum).
    Min,
    /// The higher of the two neighbours (a band maximum).
    Max,
}

/// `v[start..end]`, moved `frac` (0 .. 1) of a sample later in the
/// signal: point j is at `start + j + frac`. Past the end it repeats the
/// last sample.
pub fn shift(v: &[f64], start: usize, end: usize, frac: f64, how: Shift) -> Vec<f64> {
    let end = end.min(v.len());
    let start = start.min(end);
    if frac <= 0.0 {
        return v[start..end].to_vec();
    }
    (start..end)
        .map(|i| {
            let (a, b) = (v[i], v[(i + 1).min(v.len() - 1)]);
            match how {
                Shift::Line => a + (b - a) * frac,
                Shift::Min => a.min(b),
                Shift::Max => a.max(b),
            }
        })
        .collect()
}

/// Volts at the BNC to the probe tip: AC mean off, probe, invert.
fn condition(v: &mut [f64], mean: f64, k: f64) {
    for x in v {
        *x = (*x - mean) * k;
    }
}

/// The traces of CH1 and CH2 cut out of a block for the screen.
pub struct Cut {
    pub traces: [Option<Vec<f64>>; 2],
    pub peak: [Option<(Vec<f64>, Vec<f64>)>; 2],
    /// Time of sample 0 relative to the trigger.
    pub t0: f64,
    pub triggered: bool,
}

/// Cut the screen (with its margins) out of `block` (volts at the BNC)
/// at the trigger `trig` (a fractional index in the block), or from the
/// start of the block without one, and take it to the probe tip.
/// `ac_mean` is the mean AC coupling takes away (volts at the BNC). With
/// averaging, the capture moves by the fraction of a sample of its
/// trigger, so the edge sits at the same place in each one.
pub fn cut(block: &Block, p: &ProcSettings, ac_mean: [f64; 2], trig: Option<f64>) -> Cut {
    let dt = 1.0 / block.rate;
    let (d, pre, _) = p.window(block.rate);
    let mut frac = 0.0;
    let (start, t0) = match trig {
        Some(t) => {
            let tf = t.floor();
            let start = (tf as usize).saturating_sub(pre + MARGIN);
            if p.averaging > 1 {
                frac = t - tf;
                (start, (start as f64 - tf) * dt)
            } else {
                (start, (start as f64 - t) * dt)
            }
        }
        // No trigger: the start of the block, as if it triggered there.
        None => (0, -((pre + MARGIN) as f64) * dt),
    };
    let end = start + capture_len(d);
    let mut traces: [Option<Vec<f64>>; 2] = Default::default();
    let mut peak: [Option<(Vec<f64>, Vec<f64>)>; 2] = Default::default();
    for c in 0..2 {
        let data = &block.data[c];
        if data.is_empty() {
            continue;
        }
        let k = p.k(c);
        let mean = ac_mean[c];
        let mut t = shift(data, start, end, frac, Shift::Line);
        condition(&mut t, mean, k);
        traces[c] = Some(t);
        // A block without min and max has a band as thin as the trace;
        // else the moved band holds both neighbours.
        let raw = block.min[c].len() == data.len() && block.max[c].len() == data.len();
        let (mut lo, mut hi) = if raw {
            (
                shift(&block.min[c], start, end, frac, Shift::Min),
                shift(&block.max[c], start, end, frac, Shift::Max),
            )
        } else {
            let line = shift(data, start, end, frac, Shift::Line);
            (line.clone(), line)
        };
        condition(&mut lo, mean, k);
        condition(&mut hi, mean, k);
        // An inverted channel swaps them.
        peak[c] = Some(if k < 0.0 { (hi, lo) } else { (lo, hi) });
    }
    Cut {
        traces,
        peak,
        t0,
        triggered: trig.is_some(),
    }
}

/// The math trace from CH1 and CH2.
pub fn math_trace(mode: Option<MathMode>, traces: &[Option<Vec<f64>>; 3]) -> Option<Vec<f64>> {
    let mode = mode?;
    let empty = Vec::new();
    let a = traces[0].as_ref().unwrap_or(&empty);
    let b = traces[1].as_ref().unwrap_or(&empty);
    if (mode.uses_ch1() && a.is_empty()) || (mode.uses_ch2() && b.is_empty()) {
        return None;
    }
    Some(combine(mode, a, b))
}

/// The peak band of the math trace from the bands of CH1 and CH2, by
/// interval arithmetic (a band that holds every value the function can
/// take there). Division gets no band (the line only).
pub fn math_peak(
    mode: Option<MathMode>,
    traces: &[Option<Vec<f64>>; 3],
    peak: &[Option<(Vec<f64>, Vec<f64>)>; 3],
) -> Option<(Vec<f64>, Vec<f64>)> {
    use crate::dsp::math::MathMode::*;
    let mode = mode?;
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
    match mode {
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

/// The frame of CH1 and CH2 traces and bands: adds the math trace.
#[allow(clippy::too_many_arguments)]
pub fn frame(
    p: &ProcSettings,
    traces: [Option<Vec<f64>>; 2],
    peak: [Option<(Vec<f64>, Vec<f64>)>; 2],
    rate: f64,
    t0: f64,
    triggered: bool,
    roll: bool,
    clipped: [bool; 2],
) -> Frame {
    let [a, b] = traces;
    let mut traces = [a, b, None];
    traces[MATH] = math_trace(p.math, &traces);
    let [pa, pb] = peak;
    let mut peak = [pa, pb, None];
    peak[MATH] = math_peak(p.math, &traces, &peak);
    Frame {
        traces,
        peak,
        dt: 1.0 / rate,
        t0,
        triggered,
        roll,
        clipped,
        rate,
    }
}

/// The average and the peak band of CH1 and CH2 over the last triggers.
#[derive(Clone, Debug, Default)]
pub struct Averaging {
    avg: [Averager; 2],
    peaks: [PeakHold; 2],
}

impl Averaging {
    pub fn reset(&mut self) {
        for a in &mut self.avg {
            a.reset();
        }
        for p in &mut self.peaks {
            p.reset();
        }
    }

    /// Add one triggered capture.
    pub fn add(&mut self, cut: &Cut, depth: usize) {
        for c in 0..2 {
            if let Some(t) = &cut.traces[c] {
                self.avg[c].add(t, depth);
            }
            if let Some((lo, hi)) = &cut.peak[c] {
                self.peaks[c].add(lo, hi, depth);
            }
        }
    }

    /// The averaged traces and bands; `last` (the last capture) is what
    /// is shown without averaging, and for a channel the average lacks.
    pub fn value(&self, last: &Cut, depth: usize) -> Cut {
        let mut out = Cut {
            traces: last.traces.clone(),
            peak: last.peak.clone(),
            t0: last.t0,
            triggered: last.triggered,
        };
        if depth > 1 {
            for c in 0..2 {
                if out.traces[c].is_some() && self.avg[c].count() > 0 {
                    out.traces[c] = Some(self.avg[c].value());
                }
                if out.peak[c].is_some() && self.peaks[c].count() > 0 {
                    out.peak[c] = Some(self.peaks[c].value());
                }
            }
        }
        out
    }

    /// Triggers in the average.
    pub fn count(&self) -> usize {
        self.avg.iter().map(|a| a.count()).max().unwrap_or(0)
    }
}

/// Roll mode: the newest screen of the stream, in volts at the BNC
/// (mean, min and max), scrolling.
#[derive(Clone, Debug, Default)]
pub struct Roll {
    mean: [VecDeque<f64>; 2],
    min: [VecDeque<f64>; 2],
    max: [VecDeque<f64>; 2],
    rate: f64,
    clipped: [bool; 2],
}

impl Roll {
    pub fn clear(&mut self) {
        for r in self
            .mean
            .iter_mut()
            .chain(&mut self.min)
            .chain(&mut self.max)
        {
            r.clear();
        }
        self.clipped = [false; 2];
    }

    /// Append a piece of the stream (a block of `rate` samples per
    /// second; min and max may be empty: then they are the mean). Keeps
    /// one screen of `screen_time`.
    pub fn push(&mut self, block: &Block, screen_time: f64) {
        let rate = block.rate;
        if (rate - self.rate).abs() > 1e-9 {
            self.clear();
            self.rate = rate;
        }
        let keep = (screen_time * rate).ceil() as usize + 1;
        for c in 0..2 {
            let d = &block.data[c];
            if d.is_empty() {
                continue;
            }
            let raw = block.min[c].len() == d.len() && block.max[c].len() == d.len();
            let (lo, hi) = if raw {
                (&block.min[c], &block.max[c])
            } else {
                (d, d)
            };
            for (buf, v) in [
                (&mut self.mean[c], d),
                (&mut self.min[c], lo),
                (&mut self.max[c], hi),
            ] {
                buf.extend(v.iter().copied());
                let over = buf.len().saturating_sub(keep);
                buf.drain(..over);
            }
            self.clipped[c] |= block.clipped[c];
        }
    }

    /// The roll frame: the newest sample at the right edge.
    pub fn frame(&self, p: &ProcSettings) -> Frame {
        let all = |b: &VecDeque<f64>| b.iter().copied().collect::<Vec<f64>>();
        let mut traces: [Option<Vec<f64>>; 2] = Default::default();
        let mut peak: [Option<(Vec<f64>, Vec<f64>)>; 2] = Default::default();
        for c in 0..2 {
            if self.mean[c].is_empty() {
                continue;
            }
            let k = p.k(c);
            let mut t = all(&self.mean[c]);
            condition(&mut t, 0.0, k);
            traces[c] = Some(t);
            let (mut lo, mut hi) = (all(&self.min[c]), all(&self.max[c]));
            condition(&mut lo, 0.0, k);
            condition(&mut hi, 0.0, k);
            peak[c] = Some(if k < 0.0 { (hi, lo) } else { (lo, hi) });
        }
        let n = traces[0]
            .as_ref()
            .or(traces[1].as_ref())
            .map_or(0, |v| v.len());
        let rate = if self.rate > 0.0 { self.rate } else { 1.0 };
        let t0 = -(n.saturating_sub(1) as f64) / rate;
        frame(p, traces, peak, rate, t0, false, true, self.clipped)
    }
}

/// Decimate raw interleaved bytes (one or two slices of a stream, from a
/// point boundary) to a block in volts at the BNC: `lines` per channel
/// (volts = a * count + b), min and max only with oversampling.
pub fn decimate_block(
    slices: [&[u8]; 2],
    cfg: &HwConfig,
    lines: [(f64, f64); 2],
    range: [(f64, f64); 2],
    gen: u64,
) -> Block {
    let ch = cfg.channels.clamp(1, 2);
    let os = cfg.rate.oversample.max(1);
    let n = (slices[0].len() + slices[1].len()) / (ch as usize * os) + 1;
    let peak = os > 1;
    let mut data: [Vec<f64>; 2] = Default::default();
    let mut min: [Vec<f64>; 2] = Default::default();
    let mut max: [Vec<f64>; 2] = Default::default();
    for c in 0..ch as usize {
        data[c].reserve(n);
        if peak {
            min[c].reserve(n);
            max[c].reserve(n);
        }
    }
    let mut dec = Decimator::new(ch, os, 0);
    let mut out = |c: usize, lo: u8, hi: u8, mean: f64| {
        let (a, b) = lines[c];
        data[c].push(a * mean + b);
        if peak {
            min[c].push(a * lo as f64 + b);
            max[c].push(a * hi as f64 + b);
        }
    };
    dec.push(slices[0], &mut out);
    dec.push(slices[1], &mut out);
    Block {
        gen,
        cfg: *cfg,
        data,
        min,
        max,
        clipped: dec.clipped,
        range,
        rate: cfg.rate.rate,
        roll: cfg.roll,
        trigger: None,
    }
}

/// What the processing hands to the screen: the newest frame. The screen
/// takes the newest one when it draws; one it did not take is replaced.
#[derive(Clone, Debug)]
pub struct Published {
    /// Hardware settings generation.
    pub gen: u64,
    /// Processing settings generation.
    pub pgen: u64,
    /// The frame to show. `None`: no new frame (waiting for a trigger);
    /// keep the one on screen.
    pub frame: Option<Frame>,
    pub state: TrigState,
    /// The block behind the frame in volts at the BNC, not averaged: the
    /// capture around the trigger (with its index), or the newest screen
    /// of the stream. For a redraw while stopped, zero calibration and
    /// autoset.
    pub snapshot: Option<Block>,
    /// Triggers since the stream or the processing settings changed.
    pub triggers: u64,
    /// Triggers in the average now.
    pub averaged: usize,
    /// Point index (since the stream started) of the frame's trigger.
    pub trigger_at: Option<f64>,
    /// Points of the stream processed so far.
    pub points: u64,
    /// The single trigger fired: the stream stops.
    pub single_done: bool,
    /// Input range at the BNC per channel (lowest and highest ADC value).
    pub range: [(f64, f64); 2],
    /// Mean AC coupling takes away per channel (volts at the BNC).
    pub ac_mean: [f64; 2],
}

/// Running min, max and mean of each channel over a screen of points.
#[derive(Clone, Copy, Debug)]
struct Stats {
    min: f64,
    max: f64,
    sum: f64,
    n: u64,
}

impl Stats {
    const EMPTY: Stats = Stats {
        min: f64::INFINITY,
        max: f64::NEG_INFINITY,
        sum: 0.0,
        n: 0,
    };

    fn add(&mut self, v: f64) {
        self.min = self.min.min(v);
        self.max = self.max.max(v);
        self.sum += v;
        self.n += 1;
    }

    fn mean(&self) -> f64 {
        self.sum / self.n.max(1) as f64
    }
}

/// A trigger waiting for the rest of its capture.
#[derive(Clone, Copy, Debug)]
struct Pending {
    t: f64,
    /// First point of the capture.
    start: u64,
}

/// The stream processing for one stream (one hardware setting).
pub struct Pipeline {
    pub gen: u64,
    pub pgen: u64,
    hw: HwConfig,
    cal: Calibration,
    lines: [(f64, f64); 2],
    range: [(f64, f64); 2],
    p: ProcSettings,
    ch: usize,
    os: u64,
    /// The first point after the unstable start of the stream.
    first: u64,
    dec: Decimator,
    /// Raw frames decimated so far.
    frame_pos: u64,
    /// Points made so far, per channel.
    pts: [u64; 2],
    /// Channel the trigger looks at (the source, else the one sampled).
    src: usize,
    scanner: EdgeScanner,
    stats: [Stats; 2],
    /// Stats of the last whole screen.
    last_stats: Option<[Stats; 2]>,
    pending: VecDeque<Pending>,
    avg: Averaging,
    /// The last capture, cut, and its block.
    last: Option<(Cut, Block, f64)>,
    /// A capture came since the last publish.
    dirty: bool,
    triggers: u64,
    /// Point of the last trigger (or of the start).
    last_trigger: u64,
    /// Point of the last untriggered or waiting publish.
    last_status: u64,
    last_publish: Option<Instant>,
    /// Single: the first point a trigger may be at.
    single_from: Option<f64>,
    single_done: bool,
    roll: Roll,
    /// Shortest time between two publishes ([`FRAME_TIME`]; tests 0).
    pub min_publish: Duration,
    /// Every accepted trigger, as a point index (tests).
    pub trigger_log: Option<Vec<f64>>,
}

impl Pipeline {
    /// For a stream of `hw` with the calibration `cal`. `newest` is the
    /// newest frame in the ring now (the single trigger waits for an edge
    /// after it).
    pub fn new(
        gen: u64,
        pgen: u64,
        hw: HwConfig,
        cal: &Calibration,
        p: ProcSettings,
        newest: u64,
    ) -> Self {
        let ch = hw.channels.clamp(1, 2) as usize;
        let os = hw.rate.oversample.max(1) as u64;
        let first = (SKIP_SAMPLES as u64).div_ceil(os);
        let mut me = Self {
            gen,
            pgen,
            hw,
            cal: cal.clone(),
            lines: [(1.0, 0.0); 2],
            range: [(0.0, 0.0); 2],
            p: p.clone(),
            ch,
            os,
            first,
            dec: Decimator::new(ch as u8, os as usize, (first * os) as usize),
            frame_pos: 0,
            pts: [first; 2],
            src: 0,
            scanner: EdgeScanner::new(0.0, Slope::Rising, 0.0, first),
            stats: [Stats::EMPTY; 2],
            last_stats: None,
            pending: VecDeque::new(),
            avg: Averaging::default(),
            last: None,
            dirty: false,
            triggers: 0,
            last_trigger: first,
            last_status: first,
            last_publish: None,
            single_from: None,
            single_done: false,
            roll: Roll::default(),
            min_publish: FRAME_TIME,
            trigger_log: None,
        };
        me.set_proc(pgen, p, newest);
        me
    }

    pub fn hw(&self) -> &HwConfig {
        &self.hw
    }

    /// New processing settings: start the trigger search, the average and
    /// the trigger count over (the stream goes on).
    pub fn set_proc(&mut self, pgen: u64, p: ProcSettings, newest: u64) {
        let arm_changed = p.arm != self.p.arm;
        self.pgen = pgen;
        self.p = p;
        self.cal.app = self.p.cal;
        let hs = self.hw.high_speed();
        self.lines = [0, 1].map(|c| self.cal.linear(self.hw.gains[c], c, hs));
        self.range = [0, 1].map(|c| self.cal.range(self.hw.gains[c], c, hs));
        let src = self.p.source.min(1);
        self.src = if src < self.ch { src } else { 0 };
        let at = self.pts[self.src];
        self.scanner = EdgeScanner::new(0.0, Slope::Rising, 0.0, at);
        self.retune();
        self.pending.clear();
        self.avg.reset();
        self.last = None;
        self.dirty = false;
        self.triggers = 0;
        self.last_trigger = at;
        self.last_status = at;
        if self.p.mode == TriggerMode::Single && (arm_changed || !self.single_done) {
            self.single_done = false;
            self.single_from = Some(newest as f64 / self.os as f64);
        } else if self.p.mode != TriggerMode::Single {
            self.single_from = None;
            self.single_done = false;
        }
    }

    /// The mean AC coupling takes away, per channel, in volts at the BNC.
    fn ac_mean(&self) -> [f64; 2] {
        [0, 1].map(
            |c| match (&self.last_stats, self.p.ac[c] && !self.hw.roll) {
                (Some(s), true) if s[c].n > 0 => {
                    let (a, b) = self.lines[c];
                    a * s[c].mean() + b
                }
                _ => 0.0,
            },
        )
    }

    /// Set the scanner's level, hysteresis and slope (in ADC counts) from
    /// the settings and the signal of the last screen.
    fn retune(&mut self) {
        let c = self.src;
        let (a, b) = self.lines[c];
        // The user's source may not be sampled; the probe of the channel
        // that is, the volts/div of the user's source (like the screen).
        let k = self.p.k(c);
        let vdiv = self.p.vdiv[self.p.source.min(1)];
        let st = self.last_stats.map(|s| s[c]);
        let ac = self.ac_mean()[c];
        let bnc = self.p.level / k + ac;
        let mut level = (bnc - b) / a;
        // A level outside the input range is never crossed: use one 5 %
        // of the range inside the nearest edge.
        let inside = TRIGGER_INSIDE * 255.0;
        if level > 255.0 {
            level = 255.0 - inside;
        } else if level < 0.0 {
            level = inside;
        }
        let pp = st.filter(|s| s.n > 0).map_or(0.0, |s| s.max - s.min);
        let hyst = (0.1 * vdiv / (a * k).abs()).min((0.25 * pp).max(2.0));
        self.scanner.set_level(level, hyst);
        self.scanner.slope = match (k < 0.0, self.p.slope) {
            (true, Slope::Rising) => Slope::Falling,
            (true, Slope::Falling) => Slope::Rising,
            (_, s) => s,
        };
    }

    /// Points of the trigger channel made so far.
    pub fn points(&self) -> u64 {
        self.pts[self.src]
    }

    pub fn triggers(&self) -> u64 {
        self.triggers
    }

    /// The single trigger fired: stop the stream.
    pub fn single_done(&self) -> bool {
        self.single_done
    }

    /// Process what the ring holds now. `out` gets each published frame.
    /// Tells the ring which frames are still needed.
    pub fn step(&mut self, ring: &Ring, out: &mut dyn FnMut(Box<Published>)) {
        let end = ring.frames();
        while self.frame_pos < end && !self.single_done {
            let to = end.min(self.frame_pos + CHUNK_FRAMES);
            if self.hw.roll {
                self.roll_chunk(ring, to);
            } else {
                self.scan_chunk(ring, to);
                self.captures(ring);
            }
            self.frame_pos = to;
            self.publish(ring, out, false);
        }
        self.publish(ring, out, true);
        ring.keep_from(self.keep());
    }

    /// The first raw frame still needed.
    fn keep(&self) -> u64 {
        if self.hw.roll {
            // Roll mode takes each point as it comes.
            return self.frame_pos;
        }
        let (d, _, _) = self.p.window(self.hw.rate.rate);
        let mut p = self.points().saturating_sub(capture_len(d) as u64);
        if let Some(c) = self.pending.front() {
            p = p.min(c.start);
        }
        p * self.os
    }

    /// Decimate raw frames up to `to` into points; look for triggers.
    fn scan_chunk(&mut self, ring: &Ring, to: u64) {
        let (d, pre, _) = self.p.window(self.hw.rate.rate);
        let len = capture_len(d) as u64;
        let screen = d as u64;
        let src = self.src;
        let first = self.first;
        let frames = self.frame_pos..to;
        let Self {
            dec,
            scanner,
            stats,
            last_stats,
            pts,
            pending,
            single_from,
            trigger_log,
            ..
        } = self;
        let mut retune = false;
        let mut cb = |c: usize, _lo: u8, _hi: u8, mean: f64| {
            pts[c] += 1;
            let s = &mut stats[c];
            s.add(mean);
            if c != src {
                return;
            }
            if s.n >= screen {
                *last_stats = Some(*stats);
                *stats = [Stats::EMPTY; 2];
                retune = true;
            }
            if let Some(t) = scanner.step(mean) {
                let tf = t.floor() as u64;
                let ok_start = tf >= first + (pre + MARGIN) as u64;
                let ok_single = single_from.is_none_or(|a| t >= a);
                if ok_start && ok_single {
                    let start = tf - (pre + MARGIN) as u64;
                    pending.push_back(Pending { t, start });
                    // Rearm at the end of the capture.
                    scanner.hold_until((start + len) as f64);
                    if let Some(log) = trigger_log {
                        log.push(t);
                    }
                    if single_from.is_some() {
                        // One is enough; later edges are not wanted.
                        scanner.hold_until(f64::INFINITY);
                    }
                }
            }
        };
        ring.read(frames, |a, b| {
            dec.push(a, &mut cb);
            dec.push(b, &mut cb);
        });
        if retune {
            self.retune();
        }
    }

    /// Cut the captures whose points are all there.
    fn captures(&mut self, ring: &Ring) {
        let (d, _, _) = self.p.window(self.hw.rate.rate);
        let len = capture_len(d) as u64;
        while let Some(c) = self.pending.front().copied() {
            if c.start + len > self.points() {
                break;
            }
            self.pending.pop_front();
            let mut block = self.block(ring, c.start, len as usize);
            let rel = c.t - c.start as f64;
            block.trigger = Some(rel);
            let cut = cut(&block, &self.p, self.ac_mean(), Some(rel));
            self.avg.add(&cut, self.p.averaging);
            self.triggers += 1;
            self.last_trigger = c.t as u64;
            self.last = Some((cut, block, c.t));
            self.dirty = true;
            if self.single_from.is_some() {
                self.single_done = true;
                self.pending.clear();
                break;
            }
        }
    }

    /// The points `start .. start + len` as a block in volts at the BNC.
    fn block(&self, ring: &Ring, start: u64, len: usize) -> Block {
        let frames = start * self.os..(start + len as u64) * self.os;
        ring.read(frames, |a, b| {
            decimate_block([a, b], &self.hw, self.lines, self.range, self.gen)
        })
    }

    /// Publish what is due: the averaged frame after a trigger, at most
    /// once per [`Pipeline::min_publish`]; without triggers an untriggered
    /// frame (Auto, after a timeout) or a waiting state, once per screen.
    /// `last`: the end of a step; else only when much is due.
    fn publish(&mut self, ring: &Ring, out: &mut dyn FnMut(Box<Published>), last: bool) {
        let now = Instant::now();
        let due = self
            .last_publish
            .is_none_or(|t| now.duration_since(t) >= self.min_publish);
        if !due || (!last && !self.single_done && !self.hw.roll && !self.dirty) {
            return;
        }
        let rate = self.hw.rate.rate;
        let (d, _, _) = self.p.window(rate);
        let pts = self.points();
        let base = Published {
            gen: self.gen,
            pgen: self.pgen,
            frame: None,
            state: TrigState::Waiting,
            snapshot: None,
            triggers: self.triggers,
            averaged: self.avg.count(),
            trigger_at: None,
            points: pts,
            single_done: self.single_done,
            range: self.range,
            ac_mean: self.ac_mean(),
        };
        if self.hw.roll {
            if self.roll.mean.iter().all(|m| m.is_empty()) {
                return;
            }
            let frame = self.roll.frame(&self.p);
            self.last_publish = Some(now);
            out(Box::new(Published {
                frame: Some(frame),
                state: TrigState::Roll,
                ..base
            }));
            return;
        }
        if self.dirty {
            let (cut, block, t) = self.last.as_ref().expect("a capture");
            let v = self.avg.value(cut, self.p.averaging);
            let frame = frame(
                &self.p,
                v.traces,
                v.peak,
                rate,
                v.t0,
                true,
                false,
                block.clipped,
            );
            self.dirty = false;
            self.last_publish = Some(now);
            out(Box::new(Published {
                frame: Some(frame),
                state: TrigState::Triggered,
                snapshot: Some(block.clone()),
                trigger_at: Some(*t),
                ..base
            }));
            return;
        }
        // No new trigger: once per screen of the stream.
        let len = capture_len(d) as u64;
        if self.single_done || pts < self.last_status + d as u64 || pts < self.first + len {
            return;
        }
        let quiet = pts - self.last_trigger;
        let timeout = (0.1f64.max(2.0 * self.p.screen_time) * rate) as u64;
        let auto = self.p.mode == TriggerMode::Auto && quiet >= timeout;
        if !auto && self.last.is_some() && self.p.mode == TriggerMode::Auto {
            // Auto keeps the last trigger's frame until the timeout.
            return;
        }
        let start = pts - len;
        if start < ring.kept().div_ceil(self.os) {
            return;
        }
        let block = self.block(ring, start, len as usize);
        self.last_status = pts;
        self.last_publish = Some(now);
        let (frame, state) = if auto {
            let c = cut(&block, &self.p, self.ac_mean(), None);
            let f = frame(
                &self.p,
                c.traces,
                c.peak,
                rate,
                c.t0,
                false,
                false,
                block.clipped,
            );
            (Some(f), TrigState::Auto)
        } else {
            (None, TrigState::Waiting)
        };
        out(Box::new(Published {
            frame,
            state,
            snapshot: Some(block),
            ..base
        }));
    }

    /// Roll mode: decimate up to `to` and append to the roll screen.
    fn roll_chunk(&mut self, ring: &Ring, to: u64) {
        let lines = self.lines;
        let peak = self.os > 1;
        let frames = self.frame_pos..to;
        let mut block = Block {
            gen: self.gen,
            cfg: self.hw,
            data: Default::default(),
            min: Default::default(),
            max: Default::default(),
            clipped: [false; 2],
            range: self.range,
            rate: self.hw.rate.rate,
            roll: true,
            trigger: None,
        };
        let Self { dec, pts, .. } = self;
        dec.clipped = [false; 2];
        let (data, min, max) = (&mut block.data, &mut block.min, &mut block.max);
        let mut cb = |c: usize, lo: u8, hi: u8, mean: f64| {
            pts[c] += 1;
            let (a, b) = lines[c];
            data[c].push(a * mean + b);
            if peak {
                min[c].push(a * lo as f64 + b);
                max[c].push(a * hi as f64 + b);
            }
        };
        ring.read(frames, |a, b| {
            dec.push(a, &mut cb);
            dec.push(b, &mut cb);
        });
        block.clipped = self.dec.clipped;
        self.roll.push(&block, self.p.screen_time);
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default, clippy::vec_box)]
mod tests {
    use super::*;
    use crate::device::fake::{FakeScope, Wave};
    use crate::device::protocol::{choose_rate, MAX_ADC_DEFAULT};
    use std::sync::Arc;

    /// Raw stream bytes of the fake scope for `cfg`.
    fn raw(fake: &FakeScope, cfg: &HwConfig, frames: usize) -> Vec<u8> {
        fake.raw(cfg, frames)
    }

    /// The calibration of the fake scope's EEPROM (all fakes have the same).
    fn fake_cal() -> Calibration {
        let f = FakeScope::new([Wave::Dc(0.0); 2]);
        let e = f.lock().eeprom;
        Calibration::from_eeprom(&e[8..88])
    }

    fn hw(screen: f64, channels: u8) -> HwConfig {
        HwConfig {
            rate: choose_rate(screen, channels, MAX_ADC_DEFAULT),
            channels,
            gains: [5, 5],
            dc: [true; 2],
            cal_freq: 1000,
            roll: false,
        }
    }

    fn settings(time_div: f64, avg: usize) -> Settings {
        let mut s = Settings::default();
        s.time_idx = crate::scope::time_index(time_div);
        s.display.averaging = avg;
        s.trigger.level = 1.0;
        s
    }

    /// Run the pipeline over `bytes` written in pieces of `piece`.
    fn run(
        cfg: HwConfig,
        p: ProcSettings,
        bytes: &[u8],
        piece: usize,
        ring_size: usize,
    ) -> (Pipeline, Vec<Box<Published>>) {
        let ring = Arc::new(Ring::new(ring_size));
        let w = ring.reset(cfg.channels);
        let cal = fake_cal();
        let mut pl = Pipeline::new(1, 1, cfg, &cal, p, 0);
        pl.min_publish = Duration::ZERO;
        pl.trigger_log = Some(Vec::new());
        let mut got = Vec::new();
        for c in bytes.chunks(piece) {
            assert!(w.write(c, Instant::now(), 0), "ring overrun");
            pl.step(&ring, &mut |p| got.push(p));
        }
        (pl, got)
    }

    fn cal_fake() -> FakeScope {
        let f = FakeScope::new([Wave::CalOut, Wave::CalOut]);
        f.lock().noise = 0.3;
        f
    }

    /// Point index of the rising edges of the 1 kHz cal wave after
    /// each rearm (capture end), from the stream start.
    fn expected(rate: f64, first: f64, d: usize, pre: usize) -> impl Fn(f64) -> f64 {
        let period = rate / 1000.0;
        move |prev: f64| {
            // The next edge at or after the rearm point.
            let rearm = prev.floor() - (pre + MARGIN) as f64 + capture_len(d) as f64;
            let from = rearm.max(first);
            // Rising edges of CalOut are at the period starts.
            (from / period).ceil() * period
        }
    }

    #[test]
    fn every_trigger_is_the_first_edge_after_rearm() {
        for (screen_div, chans) in [(1e-3, 2u8), (1e-4, 2), (1e-6, 1), (1e-4, 1)] {
            let s = settings(screen_div, 1);
            let cfg = hw(s.coarse_screen_time(), chans);
            let fake = cal_fake();
            let bytes = raw(&fake, &cfg, 600_000);
            let p = ProcSettings::of(&s, 0);
            for piece in [4093usize, 65_536, 1 << 20] {
                let (pl, _) = run(cfg, p.clone(), &bytes, piece, 1 << 24);
                let log = pl.trigger_log.clone().unwrap();
                assert!(
                    log.len() >= 2,
                    "{screen_div} {chans}: {} triggers",
                    log.len()
                );
                let (d, pre, _) = p.window(cfg.rate.rate);
                let next = expected(cfg.rate.rate, pl.first as f64, d, pre);
                for w in log.windows(2) {
                    let want = next(w[0]);
                    // The edge is between two points (the crossing of
                    // the line from one to the next).
                    assert!(
                        (w[1] - want).abs() <= 1.0,
                        "{screen_div} {chans} piece {piece}: {} after {}, want {want}",
                        w[1],
                        w[0]
                    );
                }
                assert_eq!(pl.triggers(), log.len() as u64 - pl.pending.len() as u64);
            }
        }
    }

    #[test]
    fn pieces_of_any_size_give_the_same_stream() {
        let s = settings(1e-4, 4);
        let cfg = hw(s.coarse_screen_time(), 2);
        let fake = cal_fake();
        let bytes = raw(&fake, &cfg, 120_000);
        let p = ProcSettings::of(&s, 0);
        let (whole, out) = run(cfg, p.clone(), &bytes, bytes.len(), 1 << 20);
        let want = whole.trigger_log.clone().unwrap();
        assert!(want.len() > 3);
        let last = out.iter().rev().find_map(|p| p.frame.clone()).unwrap();
        for piece in [1usize, 3, 511, 512, 4097] {
            let (pl, out) = run(cfg, p.clone(), &bytes, piece, 1 << 20);
            assert_eq!(pl.trigger_log.clone().unwrap(), want, "piece {piece}");
            let f = out.iter().rev().find_map(|p| p.frame.clone()).unwrap();
            assert_eq!(f.traces[0], last.traces[0], "piece {piece}");
            assert_eq!(f.peak[1], last.peak[1], "piece {piece}");
        }
    }

    #[test]
    fn averaging_counts_the_last_triggers() {
        let s = settings(1e-4, 64);
        let cfg = hw(s.coarse_screen_time(), 2);
        let fake = cal_fake();
        let p = ProcSettings::of(&s, 0);
        let (d, _, _) = p.window(cfg.rate.rate);
        // Enough for well over 64 captures (one per two periods).
        assert!(capture_len(d) as f64 <= 1.1 * cfg.rate.rate / 1000.0);
        let frames = (0.16 * cfg.rate.raw) as usize;
        let bytes = raw(&fake, &cfg, frames);
        let (pl, out) = run(cfg, p, &bytes, 1 << 16, 1 << 26);
        assert!(pl.triggers() >= 64, "{}", pl.triggers());
        let last = out.last().unwrap();
        assert_eq!(last.averaged, 64);
        assert_eq!(last.triggers, pl.triggers());
        // Before that, all triggers so far are in the average.
        for p in out.iter().filter(|p| p.frame.is_some()) {
            assert_eq!(p.averaged as u64, p.triggers.min(64));
        }
    }

    #[test]
    fn auto_shows_untriggered_frames_after_the_timeout() {
        // 0.5 V DC never crosses the 1 V level.
        let s = settings(1e-3, 1);
        let cfg = hw(s.coarse_screen_time(), 2);
        let fake = FakeScope::new([Wave::Dc(0.5), Wave::Dc(0.0)]);
        let bytes = raw(&fake, &cfg, (0.2 * cfg.rate.raw) as usize);
        let p = ProcSettings::of(&s, 0);
        let (pl, out) = run(cfg, p.clone(), &bytes, 1 << 16, 1 << 24);
        assert_eq!(pl.triggers(), 0);
        let rate = cfg.rate.rate;
        let first = out.iter().find(|p| p.frame.is_some()).unwrap();
        assert_eq!(first.state, TrigState::Auto);
        let timeout = (0.1f64.max(2.0 * p.screen_time) * rate) as u64;
        assert!(first.points >= timeout + pl.first, "{}", first.points);
        assert!(!first.frame.as_ref().unwrap().triggered);
        // Normal mode waits and shows nothing new.
        let mut n = p.clone();
        n.mode = TriggerMode::Normal;
        let (_, out) = run(cfg, n, &bytes, 1 << 16, 1 << 24);
        assert!(out.iter().all(|p| p.frame.is_none()));
        assert!(out.iter().all(|p| p.state == TrigState::Waiting));
        assert!(out.iter().all(|p| p.snapshot.is_some()));
    }

    #[test]
    fn single_takes_the_first_edge_after_the_arm() {
        let s = {
            let mut s = settings(1e-4, 1);
            s.trigger.mode = TriggerMode::Single;
            s
        };
        let cfg = hw(s.coarse_screen_time(), 2);
        let fake = cal_fake();
        let bytes = raw(&fake, &cfg, 200_000);
        let ring = Arc::new(Ring::new(1 << 22));
        let w = ring.reset(2);
        // Armed after 50 000 frames: the first edge after that point.
        w.write(&bytes[..100_000], Instant::now(), 0);
        let cal = fake_cal();
        let p = ProcSettings::of(&s, 1);
        let mut pl = Pipeline::new(1, 1, cfg, &cal, p, ring.frames());
        pl.min_publish = Duration::ZERO;
        pl.trigger_log = Some(Vec::new());
        let mut got = Vec::new();
        w.write(&bytes[100_000..], Instant::now(), 0);
        pl.step(&ring, &mut |p| got.push(p));
        let log = pl.trigger_log.clone().unwrap();
        assert_eq!(log.len(), 1);
        let period = cfg.rate.rate / 1000.0;
        let a = 50_000.0 / cfg.rate.oversample as f64;
        let want = (a / period).ceil() * period;
        assert!((log[0] - want).abs() <= 1.0, "{} vs {want}", log[0]);
        assert!(pl.single_done());
        let last = got.last().unwrap();
        assert!(last.single_done && last.frame.is_some());
        assert_eq!(last.triggers, 1);
    }

    #[test]
    fn roll_appends_the_stream() {
        let mut s = Settings::default();
        s.time_idx = crate::scope::time_index(0.2);
        let mut cfg = hw(s.coarse_screen_time(), 2);
        cfg.roll = true;
        let fake = FakeScope::new([Wave::Dc(1.0), Wave::Dc(-1.0)]);
        fake.lock().noise = 0.0;
        let p = ProcSettings::of(&s, 0);
        let os = cfg.rate.oversample;
        let bytes = raw(&fake, &cfg, os * 3000 + 4096);
        let (_, out) = run(cfg, p, &bytes, 1 << 16, 1 << 24);
        let f = out.last().unwrap().frame.clone().unwrap();
        assert!(f.roll);
        let n = f.traces[0].as_ref().unwrap().len();
        assert!(n >= 2900, "{n}");
        assert!(f.traces[0]
            .as_ref()
            .unwrap()
            .iter()
            .all(|v| (v - 1.0).abs() < 0.05));
        assert!(f.traces[1]
            .as_ref()
            .unwrap()
            .iter()
            .all(|v| (v + 1.0).abs() < 0.05));
    }

    #[test]
    fn decimator_and_scanner_keep_up_with_the_stream() {
        // Loose: the processing must be well above 60 MS/s of raw samples
        // (32 MB/s of USB data) in an optimized (opt-level 1) build.
        let s = settings(1e-6, 1);
        let cfg = hw(s.coarse_screen_time(), 1);
        let fake = cal_fake();
        let bytes = raw(&fake, &cfg, 8 << 20);
        let p = ProcSettings::of(&s, 0);
        let mut best: f64 = 0.0;
        // The best of three, as other tests run at the same time.
        for _ in 0..3 {
            let ring = Arc::new(Ring::new(1 << 24));
            let w = ring.reset(1);
            let cal = fake_cal();
            let mut pl = Pipeline::new(1, 1, cfg, &cal, p.clone(), 0);
            let t0 = Instant::now();
            for c in bytes.chunks(256 << 10) {
                w.write(c, Instant::now(), 0);
                pl.step(&ring, &mut |_| {});
            }
            best = best.max(bytes.len() as f64 / t0.elapsed().as_secs_f64());
            assert!(pl.triggers() > 100);
        }
        eprintln!("pipeline: {:.0} MS/s", best / 1e6);
        assert!(best > 60e6, "{:.1} MS/s", best / 1e6);
    }
}
