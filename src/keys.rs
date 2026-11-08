//! Key bindings: one table from keys to actions for every view (scope,
//! log view, settings panel, help). The help screen, the key hints and
//! the `[keys]` table of the config file are made from the same table,
//! with the user's own bindings.
//!
//! Key syntax (in the config file): a single character (`a`, `A`, `+`,
//! `?`), or a name: `Enter` `Esc` `Tab` `Shift-Tab` `Space` `Backspace`
//! `Delete` `Insert` `F1`..`F12` `Up` `Down` `Left` `Right` `PgUp` `PgDn`
//! `Home` `End`. `Ctrl-x` and `Alt-x` add a modifier. Shift is part of
//! the character: `A` is Shift-a (`Shift-a` also works). Names are not
//! case sensitive.

use std::collections::HashMap;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Where an action works. Global actions work in the scope and the log
/// view. The log view also takes the scope keys its own keys leave free.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum View {
    Global,
    Scope,
    Log,
    Panel,
}

/// Every action a key can have.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Act {
    // Global.
    Quit,
    Help,
    Settings,
    Record,
    LogView,
    // Scope view.
    RunStop,
    Single,
    Autoset,
    NextTrace,
    PrevTrace,
    Ch1,
    Ch2,
    Math,
    VoltsSmaller,
    VoltsBigger,
    /// Fine volts/div, between the steps.
    VoltsSmallerFine,
    VoltsBiggerFine,
    TraceUp,
    TraceDown,
    TraceUpFine,
    TraceDownFine,
    /// A tenth of TraceUp / TraceDown / TraceUpFine / TraceDownFine.
    TraceUpAlt,
    TraceDownAlt,
    TraceUpFineAlt,
    TraceDownFineAlt,
    TraceCenter,
    Coupling,
    Probe,
    Invert,
    MathFunction,
    TimeSmaller,
    TimeBigger,
    /// Fine time/div, between the steps.
    TimeSmallerFine,
    TimeBiggerFine,
    TriggerMode,
    TriggerSource,
    TriggerEdge,
    TriggerUp,
    TriggerDown,
    TriggerToMiddle,
    TriggerLeft,
    TriggerRight,
    /// A tenth of the trigger level and position steps.
    TriggerUpFine,
    TriggerDownFine,
    TriggerLeftFine,
    TriggerRightFine,
    Spectrum,
    SpectrumZoom,
    FftWindow,
    Xy,
    MeasurePanel,
    Cursors,
    /// Move cursor `i` (0 time A, 1 time B, 2 voltage A, 3 voltage B) by
    /// a tenth of a division: false back / down, true on / up.
    CursorMove(usize, bool),
    /// The same, a hundredth of a division (fine).
    CursorFine(usize, bool),
    Averaging,
    Persistence,
    Dots,
    /// Trace mode: average / peak / both.
    TraceMode,
    Interpolation,
    CalFreqUp,
    CalFreqDown,
    ExportCsv,
    ExportJson,
    Reset,
    // Log view.
    LogBack,
    LogOn,
    LogPageBack,
    LogPageOn,
    LogStart,
    LogEnd,
    LogZoomIn,
    LogZoomOut,
    LogFit,
    LogNative,
    LogCursors,
    /// Move log cursor A (0) or B (1) back (false) or on (true).
    LogCursor(usize, bool),
    /// The same, a tenth as far (at least one log step).
    LogCursorFine(usize, bool),
    LogTimeLabels,
    LogExport,
    /// Log view trace mode: average / peak / both.
    LogTraceMode,
    // Settings panel.
    PanelDown,
    PanelUp,
    PanelBack,
    PanelEnter,
    PanelEscape,
    PanelClose,
    PanelSearch,
    PanelToggle,
    PanelNext,
    PanelPrevious,
    PanelType,
    /// Pressed twice: the first row.
    PanelTop,
    PanelBottom,
    PanelDeleteLog,
}

/// One action: its name in the config file, its view, its default keys
/// and a short text for the config file.
#[derive(Clone, Copy, Debug)]
pub struct ActionDef {
    pub act: Act,
    pub name: &'static str,
    pub view: View,
    pub keys: &'static [&'static str],
    pub help: &'static str,
}

const fn def(
    act: Act,
    name: &'static str,
    view: View,
    keys: &'static [&'static str],
    help: &'static str,
) -> ActionDef {
    ActionDef {
        act,
        name,
        view,
        keys,
        help,
    }
}

use Act::*;
use View::{Global as G, Log as L, Panel as P, Scope as S};

/// Every action with its default keys, in the order of the config file.
#[rustfmt::skip]
pub const ACTIONS: &[ActionDef] = &[
    def(Quit, "quit", G, &["q", "Esc"], "quit (saves the settings)"),
    def(Help, "help", G, &["?", "F1"], "key help"),
    def(Settings, "settings", G, &["s", "Enter", "F2"], "settings panel"),
    def(Record, "record", G, &["r"], "start / stop the envelope logger"),
    def(LogView, "log_view", G, &["V"], "log view on / off"),
    def(RunStop, "run_stop", S, &["Space"], "run / stop"),
    def(Single, "single", S, &["o"], "single shot: arm the trigger once"),
    def(Autoset, "autoset", S, &["a"], "autoset"),
    def(NextTrace, "next_trace", S, &["Tab"], "select the next trace"),
    def(PrevTrace, "previous_trace", S, &["Shift-Tab"], "select the previous trace"),
    def(Ch1, "ch1", S, &["1"], "CH1 on / off"),
    def(Ch2, "ch2", S, &["2"], "CH2 on / off"),
    def(Math, "math", S, &["3"], "MATH on / off"),
    def(VoltsSmaller, "volts_div_smaller", S, &["Up"], "smaller volts/div of the selected trace"),
    def(VoltsBigger, "volts_div_bigger", S, &["Down"], "bigger volts/div of the selected trace"),
    def(VoltsSmallerFine, "volts_div_smaller_fine", S, &["Alt-Up"], "volts/div a little smaller (between the steps)"),
    def(VoltsBiggerFine, "volts_div_bigger_fine", S, &["Alt-Down"], "volts/div a little bigger (between the steps)"),
    def(TraceUp, "trace_up", S, &["]"], "move the selected trace up"),
    def(TraceDown, "trace_down", S, &["["], "move the selected trace down"),
    def(TraceUpFine, "trace_up_fine", S, &["}"], "move the selected trace up a little"),
    def(TraceDownFine, "trace_down_fine", S, &["{"], "move the selected trace down a little"),
    def(TraceUpAlt, "trace_up_alt", S, &["Alt-]"], "move the selected trace up 0.05 div"),
    def(TraceDownAlt, "trace_down_alt", S, &["Alt-["], "move the selected trace down 0.05 div"),
    def(TraceUpFineAlt, "trace_up_fine_alt", S, &["Alt-}"], "move the selected trace up 0.01 div"),
    def(TraceDownFineAlt, "trace_down_fine_alt", S, &["Alt-{"], "move the selected trace down 0.01 div"),
    def(TraceCenter, "trace_center", S, &["0"], "selected trace to the center"),
    def(Coupling, "coupling", S, &["c"], "coupling DC / AC"),
    def(Probe, "probe", S, &["p"], "probe x1 / x10"),
    def(Invert, "invert", S, &["i"], "invert"),
    def(MathFunction, "math_function", S, &["M"], "next math function (turns MATH on)"),
    def(TimeSmaller, "time_div_smaller", S, &["Left"], "smaller time/div"),
    def(TimeBigger, "time_div_bigger", S, &["Right"], "bigger time/div"),
    def(TimeSmallerFine, "time_div_smaller_fine", S, &["Alt-Left"], "time/div a little smaller (between the steps)"),
    def(TimeBiggerFine, "time_div_bigger_fine", S, &["Alt-Right"], "time/div a little bigger (between the steps)"),
    def(TriggerMode, "trigger_mode", S, &["t"], "trigger mode: Auto / Normal / Single"),
    def(TriggerSource, "trigger_source", S, &["S"], "trigger source CH1 / CH2"),
    def(TriggerEdge, "trigger_edge", S, &["e"], "trigger edge: rising / falling / both"),
    def(TriggerUp, "trigger_level_up", S, &["+", "="], "trigger level up"),
    def(TriggerDown, "trigger_level_down", S, &["-", "_"], "trigger level down"),
    def(TriggerToMiddle, "trigger_to_middle", S, &["T"], "trigger level to the middle of the signal"),
    def(TriggerLeft, "trigger_position_left", S, &[",", "<"], "trigger position left"),
    def(TriggerRight, "trigger_position_right", S, &[".", ">"], "trigger position right"),
    def(TriggerUpFine, "trigger_level_up_fine", S, &["Alt-+", "Alt-="], "trigger level up, fine"),
    def(TriggerDownFine, "trigger_level_down_fine", S, &["Alt--", "Alt-_"], "trigger level down, fine"),
    def(TriggerLeftFine, "trigger_position_left_fine", S, &["Alt-,", "Alt-<"], "trigger position left, fine"),
    def(TriggerRightFine, "trigger_position_right_fine", S, &["Alt-.", "Alt->"], "trigger position right, fine"),
    def(Spectrum, "spectrum", S, &["f"], "spectrum (FFT) view"),
    def(SpectrumZoom, "spectrum_zoom", S, &["F"], "spectrum zoom (span / 2)"),
    def(FftWindow, "fft_window", S, &["w"], "FFT window"),
    def(Xy, "xy", S, &["x"], "XY mode"),
    def(MeasurePanel, "measure_panel", S, &["m"], "measurement panel"),
    def(Cursors, "cursors", S, &["v"], "hide / show the cursors"),
    def(CursorMove(0, false), "cursor_time_a_left", S, &["h"], "move time cursor A left"),
    def(CursorMove(0, true), "cursor_time_a_right", S, &["l"], "move time cursor A right"),
    def(CursorMove(1, false), "cursor_time_b_left", S, &["H"], "move time cursor B left"),
    def(CursorMove(1, true), "cursor_time_b_right", S, &["L"], "move time cursor B right"),
    def(CursorMove(2, false), "cursor_volt_a_down", S, &["j"], "move voltage cursor A down"),
    def(CursorMove(2, true), "cursor_volt_a_up", S, &["k"], "move voltage cursor A up"),
    def(CursorMove(3, false), "cursor_volt_b_down", S, &["J"], "move voltage cursor B down"),
    def(CursorMove(3, true), "cursor_volt_b_up", S, &["K"], "move voltage cursor B up"),
    def(CursorFine(0, false), "cursor_time_a_left_fine", S, &["Alt-h"], "move time cursor A left, fine"),
    def(CursorFine(0, true), "cursor_time_a_right_fine", S, &["Alt-l"], "move time cursor A right, fine"),
    def(CursorFine(1, false), "cursor_time_b_left_fine", S, &["Alt-H"], "move time cursor B left, fine"),
    def(CursorFine(1, true), "cursor_time_b_right_fine", S, &["Alt-L"], "move time cursor B right, fine"),
    def(CursorFine(2, false), "cursor_volt_a_down_fine", S, &["Alt-j"], "move voltage cursor A down, fine"),
    def(CursorFine(2, true), "cursor_volt_a_up_fine", S, &["Alt-k"], "move voltage cursor A up, fine"),
    def(CursorFine(3, false), "cursor_volt_b_down_fine", S, &["Alt-J"], "move voltage cursor B down, fine"),
    def(CursorFine(3, true), "cursor_volt_b_up_fine", S, &["Alt-K"], "move voltage cursor B up, fine"),
    def(Averaging, "averaging", S, &["A"], "averaging: 1 2 4 8 16 32 64"),
    def(Persistence, "persistence", S, &["P"], "persistence: 0 2 4 8 16 traces"),
    def(Dots, "dots", S, &["d"], "dots / lines"),
    def(TraceMode, "trace_mode", S, &["D"], "trace mode: average / peak (min..max area) / both"),
    def(Interpolation, "interpolation", S, &["z"], "sin(x)/x / linear interpolation"),
    def(CalFreqUp, "cal_freq_up", S, &["g"], "calibration output frequency up"),
    def(CalFreqDown, "cal_freq_down", S, &["G"], "calibration output frequency down"),
    def(ExportCsv, "export_csv", S, &["E"], "export the screen as CSV"),
    def(ExportJson, "export_json", S, &["W"], "export the screen as JSON"),
    def(Reset, "reset", S, &["R"], "reset all settings"),
    def(LogBack, "log_back", L, &["Left"], "log view: scroll one division back"),
    def(LogOn, "log_on", L, &["Right"], "log view: scroll one division on"),
    def(LogPageBack, "log_page_back", L, &["PgUp"], "log view: scroll one screen back"),
    def(LogPageOn, "log_page_on", L, &["PgDn"], "log view: scroll one screen on"),
    def(LogStart, "log_start", L, &["Home"], "log view: start of the log"),
    def(LogEnd, "log_end", L, &["End"], "log view: newest data (and follow it)"),
    def(LogZoomIn, "log_zoom_in", L, &["+", "="], "log view: zoom in"),
    def(LogZoomOut, "log_zoom_out", L, &["-"], "log view: zoom out"),
    def(LogFit, "log_fit", L, &["F"], "log view: the whole log"),
    def(LogNative, "log_native", L, &["n"], "log view: one step per dot column"),
    def(LogCursors, "log_cursors", L, &["v"], "log view: time cursors on / off"),
    def(LogCursor(0, false), "log_cursor_a_left", L, &["h"], "log view: move cursor A left"),
    def(LogCursor(0, true), "log_cursor_a_right", L, &["l"], "log view: move cursor A right"),
    def(LogCursor(1, false), "log_cursor_b_left", L, &["H"], "log view: move cursor B left"),
    def(LogCursor(1, true), "log_cursor_b_right", L, &["L"], "log view: move cursor B right"),
    def(LogCursorFine(0, false), "log_cursor_a_left_fine", L, &["Alt-h"], "log view: move cursor A left, fine"),
    def(LogCursorFine(0, true), "log_cursor_a_right_fine", L, &["Alt-l"], "log view: move cursor A right, fine"),
    def(LogCursorFine(1, false), "log_cursor_b_left_fine", L, &["Alt-H"], "log view: move cursor B left, fine"),
    def(LogCursorFine(1, true), "log_cursor_b_right_fine", L, &["Alt-L"], "log view: move cursor B right, fine"),
    def(LogTimeLabels, "log_time_labels", L, &["t"], "log view: clock / seconds time labels"),
    def(LogExport, "log_export", L, &["E", "e"], "log view: export CSV (A to B with cursors)"),
    def(LogTraceMode, "log_trace_mode", L, &["D"], "log view: average / peak (min..max area) / both"),
    def(PanelDown, "panel_down", P, &["j", "Down"], "panel: move down (in numbers: step down)"),
    def(PanelUp, "panel_up", P, &["k", "Up"], "panel: move up (in numbers: step up)"),
    def(PanelBack, "panel_back", P, &["h", "Left"], "panel: back one column"),
    def(PanelEnter, "panel_enter", P, &["l", "Right", "Enter"], "panel: into the next column, run"),
    def(PanelEscape, "panel_escape", P, &["Esc"], "panel: back; at the left clear search, then close"),
    def(PanelClose, "panel_close", P, &["q", "s", "F2"], "panel: close (changes stay)"),
    def(PanelSearch, "panel_search", P, &["/"], "panel: search all settings"),
    def(PanelToggle, "panel_toggle", P, &["Space"], "panel: toggle, next value, cursor on/off"),
    def(PanelNext, "panel_next", P, &["+"], "panel: next value, number step up"),
    def(PanelPrevious, "panel_previous", P, &["-"], "panel: previous value, number step down"),
    def(PanelType, "panel_type", P, &["=", "i"], "panel: type a number (from its value)"),
    def(PanelTop, "panel_top", P, &["g"], "panel: first row (press twice)"),
    def(PanelBottom, "panel_bottom", P, &["G"], "panel: last row"),
    def(PanelDeleteLog, "delete_log", P, &["d"], "panel: delete the selected log in Logger › Open log (asks y / n)"),
];

/// The definition of an action.
pub fn action_def(a: Act) -> &'static ActionDef {
    ACTIONS
        .iter()
        .find(|d| d.act == a)
        .expect("every action is in ACTIONS")
}

/// One key: a key code and Ctrl / Alt. Shift is in the character (or in
/// `BackTab` for Shift-Tab).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeySpec {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl KeySpec {
    pub fn matches(&self, ev: &KeyEvent) -> bool {
        let mods = ev.modifiers & (KeyModifiers::CONTROL | KeyModifiers::ALT);
        let code = match ev.code {
            // Some terminals send Shift-letter as a lowercase char with
            // the Shift modifier.
            KeyCode::Char(c) if ev.modifiers.contains(KeyModifiers::SHIFT) => {
                KeyCode::Char(c.to_ascii_uppercase())
            }
            c => c,
        };
        self.code == code && self.mods == mods
    }
}

const NAMED: [(&str, KeyCode); 16] = [
    ("Enter", KeyCode::Enter),
    ("Esc", KeyCode::Esc),
    ("Tab", KeyCode::Tab),
    ("Shift-Tab", KeyCode::BackTab),
    ("Space", KeyCode::Char(' ')),
    ("Backspace", KeyCode::Backspace),
    ("Delete", KeyCode::Delete),
    ("Insert", KeyCode::Insert),
    ("Up", KeyCode::Up),
    ("Down", KeyCode::Down),
    ("Left", KeyCode::Left),
    ("Right", KeyCode::Right),
    ("PgUp", KeyCode::PageUp),
    ("PgDn", KeyCode::PageDown),
    ("Home", KeyCode::Home),
    ("End", KeyCode::End),
];

/// Other names that also parse.
const ALIASES: [(&str, KeyCode); 6] = [
    ("Escape", KeyCode::Esc),
    ("Return", KeyCode::Enter),
    ("BackTab", KeyCode::BackTab),
    ("PageUp", KeyCode::PageUp),
    ("PageDown", KeyCode::PageDown),
    ("Del", KeyCode::Delete),
];

/// Parse a key like `a`, `A`, `Enter`, `F5`, `Ctrl-x`, `Alt-Left`.
pub fn parse_key(text: &str) -> Result<KeySpec, String> {
    let mut chars = text.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Ok(KeySpec {
            code: KeyCode::Char(c),
            mods: KeyModifiers::NONE,
        });
    }
    let t = text.trim();
    let mut chars = t.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Ok(KeySpec {
            code: KeyCode::Char(c),
            mods: KeyModifiers::NONE,
        });
    }
    if t.is_empty() {
        return Err("empty key".into());
    }
    let lower = t.to_ascii_lowercase();
    for (name, code) in NAMED.iter().chain(&ALIASES) {
        if lower == name.to_ascii_lowercase() {
            return Ok(KeySpec {
                code: *code,
                mods: KeyModifiers::NONE,
            });
        }
    }
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        if (1..=12).contains(&n) {
            return Ok(KeySpec {
                code: KeyCode::F(n),
                mods: KeyModifiers::NONE,
            });
        }
    }
    // Modifiers: the text before the first `-` (the key may be `-`).
    if let Some((m, rest)) = t.split_once('-') {
        if !rest.is_empty() {
            let mut k = parse_key(rest)?;
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "c" | "control" => k.mods |= KeyModifiers::CONTROL,
                "alt" | "a" | "meta" | "m" => k.mods |= KeyModifiers::ALT,
                "shift" | "s" => match k.code {
                    KeyCode::Char(c) => k.code = KeyCode::Char(c.to_ascii_uppercase()),
                    KeyCode::Tab => k.code = KeyCode::BackTab,
                    _ => return Err(format!("Shift works only with characters and Tab: {t}")),
                },
                _ => return Err(format!("unknown modifier {m} in {t}")),
            }
            return Ok(k);
        }
    }
    Err(format!("unknown key: {t}"))
}

/// The key in config file syntax, like `Space`, `Ctrl-c`, `F2`, `a`.
pub fn key_name(k: &KeySpec) -> String {
    let base = match k.code {
        KeyCode::Char(c) if c != ' ' => c.to_string(),
        KeyCode::F(n) => format!("F{n}"),
        code => NAMED
            .iter()
            .find(|(_, c)| *c == code)
            .map_or_else(|| format!("{code:?}"), |(n, _)| n.to_string()),
    };
    let mut out = String::new();
    if k.mods.contains(KeyModifiers::CONTROL) {
        out.push_str("Ctrl-");
    }
    if k.mods.contains(KeyModifiers::ALT) {
        out.push_str("Alt-");
    }
    out + &base
}

/// The key as shown on screen: arrows as `←`, Space as `space`.
pub fn key_label(k: &KeySpec) -> String {
    let arrow = match k.code {
        KeyCode::Up => Some("↑"),
        KeyCode::Down => Some("↓"),
        KeyCode::Left => Some("←"),
        KeyCode::Right => Some("→"),
        KeyCode::Char(' ') => Some("space"),
        KeyCode::BackTab => Some("S-Tab"),
        _ => None,
    };
    match arrow {
        Some(a) if k.mods.is_empty() => a.to_string(),
        _ => key_name(k),
    }
}

/// The bindings in use: the defaults with the user's `[keys]` table.
#[derive(Clone, Debug)]
pub struct KeyMap {
    keys: HashMap<Act, Vec<KeySpec>>,
}

impl Default for KeyMap {
    fn default() -> Self {
        let keys = ACTIONS
            .iter()
            .map(|d| {
                let k = d.keys.iter().map(|k| parse_key(k).expect("default key"));
                (d.act, k.collect())
            })
            .collect();
        Self { keys }
    }
}

/// Views that are on screen together: a key may mean only one action in
/// each group.
const VIEW_GROUPS: [&[View]; 3] = [
    &[View::Global, View::Scope],
    &[View::Global, View::Log],
    &[View::Panel],
];

impl KeyMap {
    /// The defaults with `table` (action name → key or list of keys) on
    /// top. Returns the warnings: unknown actions, bad keys, and keys bound
    /// to two actions of one view (the first in the table wins; actions
    /// not in the table come after the ones in it).
    pub fn from_table(table: &[(String, Vec<String>)]) -> (Self, Vec<String>) {
        let mut map = KeyMap::default();
        let mut warnings = Vec::new();
        let mut order: Vec<Act> = Vec::new();
        for (name, keys) in table {
            let Some(d) = ACTIONS.iter().find(|d| d.name == name) else {
                warnings.push(format!("unknown action keys.{name}"));
                continue;
            };
            let mut parsed = Vec::new();
            let mut ok = true;
            for k in keys {
                match parse_key(k) {
                    Ok(k) => parsed.push(k),
                    Err(e) => {
                        warnings.push(format!("keys.{name}: {e}; using the default"));
                        ok = false;
                    }
                }
            }
            if ok {
                map.keys.insert(d.act, parsed);
                if !order.contains(&d.act) {
                    order.push(d.act);
                }
            }
        }
        for d in ACTIONS {
            if !order.contains(&d.act) {
                order.push(d.act);
            }
        }
        // Conflicts: walk the actions in order; a key taken earlier in a
        // view group is dropped from the later action.
        for group in VIEW_GROUPS {
            let mut taken: HashMap<KeySpec, Act> = HashMap::new();
            for &a in &order {
                let d = action_def(a);
                if !group.contains(&d.view) {
                    continue;
                }
                let keys = map.keys.get_mut(&a).expect("every action has keys");
                keys.retain(|k| match taken.get(k) {
                    Some(first) if *first != a => {
                        let msg = format!(
                            "key {} is bound to {} and {}; {} keeps it",
                            key_name(k),
                            action_def(*first).name,
                            d.name,
                            action_def(*first).name
                        );
                        if !warnings.contains(&msg) {
                            warnings.push(msg);
                        }
                        false
                    }
                    _ => {
                        taken.insert(*k, a);
                        true
                    }
                });
            }
        }
        (map, warnings)
    }

    /// The keys of an action.
    pub fn keys(&self, a: Act) -> &[KeySpec] {
        self.keys.get(&a).map_or(&[], |v| v.as_slice())
    }

    /// True when `ev` is a key of `a`.
    pub fn is(&self, a: Act, ev: &KeyEvent) -> bool {
        self.keys(a).iter().any(|k| k.matches(ev))
    }

    /// The action of `ev` in the first of `views` that has one.
    pub fn action(&self, views: &[View], ev: &KeyEvent) -> Option<Act> {
        views.iter().find_map(|v| {
            ACTIONS
                .iter()
                .filter(|d| d.view == *v)
                .find(|d| self.is(d.act, ev))
                .map(|d| d.act)
        })
    }

    /// The keys of some actions for the screen, like `h l` or `S F2`.
    pub fn label(&self, acts: &[Act]) -> String {
        let mut out: Vec<String> = Vec::new();
        for a in acts {
            for k in self.keys(*a) {
                let l = key_label(k);
                if !out.contains(&l) {
                    out.push(l);
                }
            }
        }
        if out.is_empty() {
            "--".into()
        } else {
            out.join(" ")
        }
    }

    /// The first key of an action for a hint, like `s`.
    pub fn first(&self, a: Act) -> String {
        self.keys(a).first().map_or("--".into(), key_label)
    }
}

/// Help rows of the scope view: actions and what they do.
#[rustfmt::skip]
pub const SCOPE_HELP: &[(&[Act], &str)] = &[
    (&[Quit], "quit (saves settings)"),
    (&[Help], "this help"),
    (&[Settings], "settings panel (h j k l, / search, type numbers)"),
    (&[RunStop], "run / stop"),
    (&[Single], "single shot: arm the trigger once"),
    (&[Autoset], "autoset"),
    (&[NextTrace, PrevTrace], "select CH1 / CH2 / MATH"),
    (&[Ch1, Ch2, Math], "CH1 / CH2 / MATH on or off"),
    (&[VoltsSmaller, VoltsBigger], "volts/div of the selected trace"),
    (&[VoltsSmallerFine, VoltsBiggerFine], "volts/div between the steps (fine)"),
    (&[TraceDown, TraceUp], "move the selected trace down / up"),
    (&[TraceDownFine, TraceUpFine], "move the selected trace a little"),
    (&[TraceCenter], "selected trace to the center"),
    (&[Coupling], "coupling DC / AC"),
    (&[Probe], "probe x1 / x10"),
    (&[Invert], "invert"),
    (&[MathFunction], "next math function (turns MATH on)"),
    (&[TimeSmaller, TimeBigger], "time/div"),
    (&[TimeSmallerFine, TimeBiggerFine], "time/div between the steps (fine)"),
    (&[TriggerMode], "trigger mode: Auto / Normal / Single"),
    (&[TriggerSource], "trigger source CH1 / CH2"),
    (&[TriggerEdge], "trigger edge: rising / falling / both"),
    (&[TriggerUp, TriggerDown], "trigger level"),
    (&[TriggerToMiddle], "trigger level to the middle of the signal"),
    (&[TriggerLeft, TriggerRight], "trigger position left / right"),
    (&[TraceDownAlt, TraceUpAlt, TraceDownFineAlt, TraceUpFineAlt], "move the selected trace, fine"),
    (&[TriggerDownFine, TriggerUpFine], "trigger level, fine"),
    (&[TriggerLeftFine, TriggerRightFine], "trigger position, fine"),
    (&[Spectrum], "spectrum (FFT) view"),
    (&[SpectrumZoom], "spectrum zoom (span / 2)"),
    (&[FftWindow], "FFT window"),
    (&[Xy], "XY mode"),
    (&[MeasurePanel], "measurement panel"),
    (&[Cursors], "hide / show the cursors (Settings › Cursors)"),
    (&[CursorMove(0, false), CursorMove(0, true)], "move time cursor A (when on)"),
    (&[CursorMove(1, false), CursorMove(1, true)], "move time cursor B (when on)"),
    (&[CursorMove(2, false), CursorMove(2, true)], "move voltage cursor A (when on)"),
    (&[CursorMove(3, false), CursorMove(3, true)], "move voltage cursor B (when on)"),
    (&[CursorFine(0, false), CursorFine(0, true)], "move time cursor A, fine"),
    (&[CursorFine(1, false), CursorFine(1, true)], "move time cursor B, fine"),
    (&[CursorFine(2, false), CursorFine(2, true)], "move voltage cursor A, fine"),
    (&[CursorFine(3, false), CursorFine(3, true)], "move voltage cursor B, fine"),
    (&[Averaging], "averaging: 1 2 4 8 16 32 64"),
    (&[Persistence], "persistence: 0 2 4 8 16 traces"),
    (&[Dots], "dots / lines"),
    (&[TraceMode], "trace mode: average / peak (min..max area) / both"),
    (&[Interpolation], "sin(x)/x / linear interpolation"),
    (&[CalFreqUp, CalFreqDown], "calibration output frequency up / down"),
    (&[ExportCsv], "export the screen as CSV"),
    (&[ExportJson], "export the screen as JSON"),
    (&[Reset], "reset all settings"),
    (&[Record], "start / stop the envelope logger"),
    (&[LogView], "log view (strip chart of the log)"),
];

/// Help rows of the log view.
#[rustfmt::skip]
pub const LOG_HELP: &[(&[Act], &str)] = &[
    (&[Record], "start / stop recording (in every view)"),
    (&[LogView], "log view on / off (in every view)"),
    (&[LogBack, LogOn], "scroll one division back / on"),
    (&[LogPageBack, LogPageOn], "scroll one screen"),
    (&[LogStart, LogEnd], "start of the log / newest (follow)"),
    (&[LogZoomIn, LogZoomOut], "zoom in / out"),
    (&[LogFit, LogNative], "whole log / one step per dot column"),
    (&[LogCursors], "time cursors on / off"),
    (&[LogCursor(0, false), LogCursor(0, true)], "move cursor A"),
    (&[LogCursor(1, false), LogCursor(1, true)], "move cursor B"),
    (&[LogCursorFine(0, false), LogCursorFine(0, true)], "move cursor A, fine"),
    (&[LogCursorFine(1, false), LogCursorFine(1, true)], "move cursor B, fine"),
    (&[LogTimeLabels], "time labels: clock / seconds"),
    (&[LogExport], "export CSV (A–B when the cursors are on)"),
    (&[LogTraceMode], "average / peak (min..max area) / both"),
];

/// Scope keys that also work in the log view, for its help.
#[rustfmt::skip]
pub const LOG_SCOPE_HELP: &[(&[Act], &str)] = &[
    (&[Quit], "quit (saves settings)"),
    (&[Help], "this help"),
    (&[Settings], "settings panel"),
    (&[NextTrace, PrevTrace], "select CH1 / CH2"),
    (&[Ch1, Ch2], "CH1 / CH2 on or off"),
    (&[VoltsSmaller, VoltsBigger], "volts/div of the selected trace"),
    (&[TraceDown, TraceUp], "move the selected trace down / up"),
    (&[TraceDownFine, TraceUpFine], "move the selected trace a little"),
    (&[TraceCenter], "selected trace to the center"),
];

/// Help rows with the user's keys. Alt keys are not listed one by one
/// (see [`ALT_NOTE`]): a row whose keys are all Alt keys is left out,
/// and Alt keys are left out of the other rows.
pub fn help_rows(map: &KeyMap, rows: &[(&[Act], &str)]) -> Vec<(String, String)> {
    rows.iter()
        .filter_map(|(acts, text)| {
            let all: Vec<KeySpec> = acts.iter().flat_map(|a| map.keys(*a).to_vec()).collect();
            let plain: Vec<String> = all
                .iter()
                .filter(|k| !k.mods.contains(KeyModifiers::ALT))
                .map(key_label)
                .fold(Vec::new(), |mut v, l| {
                    if !v.contains(&l) {
                        v.push(l);
                    }
                    v
                });
            match (all.is_empty(), plain.is_empty()) {
                (true, _) => Some(("--".into(), text.to_string())),
                (false, true) => None,
                (false, false) => Some((plain.join(" "), text.to_string())),
            }
        })
        .collect()
}

/// The help row for all Alt keys: Alt with a move or step key moves or
/// steps finely.
pub const ALT_NOTE: (&str, &str) = (
    "Alt + key",
    "= fine step of that move/step key (cursors, volts/div, time/div, position, level)",
);

/// The Alt row of the log view help.
pub const LOG_ALT_NOTE: (&str, &str) = ("Alt + key", "= fine step of that cursor key");

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn every_default_key_parses_and_names_round_trip() {
        for d in ACTIONS {
            for k in d.keys {
                let spec = parse_key(k).unwrap();
                assert_eq!(parse_key(&key_name(&spec)).unwrap(), spec, "{k}");
            }
        }
        // Names are unique.
        for (i, d) in ACTIONS.iter().enumerate() {
            assert!(ACTIONS[i + 1..]
                .iter()
                .all(|e| e.name != d.name && e.act != d.act));
        }
    }

    #[test]
    fn key_syntax() {
        let k = |t: &str| parse_key(t).unwrap();
        let none = KeyModifiers::NONE;
        assert_eq!(
            k("a"),
            KeySpec {
                code: KeyCode::Char('a'),
                mods: none
            }
        );
        assert_eq!(k("A").code, KeyCode::Char('A'));
        assert_eq!(k("Shift-a").code, KeyCode::Char('A'));
        assert_eq!(k("+").code, KeyCode::Char('+'));
        assert_eq!(k("-").code, KeyCode::Char('-'));
        assert_eq!(k("space").code, KeyCode::Char(' '));
        assert_eq!(k("Space").code, KeyCode::Char(' '));
        assert_eq!(k("ENTER").code, KeyCode::Enter);
        assert_eq!(k("Esc").code, KeyCode::Esc);
        assert_eq!(k("Tab").code, KeyCode::Tab);
        assert_eq!(k("Shift-Tab").code, KeyCode::BackTab);
        assert_eq!(k("F1").code, KeyCode::F(1));
        assert_eq!(k("f12").code, KeyCode::F(12));
        for (t, c) in [
            ("Up", KeyCode::Up),
            ("Down", KeyCode::Down),
            ("Left", KeyCode::Left),
            ("Right", KeyCode::Right),
            ("PgUp", KeyCode::PageUp),
            ("PgDn", KeyCode::PageDown),
            ("Home", KeyCode::Home),
            ("End", KeyCode::End),
        ] {
            assert_eq!(k(t).code, c, "{t}");
        }
        let c = k("Ctrl-x");
        assert_eq!(
            (c.code, c.mods),
            (KeyCode::Char('x'), KeyModifiers::CONTROL)
        );
        let a = k("Alt-Left");
        assert_eq!((a.code, a.mods), (KeyCode::Left, KeyModifiers::ALT));
        assert_eq!(k("Ctrl--").code, KeyCode::Char('-'));
        assert_eq!(
            k("ctrl-alt-q").mods,
            KeyModifiers::CONTROL | KeyModifiers::ALT
        );
        assert!(parse_key("F13").is_err());
        assert!(parse_key("Hyper-x").is_err());
        assert!(parse_key("nothing").is_err());
        assert!(parse_key("").is_err());
        assert_eq!(key_name(&k("ctrl-x")), "Ctrl-x");
        assert_eq!(key_name(&k(" ")), "Space");
    }

    #[test]
    fn default_keys_map_to_actions() {
        let m = KeyMap::default();
        let c = |ch| ev(KeyCode::Char(ch), KeyModifiers::NONE);
        let scope = [View::Global, View::Scope];
        let log = [View::Global, View::Log, View::Scope];
        assert_eq!(m.action(&scope, &c('s')), Some(Settings));
        assert_eq!(
            m.action(&scope, &ev(KeyCode::Enter, KeyModifiers::NONE)),
            Some(Settings)
        );
        assert_eq!(m.action(&scope, &c('S')), Some(TriggerSource));
        assert_eq!(m.action(&scope, &c('r')), Some(Record));
        assert_eq!(m.action(&scope, &c('h')), Some(CursorMove(0, false)));
        assert_eq!(m.action(&log, &c('h')), Some(LogCursor(0, false)));
        // The log view takes the scope keys it leaves free.
        assert_eq!(m.action(&log, &c('1')), Some(Ch1));
        // Ctrl and Alt must match.
        let ctrl = ev(KeyCode::Char('r'), KeyModifiers::CONTROL);
        assert_eq!(m.action(&scope, &ctrl), None);
        // Shift reported as a modifier still gives the capital.
        let shift = ev(KeyCode::Char('s'), KeyModifiers::SHIFT);
        assert_eq!(m.action(&scope, &shift), Some(TriggerSource));
        let backtab = ev(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(m.action(&scope, &backtab), Some(PrevTrace));
        // No key twice in a view.
        let (_, warnings) = KeyMap::from_table(&[]);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn table_rebinds_and_warns() {
        let t = |a: &str, k: &[&str]| (a.to_string(), k.iter().map(|s| s.to_string()).collect());
        let (m, w) = KeyMap::from_table(&[t("settings", &["o"]), t("run_stop", &["Ctrl-r", "F5"])]);
        let c = |ch| ev(KeyCode::Char(ch), KeyModifiers::NONE);
        let scope = [View::Global, View::Scope];
        assert_eq!(m.action(&scope, &c('o')), Some(Settings));
        assert_eq!(m.action(&scope, &c('s')), None);
        assert_eq!(
            m.action(&scope, &ev(KeyCode::Char('r'), KeyModifiers::CONTROL)),
            Some(RunStop)
        );
        assert_eq!(m.action(&scope, &c(' ')), None);
        // `o` was single's: settings came first, single lost it.
        assert_eq!(m.keys(Single), &[]);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(
            w[0].contains("key o is bound to settings and single"),
            "{w:?}"
        );
        // Bad and unknown entries warn and keep the defaults.
        let (m, w) = KeyMap::from_table(&[t("autoset", &["Hyper-a"]), t("fly", &["y"])]);
        assert_eq!(m.action(&scope, &c('a')), Some(Autoset));
        assert_eq!(w.len(), 2, "{w:?}");
        // The same key in the log view and the scope view is fine.
        let (_, w) = KeyMap::from_table(&[t("log_fit", &["x"])]);
        assert!(w.is_empty(), "{w:?}");
        // An empty list unbinds.
        let (m, w) = KeyMap::from_table(&[t("xy", &[])]);
        assert!(w.is_empty() && m.keys(Xy).is_empty());
    }

    #[test]
    fn help_uses_the_bindings() {
        let (m, _) = KeyMap::from_table(&[("help".into(), vec!["F10".into()])]);
        let rows = help_rows(&m, SCOPE_HELP);
        assert_eq!(rows[1].0, "F10");
        let rows = help_rows(&KeyMap::default(), SCOPE_HELP);
        assert_eq!(rows[2].0, "s Enter F2");
        assert!(rows.iter().any(|(k, _)| k == "h l"));
        assert!(rows.iter().any(|(k, _)| k == "← →"));
    }
}
