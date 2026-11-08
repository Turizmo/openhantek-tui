//! Driver for the DSO-6022BE with the OpenHantek6022 firmware. Works over
//! any [`Transport`]: real USB or the fake scope.

use std::time::Duration;

use anyhow::{bail, Context, Result};

use super::protocol::*;
use super::transport::{StreamSetup, Transport};

/// One captured block, in volts at the BNC.
#[derive(Clone, Debug, Default)]
pub struct Captured {
    /// Samples per channel. CH2 is empty in 1-channel mode.
    pub data: [Vec<f64>; 2],
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

pub struct Scope6022<T: Transport> {
    t: T,
    pub cal: Calibration,
    pub serial: String,
    /// Settings the hardware has now. `None` = unknown, send all.
    sent: Option<HwConfig>,
    /// Roll mode stream, while running.
    stream: Option<Converter>,
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
        let rate_index = RATES.iter().position(|r| *r == cfg.rate).unwrap_or(0) as u8;
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
                .control_out(REQ_SET_SAMPLERATE, 0, 0, &[cfg.rate.id, rate_index])?;
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

    /// Start sampling. Needed before every block.
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

    /// Time to wait for `bytes` of samples, with a good margin.
    fn timeout_for(cfg: &HwConfig, bytes: usize) -> Duration {
        let secs = bytes as f64 / (cfg.rate.raw * cfg.channels as f64);
        Duration::from_secs_f64(secs * 2.0 + 1.0)
    }

    /// Input range of each channel for `cfg`, with this scope's calibration.
    pub fn range(&self, cfg: &HwConfig) -> [(f64, f64); 2] {
        [0, 1].map(|c| self.cal.range(cfg.gains[c], c, cfg.high_speed()))
    }

    /// Capture one block. The whole block is one bulk transfer: libusb
    /// keeps the pipe busy, so no samples are lost inside the block.
    pub fn capture(&mut self, cfg: &HwConfig) -> Result<Captured> {
        if self.sent != Some(*cfg) {
            self.configure(cfg)?;
        }
        self.end_stream();
        let gross = cfg.gross();
        let bytes = cfg.block_bytes();
        let mut raw = vec![0u8; bytes];
        self.start()?;
        let n = self
            .t
            .bulk_in(EP_BULK_IN, &mut raw, Self::timeout_for(cfg, bytes))
            .context("reading samples failed")?;
        if n < bytes {
            bail!("short block from the scope: {n} of {bytes} bytes");
        }
        let mut conv = Converter::new(cfg, skip_samples(gross));
        let mut data = [
            Vec::with_capacity(BLOCK_SAMPLES + 8),
            Vec::with_capacity(BLOCK_SAMPLES + 8),
        ];
        conv.push(&raw, &self.cal, &mut data);
        for d in &mut data {
            d.truncate(BLOCK_SAMPLES);
        }
        Ok(Captured {
            data,
            clipped: conv.clipped,
            rate: cfg.rate.rate,
            range: self.range(cfg),
            lost: 0,
            at: None,
        })
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

    /// Roll mode and the logger: the next piece of a stream with no end
    /// (one transfer). Starts the stream when needed: queues the async
    /// transfers first, then starts the scope, so no data comes before a
    /// buffer is ready. On an error the stream ends; the next call starts
    /// it again.
    pub fn stream(&mut self, cfg: &HwConfig) -> Result<Captured> {
        if self.sent != Some(*cfg) {
            self.configure(cfg)?;
        }
        if self.stream.is_none() {
            self.t.stream_start(Self::stream_setup(cfg))?;
            if let Err(e) = self.start() {
                self.t.stream_stop();
                return Err(e);
            }
            // Throw away the first samples, like a normal block does.
            self.stream = Some(Converter::new(cfg, 2048));
        }
        let wait = Duration::from_secs_f64(Self::transfer_secs(cfg) * 2.0 + 1.0);
        let chunk = match self.t.stream_next(wait) {
            Ok(Some(c)) => c,
            Ok(None) => {
                self.end_stream();
                return Err(anyhow::Error::new(rusb::Error::Timeout).context(format!(
                    "no samples from the scope for {:.1} s",
                    wait.as_secs_f64()
                )));
            }
            Err(e) => {
                self.end_stream();
                return Err(e.context("reading samples failed"));
            }
        };
        let mut data = [Vec::new(), Vec::new()];
        let conv = self.stream.as_mut().expect("stream is set");
        conv.clipped = [false; 2];
        conv.push(&chunk.data, &self.cal, &mut data);
        Ok(Captured {
            data,
            clipped: conv.clipped,
            rate: cfg.rate.rate,
            range: self.range(cfg),
            lost: chunk.lost / cfg.channels.clamp(1, 2) as u64,
            at: Some(chunk.at),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::fake::{FakeScope, Wave};

    fn cfg(rate_idx: usize, channels: u8, gains: [usize; 2]) -> HwConfig {
        HwConfig {
            rate: RATES[rate_idx],
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

    #[test]
    fn configure_sends_only_changes() {
        let (mut s, fake) = scope([Wave::Dc(0.0); 2]);
        let c = cfg(12, 2, [5, 5]);
        s.configure(&c).unwrap();
        let first: Vec<u8> = fake.lock().calls.iter().map(|c| c.request).collect();
        // EEPROM read, then E6 E5 E4 E2 E1 E0.
        assert_eq!(first, vec![0xA2, 0xE6, 0xE5, 0xE4, 0xE2, 0xE1, 0xE0]);
        fake.lock().calls.clear();
        s.configure(&c).unwrap();
        assert!(fake.lock().calls.is_empty());
        // 1 V to 2 V/div: same gain byte, nothing to send.
        s.configure(&cfg(12, 2, [6, 5])).unwrap();
        assert!(fake.lock().calls.is_empty());
        s.configure(&cfg(12, 2, [2, 5])).unwrap();
        let calls = fake.lock().calls.clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            (calls[0].request, calls[0].data.clone()),
            (0xE0, vec![10, 2])
        );
    }

    #[test]
    fn capture_gives_volts() {
        let (mut s, fake) = scope([Wave::Dc(1.5), Wave::Dc(-0.25)]);
        fake.lock().noise = 0.0;
        let c = cfg(12, 2, [5, 3]);
        let block = s.capture(&c).unwrap();
        assert_eq!(block.data[0].len(), BLOCK_SAMPLES);
        assert_eq!(block.data[1].len(), BLOCK_SAMPLES);
        assert_eq!(block.rate, 1e6);
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
        // Every block starts sampling with E3 [01].
        let starts = fake
            .lock()
            .calls
            .iter()
            .filter(|c| c.request == 0xE3 && c.data == [1])
            .count();
        assert_eq!(starts, 1);
    }

    #[test]
    fn capture_single_channel_and_clipping() {
        let (mut s, fake) = scope([Wave::Dc(3.0), Wave::Dc(0.0)]);
        fake.lock().noise = 0.0;
        // 3 V at 100 mV/div (x10 gain) is far past the range.
        let block = s.capture(&cfg(15, 1, [2, 2])).unwrap();
        assert!(block.data[1].is_empty());
        assert_eq!(block.data[0].len(), BLOCK_SAMPLES);
        assert!(block.clipped[0]);
        assert_eq!(fake.lock().channels, 1);
    }

    #[test]
    fn slow_block_is_one_transfer() {
        let (mut s, fake) = scope([Wave::Dc(0.0); 2]);
        // 10 kS/s with 100x oversampling: about 4 MB in one read.
        let c = cfg(6, 2, [5, 5]);
        let block = s.capture(&c).unwrap();
        assert_eq!(block.data[0].len(), BLOCK_SAMPLES);
        assert_eq!(fake.lock().bulk_bytes, c.block_bytes());
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
        let mut c = cfg(6, 2, [5, 5]);
        c.roll = true;
        let mut total = 0;
        for _ in 0..5 {
            let piece = s.stream(&c).unwrap();
            assert_eq!(piece.data[0].len(), piece.data[1].len());
            total += piece.data[0].len();
        }
        assert!(total > 400, "total {total}");
        // The transfers were queued before the start command.
        assert!(fake.lock().started_with_stream);
        // The ADC runs at 1 MS/s x 2: 20 ms is 79 packets, 250 ms 13 transfers.
        let st = Scope6022::<FakeScope>::stream_setup(&c);
        assert_eq!((st.transfer, st.transfers), (79 * 512, 13));
        // One start for the whole stream.
        let starts = fake
            .lock()
            .calls
            .iter()
            .filter(|c| c.request == 0xE3)
            .count();
        assert_eq!(starts, 1);
    }

    #[test]
    fn stream_sizes_stay_in_usbfs_memory() {
        for (rate_id, ch) in [(16, 2), (30, 1), (10, 2), (102, 2)] {
            let mut c = cfg(12, ch, [5, 5]);
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
    fn stream_stops_before_a_block() {
        let (mut s, fake) = scope([Wave::Dc(0.0); 2]);
        let mut c = cfg(6, 2, [5, 5]);
        c.roll = true;
        s.stream(&c).unwrap();
        assert!(fake.lock().stream.is_some());
        c.roll = false;
        s.capture(&c).unwrap();
        assert!(fake.lock().stream.is_none());
        c.roll = true;
        s.stream(&c).unwrap();
        s.stop().unwrap();
        assert!(fake.lock().stream.is_none());
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
        s.capture(&cfg(12, 2, [5, 5])).unwrap();
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
