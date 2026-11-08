//! Scope settings. These are saved to the settings file on quit.

use serde::{Deserialize, Serialize};

use crate::dsp::fft::Window;
use crate::dsp::math::MathMode;
use crate::dsp::trigger::Slope;
use crate::theme::ColorMode;

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
    /// Fine volts/div (Alt-↑ ↓): a factor on the step `volt_idx`, between
    /// the step below and this one; 1 = the step itself. The hardware gain
    /// is the one of the step, so the range covers the screen.
    pub volts_fine: f64,
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
            volts_fine: 1.0,
        }
    }
}

impl ChannelSettings {
    /// Volts per division at the probe tip.
    pub fn volts_per_div(&self) -> f64 {
        self.volts_div_bnc() * self.probe as f64
    }

    /// Volts per division at the BNC (probe x1), with the fine factor.
    pub fn volts_div_bnc(&self) -> f64 {
        VOLT_DIVS[self.volt_idx.min(VOLT_DIVS.len() - 1)] * self.volts_fine
    }
}

/// A value between the steps of a 1-2-5 list: the index of the step at or
/// above it, and the factor on that step (1 = on the step). Values out
/// of the list's range are clamped to it.
pub fn fine_step(steps: &[f64], value: f64) -> (usize, f64) {
    let v = if value.is_finite() { value } else { steps[0] };
    let v = v.clamp(steps[0], steps[steps.len() - 1]);
    let i = steps
        .iter()
        .position(|s| *s >= v * (1.0 - 1e-6))
        .unwrap_or(steps.len() - 1);
    let f = v / steps[i];
    (i, if f > 0.9999 { 1.0 } else { f })
}

/// One fine step from `value` (Alt with ↑ ↓ ← →): a tenth of the gap to
/// the next step down (`up` false) or up, rounded to 3 significant digits.
pub fn fine_move(steps: &[f64], value: f64, up: bool) -> (usize, f64) {
    let eps = 1e-6;
    let lo = steps
        .iter()
        .rev()
        .find(|s| {
            if up {
                **s <= value * (1.0 + eps)
            } else {
                **s < value * (1.0 - eps)
            }
        })
        .copied();
    let hi = steps
        .iter()
        .find(|s| {
            if up {
                **s > value * (1.0 + eps)
            } else {
                **s >= value * (1.0 - eps)
            }
        })
        .copied();
    let (Some(lo), Some(hi)) = (lo, hi) else {
        return fine_step(steps, value);
    };
    let v = value + if up { 1.0 } else { -1.0 } * (hi - lo) / 10.0;
    fine_step(steps, round_sig(v, 3))
}

/// `v` rounded to `n` significant digits.
pub fn round_sig(v: f64, n: i32) -> f64 {
    if v == 0.0 || !v.is_finite() {
        return v;
    }
    let p = 10f64.powi(n - 1 - v.abs().log10().floor() as i32);
    (v * p).round() / p
}

/// One normal step of a 1-2-5 list from (`idx`, `fine`): a fine value
/// snaps to the step below (`up` false) or to its own step.
pub fn coarse_move(len: usize, idx: usize, fine: f64, up: bool) -> usize {
    if up {
        (idx + usize::from(fine >= 1.0)).min(len - 1)
    } else {
        idx.saturating_sub(1)
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
    /// Fine units/div: a factor on the step, like `volts_fine`.
    pub units_fine: f64,
}

impl Default for MathSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: MathMode::Add,
            volt_idx: 6, // 2 per div
            offset: 0.0,
            units_fine: 1.0,
        }
    }
}

impl MathSettings {
    pub fn units_per_div(&self) -> f64 {
        VOLT_DIVS[self.volt_idx.min(VOLT_DIVS.len() - 1)] * self.units_fine
    }
}

/// The cursor mode of old settings files (one switch for all cursors).
/// It only loads: `Time` turns time cursors A and B on, `Volt` the
/// voltage cursors, `Off` none.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Default)]
pub enum CursorMode {
    #[default]
    Off,
    Time,
    Volt,
}

/// Which cursor: time A, time B, voltage A, voltage B.
pub const CURSOR_NAMES: [&str; 4] = ["Time A", "Time B", "Voltage A", "Voltage B"];

/// Cursor colours: a small palette that also works with 16 colours.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CursorColor {
    /// The grey of the first versions.
    #[default]
    Grey,
    White,
    Yellow,
    Cyan,
    Magenta,
    Green,
    Red,
    Blue,
}

impl CursorColor {
    pub const ALL: [CursorColor; 8] = [
        CursorColor::Grey,
        CursorColor::White,
        CursorColor::Yellow,
        CursorColor::Cyan,
        CursorColor::Magenta,
        CursorColor::Green,
        CursorColor::Red,
        CursorColor::Blue,
    ];

    pub fn name(self) -> &'static str {
        match self {
            CursorColor::Grey => "grey",
            CursorColor::White => "white",
            CursorColor::Yellow => "yellow",
            CursorColor::Cyan => "cyan",
            CursorColor::Magenta => "magenta",
            CursorColor::Green => "green",
            CursorColor::Red => "red",
            CursorColor::Blue => "blue",
        }
    }
}

/// One cursor line.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cursor {
    pub enabled: bool,
    /// Position in divisions from the screen center (x for time cursors,
    /// y for voltage cursors).
    pub position: f64,
    pub style: CursorStyle,
    pub color: CursorColor,
}

impl Cursor {
    fn at(position: f64) -> Self {
        Self {
            enabled: false,
            position,
            style: CursorStyle::Classic,
            color: CursorColor::Grey,
        }
    }
}

/// The four cursors. `v` hides all of them and shows them again, each
/// with its own `enabled`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "CursorFile")]
pub struct CursorSettings {
    /// All cursors hidden (key `v`); `enabled` of each stays.
    pub hidden: bool,
    pub time_a: Cursor,
    pub time_b: Cursor,
    pub volt_a: Cursor,
    pub volt_b: Cursor,
    /// Loaded from a file of the old format: the old global cursor style
    /// (`display.cursor_style`) still has to be applied.
    #[serde(skip)]
    pub legacy: bool,
}

impl Default for CursorSettings {
    fn default() -> Self {
        Self {
            hidden: false,
            time_a: Cursor::at(-2.0),
            time_b: Cursor::at(2.0),
            volt_a: Cursor::at(-1.0),
            volt_b: Cursor::at(1.0),
            legacy: false,
        }
    }
}

impl CursorSettings {
    /// Cursor `i`: 0 time A, 1 time B, 2 voltage A, 3 voltage B.
    pub fn get(&self, i: usize) -> &Cursor {
        match i {
            0 => &self.time_a,
            1 => &self.time_b,
            2 => &self.volt_a,
            _ => &self.volt_b,
        }
    }

    pub fn get_mut(&mut self, i: usize) -> &mut Cursor {
        match i {
            0 => &mut self.time_a,
            1 => &mut self.time_b,
            2 => &mut self.volt_a,
            _ => &mut self.volt_b,
        }
    }

    /// True when cursor `i` is on screen: enabled and not hidden.
    pub fn shown(&self, i: usize) -> bool {
        !self.hidden && self.get(i).enabled
    }

    /// True when cursor `i` is a time cursor (a vertical line).
    pub fn is_time(i: usize) -> bool {
        i < 2
    }
}

/// One cursor as a file has it: every field may be missing.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(default)]
struct CursorEntry {
    enabled: Option<bool>,
    position: Option<f64>,
    style: Option<CursorStyle>,
    color: Option<CursorColor>,
}

/// The `[cursors]` table of a file, new or old format.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct CursorFile {
    hidden: bool,
    time_a: Option<CursorEntry>,
    time_b: Option<CursorEntry>,
    volt_a: Option<CursorEntry>,
    volt_b: Option<CursorEntry>,
    /// Old format: one mode for all, positions in two pairs.
    mode: Option<CursorMode>,
    t: Option<[f64; 2]>,
    v: Option<[f64; 2]>,
}

impl From<CursorFile> for CursorSettings {
    fn from(f: CursorFile) -> Self {
        let mut c = CursorSettings {
            hidden: f.hidden,
            ..CursorSettings::default()
        };
        let entries = [f.time_a, f.time_b, f.volt_a, f.volt_b];
        c.legacy = entries.iter().all(|e| e.is_none());
        let mode = f.mode.unwrap_or_default();
        for (i, e) in entries.iter().enumerate() {
            let time = CursorSettings::is_time(i);
            let cur = c.get_mut(i);
            // Old files: positions from `t` and `v`, on from `mode`.
            if let Some(p) = if time { f.t } else { f.v } {
                cur.position = p[i % 2];
            }
            cur.enabled = if time {
                mode == CursorMode::Time
            } else {
                mode == CursorMode::Volt
            };
            if let Some(e) = e {
                cur.enabled = e.enabled.unwrap_or(cur.enabled);
                cur.position = e.position.unwrap_or(cur.position);
                cur.style = e.style.unwrap_or(cur.style);
                cur.color = e.color.unwrap_or(cur.color);
            }
        }
        c
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

/// What a trace shows of the raw samples behind each point.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TraceMode {
    /// The mean as a line.
    #[default]
    Average,
    /// The area between the lowest and the highest raw sample (over the
    /// samples behind each point and the averaged captures).
    Peak,
    /// The mean as a bright line over the peak area, muted.
    Both,
}

impl TraceMode {
    pub const ALL: [TraceMode; 3] = [TraceMode::Average, TraceMode::Peak, TraceMode::Both];

    /// The next mode (key D).
    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|m| *m == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub fn name(self) -> &'static str {
        match self {
            TraceMode::Average => "average",
            TraceMode::Peak => "peak",
            TraceMode::Both => "average + peak",
        }
    }

    /// The peak area is drawn.
    pub fn peak(self) -> bool {
        self != TraceMode::Average
    }

    /// The mean line is drawn.
    pub fn line(self) -> bool {
        self != TraceMode::Peak
    }
}

/// How a trace is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TraceStyle {
    /// Braille lines from point to point, through the low and high value
    /// of each dot column. Thicker where the signal is noisy or steep.
    /// Files from before the rename say `Classic`; the old one dot thick
    /// `Thin` style is gone and loads as this one.
    #[default]
    #[serde(alias = "Classic", alias = "Thin")]
    BrailleThin,
    /// One braille dot run per dot column, two dots wide.
    #[serde(alias = "Bold")]
    BrailleBold,
    /// No braille: one text character per cell, like an old ASCII scope.
    /// `|` for steep runs, `/` and `\` for slopes, `_`, `-` and `‾` for
    /// flat parts at the bottom, middle and top of the cell.
    Line,
    /// No braille: one connected line of box-drawing characters, one cell
    /// row per level. `─` for flat runs, `│` for steep ones, `╭ ╮ ╰ ╯`
    /// where it turns, `╵ ╷` where a run stops in a cell.
    Box,
}

impl TraceStyle {
    pub const ALL: [TraceStyle; 4] = [
        TraceStyle::BrailleThin,
        TraceStyle::BrailleBold,
        TraceStyle::Line,
        TraceStyle::Box,
    ];

    pub fn name(self) -> &'static str {
        match self {
            TraceStyle::BrailleThin => "braille thin",
            TraceStyle::BrailleBold => "braille bold",
            TraceStyle::Line => "ascii lines",
            TraceStyle::Box => "box lines",
        }
    }
}

/// How a cursor line is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CursorStyle {
    /// Dashes of a fixed length, name inside the plot.
    #[default]
    Classic,
    /// Short dashes on every cell, name in the margin.
    Fine,
    /// A solid line, name inside the plot.
    Solid,
    /// One dot per cell (every second cell across), name inside the plot.
    Dotted,
}

impl CursorStyle {
    pub const ALL: [CursorStyle; 4] = [
        CursorStyle::Classic,
        CursorStyle::Fine,
        CursorStyle::Solid,
        CursorStyle::Dotted,
    ];

    pub fn name(self) -> &'static str {
        match self {
            CursorStyle::Classic => "classic",
            CursorStyle::Fine => "fine dashes",
            CursorStyle::Solid => "solid",
            CursorStyle::Dotted => "dotted",
        }
    }
}

/// How the division lines of the grid, or the center cross (the axes),
/// are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum GridStyle {
    /// `·` dots.
    #[default]
    Dots,
    /// `╌` and `╎` dashes.
    Dashed,
    /// `─` and `│` lines with `┼` where they cross.
    Solid,
    /// `+` where the division lines cross (the grid: nothing between;
    /// the axes: `·` between in full).
    Crosses,
    /// Nothing.
    None,
}

impl GridStyle {
    pub const ALL: [GridStyle; 5] = [
        GridStyle::Dots,
        GridStyle::Dashed,
        GridStyle::Solid,
        GridStyle::Crosses,
        GridStyle::None,
    ];

    pub fn name(self) -> &'static str {
        match self {
            GridStyle::Dots => "dots",
            GridStyle::Dashed => "dashed",
            GridStyle::Solid => "solid",
            GridStyle::Crosses => "crosses",
            GridStyle::None => "none",
        }
    }
}

/// Where the grid (or the center cross) draws its lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum GridExtent {
    /// Marks only where the division lines cross (the axes: also the
    /// ticks at 1/5 division).
    #[default]
    Points,
    /// Every division line across the whole plot.
    Full,
}

impl GridExtent {
    pub const ALL: [GridExtent; 2] = [GridExtent::Points, GridExtent::Full];

    pub fn name(self) -> &'static str {
        match self {
            GridExtent::Points => "points",
            GridExtent::Full => "full",
        }
    }
}

/// Grid brightness steps, in percent.
pub const GRID_BRIGHTNESS: [u8; 10] = [10, 20, 30, 40, 50, 60, 70, 80, 90, 100];
/// The grid brightness of the first versions.
pub const GRID_BRIGHTNESS_DEFAULT: u8 = 30;
/// The center cross of the first versions: dashed whole lines, as bright
/// as [`GRID_BRIGHTNESS_DEFAULT`] makes it.
pub const AXIS_STYLE_DEFAULT: GridStyle = GridStyle::Dashed;
pub const AXIS_EXTENT_DEFAULT: GridExtent = GridExtent::Full;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplaySettings {
    /// Show the spectrum below the time view.
    pub spectrum: bool,
    /// Spectrum span is the Nyquist frequency divided by 2^zoom.
    pub spectrum_zoom: u32,
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
    /// Colours: auto, true colour, 256 or 16 colours.
    pub colors: ColorMode,
    /// How CH1, CH2 and MATH are drawn (also in the spectrum).
    pub trace_style: [TraceStyle; 3],
    /// Mean line, peak area or both (scope and log view).
    pub trace_mode: TraceMode,
    /// The one cursor style of old files. It only loads: it becomes the
    /// style of each cursor (see [`Settings::sanitize`]).
    #[serde(skip_serializing)]
    pub cursor_style: Option<CursorStyle>,
    pub grid_style: GridStyle,
    pub grid_extent: GridExtent,
    /// Grid brightness in percent, one of [`GRID_BRIGHTNESS`].
    pub grid_brightness: u8,
    /// The center cross, on its own: drawn over the grid, under the
    /// traces.
    pub axis_style: GridStyle,
    pub axis_extent: GridExtent,
    /// Axis brightness in percent, one of [`GRID_BRIGHTNESS`].
    pub axis_brightness: u8,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            spectrum: false,
            spectrum_zoom: 0,
            window: Window::Hann,
            xy: false,
            interpolation: Interpolation::Sinc,
            averaging: 1,
            persistence: 0,
            dots: false,
            measurements: true,
            colors: ColorMode::Auto,
            trace_style: [TraceStyle::BrailleThin; 3],
            trace_mode: TraceMode::Average,
            cursor_style: None,
            grid_style: GridStyle::Dots,
            grid_extent: GridExtent::Points,
            grid_brightness: GRID_BRIGHTNESS_DEFAULT,
            axis_style: AXIS_STYLE_DEFAULT,
            axis_extent: AXIS_EXTENT_DEFAULT,
            axis_brightness: GRID_BRIGHTNESS_DEFAULT,
        }
    }
}

/// Averaging steps for the `A` key.
pub const AVERAGING: [usize; 7] = [1, 2, 4, 8, 16, 32, 64];
/// Persistence steps for the `P` key.
pub const PERSISTENCE: [usize; 5] = [0, 2, 4, 8, 16];

/// Spectrum zoom steps: up to 1/256 of the Nyquist frequency.
pub const MAX_SPECTRUM_ZOOM: u32 = 8;

/// Frequencies of the calibration output pin, in Hz (the steps OpenHantek6022 offers).
pub const CAL_FREQS: [u32; 38] = [
    32, 40, 50, 60, 80, 100, 120, 160, 200, 250, 300, 400, 440, 500, 600, 660, 800, 1_000, 1_200,
    1_600, 2_000, 2_500, 3_000, 4_000, 5_000, 6_000, 8_000, 10_000, 12_000, 16_000, 20_000, 25_000,
    30_000, 40_000, 50_000, 60_000, 80_000, 100_000,
];

/// Time labels of the log view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TimeAxis {
    /// Wall-clock time (local time zone is not known: UTC), HH:MM:SS.
    #[default]
    Clock,
    /// Seconds since the start of the log.
    Seconds,
}

impl TimeAxis {
    pub const ALL: [TimeAxis; 2] = [TimeAxis::Clock, TimeAxis::Seconds];

    pub fn name(self) -> &'static str {
        match self {
            TimeAxis::Clock => "clock (UTC)",
            TimeAxis::Seconds => "seconds since start",
        }
    }
}

/// Time column of a log CSV export.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CsvTime {
    /// ISO-8601 UTC timestamp, like `2026-11-07T12:34:56.789Z`.
    #[default]
    Iso,
    /// Seconds since the start of the log.
    Seconds,
}

impl CsvTime {
    pub const ALL: [CsvTime; 2] = [CsvTime::Iso, CsvTime::Seconds];

    pub fn name(self) -> &'static str {
        match self {
            CsvTime::Iso => "ISO 8601 UTC",
            CsvTime::Seconds => "seconds since start",
        }
    }
}

/// Acquisition settings (the `[acquisition]` table).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AcquisitionSettings {
    /// Highest ADC rate in S/s with both channels sampled (one of the
    /// [`crate::device::protocol::ADC_CAPS`] choices; CH1 alone gets its
    /// pair). Caps the ADC in every mode: blocks, roll and the logger.
    pub max_adc_rate: f64,
}

impl Default for AcquisitionSettings {
    fn default() -> Self {
        Self {
            max_adc_rate: crate::device::protocol::MAX_ADC_DEFAULT,
        }
    }
}

impl AcquisitionSettings {
    /// Index of the setting in [`crate::device::protocol::ADC_CAPS`].
    pub fn cap_idx(&self) -> usize {
        crate::device::protocol::cap_index(self.max_adc_rate)
    }
}

/// Envelope logger settings (the `[logger]` table).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggerSettings {
    /// Time step of the log, in seconds (one of [`crate::logger::STEPS`]),
    /// or 0 for raw ([`crate::logger::RAW`]: one ADC sample per step).
    pub step: f64,
    /// Stop the log when the file reaches this many bytes; 0 = no limit.
    pub log_limit: u64,
    /// Folder for log files. Empty: `~/.local/share/openhantek-tui/logs`.
    pub log_dir: Option<std::path::PathBuf>,
    pub time_axis: TimeAxis,
    pub csv_time: CsvTime,
    /// Mean line, min..max area or both, in the log view.
    pub trace_mode: TraceMode,
}

impl Default for LoggerSettings {
    fn default() -> Self {
        Self {
            step: crate::logger::STEP_DEFAULT,
            log_limit: crate::logger::LIMIT_DEFAULT,
            log_dir: None,
            time_axis: TimeAxis::Clock,
            csv_time: CsvTime::Iso,
            trace_mode: TraceMode::Both,
        }
    }
}

impl LoggerSettings {
    /// Index of the step in [`crate::logger::STEPS`] (the nearest; raw
    /// gives 0).
    pub fn step_idx(&self) -> usize {
        crate::logger::nearest(&crate::logger::STEPS, self.step)
    }

    /// Index of the limit in [`crate::logger::LIMITS`].
    pub fn limit_idx(&self) -> usize {
        let l = &crate::logger::LIMITS;
        if self.log_limit == 0 {
            return l.len() - 1;
        }
        let v: Vec<f64> = l[..l.len() - 1].iter().map(|x| *x as f64).collect();
        crate::logger::nearest(&v, self.log_limit as f64)
    }

    pub fn dir(&self) -> std::path::PathBuf {
        self.log_dir
            .clone()
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or_else(crate::logger::default_log_dir)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub channels: [ChannelSettings; 2],
    pub math: MathSettings,
    /// Index into [`TIME_DIVS`].
    pub time_idx: usize,
    /// Fine time/div (Alt-← →): a factor on the step `time_idx`, between
    /// the step below and this one; 1 = the step. The sample rate is the
    /// one of the step, so enough samples cover the screen.
    pub time_fine: f64,
    pub trigger: TriggerSettings,
    pub cursors: CursorSettings,
    pub display: DisplaySettings,
    /// Calibration output frequency, in Hz.
    pub cal_freq: u32,
    /// Selected trace: 0 = CH1, 1 = CH2, 2 = MATH.
    pub selected: usize,
    pub acquisition: AcquisitionSettings,
    pub logger: LoggerSettings,
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
            time_fine: 1.0,
            trigger: TriggerSettings::default(),
            cursors: CursorSettings::default(),
            display: DisplaySettings::default(),
            cal_freq: 1_000,
            selected: 0,
            acquisition: AcquisitionSettings::default(),
            logger: LoggerSettings::default(),
        }
    }
}

impl Settings {
    /// Fix values that a hand-edited file may have out of range.
    pub fn sanitize(&mut self) {
        let half_v = V_DIVS / 2.0;
        let fine = |steps: &[f64], idx: &mut usize, f: &mut f64| {
            let i = (*idx).min(steps.len() - 1);
            let v = steps[i] * if f.is_finite() && *f > 0.0 { *f } else { 1.0 };
            (*idx, *f) = fine_step(steps, v);
        };
        for ch in &mut self.channels {
            fine(&VOLT_DIVS, &mut ch.volt_idx, &mut ch.volts_fine);
            ch.offset = finite_or(ch.offset, 0.0).clamp(-half_v, half_v);
            if ch.probe != 1 && ch.probe != 10 {
                ch.probe = 1;
            }
        }
        fine(
            &VOLT_DIVS,
            &mut self.math.volt_idx,
            &mut self.math.units_fine,
        );
        self.math.offset = finite_or(self.math.offset, 0.0).clamp(-half_v, half_v);
        fine(&TIME_DIVS, &mut self.time_idx, &mut self.time_fine);
        self.trigger.source = self.trigger.source.min(1);
        self.trigger.level = finite_or(self.trigger.level, 0.0);
        self.trigger.position = finite_or(self.trigger.position, 0.5).clamp(0.0, 1.0);
        let half_h = H_DIVS / 2.0;
        // An old file: its one cursor style is now the style of each cursor.
        if let Some(st) = self.display.cursor_style.take() {
            if self.cursors.legacy {
                for i in 0..4 {
                    self.cursors.get_mut(i).style = st;
                }
            }
        }
        self.cursors.legacy = false;
        for i in 0..4 {
            let half = if CursorSettings::is_time(i) {
                half_h
            } else {
                half_v
            };
            let c = self.cursors.get_mut(i);
            c.position = finite_or(c.position, 0.0).clamp(-half, half);
        }
        self.display.averaging = self.display.averaging.clamp(1, 128);
        self.display.persistence = self.display.persistence.min(16);
        self.display.spectrum_zoom = self.display.spectrum_zoom.min(MAX_SPECTRUM_ZOOM);
        // The nearest step, so a hand-typed 35 still works.
        let near = |b: u8| {
            *GRID_BRIGHTNESS
                .iter()
                .min_by_key(|g| g.abs_diff(b))
                .unwrap_or(&GRID_BRIGHTNESS_DEFAULT)
        };
        self.display.grid_brightness = near(self.display.grid_brightness);
        self.display.axis_brightness = near(self.display.axis_brightness);
        if !CAL_FREQS.contains(&self.cal_freq) {
            self.cal_freq = 1_000;
        }
        self.selected = self.selected.min(2);
        let lg = &mut self.logger;
        // Raw is 0 (a negative value too); else the nearest step, so a
        // hand-typed 35 still works.
        lg.step = if lg.step.is_nan() {
            crate::logger::STEP_DEFAULT
        } else if crate::logger::is_raw(lg.step) {
            crate::logger::RAW
        } else {
            crate::logger::STEPS[lg.step_idx()]
        };
        let caps = &crate::device::protocol::ADC_CAPS;
        self.acquisition.max_adc_rate =
            crate::device::protocol::raw_rate_of_id(caps[self.acquisition.cap_idx()].0);
        lg.log_limit = crate::logger::LIMITS[lg.limit_idx()];
    }

    /// Time per division, with the fine factor.
    pub fn time_per_div(&self) -> f64 {
        self.coarse_time_per_div() * self.time_fine
    }

    /// The time/div step (`time_idx`), without the fine factor: the
    /// sample rate and roll mode are chosen for it.
    pub fn coarse_time_per_div(&self) -> f64 {
        TIME_DIVS[self.time_idx.min(TIME_DIVS.len() - 1)]
    }

    /// Time across the whole screen, in seconds.
    pub fn screen_time(&self) -> f64 {
        self.time_per_div() * H_DIVS
    }

    /// Screen time of the time/div step: the sample rate covers it.
    pub fn coarse_screen_time(&self) -> f64 {
        self.coarse_time_per_div() * H_DIVS
    }

    /// True when the screen scrolls (slow time bases).
    pub fn roll(&self) -> bool {
        self.coarse_time_per_div() >= ROLL_FROM * 0.999
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
        s.display.grid_brightness = 255;
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
        assert_eq!(s.display.grid_brightness, 100);
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
        s.display.trace_style = [TraceStyle::BrailleBold, TraceStyle::Box, TraceStyle::Line];
        s.cursors.time_b.style = CursorStyle::Fine;
        s.cursors.volt_a.color = CursorColor::Cyan;
        s.cursors.volt_a.enabled = true;
        s.cursors.hidden = true;
        s.display.grid_style = GridStyle::Solid;
        s.display.grid_extent = GridExtent::Full;
        s.display.grid_brightness = 70;
        let text = toml::to_string_pretty(&s).unwrap();
        let back: Settings = toml::from_str(&text).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn old_files_get_the_classic_styles() {
        // A file from before the style settings: no trace_style, no
        // cursor_style.
        let text = "[display]\ndots = true\n";
        let s: Settings = toml::from_str(text).unwrap();
        assert!(s.display.dots);
        assert_eq!(s.display.trace_style, [TraceStyle::BrailleThin; 3]);
        assert_eq!(s.cursors.time_a.style, CursorStyle::Classic);
        // And the grid of the first versions.
        assert_eq!(s.display.grid_style, GridStyle::Dots);
        assert_eq!(s.display.grid_extent, GridExtent::Points);
        assert_eq!(s.display.grid_brightness, 30);
        // Each style is saved by name.
        let mut s = Settings::default();
        s.display.trace_style[1] = TraceStyle::BrailleBold;
        s.display.trace_style[2] = TraceStyle::Box;
        let text = toml::to_string_pretty(&s).unwrap();
        let text: String = text.split_whitespace().collect();
        assert!(
            text.contains(r#"trace_style=["BrailleThin","BrailleBold","Box",]"#),
            "{text}"
        );
    }

    #[test]
    fn old_cursor_settings_load() {
        let old = |mode: &str| {
            let text = format!(
                "[cursors]\nmode = \"{mode}\"\nt = [-3.0, 1.5]\nv = [0.5, 2.0]\n\n\
                 [display]\ncursor_style = \"Fine\"\n"
            );
            let mut s: Settings = toml::from_str(&text).unwrap();
            s.sanitize();
            s
        };
        let s = old("Time");
        let c = &s.cursors;
        assert!(c.time_a.enabled && c.time_b.enabled);
        assert!(!c.volt_a.enabled && !c.volt_b.enabled);
        assert_eq!((c.time_a.position, c.time_b.position), (-3.0, 1.5));
        assert_eq!((c.volt_a.position, c.volt_b.position), (0.5, 2.0));
        // The old global style is the style of each cursor.
        assert!((0..4).all(|i| c.get(i).style == CursorStyle::Fine));
        assert!((0..4).all(|i| c.get(i).color == CursorColor::Grey));
        assert_eq!(s.display.cursor_style, None);
        let c = old("Volt").cursors;
        assert!(!c.time_a.enabled && c.volt_a.enabled && c.volt_b.enabled);
        let c = old("Off").cursors;
        assert!((0..4).all(|i| !c.get(i).enabled));
        // The new format: each cursor on its own, missing fields default.
        let text = "[cursors]\n[cursors.volt_b]\nenabled = true\nstyle = \"Dotted\"\n";
        let mut s: Settings = toml::from_str(text).unwrap();
        s.sanitize();
        assert!(s.cursors.volt_b.enabled);
        assert_eq!(s.cursors.volt_b.style, CursorStyle::Dotted);
        assert_eq!(s.cursors.volt_b.position, 1.0);
        assert!(!s.cursors.time_a.enabled);
        // The old cursor_style is never written.
        let text = toml::to_string_pretty(&s).unwrap();
        assert!(!text.contains("cursor_style"), "{text}");
        assert!(
            text.contains("[cursors.volt_b]") && !text.contains("\nt = "),
            "{text}"
        );
    }

    #[test]
    fn fine_steps_between_the_1_2_5_steps() {
        // 1 V up: a tenth of the gap to 2 V.
        assert_eq!(fine_move(&VOLT_DIVS, 1.0, true), (6, 0.55));
        // 1.1 V down: back on 1 V.
        assert_eq!(fine_move(&VOLT_DIVS, 1.1, false), (5, 1.0));
        // 2 V down: a tenth of the gap to 1 V.
        let (i, f) = fine_move(&VOLT_DIVS, 2.0, false);
        assert_eq!(i, 6);
        assert!((VOLT_DIVS[i] * f - 1.9).abs() < 1e-12);
        // At the ends it stays.
        assert_eq!(fine_move(&VOLT_DIVS, 10.0, true), (8, 1.0));
        assert_eq!(fine_move(&VOLT_DIVS, 0.02, false), (0, 1.0));
        // Time: 1 ms up gives 1.1 ms on the 2 ms step.
        let (i, f) = fine_move(&TIME_DIVS, 1e-3, true);
        assert_eq!(TIME_DIVS[i], 2e-3);
        assert!((TIME_DIVS[i] * f - 1.1e-3).abs() < 1e-15);
        // Normal steps snap: 1.35 (on the 2 step) goes to 1 down, 2 up.
        assert_eq!(coarse_move(9, 6, 0.675, false), 5);
        assert_eq!(coarse_move(9, 6, 0.675, true), 6);
        assert_eq!(coarse_move(9, 6, 1.0, true), 7);
        assert_eq!(round_sig(1.34999, 3), 1.35);
    }

    #[test]
    fn fine_values_round_trip_and_old_files_load() {
        let mut s = Settings::default();
        s.channels[0].volt_idx = 6;
        s.channels[0].volts_fine = 0.675;
        s.math.units_fine = 0.8;
        s.time_idx = 15;
        s.time_fine = 0.55;
        s.sanitize();
        assert!((s.channels[0].volts_per_div() - 1.35).abs() < 1e-12);
        assert!((s.time_per_div() - 1.1e-3).abs() < 1e-15);
        assert_eq!(s.coarse_time_per_div(), 2e-3);
        let back: Settings = toml::from_str(&toml::to_string_pretty(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        // Old files have no fine factors: the steps.
        let s: Settings =
            toml::from_str("time_idx = 3\n[[channels]]\nvolt_idx = 2\n[[channels]]\n").unwrap();
        assert_eq!((s.time_fine, s.channels[0].volts_fine), (1.0, 1.0));
        // A factor that is out of its step is moved to the right step.
        let mut s = Settings::default();
        s.channels[1].volt_idx = 6; // 2 V
        s.channels[1].volts_fine = 0.25; // 0.5 V
        s.sanitize();
        assert_eq!((s.channels[1].volt_idx, s.channels[1].volts_fine), (4, 1.0));
    }

    #[test]
    fn logger_settings_load_and_fix() {
        // Old files have no [logger] table.
        let s: Settings = toml::from_str("selected = 1\n").unwrap();
        assert_eq!(s.logger, LoggerSettings::default());
        let text = "[logger]\nstep = 0.0011\nlog_limit = 1300000000\ncsv_time = \"Seconds\"\n";
        let mut s: Settings = toml::from_str(text).unwrap();
        s.sanitize();
        assert_eq!(s.logger.step, 1e-3);
        assert_eq!(s.logger.log_limit, 1_000_000_000);
        assert_eq!(s.logger.csv_time, CsvTime::Seconds);
        s.logger.log_limit = 0;
        s.sanitize();
        assert_eq!(s.logger.log_limit, 0);
        let back: Settings = toml::from_str(&toml::to_string_pretty(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn old_style_names_still_load() {
        // Before the rename: Classic was the default, Thin a one dot
        // line (now gone), Bold the two dot line.
        let text = "[display]\ntrace_style = [\"Classic\", \"Thin\", \"Bold\"]\n";
        let s: Settings = toml::from_str(text).unwrap();
        assert_eq!(
            s.display.trace_style,
            [
                TraceStyle::BrailleThin,
                TraceStyle::BrailleThin,
                TraceStyle::BrailleBold
            ]
        );
    }
}
