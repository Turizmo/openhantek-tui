//! A fake DSO-6022BE. It acts like the firmware on the USB side: it takes
//! the same vendor requests and sends 8-bit samples. Demo mode and the
//! tests use it, so they run the real driver code.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;

use super::protocol::{
    cal_freq_of_code, raw_rate_of_id, REQ_EEPROM, REQ_RW_INTERNAL, REQ_SET_CAL_FREQ,
    REQ_SET_CHANNELS, REQ_SET_COUPLING, REQ_SET_GAIN_CH1, REQ_SET_GAIN_CH2, REQ_SET_SAMPLERATE,
    REQ_START,
};
use super::transport::{ControlCall, Transport, VENDOR_IN, VENDOR_OUT};

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
    pub calls: Vec<ControlCall>,
    pub bulk_bytes: usize,
    /// Signal time, in seconds. Runs on with every sample.
    t: f64,
    since_start: usize,
    rng: u64,
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
                calls: Vec::new(),
                bulk_bytes: 0,
                t: 0.0,
                since_start: 0,
                rng: 0x9E37_79B9_7F4A_7C15,
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

    fn sample(&mut self, ch: usize) -> u8 {
        if self.since_start < self.junk {
            return if self.since_start.is_multiple_of(2) {
                0x00
            } else {
                0xFF
            };
        }
        let cal_freq = cal_freq_of_code(self.cal_code);
        let v = self.waves[ch].at(self.t, cal_freq);
        let n = self.random() * self.noise;
        let raw = self.zero[ch] as f64 + v * counts_per_volt(self.gain_byte[ch]) + n;
        raw.round().clamp(0.0, 255.0) as u8
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
                s.since_start = 0;
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
            if !s.sampling {
                drop(s);
                std::thread::sleep(timeout.min(Duration::from_millis(50)));
                return Err(rusb::Error::Timeout.into());
            }
            let raw_rate = raw_rate_of_id(s.rate_id).max(1.0);
            let channels = s.channels.clamp(1, 2) as usize;
            let dt = 1.0 / raw_rate;
            let n = buf.len() - buf.len() % channels;
            for frame in buf[..n].chunks_mut(channels) {
                for (c, b) in frame.iter_mut().enumerate() {
                    *b = s.sample(c);
                }
                s.t += dt;
                s.since_start += 1;
            }
            s.bulk_bytes += n;
            // Real time: the data cannot come faster than the ADC makes it.
            let pause = if s.realtime {
                let clock = *s.clock.get_or_insert_with(Instant::now);
                let due = Duration::from_secs_f64(s.since_start as f64 * dt);
                due.checked_sub(clock.elapsed()).map(|d| d.min(timeout))
            } else {
                None
            };
            (pause, n)
        };
        if let Some(p) = pause {
            std::thread::sleep(p);
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
