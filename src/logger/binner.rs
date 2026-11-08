//! Turns the raw ADC stream into log records: min, max and mean per
//! channel for each step, or for raw logs the ADC values themselves.
//!
//! The binner works on the raw bytes: min, max and the sum per step are
//! taken in ADC counts, and only each record is turned into volts (the
//! calibration is a straight line, so this is exact). That keeps the
//! logger cheap at the top ADC rate.

use super::writer::Msg;
use crate::device::protocol::Decimator;

/// Bins raw samples into records. The records hold the volts at the probe
/// tip that the scope view shows (probe and invert applied).
#[derive(Clone, Debug)]
pub struct Binner {
    samples_per_step: u32,
    /// Channels in the stream (1 or 2).
    hw: usize,
    /// Logged channels, in file order.
    logged: Vec<usize>,
    /// Volts at the probe tip of an ADC count, `a * count + b`, per channel.
    lin: [(f64, f64); 2],
    /// Raw log: the ADC bytes are the records.
    raw: bool,
    dec: Decimator,
    /// Step number of the record being filled.
    index: u64,
    /// Samples per channel in it so far.
    pos: u32,
    /// Values of the record being filled: min, max, mean per channel.
    cur: [[f64; 3]; 2],
    /// A sample hit the end of the ADC range since the last
    /// [`Binner::take_clipped`].
    clipped: bool,
    /// Messages for the writer, made since the last [`Binner::take`].
    out: Vec<Msg>,
}

impl Binner {
    /// `hw` channels in the stream, `logged` channels to log, `lin` the
    /// calibration line per channel (volts at the probe tip per count).
    pub fn new(
        samples_per_step: u32,
        hw: u8,
        logged: Vec<usize>,
        lin: [(f64, f64); 2],
        raw: bool,
    ) -> Self {
        let sps = if raw { 1 } else { samples_per_step.max(1) };
        Self {
            samples_per_step: sps,
            hw: hw.clamp(1, 2) as usize,
            logged,
            lin,
            raw,
            dec: Decimator::new(hw, sps as usize, 0),
            index: 0,
            pos: 0,
            cur: [[0.0; 3]; 2],
            clipped: false,
            out: Vec::new(),
        }
    }

    /// Step number of the next record.
    pub fn next_index(&self) -> u64 {
        self.index
    }

    /// True when a sample hit the end of the range since the last call.
    pub fn take_clipped(&mut self) -> bool {
        std::mem::take(&mut self.clipped)
    }

    /// The f32 record list that step `index` goes on, new when needed.
    fn f32_out(out: &mut Vec<Msg>, index: u64, per: usize) -> &mut Vec<f32> {
        let fresh = match out.last() {
            Some(Msg::Records { first, data }) => *first + (data.len() / per) as u64 != index,
            _ => true,
        };
        if fresh {
            out.push(Msg::Records {
                first: index,
                data: Vec::new(),
            });
        }
        match out.last_mut() {
            Some(Msg::Records { data, .. }) => data,
            _ => unreachable!(),
        }
    }

    /// The raw record list that step `index` goes on, new when needed.
    fn raw_out(out: &mut Vec<Msg>, index: u64, per: usize) -> &mut Vec<u8> {
        let fresh = match out.last() {
            Some(Msg::Raw { first, data }) => *first + (data.len() / per) as u64 != index,
            _ => true,
        };
        if fresh {
            out.push(Msg::Raw {
                first: index,
                data: Vec::new(),
            });
        }
        match out.last_mut() {
            Some(Msg::Raw { data, .. }) => data,
            _ => unreachable!(),
        }
    }

    /// Close the record in `cur` (also a partial one).
    fn emit(&mut self) {
        emit_into(
            &mut self.out,
            self.index,
            &self.logged,
            &self.cur,
            self.samples_per_step == 1,
        );
        self.index += 1;
        self.pos = 0;
    }

    /// Add raw bytes: whole frames of `hw` interleaved channels.
    pub fn push(&mut self, bytes: &[u8]) {
        if self.raw {
            self.push_raw(bytes);
            return;
        }
        let frames = (bytes.len() / self.hw) as u64;
        let last = self.hw - 1;
        let single = self.samples_per_step == 1;
        let Binner {
            dec,
            out,
            cur,
            index,
            logged,
            lin,
            ..
        } = self;
        dec.push(bytes, |c, lo, hi, mean| {
            let (a, b) = lin[c];
            let (x, y) = (a * lo as f64 + b, a * hi as f64 + b);
            cur[c] = [x.min(y), x.max(y), a * mean + b];
            if c == last {
                emit_into(out, *index, logged, cur, single);
                *index += 1;
            }
        });
        let sps = self.samples_per_step as u64;
        self.pos = ((self.pos as u64 + frames) % sps) as u32;
        let clip = self.dec.clipped;
        self.dec.clipped = [false; 2];
        if self.logged.iter().any(|&c| clip[c]) {
            self.clipped = true;
        }
    }

    fn push_raw(&mut self, bytes: &[u8]) {
        let per = self.logged.len();
        let all = self.logged.len() == self.hw;
        let frames = bytes.len() / self.hw;
        if bytes.iter().any(|&b| b == 0 || b == 0xFF) {
            self.clipped = true;
        }
        let data = Self::raw_out(&mut self.out, self.index, per);
        if all {
            data.extend_from_slice(&bytes[..frames * self.hw]);
        } else {
            let c = self.logged[0];
            data.extend(bytes.chunks_exact(self.hw).map(|f| f[c]));
        }
        self.index += frames as u64;
    }

    /// Samples were lost: `missing` samples (an estimate) did not come.
    /// Closes the record being filled and goes on at the step where the
    /// next sample falls (at least the next step). The writer starts a new
    /// chunk marked as a gap.
    pub fn gap(&mut self, missing: u64) {
        let sps = self.samples_per_step as u64;
        let at = self.index * sps + self.pos as u64 + missing;
        self.finish();
        self.index = self.index.max(at.div_ceil(sps));
        self.out.push(Msg::Gap);
    }

    /// End of the log: close a partial record.
    pub fn finish(&mut self) {
        if self.pos > 0 {
            let lin = self.lin;
            let cur = &mut self.cur;
            self.dec.flush(|c, lo, hi, mean| {
                let (a, b) = lin[c];
                let (x, y) = (a * lo as f64 + b, a * hi as f64 + b);
                cur[c] = [x.min(y), x.max(y), a * mean + b];
            });
            self.emit();
            let clip = self.dec.clipped;
            if self.logged.iter().any(|&c| clip[c]) {
                self.clipped = true;
            }
        }
        self.dec.reset();
        self.pos = 0;
    }

    /// The messages made since the last call.
    pub fn take(&mut self) -> Vec<Msg> {
        std::mem::take(&mut self.out)
    }
}

/// Append the record of step `index` (values `cur` of the `logged`
/// channels) to the messages.
fn emit_into(out: &mut Vec<Msg>, index: u64, logged: &[usize], cur: &[[f64; 3]; 2], single: bool) {
    let per = logged.len() * if single { 1 } else { 3 };
    let data = Binner::f32_out(out, index, per);
    for &c in logged {
        let v = cur[c];
        if single {
            data.push(v[2] as f32);
        } else {
            data.extend_from_slice(&[v[0] as f32, v[1] as f32, v[2] as f32]);
        }
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

    /// Volts = count - 128 on CH1; CH2 x -10 (probe 10, inverted).
    const LIN: [(f64, f64); 2] = [(1.0, -128.0), (-10.0, 1280.0)];

    #[test]
    fn min_max_mean_per_step() {
        let mut b = Binner::new(4, 2, vec![0, 1], LIN, false);
        b.push(&[129, 128, 130, 129, 131, 130]);
        assert!(b.take().is_empty());
        b.push(&[132, 131, 133, 132]);
        let r = records(&b.take());
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, 0);
        let d = &r[0].1;
        assert_eq!(&d[..3], &[1.0, 4.0, 2.5]);
        // Inverted: the highest count is the lowest volts.
        assert_eq!(&d[3..6], &[-30.0, 0.0, -15.0]);
        assert_eq!(b.next_index(), 1);
        // The 133 is in record 1, which a finish closes.
        b.finish();
        let r = records(&b.take());
        assert_eq!(r[0].0, 1);
        assert_eq!(&r[0].1[..3], &[5.0, 5.0, 5.0]);
        assert!(!b.take_clipped());
        b.push(&[0, 128]);
        assert!(b.take_clipped());
    }

    #[test]
    fn single_sample_steps_store_the_value() {
        // CH2 alone in a 2-channel stream.
        let mut b = Binner::new(1, 2, vec![1], LIN, false);
        b.push(&[0, 128, 0, 127, 0, 126]);
        let r = records(&b.take());
        assert_eq!(r, vec![(0, vec![0.0, 10.0, 20.0])]);
        // CH1 is not logged: its 0x00 does not count as clipping.
        assert!(!b.take_clipped());
    }

    #[test]
    fn raw_steps_are_the_bytes() {
        let mut b = Binner::new(1, 2, vec![0, 1], LIN, true);
        b.push(&[1, 2, 3, 4]);
        b.push(&[5, 6]);
        assert_eq!(
            b.take(),
            vec![Msg::Raw {
                first: 0,
                data: vec![1, 2, 3, 4, 5, 6]
            }]
        );
        assert_eq!(b.next_index(), 3);
        // CH2 alone: every second byte.
        let mut b = Binner::new(1, 2, vec![1], LIN, true);
        b.push(&[1, 2, 3, 0xFF]);
        assert_eq!(
            b.take(),
            vec![Msg::Raw {
                first: 0,
                data: vec![2, 0xFF]
            }]
        );
        assert!(b.take_clipped());
        // A gap goes on at the step of the next sample.
        b.gap(10);
        b.push(&[7, 8]);
        let msgs = b.take();
        assert!(matches!(msgs[0], Msg::Gap));
        assert_eq!(
            msgs[1],
            Msg::Raw {
                first: 12,
                data: vec![8]
            }
        );
    }

    #[test]
    fn gaps_skip_steps() {
        let mut b = Binner::new(10, 1, vec![0], LIN, false);
        b.push(&[128; 25]);
        // 25 samples: records 0 and 1, 5 samples in record 2. Then 100
        // samples are lost: the next sample is sample 125, so the log goes
        // on at step 13.
        b.gap(100);
        b.push(&[129; 10]);
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
