//! Turns a stream of samples into log records: min, max and mean per
//! channel for each step.

use super::writer::Msg;

#[derive(Clone, Copy, Debug)]
struct Acc {
    min: f64,
    max: f64,
    sum: f64,
}

impl Acc {
    const EMPTY: Acc = Acc {
        min: f64::INFINITY,
        max: f64::NEG_INFINITY,
        sum: 0.0,
    };
}

/// Bins samples into records. Samples come in volts at the BNC; the
/// binner applies the probe and invert, so the records hold the volts
/// the scope view shows.
#[derive(Clone, Debug)]
pub struct Binner {
    samples_per_step: u32,
    /// Logged channels, in file order.
    logged: Vec<usize>,
    /// Probe times invert (+1 or -1), per channel.
    k: [f64; 2],
    /// Step number of the record being filled.
    index: u64,
    /// Samples in it so far.
    pos: u32,
    acc: [Acc; 2],
    /// Messages for the writer, made since the last [`Binner::take`].
    out: Vec<Msg>,
}

impl Binner {
    pub fn new(samples_per_step: u32, logged: Vec<usize>, k: [f64; 2]) -> Self {
        Self {
            samples_per_step: samples_per_step.max(1),
            logged,
            k,
            index: 0,
            pos: 0,
            acc: [Acc::EMPTY; 2],
            out: Vec::new(),
        }
    }

    /// Step number of the next record.
    pub fn next_index(&self) -> u64 {
        self.index
    }

    fn values(&mut self) -> &mut Vec<f32> {
        let index = self.index;
        let fresh = match self.out.last() {
            Some(Msg::Records { first, data }) => {
                let per = self.record_values();
                *first + (data.len() / per) as u64 != index
            }
            _ => true,
        };
        if fresh {
            self.out.push(Msg::Records {
                first: index,
                data: Vec::new(),
            });
        }
        match self.out.last_mut() {
            Some(Msg::Records { data, .. }) => data,
            _ => unreachable!(),
        }
    }

    fn record_values(&self) -> usize {
        self.logged.len() * if self.samples_per_step == 1 { 1 } else { 3 }
    }

    /// Close the record being filled (also a partial one).
    fn emit(&mut self) {
        let n = self.pos as f64;
        let acc = self.acc;
        let logged = self.logged.clone();
        let single = self.samples_per_step == 1;
        let data = self.values();
        for c in logged {
            let a = acc[c];
            if single {
                data.push(a.sum as f32);
            } else {
                data.extend_from_slice(&[a.min as f32, a.max as f32, (a.sum / n) as f32]);
            }
        }
        self.index += 1;
        self.pos = 0;
        self.acc = [Acc::EMPTY; 2];
    }

    /// Add samples. `data[c]` are the samples of channel `c` (both the
    /// same length, or empty for a channel that is not logged).
    pub fn push(&mut self, data: &[Vec<f64>; 2]) {
        let n = self
            .logged
            .iter()
            .map(|&c| data[c].len())
            .min()
            .unwrap_or(0);
        let sps = self.samples_per_step;
        #[allow(clippy::needless_range_loop)]
        for i in 0..n {
            for &c in &self.logged {
                let v = data[c][i] * self.k[c];
                let a = &mut self.acc[c];
                a.min = a.min.min(v);
                a.max = a.max.max(v);
                a.sum += v;
            }
            self.pos += 1;
            if self.pos == sps {
                self.emit();
            }
        }
    }

    /// Samples were lost: `missing` samples (an estimate) did not come.
    /// Closes the record being filled and goes on at the step where the
    /// next sample falls (at least the next step). The writer starts a new
    /// chunk marked as a gap.
    pub fn gap(&mut self, missing: u64) {
        let sps = self.samples_per_step as u64;
        let at = self.index * sps + self.pos as u64 + missing;
        if self.pos > 0 {
            self.emit();
        }
        self.index = self.index.max(at.div_ceil(sps));
        self.out.push(Msg::Gap);
    }

    /// End of the log: close a partial record.
    pub fn finish(&mut self) {
        if self.pos > 0 {
            self.emit();
        }
    }

    /// The messages made since the last call.
    pub fn take(&mut self) -> Vec<Msg> {
        std::mem::take(&mut self.out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records(msgs: &[Msg]) -> Vec<(u64, Vec<f32>)> {
        msgs.iter()
            .filter_map(|m| match m {
                Msg::Records { first, data } => Some((*first, data.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn min_max_mean_per_step() {
        let mut b = Binner::new(4, vec![0, 1], [1.0, -10.0]);
        b.push(&[vec![1.0, 2.0, 3.0], vec![0.1, 0.2, 0.3]]);
        assert!(b.take().is_empty());
        b.push(&[vec![4.0, 5.0], vec![0.4, 0.5]]);
        let r = records(&b.take());
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, 0);
        let d = &r[0].1;
        assert_eq!(&d[..3], &[1.0, 4.0, 2.5]);
        assert_eq!(&d[3..5], &[-4.0, -1.0]);
        assert!((d[5] + 2.5).abs() < 1e-6);
        assert_eq!(b.next_index(), 1);
        // The 5 is in record 1, which a finish closes.
        b.finish();
        let r = records(&b.take());
        assert_eq!(r[0].0, 1);
        assert_eq!(&r[0].1[..3], &[5.0, 5.0, 5.0]);
    }

    #[test]
    fn single_sample_steps_store_the_value() {
        let mut b = Binner::new(1, vec![1], [1.0, 2.0]);
        b.push(&[vec![], vec![1.0, 2.0, 3.0]]);
        let r = records(&b.take());
        assert_eq!(r, vec![(0, vec![2.0, 4.0, 6.0])]);
    }

    #[test]
    fn gaps_skip_steps() {
        let mut b = Binner::new(10, vec![0], [1.0, 1.0]);
        b.push(&[vec![0.0; 25], vec![]]);
        // 25 samples: records 0 and 1, 5 samples in record 2. Then 100
        // samples are lost: the next sample is sample 125, so the log goes
        // on at step 13.
        b.gap(100);
        b.push(&[vec![1.0; 10], vec![]]);
        let msgs = b.take();
        assert!(matches!(msgs[1], Msg::Gap));
        let r = records(&msgs);
        assert_eq!(r[0].0, 0);
        assert_eq!(r[0].1.len(), 9); // records 0, 1 and the partial 2
        assert_eq!(r[1], (13, vec![1.0, 1.0, 1.0]));
        // A gap of nothing on a step boundary keeps the step number.
        b.gap(0);
        assert_eq!(b.next_index(), 14);
    }
}
