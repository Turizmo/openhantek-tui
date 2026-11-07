//! Scope settings. These are saved to the settings file on quit.

use serde::{Deserialize, Serialize};

use crate::dsp::fft::Window;
use crate::dsp::math::MathMode;
use crate::dsp::trigger::Slope;

/// Screen grid: 10 divisions wide, 8 divisions high, like a real scope.
pub const H_DIVS: f64 = 10.0;
pub const V_DIVS: f64 = 8.0;

/// Time per division steps, in seconds (1-2-5 sequence).
pub const TIME_DIVS: [f64; 27] = [
    20e-9, 50e-9, 100e-9, 200e-9, 500e-9, 1e-6, 2e-6, 5e-6, 10e-6, 20e-6, 50e-6, 100e-6, 200e-6,
    500e-6, 1e-3, 2e-3, 5e-3, 10e-3, 20e-3, 50e-3, 100e-3, 200e-3, 500e-3, 1.0, 2.0, 5.0, 10.0,
];

/// Volts per division steps at the scope input (probe x1).
pub const VOLT_DIVS: [f64; 9] = [0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0];

/// Time per division where roll mode starts. Slower screens scroll.
pub const ROLL_FROM: f64 = 100e-3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Coupling {
    #[default]
    Dc,
    Ac,
}

impl Coupling {
    pub fn toggle(self) -> Self {
        match self {
            Coupling::Dc => Coupling::Ac,
            Coupling::Ac => Coupling::Dc,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Coupling::Dc => "DC",
            Coupling::Ac => "AC",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelSettings {
    pub enabled: bool,
    /// Index into [`VOLT_DIVS`].
    pub volt_idx: usize,
    /// Vertical position of the 0 V line, in divisions.
    pub offset: f64,
    pub coupling: Coupling,
    /// Probe attenuation: 1 or 10.
    pub probe: u32,
    pub invert: bool,
}

impl Default for ChannelSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            volt_idx: 5, // 1 V/div
            offset: 0.0,
            coupling: Coupling::Dc,
            probe: 1,
            invert: false,
        }
    }
}

impl ChannelSettings {
    /// Volts per division at the probe tip.
    pub fn volts_per_div(&self) -> f64 {
        VOLT_DIVS[self.volt_idx.min(VOLT_DIVS.len() - 1)] * self.probe as f64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TriggerMode {
    /// Show a trace even with no trigger.
    #[default]
    Auto,
    /// Show a trace only when it triggers.
    Normal,
    /// Show the first trace that triggers, then stop.
    Single,
}

impl TriggerMode {
    pub fn next(self) -> Self {
        match self {
            TriggerMode::Auto => TriggerMode::Normal,
            TriggerMode::Normal => TriggerMode::Single,
            TriggerMode::Single => TriggerMode::Auto,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            TriggerMode::Auto => "Auto",
            TriggerMode::Normal => "Normal",
            TriggerMode::Single => "Single",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TriggerSettings {
    pub mode: TriggerMode,
    /// 0 = CH1, 1 = CH2.
    pub source: usize,
    pub slope: Slope,
    /// Trigger level at the probe tip, in volts.
    pub level: f64,
    /// Where the trigger point sits on the screen: 0 = left edge, 1 = right edge.
    pub position: f64,
}

impl Default for TriggerSettings {
    fn default() -> Self {
        Self {
            mode: TriggerMode::Auto,
            source: 0,
            slope: Slope::Rising,
            level: 0.0,
            position: 0.5,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MathSettings {
    pub enabled: bool,
    pub mode: MathMode,
    /// Index into [`VOLT_DIVS`] (units per division).
    pub volt_idx: usize,
    pub offset: f64,
}

impl Default for MathSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: MathMode::Add,
            volt_idx: 6, // 2 per div
            offset: 0.0,
        }
    }
}

impl MathSettings {
    pub fn units_per_div(&self) -> f64 {
        VOLT_DIVS[self.volt_idx.min(VOLT_DIVS.len() - 1)]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CursorMode {
    #[default]
    Off,
    /// Two vertical lines: time and frequency between them.
    Time,
    /// Two horizontal lines: voltage between them.
    Volt,
}

impl CursorMode {
    pub fn next(self) -> Self {
        match self {
            CursorMode::Off => CursorMode::Time,
            CursorMode::Time => CursorMode::Volt,
            CursorMode::Volt => CursorMode::Off,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CursorSettings {
    pub mode: CursorMode,
    /// Time cursors, in divisions from the screen center.
    pub t: [f64; 2],
    /// Voltage cursors, in divisions from the screen center.
    pub v: [f64; 2],
}

impl Default for CursorSettings {
    fn default() -> Self {
        Self {
            mode: CursorMode::Off,
            t: [-2.0, 2.0],
            v: [-1.0, 1.0],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Interpolation {
    /// Straight lines between samples.
    Linear,
    /// sin(x)/x, when a screen has few samples.
    #[default]
    Sinc,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplaySettings {
    /// Show the spectrum below the time view.
    pub spectrum: bool,
    pub window: Window,
    /// CH1 on X, CH2 on Y.
    pub xy: bool,
    pub interpolation: Interpolation,
    /// Number of captures to average: 1 means off.
    pub averaging: usize,
    /// Number of old traces to keep on screen, dimmed: 0 means off.
    pub persistence: usize,
    /// Draw dots, not lines.
    pub dots: bool,
    pub measurements: bool,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            spectrum: false,
            window: Window::Hann,
            xy: false,
            interpolation: Interpolation::Sinc,
            averaging: 1,
            persistence: 0,
            dots: false,
            measurements: true,
        }
    }
}

/// Frequencies of the calibration output pin, in Hz (the steps OpenHantek6022 offers).
pub const CAL_FREQS: [u32; 38] = [
    32, 40, 50, 60, 80, 100, 120, 160, 200, 250, 300, 400, 440, 500, 600, 660, 800, 1_000, 1_200,
    1_600, 2_000, 2_500, 3_000, 4_000, 5_000, 6_000, 8_000, 10_000, 12_000, 16_000, 20_000, 25_000,
    30_000, 40_000, 50_000, 60_000, 80_000, 100_000,
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub channels: [ChannelSettings; 2],
    pub math: MathSettings,
    /// Index into [`TIME_DIVS`].
    pub time_idx: usize,
    pub trigger: TriggerSettings,
    pub cursors: CursorSettings,
    pub display: DisplaySettings,
    /// Calibration output frequency, in Hz.
    pub cal_freq: u32,
    /// Selected trace: 0 = CH1, 1 = CH2, 2 = MATH.
    pub selected: usize,
}

impl Default for Settings {
    fn default() -> Self {
        let ch2 = ChannelSettings {
            offset: -2.0,
            ..ChannelSettings::default()
        };
        let ch1 = ChannelSettings {
            offset: 2.0,
            ..ChannelSettings::default()
        };
        Self {
            channels: [ch1, ch2],
            math: MathSettings::default(),
            time_idx: 14, // 1 ms/div
            trigger: TriggerSettings::default(),
            cursors: CursorSettings::default(),
            display: DisplaySettings::default(),
            cal_freq: 1_000,
            selected: 0,
        }
    }
}

impl Settings {
    /// Fix values that a hand-edited file may have out of range.
    pub fn sanitize(&mut self) {
        let half_v = V_DIVS / 2.0;
        for ch in &mut self.channels {
            ch.volt_idx = ch.volt_idx.min(VOLT_DIVS.len() - 1);
            ch.offset = finite_or(ch.offset, 0.0).clamp(-half_v, half_v);
            if ch.probe != 1 && ch.probe != 10 {
                ch.probe = 1;
            }
        }
        self.math.volt_idx = self.math.volt_idx.min(VOLT_DIVS.len() - 1);
        self.math.offset = finite_or(self.math.offset, 0.0).clamp(-half_v, half_v);
        self.time_idx = self.time_idx.min(TIME_DIVS.len() - 1);
        self.trigger.source = self.trigger.source.min(1);
        self.trigger.level = finite_or(self.trigger.level, 0.0);
        self.trigger.position = finite_or(self.trigger.position, 0.5).clamp(0.0, 1.0);
        let half_h = H_DIVS / 2.0;
        for t in &mut self.cursors.t {
            *t = finite_or(*t, 0.0).clamp(-half_h, half_h);
        }
        for v in &mut self.cursors.v {
            *v = finite_or(*v, 0.0).clamp(-half_v, half_v);
        }
        self.display.averaging = self.display.averaging.clamp(1, 128);
        self.display.persistence = self.display.persistence.min(16);
        if !CAL_FREQS.contains(&self.cal_freq) {
            self.cal_freq = 1_000;
        }
        self.selected = self.selected.min(2);
    }

    pub fn time_per_div(&self) -> f64 {
        TIME_DIVS[self.time_idx.min(TIME_DIVS.len() - 1)]
    }

    /// Time across the whole screen, in seconds.
    pub fn screen_time(&self) -> f64 {
        self.time_per_div() * H_DIVS
    }

    /// True when the screen scrolls (slow time bases).
    pub fn roll(&self) -> bool {
        self.time_per_div() >= ROLL_FROM * 0.999
    }
}

fn finite_or(v: f64, default: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        default
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let mut s = Settings::default();
        let before = s.clone();
        s.sanitize();
        assert_eq!(s, before);
        assert_eq!(s.time_per_div(), 1e-3);
        assert_eq!(s.channels[0].volts_per_div(), 1.0);
        assert!(!s.roll());
    }

    #[test]
    fn sanitize_fixes_bad_values() {
        let mut s = Settings::default();
        s.time_idx = 999;
        s.channels[0].volt_idx = 50;
        s.channels[1].probe = 7;
        s.channels[1].offset = f64::NAN;
        s.trigger.position = 3.0;
        s.trigger.source = 5;
        s.cal_freq = 12345;
        s.display.averaging = 0;
        s.selected = 9;
        s.sanitize();
        assert_eq!(s.time_idx, TIME_DIVS.len() - 1);
        assert_eq!(s.channels[0].volt_idx, VOLT_DIVS.len() - 1);
        assert_eq!(s.channels[1].probe, 1);
        assert_eq!(s.channels[1].offset, 0.0);
        assert_eq!(s.trigger.position, 1.0);
        assert_eq!(s.trigger.source, 1);
        assert_eq!(s.cal_freq, 1000);
        assert_eq!(s.display.averaging, 1);
        assert_eq!(s.selected, 2);
    }

    #[test]
    fn probe_scales_volts_per_div() {
        let ch = ChannelSettings {
            probe: 10,
            ..Default::default()
        };
        assert_eq!(ch.volts_per_div(), 10.0);
    }

    #[test]
    fn roll_mode_on_slow_time_base() {
        let mut s = Settings::default();
        s.time_idx = TIME_DIVS.iter().position(|t| *t == 100e-3).unwrap();
        assert!(s.roll());
        s.time_idx -= 1;
        assert!(!s.roll());
    }

    #[test]
    fn toml_round_trip() {
        let mut s = Settings::default();
        s.trigger.slope = Slope::Both;
        s.display.window = Window::FlatTop;
        s.math.mode = MathMode::Mul;
        let text = toml::to_string_pretty(&s).unwrap();
        let back: Settings = toml::from_str(&text).unwrap();
        assert_eq!(back, s);
    }
}
