//! Envelope logger (chart recorder): streams the scope without gaps, and
//! stores min, max and mean per channel for each time step in a compact
//! binary file. See the README for the file format.
//!
//! - [`format`]: the file layout (header, chunks, records).
//! - [`binner`]: turns samples into records, one per step.
//! - [`writer`]: the writer thread (buffering, fsync, size limit).
//! - [`reader`]: reads a log, also while it is being written.
//! - [`pyramid`]: min/max/mean summaries for fast zoomed-out drawing.
//! - [`csv`]: CSV export of a log.
//! - [`view`]: the state of the log view (zoom, scroll, cursors).

pub mod binner;
pub mod csv;
pub mod format;
pub mod gapcheck;
pub mod pyramid;
pub mod reader;
pub mod view;
pub mod writer;

use std::path::{Path, PathBuf};

use crate::device::protocol::Rate;

/// Log steps in seconds: 0.5 µs up to one minute. The raw step ([`RAW`])
/// comes before them in the settings panel.
pub const STEPS: [f64; 26] = [
    0.5e-6, 1e-6, 2e-6, 5e-6, 10e-6, 20e-6, 50e-6, 100e-6, 200e-6, 500e-6, 1e-3, 2e-3, 5e-3, 10e-3,
    20e-3, 50e-3, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 30.0, 60.0,
];
/// The default step: 1 ms.
pub const STEP_DEFAULT: f64 = 1e-3;

/// Size limits in bytes; 0 means no limit. 13 entries, no more than the
/// panel categories, so the list does not make the settings panel higher.
pub const LIMITS: [u64; 13] = [
    10_000_000,
    20_000_000,
    50_000_000,
    100_000_000,
    200_000_000,
    500_000_000,
    1_000_000_000,
    2_000_000_000,
    5_000_000_000,
    10_000_000_000,
    20_000_000_000,
    50_000_000_000,
    0,
];
/// The default limit: 1 GB.
pub const LIMIT_DEFAULT: u64 = 1_000_000_000;

/// The step value of the raw steps: one ADC sample per step, stored as
/// the raw 1-byte ADC value (see [`format`]). The step is then the ADC
/// sample time: 62.5 ns with two channels at 16 MS/s, 33.3 ns with CH1
/// alone at 30 MS/s (at the default max ADC rate).
pub const RAW: f64 = 0.0;

/// True for the raw step value.
pub fn is_raw(step: f64) -> bool {
    step <= 0.0
}

/// Channels the scope samples for a log of `logged` channels: CH1 alone
/// can use 1-channel mode (twice the rate); CH2 alone cannot (the
/// firmware's 1-channel mode is CH1).
pub fn hw_channels(logged: [bool; 2], max_adc: f64) -> u8 {
    crate::device::protocol::hw_channels(logged != [true, false], max_adc)
}

/// A step in nanoseconds (the file stores it like this). 0 for raw.
pub fn step_ns(step: f64) -> u64 {
    if is_raw(step) {
        return 0;
    }
    (step * 1e9).round().max(1.0) as u64
}

/// Whole ADC samples in a step of `step_ns` at `raw` S/s, if it is a
/// whole number (at least 1).
fn whole(raw: f64, step_ns: u64) -> Option<u64> {
    let n = raw * step_ns as f64 / 1e9;
    ((n - n.round()).abs() < 1e-6 && n >= 1.0).then_some(n.round() as u64)
}

/// The ADC rate and samples per step for a step (`step_ns`, 0 = raw) with
/// `channels` (1 or 2, see [`hw_channels`]) sampled. The ADC always runs
/// at the top rate the max ADC rate setting allows: each step is the
/// min, max and mean of all its raw samples. A step that is not a whole
/// number of ADC samples (too short for a low max ADC rate) becomes the
/// shortest step that is (see [`fit_step`]). No oversampling: the logger
/// bins the raw samples itself.
pub fn adc_for_step(step_ns: u64, channels: u8, max_adc: f64) -> (Rate, u32) {
    let (id, raw) = crate::device::protocol::adc_rate(channels, max_adc);
    let n = if step_ns == 0 {
        1
    } else {
        let ns = step_ns_fit(step_ns, raw);
        whole(raw, ns).unwrap_or(1).min(u32::MAX as u64) as u32
    };
    (
        Rate {
            rate: raw,
            id,
            raw,
            oversample: 1,
        },
        n,
    )
}

/// `step_ns`, or the shortest of [`STEPS`] at least as long that is a
/// whole number of samples at `raw`.
fn step_ns_fit(step_ns: u64, raw: f64) -> u64 {
    if whole(raw, step_ns).is_some() {
        return step_ns;
    }
    STEPS
        .iter()
        .map(|s| self::step_ns(*s))
        .find(|&ns| ns >= step_ns && whole(raw, ns).is_some())
        .unwrap_or_else(|| self::step_ns(STEPS[STEPS.len() - 1]))
}

/// The step a log of `logged` channels uses for the step setting `step`:
/// the setting, or when it is shorter than the ADC allows (a low max ADC
/// rate) the next step that fits. [`RAW`] stays raw.
pub fn fit_step(step: f64, logged: [bool; 2], max_adc: f64) -> f64 {
    if is_raw(step) {
        return RAW;
    }
    let (_, raw) = crate::device::protocol::adc_rate(hw_channels(logged, max_adc), max_adc);
    let ns = step_ns_fit(step_ns(step), raw);
    STEPS
        .iter()
        .copied()
        .find(|s| step_ns(*s) == ns)
        .unwrap_or(ns as f64 * 1e-9)
}

/// The step choices of the settings panel for `logged` channels: raw
/// first, then the steps of [`STEPS`] that the ADC rate allows.
pub fn step_choices(logged: [bool; 2], max_adc: f64) -> Vec<f64> {
    let logged = or_ch1(logged);
    let (_, raw) = crate::device::protocol::adc_rate(hw_channels(logged, max_adc), max_adc);
    std::iter::once(RAW)
        .chain(
            STEPS
                .iter()
                .copied()
                .filter(|s| whole(raw, step_ns(*s)).is_some()),
        )
        .collect()
}

/// Seconds per step: the ADC sample time for raw.
pub fn step_secs(step: f64, logged: [bool; 2], max_adc: f64) -> f64 {
    if is_raw(step) {
        let logged = or_ch1(logged);
        let (_, raw) = crate::device::protocol::adc_rate(hw_channels(logged, max_adc), max_adc);
        1.0 / raw
    } else {
        fit_step(step, or_ch1(logged), max_adc)
    }
}

/// Text of a step choice: like `2.5 µs`, or `raw (62.5 ns)`.
pub fn step_label(step: f64, logged: [bool; 2], max_adc: f64) -> String {
    if is_raw(step) {
        format!("raw ({})", fmt_step(step_secs(step, logged, max_adc)))
    } else {
        fmt_step(step)
    }
}

/// Bytes of one record: one byte per channel for raw logs, one f32 when
/// a step is one sample, else min, max and mean as f32.
pub fn record_bytes(channels: usize, samples_per_step: u32, raw: bool) -> usize {
    if raw {
        return channels;
    }
    channels * if samples_per_step == 1 { 1 } else { 3 } * 4
}

/// The channels to log, for the estimates: CH1 when none is on.
fn or_ch1(logged: [bool; 2]) -> [bool; 2] {
    if logged == [false, false] {
        [true, false]
    } else {
        logged
    }
}

/// File bytes per second for a step setting and the logged channels.
pub fn bytes_per_second(step: f64, logged: [bool; 2], max_adc: f64) -> f64 {
    let logged = or_ch1(logged);
    let raw = is_raw(step);
    let step = fit_step(step, logged, max_adc);
    let (_, n) = adc_for_step(step_ns(step), hw_channels(logged, max_adc), max_adc);
    let count = logged.iter().filter(|&&c| c).count();
    record_bytes(count, n, raw) as f64 / step_secs(step, logged, max_adc)
}

/// Bytes like `412 MB` or `1 GB` (powers of 1000, like disk makers).
pub fn fmt_bytes(b: u64) -> String {
    let b = b as f64;
    for (scale, unit) in [(1e12, "TB"), (1e9, "GB"), (1e6, "MB"), (1e3, "kB")] {
        if b >= scale {
            let n = b / scale;
            return if n >= 100.0 || (n - n.round()).abs() < 0.05 {
                format!("{:.0} {unit}", n)
            } else {
                format!("{:.1} {unit}", n)
            };
        }
    }
    format!("{b:.0} B")
}

/// A long time, rough: `2 h 50 min`, `41 min`, `12 s`, `3 d 4 h`.
pub fn fmt_span(secs: f64) -> String {
    if !secs.is_finite() {
        return "--".into();
    }
    let s = secs.max(0.0).round() as u64;
    let (d, h, m) = (s / 86_400, s % 86_400 / 3600, s % 3600 / 60);
    if d > 0 {
        format!("{d} d {h} h")
    } else if h > 0 {
        format!("{h} h {m} min")
    } else if m > 0 {
        format!("{m} min")
    } else {
        format!("{s} s")
    }
}

/// A clock time like `0:41:10`, or `2 d 3:04:05` past a day.
pub fn fmt_hms(secs: f64) -> String {
    let s = secs.max(0.0).floor() as u64;
    let (d, h, m, sec) = (s / 86_400, s % 86_400 / 3600, s % 3600 / 60, s % 60);
    if d > 0 {
        format!("{d} d {h}:{m:02}:{sec:02}")
    } else {
        format!("{h}:{m:02}:{sec:02}")
    }
}

/// The step as text, like `2.5 µs`, `62.5 ns` or `1 min`.
pub fn fmt_step(step: f64) -> String {
    if step >= 59.999 {
        format!("{:.0} min", step / 60.0)
    } else {
        crate::si::fmt_si(step, "s")
    }
}

/// How long a log of `limit` bytes lasts, like `Limit 1 GB ≈ 11 h 34 min
/// at 1 ms step`. `logged` are the logged channels.
pub fn limit_estimate(limit: u64, step: f64, logged: [bool; 2], max_adc: f64) -> String {
    let bps = bytes_per_second(step, logged, max_adc);
    let label = step_label(fit_step(step, or_ch1(logged), max_adc), logged, max_adc);
    if limit == 0 {
        return format!(
            "no limit · {}/h at {} step",
            fmt_bytes((bps * 3600.0) as u64),
            label
        );
    }
    let secs = limit as f64 / bps;
    format!(
        "Limit {} ≈ {} at {} step",
        fmt_bytes(limit),
        fmt_span(secs),
        label
    )
}

/// The nearest entry of a sorted list of positive values (by ratio).
pub fn nearest<T: Copy + Into<f64>>(list: &[T], v: f64) -> usize {
    let far = |x: f64| {
        if x > 0.0 && v > 0.0 {
            (x / v).ln().abs()
        } else {
            (x - v).abs() * 1e9
        }
    };
    (0..list.len())
        .min_by(|&a, &b| far(list[a].into()).total_cmp(&far(list[b].into())))
        .unwrap_or(0)
}

/// `$XDG_DATA_HOME/openhantek-tui/logs`, or
/// `~/.local/share/openhantek-tui/logs`, or `./logs`.
pub fn default_log_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir).join("openhantek-tui/logs");
    }
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join(".local/share/openhantek-tui/logs"),
        None => PathBuf::from("logs"),
    }
}

/// Free bytes for this user on the file system of `path` (or of its
/// nearest existing parent).
pub fn free_space(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let mut p = path;
    while !p.exists() {
        p = p.parent()?;
    }
    let p = if p.as_os_str().is_empty() {
        Path::new(".")
    } else {
        p
    };
    let c = std::ffi::CString::new(p.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid C string and `st` a valid out pointer.
    let r = unsafe { libc::statvfs(c.as_ptr(), &mut st) };
    (r == 0).then(|| st.f_bavail as u64 * st.f_frsize as u64)
}

/// File name extension of logs.
pub const EXT: &str = "ohlog";

/// Log files in `dir`, newest first, at most `max`.
pub fn recent_logs(dir: &Path, max: usize) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<(std::time::SystemTime, PathBuf)> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == EXT))
        .map(|p| {
            let t = std::fs::metadata(&p)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (t, p)
        })
        .collect();
    v.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    v.into_iter().take(max).map(|(_, p)| p).collect()
}

/// A new log file name in `dir`: `log-YYYYMMDD-HHMMSS.ohlog` (UTC), with
/// `-2`, `-3` ... when it exists.
pub fn new_log_path(dir: &Path, now: std::time::SystemTime) -> PathBuf {
    let stamp = crate::export::timestamp(now);
    let mut p = dir.join(format!("log-{stamp}.{EXT}"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("log-{stamp}-{n}.{EXT}"));
        n += 1;
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOP: f64 = crate::device::protocol::MAX_ADC_DEFAULT;

    #[test]
    fn adc_rate_per_step() {
        // The ADC always runs at the top rate: 16 MS/s with two channels.
        let cases: [(f64, u32); 6] = [
            (0.5e-6, 8),
            (1e-6, 16),
            (10e-6, 160),
            (1e-3, 16_000),
            (1.0, 16_000_000),
            (60.0, 960_000_000),
        ];
        for (step, n) in cases {
            let (r, got) = adc_for_step(step_ns(step), 2, TOP);
            assert_eq!((r.raw, got), (16e6, n), "{step}");
            assert_eq!(r.oversample, 1);
        }
        // CH1 alone: 1-channel mode, 30 MS/s.
        assert_eq!(adc_for_step(step_ns(0.5e-6), 1, TOP).0.raw, 30e6);
        assert_eq!(adc_for_step(step_ns(0.5e-6), 1, TOP).1, 15);
        // Raw: one sample per step.
        assert_eq!(adc_for_step(0, 2, TOP).1, 1);
        assert_eq!(adc_for_step(0, 1, TOP).0.raw, 30e6);
        for ch in [1, 2] {
            for step in STEPS {
                let (r, n) = adc_for_step(step_ns(step), ch, TOP);
                assert!((r.raw * step - n as f64).abs() < 1e-6, "{step}");
                assert_eq!(crate::device::protocol::raw_rate_of_id(r.id), r.raw);
            }
        }
        assert_eq!(hw_channels([true, false], TOP), 1);
        assert_eq!(hw_channels([false, true], TOP), 2);
        assert_eq!(hw_channels([true, true], TOP), 2);
        assert_eq!(hw_channels([true, false], 400e3), 2);
    }

    #[test]
    fn steps_adapt_to_the_max_adc_rate() {
        const BOTH: [bool; 2] = [true, true];
        const CH1: [bool; 2] = [true, false];
        let c = step_choices(BOTH, TOP);
        assert_eq!(c[0], RAW);
        assert_eq!(c.len(), STEPS.len() + 1);
        assert_eq!(step_label(RAW, BOTH, TOP), "raw (62.5 ns)");
        assert_eq!(step_label(RAW, CH1, TOP), "raw (33.3 ns)");
        assert_eq!(step_label(RAW, [false, true], TOP), "raw (62.5 ns)");
        assert_eq!(step_label(RAW, BOTH, 1e6), "raw (1 µs)");
        // At 400 kS/s the steps start at 2.5 µs... which is not a
        // 1-2-5 step: 5 µs is the first.
        let c = step_choices(BOTH, 400e3);
        assert_eq!(c[1], 5e-6);
        assert_eq!(fit_step(1e-6, BOTH, 400e3), 5e-6);
        assert_eq!(fit_step(1e-3, BOTH, 400e3), 1e-3);
        assert_eq!(adc_for_step(step_ns(1e-6), 2, 400e3).1, 2);
        assert!(is_raw(fit_step(RAW, BOTH, 400e3)));
    }

    #[test]
    fn sizes_and_estimates() {
        const BOTH: [bool; 2] = [true, true];
        const CH1: [bool; 2] = [true, false];
        assert_eq!(record_bytes(2, 1, false), 8);
        assert_eq!(record_bytes(2, 20, false), 24);
        assert_eq!(record_bytes(1, 20, false), 12);
        assert_eq!(record_bytes(2, 1, true), 2);
        // 2 channels at 1 ms: 24 B per ms = 86.4 MB per hour.
        assert!((bytes_per_second(1e-3, BOTH, TOP) - 24_000.0).abs() < 1e-6);
        // 2 channels at 0.5 µs: 24 B per 0.5 µs = 48 MB/s.
        assert!((bytes_per_second(0.5e-6, BOTH, TOP) - 48e6).abs() < 1e-3);
        assert!((bytes_per_second(0.5e-6, CH1, TOP) - 24e6).abs() < 1e-3);
        // Raw: one byte per channel and sample.
        assert!((bytes_per_second(RAW, BOTH, TOP) - 32e6).abs() < 1e-3);
        assert!((bytes_per_second(RAW, CH1, TOP) - 30e6).abs() < 1e-3);
        assert!((bytes_per_second(RAW, [false, true], TOP) - 16e6).abs() < 1e-3);
        assert!((bytes_per_second(RAW, BOTH, 1e6) - 2e6).abs() < 1e-3);
        // No channel on: estimated for CH1.
        assert_eq!(
            bytes_per_second(1e-3, [false, false], TOP),
            bytes_per_second(1e-3, CH1, TOP)
        );
        assert_eq!(
            limit_estimate(1_000_000_000, 1e-3, BOTH, TOP),
            "Limit 1 GB ≈ 11 h 34 min at 1 ms step"
        );
        assert_eq!(
            limit_estimate(1_000_000_000, 1e-3, CH1, TOP),
            "Limit 1 GB ≈ 23 h 8 min at 1 ms step"
        );
        assert_eq!(
            limit_estimate(1_000_000_000, RAW, BOTH, TOP),
            "Limit 1 GB ≈ 31 s at raw (62.5 ns) step"
        );
        assert_eq!(
            limit_estimate(0, 1e-3, BOTH, TOP),
            "no limit · 86.4 MB/h at 1 ms step"
        );
        assert_eq!(fmt_bytes(412_300_000), "412 MB");
        assert_eq!(fmt_bytes(1_500_000_000), "1.5 GB");
        assert_eq!(fmt_bytes(999), "999 B");
        assert_eq!(fmt_hms(2470.0), "0:41:10");
        assert_eq!(fmt_hms(90_061.0), "1 d 1:01:01");
        assert_eq!(fmt_step(60.0), "1 min");
        assert_eq!(fmt_step(0.5e-6), "500 ns");
        assert_eq!(nearest(&STEPS, 0.0011), 10);
    }

    #[test]
    fn free_space_of_temp() {
        let f = free_space(&std::env::temp_dir().join("no/such/dir"));
        assert!(f.is_some_and(|b| b > 0));
    }

    #[test]
    fn log_names_do_not_clash() {
        let dir = std::env::temp_dir().join(format!("oht-names-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_794_573_296);
        let a = new_log_path(&dir, t);
        assert!(a.ends_with("log-20261113-123456.ohlog"));
        std::fs::write(&a, b"x").unwrap();
        let b = new_log_path(&dir, t);
        assert!(b.ends_with("log-20261113-123456-2.ohlog"));
        assert_eq!(recent_logs(&dir, 5), vec![a]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
