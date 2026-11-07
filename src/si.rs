//! Number formatting with SI prefixes, like `200 µs` or `1.5 kHz`.

const PREFIXES: [(f64, &str); 9] = [
    (1e9, "G"),
    (1e6, "M"),
    (1e3, "k"),
    (1.0, ""),
    (1e-3, "m"),
    (1e-6, "µ"),
    (1e-9, "n"),
    (1e-12, "p"),
    (1e-15, "f"),
];

/// Format a value with an SI prefix and about 3 significant digits.
pub fn fmt_si(value: f64, unit: &str) -> String {
    if !value.is_finite() {
        return format!("-- {unit}");
    }
    if value == 0.0 {
        return format!("0 {unit}");
    }
    let (scale, prefix) = PREFIXES
        .iter()
        .find(|(scale, _)| value.abs() >= *scale * 0.9995)
        .copied()
        .unwrap_or((1e-15, "f"));
    let n = value / scale;
    let text = if n.abs() >= 99.95 {
        format!("{n:.0}")
    } else if n.abs() >= 9.995 {
        format!("{n:.1}")
    } else {
        format!("{n:.2}")
    };
    let text = if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    };
    format!("{text} {prefix}{unit}")
}

/// Like [`fmt_si`], but `None` shows as `--`.
pub fn fmt_opt(value: Option<f64>, unit: &str) -> String {
    match value {
        Some(v) => fmt_si(v, unit),
        None => format!("-- {unit}"),
    }
}

/// Format a ratio in percent with one decimal.
pub fn fmt_percent(value: Option<f64>) -> String {
    match value {
        Some(v) if v.is_finite() => format!("{:.1} %", v * 100.0),
        _ => "-- %".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes() {
        assert_eq!(fmt_si(200e-6, "s"), "200 µs");
        assert_eq!(fmt_si(1500.0, "Hz"), "1.5 kHz");
        assert_eq!(fmt_si(1.0, "V"), "1 V");
        assert_eq!(fmt_si(0.0, "V"), "0 V");
        assert_eq!(fmt_si(-0.05, "V"), "-50 mV");
        assert_eq!(fmt_si(48e6, "S/s"), "48 MS/s");
        assert_eq!(fmt_si(2.5e-9, "s"), "2.5 ns");
        assert_eq!(fmt_si(1.234567, "V"), "1.23 V");
        assert_eq!(fmt_si(12.34567, "V"), "12.3 V");
    }

    #[test]
    fn rounding_at_prefix_edge() {
        // 999.96 rounds up to 1000, so it must show as 1 k, not "1000".
        assert_eq!(fmt_si(999.96, "Hz"), "1 kHz");
        assert_eq!(fmt_si(0.99999, "V"), "1 V");
    }

    #[test]
    fn not_finite_and_none() {
        assert_eq!(fmt_si(f64::NAN, "V"), "-- V");
        assert_eq!(fmt_opt(None, "Hz"), "-- Hz");
        assert_eq!(fmt_percent(Some(0.25)), "25.0 %");
        assert_eq!(fmt_percent(None), "-- %");
    }
}
