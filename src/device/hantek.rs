//! Driver for the DSO-6022BE with the OpenHantek6022 firmware. Works over
//! any [`Transport`]: real USB or the fake scope.

use std::time::Duration;

use anyhow::{bail, Context, Result};

use super::protocol::*;
use super::transport::{Chunk, StreamSetup, Transport};

/// One captured block or piece of a stream, in volts at the BNC.
#[derive(Clone, Debug, Default)]
pub struct Captured {
    /// Samples per channel: the mean of the raw samples of each output
    /// sample. CH2 is empty in 1-channel mode.
    pub data: [Vec<f64>; 2],
    /// Lowest and highest raw sample behind each output sample (the peak
    /// display). Empty when each output sample is one raw sample: then
    /// they are `data`.
    pub min: [Vec<f64>; 2],
    pub max: [Vec<f64>; 2],
    /// A raw sample hit 0x00 or 0xFF: the signal is too big for the range.
    pub clipped: [bool; 2],
    /// Output samples per second.
    pub rate: f64,
    /// Volts at the BNC of the lowest and highest ADC value, per channel:
    /// what the input can measure at the gain of the capture.
    pub range: [(f64, f64); 2],
    /// Samples per channel the host dropped just before this piece of a
    /// stream (it could not keep up). 0 for blocks.
    pub lost: u64,
    /// When a piece of a stream came off the bus. `None` for blocks.
    pub at: Option<std::time::Instant>,
}

/// A piece of the stream as raw ADC bytes (the logger bins them itself).
#[derive(Clone, Debug)]
pub struct RawPiece {
    /// Whole frames of interleaved samples (CH1, CH2, CH1, ... with two
    /// channels), after the unstable samples at the start of the stream.
    pub data: Vec<u8>,
    /// Samples per channel the host dropped just before this piece.
    pub lost: u64,
    /// When the piece came off the bus.
    pub at: std::time::Instant,
}

impl Live {
    /// Whole points (all channels) in the ring, after `head`.
    fn points(&self) -> usize {
        let n = self.cfg.channels.clamp(1, 2) as usize;
        self.ring[..n].iter().map(|r| r.len()).min().unwrap_or(0) - self.head
    }
}

/// Raw samples per channel thrown away after each start: the ADC is not
/// stable just after sampling starts.
pub const SKIP_SAMPLES: usize = 2048;

/// One output point of a channel in ADC counts: mean, min, max.
#[derive(Clone, Copy, Debug)]
struct Pt {
    mean: f32,
    min: u8,
    max: u8,
}

/// The running stream: the settings it was started with, the decimator,
/// and the points of the block being filled.
struct Live {
    cfg: HwConfig,
    dec: Decimator,
    /// Raw path: bytes still to throw away at the start.
    skip_bytes: usize,
    /// Block mode: the points of the next blocks, per channel, from
    /// `head` on.
    ring: [Vec<Pt>; 2],
    head: usize,
}

pub struct Scope6022<T: Transport> {
    t: T,
    pub cal: Calibration,
    pub serial: String,
    /// Settings the hardware has now. `None` = unknown, send all.
    sent: Option<HwConfig>,
    /// The async stream, while running.
    stream: Option<Live>,
}

impl<T: Transport> Scope6022<T> {
    pub fn new(t: T, serial: impl Into<String>) -> Self {
        Self {
            t,
            cal: Calibration::default(),
            serial: serial.into(),
            sent: None,
            stream: None,
        }
    }

    pub fn transport(&mut self) -> &mut T {
        &mut self.t
    }

    /// Read the calibration bytes from the EEPROM. Never writes.
    pub fn read_eeprom(&mut self) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; EEPROM_CAL_LEN];
        let n = self
            .t
            .control_in(REQ_EEPROM, EEPROM_CAL_ADDR, 0, &mut buf)
            .context("cannot read the calibration EEPROM")?;
        if n != EEPROM_CAL_LEN {
            bail!("EEPROM read gave {n} of {EEPROM_CAL_LEN} bytes");
        }
        Ok(buf)
    }

    /// Load calibration: EEPROM first, then the OpenHantek6022 ini text.
    /// A failed EEPROM read gives nominal values and a warning, not an error.
    pub fn load_calibration(&mut self, ini: Option<&str>) -> Vec<String> {
        let mut notes = Vec::new();
        self.cal = match self.read_eeprom() {
            Ok(bytes) => Calibration::from_eeprom(&bytes),
            Err(e) => {
                notes.push(format!("{e:#}; using nominal calibration"));
                Calibration::default()
            }
        };
        if let Some(text) = ini {
            let n = self.cal.apply_ini(text);
            if n > 0 {
                notes.push(format!(
                    "Using {n} values from the OpenHantek6022 calibration file"
                ));
            }
        }
        notes
    }

    /// Send the settings that changed. The firmware stops sampling on any
    /// command, so this also ends a running stream.
    pub fn configure(&mut self, cfg: &HwConfig) -> Result<()> {
        self.end_stream();
        let old = self.sent;
        let changed = |f: &dyn Fn(&HwConfig) -> u64| old.is_none_or(|o| f(&o) != f(cfg));
        if changed(&|c| c.cal_freq as u64) {
            self.t
                .control_out(REQ_SET_CAL_FREQ, 0, 0, &[cal_freq_code(cfg.cal_freq)])?;
        }
        if changed(&|c| coupling_byte(c.dc) as u64) {
            self.t
                .control_out(REQ_SET_COUPLING, 0, 0, &[coupling_byte(cfg.dc)])?;
        }
        if changed(&|c| c.channels as u64) {
            self.t
                .control_out(REQ_SET_CHANNELS, 0, 0, &[cfg.channels])?;
        }
        if changed(&|c| c.rate.id as u64) {
            self.t
                .control_out(REQ_SET_SAMPLERATE, 0, 0, &[cfg.rate.id, 0])?;
        }
        for ch in [1usize, 0] {
            let g = cfg.gains[ch].min(GAIN_STEPS - 1);
            if changed(&|c| GAIN_BYTE[c.gains[ch].min(GAIN_STEPS - 1)] as u64) {
                let req = if ch == 0 {
                    REQ_SET_GAIN_CH1
                } else {
                    REQ_SET_GAIN_CH2
                };
                self.t.control_out(req, 0, 0, &[GAIN_BYTE[g], g as u8])?;
            }
        }
        self.sent = Some(*cfg);
        Ok(())
    }

    /// Start sampling.
    pub fn start(&mut self) -> Result<()> {
        self.t.control_out(REQ_START, 0, 0, &[0x01])
    }

    /// Stop sampling (the LED goes red).
    pub fn stop(&mut self) -> Result<()> {
        self.end_stream();
        self.t.control_out(REQ_START, 0, 0, &[0x00])
    }

    /// Cancel the async stream's transfers, if one runs.
    fn end_stream(&mut self) {
        if self.stream.take().is_some() {
            self.t.stream_stop();
        }
    }

    /// Input range of each channel for `cfg`, with this scope's calibration.
    pub fn range(&self, cfg: &HwConfig) -> [(f64, f64); 2] {
        [0, 1].map(|c| self.cal.range(cfg.gains[c], c, cfg.high_speed()))
    }

    /// Sizes of the async stream for `cfg`: transfers of about 20 ms of
    /// data (one 512 B packet to 256 KiB), enough of them for about
    /// 250 ms (8 to 32), so at most 8 MiB in flight (Linux allows 16 MiB
    /// by default, `usbfs_memory_mb`). The host queues up to 2 s of data
    /// (4 to 64 MiB) for a slow consumer before it drops any.
    pub fn stream_setup(cfg: &HwConfig) -> StreamSetup {
        let byte_rate = cfg.rate.raw * cfg.channels.clamp(1, 2) as f64;
        let packets = (byte_rate * 0.02 / PACKET_SIZE as f64).ceil() as usize;
        let transfer = packets.clamp(1, 512) * PACKET_SIZE;
        let transfers = ((byte_rate * 0.25 / transfer as f64).ceil() as usize).clamp(8, 32);
        StreamSetup {
            endpoint: EP_BULK_IN,
            transfer,
            transfers,
            queue_max: ((byte_rate * 2.0) as usize).clamp(4 << 20, 64 << 20),
        }
    }

    /// Seconds of data in one stream transfer.
    pub fn transfer_secs(cfg: &HwConfig) -> f64 {
        Self::stream_setup(cfg).transfer as f64 / (cfg.rate.raw * cfg.channels.clamp(1, 2) as f64)
    }

    /// The next transfer of the stream for `cfg`. Starts the stream when
    /// none runs or it runs with other settings: sends the settings that
    /// changed, queues the async transfers, then starts the scope, so no
    /// data comes before a buffer is ready. On an error the stream ends;
    /// the next call starts it again.
    fn next_chunk(&mut self, cfg: &HwConfig) -> Result<Chunk> {
        if self.stream.as_ref().is_some_and(|l| l.cfg != *cfg) || self.sent != Some(*cfg) {
            self.configure(cfg)?;
        }
        if self.stream.is_none() {
            self.t.stream_start(Self::stream_setup(cfg))?;
            if let Err(e) = self.start() {
                self.t.stream_stop();
                return Err(e);
            }
            let ch = cfg.channels.clamp(1, 2);
            self.stream = Some(Live {
                cfg: *cfg,
                dec: Decimator::new(ch, cfg.rate.oversample, SKIP_SAMPLES),
                skip_bytes: SKIP_SAMPLES * ch as usize,
                ring: Default::default(),
                head: 0,
            });
        }
        let wait = Duration::from_secs_f64(Self::transfer_secs(cfg) * 2.0 + 1.0);
        match self.t.stream_next(wait) {
            Ok(Some(c)) => Ok(c),
            Ok(None) => {
                self.end_stream();
                Err(anyhow::Error::new(rusb::Error::Timeout).context(format!(
                    "no samples from the scope for {:.1} s",
                    wait.as_secs_f64()
                )))
            }
            Err(e) => {
                self.end_stream();
                Err(e.context("reading samples failed"))
            }
        }
    }

    /// Volts per ADC count and volts at count 0, per channel, for `cfg`.
    fn lines(&self, cfg: &HwConfig) -> [(f64, f64); 2] {
        [0, 1].map(|c| self.cal.linear(cfg.gains[c], c, cfg.high_speed()))
    }

    /// Capture one block of [`BLOCK_SAMPLES`] output samples per channel
    /// for the trigger. The scope streams without a stop (the async
    /// stream), the ADC at the rate of `cfg` (the top rate), and each
    /// piece is decimated as it comes into the points of the next blocks,
    /// so no raw data is kept. Blocks come in stream order, each right
    /// after the one before: one transfer can fill several (13 at 30 MS/s).
    /// When data was lost (the caller was more than the stream's 2 s queue
    /// behind), the points before the gap are dropped.
    pub fn capture(&mut self, cfg: &HwConfig) -> Result<Captured> {
        loop {
            let live = self.stream.as_mut().filter(|l| l.cfg == *cfg);
            if live.is_some_and(|l| l.points() >= BLOCK_SAMPLES) {
                break;
            }
            let chunk = self.next_chunk(cfg)?;
            let live = self.stream.as_mut().expect("stream is set");
            // Less than a block is left: move it to the front (cheap).
            let head = std::mem::take(&mut live.head);
            for r in &mut live.ring {
                r.drain(..head.min(r.len()));
            }
            if chunk.lost > 0 {
                live.ring[0].clear();
                live.ring[1].clear();
                live.dec.reset();
            }
            let ring = &mut live.ring;
            live.dec.push(&chunk.data, |c, min, max, mean| {
                ring[c].push(Pt {
                    mean: mean as f32,
                    min,
                    max,
                })
            });
        }
        let lines = self.lines(cfg);
        let live = self.stream.as_mut().expect("stream is set");
        let range = live.head..live.head + BLOCK_SAMPLES;
        let mut out = Captured {
            rate: cfg.rate.rate,
            ..Default::default()
        };
        let peak = cfg.rate.oversample > 1;
        for (c, &(a, b)) in lines.iter().enumerate().take(cfg.channels.clamp(1, 2) as usize) {
            let pts = &live.ring[c][range.clone()];
            out.data[c] = pts.iter().map(|p| a * p.mean as f64 + b).collect();
            if peak {
                // A negative slope would swap them; it is positive.
                out.min[c] = pts.iter().map(|p| a * p.min as f64 + b).collect();
                out.max[c] = pts.iter().map(|p| a * p.max as f64 + b).collect();
            }
            out.clipped[c] = pts.iter().any(|p| p.min == 0 || p.max == 0xFF);
        }
        live.head += BLOCK_SAMPLES;
        out.range = self.range(cfg);
        Ok(out)
    }

    /// Roll mode: the next piece of the stream (one transfer), decimated
    /// to the output rate of `cfg`, with min and max per output sample.
    pub fn stream(&mut self, cfg: &HwConfig) -> Result<Captured> {
        let chunk = self.next_chunk(cfg)?;
        let lines = self.lines(cfg);
        let live = self.stream.as_mut().expect("stream is set");
        let ch = cfg.channels.clamp(1, 2) as u64;
        let mut out = Captured {
            rate: cfg.rate.rate,
            lost: chunk.lost / ch,
            at: Some(chunk.at),
            ..Default::default()
        };
        if chunk.lost > 0 {
            live.dec.reset();
        }
        let peak = cfg.rate.oversample > 1;
        live.dec.clipped = [false; 2];
        let (data, min, max) = (&mut out.data, &mut out.min, &mut out.max);
        live.dec.push(&chunk.data, |c, lo, hi, mean| {
            let (a, b) = lines[c];
            data[c].push(a * mean + b);
            if peak {
                min[c].push(a * lo as f64 + b);
                max[c].push(a * hi as f64 + b);
            }
        });
        out.clipped = live.dec.clipped;
        out.range = self.range(cfg);
        Ok(out)
    }

    /// The logger: the next piece of the stream as raw ADC bytes, after
    /// the unstable samples at the start.
    pub fn stream_raw(&mut self, cfg: &HwConfig) -> Result<RawPiece> {
        let mut chunk = self.next_chunk(cfg)?;
        let live = self.stream.as_mut().expect("stream is set");
        let skip = live.skip_bytes.min(chunk.data.len());
        live.skip_bytes -= skip;
        chunk.data.drain(..skip);
        let ch = cfg.channels.clamp(1, 2) as usize;
        Ok(RawPiece {
            lost: chunk.lost / ch as u64,
            data: chunk.data,
            at: chunk.at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::fake::{FakeScope, Wave};

    /// Settings for a screen of `screen` seconds at the default max ADC
    /// rate.
    fn cfg(screen: f64, channels: u8, gains: [usize; 2]) -> HwConfig {
        HwConfig {
            rate: choose_rate(screen, channels, MAX_ADC_DEFAULT),
            channels,
            gains,
            dc: [true; 2],
            cal_freq: 1000,
            roll: false,
        }
    }

    fn scope(waves: [Wave; 2]) -> (Scope6022<FakeScope>, FakeScope) {
        let fake = FakeScope::new(waves);
        let mut s = Scope6022::new(fake.clone(), "TEST");
        s.load_calibration(None);
        (s, fake)
    }

    fn starts(fake: &FakeScope) -> usize {
        fake.lock()
            .calls
            .iter()
            .filter(|c| c.request == 0xE3 && c.data == [1])
            .count()
    }

    #[test]
    fn configure_sends_only_changes() {
        let (mut s, fake) = scope([Wave::Dc(0.0); 2]);
        let c = cfg(0.01, 2, [5, 5]);
        s.configure(&c).unwrap();
        let first: Vec<u8> = fake.lock().calls.iter().map(|c| c.request).collect();
        // EEPROM read, then E6 E5 E4 E2 E1 E0.
        assert_eq!(first, vec![0xA2, 0xE6, 0xE5, 0xE4, 0xE2, 0xE1, 0xE0]);
        fake.lock().calls.clear();
        s.configure(&c).unwrap();
        assert!(fake.lock().calls.is_empty());
        // 1 V to 2 V/div: same gain byte, nothing to send.
        s.configure(&cfg(0.01, 2, [6, 5])).unwrap();
        assert!(fake.lock().calls.is_empty());
        s.configure(&cfg(0.01, 2, [2, 5])).unwrap();
        let calls = fake.lock().calls.clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            (calls[0].request, calls[0].data.clone()),
            (0xE0, vec![10, 2])
        );
        // A new output rate with the same ADC rate sends nothing either.
        fake.lock().calls.clear();
        s.configure(&cfg(0.001, 2, [2, 5])).unwrap();
        assert!(fake.lock().calls.is_empty());
    }

    #[test]
    fn capture_gives_volts_from_the_top_adc_rate() {
        let (mut s, fake) = scope([Wave::Dc(1.5), Wave::Dc(-0.25)]);
        fake.lock().noise = 0.0;
        // 1 ms/div: 1 MS/s out of 16 MS/s.
        let c = cfg(0.01, 2, [5, 3]);
        assert_eq!(
            (c.rate.rate, c.rate.raw, c.rate.oversample),
            (1e6, 16e6, 16)
        );
        let block = s.capture(&c).unwrap();
        assert_eq!(block.data[0].len(), BLOCK_SAMPLES);
        assert_eq!(block.data[1].len(), BLOCK_SAMPLES);
        assert_eq!(block.min[0].len(), BLOCK_SAMPLES);
        assert_eq!(block.rate, 1e6);
        assert_eq!(fake.lock().rate_id, 16);
        // The fake adds its EEPROM offset; calibration removes it again.
        // Rounding to whole ADC counts leaves at most half a count.
        for v in &block.data[0] {
            assert!((v - 1.5).abs() <= 0.5 / 24.75 + 1e-9, "{v}");
        }
        for v in &block.data[1] {
            assert!((v + 0.25).abs() <= 0.5 / 126.25 + 1e-9, "{v}");
        }
        // The junk at the start was skipped, so nothing looks clipped.
        assert_eq!(block.clipped, [false, false]);
        // The stream keeps running: more blocks, one start.
        for _ in 0..3 {
            s.capture(&c).unwrap();
        }
        assert_eq!(starts(&fake), 1);
        assert!(fake.lock().started_with_stream);
    }

    #[test]
    fn blocks_follow_each_other_without_a_gap() {
        // A slow ramp: the time of each sample is in its value.
        let (mut s, fake) = scope([
            Wave::Triangle {
                freq: 20.0,
                amp: 2.0,
                dc: 0.0,
            },
            Wave::Dc(0.0),
        ]);
        fake.lock().noise = 0.0;
        // 100 µs/div: 8 MS/s out of 16, blocks of 2.5 ms.
        let c = cfg(1e-3, 2, [5, 5]);
        let a = s.capture(&c).unwrap();
        let b = s.capture(&c).unwrap();
        // Rising part of the triangle: b starts one step after a ends.
        let slope = 2.0 * 2.0 * 2.0 * 20.0 / 8e6; // V per output sample
        let step = b.data[0][0] - a.data[0][BLOCK_SAMPLES - 1];
        assert!((step - slope).abs() < 0.05, "{step}");
    }

    #[test]
    fn every_block_of_a_big_transfer_comes() {
        // 30 MS/s with CH1: a 256 KiB transfer holds 13 blocks. Each one
        // must come, in order: a ramp of 1 count per 1000 samples.
        let (mut s, fake) = scope([
            Wave::Triangle {
                freq: 30e6 / 2.0 / 120_000.0,
                amp: 2.4,
                dc: 0.0,
            },
            Wave::Dc(0.0),
        ]);
        fake.lock().noise = 0.0;
        let c = cfg(1e-6, 1, [5, 5]);
        assert_eq!((c.rate.raw, c.rate.oversample), (30e6, 1));
        let mut prev: Option<f64> = None;
        let mut jumps = 0;
        for _ in 0..30 {
            let b = s.capture(&c).unwrap();
            if let Some(p) = prev {
                // Next to the last sample of the block before.
                if (b.data[0][0] - p).abs() > 0.1 {
                    jumps += 1;
                }
            }
            prev = Some(b.data[0][BLOCK_SAMPLES - 1]);
        }
        assert_eq!(jumps, 0);
        assert_eq!(starts(&fake), 1);
    }

    #[test]
    fn peaks_are_the_raw_extremes() {
        // A 1 MHz square at 1 ms/div: 16 raw samples per point see both
        // levels; the mean is between them, min and max are the levels.
        let (mut s, fake) = scope([
            Wave::Square {
                freq: 1e6,
                low: -1.0,
                high: 1.0,
                duty: 0.5,
            },
            Wave::Dc(0.0),
        ]);
        fake.lock().noise = 0.0;
        let block = s.capture(&cfg(0.01, 2, [5, 5])).unwrap();
        let tol = 0.5 / 24.75 + 1e-9;
        for i in 0..BLOCK_SAMPLES {
            assert!(block.data[0][i].abs() < 0.2, "{}", block.data[0][i]);
            assert!((block.min[0][i] + 1.0).abs() <= tol);
            assert!((block.max[0][i] - 1.0).abs() <= tol);
        }
    }

    #[test]
    fn capture_single_channel_and_clipping() {
        let (mut s, fake) = scope([Wave::Dc(3.0), Wave::Dc(0.0)]);
        fake.lock().noise = 0.0;
        // 3 V at 100 mV/div (x10 gain) is far past the range.
        let c = cfg(1e-6, 1, [2, 2]);
        assert_eq!((c.rate.rate, c.rate.oversample), (30e6, 1));
        let block = s.capture(&c).unwrap();
        assert!(block.data[1].is_empty());
        assert!(block.min[0].is_empty());
        assert_eq!(block.data[0].len(), BLOCK_SAMPLES);
        assert!(block.clipped[0]);
        assert_eq!(fake.lock().channels, 1);
        assert_eq!(fake.lock().rate_id, 30);
    }

    #[test]
    fn lower_max_adc_rate() {
        let (mut s, fake) = scope([Wave::Dc(0.0); 2]);
        let mut c = cfg(0.01, 2, [5, 5]);
        c.rate = choose_rate(0.01, 2, 2e6);
        s.capture(&c).unwrap();
        assert_eq!(fake.lock().rate_id, 2);
    }

    #[test]
    fn roll_stream_continues() {
        let (mut s, fake) = scope([
            Wave::Sine {
                freq: 1.0,
                amp: 1.0,
                dc: 0.0,
            },
            Wave::Dc(0.0),
        ]);
        let mut c = cfg(1.0, 2, [5, 5]);
        c.roll = true;
        let mut total = 0;
        for _ in 0..5 {
            let piece = s.stream(&c).unwrap();
            assert_eq!(piece.data[0].len(), piece.data[1].len());
            assert_eq!(piece.data[0].len(), piece.max[0].len());
            total += piece.data[0].len();
        }
        assert!(total > 400, "total {total}");
        // The transfers were queued before the start command.
        assert!(fake.lock().started_with_stream);
        // The ADC runs at 16 MS/s x 2: 20 ms is 1250 packets, more than
        // the 512 of the biggest transfer; 250 ms is 31 transfers.
        let st = Scope6022::<FakeScope>::stream_setup(&c);
        assert_eq!((st.transfer, st.transfers), (512 * 512, 31));
        // One start for the whole stream.
        assert_eq!(starts(&fake), 1);
    }

    #[test]
    fn stream_sizes_stay_in_usbfs_memory() {
        for (rate_id, ch) in [(16, 2), (30, 1), (10, 2), (140, 2)] {
            let mut c = cfg(0.01, ch, [5, 5]);
            c.rate = Rate {
                rate: raw_rate_of_id(rate_id),
                id: rate_id,
                raw: raw_rate_of_id(rate_id),
                oversample: 1,
            };
            let st = Scope6022::<FakeScope>::stream_setup(&c);
            assert_eq!(st.transfer % PACKET_SIZE, 0);
            assert!(st.transfer * st.transfers <= 8 << 20, "{st:?}");
            // At least about 200 ms of data in flight, or 8 transfers.
            let secs = (st.transfer * st.transfers) as f64 / (c.rate.raw * ch as f64);
            assert!(secs > 0.2 || st.transfers == 8, "{st:?}");
        }
    }

    #[test]
    fn mode_and_setting_changes_restart_the_stream() {
        let (mut s, fake) = scope([Wave::Dc(0.0); 2]);
        let mut c = cfg(1.0, 2, [5, 5]);
        c.roll = true;
        s.stream(&c).unwrap();
        assert!(fake.lock().stream.is_some());
        c.roll = false;
        s.capture(&c).unwrap();
        assert_eq!(starts(&fake), 2);
        // A new gain: the firmware stops on the command, a new start.
        c.gains = [2, 5];
        s.capture(&c).unwrap();
        assert_eq!(starts(&fake), 3);
        s.stop().unwrap();
        assert!(fake.lock().stream.is_none());
    }

    #[test]
    fn raw_stream_skips_the_start() {
        let (mut s, fake) = scope([Wave::Dc(0.0); 2]);
        fake.lock().noise = 0.0;
        let mut c = cfg(1.0, 2, [5, 5]);
        c.rate.oversample = 1;
        c.roll = true;
        let mut n = 0;
        while n < 100_000 {
            let p = s.stream_raw(&c).unwrap();
            assert_eq!(p.data.len() % 2, 0);
            // No junk bytes from the start.
            assert!(p.data.iter().all(|b| (120..=135).contains(b)));
            n += p.data.len();
        }
    }

    #[test]
    fn eeprom_failure_is_a_warning() {
        let (mut s, fake) = scope([Wave::Dc(0.0); 2]);
        fake.lock().unplugged = true;
        let notes = s.load_calibration(None);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("nominal"));
        assert_eq!(s.cal, Calibration::default());
    }

    #[test]
    fn never_writes_eeprom() {
        let (mut s, fake) = scope([Wave::Dc(0.0); 2]);
        s.load_calibration(Some(
            "[eeprom]\nreplace_eeprom=true\n[offset]\nch0\\1000mV=2\n",
        ));
        s.capture(&cfg(0.01, 2, [5, 5])).unwrap();
        s.stop().unwrap();
        let writes = fake
            .lock()
            .calls
            .iter()
            .filter(|c| c.request == 0xA2 && c.request_type == 0x40)
            .count();
        assert_eq!(writes, 0);
    }
}
