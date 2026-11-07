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
pub mod pyramid;
pub mod reader;
pub mod view;
pub mod writer;

use std::path::{Path, PathBuf};

use crate::device::protocol::Rate;

/// Log steps in seconds: one raw sample at 400 kS/s up to one minute.
pub const STEPS: [f64; 24] = [
    2.5e-6, 5e-6, 10e-6, 20e-6, 50e-6, 100e-6, 200e-6, 500e-6, 1e-3, 2e-3, 5e-3, 10e-3, 20e-3,
    50e-3, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 30.0, 60.0,
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

/// ADC rates the logger uses (rate id for request 0xE2, and samples per
/// second). The scope streams without loss up to 400 kS/s (see
/// [`crate::device::protocol::MAX_ROLL_RAW`]).
pub const ADC_RATES: [(u8, f64); 5] = [
    (102, 20e3),
    (104, 40e3),
    (110, 100e3),
    (120, 200e3),
    (140, 400e3),
];

/// Samples per step the logger aims for, when the ADC is fast enough.
pub const SAMPLES_PER_STEP_MIN: u64 = 10;

/// A step in nanoseconds (the file stores it like this).
pub fn step_ns(step: f64) -> u64 {
    (step * 1e9).round().max(1.0) as u64
}

/// The ADC rate for a step: the lowest rate that gives at least
/// [`SAMPLES_PER_STEP_MIN`] whole samples per step, else the fastest
/// rate that gives a whole number of samples. Returns the rate (no
/// oversampling: the logger bins the raw samples itself) and the
/// samples per step.
pub fn adc_for_step(step_ns: u64) -> (Rate, u32) {
    let whole = |raw: f64| {
        let n = raw * step_ns as f64 / 1e9;
        ((n - n.round()).abs() < 1e-6 && n >= 1.0).then_some(n.round() as u64)
    };
    let pick = ADC_RATES
        .iter()
        .find(|(_, raw)| whole(*raw).is_some_and(|n| n >= SAMPLES_PER_STEP_MIN))
        .or_else(|| {
            ADC_RATES
                .iter()
                .rev()
                .find(|(_, raw)| whole(*raw).is_some())
        })
        .copied()
        .unwrap_or(ADC_RATES[ADC_RATES.len() - 1]);
    let (id, raw) = pick;
    let n = whole(raw).unwrap_or(1).min(u32::MAX as u64) as u32;
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

/// Bytes of one record: one value per channel when a step is one
/// sample, else min, max and mean.
pub fn record_bytes(channels: usize, samples_per_step: u32) -> usize {
    channels * if samples_per_step == 1 { 1 } else { 3 } * 4
}

/// File bytes per second for a step and a number of channels.
pub fn bytes_per_second(step: f64, channels: usize) -> f64 {
    let (_, n) = adc_for_step(step_ns(step));
    record_bytes(channels.max(1), n) as f64 / step
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

/// The step as text, like `2.5 µs` or `1 min`.
pub fn fmt_step(step: f64) -> String {
    if step >= 59.999 {
        format!("{:.0} min", step / 60.0)
    } else {
        crate::si::fmt_si(step, "s")
    }
}

/// How long a log of `limit` bytes lasts, like `Limit 1 GB ≈ 11 h 34 min
/// at 1 ms step`. `channels` is the number of logged channels.
pub fn limit_estimate(limit: u64, step: f64, channels: usize) -> String {
    if limit == 0 {
        return format!(
            "no limit · {}/h at {} step",
            fmt_bytes((bytes_per_second(step, channels) * 3600.0) as u64),
            fmt_step(step)
        );
    }
    let secs = limit as f64 / bytes_per_second(step, channels);
    format!(
        "Limit {} ≈ {} at {} step",
        fmt_bytes(limit),
        fmt_span(secs),
        fmt_step(step)
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

    #[test]
    fn adc_rate_per_step() {
        let cases: [(f64, f64, u32); 9] = [
            (2.5e-6, 400e3, 1),
            (5e-6, 400e3, 2),
            (20e-6, 400e3, 8),
            (50e-6, 200e3, 10),
            (100e-6, 100e3, 10),
            (200e-6, 100e3, 20),
            (500e-6, 20e3, 10),
            (1e-3, 20e3, 20),
            (60.0, 20e3, 1_200_000),
        ];
        for (step, raw, n) in cases {
            let (r, got) = adc_for_step(step_ns(step));
            assert_eq!((r.raw, got), (raw, n), "{step}");
            assert_eq!(r.oversample, 1);
        }
        for step in STEPS {
            let (r, n) = adc_for_step(step_ns(step));
            assert!(r.raw <= crate::device::protocol::MAX_ROLL_RAW);
            assert!((r.raw * step - n as f64).abs() < 1e-6, "{step}");
        }
    }

    #[test]
    fn sizes_and_estimates() {
        assert_eq!(record_bytes(2, 1), 8);
        assert_eq!(record_bytes(2, 20), 24);
        assert_eq!(record_bytes(1, 20), 12);
        // 2 channels at 1 ms: 24 B per ms = 86.4 MB per hour.
        assert!((bytes_per_second(1e-3, 2) - 24_000.0).abs() < 1e-6);
        // 2 channels at 2.5 µs: 8 B per 2.5 µs = 3.2 MB/s.
        assert!((bytes_per_second(2.5e-6, 2) - 3.2e6).abs() < 1e-3);
        assert_eq!(
            limit_estimate(1_000_000_000, 1e-3, 2),
            "Limit 1 GB ≈ 11 h 34 min at 1 ms step"
        );
        assert_eq!(
            limit_estimate(1_000_000_000, 1e-3, 1),
            "Limit 1 GB ≈ 23 h 8 min at 1 ms step"
        );
        assert_eq!(
            limit_estimate(0, 1e-3, 2),
            "no limit · 86.4 MB/h at 1 ms step"
        );
        assert_eq!(fmt_bytes(412_300_000), "412 MB");
        assert_eq!(fmt_bytes(1_500_000_000), "1.5 GB");
        assert_eq!(fmt_bytes(999), "999 B");
        assert_eq!(fmt_hms(2470.0), "0:41:10");
        assert_eq!(fmt_hms(90_061.0), "1 d 1:01:01");
        assert_eq!(fmt_step(60.0), "1 min");
        assert_eq!(fmt_step(2.5e-6), "2.5 µs");
        assert_eq!(nearest(&STEPS, 0.0011), 8);
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
