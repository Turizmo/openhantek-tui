//! Settings panel, with yazi-style keys: three columns (categories,
//! settings, values), h/j/k/l to move, `/` to search. Number settings
//! show their value in the value column: j/k step it, digits type it.
//!
//! This is the model only: no drawing and no terminal. The drawing is in
//! `ui.rs`.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::keys::{Act, KeyMap, View};

use crate::dsp::fft::Window;
use crate::dsp::trigger::Slope;
use crate::settings::*;
use crate::si::fmt_si;
use crate::theme::ColorMode;

/// Things a panel item can do that are not settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    RunStop,
    Single,
    Autoset,
    TriggerToMiddle,
    ExportCsv,
    ExportJson,
    Reset,
    /// Start / stop the envelope logger.
    Record,
    /// Log view on / off.
    LogView,
    /// Export the open log as CSV.
    ExportLog,
    /// Export the open log between the cursors.
    ExportLogRange,
    /// Open recent log number `i` (newest first).
    OpenLog(usize),
    /// Ask to delete recent log number `i` (newest first).
    DeleteLog(usize),
    /// Zero calibration (asks for grounded inputs first).
    ZeroCal,
    /// Forget the app's zero values: the EEPROM values again.
    ClearZeroCal,
    /// The probe helper on the calibration output.
    ProbeCal,
    /// Forget the gain corrections of both channels.
    ClearGainCal,
}

/// One setting in the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    ChEnabled(usize),
    ChVolts(usize),
    ChOffset(usize),
    ChCoupling(usize),
    ChProbe(usize),
    ChInvert(usize),
    /// Trace style of CH1, CH2 or MATH.
    Style(usize),
    MathEnabled,
    MathMode,
    MathScale,
    MathOffset,
    TrigMode,
    TrigSource,
    TrigSlope,
    TrigLevel,
    TrigPosition,
    TimeDiv,
    Roll,
    /// The max ADC rate setting.
    MaxAdc,
    Selected,
    /// Average / peak / both.
    TraceMode,
    Interpolation,
    Averaging,
    Persistence,
    Xy,
    Colors,
    GridLook,
    GridExtent,
    GridBrightness,
    AxisLook,
    AxisExtent,
    AxisBrightness,
    SpecOn,
    SpecWindow,
    SpecZoom,
    /// Cursor `i` (time A, time B, voltage A, voltage B): its position,
    /// and Space turns it on or off.
    CursorPos(usize),
    /// Line style of cursor `i`.
    CursorLook(usize),
    /// Colour of cursor `i`.
    CursorColour(usize),
    MeasPanel,
    CalFreq,
    /// Where the zero values come from.
    CalZero,
    /// The gain corrections.
    CalGain,
    DevModel,
    DevSerial,
    DevFirmware,
    DevState,
    DevCalibration,
    DevRate,
    ExportDir,
    SettingsFile,
    ConfigWarnings,
    LogStep,
    LogLimit,
    LogDir,
    LogFree,
    LogAxis,
    LogCsvTime,
    /// Trace mode of the log view.
    LogTraceMode,
    /// Pick one of the recent logs to open.
    LogOpen,
    Act(Action),
}

/// What kind of value a field has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// One of a list of values.
    Choice,
    /// On or off.
    Bool,
    /// A number you can type.
    Number,
    /// Read-only text.
    Info,
    /// Runs something.
    Action,
}

/// Read-only facts the panel shows, from the app.
#[derive(Clone, Debug, Default)]
pub struct DeviceInfo {
    /// Like `DSO-6022BE 0694CE656429 FW0210`.
    pub name: String,
    pub connected: bool,
    /// Notes from connecting, like where the calibration came from.
    pub notes: Vec<String>,
    /// Output sample rate, when known.
    pub rate: Option<f64>,
    /// ADC sample rate, when known (the output rate decimates it).
    pub adc_rate: Option<f64>,
    pub running: bool,
    pub export_dir: String,
    pub settings_file: String,
    /// Problems found in the config file.
    pub config_warnings: Vec<String>,
    /// A log is being recorded.
    pub recording: bool,
    pub log_dir: String,
    /// Free bytes in the log folder.
    pub log_free: Option<u64>,
    /// File names of the recent logs, newest first.
    pub recent_logs: Vec<String>,
}

impl DeviceInfo {
    /// Model, serial and firmware from the name, when it has all three.
    fn parts(&self) -> (String, String, String) {
        let p: Vec<&str> = self.name.split_whitespace().collect();
        if p.len() == 3 && p[2].starts_with("FW") {
            (p[0].into(), p[1].into(), p[2][2..].into())
        } else {
            (self.name.clone(), "--".into(), "--".into())
        }
    }
}

#[derive(Clone, Debug)]
pub struct Category {
    pub name: &'static str,
    pub fields: Vec<Field>,
}

/// Every setting, by category.
pub fn categories() -> Vec<Category> {
    use Field::*;
    let ch = |c| {
        vec![
            ChEnabled(c),
            ChVolts(c),
            ChOffset(c),
            ChCoupling(c),
            ChProbe(c),
            ChInvert(c),
            Style(c),
        ]
    };
    vec![
        Category {
            name: "Channel 1",
            fields: ch(0),
        },
        Category {
            name: "Channel 2",
            fields: ch(1),
        },
        Category {
            name: "Math",
            fields: vec![MathEnabled, MathMode, MathScale, MathOffset, Style(2)],
        },
        Category {
            name: "Trigger",
            fields: vec![
                TrigMode,
                TrigSource,
                TrigSlope,
                TrigLevel,
                TrigPosition,
                Act(Action::TriggerToMiddle),
                Act(Action::Single),
            ],
        },
        Category {
            name: "Acquisition",
            fields: vec![
                TimeDiv,
                Roll,
                MaxAdc,
                Act(Action::RunStop),
                Act(Action::Autoset),
            ],
        },
        Category {
            name: "Display",
            fields: vec![
                TraceMode,
                Interpolation,
                Averaging,
                Persistence,
                Xy,
                Colors,
                GridLook,
                GridExtent,
                GridBrightness,
                AxisLook,
                AxisExtent,
                AxisBrightness,
            ],
        },
        Category {
            name: "Spectrum",
            fields: vec![SpecOn, SpecWindow, SpecZoom],
        },
        Category {
            name: "Cursors",
            fields: (0..4)
                .flat_map(|i| [CursorPos(i), CursorLook(i), CursorColour(i)])
                .collect(),
        },
        Category {
            name: "Measurements",
            fields: vec![MeasPanel, Selected],
        },
        Category {
            name: "Calibration",
            fields: vec![
                CalFreq,
                CalZero,
                Act(Action::ZeroCal),
                Act(Action::ClearZeroCal),
                CalGain,
                Act(Action::ProbeCal),
                Act(Action::ClearGainCal),
            ],
        },
        Category {
            name: "Device",
            fields: vec![
                DevModel,
                DevSerial,
                DevFirmware,
                DevState,
                DevCalibration,
                DevRate,
            ],
        },
        Category {
            name: "Files",
            fields: vec![
                ExportDir,
                SettingsFile,
                ConfigWarnings,
                Act(Action::ExportCsv),
                Act(Action::ExportJson),
                LogOpen,
                Act(Action::Reset),
            ],
        },
        Category {
            name: "Logger",
            fields: vec![
                Act(Action::Record),
                LogStep,
                LogLimit,
                LogFree,
                LogDir,
                LogAxis,
                LogCsvTime,
                LogTraceMode,
                Act(Action::LogView),
                LogOpen,
                Act(Action::ExportLog),
                Act(Action::ExportLogRange),
            ],
        },
    ]
}

const OFF_ON: [&str; 2] = ["off", "on"];

/// The end of a long text, like `…/openhantek-tui/logs`.
fn tail(text: &str, max: usize) -> String {
    let n = text.chars().count();
    if n <= max {
        return text.to_string();
    }
    let end: String = text.chars().skip(n + 1 - max).collect();
    format!("…{end}")
}

/// Channels a log would record now (CH1 when none is on, for the
/// estimates).
fn logged_channels(s: &Settings) -> [bool; 2] {
    match [s.channels[0].enabled, s.channels[1].enabled] {
        [false, false] => [true, false],
        on => on,
    }
}

/// A size limit with how long it lasts: `1 GB ≈ 11 h 34 min`.
fn limit_text(l: u64, s: &Settings) -> String {
    use crate::logger as lg;
    if l == 0 {
        return "off".into();
    }
    let max = s.acquisition.max_adc_rate;
    let secs = l as f64 / lg::bytes_per_second(s.logger.step, logged_channels(s), max);
    format!("{} ≈ {}", lg::fmt_bytes(l), lg::fmt_span(secs))
}
/// Help line of the raw log step: its limits in plain words, the most
/// important first (the line is cut at the panel width; `?` and the
/// README have all of it).
pub fn raw_step_help(adc: f64, bytes_per_second: f64, limit: u64, logged: [bool; 2]) -> String {
    use crate::logger as lg;
    let lasts = if limit > 0 {
        format!(
            ", so {} lasts ≈ {}",
            lg::fmt_bytes(limit),
            lg::fmt_span(limit as f64 / bytes_per_second)
        )
    } else {
        String::new()
    };
    let ch2 = if logged == [false, true] {
        " · CH2 alone still uses 2-channel mode (16 MS/s at most)"
    } else {
        " · CH2 alone uses 2-channel mode"
    };
    format!(
        "raw: one 8-bit sample per step at {}, no min/max per step · the file grows {}/s{lasts} · the disk must keep up · analog bandwidth ≈ 20 MHz: finer detail is not real{ch2} · Max ADC rate lowers the rate · ? for more",
        fmt_si(adc, "S/s"),
        lg::fmt_bytes(bytes_per_second as u64),
    )
}

const NAMES: [&str; 3] = ["CH1", "CH2", "MATH"];

fn slope_name(s: Slope) -> &'static str {
    match s {
        Slope::Rising => "rising ↑",
        Slope::Falling => "falling ↓",
        Slope::Both => "both ↕",
    }
}

fn index_of<T: PartialEq>(list: &[T], v: &T) -> usize {
    list.iter().position(|x| x == v).unwrap_or(0)
}

impl Field {
    pub fn label(self) -> &'static str {
        use Field::*;
        match self {
            ChEnabled(_) | MathEnabled => "On",
            ChVolts(_) => "Volts/div",
            ChOffset(_) | MathOffset => "Position",
            ChCoupling(_) => "Coupling",
            ChProbe(_) => "Probe",
            ChInvert(_) => "Invert",
            Style(_) => "Style",
            MathMode => "Function",
            MathScale => "Units/div",
            TrigMode => "Mode",
            TrigSource => "Source",
            TrigSlope => "Edge",
            TrigLevel => "Level",
            TrigPosition => "Position",
            TimeDiv => "Time/div",
            Roll => "Roll mode",
            MaxAdc => "Max ADC rate",
            Selected => "Selected trace",
            TraceMode => "Trace mode",
            Interpolation => "Interpolation",
            Averaging => "Averaging",
            Persistence => "Persistence",
            Xy => "XY mode",
            Colors => "Colours",
            GridLook => "Grid style",
            GridExtent => "Grid lines",
            GridBrightness => "Grid brightness",
            AxisLook => "Axis style",
            AxisExtent => "Axis lines",
            AxisBrightness => "Axis brightness",
            SpecOn => "Show spectrum",
            SpecWindow => "Window",
            SpecZoom => "Span",
            CursorPos(0) => "Time A",
            CursorPos(1) => "Time B",
            CursorPos(2) => "Voltage A",
            CursorPos(_) => "Voltage B",
            CursorLook(0) => "Time A style",
            CursorLook(1) => "Time B style",
            CursorLook(2) => "Voltage A style",
            CursorLook(_) => "Voltage B style",
            CursorColour(0) => "Time A colour",
            CursorColour(1) => "Time B colour",
            CursorColour(2) => "Voltage A colour",
            CursorColour(_) => "Voltage B colour",
            MeasPanel => "Show panel",
            CalFreq => "Output frequency",
            CalZero => "Zero",
            CalGain => "Gain correction",
            DevModel => "Model",
            DevSerial => "Serial",
            DevFirmware => "Firmware",
            DevState => "State",
            DevCalibration => "Calibration",
            DevRate => "Sample rate",
            ExportDir => "Export folder",
            SettingsFile => "Config file",
            ConfigWarnings => "Config warnings",
            LogStep => "Step",
            LogLimit => "Size limit",
            LogDir => "Log folder",
            LogFree => "Free space",
            LogAxis => "Time labels",
            LogCsvTime => "CSV time",
            LogTraceMode => "Trace mode",
            LogOpen => "Open log",
            Act(a) => match a {
                Action::RunStop => "Run / stop",
                Action::Single => "Single shot",
                Action::Autoset => "Autoset",
                Action::TriggerToMiddle => "Level to middle",
                Action::ExportCsv => "Export CSV",
                Action::ExportJson => "Export JSON",
                Action::Reset => "Reset all settings",
                Action::Record => "Record",
                Action::LogView => "Log view",
                Action::ExportLog => "Export log CSV",
                Action::ExportLogRange => "Export A–B CSV",
                Action::OpenLog(_) => "Open log",
                Action::DeleteLog(_) => "Delete log",
                Action::ZeroCal => "Zero calibration",
                Action::ClearZeroCal => "Clear zero calibration",
                Action::ProbeCal => "Probe helper",
                Action::ClearGainCal => "Clear gain correction",
            },
        }
    }

    pub fn help(self) -> &'static str {
        use Field::*;
        match self {
            ChEnabled(_) => "Show this channel. Keys 1 and 2.",
            ChVolts(_) => "Volts per division at the probe tip. Keys ↑ ↓.",
            ChOffset(_) => "Height of the 0 V line, in divisions. Type a value in V to set it in volts. Keys [ ] { } 0.",
            ChCoupling(_) => "AC takes away the DC part (in software). Key c.",
            ChProbe(_) => "Set this to match the switch on the probe. Key p.",
            ChInvert(_) => "Turn the trace upside down. Key i.",
            Style(_) => "braille thin: lines through the low and high value of each dot column. braille bold: two dots thick. ascii and box lines: no braille.",
            MathEnabled => "Show the math trace. Key 3.",
            MathMode => "How the math trace is made from CH1 and CH2. Key m.",
            MathScale => "Units per division of the math trace.",
            MathOffset => "Height of the math 0 line, in divisions.",
            TrigMode => "Auto shows an untriggered trace when no trigger came for 100 ms (or two screens); Normal waits; Single stops after one. Key t.",
            TrigSource => "Which channel the trigger looks at. Key s.",
            TrigSlope => "Which edge starts a trace. Key e.",
            TrigLevel => "Trigger level in volts at the probe tip. Keys + - T.",
            TrigPosition => "Where the trigger point is on the screen, 0 % to 100 %. Keys , .",
            TimeDiv => "Time per division. Keys ← →.",
            Roll => "Slow time bases scroll from right to left, with no trigger.",
            MaxAdc => "Highest ADC rate in every mode (scope, roll, logger). The ADC always runs at it; slower rates average its samples. Lower it if the USB link loses data.",
            TraceMode => "average: the mean of the samples behind each point, as a line. peak: the area from the lowest to the highest sample (also over the averaged captures). average + peak: the same line as average, the muted area only around it. Key D.",
            Selected => "The trace the arrow keys, cursors and spectrum use. Key Tab.",
            Interpolation => "sin(x)/x: a smooth line through few samples. linear: straight lines. samples only: one dot per sample, no lines. Key z.",
            Averaging => "Average the last N triggers (a sliding window): less noise. Key A.",
            Persistence => "Keep this many old traces on screen, dimmed. Key P.",
            Xy => "Draw CH1 against CH2. Key x.",
            Colors => "auto uses true colour when COLORTERM says so.",
            GridLook => "Look of the division lines. The center cross has its own Axis settings.",
            GridExtent => "points: marks where division lines cross. full: every division line across the whole plot.",
            GridBrightness => "How bright the grid is. Traces are always drawn over it.",
            AxisLook => "Look of the center cross (the axes through 0). none shows the grid there.",
            AxisExtent => "points: marks at the divisions and ticks. full: the whole center lines.",
            AxisBrightness => "How bright the center cross is. It is drawn over the grid, under the traces.",
            CursorLook(_) => "classic: dashes, name inside the plot. fine dashes: short dashes, name in the margin. solid, dotted: name inside.",
            CursorColour(_) => "Colour of the cursor line and its name.",
            SpecOn => "Show the spectrum (FFT) below the trace. Key f.",
            SpecWindow => "Window function for the FFT. Key w.",
            SpecZoom => "Highest frequency on the spectrum screen. Key F.",
            CursorPos(i) if i < 2 => "Space: on/off. Time from the screen center, in seconds. Keys h l H L; v hides all.",
            CursorPos(_) => "Space: on/off. Voltage of the selected trace. Keys j k J K; v hides all.",
            MeasPanel => "The measurement panel on the right. Key M.",
            CalFreq => "Frequency of the 2 Vpp square wave on the calibration pins. Keys g G.",
            CalZero => "Where the 0 V values come from: the app's zero calibration (config file) or the scope's EEPROM.",
            CalGain => "Gain corrections from the probe helper (config file), on top of the EEPROM gain.",
            DevModel | DevSerial | DevFirmware | DevState => "Read from the scope.",
            DevCalibration => "Where the calibration values came from.",
            DevRate => "Sample rate of the traces now, and the ADC rate it is made from (the mean of each few ADC samples).",
            ExportDir => "Exports go here. Set it with --export-dir.",
            SettingsFile => "Settings and keys. Changes are saved here on quit, in place. Set it with --config.",
            ConfigWarnings => "Problems in the config file; those entries use their defaults.",
            LogStep => "Time per log step: min, max and mean of each channel are saved per step.",
            LogLimit => "The log stops when the file reaches this size.",
            LogDir => "New logs go here. Set it with --log-dir or log_dir in the settings file.",
            LogFree => "Free space on the disk of the log folder.",
            LogAxis => "Time labels of the log view: wall clock (UTC) or seconds since the start. Key t.",
            LogCsvTime => "Time column of the log CSV export.",
            LogTraceMode => "Log view: average: the mean line. peak: the min..max area of each step (raw logs: the samples). average + peak: the line over the area, muted. Key D in the log view.",
            LogOpen => "Open a recent log in the log view (Enter), d deletes it (asks first). Also: --open-log FILE.",
            Act(a) => match a {
                Action::RunStop => "Start or stop the capture. Key space.",
                Action::Single => "Arm the trigger for one capture. Key o.",
                Action::Autoset => "Find good volts/div, time/div and trigger level. Key a.",
                Action::TriggerToMiddle => "Set the trigger level to the middle of the signal. Key T.",
                Action::ExportCsv => "Save the traces on screen as CSV. Key E.",
                Action::ExportJson => "Save the traces and settings as JSON. Key W.",
                Action::Reset => "All settings back to the defaults. Key R.",
                Action::Record => "Start or stop the envelope logger. Uses the volts/div, probe and channels of the scope. Key r.",
                Action::LogView => "Show the log as a strip chart. Key V.",
                Action::ExportLog => "Save the whole open log as CSV in the export folder.",
                Action::ExportLogRange => "Save the open log between cursors A and B as CSV. Key E in the log view.",
                Action::OpenLog(_) => "Open a log.",
                Action::DeleteLog(_) => "Delete a log and its .idx file.",
                Action::ZeroCal => "Ground the probe tips (or BNC inputs), then y: measures 0 V of every gain, both channels at 16 MS/s and CH1 alone at 30 MS/s. Saved in the config file, never the EEPROM. Key Z.",
                Action::ClearZeroCal => "Forget the zero calibration: the EEPROM values again.",
                Action::ProbeCal => "Probe tip on the calibration pin: sets time/div and volts/div, shows the edge overshoot and which way to turn the probe trimmer, and the amplitude against 2 V. Enter saves a gain correction, Esc puts your settings back. Key C.",
                Action::ClearGainCal => "Forget the gain corrections of both channels.",
            },
        }
    }

    pub fn kind(self) -> Kind {
        use Field::*;
        match self {
            ChEnabled(_) | ChInvert(_) | MathEnabled | Xy | SpecOn | MeasPanel => {
                Kind::Bool
            }
            ChOffset(_) | MathOffset | TrigLevel | TrigPosition | CursorPos(_) | CalFreq => {
                Kind::Number
            }
            Roll | DevModel | DevSerial | DevFirmware | DevState | DevCalibration | DevRate
            | ExportDir | SettingsFile | ConfigWarnings | LogDir | LogFree => Kind::Info,
            CalZero | CalGain => Kind::Info,
            Act(_) => Kind::Action,
            _ => Kind::Choice,
        }
    }

    /// Option names, for choices and bools.
    pub fn options(self, s: &Settings, d: &DeviceInfo) -> Vec<String> {
        use Field::*;
        let list = |v: &[&str]| v.iter().map(|x| x.to_string()).collect();
        match self {
            _ if self.kind() == Kind::Bool => list(&OFF_ON),
            ChVolts(c) => VOLT_DIVS
                .iter()
                .map(|v| format!("{}/div", fmt_si(v * s.channels[c].probe as f64, "V")))
                .collect(),
            ChCoupling(_) => list(&["DC", "AC"]),
            ChProbe(_) => list(&["x1", "x10"]),
            MathMode => crate::dsp::math::MathMode::ALL
                .iter()
                .map(|m| m.name().to_string())
                .collect(),
            MathScale => VOLT_DIVS
                .iter()
                .map(|v| format!("{}/div", fmt_si(*v, s.math.mode.unit())))
                .collect(),
            TrigMode => list(&["Auto", "Normal", "Single"]),
            TrigSource => list(&["CH1", "CH2"]),
            TrigSlope => [Slope::Rising, Slope::Falling, Slope::Both]
                .iter()
                .map(|s| slope_name(*s).to_string())
                .collect(),
            TimeDiv => TIME_DIVS
                .iter()
                .map(|t| format!("{}/div", fmt_si(*t, "s")))
                .collect(),
            Selected => list(&NAMES),
            MaxAdc => (0..crate::device::protocol::ADC_CAPS.len())
                .map(crate::device::protocol::cap_label)
                .collect(),
            TraceMode => crate::settings::TraceMode::ALL
                .iter()
                .map(|m| m.name().to_string())
                .collect(),
            Interpolation => crate::settings::Interpolation::ALL
                .iter()
                .map(|m| m.name().to_string())
                .collect(),
            Averaging => AVERAGING
                .iter()
                .map(|n| if *n == 1 { "off".into() } else { n.to_string() })
                .collect(),
            Persistence => PERSISTENCE
                .iter()
                .map(|n| {
                    if *n == 0 {
                        "off".into()
                    } else {
                        format!("{n} traces")
                    }
                })
                .collect(),
            Colors => ColorMode::ALL
                .iter()
                .map(|m| m.name().to_string())
                .collect(),
            Style(_) => crate::settings::TraceStyle::ALL
                .iter()
                .map(|m| m.name().to_string())
                .collect(),
            CursorLook(_) => crate::settings::CursorStyle::ALL
                .iter()
                .map(|m| m.name().to_string())
                .collect(),
            CursorColour(_) => CursorColor::ALL
                .iter()
                .map(|m| m.name().to_string())
                .collect(),
            GridLook | AxisLook => GridStyle::ALL
                .iter()
                .map(|m| m.name().to_string())
                .collect(),
            GridExtent | AxisExtent => crate::settings::GridExtent::ALL
                .iter()
                .map(|m| m.name().to_string())
                .collect(),
            GridBrightness | AxisBrightness => {
                GRID_BRIGHTNESS.iter().map(|b| format!("{b} %")).collect()
            }
            SpecWindow => Window::ALL.iter().map(|w| w.name().to_string()).collect(),
            SpecZoom => (0..=MAX_SPECTRUM_ZOOM)
                .map(|z| match d.rate {
                    Some(r) => format!("0 .. {}", fmt_si(r / 2.0 / 2f64.powi(z as i32), "Hz")),
                    None => format!("Nyquist / {}", 1u32 << z),
                })
                .collect(),
            LogStep => {
                let (ch, max) = (logged_channels(s), s.acquisition.max_adc_rate);
                crate::logger::step_choices(ch, max)
                    .iter()
                    .map(|t| crate::logger::step_label(*t, ch, max))
                    .collect()
            }
            LogLimit => crate::logger::LIMITS
                .iter()
                .map(|l| limit_text(*l, s))
                .collect(),
            LogAxis => TimeAxis::ALL.iter().map(|m| m.name().to_string()).collect(),
            LogCsvTime => CsvTime::ALL.iter().map(|m| m.name().to_string()).collect(),
            LogTraceMode => crate::settings::TraceMode::ALL
                .iter()
                .map(|m| m.name().to_string())
                .collect(),
            LogOpen => d.recent_logs.clone(),
            _ => Vec::new(),
        }
    }

    /// The help line text: the fixed help, or for some settings a text
    /// from the values (like the size limit estimate).
    pub fn help_text(self, s: &Settings, d: &DeviceInfo) -> String {
        use crate::logger as lg;
        let ch = logged_channels(s);
        let max = s.acquisition.max_adc_rate;
        match self {
            Field::LogLimit => {
                let l = s.logger.log_limit;
                let mut t = lg::limit_estimate(l, s.logger.step, ch, max);
                if let Some(f) = d.log_free {
                    if l == 0 || f < l {
                        t.push_str(&format!(
                            " · only {} free (≈ {})",
                            lg::fmt_bytes(f),
                            lg::fmt_span(f as f64 / lg::bytes_per_second(s.logger.step, ch, max))
                        ));
                    }
                }
                t
            }
            Field::LogStep => {
                let step = lg::fit_step(s.logger.step, ch, max);
                let (r, n) = lg::adc_for_step(lg::step_ns(step), lg::hw_channels(ch, max), max);
                let count = ch.iter().filter(|&&c| c).count();
                let bps = lg::bytes_per_second(step, ch, max);
                if lg::is_raw(step) {
                    return raw_step_help(r.raw, bps, s.logger.log_limit, ch);
                }
                format!(
                    "{} ADC at {}, {} sample{} per step, {}/h for {} channel{}.",
                    self.help(),
                    fmt_si(r.raw, "S/s"),
                    n,
                    if n == 1 { "" } else { "s" },
                    lg::fmt_bytes((bps * 3600.0) as u64),
                    count,
                    if count == 1 { "" } else { "s" }
                )
            }
            Field::LogDir => format!("{} · {}", d.log_dir, self.help()),
            Field::ConfigWarnings if !d.config_warnings.is_empty() => d.config_warnings.join(" · "),
            _ => self.help().to_string(),
        }
    }

    /// Index of the current value in [`Field::options`].
    pub fn index(self, s: &Settings) -> usize {
        use Field::*;
        let b = |v: bool| v as usize;
        match self {
            ChEnabled(c) => b(s.channels[c].enabled),
            ChInvert(c) => b(s.channels[c].invert),
            MathEnabled => b(s.math.enabled),
            Xy => b(s.display.xy),
            SpecOn => b(s.display.spectrum),
            MeasPanel => b(s.display.measurements),
            ChVolts(c) => s.channels[c].volt_idx,
            ChCoupling(c) => b(s.channels[c].coupling == Coupling::Ac),
            ChProbe(c) => b(s.channels[c].probe == 10),
            MathMode => index_of(&crate::dsp::math::MathMode::ALL, &s.math.mode),
            MathScale => s.math.volt_idx,
            TrigMode => index_of(
                &[TriggerMode::Auto, TriggerMode::Normal, TriggerMode::Single],
                &s.trigger.mode,
            ),
            TrigSource => s.trigger.source,
            TrigSlope => index_of(
                &[Slope::Rising, Slope::Falling, Slope::Both],
                &s.trigger.slope,
            ),
            TimeDiv => s.time_idx,
            Selected => s.selected,
            MaxAdc => s.acquisition.cap_idx(),
            TraceMode => index_of(&crate::settings::TraceMode::ALL, &s.display.trace_mode),
            Interpolation => index_of(
                &crate::settings::Interpolation::ALL,
                &s.display.interpolation,
            ),
            Averaging => index_of(&AVERAGING, &s.display.averaging),
            Persistence => index_of(&PERSISTENCE, &s.display.persistence),
            Colors => index_of(&ColorMode::ALL, &s.display.colors),
            Style(t) => index_of(&crate::settings::TraceStyle::ALL, &s.display.trace_style[t]),
            CursorLook(i) => index_of(&crate::settings::CursorStyle::ALL, &s.cursors.get(i).style),
            CursorColour(i) => index_of(&CursorColor::ALL, &s.cursors.get(i).color),
            GridLook => index_of(&GridStyle::ALL, &s.display.grid_style),
            GridExtent => index_of(&crate::settings::GridExtent::ALL, &s.display.grid_extent),
            GridBrightness => index_of(&GRID_BRIGHTNESS, &s.display.grid_brightness),
            AxisLook => index_of(&GridStyle::ALL, &s.display.axis_style),
            AxisExtent => index_of(&crate::settings::GridExtent::ALL, &s.display.axis_extent),
            AxisBrightness => index_of(&GRID_BRIGHTNESS, &s.display.axis_brightness),
            SpecWindow => index_of(&Window::ALL, &s.display.window),
            SpecZoom => s.display.spectrum_zoom as usize,
            LogStep => {
                let ch = logged_channels(s);
                let max = s.acquisition.max_adc_rate;
                let step = crate::logger::fit_step(s.logger.step, ch, max);
                let all = crate::logger::step_choices(ch, max);
                if crate::logger::is_raw(step) {
                    0
                } else {
                    1 + crate::logger::nearest(&all[1..], step)
                }
            }
            LogLimit => s.logger.limit_idx(),
            LogAxis => index_of(&TimeAxis::ALL, &s.logger.time_axis),
            LogCsvTime => index_of(&CsvTime::ALL, &s.logger.csv_time),
            LogTraceMode => index_of(&crate::settings::TraceMode::ALL, &s.logger.trace_mode),
            _ => 0,
        }
    }

    /// Set option `i` (from [`Field::options`]).
    pub fn set_index(self, s: &mut Settings, i: usize) {
        use Field::*;
        let on = i == 1;
        match self {
            ChEnabled(c) => s.channels[c].enabled = on,
            ChInvert(c) => s.channels[c].invert = on,
            MathEnabled => s.math.enabled = on,
            Xy => s.display.xy = on,
            SpecOn => s.display.spectrum = on,
            MeasPanel => s.display.measurements = on,
            ChVolts(c) => {
                s.channels[c].volt_idx = i.min(VOLT_DIVS.len() - 1);
                s.channels[c].volts_fine = 1.0;
            }
            ChCoupling(c) => s.channels[c].coupling = if on { Coupling::Ac } else { Coupling::Dc },
            ChProbe(c) => {
                let is_src = s.trigger_source() == c;
                let ch = &mut s.channels[c];
                let old = ch.probe as f64;
                ch.probe = if on { 10 } else { 1 };
                // Keep the trigger level where it is on the screen.
                if is_src {
                    s.trigger.level *= ch.probe as f64 / old;
                }
            }
            MathMode => {
                let all = crate::dsp::math::MathMode::ALL;
                s.math.mode = all[i.min(all.len() - 1)]
            }
            MathScale => {
                s.math.volt_idx = i.min(VOLT_DIVS.len() - 1);
                s.math.units_fine = 1.0;
            }
            TrigMode => {
                s.trigger.mode =
                    [TriggerMode::Auto, TriggerMode::Normal, TriggerMode::Single][i.min(2)]
            }
            TrigSource => s.trigger.source = i.min(1),
            TrigSlope => s.trigger.slope = [Slope::Rising, Slope::Falling, Slope::Both][i.min(2)],
            TimeDiv => {
                s.time_idx = i.min(TIME_DIVS.len() - 1);
                s.time_fine = 1.0;
            }
            Selected => s.selected = i.min(2),
            Interpolation => {
                let all = crate::settings::Interpolation::ALL;
                s.display.interpolation = all[i.min(all.len() - 1)]
            }
            Averaging => s.display.averaging = AVERAGING[i.min(AVERAGING.len() - 1)],
            Persistence => s.display.persistence = PERSISTENCE[i.min(PERSISTENCE.len() - 1)],
            Colors => s.display.colors = ColorMode::ALL[i.min(ColorMode::ALL.len() - 1)],
            Style(t) => {
                let all = crate::settings::TraceStyle::ALL;
                s.display.trace_style[t] = all[i.min(all.len() - 1)]
            }
            CursorLook(c) => {
                let all = crate::settings::CursorStyle::ALL;
                s.cursors.get_mut(c).style = all[i.min(all.len() - 1)]
            }
            CursorColour(c) => {
                s.cursors.get_mut(c).color = CursorColor::ALL[i.min(CursorColor::ALL.len() - 1)]
            }
            GridLook => s.display.grid_style = GridStyle::ALL[i.min(GridStyle::ALL.len() - 1)],
            GridExtent => {
                let all = crate::settings::GridExtent::ALL;
                s.display.grid_extent = all[i.min(all.len() - 1)]
            }
            GridBrightness => {
                s.display.grid_brightness = GRID_BRIGHTNESS[i.min(GRID_BRIGHTNESS.len() - 1)]
            }
            AxisLook => s.display.axis_style = GridStyle::ALL[i.min(GridStyle::ALL.len() - 1)],
            AxisExtent => {
                let all = crate::settings::GridExtent::ALL;
                s.display.axis_extent = all[i.min(all.len() - 1)]
            }
            AxisBrightness => {
                s.display.axis_brightness = GRID_BRIGHTNESS[i.min(GRID_BRIGHTNESS.len() - 1)]
            }
            SpecWindow => s.display.window = Window::ALL[i.min(Window::ALL.len() - 1)],
            SpecZoom => s.display.spectrum_zoom = (i as u32).min(MAX_SPECTRUM_ZOOM),
            LogStep => {
                let all = crate::logger::step_choices(logged_channels(s), s.acquisition.max_adc_rate);
                s.logger.step = all[i.min(all.len() - 1)]
            }
            MaxAdc => {
                let caps = &crate::device::protocol::ADC_CAPS;
                s.acquisition.max_adc_rate =
                    crate::device::protocol::raw_rate_of_id(caps[i.min(caps.len() - 1)].0)
            }
            TraceMode => {
                let all = crate::settings::TraceMode::ALL;
                s.display.trace_mode = all[i.min(all.len() - 1)]
            }
            LogLimit => {
                let all = &crate::logger::LIMITS;
                s.logger.log_limit = all[i.min(all.len() - 1)]
            }
            LogAxis => s.logger.time_axis = TimeAxis::ALL[i.min(1)],
            LogCsvTime => s.logger.csv_time = CsvTime::ALL[i.min(1)],
            LogTraceMode => {
                let all = crate::settings::TraceMode::ALL;
                s.logger.trace_mode = all[i.min(all.len() - 1)]
            }
            _ => {}
        }
    }

    /// The fine factor of volts/div and time/div (1 for the others).
    fn fine(self, s: &Settings) -> f64 {
        match self {
            Field::ChVolts(c) => s.channels[c].volts_fine,
            Field::MathScale => s.math.units_fine,
            Field::TimeDiv => s.time_fine,
            _ => 1.0,
        }
    }

    /// Unit of a number field, as typed.
    pub fn unit(self) -> &'static str {
        use Field::*;
        match self {
            ChOffset(_) | MathOffset => "div",
            TrigLevel => "V",
            CursorPos(i) if i >= 2 => "V",
            TrigPosition => "%",
            CursorPos(_) => "s",
            CalFreq => "Hz",
            _ => "",
        }
    }

    /// Scale (units per division) of the trace a voltage cursor reads.
    fn cursor_scale(s: &Settings) -> (f64, f64) {
        if s.selected == 2 {
            (s.math.units_per_div(), s.math.offset)
        } else {
            let c = &s.channels[s.selected];
            (c.volts_per_div(), c.offset)
        }
    }

    /// Value of a number field, in its unit.
    pub fn number(self, s: &Settings) -> f64 {
        use Field::*;
        match self {
            ChOffset(c) => s.channels[c].offset,
            MathOffset => s.math.offset,
            TrigLevel => s.trigger.level,
            TrigPosition => s.trigger.position * 100.0,
            CursorPos(i) if i < 2 => s.cursors.get(i).position * s.time_per_div(),
            CursorPos(i) => {
                let (scale, offset) = Self::cursor_scale(s);
                (s.cursors.get(i).position - offset) * scale
            }
            CalFreq => s.cal_freq as f64,
            _ => 0.0,
        }
    }

    /// Set a number field. `unit` is the unit the user typed ("" means the
    /// field's own unit). Offsets also take volts.
    pub fn set_number(self, s: &mut Settings, v: f64, unit: &str) -> Result<(), String> {
        use Field::*;
        let own = self.unit();
        let unit_ok = unit.is_empty() || unit.eq_ignore_ascii_case(own);
        let half_v = V_DIVS / 2.0;
        let half_h = H_DIVS / 2.0;
        match self {
            ChOffset(c) if unit.eq_ignore_ascii_case("v") => {
                let vdiv = s.channels[c].volts_per_div();
                s.channels[c].offset = (v / vdiv).clamp(-half_v, half_v);
            }
            MathOffset if unit.eq_ignore_ascii_case("v") => {
                s.math.offset = (v / s.math.units_per_div()).clamp(-half_v, half_v);
            }
            _ if !unit_ok => return Err(format!("use {own} for {}", self.label())),
            ChOffset(c) => s.channels[c].offset = v.clamp(-half_v, half_v),
            MathOffset => s.math.offset = v.clamp(-half_v, half_v),
            TrigLevel => s.trigger.level = v,
            TrigPosition => s.trigger.position = (v / 100.0).clamp(0.0, 1.0),
            CursorPos(i) if i < 2 => {
                s.cursors.get_mut(i).position = (v / s.time_per_div()).clamp(-half_h, half_h)
            }
            CursorPos(i) => {
                let (scale, offset) = Self::cursor_scale(s);
                s.cursors.get_mut(i).position = (v / scale + offset).clamp(-half_v, half_v);
            }
            // The calibration output has fixed steps: the nearest one.
            CalFreq if v > 0.0 => {
                let far = |f: &&u32| ((**f as f64) / v).ln().abs();
                s.cal_freq = *CAL_FREQS
                    .iter()
                    .min_by(|a, b| far(a).total_cmp(&far(b)))
                    .unwrap_or(&1_000);
            }
            CalFreq => return Err("the frequency must be above 0".into()),
            _ => return Err(format!("{} is not a number", self.label())),
        }
        // Setting a cursor position turns that cursor on, and shows the
        // cursors.
        if let CursorPos(i) = self {
            s.cursors.get_mut(i).enabled = true;
            s.cursors.hidden = false;
        }
        Ok(())
    }

    /// One step up (`dir` 1) or down (-1) of a number field: the next
    /// step of the calibration frequency, the value plus [`Field::step`]
    /// for the others.
    pub fn step_number(self, s: &mut Settings, dir: i64) {
        self.step_number_by(s, dir, 1.0);
    }

    /// Like [`Field::step_number`], with the step times `scale` (0.1 for
    /// Alt-j/k: a fine step). The calibration frequency has fixed steps.
    pub fn step_number_by(self, s: &mut Settings, dir: i64, scale: f64) {
        if self == Field::CalFreq {
            let i = index_of(&CAL_FREQS, &s.cal_freq) as i64 + dir;
            s.cal_freq = CAL_FREQS[i.clamp(0, CAL_FREQS.len() as i64 - 1) as usize];
        } else {
            let v = self.number(s) + dir as f64 * self.step(s) * scale;
            let _ = self.set_number(s, v, "");
        }
    }

    /// The step of [`Field::step_number`] as text.
    pub fn step_text(self, s: &Settings) -> String {
        match self {
            Field::ChOffset(_) | Field::MathOffset => format!("± {:.1} div", self.step(s)),
            Field::TrigPosition => format!("± {:.0} %", self.step(s)),
            Field::CalFreq => "next / previous step".into(),
            Field::CursorPos(_) => {
                format!("± {} · space on/off", fmt_si(self.step(s), self.unit()))
            }
            _ => format!("± {}", fmt_si(self.step(s), self.unit())),
        }
    }

    /// Step for `+` and `-` on a number field, in its unit.
    pub fn step(self, s: &Settings) -> f64 {
        use Field::*;
        match self {
            ChOffset(_) | MathOffset => 0.1,
            TrigLevel => 0.1 * s.channels[s.trigger_source()].volts_per_div(),
            TrigPosition => 5.0,
            CursorPos(i) if i < 2 => 0.1 * s.time_per_div(),
            CursorPos(_) => 0.1 * Self::cursor_scale(s).0,
            _ => 0.0,
        }
    }

    /// The value as shown in the settings column.
    pub fn value(self, s: &Settings, d: &DeviceInfo) -> String {
        use Field::*;
        match self.kind() {
            Kind::Choice if self == LogOpen => d
                .recent_logs
                .first()
                .map_or("none".into(), |_| format!("{} logs", d.recent_logs.len())),
            // A fine value (Alt-↑ ↓ ← →) between the steps of the list.
            Kind::Choice
                if matches!(self, ChVolts(_) | MathScale | TimeDiv) && self.fine(s) < 1.0 =>
            {
                match self {
                    ChVolts(c) => format!("{}/div", fmt_si(s.channels[c].volts_per_div(), "V")),
                    MathScale => {
                        format!("{}/div", fmt_si(s.math.units_per_div(), s.math.mode.unit()))
                    }
                    _ => format!("{}/div", fmt_si(s.time_per_div(), "s")),
                }
            }
            Kind::Choice | Kind::Bool => self
                .options(s, d)
                .get(self.index(s))
                .cloned()
                .unwrap_or_default(),
            Kind::Number => match self {
                CursorPos(i) if !s.cursors.get(i).enabled => {
                    format!("off · {}", fmt_si(self.number(s), self.unit()))
                }
                ChOffset(_) | MathOffset => format!("{:.1} div", self.number(s)),
                TrigPosition => format!("{:.0} %", self.number(s)),
                _ => fmt_si(self.number(s), self.unit()),
            },
            Kind::Info => {
                let (model, serial, fw) = d.parts();
                match self {
                    Roll => if s.roll() { "on" } else { "off" }.to_string(),
                    DevModel => model,
                    DevSerial => serial,
                    DevFirmware => fw,
                    DevState => if d.connected {
                        "connected"
                    } else {
                        "not connected"
                    }
                    .to_string(),
                    DevCalibration => {
                        let n: Vec<&String> = d
                            .notes
                            .iter()
                            .filter(|n| n.contains("alibration"))
                            .collect();
                        if n.is_empty() {
                            "EEPROM".into()
                        } else {
                            n.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("; ")
                        }
                    }
                    DevRate => match (d.rate, d.adc_rate) {
                        (Some(r), Some(a)) if (a - r).abs() > 0.5 => {
                            format!("{} (ADC {})", fmt_si(r, "S/s"), fmt_si(a, "S/s"))
                        }
                        (Some(r), _) => fmt_si(r, "S/s"),
                        _ => "--".into(),
                    },
                    ExportDir => d.export_dir.clone(),
                    SettingsFile => d.settings_file.clone(),
                    ConfigWarnings => match d.config_warnings.len() {
                        0 => "none".into(),
                        n => format!("{n}"),
                    },
                    LogDir => tail(&d.log_dir, 24),
                    LogFree => d.log_free.map_or("--".into(), crate::logger::fmt_bytes),
                    CalZero => match s.calibration.zero_count() {
                        0 => "EEPROM".into(),
                        n => format!("app, {n} values"),
                    },
                    CalGain => {
                        let g = &s.calibration;
                        let one = |c: usize| {
                            if g.has_gain(c) {
                                format!("CH{} {:+.2} %", c + 1, (g.gain[c] - 1.0) * 100.0)
                            } else {
                                format!("CH{} none", c + 1)
                            }
                        };
                        format!("{} · {}", one(0), one(1))
                    }
                    _ => String::new(),
                }
            }
            Kind::Action => match self {
                Act(Action::RunStop) => if d.running { "RUN" } else { "STOP" }.into(),
                Act(Action::Record) => if d.recording { "● REC" } else { "off" }.into(),
                _ => "⏎".into(),
            },
        }
    }
}

/// Parse a number with an optional SI prefix and unit: `500m`, `2k`,
/// `1.5u`, `-0.2 V`, `50%`, `10 ms`. Returns the value and the unit text.
pub fn parse_value(text: &str) -> Result<(f64, String), String> {
    let t: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if t.is_empty() {
        return Err("type a number".into());
    }
    // The longest start that is a number.
    let mut end = 0;
    for i in (1..=t.len()).rev() {
        if !t.is_char_boundary(i) {
            continue;
        }
        let head = &t[..i];
        // "1e" or "2E" alone are not numbers but must not eat a unit.
        if head.parse::<f64>().is_ok() && !head.ends_with(['e', 'E']) {
            end = i;
            break;
        }
    }
    if end == 0 {
        return Err(format!("not a number: {text}"));
    }
    let n: f64 = t[..end]
        .parse()
        .map_err(|_| format!("not a number: {text}"))?;
    if !n.is_finite() {
        return Err(format!("not a number: {text}"));
    }
    let rest = &t[end..];
    const PREFIXES: [(char, f64); 9] = [
        ('p', 1e-12),
        ('n', 1e-9),
        ('u', 1e-6),
        ('µ', 1e-6),
        ('μ', 1e-6),
        ('m', 1e-3),
        ('k', 1e3),
        ('M', 1e6),
        ('G', 1e9),
    ];
    const UNITS: [&str; 6] = ["V", "s", "Hz", "%", "div", "S/s"];
    let unit_ok = |u: &str| u.is_empty() || UNITS.iter().any(|x| x.eq_ignore_ascii_case(u));
    if unit_ok(rest) {
        return Ok((n, rest.to_string()));
    }
    let mut chars = rest.chars();
    if let Some(p) = chars.next() {
        if let Some((_, scale)) = PREFIXES.iter().find(|(c, _)| *c == p) {
            let unit = chars.as_str();
            if unit_ok(unit) {
                return Ok((n * scale, unit.to_string()));
            }
        }
    }
    Err(format!("unknown unit: {rest}"))
}

/// Which column has the keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Categories,
    Items,
    Values,
}

/// What the app must do after a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing changed in the settings.
    None,
    /// Settings changed: send them to the scope and redraw.
    Changed,
    Close,
    Action(Action),
    /// Show this text on the message line.
    Message(String, bool),
}

/// Longest option list that sets the height of the panel. Longer lists
/// (like time/div) scroll in the value column instead, so the panel does
/// not take most of the screen for one setting.
pub const OPTION_ROWS_MAX: usize = 20;

/// A list of items to show in the middle column.
#[derive(Clone, Debug)]
pub struct Group {
    pub name: String,
    /// Field and the label to show for it.
    pub items: Vec<(Field, String)>,
}

/// Panel state: where the cursor is, and what is being typed.
#[derive(Clone, Debug)]
pub struct Panel {
    pub focus: Focus,
    pub cat: usize,
    pub item: usize,
    pub value: usize,
    /// Search text. Empty means no search.
    pub filter: String,
    /// True while the search text is being typed.
    pub searching: bool,
    /// Text being typed for a number field.
    pub input: Option<String>,
    /// Error from the last typed value.
    pub error: Option<String>,
    pending_g: bool,
    cats: Vec<Category>,
}

impl Default for Panel {
    fn default() -> Self {
        Self::new()
    }
}

impl Panel {
    pub fn new() -> Self {
        Self {
            focus: Focus::Categories,
            cat: 0,
            item: 0,
            value: 0,
            filter: String::new(),
            searching: false,
            input: None,
            error: None,
            pending_g: false,
            cats: categories(),
        }
    }

    /// The groups to show: the categories, or one group of search results.
    pub fn groups(&self) -> Vec<Group> {
        if self.filter.is_empty() {
            return self
                .cats
                .iter()
                .map(|c| Group {
                    name: c.name.to_string(),
                    items: c
                        .fields
                        .iter()
                        .map(|f| (*f, f.label().to_string()))
                        .collect(),
                })
                .collect();
        }
        let q = self.filter.to_lowercase();
        let mut items = Vec::new();
        for c in &self.cats {
            let cat_hit = c.name.to_lowercase().contains(&q);
            for f in &c.fields {
                if cat_hit || f.label().to_lowercase().contains(&q) {
                    items.push((*f, format!("{} › {}", c.name, f.label())));
                }
            }
        }
        vec![Group {
            name: format!("Search: {}", self.filter),
            items,
        }]
    }

    /// Rows the three columns need, the same wherever the cursor is: the
    /// most of the number of categories, the settings in any category,
    /// and the rows of any setting in the value column. An option list
    /// longer than [`OPTION_ROWS_MAX`] does not count: it scrolls.
    pub fn column_rows(&self, s: &Settings, d: &DeviceInfo) -> usize {
        let items = self.cats.iter().map(|c| c.fields.len()).max().unwrap_or(0);
        let values = self
            .cats
            .iter()
            .flat_map(|c| &c.fields)
            .map(|f| match f.kind() {
                Kind::Choice | Kind::Bool => f.options(s, d).len(),
                // The value box, the step, the key hint and an error.
                Kind::Number => 6,
                Kind::Info | Kind::Action => 1,
            })
            .filter(|&n| n <= OPTION_ROWS_MAX)
            .max()
            .unwrap_or(0);
        self.cats.len().max(items).max(values).max(1)
    }

    /// The field under the cursor.
    pub fn current(&self) -> Option<Field> {
        let g = self.groups();
        g.get(self.cat)?.items.get(self.item).map(|(f, _)| *f)
    }

    /// Name of the current group and field, like `Trigger › Level`.
    pub fn breadcrumb(&self) -> String {
        let g = self.groups();
        let Some(group) = g.get(self.cat) else {
            return String::new();
        };
        match (self.focus, group.items.get(self.item)) {
            (Focus::Categories, _) | (_, None) => group.name.clone(),
            (_, Some((_, label))) if !self.filter.is_empty() => label.clone(),
            (_, Some((_, label))) => format!("{} › {label}", group.name),
        }
    }

    fn clamp(&mut self) {
        let g = self.groups();
        self.cat = self.cat.min(g.len().saturating_sub(1));
        let n = g.get(self.cat).map_or(0, |g| g.items.len());
        self.item = self.item.min(n.saturating_sub(1));
    }

    /// True when the value column shows a number (not a list).
    pub fn on_number(&self) -> bool {
        self.focus == Focus::Values && self.current().is_some_and(|f| f.kind() == Kind::Number)
    }

    /// Handle one key. `s` is changed live; `d` gives read-only facts;
    /// `keys` has the bindings. Typing a value or a search uses fixed
    /// keys (characters, Enter, Esc, Backspace, arrows).
    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        s: &mut Settings,
        d: &DeviceInfo,
        keys: &KeyMap,
    ) -> Outcome {
        if self.input.is_some() {
            return self.input_key(key, s, keys);
        }
        if self.searching {
            return self.search_key(key, s, d);
        }
        let act = keys.action(&[View::Panel], &key);
        if self.on_number() {
            if let Some(out) = self.number_key(key, act, s, keys) {
                self.pending_g = false;
                return out;
            }
        }
        let g_was = std::mem::take(&mut self.pending_g);
        let Some(act) = act else {
            return Outcome::None;
        };
        let items = self.focus == Focus::Items;
        match act {
            Act::PanelTop if g_was => self.jump(0, s, d),
            Act::PanelTop => {
                self.pending_g = true;
                Outcome::None
            }
            Act::PanelBottom => self.jump(usize::MAX, s, d),
            Act::PanelDeleteLog
                if self.focus == Focus::Values
                    && self.current() == Some(Field::LogOpen)
                    && self.value < d.recent_logs.len() =>
            {
                Outcome::Action(Action::DeleteLog(self.value))
            }
            Act::PanelDown => self.step(1, s, d),
            Act::PanelUp => self.step(-1, s, d),
            Act::PanelBack => self.back(),
            Act::PanelEnter => self.enter(s, d),
            Act::PanelEscape => match self.focus {
                Focus::Categories if !self.filter.is_empty() => {
                    self.filter.clear();
                    self.cat = 0;
                    self.item = 0;
                    Outcome::None
                }
                Focus::Categories => Outcome::Close,
                _ => self.back(),
            },
            Act::PanelClose => {
                if self.focus == Focus::Values {
                    self.focus = Focus::Items;
                }
                Outcome::Close
            }
            Act::PanelSearch => {
                self.searching = true;
                self.filter.clear();
                self.cat = 0;
                self.item = 0;
                self.focus = Focus::Items;
                Outcome::None
            }
            Act::PanelToggle if items => self.toggle(s, d, 1, true),
            Act::PanelNext if items => self.toggle(s, d, 1, false),
            Act::PanelPrevious if items => self.toggle(s, d, -1, false),
            // Types a value on number fields; the next value on others.
            Act::PanelType if items => match self.current().map(Field::kind) {
                Some(Kind::Number) => self.open_input(s),
                _ => self.toggle(s, d, 1, false),
            },
            _ => Outcome::None,
        }
    }

    /// Move to the first (`to == 0`) or last item of the focused column.
    fn jump(&mut self, to: usize, s: &mut Settings, d: &DeviceInfo) -> Outcome {
        let g = self.groups();
        match self.focus {
            Focus::Categories => {
                self.cat = to.min(g.len().saturating_sub(1));
                self.item = 0;
                Outcome::None
            }
            Focus::Items => {
                let n = g.get(self.cat).map_or(0, |g| g.items.len());
                self.item = to.min(n.saturating_sub(1));
                Outcome::None
            }
            Focus::Values => {
                let n = self.current().map_or(0, |f| f.options(s, d).len());
                self.value = to.min(n.saturating_sub(1));
                self.apply_value(s)
            }
        }
    }

    fn step(&mut self, by: i64, s: &mut Settings, d: &DeviceInfo) -> Outcome {
        let mv = |i: usize, n: usize| -> usize {
            if n == 0 {
                0
            } else {
                (i as i64 + by).clamp(0, n as i64 - 1) as usize
            }
        };
        let g = self.groups();
        match self.focus {
            Focus::Categories => {
                let c = mv(self.cat, g.len());
                if c != self.cat {
                    self.cat = c;
                    self.item = 0;
                }
                Outcome::None
            }
            Focus::Items => {
                self.item = mv(self.item, g.get(self.cat).map_or(0, |g| g.items.len()));
                Outcome::None
            }
            Focus::Values => {
                let n = self.current().map_or(0, |f| f.options(s, d).len());
                self.value = mv(self.value, n);
                self.apply_value(s)
            }
        }
    }

    /// Apply the option under the value cursor, live.
    fn apply_value(&mut self, s: &mut Settings) -> Outcome {
        let Some(f) = self.current() else {
            return Outcome::None;
        };
        let before = s.clone();
        f.set_index(s, self.value);
        if *s == before {
            Outcome::None
        } else {
            Outcome::Changed
        }
    }

    /// h / Esc: go to the parent column. A value picked in the value
    /// column is already applied and stays.
    fn back(&mut self) -> Outcome {
        match self.focus {
            Focus::Values => {
                self.focus = Focus::Items;
                Outcome::None
            }
            Focus::Items => {
                self.focus = Focus::Categories;
                Outcome::None
            }
            Focus::Categories => Outcome::None,
        }
    }

    /// l / Enter: go into the child column, or run the item.
    fn enter(&mut self, s: &mut Settings, d: &DeviceInfo) -> Outcome {
        match self.focus {
            Focus::Categories => {
                let g = self.groups();
                if g.get(self.cat).is_some_and(|g| !g.items.is_empty()) {
                    self.focus = Focus::Items;
                    self.item = self.item.min(g[self.cat].items.len() - 1);
                }
                Outcome::None
            }
            Focus::Items => {
                let Some(f) = self.current() else {
                    return Outcome::None;
                };
                match f.kind() {
                    Kind::Choice | Kind::Bool => {
                        self.value = f.index(s);
                        self.focus = Focus::Values;
                        Outcome::None
                    }
                    Kind::Number => {
                        self.focus = Focus::Values;
                        Outcome::None
                    }
                    Kind::Action => match f {
                        Field::Act(a) => Outcome::Action(a),
                        _ => Outcome::None,
                    },
                    Kind::Info => {
                        Outcome::Message(format!("{}: {}", f.label(), f.value(s, d)), false)
                    }
                }
            }
            Focus::Values if self.current() == Some(Field::LogOpen) => {
                if d.recent_logs.is_empty() {
                    return Outcome::None;
                }
                self.focus = Focus::Items;
                Outcome::Action(Action::OpenLog(self.value))
            }
            Focus::Values => {
                // The value is already applied: go back to the settings.
                self.focus = Focus::Items;
                Outcome::None
            }
        }
    }

    /// Space, + and -: toggle a bool, step a choice or a number.
    /// `space`: the key was Space, which turns a cursor on or off.
    fn toggle(&mut self, s: &mut Settings, d: &DeviceInfo, dir: i64, space: bool) -> Outcome {
        let Some(f) = self.current() else {
            return Outcome::None;
        };
        let before = s.clone();
        match f.kind() {
            Kind::Bool => f.set_index(s, 1 - f.index(s)),
            Kind::Choice => {
                let n = f.options(s, d).len() as i64;
                let i = (f.index(s) as i64 + dir).rem_euclid(n.max(1));
                f.set_index(s, i as usize);
            }
            Kind::Number => match f {
                Field::CursorPos(i) if space => {
                    let c = s.cursors.get_mut(i);
                    c.enabled = !c.enabled;
                    if c.enabled {
                        s.cursors.hidden = false;
                    }
                }
                _ => f.step_number(s, dir),
            },
            Kind::Action => {
                if let Field::Act(a) = f {
                    return Outcome::Action(a);
                }
            }
            Kind::Info => {}
        }
        if *s == before {
            Outcome::None
        } else {
            Outcome::Changed
        }
    }

    /// Start typing a number, with the current value as the start text
    /// (so Backspace edits it). Also moves to the value column: `=` and
    /// `i` on a number setting.
    fn open_input(&mut self, s: &Settings) -> Outcome {
        if let Some(f) = self.current() {
            if f.kind() == Kind::Number {
                let v = f.number(s);
                let text = match f {
                    Field::ChOffset(_) | Field::MathOffset => format!("{v:.1}"),
                    Field::TrigPosition => format!("{v:.0}"),
                    _ => fmt_si(v, f.unit()).replace(' ', ""),
                };
                self.input = Some(text);
                self.error = None;
                self.focus = Focus::Values;
            }
        }
        Outcome::None
    }

    /// A key in the value column of a number setting, when not typing.
    /// `None`: the key does what it does in any column.
    fn number_key(
        &mut self,
        key: KeyEvent,
        act: Option<Act>,
        s: &mut Settings,
        keys: &KeyMap,
    ) -> Option<Outcome> {
        let f = self.current()?;
        let step = |s: &mut Settings, dir: i64| {
            let before = s.clone();
            f.step_number(s, dir);
            if *s == before {
                Outcome::None
            } else {
                Outcome::Changed
            }
        };
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        // Alt with a step key: a tenth of the step.
        if key.modifiers == KeyModifiers::ALT
            || key.modifiers == KeyModifiers::ALT | KeyModifiers::SHIFT
        {
            let bare = KeyEvent::new(key.code, key.modifiers - KeyModifiers::ALT);
            let dir = match keys.action(&[View::Panel], &bare) {
                Some(Act::PanelUp | Act::PanelNext) => 1,
                Some(Act::PanelDown | Act::PanelPrevious) => -1,
                _ => 0,
            };
            if dir != 0 {
                let before = s.clone();
                f.step_number_by(s, dir, 0.1);
                return Some(if *s == before {
                    Outcome::None
                } else {
                    Outcome::Changed
                });
            }
        }
        Some(match (act, key.code) {
            // Digits, a point or a minus start a new value (before any
            // binding: `-` types a minus here).
            (_, KeyCode::Char(c)) if plain && (c.is_ascii_digit() || c == '.' || c == '-') => {
                self.input = Some(c.to_string());
                self.error = None;
                Outcome::None
            }
            (Some(Act::PanelUp | Act::PanelNext), _) => step(s, 1),
            (Some(Act::PanelDown | Act::PanelPrevious), _) => step(s, -1),
            (Some(Act::PanelToggle), _) if matches!(f, Field::CursorPos(_)) => {
                let Field::CursorPos(i) = f else {
                    return None;
                };
                let c = s.cursors.get_mut(i);
                c.enabled = !c.enabled;
                if c.enabled {
                    s.cursors.hidden = false;
                }
                Outcome::Changed
            }
            (Some(Act::PanelType), _) => self.open_input(s),
            (_, KeyCode::Backspace) => {
                self.open_input(s);
                if let Some(t) = self.input.as_mut() {
                    t.pop();
                }
                Outcome::None
            }
            (Some(Act::PanelTop | Act::PanelBottom), _) => Outcome::None,
            _ => return None,
        })
    }

    /// Apply the typed text. On an error the text stays, with the error.
    fn apply_input(&mut self, s: &mut Settings) -> Result<Outcome, ()> {
        let text = self.input.clone().unwrap_or_default();
        let Some(f) = self.current() else {
            self.input = None;
            return Ok(Outcome::None);
        };
        let before = s.clone();
        match parse_value(&text).and_then(|(v, unit)| f.set_number(s, v, &unit)) {
            Ok(()) => {
                self.input = None;
                self.error = None;
                Ok(if *s == before {
                    Outcome::None
                } else {
                    Outcome::Changed
                })
            }
            Err(e) => {
                self.error = Some(e);
                Err(())
            }
        }
    }

    /// A key while typing a number. Enter applies it; moving away (←, h,
    /// ↑ ↓, closing) applies it too; Esc drops only the typed text.
    fn input_key(&mut self, key: KeyEvent, s: &mut Settings, keys: &KeyMap) -> Outcome {
        // Close keys that are not plain characters (like F2) close while
        // typing too; characters are typed.
        let close = keys.is(Act::PanelClose, &key) && !matches!(key.code, KeyCode::Char(_));
        match key.code {
            KeyCode::Esc => {
                self.input = None;
                self.error = None;
                Outcome::None
            }
            KeyCode::Backspace => {
                if let Some(t) = self.input.as_mut() {
                    t.pop();
                }
                Outcome::None
            }
            KeyCode::Enter => self.apply_input(s).unwrap_or(Outcome::None),
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Tab => match self.apply_input(s) {
                Ok(out) => {
                    self.focus = Focus::Items;
                    out
                }
                Err(()) => Outcome::None,
            },
            KeyCode::Up | KeyCode::Down => match self.apply_input(s) {
                Ok(out) => {
                    let dir = if key.code == KeyCode::Up { 1 } else { -1 };
                    let before = s.clone();
                    if let Some(f) = self.current() {
                        f.step_number(s, dir);
                    }
                    if out == Outcome::Changed || *s != before {
                        Outcome::Changed
                    } else {
                        Outcome::None
                    }
                }
                Err(()) => Outcome::None,
            },
            _ if close => {
                // Close; a value that does not parse is dropped.
                let _ = self.apply_input(s);
                self.input = None;
                self.error = None;
                self.focus = Focus::Items;
                Outcome::Close
            }
            KeyCode::Char(c) => {
                if let Some(t) = self.input.as_mut() {
                    t.push(c);
                }
                Outcome::None
            }
            _ => Outcome::None,
        }
    }

    fn search_key(&mut self, key: KeyEvent, s: &mut Settings, d: &DeviceInfo) -> Outcome {
        match key.code {
            KeyCode::Esc => {
                self.searching = false;
                self.filter.clear();
                self.cat = 0;
                self.item = 0;
                self.focus = Focus::Categories;
            }
            KeyCode::Enter => {
                self.searching = false;
                if self.groups()[0].items.is_empty() {
                    self.filter.clear();
                    self.focus = Focus::Categories;
                }
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.cat = 0;
                self.item = 0;
            }
            KeyCode::Down => return self.step(1, s, d),
            KeyCode::Up => return self.step(-1, s, d),
            KeyCode::Char(c) => {
                self.filter.push(c);
                self.cat = 0;
                self.item = 0;
            }
            _ => {}
        }
        self.clamp();
        Outcome::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn key(p: &mut Panel, s: &mut Settings, code: KeyCode) -> Outcome {
        p.handle_key(
            KeyEvent::new(code, KeyModifiers::NONE),
            s,
            &DeviceInfo::default(),
            &KeyMap::default(),
        )
    }

    fn keys(p: &mut Panel, s: &mut Settings, text: &str) -> Vec<Outcome> {
        text.chars().map(|c| key(p, s, KeyCode::Char(c))).collect()
    }

    /// Go to category `name` and item `label`, from the top.
    fn goto(p: &mut Panel, s: &mut Settings, name: &str, label: &str) {
        *p = Panel::new();
        while p.groups()[p.cat].name != name {
            assert!(p.cat < 20, "no category {name}");
            key(p, s, KeyCode::Char('j'));
        }
        key(p, s, KeyCode::Char('l'));
        while p.current().unwrap().label() != label {
            let before = p.item;
            key(p, s, KeyCode::Char('j'));
            assert_ne!(before, p.item, "no item {label} in {name}");
        }
    }

    #[test]
    fn every_field_has_label_help_and_a_valid_value() {
        let s = Settings::default();
        let d = DeviceInfo {
            rate: Some(1e6),
            ..Default::default()
        };
        for c in categories() {
            for f in c.fields {
                assert!(!f.label().is_empty() && !f.help().is_empty(), "{f:?}");
                match f.kind() {
                    // Its options are the log files on the disk.
                    _ if f == Field::LogOpen => assert_eq!(f.value(&s, &d), "none"),
                    Kind::Choice | Kind::Bool => {
                        let o = f.options(&s, &d);
                        assert!(f.index(&s) < o.len(), "{f:?}");
                        assert!(!f.value(&s, &d).is_empty(), "{f:?}");
                    }
                    Kind::Number => assert!(!f.unit().is_empty(), "{f:?}"),
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn set_index_round_trips_for_every_option() {
        let d = DeviceInfo::default();
        for c in categories() {
            for f in c.fields {
                if !matches!(f.kind(), Kind::Choice | Kind::Bool) {
                    continue;
                }
                let mut s = Settings::default();
                for i in 0..f.options(&s, &d).len() {
                    f.set_index(&mut s, i);
                    assert_eq!(f.index(&s), i, "{f:?} option {i}");
                }
            }
        }
    }

    #[test]
    fn logger_category_and_estimate() {
        let mut s = Settings::default();
        let mut d = DeviceInfo {
            recent_logs: vec!["log-b.ohlog".into(), "log-a.ohlog".into()],
            log_free: Some(500_000_000),
            ..Default::default()
        };
        let cats = categories();
        let lg = cats.iter().find(|c| c.name == "Logger").unwrap();
        // No more rows than the categories.
        assert!(lg.fields.len() <= categories().len());
        assert_eq!(Field::LogLimit.value(&s, &d), "1 GB ≈ 11 h 34 min");
        assert_eq!(
            Field::LogLimit.help_text(&s, &d),
            "Limit 1 GB ≈ 11 h 34 min at 1 ms step · only 500 MB free (≈ 5 h 47 min)"
        );
        d.log_free = Some(5_000_000_000);
        assert_eq!(
            Field::LogLimit.help_text(&s, &d),
            "Limit 1 GB ≈ 11 h 34 min at 1 ms step"
        );
        s.channels[1].enabled = false;
        s.logger.step = 0.5e-6;
        assert_eq!(Field::LogLimit.value(&s, &d), "1 GB ≈ 42 s");
        // CH1 alone: 1-channel mode at 30 MS/s.
        assert!(Field::LogStep
            .help_text(&s, &d)
            .contains("ADC at 30 MS/s, 15 samples per step, 86.4 GB/h for 1 channel."));
        s.channels[1].enabled = true;
        assert!(Field::LogStep
            .help_text(&s, &d)
            .contains("ADC at 16 MS/s, 8 samples per step, 173 GB/h for 2 channels."));
        // The raw step: first in the list, with its limits in the help.
        assert_eq!(Field::LogStep.options(&s, &d)[0], "raw (62.5 ns)");
        Field::LogStep.set_index(&mut s, 0);
        assert_eq!(s.logger.step, crate::logger::RAW);
        assert_eq!(Field::LogStep.value(&s, &d), "raw (62.5 ns)");
        let help = Field::LogStep.help_text(&s, &d);
        for part in [
            "at 16 MS/s",
            "no min/max",
            "32 MB/s, so 1 GB lasts ≈ 31 s",
            "disk must keep up",
            "20 MHz",
            "CH2 alone",
            "Max ADC rate",
        ] {
            assert!(help.contains(part), "{part}: {help}");
        }
        s.channels[1].enabled = false;
        assert_eq!(Field::LogStep.value(&s, &d), "raw (33.3 ns)");
        assert!(Field::LogLimit.value(&s, &d).starts_with("1 GB ≈ 33 s"));
        // A lower max ADC rate: the steps follow it.
        s.acquisition.max_adc_rate = 1e6;
        assert_eq!(Field::LogStep.value(&s, &d), "raw (1 µs)");
        s.logger.step = 0.5e-6;
        assert_eq!(Field::LogStep.value(&s, &d), "1 µs");
        s.acquisition.max_adc_rate = 16e6;
        s.channels[1].enabled = true;
        s.logger.step = 1e-3;
        assert_eq!(Field::LogOpen.value(&s, &d), "2 logs");
        d.log_dir = "/home/someone/.local/share/openhantek-tui/logs".into();
        assert_eq!(Field::LogDir.value(&s, &d), "…are/openhantek-tui/logs");
        assert!(Field::LogDir
            .help_text(&s, &d)
            .starts_with("/home/someone/"));
        // Enter on a recent log opens it.
        let mut p = Panel::new();
        p.cat = cats.iter().position(|c| c.name == "Logger").unwrap();
        p.focus = Focus::Items;
        p.item = lg.fields.iter().position(|f| *f == Field::LogOpen).unwrap();
        let k = |p: &mut Panel, s: &mut Settings, c| {
            p.handle_key(KeyEvent::from(c), s, &d, &KeyMap::default())
        };
        k(&mut p, &mut s, KeyCode::Enter);
        assert_eq!(p.focus, Focus::Values);
        k(&mut p, &mut s, KeyCode::Char('j'));
        assert_eq!(
            k(&mut p, &mut s, KeyCode::Enter),
            Outcome::Action(Action::OpenLog(1))
        );
        // The 13 categories are still the most rows (the size limit list
        // has 13 too).
        assert_eq!(Panel::new().column_rows(&s, &d), 13);
    }

    #[test]
    fn hjkl_moves_between_columns() {
        let mut p = Panel::new();
        let mut s = Settings::default();
        assert_eq!(p.focus, Focus::Categories);
        key(&mut p, &mut s, KeyCode::Char('j'));
        assert_eq!(p.groups()[p.cat].name, "Channel 2");
        key(&mut p, &mut s, KeyCode::Char('k'));
        key(&mut p, &mut s, KeyCode::Char('k'));
        assert_eq!(p.cat, 0);
        key(&mut p, &mut s, KeyCode::Char('l'));
        assert_eq!(p.focus, Focus::Items);
        key(&mut p, &mut s, KeyCode::Char('G'));
        assert_eq!(p.current(), Some(Field::Style(0)));
        keys(&mut p, &mut s, "gg");
        assert_eq!(p.current(), Some(Field::ChEnabled(0)));
        key(&mut p, &mut s, KeyCode::Char('h'));
        assert_eq!(p.focus, Focus::Categories);
        key(&mut p, &mut s, KeyCode::Char('G'));
        assert_eq!(p.groups()[p.cat].name, "Logger");
        assert_eq!(key(&mut p, &mut s, KeyCode::Esc), Outcome::Close);
    }

    #[test]
    fn value_column_applies_at_once_and_keeps_it() {
        let mut p = Panel::new();
        let mut s = Settings::default();
        goto(&mut p, &mut s, "Channel 1", "Coupling");
        key(&mut p, &mut s, KeyCode::Char('l'));
        assert_eq!(p.focus, Focus::Values);
        assert_eq!(key(&mut p, &mut s, KeyCode::Char('j')), Outcome::Changed);
        assert_eq!(s.channels[0].coupling, Coupling::Ac);
        // Esc goes back a column and keeps the new value.
        assert_eq!(key(&mut p, &mut s, KeyCode::Esc), Outcome::None);
        assert_eq!(s.channels[0].coupling, Coupling::Ac);
        assert_eq!(p.focus, Focus::Items);
        // h keeps it too.
        key(&mut p, &mut s, KeyCode::Char('l'));
        assert_eq!(key(&mut p, &mut s, KeyCode::Char('k')), Outcome::Changed);
        assert_eq!(s.channels[0].coupling, Coupling::Dc);
        assert_eq!(key(&mut p, &mut s, KeyCode::Char('h')), Outcome::None);
        assert_eq!(s.channels[0].coupling, Coupling::Dc);
        // Enter only goes back, the value is already set.
        key(&mut p, &mut s, KeyCode::Char('l'));
        key(&mut p, &mut s, KeyCode::Char('j'));
        assert_eq!(key(&mut p, &mut s, KeyCode::Enter), Outcome::None);
        assert_eq!(s.channels[0].coupling, Coupling::Ac);
        assert_eq!(p.focus, Focus::Items);
        // Esc at the top closes, still with the new value.
        key(&mut p, &mut s, KeyCode::Char('l'));
        key(&mut p, &mut s, KeyCode::Char('k'));
        key(&mut p, &mut s, KeyCode::Esc);
        key(&mut p, &mut s, KeyCode::Esc);
        assert_eq!(key(&mut p, &mut s, KeyCode::Esc), Outcome::Close);
        assert_eq!(s.channels[0].coupling, Coupling::Dc);
    }

    #[test]
    fn space_toggles_and_cycles() {
        let mut p = Panel::new();
        let mut s = Settings::default();
        goto(&mut p, &mut s, "Channel 2", "Invert");
        assert_eq!(key(&mut p, &mut s, KeyCode::Char(' ')), Outcome::Changed);
        assert!(s.channels[1].invert);
        goto(&mut p, &mut s, "Trigger", "Mode");
        key(&mut p, &mut s, KeyCode::Char(' '));
        assert_eq!(s.trigger.mode, TriggerMode::Normal);
        key(&mut p, &mut s, KeyCode::Char('-'));
        assert_eq!(s.trigger.mode, TriggerMode::Auto);
        key(&mut p, &mut s, KeyCode::Char('-'));
        assert_eq!(s.trigger.mode, TriggerMode::Single);
    }

    #[test]
    fn plus_minus_step_numbers() {
        let mut p = Panel::new();
        let mut s = Settings::default();
        goto(&mut p, &mut s, "Trigger", "Level");
        key(&mut p, &mut s, KeyCode::Char('+'));
        assert!((s.trigger.level - 0.1).abs() < 1e-12);
        key(&mut p, &mut s, KeyCode::Char('-'));
        key(&mut p, &mut s, KeyCode::Char('-'));
        assert!((s.trigger.level + 0.1).abs() < 1e-12);
    }

    #[test]
    fn typed_values() {
        let mut p = Panel::new();
        let mut s = Settings::default();
        goto(&mut p, &mut s, "Trigger", "Level");
        // `=` still types, with the value as the start text.
        key(&mut p, &mut s, KeyCode::Char('='));
        assert_eq!(p.focus, Focus::Values);
        assert_eq!(p.input.as_deref(), Some("0V"));
        for _ in 0..10 {
            key(&mut p, &mut s, KeyCode::Backspace);
        }
        keys(&mut p, &mut s, "500m");
        assert_eq!(key(&mut p, &mut s, KeyCode::Enter), Outcome::Changed);
        assert!((s.trigger.level - 0.5).abs() < 1e-12);
        assert!(p.input.is_none());
        assert_eq!(p.focus, Focus::Values);
        // A bad value keeps the input open, with an error.
        keys(&mut p, &mut s, "abc");
        assert!(p.input.is_none(), "letters do not start a value");
        keys(&mut p, &mut s, "1x");
        assert_eq!(key(&mut p, &mut s, KeyCode::Enter), Outcome::None);
        assert!(p.error.is_some() && p.input.is_some());
        // Esc drops only the typed text.
        key(&mut p, &mut s, KeyCode::Esc);
        assert!(p.input.is_none());
        assert_eq!(p.focus, Focus::Values);
        assert!((s.trigger.level - 0.5).abs() < 1e-12);
        // Position takes percent.
        goto(&mut p, &mut s, "Trigger", "Position");
        key(&mut p, &mut s, KeyCode::Enter);
        keys(&mut p, &mut s, "25%");
        key(&mut p, &mut s, KeyCode::Enter);
        assert!((s.trigger.position - 0.25).abs() < 1e-12);
    }

    #[test]
    fn number_value_column_steps_and_types() {
        let mut p = Panel::new();
        let mut s = Settings::default();
        goto(&mut p, &mut s, "Trigger", "Level");
        // l goes to the value column, which shows the number, no list.
        key(&mut p, &mut s, KeyCode::Char('l'));
        assert_eq!(p.focus, Focus::Values);
        assert!(p.on_number());
        assert!(p.input.is_none());
        // k and ↑ step up, j and ↓ down, by the +/- step (0.1 div = 100 mV).
        assert_eq!(key(&mut p, &mut s, KeyCode::Char('k')), Outcome::Changed);
        assert!((s.trigger.level - 0.1).abs() < 1e-12);
        key(&mut p, &mut s, KeyCode::Up);
        assert!((s.trigger.level - 0.2).abs() < 1e-12);
        key(&mut p, &mut s, KeyCode::Char('j'));
        key(&mut p, &mut s, KeyCode::Down);
        key(&mut p, &mut s, KeyCode::Down);
        assert!((s.trigger.level + 0.1).abs() < 1e-12);
        assert_eq!(p.focus, Focus::Values);
        // A digit starts a new value at once; SI suffixes work.
        keys(&mut p, &mut s, "1.5");
        assert_eq!(p.input.as_deref(), Some("1.5"));
        assert!((s.trigger.level + 0.1).abs() < 1e-12, "not applied yet");
        key(&mut p, &mut s, KeyCode::Backspace);
        keys(&mut p, &mut s, "25m");
        assert_eq!(p.input.as_deref(), Some("1.25m"));
        assert_eq!(key(&mut p, &mut s, KeyCode::Enter), Outcome::Changed);
        assert!((s.trigger.level - 1.25e-3).abs() < 1e-12);
        // A minus starts a negative value; moving away applies it.
        keys(&mut p, &mut s, "-2u");
        assert_eq!(key(&mut p, &mut s, KeyCode::Left), Outcome::Changed);
        assert!((s.trigger.level + 2e-6).abs() < 1e-15);
        assert_eq!(p.focus, Focus::Items);
        assert!(p.input.is_none());
        // ↑ while typing applies, then steps.
        key(&mut p, &mut s, KeyCode::Char('l'));
        keys(&mut p, &mut s, "1");
        key(&mut p, &mut s, KeyCode::Up);
        assert!((s.trigger.level - 1.1).abs() < 1e-12);
        // Backspace edits the current value.
        key(&mut p, &mut s, KeyCode::Backspace);
        assert_eq!(p.input.as_deref(), Some("1.1"));
        // Esc cancels the typing, a second Esc goes back.
        key(&mut p, &mut s, KeyCode::Esc);
        assert_eq!(p.focus, Focus::Values);
        key(&mut p, &mut s, KeyCode::Esc);
        assert_eq!(p.focus, Focus::Items);
        assert!((s.trigger.level - 1.1).abs() < 1e-12);
        // The calibration frequency steps through its fixed steps, and a
        // typed value snaps to the nearest one.
        goto(&mut p, &mut s, "Calibration", "Output frequency");
        key(&mut p, &mut s, KeyCode::Enter);
        key(&mut p, &mut s, KeyCode::Char('k'));
        assert_eq!(s.cal_freq, 1_200);
        keys(&mut p, &mut s, "9.5k");
        key(&mut p, &mut s, KeyCode::Enter);
        assert_eq!(s.cal_freq, 10_000);
        // Lists keep their list: j moves the option.
        goto(&mut p, &mut s, "Display", "Averaging");
        key(&mut p, &mut s, KeyCode::Char('l'));
        assert!(!p.on_number());
        key(&mut p, &mut s, KeyCode::Char('j'));
        assert_eq!(s.display.averaging, 2);
        // Closing while typing applies the value.
        goto(&mut p, &mut s, "Trigger", "Level");
        key(&mut p, &mut s, KeyCode::Char('l'));
        keys(&mut p, &mut s, "300m");
        assert_eq!(key(&mut p, &mut s, KeyCode::F(2)), Outcome::Close);
        assert!((s.trigger.level - 0.3).abs() < 1e-12);
    }

    #[test]
    fn offsets_take_divisions_or_volts() {
        let mut s = Settings::default();
        Field::ChOffset(0).set_number(&mut s, 1.5, "").unwrap();
        assert_eq!(s.channels[0].offset, 1.5);
        // 1 V/div: 2 V is 2 divisions.
        Field::ChOffset(0).set_number(&mut s, 2.0, "V").unwrap();
        assert_eq!(s.channels[0].offset, 2.0);
        Field::ChOffset(0).set_number(&mut s, 99.0, "").unwrap();
        assert_eq!(s.channels[0].offset, 4.0);
        assert!(Field::TrigLevel.set_number(&mut s, 1.0, "s").is_err());
    }

    #[test]
    fn cursor_values_in_time_and_volts() {
        let mut s = Settings::default();
        // 1 ms/div: 2 ms is 2 divisions.
        Field::CursorPos(1).set_number(&mut s, 2e-3, "s").unwrap();
        assert!((s.cursors.time_b.position - 2.0).abs() < 1e-9);
        assert!(s.cursors.time_b.enabled && !s.cursors.time_a.enabled);
        assert!((Field::CursorPos(1).number(&s) - 2e-3).abs() < 1e-12);
        // CH1 at 1 V/div with its 0 V line at +2 div: 1 V is at +3 div.
        s.cursors.hidden = true;
        Field::CursorPos(2).set_number(&mut s, 1.0, "V").unwrap();
        assert!((s.cursors.volt_a.position - 3.0).abs() < 1e-9);
        assert!(s.cursors.volt_a.enabled && !s.cursors.hidden);
    }

    #[test]
    fn cursors_category_has_on_style_and_colour_per_cursor() {
        let cats = categories();
        let c = cats.iter().find(|c| c.name == "Cursors").unwrap();
        assert_eq!(c.fields.len(), 12);
        let mut s = Settings::default();
        let mut p = Panel::new();
        goto(&mut p, &mut s, "Cursors", "Voltage B");
        assert_eq!(p.current(), Some(Field::CursorPos(3)));
        assert!(Field::CursorPos(3)
            .value(&s, &DeviceInfo::default())
            .starts_with("off"));
        // Space turns it on and off; + moves it.
        assert_eq!(key(&mut p, &mut s, KeyCode::Char(' ')), Outcome::Changed);
        assert!(s.cursors.volt_b.enabled);
        key(&mut p, &mut s, KeyCode::Char('+'));
        assert!((s.cursors.volt_b.position - 1.1).abs() < 1e-9);
        // Also in the value column.
        key(&mut p, &mut s, KeyCode::Char('l'));
        key(&mut p, &mut s, KeyCode::Char(' '));
        assert!(!s.cursors.volt_b.enabled);
        key(&mut p, &mut s, KeyCode::Char('h'));
        // Style and colour of each cursor.
        key(&mut p, &mut s, KeyCode::Char('j'));
        assert_eq!(p.current(), Some(Field::CursorLook(3)));
        key(&mut p, &mut s, KeyCode::Char(' '));
        assert_eq!(s.cursors.volt_b.style, CursorStyle::Fine);
        assert_eq!(s.cursors.volt_a.style, CursorStyle::Classic);
        key(&mut p, &mut s, KeyCode::Char('j'));
        key(&mut p, &mut s, KeyCode::Char('-'));
        assert_eq!(s.cursors.volt_b.color, CursorColor::Blue);
    }

    #[test]
    fn parse_si_values() {
        let ok = |t: &str| parse_value(t).unwrap();
        assert_eq!(ok("500m"), (0.5, String::new()));
        assert_eq!(ok("2k"), (2000.0, String::new()));
        let (v, u) = ok("1.5u");
        assert!((v - 1.5e-6).abs() < 1e-18 && u.is_empty());
        let (v, u) = ok("1.5µs");
        assert!((v - 1.5e-6).abs() < 1e-18 && u == "s");
        assert_eq!(ok("-0.2 V"), (-0.2, "V".to_string()));
        assert_eq!(ok("50%"), (50.0, "%".to_string()));
        assert_eq!(ok("10 ms"), (0.01, "s".to_string()));
        assert_eq!(ok("3div"), (3.0, "div".to_string()));
        assert_eq!(ok("1e3"), (1000.0, String::new()));
        assert_eq!(ok("2M"), (2e6, String::new()));
        assert!(parse_value("").is_err());
        assert!(parse_value("abc").is_err());
        assert!(parse_value("5 parsecs").is_err());
    }

    #[test]
    fn search_filters_across_categories() {
        let mut p = Panel::new();
        let mut s = Settings::default();
        key(&mut p, &mut s, KeyCode::Char('/'));
        assert!(p.searching);
        keys(&mut p, &mut s, "coup");
        let g = p.groups();
        assert_eq!(g.len(), 1);
        let labels: Vec<&str> = g[0].items.iter().map(|(_, l)| l.as_str()).collect();
        assert_eq!(labels, ["Channel 1 › Coupling", "Channel 2 › Coupling"]);
        // Down while typing moves in the results.
        key(&mut p, &mut s, KeyCode::Down);
        assert_eq!(p.current(), Some(Field::ChCoupling(1)));
        key(&mut p, &mut s, KeyCode::Enter);
        assert!(!p.searching);
        assert_eq!(p.filter, "coup");
        // The result works like any item.
        key(&mut p, &mut s, KeyCode::Char(' '));
        assert_eq!(s.channels[1].coupling, Coupling::Ac);
        // Esc goes back, and Esc at the top clears the search.
        key(&mut p, &mut s, KeyCode::Esc);
        assert_eq!(p.focus, Focus::Categories);
        assert_eq!(key(&mut p, &mut s, KeyCode::Esc), Outcome::None);
        assert!(p.filter.is_empty());
        assert_eq!(p.groups().len(), categories().len());
        // A category name matches all its items.
        key(&mut p, &mut s, KeyCode::Char('/'));
        keys(&mut p, &mut s, "spectrum");
        assert_eq!(p.groups()[0].items.len(), 3);
        key(&mut p, &mut s, KeyCode::Esc);
        assert!(p.filter.is_empty() && !p.searching);
    }

    #[test]
    fn actions_and_info() {
        let mut p = Panel::new();
        let mut s = Settings::default();
        goto(&mut p, &mut s, "Acquisition", "Autoset");
        assert_eq!(
            key(&mut p, &mut s, KeyCode::Enter),
            Outcome::Action(Action::Autoset)
        );
        goto(&mut p, &mut s, "Device", "Serial");
        assert!(matches!(
            key(&mut p, &mut s, KeyCode::Enter),
            Outcome::Message(..)
        ));
        let d = DeviceInfo {
            name: "DSO-6022BE 0694CE656429 FW0210".into(),
            ..Default::default()
        };
        assert_eq!(Field::DevSerial.value(&s, &d), "0694CE656429");
        assert_eq!(Field::DevFirmware.value(&s, &d), "0210");
        assert_eq!(Field::DevModel.value(&s, &d), "DSO-6022BE");
    }

    #[test]
    fn close_keys() {
        for code in [KeyCode::Char('q'), KeyCode::Char('s'), KeyCode::F(2)] {
            let mut p = Panel::new();
            let mut s = Settings::default();
            goto(&mut p, &mut s, "Channel 1", "Volts/div");
            key(&mut p, &mut s, KeyCode::Char('l'));
            key(&mut p, &mut s, KeyCode::Char('j'));
            // Closing keeps the live value.
            assert_eq!(key(&mut p, &mut s, code), Outcome::Close);
            assert_eq!(s.channels[0].volt_idx, 6);
        }
    }

    #[test]
    fn probe_keeps_trigger_level_on_screen() {
        let mut s = Settings::default();
        s.trigger.level = 0.5;
        Field::ChProbe(0).set_index(&mut s, 1);
        assert_eq!(s.channels[0].probe, 10);
        assert!((s.trigger.level - 5.0).abs() < 1e-12);
    }
}
