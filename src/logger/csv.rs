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

/// `2026-11-13T12:34:56.123Z` for ns since 1970, with `digits` digits.
pub fn iso_utc(ns: i64, digits: usize) -> String {
    let secs = ns.div_euclid(1_000_000_000);
    let frac = ns.rem_euclid(1_000_000_000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = crate::export::civil_from_days(days);
    let frac = format!("{frac:09}");
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}{}{}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        if digits > 0 { "." } else { "" },
        &frac[..digits.min(9)]
    )
}

fn time_cell(f: &LogFile, step: u64, time: CsvTime, digits: usize) -> String {
    let ns = step as i128 * f.header.step_ns as i128;
    match time {
        CsvTime::Iso => iso_utc((f.header.start_ns as i128 + ns) as i64, digits),
        CsvTime::Seconds => {
            let s = ns / 1_000_000_000;
            let frac = format!("{:09}", ns % 1_000_000_000);
            format!("{s}.{}", &frac[..digits])
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
    let digits = time_digits(h.step_ns);
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
