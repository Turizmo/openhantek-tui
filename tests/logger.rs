//! The envelope logger on the fake scope: acquisition thread, binner,
//! writer and reader together.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::*;
use openhantek_tui::acquire::{Acquisition, Command, Event, LogRequest};
use openhantek_tui::device::fake::{FakeScope, Wave};
use openhantek_tui::logger::reader::LogFile;
use openhantek_tui::logger::writer::{self, LogState, Shared, StopReason};
use openhantek_tui::logger::{self as lg, pyramid::Pyramid};

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("oht-log-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn request(path: &Path, step: f64, channels: [bool; 2], limit: u64) -> (LogRequest, Shared) {
    let shared = writer::shared(path.to_path_buf(), limit);
    let req = LogRequest {
        shared: shared.clone(),
        step_ns: lg::step_ns(step),
        channels,
        probe: [1, 1],
        invert: [false, false],
        volts_per_div: [1.0, 1.0],
        gains: [5, 5],
        dc: [true, true],
        cal_freq: 1000,
    };
    (req, shared)
}

fn realtime_fake(waves: [Wave; 2]) -> FakeScope {
    let fake = FakeScope::new(waves);
    fake.lock().realtime = true;
    fake
}

/// Drain events so the thread never blocks on a full queue.
fn drain(acq: &Acquisition) {
    while acq.try_recv().is_some() {}
}

fn wait_state(acq: &Acquisition, sh: &Shared, secs: f64, ok: impl Fn(&LogState) -> bool) {
    let t0 = Instant::now();
    while !ok(&sh.lock().unwrap().state) {
        drain(acq);
        assert!(
            t0.elapsed().as_secs_f64() < secs,
            "{:?}",
            sh.lock().unwrap().state
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Record for `secs` seconds of data; returns the measured recording
/// time (from the first data to the stop).
fn record(acq: &Acquisition, req: LogRequest, secs: f64) -> f64 {
    let sh = req.shared.clone();
    acq.send(Command::StartLog(req));
    wait_state(acq, &sh, 10.0, |s| *s == LogState::Recording);
    let t0 = Instant::now();
    while t0.elapsed().as_secs_f64() < secs {
        drain(acq);
        std::thread::sleep(Duration::from_millis(10));
    }
    acq.send(Command::StopLog);
    let took = t0.elapsed().as_secs_f64();
    wait_state(acq, &sh, 10.0, |s| matches!(s, LogState::Done(_)));
    took
}

#[test]
fn records_without_gaps_at_1_ms() {
    let dir = temp_dir("1ms");
    let path = dir.join("a.ohlog");
    let fake = realtime_fake([Wave::Dc(1.0), SINE_1K]);
    let acq = Acquisition::spawn(opener(fake.clone()));
    let (req, sh) = request(&path, 1e-3, [true, true], 0);
    let took = record(&acq, req, 3.0);
    assert_eq!(sh.lock().unwrap().state, LogState::Done(StopReason::User));
    let f = LogFile::open(&path).unwrap();
    assert!(f.gaps().is_empty(), "{:?}", f.gaps());
    assert_eq!(f.header.samples_per_step, 20);
    assert_eq!(f.header.adc_rate, 20e3);
    // Steps match the time: the stream gives as many samples as the
    // clock says (within about one read of 100 ms each way).
    let want = took / 1e-3;
    let got = f.end() as f64;
    println!(
        "1 ms: {got} steps in {took:.3} s ({:.0} kS/s)",
        got * 20.0 / took / 1e3
    );
    assert!((got - want).abs() < 250.0, "{got} steps for {want}");
    let r = f.read(100..f.end()).unwrap();
    for (_, x) in &r {
        assert!((x[0][2] - 1.0).abs() < 0.08, "{x:?}");
    }
    // 1 kHz sine of 1.5 V: each 1 ms step has a whole period.
    let x = r[r.len() / 2].1;
    assert!(
        x[1][0] < -1.35 && x[1][1] > 1.35 && x[1][2].abs() < 0.1,
        "{x:?}"
    );
    // After the log, the scope goes back to blocks for the UI.
    let mut block = false;
    for _ in 0..100 {
        if let Some(Event::Block(_)) = acq.recv_timeout(Duration::from_millis(50)) {
            block = true;
            break;
        }
    }
    let _ = block; // no Configure was sent, so there may be none
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn records_single_samples_at_2_5_us() {
    let dir = temp_dir("fast");
    let path = dir.join("b.ohlog");
    let fake = realtime_fake([SINE_1K, Wave::Dc(-0.5)]);
    let acq = Acquisition::spawn(opener(fake.clone()));
    let (req, _sh) = request(&path, 2.5e-6, [true, true], 0);
    let took = record(&acq, req, 2.0);
    let f = LogFile::open(&path).unwrap();
    assert!(f.header.single());
    assert_eq!(f.header.adc_rate, 400e3);
    assert!(f.gaps().is_empty(), "{:?}", f.gaps());
    let got = f.end() as f64;
    let want = took * 400e3;
    println!(
        "2.5 µs: {got} samples in {took:.3} s = {:.1} kS/s, file {} bytes",
        got / took / 1e3,
        std::fs::metadata(&path).unwrap().len()
    );
    assert!((got - want).abs() < 0.12 * 400e3, "{got} for {want}");
    // The sine shows: 400 samples per period.
    let r = f.read(10_000..10_400).unwrap();
    let max = r.iter().map(|x| x.1[0][1]).fold(f32::MIN, f32::max);
    let min = r.iter().map(|x| x.1[0][0]).fold(f32::MAX, f32::min);
    assert!(max > 1.4 && min < -1.4, "{min} {max}");
    assert!(r.iter().all(|x| (x.1[1][2] + 0.5).abs() < 0.08));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn lost_samples_become_a_gap() {
    let dir = temp_dir("gap");
    let path = dir.join("c.ohlog");
    let fake = realtime_fake([Wave::Dc(0.5), Wave::Dc(0.0)]);
    let acq = Acquisition::spawn(opener(fake.clone()));
    let (req, sh) = request(&path, 1e-3, [true, false], 0);
    acq.send(Command::StartLog(req));
    wait_state(&acq, &sh, 10.0, |s| *s == LogState::Recording);
    std::thread::sleep(Duration::from_millis(700));
    // Half a second of samples does not come (20 kS/s ADC).
    fake.lock().drop_samples = 10_000;
    std::thread::sleep(Duration::from_millis(1500));
    acq.send(Command::StopLog);
    wait_state(&acq, &sh, 10.0, |s| matches!(s, LogState::Done(_)));
    assert_eq!(sh.lock().unwrap().gaps, 1);
    let f = LogFile::open(&path).unwrap();
    let gaps = f.gaps();
    assert_eq!(gaps.len(), 1, "{gaps:?}");
    let (a, b) = gaps[0];
    let lost = b - a;
    assert!((400..=600).contains(&lost), "lost {lost} steps");
    assert!((500..1000).contains(&a), "gap at {a}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn unplugging_makes_a_gap_and_logging_goes_on() {
    let dir = temp_dir("unplug");
    let path = dir.join("d.ohlog");
    let fake = realtime_fake([Wave::Dc(0.5), Wave::Dc(0.0)]);
    let acq = Acquisition::spawn(opener(fake.clone()));
    let (req, sh) = request(&path, 10e-3, [true, true], 0);
    acq.send(Command::StartLog(req));
    wait_state(&acq, &sh, 10.0, |s| *s == LogState::Recording);
    std::thread::sleep(Duration::from_millis(500));
    fake.lock().unplugged = true;
    std::thread::sleep(Duration::from_millis(800));
    drain(&acq);
    fake.lock().unplugged = false;
    std::thread::sleep(Duration::from_millis(2000));
    acq.send(Command::StopLog);
    wait_state(&acq, &sh, 10.0, |s| matches!(s, LogState::Done(_)));
    let f = LogFile::open(&path).unwrap();
    let gaps = f.gaps();
    assert_eq!(gaps.len(), 1, "{gaps:?}");
    // About 0.8 s or more (reconnecting takes a while) at 10 ms steps.
    assert!(gaps[0].1 - gaps[0].0 >= 60, "{gaps:?}");
    assert!(f.end() > gaps[0].1 + 50, "no data after the gap");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn size_limit_stops_the_log() {
    let dir = temp_dir("limit");
    let path = dir.join("e.ohlog");
    let fake = realtime_fake([Wave::Dc(0.5), Wave::Dc(0.0)]);
    let acq = Acquisition::spawn(opener(fake.clone()));
    // 3.2 MB/s: 400 kB is about 0.12 s.
    let (req, sh) = request(&path, 2.5e-6, [true, true], 400_000);
    acq.send(Command::StartLog(req));
    wait_state(&acq, &sh, 10.0, |s| *s == LogState::Done(StopReason::Limit));
    let len = std::fs::metadata(&path).unwrap().len();
    assert!(len <= 400_000 && len > 399_000, "{len}");
    assert_eq!(sh.lock().unwrap().bytes, len);
    let f = LogFile::open(&path).unwrap();
    assert!(f.end() > 49_000);
    // The thread goes back to normal capture: a Configure gives blocks.
    let app = openhantek_tui::app::App::new(Default::default(), None);
    let cfg = app.scope.hw_config();
    acq.send(Command::Configure { gen: 7, cfg });
    let mut ok = false;
    for _ in 0..200 {
        if let Some(Event::Block(b)) = acq.recv_timeout(Duration::from_millis(50)) {
            if b.gen == 7 {
                ok = true;
                break;
            }
        }
    }
    assert!(ok, "no block after the log stopped");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn settings_changes_wait_until_the_log_stops() {
    let dir = temp_dir("cfg");
    let path = dir.join("f.ohlog");
    let fake = realtime_fake([Wave::Dc(0.5), Wave::Dc(0.0)]);
    let acq = Acquisition::spawn(opener(fake.clone()));
    let (req, sh) = request(&path, 1e-3, [true, true], 0);
    acq.send(Command::StartLog(req));
    wait_state(&acq, &sh, 10.0, |s| *s == LogState::Recording);
    let app = openhantek_tui::app::App::new(Default::default(), None);
    let mut cfg = app.scope.hw_config();
    cfg.gains = [0, 0];
    acq.send(Command::Configure { gen: 3, cfg });
    acq.send(Command::Run(false));
    std::thread::sleep(Duration::from_millis(500));
    drain(&acq);
    // Still the logger's gain and still sampling.
    assert_eq!(fake.lock().gain_byte, [1, 1]);
    assert!(fake.lock().sampling);
    acq.send(Command::StopLog);
    wait_state(&acq, &sh, 10.0, |s| matches!(s, LogState::Done(_)));
    let f = LogFile::open(&path).unwrap();
    assert!(f.gaps().is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn pyramid_of_a_recorded_log() {
    let dir = temp_dir("pyr");
    let path = dir.join("g.ohlog");
    let fake = realtime_fake([SINE_1K, Wave::Dc(1.0)]);
    let acq = Acquisition::spawn(opener(fake.clone()));
    let (req, _sh) = request(&path, 10e-6, [true, true], 0);
    record(&acq, req, 1.5);
    let f = LogFile::open(&path).unwrap();
    let mut p = Pyramid::default();
    while p.update(&f, 1 << 20).unwrap() {}
    let all = p.agg(&f, 0..f.end()).unwrap();
    let mut lo = f32::MAX;
    let mut hi = f32::MIN;
    f.for_each(0..f.end(), |_, r| {
        for x in r {
            lo = lo.min(x[0][0]);
            hi = hi.max(x[0][1]);
        }
    })
    .unwrap();
    assert_eq!((all[0].min, all[0].max), (lo, hi));
    assert_eq!(all[0].n as u64, f.end());
    assert!((all[1].mean - 1.0).abs() < 0.05);
    std::fs::remove_dir_all(&dir).unwrap();
}
