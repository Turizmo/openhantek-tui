//! CSV export of a log.
//!
//! Columns: `time`, then for each logged channel `ch1_min`, `ch1_max`,
//! `ch1_mean` (a log with one sample per step has only `ch1`, `ch2`).
//! The time is the start of the step, as an ISO-8601 UTC timestamp
//! (`time_utc`) or in seconds since the start of the log (`time_s`). Each
//! gap is one row with the time where the data stops and empty value
//! cells, so plotting tools break the line there.

use std::io::Write;
use std::ops::Range;
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::reader::LogFile;
use crate::settings::CsvTime;

/// Digits after the decimal point that show every step exactly (at
/// least milliseconds).
pub fn time_digits(step_ns: u64) -> usize {
    (3..=9)
        .find(|&d| step_ns.is_multiple_of(10u64.pow(9 - d as u32)))
        .unwrap_or(9)
}

/// Digits of the time column for a log: like [`time_digits`], and 10
/// (0.1 ns) for raw logs, whose steps are the ADC sample time (62.5 ns,
/// 33.3 ns).
pub fn log_time_digits(h: &super::format::Header) -> usize {
    if h.raw {
        10
    } else {
        time_digits(h.step_ns)
    }
}

/// `2026-11-13T12:34:56.123Z` for ns since 1970, with `digits` digits.
pub fn iso_utc(ns: i64, digits: usize) -> String {
    iso_utc_ps(ns as i128 * 1000, digits)
}

/// Like [`iso_utc`] for ps since 1970, with up to 12 digits.
pub fn iso_utc_ps(ps: i128, digits: usize) -> String {
    const PS: i128 = 1_000_000_000_000;
    let secs = ps.div_euclid(PS) as i64;
    let frac = ps.rem_euclid(PS);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = crate::export::civil_from_days(days);
    let frac = format!("{frac:012}");
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}{}{}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        if digits > 0 { "." } else { "" },
        &frac[..digits.min(12)]
    )
}

fn time_cell(f: &LogFile, step: u64, time: CsvTime, digits: usize) -> String {
    // Rounded to the last digit shown.
    let unit = 10i128.pow(12 - digits.min(12) as u32);
    let ps = (f.header.step_ps(step) + unit / 2) / unit * unit;
    match time {
        CsvTime::Iso => iso_utc_ps(f.header.start_ns as i128 * 1000 + ps, digits),
        CsvTime::Seconds => {
            let s = ps / 1_000_000_000_000;
            let frac = format!("{:012}", ps % 1_000_000_000_000);
            format!("{s}.{}", &frac[..digits.min(12)])
        }
    }
}

/// Write the steps in `range` as CSV. Returns the rows written.
pub fn write_log_csv(
    out: &mut impl Write,
    f: &LogFile,
    range: Range<u64>,
    time: CsvTime,
) -> Result<u64> {
    let h = &f.header;
    let logged = h.logged();
    write!(
        out,
        "{}",
        match time {
            CsvTime::Iso => "time_utc",
            CsvTime::Seconds => "time_s",
        }
    )?;
    for &c in &logged {
        if h.single() {
            write!(out, ",ch{}", c + 1)?;
        } else {
            write!(out, ",ch{0}_min,ch{0}_max,ch{0}_mean", c + 1)?;
        }
    }
    writeln!(out)?;
    let cells = logged.len() * if h.single() { 1 } else { 3 };
    let empty = ",".repeat(cells);
    let digits = log_time_digits(h);
    let mut gaps = f.gaps().into_iter().peekable();
    let mut rows = 0u64;
    let mut err = None;
    let mut line = String::new();
    f.for_each(range.clone(), |first, recs| {
        if err.is_some() {
            return;
        }
        for (k, r) in recs.iter().enumerate() {
            let i = first + k as u64;
            // Gap rows before this record.
            while let Some(&(from, _)) = gaps.peek() {
                if from > i {
                    break;
                }
                gaps.next();
                if from > range.start {
                    line.clear();
                    line.push_str(&time_cell(f, from, time, digits));
                    line.push_str(&empty);
                    line.push('\n');
                    if let Err(e) = out.write_all(line.as_bytes()) {
                        err = Some(e);
                        return;
                    }
                    rows += 1;
                }
            }
            line.clear();
            line.push_str(&time_cell(f, i, time, digits));
            for &c in &logged {
                if h.single() {
                    line.push_str(&format!(",{:.6}", r[c][2]));
                } else {
                    let [lo, hi, mean] = r[c];
                    line.push_str(&format!(",{lo:.6},{hi:.6},{mean:.6}"));
                }
            }
            line.push('\n');
            if let Err(e) = out.write_all(line.as_bytes()) {
                err = Some(e);
                return;
            }
            rows += 1;
        }
    })?;
    if let Some(e) = err {
        return Err(e.into());
    }
    Ok(rows)
}

/// Save to `dir/openhantek-log-<timestamp>.csv` and return the path.
pub fn save_log_csv(dir: &Path, f: &LogFile, range: Range<u64>, time: CsvTime) -> Result<PathBuf> {
    let stamp = crate::export::timestamp(std::time::SystemTime::now());
    let mut path = dir.join(format!("openhantek-log-{stamp}.csv"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("openhantek-log-{stamp}-{n}.csv"));
        n += 1;
    }
    let file = std::fs::File::create(&path)?;
    let mut out = std::io::BufWriter::new(file);
    write_log_csv(&mut out, f, range, time)?;
    out.flush()?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_texts() {
        assert_eq!(time_digits(2500), 7);
        assert_eq!(time_digits(1_000_000), 3);
        assert_eq!(time_digits(60_000_000_000), 3);
        assert_eq!(time_digits(20_000), 5);
        assert_eq!(
            iso_utc(1_794_573_296_123_456_789, 3),
            "2026-11-13T12:34:56.123Z"
        );
        assert_eq!(iso_utc(0, 0), "1970-01-01T00:00:00Z");
        assert_eq!(
            iso_utc_ps(62_500, 10),
            "1970-01-01T00:00:00.0000000625Z"
        );
    }

    #[test]
    fn exports_raw_logs_in_volts() {
        use super::super::format::{test_raw_header, ChunkHead, OPEN};
        let dir = std::env::temp_dir().join(format!("oht-csv-raw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("r.ohlog");
        let mut h = test_raw_header([true, false]);
        h.adc_rate = 30e6;
        h.channels[0].zero_count = 128.0;
        h.channels[0].volts_per_count = 0.04;
        let mut b = h.encode();
        b.extend_from_slice(
            &ChunkHead {
                flags: 0,
                first: 0,
                count: OPEN,
            }
            .encode(),
        );
        b.extend_from_slice(&[128, 153, 103]);
        std::fs::write(&path, b).unwrap();
        let f = LogFile::open(&path).unwrap();
        assert_eq!(f.end(), 3);
        let mut out = Vec::new();
        write_log_csv(&mut out, &f, 0..3, CsvTime::Seconds).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "time_s,ch1\n\
             0.0000000000,0.000000\n\
             0.0000000333,1.000000\n\
             0.0000000667,-1.000000\n"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn exports_rows_gaps_and_ranges() {
        use super::super::format::test_header;
        use super::super::reader::tests::write_log;
        let dir = std::env::temp_dir().join(format!("oht-csv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.ohlog");
        let h = test_header([false, true], 1e-3);
        let rec = |v: f32| vec![v - 1.0, v + 1.0, v];
        write_log(
            &path,
            &h,
            &[
                (0, [rec(1.0), rec(2.0)].concat(), true),
                (4, rec(5.0), false),
            ],
        );
        let f = LogFile::open(&path).unwrap();
        let mut out = Vec::new();
        let rows = write_log_csv(&mut out, &f, 0..f.end(), CsvTime::Seconds).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(rows, 4);
        assert_eq!(
            text,
            "time_s,ch2_min,ch2_max,ch2_mean\n\
             0.000,0.000000,2.000000,1.000000\n\
             0.001,1.000000,3.000000,2.000000\n\
             0.002,,,\n\
             0.004,4.000000,6.000000,5.000000\n"
        );
        let mut out = Vec::new();
        write_log_csv(&mut out, &f, 1..2, CsvTime::Iso).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(
            text,
            "time_utc,ch2_min,ch2_max,ch2_mean\n2026-11-13T12:34:56.001Z,1.000000,3.000000,2.000000\n"
        );
        let saved = save_log_csv(&dir, &f, 0..f.end(), CsvTime::Iso).unwrap();
        assert!(saved
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("openhantek-log-"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
