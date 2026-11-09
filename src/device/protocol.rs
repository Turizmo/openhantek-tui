//! DSO-6022BE protocol with the OpenHantek6022 firmware (version 0x0210).
//! Pure data and math: no USB calls here.

/// Cypress FX2LP with no firmware loaded.
pub const VID_LOADER: u16 = 0x04B4;
/// OpenHantek firmware running.
pub const VID_FIRMWARE: u16 = 0x04B5;
pub const PID_6022BE: u16 = 0x6022;
/// Voltcraft DSO-2020: same hardware, becomes a 6022BE with this firmware.
pub const PID_DSO2020: u16 = 0x2020;
/// Firmware version in bcdDevice. Newer is accepted too.
pub const FIRMWARE_VERSION: u16 = 0x0210;

pub const INTERFACE: u8 = 0;
pub const EP_BULK_IN: u8 = 0x86;
pub const PACKET_SIZE: usize = 512;

/// Vendor requests.
pub const REQ_RW_INTERNAL: u8 = 0xA0;
pub const REQ_EEPROM: u8 = 0xA2;
pub const REQ_SET_GAIN_CH1: u8 = 0xE0;
pub const REQ_SET_GAIN_CH2: u8 = 0xE1;
pub const REQ_SET_SAMPLERATE: u8 = 0xE2;
pub const REQ_START: u8 = 0xE3;
pub const REQ_SET_CHANNELS: u8 = 0xE4;
pub const REQ_SET_COUPLING: u8 = 0xE5;
pub const REQ_SET_CAL_FREQ: u8 = 0xE6;

/// FX2 CPU control register: write 1 to hold the CPU in reset, 0 to run.
pub const CPUCS: u16 = 0xE600;
/// Largest firmware chunk per 0xA0 write.
pub const FIRMWARE_CHUNK: usize = 1023;

/// EEPROM address and size of the calibration data.
pub const EEPROM_CAL_ADDR: u16 = 0x0008;
pub const EEPROM_CAL_LEN: usize = 80;

/// One sample rate. The ADC runs at `raw` samples per second; each output
/// sample is made from `oversample` raw samples (their mean, and their
/// min and max for the peak display).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rate {
    /// Output samples per second.
    pub rate: f64,
    /// Rate id byte for request 0xE2.
    pub id: u8,
    /// ADC samples per second.
    pub raw: f64,
    /// Raw samples per output sample (the decimation factor).
    pub oversample: usize,
}

/// ADC rate for a rate id, as the firmware decodes it.
pub fn raw_rate_of_id(id: u8) -> f64 {
    if id < 100 {
        id as f64 * 1e6
    } else {
        (id as f64 - 100.0) * 10e3
    }
}

/// The choices of the max ADC rate setting: the rate id with both
/// channels, and with CH1 alone. The first is the top clean rate of the
/// async stream (32 MB/s: 16 MS/s with two channels, 30 MS/s with CH1).
/// 24 MS/s with two channels and 48 MS/s lose packets, so they are not
/// here. Below 1 MS/s both channels are always sampled.
pub const ADC_CAPS: [(u8, u8); 8] = [
    (16, 30),
    (12, 24),
    (10, 10),
    (5, 5),
    (2, 2),
    (1, 1),
    (150, 150),
    (140, 140),
];
/// The default max ADC rate (with both channels): the top clean rate.
pub const MAX_ADC_DEFAULT: f64 = 16e6;

/// Index into [`ADC_CAPS`] of a max ADC rate setting (S/s with both
/// channels): the nearest choice.
pub fn cap_index(max_adc: f64) -> usize {
    let far = |i: usize| {
        let r = raw_rate_of_id(ADC_CAPS[i].0);
        if max_adc > 0.0 && max_adc.is_finite() {
            (r / max_adc).ln().abs()
        } else {
            (i != 0) as u8 as f64
        }
    };
    (0..ADC_CAPS.len())
        .min_by(|&a, &b| far(a).total_cmp(&far(b)))
        .unwrap_or(0)
}

/// Channels the scope samples: CH1 alone (1-channel mode, twice the
/// rate) when CH2 is not needed, else both. Below 1 MS/s the ADC always
/// samples both channels.
pub fn hw_channels(need_ch2: bool, max_adc: f64) -> u8 {
    let (_, one) = ADC_CAPS[cap_index(max_adc)];
    if need_ch2 || raw_rate_of_id(one) < 1e6 {
        2
    } else {
        1
    }
}

/// The ADC rate (id and S/s) for `channels` (1 or 2) under the max ADC
/// rate setting `max_adc`.
pub fn adc_rate(channels: u8, max_adc: f64) -> (u8, f64) {
    let (two, one) = ADC_CAPS[cap_index(max_adc)];
    let id = if channels == 1 { one } else { two };
    (id, raw_rate_of_id(id))
}

/// Text of a max ADC rate choice, like `16 MS/s · CH1 alone 30 MS/s`.
pub fn cap_label(i: usize) -> String {
    let (two, one) = ADC_CAPS[i.min(ADC_CAPS.len() - 1)];
    let f = |id: u8| crate::si::fmt_si(raw_rate_of_id(id), "S/s");
    if two == one {
        f(two)
    } else {
        format!("{} · CH1 alone {}", f(two), f(one))
    }
}

/// Output samples per block, per channel (as OpenHantek6022).
pub const BLOCK_SAMPLES: usize = 20_000;
/// At most this many samples on one screen, so the block has room for the
/// trigger search before and after the screen.
pub const MAX_SCREEN_SAMPLES: f64 = 10_000.0;

/// Output rates the scope view can use with the ADC at `raw`: every
/// 1-2-5 rate from 100 S/s that divides `raw` exactly, and `raw`,
/// `raw / 2`, `raw / 4`. Slowest first.
pub fn output_rates(raw: f64) -> Vec<f64> {
    let raw_i = raw.round() as u64;
    let mut v: Vec<f64> = (2..8)
        .flat_map(|e| [1u64, 2, 5].map(|m| m * 10u64.pow(e)))
        .filter(|&r| r <= raw_i && raw_i.is_multiple_of(r))
        .map(|r| r as f64)
        .collect();
    for d in [4u64, 2, 1] {
        if raw_i.is_multiple_of(d) {
            v.push((raw_i / d) as f64);
        }
    }
    v.sort_by(f64::total_cmp);
    v.dedup();
    v
}

/// Pick the sample rate for a screen of `screen_time` seconds: the ADC
/// at the top rate the max ADC rate setting allows for `channels`, and
/// the highest output rate that fits [`MAX_SCREEN_SAMPLES`] on the
/// screen. The output rate is made by decimating the ADC stream.
pub fn choose_rate(screen_time: f64, channels: u8, max_adc: f64) -> Rate {
    let (id, raw) = adc_rate(channels, max_adc);
    let rates = output_rates(raw);
    let rate = rates
        .iter()
        .rev()
        .find(|&&r| r * screen_time <= MAX_SCREEN_SAMPLES * 1.0001)
        .copied()
        .unwrap_or(rates[0]);
    Rate {
        rate,
        id,
        raw,
        oversample: ((raw / rate).round() as usize).max(1),
    }
}

/// Gain steps. Index g matches the volts/div steps 20 mV .. 5 V.
pub const GAIN_STEPS: usize = 8;
/// Volts per division for each gain index (at the BNC, probe x1).
pub const GAIN_VDIV: [f64; GAIN_STEPS] = [0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0];
/// Byte sent with 0xE0/0xE1: the hardware gain (x10, x5, x2, x1).
pub const GAIN_BYTE: [u8; GAIN_STEPS] = [10, 10, 10, 5, 2, 1, 1, 1];
/// ADC counts per volt at the BNC for each gain index.
pub const COUNTS_PER_VOLT: [f64; GAIN_STEPS] =
    [250.0, 250.0, 250.0, 126.25, 49.5, 24.75, 24.75, 24.75];

/// Gain index for a volts/div value at the BNC: the first step that is
/// at least that big, else the last.
pub fn gain_index(volts_per_div: f64) -> usize {
    GAIN_VDIV
        .iter()
        .position(|&v| v >= volts_per_div * 0.999)
        .unwrap_or(GAIN_STEPS - 1)
}

/// Byte for request 0xE6 for a calibration output frequency, in Hz.
pub fn cal_freq_code(hz: u32) -> u8 {
    let f = hz as f64;
    if hz < 1000 {
        let code = 100 + (f / 10.0).round() as u32;
        if code == 110 {
            0
        } else {
            code.min(255) as u8
        }
    } else if hz <= 5500 && !hz.is_multiple_of(1000) {
        (200 + (f / 100.0).round() as u32).min(255) as u8
    } else {
        (f / 1000.0).round().clamp(1.0, 100.0) as u8
    }
}

/// Calibration output frequency in Hz for a 0xE6 byte, as the firmware
/// decodes it.
pub fn cal_freq_of_code(code: u8) -> f64 {
    match code {
        0 => 100.0,
        1..=100 => code as f64 * 1000.0,
        103 => 32.0,
        101..=200 => (code as f64 - 100.0) * 10.0,
        _ => (code as f64 - 200.0) * 100.0,
    }
}

/// Byte for request 0xE5: bit 0 = CH1 DC, bit 4 = CH2 DC.
pub fn coupling_byte(dc: [bool; 2]) -> u8 {
    (dc[0] as u8) | ((dc[1] as u8) << 4)
}

/// Calibration from the EEPROM and from the OpenHantek6022 ini file.
#[derive(Clone, Debug, PartialEq)]
pub struct Calibration {
    /// ADC count that means 0 V, per gain index and channel. Two banks:
    /// [0] for rates below 30 MS/s, [1] for 30 MS/s and up.
    pub offset: [[[f64; 2]; GAIN_STEPS]; 2],
    /// Gain correction factor, per gain index and channel.
    pub gain: [[f64; 2]; GAIN_STEPS],
    /// Extra offset in ADC counts from the ini file.
    pub ini_offset: [[f64; 2]; GAIN_STEPS],
    /// Extra gain factor from the ini file.
    pub ini_gain: [[f64; 2]; GAIN_STEPS],
    /// Values of the app's own calibration (the config file). They win
    /// over the EEPROM and the ini file.
    pub app: AppCal,
}

/// The app's own calibration, from the config file: never from or to the
/// EEPROM.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AppCal {
    /// ADC count that means 0 V, per bank (like
    /// [`Calibration::offset`]), gain index and channel. 0 = not set:
    /// the EEPROM value (and the ini offset) is used.
    pub zero: [[[f64; 2]; GAIN_STEPS]; 2],
    /// Gain correction factor per channel (from the calibration output),
    /// on top of the other gain values. 1 = none.
    pub gain: [f64; 2],
}

impl Default for AppCal {
    fn default() -> Self {
        Self {
            zero: [[[0.0; 2]; GAIN_STEPS]; 2],
            gain: [1.0; 2],
        }
    }
}

impl Default for Calibration {
    fn default() -> Self {
        Self {
            offset: [[[128.0; 2]; GAIN_STEPS]; 2],
            gain: [[1.0; 2]; GAIN_STEPS],
            ini_offset: [[0.0; 2]; GAIN_STEPS],
            ini_gain: [[1.0; 2]; GAIN_STEPS],
            app: AppCal::default(),
        }
    }
}

fn valid(b: u8) -> bool {
    b != 0x00 && b != 0xFF
}

impl Calibration {
    /// Read the 80 calibration bytes from EEPROM address 8.
    /// Layout: offset low speed [16], offset high speed [16], gain [16],
    /// fine offset low speed [16], fine offset high speed [16]. Byte
    /// `g * 2 + c` is gain index g, channel c. 0x00 and 0xFF mean "not set".
    pub fn from_eeprom(bytes: &[u8]) -> Self {
        let mut cal = Self::default();
        if bytes.len() < EEPROM_CAL_LEN {
            return cal;
        }
        for g in 0..GAIN_STEPS {
            for c in 0..2 {
                let i = g * 2 + c;
                for bank in 0..2 {
                    let off = bytes[bank * 16 + i];
                    let fine = bytes[48 + bank * 16 + i];
                    if valid(off) {
                        // Factory EEPROMs have no fine byte: use the plain offset.
                        let frac = if valid(fine) {
                            (fine as f64 - 128.0) / 250.0
                        } else {
                            0.0
                        };
                        cal.offset[bank][g][c] = off as f64 + frac;
                    }
                }
                let gain = bytes[32 + i];
                if valid(gain) {
                    cal.gain[g][c] = 1.0 + (gain as f64 - 128.0) / 500.0;
                }
            }
        }
        cal
    }

    /// Apply an OpenHantek6022 calibration ini file (`[offset]` and `[gain]`
    /// sections with keys like `ch0\1000mV`). Unknown keys are ignored.
    /// Returns how many values were read.
    pub fn apply_ini(&mut self, text: &str) -> usize {
        let mut section = String::new();
        let mut count = 0;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') && line.ends_with(']') {
                section = line[1..line.len() - 1].to_string();
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let Ok(value) = value.trim().parse::<f64>() else {
                continue;
            };
            let Some((ch, mv)) = key.trim().split_once('\\') else {
                continue;
            };
            let c = match ch {
                "ch0" => 0,
                "ch1" => 1,
                _ => continue,
            };
            let Some(mv) = mv.strip_suffix("mV").and_then(|m| m.parse::<f64>().ok()) else {
                continue;
            };
            let Some(g) = GAIN_VDIV.iter().position(|v| (v * 1000.0 - mv).abs() < 0.5) else {
                continue;
            };
            match section.as_str() {
                "offset" => self.ini_offset[g][c] = value,
                "gain" if value.is_finite() && value > 0.0 => self.ini_gain[g][c] = value,
                _ => continue,
            }
            count += 1;
        }
        count
    }

    /// ADC count that means 0 V: the app's zero when it has one, else
    /// the EEPROM offset plus the ini offset.
    pub fn zero(&self, gain: usize, channel: usize, high_speed: bool) -> f64 {
        let bank = high_speed as usize;
        let app = self.app.zero[bank][gain][channel];
        if app > 0.0 {
            app
        } else {
            self.offset[bank][gain][channel] + self.ini_offset[gain][channel]
        }
    }

    /// The EEPROM (and ini) zero, without the app's value.
    pub fn device_zero(&self, gain: usize, channel: usize, high_speed: bool) -> f64 {
        self.offset[high_speed as usize][gain][channel] + self.ini_offset[gain][channel]
    }

    /// Volts at the BNC per ADC count, with every gain value.
    fn volts_per_count(&self, gain: usize, channel: usize) -> f64 {
        self.ini_gain[gain][channel] / COUNTS_PER_VOLT[gain]
            * self.gain[gain][channel]
            * self.app.gain[channel]
    }

    /// Volts at the BNC for an (averaged) ADC value.
    pub fn volts(&self, raw: f64, gain: usize, channel: usize, high_speed: bool) -> f64 {
        (raw - self.zero(gain, channel, high_speed)) * self.volts_per_count(gain, channel)
    }

    /// The calibration as a line: volts at the BNC = `a * raw + b`.
    pub fn linear(&self, gain: usize, channel: usize, high_speed: bool) -> (f64, f64) {
        let b = self.volts(0.0, gain, channel, high_speed);
        (self.volts(1.0, gain, channel, high_speed) - b, b)
    }

    /// Volts at the BNC of the lowest and the highest ADC value (0x00 and
    /// 0xFF): what the input can measure at this gain.
    pub fn range(&self, gain: usize, channel: usize, high_speed: bool) -> (f64, f64) {
        let a = self.volts(0.0, gain, channel, high_speed);
        let b = self.volts(255.0, gain, channel, high_speed);
        (a.min(b), a.max(b))
    }

    /// ADC value for a voltage at the BNC (the inverse of [`Self::volts`]).
    pub fn raw(&self, volts: f64, gain: usize, channel: usize, high_speed: bool) -> f64 {
        volts / self.volts_per_count(gain, channel) + self.zero(gain, channel, high_speed)
    }
}

/// What the hardware must do, made from the scope settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HwConfig {
    pub rate: Rate,
    /// 1 = CH1 only, 2 = both channels.
    pub channels: u8,
    /// Gain index per channel.
    pub gains: [usize; 2],
    /// DC coupling per channel (only has an effect on modded scopes).
    pub dc: [bool; 2],
    /// Calibration output frequency, in Hz.
    pub cal_freq: u32,
    /// Roll mode: append the stream to the screen, do not trigger (slow
    /// time bases). Else blocks for the trigger.
    pub roll: bool,
}

impl HwConfig {
    /// True when the high-speed offset bank applies: the ADC runs at
    /// 30 MS/s or more.
    pub fn high_speed(&self) -> bool {
        self.rate.raw >= 30e6
    }
}

/// The input range of each channel for `cfg` with the nominal
/// calibration (no EEPROM values).
pub fn nominal_range(cfg: &HwConfig) -> [(f64, f64); 2] {
    let cal = Calibration::default();
    [0, 1].map(|c| cal.range(cfg.gains[c], c, cfg.high_speed()))
}

/// Sum, count, min and max of the raw samples of one output point.
#[derive(Clone, Copy, Debug)]
struct Acc {
    sum: u64,
    n: u32,
    min: u8,
    max: u8,
}

impl Acc {
    const EMPTY: Acc = Acc {
        sum: 0,
        n: 0,
        min: u8::MAX,
        max: 0,
    };
}

/// Min, max and sum of some ADC bytes.
#[inline]
fn span(bytes: impl Iterator<Item = u8>) -> (u8, u8, u64) {
    bytes.fold((u8::MAX, 0, 0), |(lo, hi, sum), b| {
        (lo.min(b), hi.max(b), sum + b as u64)
    })
}

/// [`span`] of both channels of interleaved bytes (CH1 first), in one
/// pass. Long pieces go 16 samples per channel at a time, in lanes the
/// compiler turns into SIMD; short ones (a point of a few samples) one
/// pair at a time.
#[inline]
fn span2(bytes: &[u8]) -> [(u8, u8, u64); 2] {
    const L: usize = 16;
    if bytes.len() >= 32 * L {
        let (mut lo, mut hi) = ([[u8::MAX; L]; 2], [[0u8; L]; 2]);
        let mut sum = [0u64; 2];
        let mut blocks = bytes.chunks_exact(2 * L);
        // u32 lane sums: 2^15 blocks at most between two flushes.
        let mut s = [[0u32; L]; 2];
        for (k, blk) in blocks.by_ref().enumerate() {
            for j in 0..L {
                let (a, b) = (blk[2 * j], blk[2 * j + 1]);
                lo[0][j] = lo[0][j].min(a);
                hi[0][j] = hi[0][j].max(a);
                s[0][j] += a as u32;
                lo[1][j] = lo[1][j].min(b);
                hi[1][j] = hi[1][j].max(b);
                s[1][j] += b as u32;
            }
            if k % (1 << 15) == (1 << 15) - 1 {
                for c in 0..2 {
                    sum[c] += s[c].iter().map(|&x| x as u64).sum::<u64>();
                    s[c] = [0; L];
                }
            }
        }
        let tail = span2(blocks.remainder());
        return [0, 1].map(|c| {
            (
                lo[c].iter().copied().fold(tail[c].0, u8::min),
                hi[c].iter().copied().fold(tail[c].1, u8::max),
                sum[c] + s[c].iter().map(|&x| x as u64).sum::<u64>() + tail[c].2,
            )
        });
    }
    let (mut lo, mut hi, mut sum) = ([u8::MAX; 2], [0u8; 2], [0u64; 2]);
    // u32 sums inside pieces of 2^24 bytes (no overflow), which the
    // compiler handles better.
    for part in bytes.chunks(1 << 24) {
        let mut s = [0u32; 2];
        for pair in part.chunks_exact(2) {
            for c in 0..2 {
                let b = pair[c];
                lo[c] = lo[c].min(b);
                hi[c] = hi[c].max(b);
                s[c] += b as u32;
            }
        }
        sum[0] += s[0] as u64;
        sum[1] += s[1] as u64;
    }
    [0, 1].map(|c| (lo[c], hi[c], sum[c]))
}

/// Decimates interleaved raw ADC bytes: each `factor` raw samples of a
/// channel make one output point with their min, max and mean (in ADC
/// counts). Keeps partial points between calls, so a stream can come in
/// pieces of any size.
#[derive(Clone, Debug)]
pub struct Decimator {
    channels: usize,
    factor: u32,
    /// Raw samples per channel still to throw away.
    skip: usize,
    /// Next byte belongs to this channel.
    next_ch: usize,
    acc: [Acc; 2],
    /// A point had a raw sample at 0x00 or 0xFF (the signal is too big
    /// for the range). Set by [`Decimator::push`]; clear it yourself.
    pub clipped: [bool; 2],
}

impl Decimator {
    /// `channels` interleaved channels (1 or 2), `factor` raw samples per
    /// point, `skip` raw samples per channel thrown away at the start.
    pub fn new(channels: u8, factor: usize, skip: usize) -> Self {
        Self {
            channels: channels.clamp(1, 2) as usize,
            factor: factor.clamp(1, u32::MAX as usize) as u32,
            skip,
            next_ch: 0,
            acc: [Acc::EMPTY; 2],
            clipped: [false; 2],
        }
    }

    pub fn factor(&self) -> usize {
        self.factor as usize
    }

    /// Take raw bytes. `out(channel, min, max, mean)` gets each finished
    /// point, in stream order.
    #[inline]
    pub fn push(&mut self, mut raw: &[u8], mut out: impl FnMut(usize, u8, u8, f64)) {
        while self.skip > 0 && !raw.is_empty() {
            self.next_ch += 1;
            if self.next_ch == self.channels {
                self.next_ch = 0;
                self.skip -= 1;
            }
            raw = &raw[1..];
        }
        let factor = self.factor;
        // One byte at a time, until a point starts at CH1 (then whole
        // points go in one go below) or the bytes end.
        let aligned = |me: &Self| me.next_ch == 0 && me.acc.iter().all(|a| a.n == 0);
        while !raw.is_empty() && !aligned(self) {
            self.byte(raw[0], &mut out);
            raw = &raw[1..];
        }
        // Whole points: min, max and sum in tight loops the compiler
        // vectorizes (the stream is up to 30 MB/s; a byte at a time was
        // most of the processing).
        let f = factor as usize;
        let frame = self.channels * f;
        let mut chunks = raw.chunks_exact(frame);
        for chunk in &mut chunks {
            let points = if self.channels == 1 {
                [span(chunk.iter().copied()), (0, 0, 0)]
            } else {
                span2(chunk)
            };
            for (c, &(lo, hi, sum)) in points.iter().enumerate().take(self.channels) {
                if lo == 0 || hi == 0xFF {
                    self.clipped[c] = true;
                }
                out(c, lo, hi, sum as f64 / factor as f64);
            }
        }
        for &b in chunks.remainder() {
            self.byte(b, &mut out);
        }
        // A partial point that clipped already: tell it now, not only
        // when the point is done (a long log step).
        for c in 0..self.channels {
            let a = &self.acc[c];
            if a.n > 0 && (a.min == 0 || a.max == 0xFF) {
                self.clipped[c] = true;
            }
        }
    }

    /// [`Decimator::push`] into `out`: the mean of each point, per
    /// channel. Without decimation (factor 1) the bytes go straight in,
    /// in a loop the compiler vectorizes.
    pub fn push_means(&mut self, mut raw: &[u8], out: &mut [Vec<f64>; 2]) {
        if self.factor == 1 && self.skip == 0 {
            // Byte by byte up to a CH1 byte, then whole frames.
            while !raw.is_empty() && self.next_ch != 0 {
                self.byte(raw[0], &mut |c, _, _, m| out[c].push(m));
                raw = &raw[1..];
            }
            let ch = self.channels;
            let whole = raw.len() / ch * ch;
            let (body, rest) = raw.split_at(whole);
            if ch == 1 {
                out[0].extend(body.iter().map(|&b| b as f64));
                if body.iter().any(|&b| b == 0 || b == 0xFF) {
                    self.clipped[0] = true;
                }
            } else {
                for (c, o) in out.iter_mut().enumerate() {
                    let it = body.iter().skip(c).step_by(2);
                    o.extend(it.clone().map(|&b| b as f64));
                    if it.clone().any(|&b| b == 0 || b == 0xFF) {
                        self.clipped[c] = true;
                    }
                }
            }
            for &b in rest {
                self.byte(b, &mut |c, _, _, m| out[c].push(m));
            }
        } else {
            self.push(raw, |c, _, _, m| out[c].push(m));
        }
    }

    /// Take one byte.
    #[inline]
    fn byte(&mut self, b: u8, out: &mut impl FnMut(usize, u8, u8, f64)) {
        let c = self.next_ch;
        self.next_ch = if c + 1 == self.channels { 0 } else { c + 1 };
        let a = &mut self.acc[c];
        a.sum += b as u64;
        a.n += 1;
        a.min = a.min.min(b);
        a.max = a.max.max(b);
        if a.n == self.factor {
            let p = std::mem::replace(a, Acc::EMPTY);
            if p.min == 0 || p.max == 0xFF {
                self.clipped[c] = true;
            }
            out(c, p.min, p.max, p.sum as f64 / self.factor as f64);
        }
    }

    /// End the partial points (data was lost, or the stream ends): each
    /// channel with samples gets a point of what it has. The next byte is
    /// CH1 again.
    pub fn flush(&mut self, mut out: impl FnMut(usize, u8, u8, f64)) {
        for c in 0..self.channels {
            let p = std::mem::replace(&mut self.acc[c], Acc::EMPTY);
            if p.n > 0 {
                if p.min == 0 || p.max == 0xFF {
                    self.clipped[c] = true;
                }
                out(c, p.min, p.max, p.sum as f64 / p.n as f64);
            }
        }
        self.next_ch = 0;
    }

    /// Drop the partial points; the next byte is CH1.
    pub fn reset(&mut self) {
        self.acc = [Acc::EMPTY; 2];
        self.next_ch = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_paths_give_what_a_byte_at_a_time_gives() {
        // Noise-like bytes with the extremes in them.
        let mut x = 12345u32;
        let raw: Vec<u8> = (0..20_011)
            .map(|_| {
                x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
                (x >> 16) as u8
            })
            .collect();
        for ch in [1u8, 2] {
            for factor in [1usize, 3, 16, 300, 1000] {
                let slow = {
                    let mut d = Decimator::new(ch, factor, 5);
                    let mut out = Vec::new();
                    for b in &raw {
                        d.push(std::slice::from_ref(b), |c, lo, hi, m| out.push((c, lo, hi, m)));
                    }
                    (out, d.clipped)
                };
                for piece in [7usize, 4096, raw.len()] {
                    let mut d = Decimator::new(ch, factor, 5);
                    let mut out = Vec::new();
                    for p in raw.chunks(piece) {
                        d.push(p, |c, lo, hi, m| out.push((c, lo, hi, m)));
                    }
                    assert_eq!((out, d.clipped), slow, "ch {ch} factor {factor} piece {piece}");
                    let mut d = Decimator::new(ch, factor, 5);
                    let mut means: [Vec<f64>; 2] = Default::default();
                    for p in raw.chunks(piece) {
                        d.push_means(p, &mut means);
                    }
                    for (c, got) in means.iter().enumerate().take(ch as usize) {
                        let want: Vec<f64> =
                            slow.0.iter().filter(|p| p.0 == c).map(|p| p.3).collect();
                        assert_eq!(*got, want, "means ch {ch} factor {factor} piece {piece}");
                    }
                }
            }
        }
    }

    #[test]
    fn caps_and_adc_rates() {
        assert_eq!(cap_index(16e6), 0);
        assert_eq!(cap_index(15e6), 0);
        assert_eq!(cap_index(1e6), 5);
        assert_eq!(cap_index(400e3), 7);
        assert_eq!(cap_index(f64::NAN), 0);
        assert_eq!(adc_rate(2, MAX_ADC_DEFAULT), (16, 16e6));
        assert_eq!(adc_rate(1, MAX_ADC_DEFAULT), (30, 30e6));
        assert_eq!(adc_rate(1, 10e6), (10, 10e6));
        assert_eq!(hw_channels(false, 16e6), 1);
        assert_eq!(hw_channels(true, 16e6), 2);
        // Below 1 MS/s both channels are sampled.
        assert_eq!(hw_channels(false, 500e3), 2);
        assert_eq!(cap_label(0), "16 MS/s · CH1 alone 30 MS/s");
        assert_eq!(cap_label(2), "10 MS/s");
        for (two, one) in ADC_CAPS {
            assert!(raw_rate_of_id(two) * 2.0 <= 32e6);
            assert!(raw_rate_of_id(one) <= 30e6);
        }
    }

    #[test]
    fn rate_choice_decimates_the_top_rate() {
        // (time/div, 2-channel rate, CH1 rate) at the default max ADC rate.
        let table = [
            (20e-6, 16e6, 30e6),
            (50e-6, 16e6, 15e6),
            (100e-6, 8e6, 10e6),
            (200e-6, 4e6, 5e6),
            (500e-6, 2e6, 2e6),
            (1e-3, 1e6, 1e6),
            (10e-3, 100e3, 100e3),
            (100e-3, 10e3, 10e3),
            (1.0, 1e3, 1e3),
            (10.0, 100.0, 100.0),
        ];
        for (tdiv, two, one) in table {
            let r2 = choose_rate(tdiv * 10.0, 2, MAX_ADC_DEFAULT);
            let r1 = choose_rate(tdiv * 10.0, 1, MAX_ADC_DEFAULT);
            assert_eq!((r2.rate, r2.raw), (two, 16e6), "{tdiv} 2ch");
            assert_eq!((r1.rate, r1.raw), (one, 30e6), "{tdiv} 1ch");
        }
        // The top rate is used: no decimation on fast screens.
        assert_eq!(choose_rate(1e-9, 2, MAX_ADC_DEFAULT).oversample, 1);
        assert_eq!(choose_rate(1e-9, 1, MAX_ADC_DEFAULT).rate, 30e6);
        // A lower max ADC rate.
        let r = choose_rate(1e-9, 2, 400e3);
        assert_eq!((r.rate, r.raw, r.id), (400e3, 400e3, 140));
        for max in ADC_CAPS.map(|c| raw_rate_of_id(c.0)) {
            for ch in [1, 2] {
                for e in -8..2 {
                    let r = choose_rate(10f64.powi(e), ch, max);
                    assert_eq!(raw_rate_of_id(r.id), r.raw);
                    assert!((r.raw / r.oversample as f64 - r.rate).abs() < 1e-6, "{r:?}");
                }
            }
        }
    }

    #[test]
    fn gain_index_steps() {
        assert_eq!(gain_index(0.02), 0);
        assert_eq!(gain_index(0.1), 2);
        assert_eq!(gain_index(0.2), 3);
        assert_eq!(gain_index(1.0), 5);
        assert_eq!(gain_index(5.0), 7);
        assert_eq!(gain_index(10.0), 7);
        assert_eq!(GAIN_BYTE[gain_index(0.5)], 2);
    }

    #[test]
    fn cal_freq_codes_match_openhantek() {
        let table = [
            (32, 103),
            (40, 0x68),
            (50, 0x69),
            (100, 0),
            (440, 0x90),
            (800, 0xB4),
            (1_000, 1),
            (1_200, 0xD4),
            (2_500, 0xE1),
            (3_000, 3),
            (10_000, 0x0A),
            (100_000, 0x64),
        ];
        for (hz, code) in table {
            assert_eq!(cal_freq_code(hz), code, "{hz} Hz");
            assert_eq!(cal_freq_of_code(code), hz as f64, "code {code}");
        }
        for hz in crate::settings::CAL_FREQS {
            assert_eq!(cal_freq_of_code(cal_freq_code(hz)), hz as f64, "{hz} Hz");
        }
    }

    #[test]
    fn coupling_bits() {
        assert_eq!(coupling_byte([true, true]), 0x11);
        assert_eq!(coupling_byte([false, true]), 0x10);
        assert_eq!(coupling_byte([true, false]), 0x01);
    }

    #[test]
    fn eeprom_factory_and_full() {
        let mut ee = [0xFFu8; 80];
        // Factory: only offsets. CH1 1 V/div (g 5) at 0x7E, CH2 at 0x83.
        ee[10] = 0x7E;
        ee[11] = 0x83;
        let cal = Calibration::from_eeprom(&ee);
        assert_eq!(cal.offset[0][5], [126.0, 131.0]);
        assert_eq!(cal.offset[0][0], [128.0, 128.0]);
        assert_eq!(cal.gain[5], [1.0, 1.0]);
        // With fine offset and gain bytes.
        ee[48 + 10] = 0x80 + 125; // +0.5 count
        ee[32 + 10] = 0x80 + 50; // gain 1.1
        let cal = Calibration::from_eeprom(&ee);
        assert!((cal.offset[0][5][0] - 126.5).abs() < 1e-9);
        assert!((cal.gain[5][0] - 1.1).abs() < 1e-9);
        // Too short: nominal values.
        assert_eq!(Calibration::from_eeprom(&[1, 2, 3]), Calibration::default());
    }

    #[test]
    fn ini_override() {
        let mut cal = Calibration::default();
        let text = "[eeprom]\nreplace_eeprom=true\n\n[gain]\nch0\\1000mV=1.05\nch1\\20mV=0.98\n\
                    ch0\\10mV=2\n\n[offset]\nch0\\1000mV=-6\nch1\\5000mV=3.5\nbad line\n";
        assert_eq!(cal.apply_ini(text), 4);
        assert_eq!(cal.ini_gain[5][0], 1.05);
        assert_eq!(cal.ini_gain[0][1], 0.98);
        assert_eq!(cal.ini_offset[5][0], -6.0);
        assert_eq!(cal.ini_offset[7][1], 3.5);
    }

    #[test]
    fn volts_and_raw_are_inverse() {
        let mut ee = [0xFFu8; 80];
        ee[10] = 0x7E;
        ee[32 + 10] = 0x80 + 20;
        let mut cal = Calibration::from_eeprom(&ee);
        cal.apply_ini("[offset]\nch0\\1000mV=1.5\n[gain]\nch0\\1000mV=1.02\n");
        for v in [-4.0, -0.3, 0.0, 1.0, 3.3] {
            let raw = cal.raw(v, 5, 0, false);
            assert!((cal.volts(raw, 5, 0, false) - v).abs() < 1e-9);
        }
        // Nominal: 0x80 is 0 V, 24.75 counts per volt at 1 V/div.
        let nominal = Calibration::default();
        assert_eq!(nominal.volts(128.0, 5, 1, false), 0.0);
        assert!((nominal.volts(128.0 + 24.75, 5, 1, false) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn decimator_interleave_skip_min_max_mean() {
        // 2 channels, 2 raw samples per point, skip 1 sample per channel.
        let mut d = Decimator::new(2, 2, 1);
        let mut out = Vec::new();
        let raw = [0u8, 255, 150, 128, 160, 120, 128, 128];
        // Split in odd pieces to test the carry between calls.
        d.push(&raw[..3], |c, a, b, m| out.push((c, a, b, m)));
        d.push(&raw[3..], |c, a, b, m| out.push((c, a, b, m)));
        assert_eq!(out, vec![(0, 150, 160, 155.0), (1, 120, 128, 124.0)]);
        // The skipped 0x00 / 0xFF bytes do not count as clipping.
        assert_eq!(d.clipped, [false, false]);
        // The last 128 128 wait for their second samples.
        out.clear();
        d.push(&[0, 0], |c, a, b, m| out.push((c, a, b, m)));
        assert_eq!(out, vec![(0, 0, 128, 64.0), (1, 0, 128, 64.0)]);
        assert_eq!(d.clipped, [true, true]);
        // Clipping shows before its point is done.
        d.clipped = [false; 2];
        d.push(&[0xFF], |_, _, _, _| {});
        assert_eq!(d.clipped, [true, false]);
        d.reset();
        // A flush ends partial points.
        out.clear();
        d.push(&[100], |c, a, b, m| out.push((c, a, b, m)));
        d.flush(|c, a, b, m| out.push((c, a, b, m)));
        assert_eq!(out, vec![(0, 100, 100, 100.0)]);
    }

    #[test]
    fn decimator_single_channel() {
        let mut d = Decimator::new(1, 1, 0);
        let mut n = 0;
        d.push(&[128, 128, 153], |c, _, _, _| {
            assert_eq!(c, 0);
            n += 1
        });
        assert_eq!(n, 3);
    }

    #[test]
    fn app_calibration_wins_over_the_eeprom() {
        let mut ee = [0xFFu8; 80];
        // CH1 1 V/div: 132 at low speed, 136 at 30 MS/s (high by 4).
        ee[10] = 132;
        ee[16 + 10] = 136;
        let mut cal = Calibration::from_eeprom(&ee);
        cal.apply_ini("[offset]\nch0\\1000mV=1\n");
        assert_eq!(cal.zero(5, 0, true), 137.0);
        // An app zero for the 30 MS/s bank only.
        cal.app.zero[1][5][0] = 132.5;
        assert_eq!(cal.zero(5, 0, true), 132.5);
        assert_eq!(cal.zero(5, 0, false), 133.0);
        assert_eq!(cal.device_zero(5, 0, true), 137.0);
        assert!(cal.volts(132.5, 5, 0, true).abs() < 1e-12);
        // The app gain scales the volts; raw stays the inverse.
        cal.app.gain[0] = 1.1;
        let v = cal.volts(132.5 + 24.75, 5, 0, true);
        assert!((v - 1.1).abs() < 1e-9, "{v}");
        for v in [-2.0, 0.0, 0.7] {
            assert!((cal.volts(cal.raw(v, 5, 0, true), 5, 0, true) - v).abs() < 1e-9);
        }
        // CH2 is not touched.
        assert_eq!(cal.volts(128.0 + 24.75, 5, 1, false), 1.0);
    }

    #[test]
    fn linear_calibration() {
        let mut cal = Calibration::default();
        cal.apply_ini("[offset]\nch0\\1000mV=1.5\n[gain]\nch0\\1000mV=1.02\n");
        let (a, b) = cal.linear(5, 0, false);
        for raw in [0.0, 100.5, 255.0] {
            assert!((a * raw + b - cal.volts(raw, 5, 0, false)).abs() < 1e-12);
        }
    }
}
