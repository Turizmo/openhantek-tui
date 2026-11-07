//! Settings panel, with yazi-style keys: three columns (categories,
//! settings, values), h/j/k/l to move, `/` to search, `=` to type a value.
//!
//! This is the model only: no drawing and no terminal. The drawing is in
//! `ui.rs`.

use ratatui::crossterm::event::{KeyCode, KeyEvent};

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
    Selected,
    Interpolation,
    Dots,
    Averaging,
    Persistence,
    Xy,
    Colors,
    CursorLook,
    SpecOn,
    SpecWindow,
    SpecZoom,
    CursorMode,
    CursorT(usize),
    CursorV(usize),
    MeasPanel,
    CalFreq,
    DevModel,
    DevSerial,
    DevFirmware,
    DevState,
    DevCalibration,
    DevRate,
    ExportDir,
    SettingsFile,
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
    /// ADC sample rate, when known.
    pub rate: Option<f64>,
    pub running: bool,
    pub export_dir: String,
    pub settings_file: String,
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
            name: "Timebase",
            fields: vec![TimeDiv, Roll, Act(Action::RunStop), Act(Action::Autoset)],
        },
        Category {
            name: "Display",
            fields: vec![
                Selected,
                Interpolation,
                Dots,
                Averaging,
                Persistence,
                Xy,
                Colors,
                CursorLook,
            ],
        },
        Category {
            name: "Spectrum",
            fields: vec![SpecOn, SpecWindow, SpecZoom],
        },
        Category {
            name: "Cursors",
            fields: vec![
                CursorMode,
                CursorT(0),
                CursorT(1),
                CursorV(0),
                CursorV(1),
                CursorLook,
            ],
        },
        Category {
            name: "Measurements",
            fields: vec![MeasPanel, Selected],
        },
        Category {
            name: "Calibration output",
            fields: vec![CalFreq],
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
                Act(Action::ExportCsv),
                Act(Action::ExportJson),
                Act(Action::Reset),
            ],
        },
    ]
}

const OFF_ON: [&str; 2] = ["off", "on"];
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
            Selected => "Selected trace",
            Interpolation => "Interpolation",
            Dots => "Dots only",
            Averaging => "Averaging",
            Persistence => "Persistence",
            Xy => "XY mode",
            Colors => "Colours",
            CursorLook => "Cursor style",
            SpecOn => "Show spectrum",
            SpecWindow => "Window",
            SpecZoom => "Span",
            CursorMode => "Cursors",
            CursorT(0) => "Time A",
            CursorT(_) => "Time B",
            CursorV(0) => "Voltage A",
            CursorV(_) => "Voltage B",
            MeasPanel => "Show panel",
            CalFreq => "Frequency",
            DevModel => "Model",
            DevSerial => "Serial",
            DevFirmware => "Firmware",
            DevState => "State",
            DevCalibration => "Calibration",
            DevRate => "Sample rate",
            ExportDir => "Export folder",
            SettingsFile => "Settings file",
            Act(a) => match a {
                Action::RunStop => "Run / stop",
                Action::Single => "Single shot",
                Action::Autoset => "Autoset",
                Action::TriggerToMiddle => "Level to middle",
                Action::ExportCsv => "Export CSV",
                Action::ExportJson => "Export JSON",
                Action::Reset => "Reset all settings",
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
            Style(_) => "classic: lines through the low and high value of each dot column. braille thin: one dot thick. bold: two dots thick.",
            MathEnabled => "Show the math trace. Key 3.",
            MathMode => "How the math trace is made from CH1 and CH2. Key m.",
            MathScale => "Units per division of the math trace.",
            MathOffset => "Height of the math 0 line, in divisions.",
            TrigMode => "Auto shows a trace with no trigger; Normal waits; Single stops after one. Key t.",
            TrigSource => "Which channel the trigger looks at. Key s.",
            TrigSlope => "Which edge starts a trace. Key e.",
            TrigLevel => "Trigger level in volts at the probe tip. Keys + - T.",
            TrigPosition => "Where the trigger point is on the screen, 0 % to 100 %. Keys , .",
            TimeDiv => "Time per division. Keys ← →.",
            Roll => "Slow time bases scroll from right to left, with no trigger.",
            Selected => "The trace the arrow keys, cursors and spectrum use. Key Tab.",
            Interpolation => "sin(x)/x draws a smooth line through few samples. Key z.",
            Dots => "Draw only the samples, no lines between them. Key d.",
            Averaging => "Average this many captures: less noise. Key A.",
            Persistence => "Keep this many old traces on screen, dimmed. Key P.",
            Xy => "Draw CH1 against CH2. Key x.",
            Colors => "auto uses true colour when COLORTERM says so.",
            CursorLook => "classic: grey dashes, names inside the plot. fine dashes: short dashes, names in the margin.",
            SpecOn => "Show the spectrum (FFT) below the trace. Key f.",
            SpecWindow => "Window function for the FFT. Key w.",
            SpecZoom => "Highest frequency on the spectrum screen. Key F.",
            CursorMode => "Two time lines or two voltage lines, with the difference. Key v.",
            CursorT(_) => "Time from the screen center, in seconds. Keys h l H L.",
            CursorV(_) => "Voltage of the selected trace. Keys j k J K.",
            MeasPanel => "The measurement panel on the right. Key M.",
            CalFreq => "Frequency of the 2 Vpp square wave on the calibration pins. Keys g G.",
            DevModel | DevSerial | DevFirmware | DevState => "Read from the scope.",
            DevCalibration => "Where the calibration values came from.",
            DevRate => "Sample rate the ADC runs at now.",
            ExportDir => "Exports go here. Set it with --export-dir.",
            SettingsFile => "Settings are saved here on quit. Set it with --settings.",
            Act(a) => match a {
                Action::RunStop => "Start or stop the capture. Key space.",
                Action::Single => "Arm the trigger for one capture. Key o.",
                Action::Autoset => "Find good volts/div, time/div and trigger level. Key a.",
                Action::TriggerToMiddle => "Set the trigger level to the middle of the signal. Key T.",
                Action::ExportCsv => "Save the traces on screen as CSV. Key E.",
                Action::ExportJson => "Save the traces and settings as JSON. Key W.",
                Action::Reset => "All settings back to the defaults. Key R.",
            },
        }
    }

    pub fn kind(self) -> Kind {
        use Field::*;
        match self {
            ChEnabled(_) | ChInvert(_) | MathEnabled | Dots | Xy | SpecOn | MeasPanel => Kind::Bool,
            ChOffset(_) | MathOffset | TrigLevel | TrigPosition | CursorT(_) | CursorV(_) => {
                Kind::Number
            }
            Roll | DevModel | DevSerial | DevFirmware | DevState | DevCalibration | DevRate
            | ExportDir | SettingsFile => Kind::Info,
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
            Interpolation => list(&["sin(x)/x", "linear"]),
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
            CursorLook => crate::settings::CursorStyle::ALL
                .iter()
                .map(|m| m.name().to_string())
                .collect(),
            SpecWindow => Window::ALL.iter().map(|w| w.name().to_string()).collect(),
            SpecZoom => (0..=MAX_SPECTRUM_ZOOM)
                .map(|z| match d.rate {
                    Some(r) => format!("0 .. {}", fmt_si(r / 2.0 / 2f64.powi(z as i32), "Hz")),
                    None => format!("Nyquist / {}", 1u32 << z),
                })
                .collect(),
            CursorMode => list(&["off", "time", "voltage"]),
            CalFreq => CAL_FREQS.iter().map(|f| fmt_si(*f as f64, "Hz")).collect(),
            _ => Vec::new(),
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
            Dots => b(s.display.dots),
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
            Interpolation => b(s.display.interpolation == crate::settings::Interpolation::Linear),
            Averaging => index_of(&AVERAGING, &s.display.averaging),
            Persistence => index_of(&PERSISTENCE, &s.display.persistence),
            Colors => index_of(&ColorMode::ALL, &s.display.colors),
            Style(t) => index_of(&crate::settings::TraceStyle::ALL, &s.display.trace_style[t]),
            CursorLook => index_of(&crate::settings::CursorStyle::ALL, &s.display.cursor_style),
            SpecWindow => index_of(&Window::ALL, &s.display.window),
            SpecZoom => s.display.spectrum_zoom as usize,
            CursorMode => index_of(
                &[
                    crate::settings::CursorMode::Off,
                    crate::settings::CursorMode::Time,
                    crate::settings::CursorMode::Volt,
                ],
                &s.cursors.mode,
            ),
            CalFreq => index_of(&CAL_FREQS, &s.cal_freq),
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
            Dots => s.display.dots = on,
            Xy => s.display.xy = on,
            SpecOn => s.display.spectrum = on,
            MeasPanel => s.display.measurements = on,
            ChVolts(c) => s.channels[c].volt_idx = i.min(VOLT_DIVS.len() - 1),
            ChCoupling(c) => s.channels[c].coupling = if on { Coupling::Ac } else { Coupling::Dc },
            ChProbe(c) => {
                let ch = &mut s.channels[c];
                let old = ch.probe as f64;
                ch.probe = if on { 10 } else { 1 };
                // Keep the trigger level where it is on the screen.
                if s.trigger.source == c {
                    s.trigger.level *= ch.probe as f64 / old;
                }
            }
            MathMode => {
                let all = crate::dsp::math::MathMode::ALL;
                s.math.mode = all[i.min(all.len() - 1)]
            }
            MathScale => s.math.volt_idx = i.min(VOLT_DIVS.len() - 1),
            TrigMode => {
                s.trigger.mode =
                    [TriggerMode::Auto, TriggerMode::Normal, TriggerMode::Single][i.min(2)]
            }
            TrigSource => s.trigger.source = i.min(1),
            TrigSlope => s.trigger.slope = [Slope::Rising, Slope::Falling, Slope::Both][i.min(2)],
            TimeDiv => s.time_idx = i.min(TIME_DIVS.len() - 1),
            Selected => s.selected = i.min(2),
            Interpolation => {
                s.display.interpolation = if on {
                    crate::settings::Interpolation::Linear
                } else {
                    crate::settings::Interpolation::Sinc
                }
            }
            Averaging => s.display.averaging = AVERAGING[i.min(AVERAGING.len() - 1)],
            Persistence => s.display.persistence = PERSISTENCE[i.min(PERSISTENCE.len() - 1)],
            Colors => s.display.colors = ColorMode::ALL[i.min(ColorMode::ALL.len() - 1)],
            Style(t) => {
                let all = crate::settings::TraceStyle::ALL;
                s.display.trace_style[t] = all[i.min(all.len() - 1)]
            }
            CursorLook => {
                let all = crate::settings::CursorStyle::ALL;
                s.display.cursor_style = all[i.min(all.len() - 1)]
            }
            SpecWindow => s.display.window = Window::ALL[i.min(Window::ALL.len() - 1)],
            SpecZoom => s.display.spectrum_zoom = (i as u32).min(MAX_SPECTRUM_ZOOM),
            CursorMode => {
                s.cursors.mode = [
                    crate::settings::CursorMode::Off,
                    crate::settings::CursorMode::Time,
                    crate::settings::CursorMode::Volt,
                ][i.min(2)]
            }
            CalFreq => s.cal_freq = CAL_FREQS[i.min(CAL_FREQS.len() - 1)],
            _ => {}
        }
    }

    /// Unit of a number field, as typed.
    pub fn unit(self) -> &'static str {
        use Field::*;
        match self {
            ChOffset(_) | MathOffset => "div",
            TrigLevel | CursorV(_) => "V",
            TrigPosition => "%",
            CursorT(_) => "s",
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
            CursorT(i) => s.cursors.t[i] * s.time_per_div(),
            CursorV(i) => {
                let (scale, offset) = Self::cursor_scale(s);
                (s.cursors.v[i] - offset) * scale
            }
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
            CursorT(i) => s.cursors.t[i] = (v / s.time_per_div()).clamp(-half_h, half_h),
            CursorV(i) => {
                let (scale, offset) = Self::cursor_scale(s);
                s.cursors.v[i] = (v / scale + offset).clamp(-half_v, half_v);
            }
            _ => return Err(format!("{} is not a number", self.label())),
        }
        // Typing a cursor position turns that kind of cursor on.
        match self {
            CursorT(_) => s.cursors.mode = crate::settings::CursorMode::Time,
            CursorV(_) => s.cursors.mode = crate::settings::CursorMode::Volt,
            _ => {}
        }
        Ok(())
    }

    /// Step for `+` and `-` on a number field, in its unit.
    pub fn step(self, s: &Settings) -> f64 {
        use Field::*;
        match self {
            ChOffset(_) | MathOffset => 0.1,
            TrigLevel => 0.1 * s.channels[s.trigger.source.min(1)].volts_per_div(),
            TrigPosition => 5.0,
            CursorT(_) => 0.1 * s.time_per_div(),
            CursorV(_) => 0.1 * Self::cursor_scale(s).0,
            _ => 0.0,
        }
    }

    /// The value as shown in the settings column.
    pub fn value(self, s: &Settings, d: &DeviceInfo) -> String {
        use Field::*;
        match self.kind() {
            Kind::Choice | Kind::Bool => self
                .options(s, d)
                .get(self.index(s))
                .cloned()
                .unwrap_or_default(),
            Kind::Number => match self {
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
                    DevRate => d.rate.map_or("--".into(), |r| fmt_si(r, "S/s")),
                    ExportDir => d.export_dir.clone(),
                    SettingsFile => d.settings_file.clone(),
                    _ => String::new(),
                }
            }
            Kind::Action => match self {
                Act(Action::RunStop) => if d.running { "RUN" } else { "STOP" }.into(),
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
    /// Settings from before entering the value column, for Esc.
    revert: Option<Settings>,
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
            revert: None,
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

    /// Handle one key. `s` is changed live; `d` gives read-only facts.
    pub fn handle_key(&mut self, key: KeyEvent, s: &mut Settings, d: &DeviceInfo) -> Outcome {
        if self.input.is_some() {
            return self.input_key(key, s);
        }
        if self.searching {
            return self.search_key(key, s, d);
        }
        let g_was = std::mem::take(&mut self.pending_g);
        match key.code {
            KeyCode::Char('g') if g_was => self.jump(0, s, d),
            KeyCode::Char('g') => {
                self.pending_g = true;
                Outcome::None
            }
            KeyCode::Char('G') => self.jump(usize::MAX, s, d),
            KeyCode::Char('j') | KeyCode::Down => self.step(1, s, d),
            KeyCode::Char('k') | KeyCode::Up => self.step(-1, s, d),
            KeyCode::Char('h') | KeyCode::Left => self.back(s),
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => self.enter(s, d),
            KeyCode::Esc => match self.focus {
                Focus::Categories if !self.filter.is_empty() => {
                    self.filter.clear();
                    self.cat = 0;
                    self.item = 0;
                    Outcome::None
                }
                Focus::Categories => Outcome::Close,
                _ => self.back(s),
            },
            KeyCode::Char('q') | KeyCode::Char('S') | KeyCode::F(2) => {
                self.revert = None;
                if self.focus == Focus::Values {
                    self.focus = Focus::Items;
                }
                Outcome::Close
            }
            KeyCode::Char('/') => {
                self.searching = true;
                self.filter.clear();
                self.cat = 0;
                self.item = 0;
                self.focus = Focus::Items;
                Outcome::None
            }
            KeyCode::Char(' ') if self.focus == Focus::Items => self.toggle(s, d, 1),
            KeyCode::Char('+') | KeyCode::Char('=') if self.focus == Focus::Items => {
                // `=` types a value on number fields; `+` steps them.
                match (key.code, self.current().map(Field::kind)) {
                    (KeyCode::Char('='), Some(Kind::Number)) => self.open_input(s),
                    _ => self.toggle(s, d, 1),
                }
            }
            KeyCode::Char('-') if self.focus == Focus::Items => self.toggle(s, d, -1),
            KeyCode::Char('i') if self.focus == Focus::Items => match self.current() {
                Some(f) if f.kind() == Kind::Number => self.open_input(s),
                _ => Outcome::None,
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

    /// h: go to the parent column. In the value column this undoes the
    /// live changes.
    fn back(&mut self, s: &mut Settings) -> Outcome {
        match self.focus {
            Focus::Values => {
                self.focus = Focus::Items;
                match self.revert.take() {
                    Some(old) if old != *s => {
                        *s = old;
                        Outcome::Changed
                    }
                    _ => Outcome::None,
                }
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
                        self.revert = Some(s.clone());
                        self.value = f.index(s);
                        self.focus = Focus::Values;
                        Outcome::None
                    }
                    Kind::Number => self.open_input(s),
                    Kind::Action => match f {
                        Field::Act(a) => Outcome::Action(a),
                        _ => Outcome::None,
                    },
                    Kind::Info => {
                        Outcome::Message(format!("{}: {}", f.label(), f.value(s, d)), false)
                    }
                }
            }
            Focus::Values => {
                // Keep the value.
                self.revert = None;
                self.focus = Focus::Items;
                Outcome::None
            }
        }
    }

    /// Space, + and -: toggle a bool, step a choice or a number.
    fn toggle(&mut self, s: &mut Settings, d: &DeviceInfo, dir: i64) -> Outcome {
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
            Kind::Number => {
                let v = f.number(s) + dir as f64 * f.step(s);
                let _ = f.set_number(s, v, "");
            }
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

    fn open_input(&mut self, s: &Settings) -> Outcome {
        if let Some(f) = self.current() {
            if f.kind() == Kind::Number {
                let v = f.number(s);
                // Start with the current value, so Backspace edits it.
                let text = match f {
                    Field::ChOffset(_) | Field::MathOffset => format!("{v:.1}"),
                    Field::TrigPosition => format!("{v:.0}"),
                    _ => fmt_si(v, f.unit()).replace(' ', ""),
                };
                self.input = Some(text);
                self.error = None;
            }
        }
        Outcome::None
    }

    fn input_key(&mut self, key: KeyEvent, s: &mut Settings) -> Outcome {
        match key.code {
            KeyCode::Esc => {
                self.input = None;
                self.error = None;
            }
            KeyCode::Backspace => {
                if let Some(t) = self.input.as_mut() {
                    t.pop();
                }
            }
            KeyCode::Char(c) => {
                if let Some(t) = self.input.as_mut() {
                    t.push(c);
                }
            }
            KeyCode::Enter => {
                let text = self.input.clone().unwrap_or_default();
                let Some(f) = self.current() else {
                    self.input = None;
                    return Outcome::None;
                };
                match parse_value(&text).and_then(|(v, unit)| f.set_number(s, v, &unit)) {
                    Ok(()) => {
                        self.input = None;
                        self.error = None;
                        return Outcome::Changed;
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            _ => {}
        }
        Outcome::None
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
        assert_eq!(p.groups()[p.cat].name, "Files");
        assert_eq!(key(&mut p, &mut s, KeyCode::Esc), Outcome::Close);
    }

    #[test]
    fn value_column_applies_live_and_esc_reverts() {
        let mut p = Panel::new();
        let mut s = Settings::default();
        goto(&mut p, &mut s, "Channel 1", "Coupling");
        key(&mut p, &mut s, KeyCode::Char('l'));
        assert_eq!(p.focus, Focus::Values);
        assert_eq!(key(&mut p, &mut s, KeyCode::Char('j')), Outcome::Changed);
        assert_eq!(s.channels[0].coupling, Coupling::Ac);
        // Esc puts the old value back.
        assert_eq!(key(&mut p, &mut s, KeyCode::Esc), Outcome::Changed);
        assert_eq!(s.channels[0].coupling, Coupling::Dc);
        assert_eq!(p.focus, Focus::Items);
        // Enter keeps the new value.
        key(&mut p, &mut s, KeyCode::Char('l'));
        key(&mut p, &mut s, KeyCode::Char('j'));
        key(&mut p, &mut s, KeyCode::Enter);
        assert_eq!(s.channels[0].coupling, Coupling::Ac);
        assert_eq!(p.focus, Focus::Items);
        // h after a change also reverts.
        key(&mut p, &mut s, KeyCode::Char('l'));
        key(&mut p, &mut s, KeyCode::Char('k'));
        assert_eq!(s.channels[0].coupling, Coupling::Dc);
        key(&mut p, &mut s, KeyCode::Char('h'));
        assert_eq!(s.channels[0].coupling, Coupling::Ac);
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
        key(&mut p, &mut s, KeyCode::Char('='));
        assert!(p.input.is_some());
        // Clear the start text, type a new value.
        for _ in 0..10 {
            key(&mut p, &mut s, KeyCode::Backspace);
        }
        keys(&mut p, &mut s, "500m");
        assert_eq!(key(&mut p, &mut s, KeyCode::Enter), Outcome::Changed);
        assert!((s.trigger.level - 0.5).abs() < 1e-12);
        assert!(p.input.is_none());
        // A bad value keeps the input open, with an error.
        key(&mut p, &mut s, KeyCode::Char('i'));
        for _ in 0..10 {
            key(&mut p, &mut s, KeyCode::Backspace);
        }
        keys(&mut p, &mut s, "abc");
        assert_eq!(key(&mut p, &mut s, KeyCode::Enter), Outcome::None);
        assert!(p.error.is_some() && p.input.is_some());
        key(&mut p, &mut s, KeyCode::Esc);
        assert!(p.input.is_none());
        assert!((s.trigger.level - 0.5).abs() < 1e-12);
        // Enter on a number field opens the input too.
        goto(&mut p, &mut s, "Trigger", "Position");
        key(&mut p, &mut s, KeyCode::Enter);
        assert_eq!(p.input.as_deref(), Some("50"));
        key(&mut p, &mut s, KeyCode::Backspace);
        key(&mut p, &mut s, KeyCode::Backspace);
        keys(&mut p, &mut s, "25%");
        key(&mut p, &mut s, KeyCode::Enter);
        assert!((s.trigger.position - 0.25).abs() < 1e-12);
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
        Field::CursorT(1).set_number(&mut s, 2e-3, "s").unwrap();
        assert!((s.cursors.t[1] - 2.0).abs() < 1e-9);
        assert_eq!(s.cursors.mode, CursorMode::Time);
        assert!((Field::CursorT(1).number(&s) - 2e-3).abs() < 1e-12);
        // CH1 at 1 V/div with its 0 V line at +2 div: 1 V is at +3 div.
        Field::CursorV(0).set_number(&mut s, 1.0, "V").unwrap();
        assert!((s.cursors.v[0] - 3.0).abs() < 1e-9);
        assert_eq!(s.cursors.mode, CursorMode::Volt);
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
        goto(&mut p, &mut s, "Timebase", "Autoset");
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
        for code in [KeyCode::Char('q'), KeyCode::Char('S'), KeyCode::F(2)] {
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
