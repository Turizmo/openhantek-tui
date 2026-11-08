//! Tests with the real scope. Run with:
//! `cargo test --release --test hardware -- --ignored --test-threads=1 --nocapture`
//! Close OpenHantek6022 first. A probe on the calibration pin makes the
//! signal checks stronger; without it they only check that capture works.
//!
//! The gapless stream tests (`hw_stream_*`, `hw_log_*`) need the CH1 probe
//! on the calibration pin: they count the 1 kHz periods. They run for
//! `OHT_HW_SECS` seconds each (default 20). The log tests write their log
//! to the system temp folder (up to 48 MB/s) and delete it afterwards.

use std::time::{Duration, Instant};

use openhantek_tui::acquire::{Acquisition, Command, LogRequest};
use openhantek_tui::device::protocol::{
    choose_rate, gain_index, MAX_ADC_DEFAULT, raw_rate_of_id, HwConfig, Rate, BLOCK_SAMPLES,
};
use openhantek_tui::device::{open_hardware, Connection};
use openhantek_tui::dsp::measure::measure;

fn open() -> Connection {
    open_hardware(None, &mut |s| eprintln!("status: {s}")).expect("open the real scope")
}

fn config(time_per_div: f64, volts_per_div: f64, channels: u8, cal_freq: u32) -> HwConfig {
    let g = gain_index(volts_per_div);
    HwConfig {
        rate: choose_rate(time_per_div * 10.0, channels, MAX_ADC_DEFAULT),
        channels,
        gains: [g, g],
        dc: [true; 2],
        cal_freq,
        roll: false,
    }
}

#[test]
#[ignore = "needs the real DSO-6022BE"]
fn hw_capture_two_channels() {
    let mut c = open();
    eprintln!("opened {} notes {:?}", c.name, c.notes);
    let cfg = config(1e-3, 1.0, 2, 1000);
    // First block after a config change can be odd: take a few.
    let mut last = None;
    for _ in 0..3 {
        last = Some(c.driver.capture(&cfg).unwrap());
    }
    let block = last.unwrap();
    assert_eq!(block.data[0].len(), BLOCK_SAMPLES);
    assert_eq!(block.data[1].len(), BLOCK_SAMPLES);
    for ch in 0..2 {
        let m = measure(&block.data[ch], 1.0 / block.rate);
        eprintln!(
            "CH{} min {:.3} max {:.3} mean {:.3} vpp {:.3} freq {:?} duty {:?} clipped {}",
            ch + 1,
            m.min,
            m.max,
            m.mean,
            m.pk_pk,
            m.freq,
            m.duty,
            block.clipped[ch]
        );
        // Inputs must read inside the ADC range.
        assert!(m.min > -5.3 && m.max < 5.3);
        if m.pk_pk > 1.0 {
            // A probe is on the calibration pin: 1 kHz square, about 2 Vpp.
            let f = m.freq.expect("a frequency on a 2 Vpp signal");
            assert!((f - 1000.0).abs() < 20.0, "freq {f}");
            assert!((m.pk_pk - 2.0).abs() < 0.5, "vpp {}", m.pk_pk);
        } else {
            eprintln!("CH{}: flat signal, only capture checked", ch + 1);
        }
    }
    c.driver.stop().unwrap();
}

#[test]
#[ignore = "needs the real DSO-6022BE"]
fn hw_calibration_frequency_follows_setting() {
    let mut c = open();
    let mut freqs = Vec::new();
    for hz in [1000u32, 2000, 500] {
        let cfg = config(1e-3, 1.0, 2, hz);
        let mut block = None;
        for _ in 0..3 {
            block = Some(c.driver.capture(&cfg).unwrap());
        }
        let block = block.unwrap();
        let m = measure(&block.data[0], 1.0 / block.rate);
        eprintln!("cal {hz} Hz: CH1 vpp {:.3} freq {:?}", m.pk_pk, m.freq);
        if m.pk_pk < 1.0 {
            eprintln!("CH1 flat: no probe on the calibration pin, skipping the check");
            c.driver.stop().unwrap();
            return;
        }
        freqs.push((hz, m.freq.unwrap()));
    }
    for (want, got) in freqs {
        assert!(
            (got - want as f64).abs() < want as f64 * 0.02,
            "{want} Hz: got {got}"
        );
    }
    c.driver.stop().unwrap();
}

#[test]
#[ignore = "needs the real DSO-6022BE"]
fn hw_fast_single_channel_and_gains() {
    let mut c = open();
    for (tdiv, vdiv) in [(20e-6, 0.5), (100e-6, 0.1), (10e-3, 2.0)] {
        let cfg = config(tdiv, vdiv, 1, 1000);
        let mut block = None;
        for _ in 0..2 {
            block = Some(c.driver.capture(&cfg).unwrap());
        }
        let block = block.unwrap();
        let m = measure(&block.data[0], 1.0 / block.rate);
        eprintln!(
            "{tdiv} s/div {vdiv} V/div rate {} S/s: min {:.3} max {:.3} freq {:?} clipped {}",
            block.rate, m.min, m.max, m.freq, block.clipped[0]
        );
        assert_eq!(block.data[0].len(), BLOCK_SAMPLES);
        assert!(block.data[1].is_empty());
    }
    c.driver.stop().unwrap();
}

#[test]
#[ignore = "needs the real DSO-6022BE"]
fn hw_roll_stream() {
    let mut c = open();
    // 200 ms/div: 5 kS/s. A 100 Hz calibration signal has 50 samples per period.
    let mut cfg = config(200e-3, 1.0, 2, 100);
    cfg.roll = true;
    let mut ch1 = Vec::new();
    let t0 = std::time::Instant::now();
    while t0.elapsed().as_secs_f64() < 2.0 {
        let piece = c.driver.stream(&cfg).unwrap();
        ch1.extend_from_slice(&piece.data[0]);
    }
    let secs = t0.elapsed().as_secs_f64();
    eprintln!(
        "roll: {} samples in {secs:.2} s at {} S/s",
        ch1.len(),
        cfg.rate.rate
    );
    // The stream is gapless now: all samples but the 2048 raw ones thrown
    // away at the start, less the start-up time and one transfer.
    assert!(
        ch1.len() as f64 > cfg.rate.rate * (secs - 0.15),
        "only {} samples",
        ch1.len()
    );
    let m = measure(&ch1, 1.0 / cfg.rate.rate);
    eprintln!("roll CH1 vpp {:.3} freq {:?}", m.pk_pk, m.freq);
    if m.pk_pk > 1.0 {
        // Lost samples would make the frequency read high.
        let f = m.freq.unwrap();
        assert!((f - 100.0).abs() < 0.5, "roll freq {f}");
    }
    c.driver.stop().unwrap();
}

/// Seconds for each long hardware run.
fn hw_secs() -> f64 {
    std::env::var("OHT_HW_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20.0)
}

/// CPU seconds (user + system) of this process so far.
fn cpu_secs() -> f64 {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out pointer.
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    let tv = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 * 1e-6;
    tv(ru.ru_utime) + tv(ru.ru_stime)
}

/// Rising edges of a 0 V / 2 V square wave, with hysteresis, across
/// pieces. Counts periods and the ones that are off by more than `slack`.
struct Edges {
    expect: f64,
    slack: f64,
    armed: bool,
    idx: u64,
    last: Option<u64>,
    periods: u64,
    breaks: Vec<(u64, i64)>,
    lo: f64,
    hi: f64,
}

impl Edges {
    fn new(expect: f64, slack: f64) -> Self {
        Self {
            expect,
            slack,
            armed: false,
            idx: 0,
            last: None,
            periods: 0,
            breaks: Vec::new(),
            lo: f64::INFINITY,
            hi: f64::NEG_INFINITY,
        }
    }

    fn push(&mut self, v: f64) {
        self.lo = self.lo.min(v);
        self.hi = self.hi.max(v);
        if v < 0.6 {
            self.armed = true;
        } else if self.armed && v > 1.4 {
            self.armed = false;
            if let Some(e) = self.last {
                let d = (self.idx - e) as f64;
                self.periods += 1;
                if (d - self.expect).abs() > self.slack && self.breaks.len() < 1000 {
                    self.breaks.push((self.idx, (d - self.expect) as i64));
                }
            }
            self.last = Some(self.idx);
        }
        self.idx += 1;
    }

    /// Skip `n` samples (a known gap): the next period does not count.
    fn skip(&mut self, n: u64) {
        self.idx += n;
        self.last = None;
    }
}

/// The async stream at `rate_id` with `channels`, through the driver:
/// count the samples against the clock and every period of the 1 kHz
/// calibration signal on CH1.
fn stream_run(rate_id: u8, channels: u8) {
    let mut c = open();
    let raw = raw_rate_of_id(rate_id);
    let mut cfg = config(1e-3, 1.0, channels, 1000);
    cfg.rate = Rate {
        rate: raw,
        id: rate_id,
        raw,
        oversample: 1,
    };
    cfg.roll = true;
    let secs = hw_secs();
    // The calibration output is not exactly 1 kHz in samples (at 16 MS/s
    // one period in 7 is 4 samples short), but a lost USB packet takes
    // 512 bytes: 256 samples with two channels, 512 with one. So a period
    // off by more than a quarter packet means lost samples.
    let mut edges = Edges::new(raw / 1000.0, 128.0 / channels as f64);
    let (mut n, mut lost, mut pieces) = (0u64, 0u64, 0u64);
    let mut first: Option<Instant> = None;
    let mut last = Instant::now();
    let cpu0 = cpu_secs();
    let t0 = Instant::now();
    while t0.elapsed().as_secs_f64() < secs {
        let p = c.driver.stream(&cfg).unwrap();
        let at = p.at.unwrap();
        if p.data[0].is_empty() {
            continue;
        }
        if p.lost > 0 {
            lost += p.lost;
            edges.skip(p.lost);
        }
        for v in &p.data[0] {
            edges.push(*v);
        }
        match first {
            // The first piece sets the time; its samples do not count.
            None => first = Some(at),
            Some(_) => n += p.data[0].len() as u64 + p.lost,
        }
        last = at;
        pieces += 1;
    }
    let wall = t0.elapsed().as_secs_f64();
    let cpu = cpu_secs() - cpu0;
    c.driver.stop().unwrap();
    let span = last.duration_since(first.unwrap()).as_secs_f64();
    let deficit = 1.0 - n as f64 / (span * raw);
    eprintln!(
        "stream {} MS/s x {channels}: {n} samples in {span:.3} s ({:.4} % short), \
         {pieces} pieces, host lost {lost}, CH1 {:.2}..{:.2} V, {} periods, {} breaks {:?}, \
         CPU {:.1} % of one core",
        raw / 1e6,
        deficit * 100.0,
        edges.lo,
        edges.hi,
        edges.periods,
        edges.breaks.len(),
        &edges.breaks[..edges.breaks.len().min(10)],
        cpu / wall * 100.0
    );
    assert!(
        edges.hi - edges.lo > 1.5,
        "no probe on the calibration pin?"
    );
    assert_eq!(lost, 0, "the host dropped samples");
    assert!(edges.breaks.is_empty(), "lost samples: {:?}", edges.breaks);
    // As many periods as the samples hold: none went missing.
    let want = edges.idx as f64 / edges.expect;
    assert!(
        (edges.periods as f64 - want).abs() < 3.0,
        "{} periods",
        edges.periods
    );
    // Clock check: within 500 ppm (the clocks differ; this scope in a VM
    // makes 150 ppm more samples than the PC clock says) plus a transfer.
    assert!(deficit.abs() < 500e-6 + 0.02 / span, "deficit {deficit}");
}

#[test]
#[ignore = "needs the real DSO-6022BE, CH1 probe on the calibration pin"]
fn hw_stream_16_ms_per_s_two_channels() {
    stream_run(16, 2);
}

#[test]
#[ignore = "needs the real DSO-6022BE, CH1 probe on the calibration pin"]
fn hw_stream_30_ms_per_s_ch1() {
    stream_run(30, 1);
}

/// A log at the fastest step (0.5 µs) through the app's acquisition
/// thread, logger and writer, for `OHT_HW_SECS`. Then read the file:
/// no gaps, as many steps as the clock says, and every 1 kHz period of the
/// calibration signal on CH1 exactly 2000 steps long.
fn log_run(name: &str, channels: [bool; 2]) {
    use openhantek_tui::logger::{self as lg, reader::LogFile, writer};
    let dir = std::env::temp_dir().join(format!("oht-hw-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("hw.ohlog");
    let step = 0.5e-6;
    let shared = writer::shared(path.clone(), 0);
    let g = gain_index(1.0);
    let req = LogRequest {
        shared: shared.clone(),
        step_ns: lg::step_ns(step),
        channels,
        probe: [1, 1],
        invert: [false, false],
        volts_per_div: [1.0, 1.0],
        gains: [g, g],
        dc: [true, true],
        cal_freq: 1000,
        max_adc: openhantek_tui::device::protocol::MAX_ADC_DEFAULT,
    };
    let (hw, per) = req.hw_config();
    let acq = Acquisition::spawn(Box::new(|status| open_hardware(None, status)));
    let secs = hw_secs();
    acq.send(Command::StartLog(req));
    let t0 = Instant::now();
    while shared.lock().unwrap().state == writer::LogState::Waiting {
        while acq.try_recv().is_some() {}
        assert!(t0.elapsed().as_secs() < 30, "no data from the scope");
        std::thread::sleep(Duration::from_millis(5));
    }
    let cpu0 = cpu_secs();
    let t1 = Instant::now();
    while t1.elapsed().as_secs_f64() < secs {
        while acq.try_recv().is_some() {}
        std::thread::sleep(Duration::from_millis(20));
    }
    let cpu = cpu_secs() - cpu0;
    let wall = t1.elapsed().as_secs_f64();
    acq.send(Command::StopLog);
    while !matches!(shared.lock().unwrap().state, writer::LogState::Done(_)) {
        while acq.try_recv().is_some() {}
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(acq);
    let f = LogFile::open(&path).unwrap();
    let bytes = std::fs::metadata(&path).unwrap().len();
    assert_eq!(f.header.adc_rate, hw.rate.raw);
    assert_eq!(f.header.samples_per_step, per);
    let gaps = f.gaps();
    // Every period of the calibration signal: 1 ms = 2000 steps, give or
    // take a step (it is not exactly 1 kHz). A lost USB packet is 32 or
    // 34 steps (256 samples of 8, 512 of 15).
    let mut edges = Edges::new(1e-3 / step, 8.0);
    let t2 = Instant::now();
    let end = f.end();
    let mut at = 0;
    while at < end {
        let to = (at + 4_000_000).min(end);
        f.for_each(at..to, |_, recs| {
            for r in recs {
                edges.push(r[0][2] as f64);
            }
        })
        .unwrap();
        at = to;
    }
    let expect = wall / step;
    eprintln!(
        "log {name}: {} MS/s x {}, {per} samples/step, {end} steps in {wall:.1} s \
         (clock says about {expect:.0}), {} gaps {:?}, {} MB ({:.1} MB/s), \
         CH1 {:.2}..{:.2} V, {} periods, {} breaks {:?}, CPU {:.1} % of one core, \
         read back in {:.1} s",
        hw.rate.raw / 1e6,
        hw.channels,
        gaps.len(),
        &gaps[..gaps.len().min(10)],
        bytes / 1_000_000,
        bytes as f64 / wall / 1e6,
        edges.lo,
        edges.hi,
        edges.periods,
        edges.breaks.len(),
        &edges.breaks[..edges.breaks.len().min(10)],
        cpu / wall * 100.0,
        t2.elapsed().as_secs_f64()
    );
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(
        edges.hi - edges.lo > 1.5,
        "no probe on the calibration pin?"
    );
    assert!(gaps.is_empty(), "{gaps:?}");
    assert!(edges.breaks.is_empty(), "lost samples: {:?}", edges.breaks);
    // The steps cover the recording time: the log also has the time from
    // the first data to the Recording state, and the stop takes a moment.
    let periods = end as f64 * step * 1000.0;
    assert!(
        (edges.periods as f64 - periods).abs() < 3.0,
        "{} periods",
        edges.periods
    );
    let low = (wall - 0.1) / step * (1.0 - 200e-6);
    assert!(
        end as f64 > low && (end as f64) < (wall + 0.5) / step,
        "{end} steps"
    );
}

#[test]
#[ignore = "needs the real DSO-6022BE, CH1 probe on the calibration pin"]
fn hw_log_fastest_two_channels() {
    log_run("2ch", [true, true]);
}

#[test]
#[ignore = "needs the real DSO-6022BE, CH1 probe on the calibration pin"]
fn hw_log_fastest_ch1() {
    log_run("1ch", [true, false]);
}
