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

/// One sample rate step. The ADC runs at `raw` samples per second; we
/// average `oversample` raw samples into one output sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rate {
    /// Output samples per second.
    pub rate: f64,
    /// Rate id byte for request 0xE2.
    pub id: u8,
    /// ADC samples per second.
    pub raw: f64,
    pub oversample: usize,
}

impl Rate {
    /// Rates below 10 kS/s always run with both channels.
    pub fn slow(&self) -> bool {
        self.rate < 10e3
    }
}

const fn r(rate: f64, id: u8, raw: f64, oversample: usize) -> Rate {
    Rate {
        rate,
        id,
        raw,
        oversample,
    }
}

/// All rates we use, slowest first. 48 MS/s is left out: USB cannot keep up.
pub const RATES: [Rate; 20] = [
    r(100.0, 102, 20e3, 200),
    r(200.0, 104, 40e3, 200),
    r(500.0, 110, 100e3, 200),
    r(1e3, 120, 200e3, 200),
    r(2e3, 140, 400e3, 200),
    r(5e3, 1, 1e6, 200),
    r(10e3, 1, 1e6, 100),
    r(20e3, 2, 2e6, 100),
    r(50e3, 5, 5e6, 100),
    r(100e3, 10, 10e6, 100),
    r(200e3, 10, 10e6, 50),
    r(500e3, 10, 10e6, 20),
    r(1e6, 10, 10e6, 10),
    r(2e6, 10, 10e6, 5),
    r(5e6, 10, 10e6, 2),
    r(10e6, 10, 10e6, 1),
    r(12e6, 12, 12e6, 1),
    r(15e6, 15, 15e6, 1),
    r(24e6, 24, 24e6, 1),
    r(30e6, 30, 30e6, 1),
];

/// Highest ADC rate with both channels on.
pub const MAX_RATE_2CH: f64 = 15e6;
/// Highest ADC rate with only CH1 on.
pub const MAX_RATE_1CH: f64 = 30e6;

/// ADC rate for a rate id, as the firmware decodes it.
pub fn raw_rate_of_id(id: u8) -> f64 {
    if id < 100 {
        id as f64 * 1e6
    } else {
        (id as f64 - 100.0) * 10e3
    }
}

/// Output samples per block, per channel (as OpenHantek6022).
pub const BLOCK_SAMPLES: usize = 20_000;
/// At most this many samples on one screen, so the block has room for the
/// trigger search before and after the screen.
pub const MAX_SCREEN_SAMPLES: f64 = 10_000.0;

/// Pick the sample rate for a screen of `screen_time` seconds: the highest
/// rate that fits `MAX_SCREEN_SAMPLES` on the screen. Like OpenHantek6022,
/// the top rate of each mode (15 MS/s for 2 channels, 30 MS/s for CH1) is
/// not picked, because USB is close to its limit there.
pub fn choose_rate(screen_time: f64, two_channels: bool) -> Rate {
    let limit = if two_channels {
        MAX_RATE_2CH
    } else {
        MAX_RATE_1CH
    };
    RATES
        .iter()
        .rev()
        .find(|r| r.rate < limit && r.rate * screen_time <= MAX_SCREEN_SAMPLES * 1.0001)
        .copied()
        .unwrap_or(RATES[0])
}

/// Fastest ADC rate used in roll mode. Roll mode streams without end, and
/// between two USB reads the scope FIFO must hold the data. At 400 kS/s it
/// holds a few ms, which is enough; at 1 MS/s samples get lost.
pub const MAX_ROLL_RAW: f64 = 400e3;

/// Sample rate for roll mode: the same output rate as [`choose_rate`],
/// but with the ADC at most at [`MAX_ROLL_RAW`] (less oversampling).
pub fn roll_rate(screen_time: f64) -> Rate {
    let r = choose_rate(screen_time, true);
    if r.raw <= MAX_ROLL_RAW {
        return r;
    }
    Rate {
        rate: r.rate,
        id: 140,
        raw: MAX_ROLL_RAW,
        oversample: ((MAX_ROLL_RAW / r.rate).round() as usize).max(1),
    }
}

/// ADC samples per channel to read for `n` output samples (OpenHantek6022's
/// grossSampleCount). Always a multiple of 1024, so of 512-byte packets.
pub fn gross_samples(n: usize) -> usize {
    ((n + 1024) / 1024 + 2) * 1024
}

/// Usable ADC samples per channel in a block of `gross` samples.
pub fn net_samples(gross: usize) -> usize {
    ((gross - 1024) / 1000 - 1) * 1000
}

/// ADC samples per channel to throw away at the start of each block. The
/// ADC is not stable just after sampling starts.
pub fn skip_samples(gross: usize) -> usize {
    gross - net_samples(gross)
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
}

impl Default for Calibration {
    fn default() -> Self {
        Self {
            offset: [[[128.0; 2]; GAIN_STEPS]; 2],
            gain: [[1.0; 2]; GAIN_STEPS],
            ini_offset: [[0.0; 2]; GAIN_STEPS],
            ini_gain: [[1.0; 2]; GAIN_STEPS],
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

    /// Volts at the BNC for an (averaged) ADC value.
    pub fn volts(&self, raw: f64, gain: usize, channel: usize, high_speed: bool) -> f64 {
        let bank = high_speed as usize;
        let s = (raw - self.offset[bank][gain][channel] - self.ini_offset[gain][channel])
            * self.ini_gain[gain][channel];
        s / COUNTS_PER_VOLT[gain] * self.gain[gain][channel]
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
        let bank = high_speed as usize;
        volts * COUNTS_PER_VOLT[gain] / self.gain[gain][channel] / self.ini_gain[gain][channel]
            + self.offset[bank][gain][channel]
            + self.ini_offset[gain][channel]
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
    /// Stream without end and do not trigger (slow time bases).
    pub roll: bool,
}

impl HwConfig {
    /// ADC samples per channel in one (non-roll) block.
    pub fn gross(&self) -> usize {
        gross_samples(BLOCK_SAMPLES * self.rate.oversample)
    }

    /// Bytes in one (non-roll) block.
    pub fn block_bytes(&self) -> usize {
        self.gross() * self.channels as usize
    }

    /// True when the high-speed offset bank applies.
    pub fn high_speed(&self) -> bool {
        self.rate.rate >= 30e6
    }
}

/// The input range of each channel for `cfg` with the nominal
/// calibration (no EEPROM values).
pub fn nominal_range(cfg: &HwConfig) -> [(f64, f64); 2] {
    let cal = Calibration::default();
    [0, 1].map(|c| cal.range(cfg.gains[c], c, cfg.high_speed()))
}

/// Turns raw ADC bytes into volts, with oversampling. Keeps partial
/// samples between calls, so a stream can come in pieces.
#[derive(Clone, Debug)]
pub struct Converter {
    channels: usize,
    oversample: usize,
    gains: [usize; 2],
    high_speed: bool,
    /// Raw samples per channel still to throw away.
    skip: usize,
    /// Sum and count of the sample being built, per channel.
    acc: [(f64, usize); 2],
    /// Next byte belongs to this channel.
    next_ch: usize,
    pub clipped: [bool; 2],
}

impl Converter {
    pub fn new(cfg: &HwConfig, skip: usize) -> Self {
        Self {
            channels: cfg.channels as usize,
            oversample: cfg.rate.oversample.max(1),
            gains: cfg.gains,
            high_speed: cfg.high_speed(),
            skip,
            acc: [(0.0, 0); 2],
            next_ch: 0,
            clipped: [false; 2],
        }
    }

    /// Convert `raw` bytes and append volts to `out` (CH2 stays empty in
    /// 1-channel mode).
    pub fn push(&mut self, raw: &[u8], cal: &Calibration, out: &mut [Vec<f64>; 2]) {
        for &b in raw {
            let c = self.next_ch;
            self.next_ch = (self.next_ch + 1) % self.channels;
            if self.skip > 0 {
                if self.next_ch == 0 {
                    self.skip -= 1;
                }
                continue;
            }
            if b == 0x00 || b == 0xFF {
                self.clipped[c] = true;
            }
            let acc = &mut self.acc[c];
            acc.0 += b as f64;
            acc.1 += 1;
            if acc.1 == self.oversample {
                let mean = acc.0 / acc.1 as f64;
                *acc = (0.0, 0);
                out[c].push(cal.volts(mean, self.gains[c], c, self.high_speed));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_ids_decode_to_raw_rates() {
        for r in RATES {
            assert_eq!(raw_rate_of_id(r.id), r.raw, "{r:?}");
            assert!((r.raw / r.oversample as f64 - r.rate).abs() < 1e-6, "{r:?}");
        }
    }

    #[test]
    fn rate_choice_matches_openhantek() {
        // (time/div, 2-channel rate, 1-channel rate) from OpenHantek6022.
        let table = [
            (20e-6, 12e6, 24e6),
            (50e-6, 12e6, 15e6),
            (100e-6, 10e6, 10e6),
            (200e-6, 5e6, 5e6),
            (1e-3, 1e6, 1e6),
            (10e-3, 100e3, 100e3),
            (100e-3, 10e3, 10e3),
            (1.0, 1e3, 1e3),
            (10.0, 100.0, 100.0),
        ];
        for (tdiv, two, one) in table {
            assert_eq!(choose_rate(tdiv * 10.0, true).rate, two, "{tdiv} 2ch");
            assert_eq!(choose_rate(tdiv * 10.0, false).rate, one, "{tdiv} 1ch");
        }
        // Very fast screens get the fastest rate allowed.
        assert_eq!(choose_rate(1e-9, true).rate, 12e6);
        assert_eq!(choose_rate(1e-9, false).rate, 24e6);
    }

    #[test]
    fn roll_rate_keeps_the_adc_slow() {
        for tdiv in [100e-3, 200e-3, 500e-3, 1.0, 10.0] {
            let r = roll_rate(tdiv * 10.0);
            assert_eq!(r.rate, choose_rate(tdiv * 10.0, true).rate);
            assert!(r.raw <= MAX_ROLL_RAW);
            assert_eq!(raw_rate_of_id(r.id), r.raw);
            assert!((r.raw / r.oversample as f64 - r.rate).abs() < 1e-9, "{r:?}");
        }
        assert_eq!(roll_rate(1.0).oversample, 40);
    }

    #[test]
    fn block_sizes_match_openhantek() {
        assert_eq!(gross_samples(20_000), 22_528);
        assert_eq!(skip_samples(22_528), 2_528);
        assert_eq!(gross_samples(200_000), 202_752);
        assert_eq!(skip_samples(202_752), 2_752);
        // Net samples always hold a full block after oversampling.
        for r in RATES {
            let g = gross_samples(BLOCK_SAMPLES * r.oversample);
            assert_eq!(g % PACKET_SIZE, 0);
            assert!(net_samples(g) >= BLOCK_SAMPLES * r.oversample, "{r:?}");
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

    fn cfg(channels: u8, oversample: usize) -> HwConfig {
        HwConfig {
            rate: Rate {
                rate: 1e6 / oversample as f64,
                id: 1,
                raw: 1e6,
                oversample,
            },
            channels,
            gains: [5, 5],
            dc: [true; 2],
            cal_freq: 1000,
            roll: false,
        }
    }

    #[test]
    fn converter_interleave_skip_and_oversample() {
        let cal = Calibration::default();
        // 2 channels, oversample 2, skip 1 sample per channel.
        let mut conv = Converter::new(&cfg(2, 2), 1);
        let mut out = [Vec::new(), Vec::new()];
        // Skipped pair, then CH1 = 128+24.75*k pattern, CH2 = 128.
        let one = 128 + 25; // close to 1 V
        let raw = [0u8, 255, one, 128, one, 128, 128, 128];
        // Split in odd pieces to test the carry between calls.
        conv.push(&raw[..3], &cal, &mut out);
        conv.push(&raw[3..], &cal, &mut out);
        assert_eq!(out[0].len(), 1);
        assert_eq!(out[1].len(), 1);
        assert!((out[0][0] - 25.0 / 24.75).abs() < 1e-9);
        assert_eq!(out[1][0], 0.0);
        // The skipped 0x00 / 0xFF bytes do not count as clipping.
        assert_eq!(conv.clipped, [false, false]);
        conv.push(&[0, 0], &cal, &mut out);
        assert_eq!(conv.clipped, [true, true]);
    }

    #[test]
    fn converter_single_channel() {
        let cal = Calibration::default();
        let mut conv = Converter::new(&cfg(1, 1), 0);
        let mut out = [Vec::new(), Vec::new()];
        conv.push(&[128, 128, 153], &cal, &mut out);
        assert_eq!(out[0].len(), 3);
        assert!(out[1].is_empty());
    }
}
