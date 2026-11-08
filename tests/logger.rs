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
        max_adc: openhantek_tui::device::protocol::MAX_ADC_DEFAULT,
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
    // The ADC runs at the top rate: 16 000 samples per 1 ms step.
    assert_eq!(f.header.samples_per_step, 16_000);
    assert_eq!(f.header.adc_rate, 16e6);
    // Steps match the time: the stream gives as many samples as the
    // clock says (within about one read of 100 ms each way).
    let want = took / 1e-3;
    let got = f.end() as f64;
    println!(
        "1 ms: {got} steps in {took:.3} s ({:.0} kS/s)",
        got * 16_000.0 / took / 1e3
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
    // No Configure was sent after the log, so there may be none.
    let _ = block;
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A log at the fastest step, 0.5 µs: 16 MS/s with both channels, or
/// 30 MS/s in 1-channel mode with CH1 alone. The fake runs as fast as it
/// can (slower than real time in a debug build) and stamps each transfer
/// with its bus time, so the logger's clock check sees a whole stream and
/// must not report a gap.
fn fast_log(name: &str, channels: [bool; 2], raw: f64, per_step: u32) {
    let dir = temp_dir(name);
    let path = dir.join("b.ohlog");
    let fake = FakeScope::new([SINE_1K, Wave::Dc(-0.5)]);
    let acq = Acquisition::spawn(opener(fake.clone()));
    let (req, sh) = request(&path, 0.5e-6, channels, 0);
    let hw = if channels == [true, false] { 1 } else { 2 };
    acq.send(Command::StartLog(req));
    wait_state(&acq, &sh, 10.0, |s| *s == LogState::Recording);
    // 0.1 s of data (5 ms periods of a 1 kHz sine are 100 of them).
    let want = (raw * 0.1) as usize * hw;
    let t0 = Instant::now();
    while fake.lock().bulk_bytes < want {
        drain(&acq);
        assert!(t0.elapsed().as_secs() < 60, "the fake is too slow");
        std::thread::sleep(Duration::from_millis(5));
    }
    acq.send(Command::StopLog);
    wait_state(&acq, &sh, 30.0, |s| matches!(s, LogState::Done(_)));
    assert_eq!(fake.lock().channels as usize, hw);
    let frames = (fake.lock().bulk_bytes / hw) as u64;
    let f = LogFile::open(&path).unwrap();
    assert_eq!(f.header.adc_rate, raw);
    assert_eq!(f.header.samples_per_step, per_step);
    assert!(f.gaps().is_empty(), "{:?}", f.gaps());
    // Every sample the fake gave is in the file, except the 2048 thrown
    // away at the start and the last partial step.
    let got = f.end() * per_step as u64;
    println!("0.5 µs at {raw} S/s: {got} samples logged, fake gave {frames}");
    let all = frames - 2048;
    assert!(
        got < all + per_step as u64 && got + 2048 > all,
        "{got} of {all}"
    );
    // The 1 kHz sine shows: one period is 2 ms / 0.5 µs = 2000 steps.
    let r = f.read(10_000..12_000).unwrap();
    if channels[0] {
        let max = r.iter().map(|x| x.1[0][1]).fold(f32::MIN, f32::max);
        let min = r.iter().map(|x| x.1[0][0]).fold(f32::MAX, f32::min);
        assert!(max > 1.4 && min < -1.4, "{min} {max}");
    }
    if channels[1] {
        assert!(r.iter().all(|x| (x.1[1][2] + 0.5).abs() < 0.08));
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn records_at_16_ms_per_s_with_two_channels() {
    fast_log("fast2", [true, true], 16e6, 8);
}

#[test]
fn records_at_30_ms_per_s_with_ch1_alone() {
    fast_log("fast1", [true, false], 30e6, 15);
}

/// CH2 alone cannot use 1-channel mode (that is CH1): both are sampled.
#[test]
fn ch2_alone_streams_both_channels() {
    fast_log("fast_ch2", [false, true], 16e6, 8);
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
    // Half a second of samples does not come (30 MS/s ADC, CH1 alone).
    fake.lock().drop_samples = 15_000_000;
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
    // 24 B per 1 µs step (10 MS/s, 10 samples each): 400 kB is
    // about 16 000 steps.
    let fake = FakeScope::new([Wave::Dc(0.5), Wave::Dc(0.0)]);
    let acq = Acquisition::spawn(opener(fake.clone()));
    let (req, sh) = request(&path, 1e-6, [true, true], 400_000);
    acq.send(Command::StartLog(req));
    wait_state(&acq, &sh, 10.0, |s| *s == LogState::Done(StopReason::Limit));
    let len = std::fs::metadata(&path).unwrap().len();
    assert!(len <= 400_000 && len > 399_000, "{len}");
    assert_eq!(sh.lock().unwrap().bytes, len);
    let f = LogFile::open(&path).unwrap();
    assert!(f.end() > 16_000, "{}", f.end());
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

#[test]
fn the_r_key_records_and_the_log_view_follows() {
    use openhantek_tui::settings::Settings;
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let dir = temp_dir("app");
    let fake = realtime_fake([SINE_1K, Wave::CalOut]);
    let acq = Acquisition::spawn(opener(fake.clone()));
    let mut app = openhantek_tui::app::App::new(Settings::default(), Some(acq));
    app.log_dir_override = Some(dir.clone());
    app.scope.settings.logger.step = 10e-3;
    tick_until(&mut app, 10.0, |a| a.connected);
    app.handle_key(KeyEvent::from(KeyCode::Char('r')));
    assert!(app.recording());
    assert!(app.show_log);
    tick_until(&mut app, 10.0, |a| {
        a.log_view.as_ref().is_some_and(|v| v.live && v.end() > 100)
    });
    // The view follows the growing file.
    let n = app.log_view.as_ref().unwrap().end();
    tick_until(&mut app, 10.0, |a| {
        a.log_view.as_ref().unwrap().end() > n + 20
    });
    app.handle_key(KeyEvent::from(KeyCode::Char('r')));
    tick_until(&mut app, 10.0, |a| !a.recording() && a.rec.is_none());
    let msg = app.current_message().unwrap().text.clone();
    assert!(msg.starts_with("Log saved: "), "{msg}");
    let logs = lg::recent_logs(&dir, 5);
    assert_eq!(logs.len(), 1);
    let f = LogFile::open(&logs[0]).unwrap();
    assert!(f.gaps().is_empty());
    assert_eq!(f.header.logged(), vec![0, 1]);
    // CH2 is the calibration output: 0 V to 2 V in each 10 ms step.
    let r = f.read(10..11).unwrap();
    assert!(r[0].1[1][0] < 0.1 && r[0].1[1][1] > 1.9, "{:?}", r[0]);
    // After the log, the scope view gets blocks again.
    app.handle_key(KeyEvent::from(KeyCode::Char('V')));
    assert!(!app.show_log);
    app.scope.frame = None;
    tick_until(&mut app, 10.0, |a| a.scope.frame.is_some());
    drop(app);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Speed of a big log: `OHT_BIG_MB=2000 cargo test --release --test
/// logger big -- --ignored --nocapture`. Writes a 2.5 µs log of that
/// size to the temp folder (deleted afterwards).
#[test]
#[ignore]
fn big_log_opens_fast_after_the_summary() {
    use openhantek_tui::logger::format::{ChannelInfo, ChunkHead, Header, OPEN};
    use openhantek_tui::logger::view::LogView;
    use std::io::Write;
    let mb: u64 = std::env::var("OHT_BIG_MB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(500);
    let dir = temp_dir("big");
    let path = dir.join("big.ohlog");
    let ch = ChannelInfo {
        enabled: true,
        ..Default::default()
    };
    let h = Header {
        start_ns: 1_794_573_296_000_000_000,
        step_ns: 2_500,
        adc_rate: 400e3,
        samples_per_step: 1,
        channels: [ch.clone(), ch],
        device: "BIG".into(),
        raw: false,
    };
    let n = mb * 1_000_000 / 8;
    let t0 = Instant::now();
    {
        let mut f =
            std::io::BufWriter::with_capacity(1 << 22, std::fs::File::create(&path).unwrap());
        f.write_all(&h.encode()).unwrap();
        f.write_all(
            &ChunkHead {
                flags: 0,
                first: 0,
                count: OPEN,
            }
            .encode(),
        )
        .unwrap();
        let mut buf = Vec::with_capacity(1 << 20);
        for i in 0..n {
            let v = ((i % 400) as f32 / 400.0 * std::f32::consts::TAU).sin();
            buf.extend_from_slice(&v.to_le_bytes());
            buf.extend_from_slice(&(v * 0.5).to_le_bytes());
            if buf.len() >= 1 << 20 {
                f.write_all(&buf).unwrap();
                buf.clear();
            }
        }
        f.write_all(&buf).unwrap();
    }
    println!(
        "wrote {mb} MB ({n} steps) in {:.2} s",
        t0.elapsed().as_secs_f64()
    );
    let t0 = Instant::now();
    let mut v = LogView::open(&path, false).unwrap();
    while v.summary_progress() < 1.0 {
        v.tick().unwrap();
    }
    v.tick().unwrap();
    println!("summary built in {:.2} s", t0.elapsed().as_secs_f64());
    assert!(openhantek_tui::logger::pyramid::Pyramid::sidecar(&path).exists());
    // Open again: the summary comes from the sidecar.
    let t0 = Instant::now();
    let mut v = LogView::open(&path, false).unwrap();
    v.tick().unwrap();
    let w = 240;
    let cols = v.columns(w).unwrap();
    let open = t0.elapsed().as_secs_f64();
    assert_eq!(cols.len(), w);
    assert!(cols.iter().all(|c| c[0].max > 0.99 && c[0].min < -0.99));
    // Some zoom levels and scroll positions.
    let t1 = Instant::now();
    let mut draws = 0;
    for m in [
        openhantek_tui::logger::view::Move::ZoomIn,
        openhantek_tui::logger::view::Move::Pages(-3),
        openhantek_tui::logger::view::Move::ZoomIn,
        openhantek_tui::logger::view::Move::ZoomIn,
        openhantek_tui::logger::view::Move::Divs(-7),
        openhantek_tui::logger::view::Move::Native,
        openhantek_tui::logger::view::Move::Home,
        openhantek_tui::logger::view::Move::Fit,
    ] {
        v.go(m, w, 24);
        v.columns(w).unwrap();
        draws += 1;
    }
    let per = t1.elapsed().as_secs_f64() / draws as f64;
    println!(
        "open + first draw {open:.3} s, then {:.1} ms per redraw",
        per * 1e3
    );
    assert!(open < 1.0);
    std::fs::remove_dir_all(&dir).unwrap();
}
