//! Save the shown traces to a CSV file.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// One trace to write: a column name and its samples.
pub struct Column<'a> {
    pub name: &'a str,
    pub samples: &'a [f64],
}

/// Write traces as CSV. The first column is time in seconds, starting at
/// `t0` with `dt` between rows. Short columns get empty cells.
pub fn write_csv(
    out: &mut impl Write,
    t0: f64,
    dt: f64,
    columns: &[Column],
) -> std::io::Result<()> {
    write!(out, "time_s")?;
    for c in columns {
        write!(out, ",{}", c.name)?;
    }
    writeln!(out)?;
    let rows = columns.iter().map(|c| c.samples.len()).max().unwrap_or(0);
    for i in 0..rows {
        write!(out, "{:.9e}", t0 + i as f64 * dt)?;
        for c in columns {
            match c.samples.get(i) {
                Some(v) if v.is_finite() => write!(out, ",{v:.6}")?,
                _ => write!(out, ",")?,
            }
        }
        writeln!(out)?;
    }
    Ok(())
}

/// Save to `dir/openhantek-<timestamp>.csv` and return the path.
pub fn save_csv(dir: &Path, t0: f64, dt: f64, columns: &[Column]) -> std::io::Result<PathBuf> {
    let path = dir.join(format!("openhantek-{}.csv", timestamp(SystemTime::now())));
    let file = std::fs::File::create(&path)?;
    let mut out = std::io::BufWriter::new(file);
    write_csv(&mut out, t0, dt, columns)?;
    out.flush()?;
    Ok(path)
}

/// `YYYYMMDD-HHMMSS` in UTC.
pub fn timestamp(time: SystemTime) -> String {
    let secs = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
pub(crate) fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn csv_layout() {
        let mut out = Vec::new();
        let ch1 = [1.0, 2.0, 3.0];
        let ch2 = [0.5, f64::NAN];
        write_csv(
            &mut out,
            -1e-3,
            1e-3,
            &[
                Column {
                    name: "CH1_V",
                    samples: &ch1,
                },
                Column {
                    name: "CH2_V",
                    samples: &ch2,
                },
            ],
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "time_s,CH1_V,CH2_V");
        assert_eq!(lines[1], "-1.000000000e-3,1.000000,0.500000");
        assert_eq!(lines[2], "0.000000000e0,2.000000,");
        assert_eq!(lines[3], "1.000000000e-3,3.000000,");
        assert_eq!(lines.len(), 4);
    }

    #[test]
    fn timestamps() {
        assert_eq!(timestamp(UNIX_EPOCH), "19700101-000000");
        let t = UNIX_EPOCH + Duration::from_secs(1_794_573_296);
        assert_eq!(timestamp(t), "20261113-123456");
        // Leap day.
        let t = UNIX_EPOCH + Duration::from_secs(951_782_400);
        assert_eq!(timestamp(t), "20000229-000000");
    }

    #[test]
    fn save_writes_a_file() {
        let dir = std::env::temp_dir().join(format!("oht-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = save_csv(
            &dir,
            0.0,
            1.0,
            &[Column {
                name: "CH1_V",
                samples: &[1.0],
            }],
        )
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("time_s,CH1_V\n"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
