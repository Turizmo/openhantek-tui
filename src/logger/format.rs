//! The log file layout. All numbers are little-endian.
//!
//! ```text
//! header   magic "OHTLOG\0\x01", version u16, header_len u16, flags u16,
//!          channel mask u8, reserved u8, start time i64 (ns since 1970
//!          UTC), step u64 (ns), ADC rate f64 (S/s), samples per step u32,
//!          reserved u32, then 2 channel blocks of 24 bytes (gain index u8,
//!          probe u8, invert u8, DC u8, volts/div f32, zero count f32,
//!          volts per count f32, reserved 8), then the device name (u16
//!          length + UTF-8), padded with zeros to header_len (a multiple
//!          of 8).
//! chunks   any number of: magic "CHNK", flags u32 (bit 0: a gap comes
//!          before this chunk), first step u64, record count u64, then the
//!          records. A count of u64::MAX means "open": the records go on
//!          to the end of the file (the last chunk while recording, or
//!          after a crash). A partial record at the end is ignored.
//! records  one per step, for each logged channel (CH1 first): min, max,
//!          mean as f32 volts at the probe tip. With one sample per step
//!          only the value (min = max = mean).
//! ```
//!
//! Step `i` covers the time `start + i * step` to `start + (i + 1) *
//! step`. Missing step numbers between chunks are gaps (no data).

use anyhow::{bail, Result};

pub const MAGIC: [u8; 8] = *b"OHTLOG\0\x01";
pub const VERSION: u16 = 1;
pub const CHUNK_MAGIC: [u8; 4] = *b"CHNK";
pub const CHUNK_HEADER: usize = 24;
/// Record count of an open chunk.
pub const OPEN: u64 = u64::MAX;
/// Chunk flag: data was lost just before this chunk.
pub const FLAG_GAP: u32 = 1;

/// What a channel was set to while logging.
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelInfo {
    pub enabled: bool,
    /// Gain index (0 = 20 mV/div range ... 7 = 5 V/div range).
    pub gain: u8,
    pub probe: u8,
    pub invert: bool,
    pub dc: bool,
    /// Volts per division at the probe tip, when the log started.
    pub volts_per_div: f32,
    /// ADC count of 0 V, from the calibration.
    pub zero_count: f32,
    /// Volts at the BNC per ADC count, from the calibration.
    pub volts_per_count: f32,
}

impl Default for ChannelInfo {
    fn default() -> Self {
        Self {
            enabled: false,
            gain: 5,
            probe: 1,
            invert: false,
            dc: true,
            volts_per_div: 1.0,
            zero_count: 128.0,
            volts_per_count: 0.04,
        }
    }
}

impl ChannelInfo {
    /// Lowest and highest volts at the probe tip the ADC can show.
    pub fn range(&self) -> (f64, f64) {
        let k = self.probe as f64 * if self.invert { -1.0 } else { 1.0 };
        let a = (0.0 - self.zero_count as f64) * self.volts_per_count as f64 * k;
        let b = (255.0 - self.zero_count as f64) * self.volts_per_count as f64 * k;
        (a.min(b), a.max(b))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    /// Time of the first sample, in ns since 1970-01-01 UTC.
    pub start_ns: i64,
    pub step_ns: u64,
    /// ADC samples per second.
    pub adc_rate: f64,
    pub samples_per_step: u32,
    pub channels: [ChannelInfo; 2],
    /// Like `DSO-6022BE 0694CE656429 FW0210`.
    pub device: String,
}

impl Header {
    /// Logged channels (0 = CH1, 1 = CH2), in file order.
    pub fn logged(&self) -> Vec<usize> {
        (0..2).filter(|&c| self.channels[c].enabled).collect()
    }

    pub fn nch(&self) -> usize {
        self.logged().len()
    }

    /// One value per channel per record (min = max = mean).
    pub fn single(&self) -> bool {
        self.samples_per_step == 1
    }

    pub fn record_bytes(&self) -> usize {
        super::record_bytes(self.nch(), self.samples_per_step)
    }

    /// The step in seconds.
    pub fn step(&self) -> f64 {
        self.step_ns as f64 * 1e-9
    }

    /// Start time in seconds since 1970 UTC.
    pub fn start_secs(&self) -> f64 {
        self.start_ns as f64 * 1e-9
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(128);
        b.extend_from_slice(&MAGIC);
        b.extend_from_slice(&VERSION.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes()); // header_len, set below
        b.extend_from_slice(&0u16.to_le_bytes()); // flags
        let mask = self.channels[0].enabled as u8 | (self.channels[1].enabled as u8) << 1;
        b.push(mask);
        b.push(0);
        b.extend_from_slice(&self.start_ns.to_le_bytes());
        b.extend_from_slice(&self.step_ns.to_le_bytes());
        b.extend_from_slice(&self.adc_rate.to_le_bytes());
        b.extend_from_slice(&self.samples_per_step.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        for c in &self.channels {
            b.extend_from_slice(&[c.gain, c.probe, c.invert as u8, c.dc as u8]);
            b.extend_from_slice(&c.volts_per_div.to_le_bytes());
            b.extend_from_slice(&c.zero_count.to_le_bytes());
            b.extend_from_slice(&c.volts_per_count.to_le_bytes());
            b.extend_from_slice(&[0; 8]);
        }
        let name = self.device.as_bytes();
        let name = &name[..name.len().min(1000)];
        b.extend_from_slice(&(name.len() as u16).to_le_bytes());
        b.extend_from_slice(name);
        while b.len() % 8 != 0 {
            b.push(0);
        }
        let len = b.len() as u16;
        b[10..12].copy_from_slice(&len.to_le_bytes());
        b
    }

    /// Read a header. Returns it and the header length (where the
    /// chunks start).
    pub fn decode(b: &[u8]) -> Result<(Header, usize)> {
        if b.len() < 98 || b[..8] != MAGIC {
            bail!("not an openhantek-tui log file");
        }
        let u16_at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
        let f32_at = |i: usize| f32::from_le_bytes(b[i..i + 4].try_into().unwrap());
        let version = u16_at(8);
        if version != VERSION {
            bail!("log file version {version} is not supported (this program reads {VERSION})");
        }
        let len = u16_at(10) as usize;
        if len < 98 || len > b.len() {
            bail!("log file header is cut off");
        }
        let mask = b[14];
        let start_ns = i64::from_le_bytes(b[16..24].try_into().unwrap());
        let step_ns = u64::from_le_bytes(b[24..32].try_into().unwrap());
        let adc_rate = f64::from_le_bytes(b[32..40].try_into().unwrap());
        let samples_per_step = u32::from_le_bytes(b[40..44].try_into().unwrap());
        if step_ns == 0 || samples_per_step == 0 || mask & 3 == 0 {
            bail!("log file header has bad values");
        }
        let ch = |c: usize| {
            let o = 48 + c * 24;
            ChannelInfo {
                enabled: mask & (1 << c) != 0,
                gain: b[o],
                probe: b[o + 1],
                invert: b[o + 2] != 0,
                dc: b[o + 3] != 0,
                volts_per_div: f32_at(o + 4),
                zero_count: f32_at(o + 8),
                volts_per_count: f32_at(o + 12),
            }
        };
        let n = u16_at(96) as usize;
        let device = String::from_utf8_lossy(&b[98..(98 + n).min(len)]).into_owned();
        Ok((
            Header {
                start_ns,
                step_ns,
                adc_rate,
                samples_per_step,
                channels: [ch(0), ch(1)],
                device,
            },
            len,
        ))
    }
}

/// The head of a chunk of records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkHead {
    pub flags: u32,
    pub first: u64,
    /// Records in the chunk, or [`OPEN`].
    pub count: u64,
}

impl ChunkHead {
    pub fn encode(&self) -> [u8; CHUNK_HEADER] {
        let mut b = [0u8; CHUNK_HEADER];
        b[..4].copy_from_slice(&CHUNK_MAGIC);
        b[4..8].copy_from_slice(&self.flags.to_le_bytes());
        b[8..16].copy_from_slice(&self.first.to_le_bytes());
        b[16..24].copy_from_slice(&self.count.to_le_bytes());
        b
    }

    pub fn decode(b: &[u8]) -> Option<ChunkHead> {
        if b.len() < CHUNK_HEADER || b[..4] != CHUNK_MAGIC {
            return None;
        }
        Some(ChunkHead {
            flags: u32::from_le_bytes(b[4..8].try_into().unwrap()),
            first: u64::from_le_bytes(b[8..16].try_into().unwrap()),
            count: u64::from_le_bytes(b[16..24].try_into().unwrap()),
        })
    }

    pub fn gap(&self) -> bool {
        self.flags & FLAG_GAP != 0
    }
}

/// One record: min, max, mean for CH1 and CH2 (NaN for a channel that
/// is not logged).
pub type Rec = [[f32; 3]; 2];

/// Unpack the records in `bytes` (whole records only).
pub fn decode_records(h: &Header, bytes: &[u8], out: &mut Vec<Rec>) {
    let size = h.record_bytes();
    let logged = h.logged();
    let per = if h.single() { 1 } else { 3 };
    for r in bytes.chunks_exact(size) {
        let mut rec = [[f32::NAN; 3]; 2];
        for (k, &c) in logged.iter().enumerate() {
            let at = |j: usize| {
                let o = (k * per + j) * 4;
                f32::from_le_bytes(r[o..o + 4].try_into().unwrap())
            };
            rec[c] = if per == 1 {
                let v = at(0);
                [v, v, v]
            } else {
                [at(0), at(1), at(2)]
            };
        }
        out.push(rec);
    }
}

#[cfg(test)]
pub(crate) fn test_header(channels: [bool; 2], step: f64) -> Header {
    let (rate, n) = super::adc_for_step(super::step_ns(step), super::hw_channels(channels));
    let mut ch = [ChannelInfo::default(), ChannelInfo::default()];
    ch[0].enabled = channels[0];
    ch[1].enabled = channels[1];
    Header {
        start_ns: 1_794_573_296_000_000_000,
        step_ns: super::step_ns(step),
        adc_rate: rate.raw,
        samples_per_step: n,
        channels: ch,
        device: "TEST".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trip() {
        let mut h = test_header([true, false], 1e-3);
        h.channels[0].probe = 10;
        h.channels[0].invert = true;
        h.channels[1].gain = 2;
        h.device = "DSO-6022BE 0694CE656429 FW0210".into();
        let b = h.encode();
        assert_eq!(b.len() % 8, 0);
        let (back, len) = Header::decode(&b).unwrap();
        assert_eq!(back, h);
        assert_eq!(len, b.len());
        assert_eq!(back.record_bytes(), 12);
        assert!(Header::decode(&b[..50]).is_err());
        let mut bad = b.clone();
        bad[0] = b'X';
        assert!(Header::decode(&bad).is_err());
    }

    #[test]
    fn chunk_and_records() {
        let c = ChunkHead {
            flags: FLAG_GAP,
            first: 123,
            count: OPEN,
        };
        assert_eq!(ChunkHead::decode(&c.encode()), Some(c));
        assert!(c.gap());
        let mut h = test_header([false, true], 2.5e-6);
        // One sample per step, as at 2.5 µs and 400 kS/s in older logs.
        h.samples_per_step = 1;
        assert!(h.single());
        let bytes: Vec<u8> = [1.5f32, -2.0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let mut out = Vec::new();
        decode_records(&h, &bytes[..3], &mut out);
        assert!(out.is_empty());
        decode_records(&h, &bytes, &mut out);
        assert_eq!(out.len(), 2);
        assert!(out[0][0][0].is_nan());
        assert_eq!(out[0][1], [1.5; 3]);
        assert_eq!(out[1][1], [-2.0; 3]);
    }

    #[test]
    fn channel_range() {
        let c = ChannelInfo {
            probe: 10,
            ..Default::default()
        };
        let (lo, hi) = c.range();
        assert!(
            (lo + 51.2).abs() < 1e-3 && (hi - 50.8).abs() < 1e-3,
            "{lo} {hi}"
        );
    }
}
