//! Math channel: a trace made from CH1 and CH2.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MathMode {
    #[default]
    Add,
    Sub,
    SubRev,
    Mul,
    Div,
    /// The mean of CH1 and CH2.
    Avg,
    /// -CH1.
    NegCh1,
    /// -CH2.
    NegCh2,
}

impl MathMode {
    pub const ALL: [MathMode; 8] = [
        MathMode::Add,
        MathMode::Sub,
        MathMode::SubRev,
        MathMode::Mul,
        MathMode::Div,
        MathMode::Avg,
        MathMode::NegCh1,
        MathMode::NegCh2,
    ];

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|m| *m == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub fn name(self) -> &'static str {
        match self {
            MathMode::Add => "CH1+CH2",
            MathMode::Sub => "CH1-CH2",
            MathMode::SubRev => "CH2-CH1",
            MathMode::Mul => "CH1×CH2",
            MathMode::Div => "CH1÷CH2",
            MathMode::Avg => "(CH1+CH2)/2",
            MathMode::NegCh1 => "-CH1",
            MathMode::NegCh2 => "-CH2",
        }
    }

    /// Unit of the result.
    pub fn unit(self) -> &'static str {
        match self {
            MathMode::Mul => "V²",
            MathMode::Div => "",
            _ => "V",
        }
    }

    /// True when the result needs CH2 data.
    pub fn uses_ch2(self) -> bool {
        self != MathMode::NegCh1
    }

    /// True when the result needs CH1 data.
    pub fn uses_ch1(self) -> bool {
        self != MathMode::NegCh2
    }

    pub fn apply(self, a: f64, b: f64) -> f64 {
        match self {
            MathMode::Add => a + b,
            MathMode::Sub => a - b,
            MathMode::SubRev => b - a,
            MathMode::Mul => a * b,
            MathMode::Div => {
                if b.abs() < 1e-6 {
                    f64::NAN
                } else {
                    a / b
                }
            }
            MathMode::Avg => (a + b) / 2.0,
            MathMode::NegCh1 => -a,
            MathMode::NegCh2 => -b,
        }
    }
}

/// Apply `mode` sample by sample. A missing input counts as zeros.
pub fn combine(mode: MathMode, ch1: &[f64], ch2: &[f64]) -> Vec<f64> {
    let n = match (mode.uses_ch1(), mode.uses_ch2()) {
        (true, true) => ch1.len().min(ch2.len()),
        (true, false) => ch1.len(),
        _ => ch2.len(),
    };
    (0..n)
        .map(|i| {
            let a = ch1.get(i).copied().unwrap_or(0.0);
            let b = ch2.get(i).copied().unwrap_or(0.0);
            mode.apply(a, b)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes() {
        let a = [1.0, 2.0, 3.0];
        let b = [0.5, 0.5, 2.0];
        assert_eq!(combine(MathMode::Add, &a, &b), vec![1.5, 2.5, 5.0]);
        assert_eq!(combine(MathMode::Sub, &a, &b), vec![0.5, 1.5, 1.0]);
        assert_eq!(combine(MathMode::SubRev, &a, &b), vec![-0.5, -1.5, -1.0]);
        assert_eq!(combine(MathMode::Mul, &a, &b), vec![0.5, 1.0, 6.0]);
        assert_eq!(combine(MathMode::Div, &a, &b), vec![2.0, 4.0, 1.5]);
        assert_eq!(combine(MathMode::Avg, &a, &b), vec![0.75, 1.25, 2.5]);
        assert_eq!(combine(MathMode::NegCh1, &a, &[]), vec![-1.0, -2.0, -3.0]);
        assert_eq!(combine(MathMode::NegCh2, &[], &b), vec![-0.5, -0.5, -2.0]);
    }

    #[test]
    fn divide_by_zero_is_nan() {
        assert!(MathMode::Div.apply(1.0, 0.0).is_nan());
    }

    #[test]
    fn length_is_shortest_input() {
        assert_eq!(combine(MathMode::Add, &[1.0, 2.0], &[1.0]).len(), 1);
    }

    #[test]
    fn cycle_visits_all() {
        let mut m = MathMode::Add;
        let mut seen = vec![m];
        for _ in 1..MathMode::ALL.len() {
            m = m.next();
            seen.push(m);
        }
        assert_eq!(seen, MathMode::ALL.to_vec());
        assert_eq!(m.next(), MathMode::Add);
    }
}
