//! A fake DSO-6022BE. It acts like the firmware on the USB side: it takes
//! the same vendor requests and sends 8-bit samples. Demo mode and the
//! tests use it, so they run the real driver code.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Result;

use super::protocol::{
    cal_freq_of_code, raw_rate_of_id, PACKET_SIZE, REQ_EEPROM, REQ_RW_INTERNAL, REQ_SET_CAL_FREQ,
    REQ_SET_CHANNELS, REQ_SET_COUPLING, REQ_SET_GAIN_CH1, REQ_SET_GAIN_CH2, REQ_SET_SAMPLERATE,
    REQ_START,
};
use super::transport::{ControlCall, StreamSetup, Transport, VENDOR_IN, VENDOR_OUT};
use crate::stream::ring::{Failure, Writer};

/// Bytes per second the fake's USB link moves at most, like a real USB 2
/// bulk pipe (about 40 MB/s). Faster streams lose whole packets on the
/// scope side, without a report.
pub const USB_MAX_BYTES: f64 = 40e6;

/// Most samples in one wave table of the fake.
const TABLE_MAX: usize = 1 << 22;
/// Noise values in the fake's table (a prime, so it does not line up
/// with wave periods).
const NOISE_LEN: usize = 65_521;

/// A test signal at the BNC input, in volts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Wave {
    Sine {
        freq: f64,
        amp: f64,
        dc: f64,
    },
    Square {
        freq: f64,
        low: f64,
        high: f64,
        duty: f64,
    },
    Triangle {
        freq: f64,
        amp: f64,
        dc: f64,
    },
    /// The calibration output pin: 0 V / 2 V square at the set frequency.
    CalOut,
    Dc(f64),
}

impl Wave {
    pub fn at(self, t: f64, cal_freq: f64) -> f64 {
        let phase = |f: f64| (t * f).rem_euclid(1.0);
        match self {
            Wave::Sine { freq, amp, dc } => dc + amp * (std::f64::consts::TAU * freq * t).sin(),
            Wave::Square {
                freq,
                low,
                high,
                duty,
            } => {
                if phase(freq) < duty {
                    high
                } else {
                    low
                }
            }
            Wave::Triangle { freq, amp, dc } => {
                let p = phase(freq);
                dc + amp
                    * if p < 0.5 {
                        4.0 * p - 1.0
                    } else {
                        3.0 - 4.0 * p
                    }
            }
            Wave::CalOut => {
                if phase(cal_freq) < 0.5 {
                    2.0
                } else {
                    0.0
                }
            }
            Wave::Dc(v) => v,
        }
    }
}

/// State of the fake scope. Shared, so a test can look at it after the
/// fake is moved into the driver.
#[derive(Debug)]
pub struct FakeState {
    pub waves: [Wave; 2],
    /// Noise amplitude, in ADC counts (peak).
    pub noise: f64,
    /// ADC count for 0 V, per channel (also written into the fake EEPROM).
    pub zero: [u8; 2],
    /// Counts added to the zero with the ADC at 30 MS/s or more (not in
    /// the EEPROM), like a scope whose high-speed offsets are off.
    pub fast_zero_shift: [f64; 2],
    pub eeprom: [u8; 256],
    pub gain_byte: [u8; 2],
    pub rate_id: u8,
    pub channels: u8,
    pub coupling: u8,
    pub cal_code: u8,
    pub sampling: bool,
    /// Bad samples after each start, per channel (the real ADC needs time).
    pub junk: usize,
    /// Act like an unplugged scope.
    pub unplugged: bool,
    /// Sleep in bulk reads like the real scope takes time.
    pub realtime: bool,
    /// Lose this many samples (per channel) at the next bulk read or
    /// stream transfer, like a scope FIFO that overflowed: the time
    /// passes, the data does not come.
    pub drop_samples: usize,
    /// End the running stream with this error (like a USB transfer that
    /// fails).
    pub fail_stream: Option<rusb::Error>,
    pub calls: Vec<ControlCall>,
    /// Bytes sent so far, by bulk reads and by the stream.
    pub bulk_bytes: usize,
    /// The queued async stream, if any (see [`Transport::stream_start`]).
    pub stream: Option<StreamSetup>,
    /// Bytes per second the link moves at most (real time only).
    pub usb_max: f64,
    /// The last start command came while a stream was queued.
    pub started_with_stream: bool,
    /// Bytes the stream made since the start command.
    streamed: u64,
    /// The thread that makes the stream's transfers.
    generator: Option<Generator>,
    /// Signal time, in seconds. Runs on with every sample.
    t: f64,
    since_start: usize,
    rng: u64,
    /// One period of each wave in clean ADC values (see
    /// [`FakeState::tables`]); empty: compute each sample.
    table: [Vec<f32>; 2],
    /// Noise values, used in turn.
    noise_table: Vec<f32>,
    noise_at: usize,
    /// The settings the tables are for.
    #[allow(clippy::type_complexity)]
    table_key: Option<(u8, [u8; 2], u8, [u8; 2], String, u64)>,
    clock: Option<Instant>,
}

#[derive(Clone, Debug)]
pub struct FakeScope {
    pub state: Arc<Mutex<FakeState>>,
}

/// ADC counts per volt for a gain byte (same values the driver uses).
fn counts_per_volt(gain_byte: u8) -> f64 {
    match gain_byte {
        10 => 250.0,
        5 => 126.25,
        2 => 49.5,
        _ => 24.75,
    }
}

impl FakeScope {
    pub fn new(waves: [Wave; 2]) -> Self {
        let zero = [126u8, 131u8];
        let mut eeprom = [0xFFu8; 256];
        // Factory style: only low and high speed offsets are set.
        for g in 0..8 {
            for c in 0..2 {
                eeprom[8 + g * 2 + c] = zero[c];
                eeprom[8 + 16 + g * 2 + c] = zero[c];
            }
        }
        Self {
            state: Arc::new(Mutex::new(FakeState {
                waves,
                noise: 0.6,
                zero,
                fast_zero_shift: [0.0; 2],
                eeprom,
                gain_byte: [1, 1],
                rate_id: 1,
                channels: 2,
                coupling: 0x11,
                cal_code: 1,
                sampling: false,
                junk: 1500,
                unplugged: false,
                realtime: false,
                drop_samples: 0,
                fail_stream: None,
                calls: Vec::new(),
                bulk_bytes: 0,
                stream: None,
                usb_max: USB_MAX_BYTES,
                started_with_stream: false,
                streamed: 0,
                generator: None,
                t: 0.0,
                since_start: 0,
                rng: 0x9E37_79B9_7F4A_7C15,
                table: Default::default(),
                noise_table: Vec::new(),
                noise_at: 0,
                table_key: None,
                clock: None,
            })),
        }
    }

    /// The demo signals: a 1 kHz sine on CH1 and the calibration output on CH2.
    pub fn demo() -> Self {
        let fake = Self::new([
            Wave::Sine {
                freq: 1000.0,
                amp: 1.5,
                dc: 0.0,
            },
            Wave::CalOut,
        ]);
        fake.state.lock().unwrap().realtime = true;
        fake
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.state.lock().unwrap()
    }

    /// `frames` frames of the stream for `cfg`, from a start (with the
    /// junk at the start), made at once. Tests use it to feed the stream
    /// processing without a thread.
    pub fn raw(&self, cfg: &super::protocol::HwConfig, frames: usize) -> Vec<u8> {
        use super::protocol::{cal_freq_code, coupling_byte, GAIN_BYTE, GAIN_STEPS};
        let mut s = self.lock();
        s.rate_id = cfg.rate.id;
        s.channels = cfg.channels.clamp(1, 2);
        s.gain_byte = cfg.gains.map(|g| GAIN_BYTE[g.min(GAIN_STEPS - 1)]);
        s.cal_code = cal_freq_code(cfg.cal_freq);
        s.coupling = coupling_byte(cfg.dc);
        s.sampling = true;
        s.since_start = 0;
        s.clock = None;
        let mut buf = vec![0u8; frames * s.channels()];
        let n = s.fill(&mut buf);
        buf.truncate(n);
        buf
    }
}

impl FakeState {
    fn random(&mut self) -> f64 {
        // Two uniform values make a softer, more natural noise.
        let mut next = || {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            (self.rng >> 11) as f64 / (1u64 << 53) as f64 - 0.5
        };
        next() + next()
    }

    /// The clean ADC value (no noise) of channel `ch` at signal time `t`.
    fn clean(&self, ch: usize, t: f64) -> f32 {
        let cal_freq = cal_freq_of_code(self.cal_code);
        let v = self.waves[ch].at(t, cal_freq);
        let shift = if self.raw_rate() >= 30e6 {
            self.fast_zero_shift[ch]
        } else {
            0.0
        };
        (self.zero[ch] as f64 + shift + v * counts_per_volt(self.gain_byte[ch])) as f32
    }

    /// Make the sample tables if the settings changed: one period of each
    /// wave in clean ADC values (when the period is a whole number of
    /// samples, up to [`TABLE_MAX`]), and a table of noise. A table
    /// lookup per sample keeps the fake fast at 30 MS/s, also in a
    /// debug build.
    fn tables(&mut self) {
        let key = (
            self.rate_id,
            self.gain_byte,
            self.cal_code,
            self.zero,
            format!("{:?} {:?}", self.waves, self.fast_zero_shift),
            self.noise.to_bits(),
        );
        if self.table_key.as_ref() == Some(&key) {
            return;
        }
        let raw = self.raw_rate();
        let cal_freq = cal_freq_of_code(self.cal_code);
        for ch in 0..2 {
            let freq = match self.waves[ch] {
                Wave::Dc(_) => None,
                Wave::CalOut => Some(cal_freq),
                Wave::Sine { freq, .. } | Wave::Square { freq, .. } | Wave::Triangle { freq, .. } => {
                    Some(freq)
                }
            };
            let period = match freq {
                None => Some(1),
                Some(f) => {
                    let n = raw / f;
                    (f > 0.0 && (n - n.round()).abs() < 1e-6 && n.round() as usize <= TABLE_MAX)
                        .then(|| n.round() as usize)
                }
            };
            self.table[ch] = match period {
                Some(n) => (0..n.max(1)).map(|i| self.clean(ch, i as f64 / raw)).collect(),
                None => Vec::new(),
            };
        }
        let mut noise = Vec::with_capacity(NOISE_LEN);
        for _ in 0..NOISE_LEN {
            noise.push((self.random() * self.noise) as f32);
        }
        self.noise_table = noise;
        self.table_key = Some(key);
    }

    /// Sample of channel `ch` at the signal time now.
    #[inline]
    fn sample(&mut self, ch: usize, raw: f64) -> u8 {
        if self.since_start < self.junk {
            return if self.since_start.is_multiple_of(2) {
                0x00
            } else {
                0xFF
            };
        }
        let tab = &self.table[ch];
        let clean = if tab.is_empty() {
            self.clean(ch, self.t)
        } else {
            let n = tab.len();
            // The index of the signal time in the period.
            let i = ((self.t * raw).round() as u64 % n as u64) as usize;
            tab[i]
        };
        self.noise_at = (self.noise_at + 1) % NOISE_LEN;
        (clean + self.noise_table[self.noise_at])
            .round()
            .clamp(0.0, 255.0) as u8
    }

    fn raw_rate(&self) -> f64 {
        raw_rate_of_id(self.rate_id).max(1.0)
    }

    fn channels(&self) -> usize {
        self.channels.clamp(1, 2) as usize
    }

    /// Let `frames` samples per channel pass without sending them.
    fn skip(&mut self, frames: usize) {
        self.t += frames as f64 / self.raw_rate();
        self.since_start += frames;
    }

    /// Fill `buf` with samples (whole frames). Returns the bytes used.
    fn fill(&mut self, buf: &mut [u8]) -> usize {
        let channels = self.channels();
        let raw = self.raw_rate();
        let dt = 1.0 / raw;
        let n = buf.len() - buf.len() % channels;
        let frames = n / channels;
        let lost = std::mem::take(&mut self.drop_samples);
        self.skip(lost);
        self.tables();
        // Whole samples from here: the table index of a sample is its
        // number, so count samples from the time now.
        let k0 = (self.t * raw).round();
        let junk = self.junk.saturating_sub(self.since_start).min(frames);
        for (j, frame) in buf[..junk * channels].chunks_exact_mut(channels).enumerate() {
            let b = if (self.since_start + j).is_multiple_of(2) {
                0x00
            } else {
                0xFF
            };
            frame.fill(b);
        }
        let fast = self.table[..channels].iter().all(|t| !t.is_empty());
        if fast {
            // The hot loop: a table lookup and a noise value per sample.
            let k = k0 as u64 + junk as u64;
            let mut idx = [0usize; 2];
            for (c, i) in idx.iter_mut().enumerate().take(channels) {
                *i = (k % self.table[c].len() as u64) as usize;
            }
            let mut ni = self.noise_at;
            let noise = &self.noise_table;
            let (t0, t1) = (&self.table[0], &self.table[1]);
            let (n0, n1) = (t0.len(), t1.len().max(1));
            for frame in buf[junk * channels..n].chunks_exact_mut(channels) {
                ni += 1;
                if ni == NOISE_LEN {
                    ni = 0;
                }
                frame[0] = (t0[idx[0]] + noise[ni] + 0.5).clamp(0.0, 255.0) as u8;
                idx[0] += 1;
                if idx[0] == n0 {
                    idx[0] = 0;
                }
                if channels == 2 {
                    ni += 1;
                    if ni == NOISE_LEN {
                        ni = 0;
                    }
                    frame[1] = (t1[idx[1]] + noise[ni] + 0.5).clamp(0.0, 255.0) as u8;
                    idx[1] += 1;
                    if idx[1] == n1 {
                        idx[1] = 0;
                    }
                }
            }
            self.noise_at = ni;
            self.since_start += frames;
        } else {
            self.since_start += junk;
            for (j, frame) in buf[junk * channels..n].chunks_mut(channels).enumerate() {
                self.t = (k0 + (junk + j) as f64) * dt;
                for (c, b) in frame.iter_mut().enumerate() {
                    *b = self.sample(c, raw);
                }
                self.since_start += 1;
            }
        }
        self.t = (k0 + frames as f64) * dt;
        self.bulk_bytes += n;
        n
    }

    /// In real time: how long until the samples made so far are due.
    fn due(&mut self, limit: Duration) -> Option<Duration> {
        if !self.realtime {
            return None;
        }
        let clock = *self.clock.get_or_insert_with(Instant::now);
        let due = Duration::from_secs_f64(self.since_start as f64 / self.raw_rate());
        due.checked_sub(clock.elapsed()).map(|d| d.min(limit))
    }
}

impl Transport for FakeScope {
    fn control_out(&mut self, request: u8, value: u16, index: u16, data: &[u8]) -> Result<()> {
        let mut s = self.lock();
        if s.unplugged {
            return Err(rusb::Error::NoDevice.into());
        }
        s.calls.push(ControlCall {
            request_type: VENDOR_OUT,
            request,
            value,
            index,
            data: data.to_vec(),
        });
        // Like the firmware: every vendor command stops sampling first.
        s.sampling = false;
        let b = data.first().copied().unwrap_or(0);
        match request {
            REQ_SET_GAIN_CH1 => s.gain_byte[0] = b,
            REQ_SET_GAIN_CH2 => s.gain_byte[1] = b,
            REQ_SET_SAMPLERATE => s.rate_id = b,
            REQ_SET_CHANNELS => s.channels = b.clamp(1, 2),
            REQ_SET_COUPLING => s.coupling = b,
            REQ_SET_CAL_FREQ => s.cal_code = b,
            REQ_START => {
                s.sampling = b == 1;
                if s.sampling {
                    s.started_with_stream = s.stream.is_some();
                }
                s.since_start = 0;
                s.streamed = 0;
                s.clock = None;
            }
            REQ_EEPROM => anyhow::bail!("fake scope: EEPROM writes are not allowed"),
            REQ_RW_INTERNAL => {}
            _ => anyhow::bail!("fake scope: unknown request {request:#04x}"),
        }
        Ok(())
    }

    fn control_in(&mut self, request: u8, value: u16, index: u16, buf: &mut [u8]) -> Result<usize> {
        let mut s = self.lock();
        if s.unplugged {
            return Err(rusb::Error::NoDevice.into());
        }
        s.calls.push(ControlCall {
            request_type: VENDOR_IN,
            request,
            value,
            index,
            data: vec![],
        });
        if request != REQ_EEPROM {
            anyhow::bail!("fake scope: unknown request {request:#04x}");
        }
        let start = value as usize;
        let n = buf.len().min(s.eeprom.len().saturating_sub(start));
        buf[..n].copy_from_slice(&s.eeprom[start..start + n]);
        Ok(n)
    }

    fn bulk_in(&mut self, _endpoint: u8, buf: &mut [u8], timeout: Duration) -> Result<usize> {
        let (pause, n) = {
            let mut s = self.lock();
            if s.unplugged {
                return Err(rusb::Error::NoDevice.into());
            }
            if s.stream.is_some() {
                anyhow::bail!("bulk read while a stream is running");
            }
            if !s.sampling {
                drop(s);
                std::thread::sleep(timeout.min(Duration::from_millis(50)));
                return Err(rusb::Error::Timeout.into());
            }
            let n = s.fill(buf);
            // Real time: the data cannot come faster than the ADC makes it.
            (s.due(timeout), n)
        };
        if let Some(p) = pause {
            std::thread::sleep(p);
        }
        Ok(n)
    }

    /// Like the async stream on real USB: a thread makes the transfers
    /// from the start command on and writes them into the ring. In real
    /// time it makes them at the ADC rate, and a full ring drops them
    /// (counted by the ring), like the real host. Else it makes them as
    /// fast as the ring has room (the tests: no data is lost, whatever
    /// the speed). A stream faster than [`FakeState::usb_max`] loses
    /// packets on the scope side, without a report, like the real one.
    fn stream_start(&mut self, setup: StreamSetup, ring: Writer) -> Result<()> {
        self.stream_stop();
        let mut s = self.lock();
        if s.unplugged {
            return Err(rusb::Error::NoDevice.into());
        }
        s.stream = Some(setup);
        let stop = Arc::new(AtomicBool::new(false));
        let state = self.state.clone();
        let flag = stop.clone();
        let handle = std::thread::Builder::new()
            .name("fake scope".into())
            .spawn(move || generate(state, setup, ring, flag))?;
        s.generator = Some(Generator { stop, handle });
        Ok(())
    }

    fn stream_stop(&mut self) {
        let g = {
            let mut s = self.lock();
            s.stream = None;
            s.generator.take()
        };
        if let Some(g) = g {
            g.stop.store(true, Ordering::Release);
            let _ = g.handle.join();
        }
    }
}

/// The thread of a running fake stream.
#[derive(Debug)]
struct Generator {
    stop: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

/// Make the transfers of a fake stream (see [`FakeScope::stream_start`]).
fn generate(state: Arc<Mutex<FakeState>>, setup: StreamSetup, ring: Writer, stop: Arc<AtomicBool>) {
    let pause = |d: Duration| std::thread::sleep(d);
    // Ends when stopped, when a newer stream uses the ring, or when
    // nobody but this thread has the fake any more.
    while !stop.load(Ordering::Acquire) && ring.current() && Arc::strong_count(&state) > 1 {
        let (data, at) = {
            let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
            if s.unplugged {
                drop(s);
                ring.fail(Failure::Usb(rusb::Error::NoDevice));
                return;
            }
            if let Some(e) = s.fail_stream.take() {
                drop(s);
                ring.fail(Failure::Usb(e));
                return;
            }
            if !s.sampling {
                drop(s);
                pause(Duration::from_millis(1));
                continue;
            }
            let channels = s.channels();
            let xfer = setup.transfer - setup.transfer % channels;
            let byte_rate = s.raw_rate() * channels as f64;
            // What the link carries: the scope's rate, at most `usb_max`.
            let link = byte_rate.min(s.usb_max);
            let clock = *s.clock.get_or_insert_with(Instant::now);
            if s.realtime {
                // The transfer is done when its last byte is made.
                let due = Duration::from_secs_f64((s.streamed + xfer as u64) as f64 / link);
                let now = clock.elapsed();
                if due > now {
                    drop(s);
                    pause((due - now).min(Duration::from_millis(5)));
                    continue;
                }
            } else if ring.ring().room() < xfer as u64 {
                drop(s);
                pause(Duration::from_micros(200));
                continue;
            }
            // Samples lost in the scope: their time passes, they never come.
            let dropped = std::mem::take(&mut s.drop_samples);
            s.skip(dropped);
            s.streamed += (dropped * channels) as u64;
            if s.realtime && byte_rate > s.usb_max {
                // Packets the link had no time for.
                let gone = xfer as f64 * (byte_rate / s.usb_max - 1.0);
                let packets = (gone / PACKET_SIZE as f64).round() as usize;
                s.skip(packets * PACKET_SIZE / channels);
            }
            let mut data = vec![0u8; xfer];
            let n = s.fill(&mut data);
            data.truncate(n);
            s.streamed += n as u64;
            // When the transfer is done on the bus: when its last byte is
            // made, not when the fake made it (that can be faster or
            // slower than real time).
            let at = clock + Duration::from_secs_f64(s.streamed as f64 / link);
            (data, at)
        };
        ring.write(&data, at, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::ring::Ring;

    #[test]
    fn waves() {
        let sq = Wave::Square {
            freq: 1.0,
            low: 0.0,
            high: 1.0,
            duty: 0.25,
        };
        assert_eq!(sq.at(0.1, 0.0), 1.0);
        assert_eq!(sq.at(0.3, 0.0), 0.0);
        let tri = Wave::Triangle {
            freq: 1.0,
            amp: 1.0,
            dc: 0.0,
        };
        assert!((tri.at(0.0, 0.0) + 1.0).abs() < 1e-12);
        assert!((tri.at(0.5, 0.0) - 1.0).abs() < 1e-12);
        assert_eq!(Wave::CalOut.at(0.0001, 1000.0), 2.0);
        assert_eq!(Wave::CalOut.at(0.0006, 1000.0), 0.0);
    }

    #[test]
    fn needs_start_before_data() {
        let mut f = FakeScope::new([Wave::Dc(0.0); 2]);
        let mut buf = [0u8; 512];
        assert!(f.bulk_in(0x86, &mut buf, Duration::from_millis(1)).is_err());
        f.control_out(REQ_START, 0, 0, &[1]).unwrap();
        assert_eq!(
            f.bulk_in(0x86, &mut buf, Duration::from_millis(1)).unwrap(),
            512
        );
        // Any command stops sampling, like the firmware.
        f.control_out(REQ_SET_GAIN_CH1, 0, 0, &[10, 0]).unwrap();
        assert!(f.bulk_in(0x86, &mut buf, Duration::from_millis(1)).is_err());
    }

    #[test]
    fn samples_follow_gain_and_offset() {
        let mut f = FakeScope::new([Wave::Dc(0.2), Wave::Dc(-0.1)]);
        {
            let mut s = f.lock();
            s.noise = 0.0;
            s.junk = 0;
        }
        f.control_out(REQ_SET_GAIN_CH1, 0, 0, &[10, 0]).unwrap();
        f.control_out(REQ_START, 0, 0, &[1]).unwrap();
        let mut buf = [0u8; 8];
        f.bulk_in(0x86, &mut buf, Duration::from_millis(1)).unwrap();
        // CH1: 126 + 0.2 * 250 = 176. CH2: 131 - 0.1 * 24.75 = 128.5 -> 129 (round half up).
        assert_eq!(&buf[..2], &[176, 129]);
    }

    #[test]
    fn junk_after_start() {
        let mut f = FakeScope::new([Wave::Dc(0.0); 2]);
        f.control_out(REQ_SET_CHANNELS, 0, 0, &[1]).unwrap();
        f.control_out(REQ_START, 0, 0, &[1]).unwrap();
        let mut buf = vec![0u8; 2000];
        f.bulk_in(0x86, &mut buf, Duration::from_millis(1)).unwrap();
        assert_eq!(buf[0], 0x00);
        assert_eq!(buf[1], 0xFF);
        assert!(buf[1600..].iter().all(|b| (120..=132).contains(b)));
    }

    fn setup(transfer: usize, transfers: usize) -> StreamSetup {
        StreamSetup {
            endpoint: 0x86,
            transfer,
            transfers,
        }
    }

    fn ring(size: usize, ch: u8) -> (Arc<Ring>, Writer) {
        let r = Arc::new(Ring::new(size));
        let w = r.reset(ch);
        (r, w)
    }

    /// Wait until the ring holds `bytes`.
    fn wait_for(r: &Ring, bytes: u64) {
        let t0 = Instant::now();
        while r.written() < bytes {
            assert!(t0.elapsed() < Duration::from_secs(10), "no data");
            r.wait(r.frames(), Duration::from_millis(50));
        }
    }

    #[test]
    fn stream_gives_transfers_with_bus_times() {
        let mut f = FakeScope::new([Wave::Dc(0.0); 2]);
        f.control_out(REQ_SET_SAMPLERATE, 0, 0, &[1, 0]).unwrap();
        let (r, w) = ring(1 << 20, 2);
        f.stream_start(setup(4096, 4), w).unwrap();
        // Queued, but no data before the start command.
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(r.written(), 0);
        f.control_out(REQ_START, 0, 0, &[1]).unwrap();
        assert!(f.lock().started_with_stream);
        wait_for(&r, 2 * 4096);
        let m = r.take_marks();
        assert_eq!((m[0].end, m[1].end), (4096, 8192));
        // 2048 frames at 1 MS/s: 2.048 ms apart on the bus.
        let dt = m[1].at.duration_since(m[0].at).as_secs_f64();
        assert!((dt - 2.048e-3).abs() < 1e-6, "{dt}");
        // A bulk read while the stream is queued is a bug.
        let mut buf = [0u8; 512];
        assert!(f.bulk_in(0x86, &mut buf, Duration::from_millis(1)).is_err());
        f.stream_stop();
        assert!(f.bulk_in(0x86, &mut buf, Duration::from_millis(1)).is_ok());
    }

    #[test]
    fn slow_reader_loses_whole_transfers_and_is_told() {
        let mut f = FakeScope::new([Wave::Dc(0.0); 2]);
        f.lock().realtime = true;
        // 1 MS/s, CH1 only: 1 MB/s into a ring of 8 kB nobody reads.
        f.control_out(REQ_SET_CHANNELS, 0, 0, &[1]).unwrap();
        f.control_out(REQ_SET_SAMPLERATE, 0, 0, &[1, 0]).unwrap();
        let (r, w) = ring(8192, 1);
        f.stream_start(setup(2048, 2), w).unwrap();
        f.control_out(REQ_START, 0, 0, &[1]).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        f.stream_stop();
        // About 100 kB came; 8 kB of it had room.
        assert_eq!(r.written(), 8192);
        let lost = r.overrun();
        assert!(lost > 80_000 && lost < 100_000, "{lost}");
        assert_eq!(lost % 2048, 0);
    }

    #[test]
    fn backpressure_waits_for_room() {
        let mut f = FakeScope::new([Wave::Dc(0.0); 2]);
        f.control_out(REQ_SET_SAMPLERATE, 0, 0, &[16, 0]).unwrap();
        let (r, w) = ring(1 << 16, 2);
        f.stream_start(setup(4096, 4), w).unwrap();
        f.control_out(REQ_START, 0, 0, &[1]).unwrap();
        wait_for(&r, 1 << 16);
        std::thread::sleep(Duration::from_millis(20));
        // Full, and nothing dropped: the fake waits.
        assert_eq!((r.written(), r.overrun()), (1 << 16, 0));
        r.keep_from(1 << 14);
        wait_for(&r, (1 << 16) + (1 << 15));
        assert_eq!(r.overrun(), 0);
        f.stream_stop();
    }

    #[test]
    fn stream_errors_and_unplug_end_the_stream() {
        let mut f = FakeScope::new([Wave::Dc(0.0); 2]);
        let (r, w) = ring(1 << 20, 2);
        f.stream_start(setup(4096, 4), w).unwrap();
        f.control_out(REQ_START, 0, 0, &[1]).unwrap();
        wait_for(&r, 4096);
        f.lock().fail_stream = Some(rusb::Error::Overflow);
        assert!(r.wait(u64::MAX - 1, Duration::from_secs(5)));
        assert_eq!(r.failure(), Some(Failure::Usb(rusb::Error::Overflow)));
        let w = r.reset(2);
        f.stream_start(setup(4096, 4), w).unwrap();
        f.control_out(REQ_START, 0, 0, &[1]).unwrap();
        f.lock().unplugged = true;
        assert!(r.wait(u64::MAX - 1, Duration::from_secs(5)));
        assert_eq!(r.failure(), Some(Failure::Usb(rusb::Error::NoDevice)));
        f.stream_stop();
    }

    #[test]
    fn too_fast_for_usb_loses_packets_silently() {
        let mut f = FakeScope::new([Wave::Dc(0.0); 2]);
        f.lock().realtime = true;
        // 24 MS/s with both channels: 48 MB/s, more than the link's 40.
        f.control_out(REQ_SET_SAMPLERATE, 0, 0, &[24, 0]).unwrap();
        let (r, w) = ring(1 << 24, 2);
        f.stream_start(setup(256 << 10, 32), w).unwrap();
        f.control_out(REQ_START, 0, 0, &[1]).unwrap();
        wait_for(&r, 4 * (256 << 10));
        f.stream_stop();
        assert_eq!(r.overrun(), 0);
        let bytes = r.written();
        // The signal time ran on by 48/40 of the data sent.
        let frames = f.lock().since_start;
        assert!(
            frames as f64 > bytes as f64 / 2.0 * 1.15,
            "{frames} {bytes}"
        );
    }

    #[test]
    fn eeprom_read_and_no_writes() {
        let mut f = FakeScope::new([Wave::Dc(0.0); 2]);
        let mut buf = [0u8; 80];
        assert_eq!(f.control_in(REQ_EEPROM, 8, 0, &mut buf).unwrap(), 80);
        assert_eq!(buf[0], 126);
        assert_eq!(buf[1], 131);
        assert_eq!(buf[32], 0xFF);
        assert!(f.control_out(REQ_EEPROM, 8, 0, &[0; 16]).is_err());
    }

    #[test]
    fn unplugged_gives_no_device() {
        let mut f = FakeScope::new([Wave::Dc(0.0); 2]);
        f.lock().unplugged = true;
        let err = f.control_out(REQ_START, 0, 0, &[1]).unwrap_err();
        assert_eq!(
            err.downcast_ref::<rusb::Error>(),
            Some(&rusb::Error::NoDevice)
        );
    }
}
