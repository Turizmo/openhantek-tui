//! The config file: every setting and the key bindings, as TOML with a
//! comment for each line, in `~/.config/openhantek-tui/config.toml`.
//!
//! - A missing file is written at start, with every setting and every key
//!   action. When it is missing but the old `settings.toml` is there, its
//!   values are taken once (the old file is left as it is).
//! - Changes made in the app are written back in place with `toml_edit`:
//!   only the values that changed, so the user's comments, order and
//!   formatting stay.
//! - A file with bad entries still loads: each bad or unknown entry gives
//!   a warning and its default, and the file is changed only when a
//!   setting is changed in the app (and then only that value). A file that
//!   is not TOML at all is never written.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use toml_edit::{DocumentMut, Item, Table};

use crate::keys::{KeyMap, ACTIONS};
use crate::settings::Settings;

/// `$XDG_CONFIG_HOME/openhantek-tui`, or `~/.config/openhantek-tui`.
pub fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir).join("openhantek-tui"));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/openhantek-tui"))
}

/// Default config file.
pub fn config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("config.toml"))
}

/// The settings file of older versions, read once when there is no
/// config file yet.
pub fn old_settings_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("settings.toml"))
}

/// A comment for every setting, by path (array indexes left out).
#[rustfmt::skip]
pub const COMMENTS: &[(&str, &str)] = &[
    ("time_idx", "time/div: index into 20ns 50ns 100ns ... 1s 2s 5s 10s (0..26, 14 = 1 ms)"),
    ("time_fine", "fine time/div (Alt-← →): factor on the time_idx step, 1 = the step"),
    ("cal_freq", "calibration output frequency in Hz (32 .. 100000, fixed steps)"),
    ("selected", "selected trace: 0 = CH1, 1 = CH2, 2 = MATH"),
    ("channels.enabled", "channel on / off"),
    ("channels.volt_idx", "volts/div: index into 20mV 50mV 100mV 200mV 500mV 1V 2V 5V 10V (x probe)"),
    ("channels.offset", "height of the 0 V line, in divisions (-4 .. 4)"),
    ("channels.coupling", "\"Dc\" or \"Ac\" (AC is done in software)"),
    ("channels.probe", "probe factor: 1 or 10"),
    ("channels.invert", "turn the trace upside down"),
    ("channels.volts_fine", "fine volts/div (Alt-↑ ↓): factor on the volt_idx step, 1 = the step"),
    ("math.enabled", "math trace on / off"),
    ("math.mode", "Add Sub SubRev Mul Div Avg NegCh1 NegCh2"),
    ("math.volt_idx", "units/div: index like channels.volt_idx"),
    ("math.offset", "height of the math 0 line, in divisions"),
    ("math.units_fine", "fine units/div: factor on the volt_idx step, 1 = the step"),
    ("trigger.mode", "\"Auto\", \"Normal\" or \"Single\""),
    ("trigger.source", "0 = CH1, 1 = CH2"),
    ("trigger.slope", "\"Rising\", \"Falling\" or \"Both\""),
    ("trigger.level", "trigger level in volts at the probe tip"),
    ("trigger.position", "trigger point on the screen: 0 = left edge, 1 = right edge"),
    ("cursors.hidden", "all cursors hidden (key v); each keeps its own enabled"),
    ("cursors.time_a.enabled", "time cursor A on / off"),
    ("cursors.time_a.position", "divisions from the screen center (-5 .. 5)"),
    ("cursors.time_a.style", "\"Classic\", \"Fine\" (fine dashes), \"Solid\" or \"Dotted\""),
    ("cursors.time_a.color", "Grey White Yellow Cyan Magenta Green Red Blue"),
    ("cursors.time_b.enabled", "time cursor B on / off"),
    ("cursors.time_b.position", "divisions from the screen center (-5 .. 5)"),
    ("cursors.time_b.style", "\"Classic\", \"Fine\", \"Solid\" or \"Dotted\""),
    ("cursors.time_b.color", "Grey White Yellow Cyan Magenta Green Red Blue"),
    ("cursors.volt_a.enabled", "voltage cursor A on / off"),
    ("cursors.volt_a.position", "divisions from the screen center (-4 .. 4)"),
    ("cursors.volt_a.style", "\"Classic\", \"Fine\", \"Solid\" or \"Dotted\""),
    ("cursors.volt_a.color", "Grey White Yellow Cyan Magenta Green Red Blue"),
    ("cursors.volt_b.enabled", "voltage cursor B on / off"),
    ("cursors.volt_b.position", "divisions from the screen center (-4 .. 4)"),
    ("cursors.volt_b.style", "\"Classic\", \"Fine\", \"Solid\" or \"Dotted\""),
    ("cursors.volt_b.color", "Grey White Yellow Cyan Magenta Green Red Blue"),
    ("display.spectrum", "spectrum (FFT) below the time view"),
    ("display.spectrum_zoom", "spectrum span = Nyquist / 2^zoom (0 .. 8)"),
    ("display.window", "FFT window: Rectangular Hann Hamming Blackman FlatTop"),
    ("display.xy", "XY mode: CH1 on x, CH2 on y"),
    ("display.interpolation", "\"Sinc\" (sin(x)/x) or \"Linear\""),
    ("display.averaging", "captures to average: 1 (off) 2 4 8 16 32 64"),
    ("display.persistence", "old traces kept on screen: 0 (off) 2 4 8 16"),
    ("display.dots", "draw only the samples"),
    ("display.measurements", "measurement panel on the right"),
    ("display.colors", "Auto TrueColor Ansi256 Ansi16"),
    ("display.trace_style", "CH1, CH2, MATH: BrailleThin BrailleBold Line Box"),
    ("display.trace_mode", "\"Average\" (mean line), \"Peak\" (min..max area) or \"Both\" (line over muted area)"),
    ("display.grid_style", "Dots Dashed Solid Crosses None"),
    ("display.grid_extent", "\"Points\" (marks at crossings) or \"Full\" (whole lines)"),
    ("display.grid_brightness", "grid brightness in percent: 10 .. 100 in steps of 10"),
    ("display.axis_style", "center cross (axes): Dots Dashed Solid Crosses None"),
    ("display.axis_extent", "\"Points\" (marks at divisions and ticks) or \"Full\" (whole lines)"),
    ("display.axis_brightness", "center cross brightness in percent: 10 .. 100 in steps of 10"),
    ("acquisition.max_adc_rate", "ADC rate cap in S/s with both channels, for every mode: 16000000 (CH1 alone 30 MS/s), 12000000 (CH1 alone 24 MS/s), 10000000 5000000 2000000 1000000 500000 400000"),
    ("logger.step", "log step in seconds (5e-7 .. 60, 1-2-5 steps); 0 = raw: one ADC sample per step (62.5 ns, CH1 alone 33.3 ns)"),
    ("logger.log_limit", "stop the log at this file size in bytes, 0 = no limit"),
    ("logger.log_dir", "log folder (default ~/.local/share/openhantek-tui/logs)"),
    ("logger.time_axis", "log view time labels: \"Clock\" (UTC) or \"Seconds\""),
    ("logger.csv_time", "log CSV time column: \"Iso\" (UTC) or \"Seconds\""),
    ("logger.trace_mode", "log view: \"Average\" (mean line), \"Peak\" (min..max area) or \"Both\""),
    ("calibration.zero_ch1", "CH1 ADC count of 0 V per gain (20m 50m 100m 200m 500m 1 2 5 V/div), ADC below 30 MS/s; 0 = EEPROM value"),
    ("calibration.zero_ch2", "CH2 ADC count of 0 V per gain, like zero_ch1; 0 = EEPROM value"),
    ("calibration.zero_ch1_30m", "CH1 alone at 30 MS/s: ADC count of 0 V per gain; 0 = EEPROM value"),
    ("calibration.gain", "gain correction of CH1, CH2 from the calibration output (0.7 .. 1.3); 1 = none"),
];

/// Comments above the tables.
#[rustfmt::skip]
const TABLE_COMMENTS: &[(&str, &str)] = &[
    ("channels", "One [[channels]] table per channel: CH1, then CH2."),
    ("math", "The math trace."),
    ("trigger", "Software trigger."),
    ("cursors", "Four cursors: time A and B, voltage A and B (volts of the selected trace)."),
    ("display", "How the screen looks."),
    ("acquisition", "How the scope samples."),
    ("logger", "Envelope logger (chart recorder)."),
    ("calibration", "The app's own calibration (Settings › Calibration). Wins over the EEPROM, which is never written."),
];

/// Settings that are optional: missing from a file of defaults.
const OPTIONAL: &[&str] = &["logger.log_dir"];

const HEADER: &str = "\
# openhantek-tui config. Every setting, then the key bindings.
# The app writes changed values back in place: your comments stay.
# A bad entry gives a warning and its default.

";

const KEYS_HEADER: &str = "\
Key bindings: action = key or [keys]. A single character (a A + ?), or
Enter Esc Tab Shift-Tab Space Backspace Delete Insert F1..F12 Up Down Left
Right PgUp PgDn Home End, with Ctrl-x or Alt-x. Shift is in the character
(A is Shift-a). [] unbinds. Ctrl-C always quits. A key bound to two
actions of one view: the first in this table keeps it (the app warns).
Typing a number or a search in the settings panel uses fixed keys.";

/// One step of a path: a table key or an index into an array of tables.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Seg {
    Key(String),
    Index(usize),
}

fn path_text(path: &[Seg]) -> String {
    let mut out = String::new();
    for s in path {
        match s {
            Seg::Key(k) => {
                if !out.is_empty() {
                    out.push('.');
                }
                out.push_str(k);
            }
            Seg::Index(i) => out.push_str(&format!("[{i}]")),
        }
    }
    out
}

/// The path without indexes, as in [`COMMENTS`].
fn path_key(path: &[Seg]) -> String {
    path.iter()
        .filter_map(|s| match s {
            Seg::Key(k) => Some(k.as_str()),
            Seg::Index(_) => None,
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn comment_for(key: &str) -> Option<&'static str> {
    COMMENTS.iter().find(|(k, _)| *k == key).map(|(_, c)| *c)
}

/// True for an array whose items are all tables (like `channels`).
fn table_array(v: &toml::Value) -> bool {
    matches!(v, toml::Value::Array(a) if !a.is_empty() && a.iter().all(|x| x.is_table()))
}

/// Every leaf of a table (values that are not tables or arrays of
/// tables), with its path.
fn leaves(t: &toml::Table) -> Vec<(Vec<Seg>, toml::Value)> {
    fn walk(v: &toml::Value, path: &mut Vec<Seg>, out: &mut Vec<(Vec<Seg>, toml::Value)>) {
        match v {
            toml::Value::Table(t) => {
                for (k, v) in t {
                    path.push(Seg::Key(k.clone()));
                    walk(v, path, out);
                    path.pop();
                }
            }
            toml::Value::Array(a) if table_array(v) => {
                for (i, v) in a.iter().enumerate() {
                    path.push(Seg::Index(i));
                    walk(v, path, out);
                    path.pop();
                }
            }
            _ => out.push((path.clone(), v.clone())),
        }
    }
    let mut out = Vec::new();
    walk(&toml::Value::Table(t.clone()), &mut Vec::new(), &mut out);
    out
}

fn get<'a>(t: &'a toml::Table, path: &[Seg]) -> Option<&'a toml::Value> {
    let (first, rest) = path.split_first()?;
    let Seg::Key(k) = first else { return None };
    let mut v = t.get(k)?;
    for s in rest {
        v = match (s, v) {
            (Seg::Key(k), toml::Value::Table(t)) => t.get(k)?,
            (Seg::Index(i), toml::Value::Array(a)) => a.get(*i)?,
            _ => return None,
        };
    }
    Some(v)
}

/// Set `path` in `t` to `value`. The parent must exist (only the last key
/// may be new).
fn set(t: &mut toml::Table, path: &[Seg], value: toml::Value) -> bool {
    let Some((Seg::Key(k), rest)) = path.split_first() else {
        return false;
    };
    if rest.is_empty() {
        t.insert(k.clone(), value);
        return true;
    }
    match t.get_mut(k) {
        Some(v) => set_in(v, rest, value),
        None => false,
    }
}

fn set_in(v: &mut toml::Value, path: &[Seg], value: toml::Value) -> bool {
    match (&path[0], v) {
        (Seg::Key(_), toml::Value::Table(t)) => set(t, path, value),
        (Seg::Index(i), toml::Value::Array(a)) => match a.get_mut(*i) {
            Some(x) if path.len() == 1 => {
                *x = value;
                true
            }
            Some(x) => set_in(x, &path[1..], value),
            None => false,
        },
        _ => false,
    }
}

fn to_table(s: &Settings) -> toml::Table {
    toml::Table::try_from(s).expect("settings serialize to a table")
}

/// The settings of a TOML table, entry by entry: an entry that does not
/// fit (wrong type, unknown name) is left at its default with a warning,
/// and so is an unknown key. `[keys]` is not a setting and is skipped.
pub fn settings_from_table(user: &toml::Table) -> (Settings, Vec<String>) {
    let mut user = user.clone();
    user.remove("keys");
    axis_from_old_grid(&mut user);
    let mut warnings = Vec::new();
    let defaults = to_table(&Settings::default());
    let known: Vec<String> = leaves(&defaults)
        .iter()
        .map(|(p, _)| path_key(p))
        .chain(OPTIONAL.iter().map(|s| s.to_string()))
        .collect();
    for (p, _) in leaves(&user) {
        if !known.contains(&path_key(&p)) {
            warnings.push(format!("unknown setting {}", path_text(&p)));
        }
    }
    if let Ok(s) = Settings::deserialize(user.clone()) {
        return (s, warnings);
    }
    let mut good = defaults;
    for (p, v) in leaves(&user) {
        if !known.contains(&path_key(&p)) {
            continue;
        }
        let mut trial = good.clone();
        if !set(&mut trial, &p, v) {
            warnings.push(format!(
                "setting {} does not fit; using the default",
                path_text(&p)
            ));
            continue;
        }
        match Settings::deserialize(trial.clone()) {
            Ok(_) => good = trial,
            Err(e) => {
                let e = e.to_string();
                let e = e.lines().last().unwrap_or("").trim();
                warnings.push(format!(
                    "bad value for {}: {e}; using the default",
                    path_text(&p)
                ));
            }
        }
    }
    let s = Settings::deserialize(good).unwrap_or_default();
    (s, warnings)
}

/// Files from before the axis settings drew the center cross with the
/// grid: solid with a solid grid, none with no grid, else dashed. A file
/// with a grid style but no axis style keeps that look.
fn axis_from_old_grid(user: &mut toml::Table) {
    let Some(toml::Value::Table(d)) = user.get_mut("display") else {
        return;
    };
    if d.contains_key("axis_style") {
        return;
    }
    let axis = match d.get("grid_style").and_then(|v| v.as_str()) {
        Some("Solid") => "Solid",
        Some("None") => "None",
        _ => return,
    };
    d.insert("axis_style".into(), toml::Value::String(axis.into()));
}

/// The `[keys]` table as (action, keys) in file order, with warnings for
/// entries that are not a string or a list of strings.
fn keys_table(doc: &DocumentMut) -> (Vec<(String, Vec<String>)>, Vec<String>) {
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    let Some(t) = doc.get("keys").and_then(Item::as_table_like) else {
        if doc.contains_key("keys") {
            warnings.push("keys must be a table".into());
        }
        return (out, warnings);
    };
    for (name, item) in t.iter() {
        let v = item.as_value();
        let keys: Option<Vec<String>> = match v {
            Some(toml_edit::Value::String(s)) => Some(vec![s.value().clone()]),
            Some(toml_edit::Value::Array(a)) => {
                a.iter().map(|x| x.as_str().map(str::to_string)).collect()
            }
            _ => None,
        };
        match keys {
            Some(k) => out.push((name.to_string(), k)),
            None => warnings.push(format!(
                "keys.{name} must be a key or a list of keys; using the default"
            )),
        }
    }
    (out, warnings)
}

/// A TOML value in `toml_edit` form.
fn edit_value(v: &toml::Value) -> toml_edit::Value {
    let mut t = toml::Table::new();
    t.insert("x".into(), v.clone());
    let text = toml::to_string(&t).expect("a value serializes");
    let doc: DocumentMut = text.parse().expect("toml writes valid toml");
    match doc.get("x") {
        Some(Item::Value(v)) => {
            let mut v = v.clone();
            v.decor_mut().clear();
            v
        }
        // An inline table may come out as a table: make it inline.
        Some(Item::Table(t)) => toml_edit::Value::InlineTable(t.clone().into_inline_table()),
        _ => toml_edit::Value::from(""),
    }
}

/// The whole config file for `s`, with a comment for each line and the
/// `[keys]` table with the default keys.
pub fn generate(s: &Settings) -> String {
    let text = toml::to_string(s).expect("settings serialize");
    let mut doc: DocumentMut = text.parse().expect("toml writes valid toml");
    doc.decor_mut().set_prefix(HEADER);
    fn decorate(t: &mut Table, path: &mut Vec<String>) {
        let keys: Vec<String> = t.iter().map(|(k, _)| k.to_string()).collect();
        for k in keys {
            path.push(k.clone());
            let full = path.join(".");
            if let Some(mut key) = t.key_mut(&k) {
                if let Some(c) = comment_for(&full) {
                    key.leaf_decor_mut().set_prefix(format!("# {c}\n"));
                }
            }
            match t.get_mut(&k) {
                Some(Item::Table(sub)) => {
                    if let Some((_, c)) = TABLE_COMMENTS.iter().find(|(n, _)| *n == full) {
                        sub.decor_mut().set_prefix(format!("\n# {c}\n"));
                    } else {
                        sub.decor_mut().set_prefix("\n");
                    }
                    decorate(sub, path);
                }
                Some(Item::ArrayOfTables(a)) => {
                    let comment = TABLE_COMMENTS.iter().find(|(n, _)| *n == full);
                    for (i, sub) in a.iter_mut().enumerate() {
                        let mut pre = String::from("\n");
                        if let (0, Some((_, c))) = (i, comment) {
                            pre.push_str(&format!("# {c}\n"));
                        }
                        pre.push_str(&format!("# CH{}\n", i + 1));
                        sub.decor_mut().set_prefix(pre);
                        decorate(sub, path);
                    }
                }
                _ => {}
            }
            path.pop();
        }
    }
    decorate(doc.as_table_mut(), &mut Vec::new());
    // The optional log folder: shown as a commented-out line.
    if s.logger.log_dir.is_none() {
        if let Some(Item::Table(lg)) = doc.get_mut("logger") {
            if let Some(mut key) = lg.key_mut("time_axis") {
                let old = key
                    .leaf_decor()
                    .prefix()
                    .and_then(|p| p.as_str())
                    .unwrap_or("")
                    .to_string();
                key.leaf_decor_mut().set_prefix(format!(
                    "# {}\n# log_dir = \"/path/to/logs\"\n{old}",
                    comment_for("logger.log_dir").unwrap_or("")
                ));
            }
        }
    }
    let mut keys = Table::new();
    let mut pre = String::from("\n");
    for line in KEYS_HEADER.lines() {
        pre.push_str(&format!("# {line}\n"));
    }
    keys.decor_mut().set_prefix(pre);
    for d in ACTIONS {
        let v: toml_edit::Value = if d.keys.len() == 1 {
            d.keys[0].into()
        } else {
            let mut a = toml_edit::Array::new();
            for k in d.keys {
                a.push(*k);
            }
            a.into()
        };
        keys.insert(d.name, toml_edit::value(v));
        if let Some(mut key) = keys.key_mut(d.name) {
            key.leaf_decor_mut().set_prefix(format!("# {}\n", d.help));
        }
    }
    doc.insert("keys", Item::Table(keys));
    doc.to_string()
}

/// Write `text` to `path` through a temp file, so a crash cannot leave
/// half a file. Makes the folder if needed.
fn write_atomic(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// What loading the config gave.
#[derive(Debug)]
pub struct Loaded {
    pub settings: Settings,
    pub keys: KeyMap,
    /// Problems to show in the app (bad entries, conflicts, a file that
    /// cannot be read).
    pub warnings: Vec<String>,
    /// Notes to show, like "made the config file".
    pub notes: Vec<String>,
    /// The file to save to, and the settings as loaded (the base the
    /// changes are worked out from). `None`: never save.
    pub save: Option<SaveTarget>,
}

#[derive(Clone, Debug)]
pub struct SaveTarget {
    pub path: PathBuf,
    /// The settings as loaded (sanitized).
    pub base: Settings,
}

fn sanitized(s: &Settings) -> Settings {
    let mut s = s.clone();
    s.sanitize();
    s
}

/// Load the config file at `path`. When it does not exist: take the
/// values of `old` (the settings.toml of older versions) if that exists,
/// else the defaults, and write a new config file with every setting.
pub fn load(path: &Path, old: Option<&Path>) -> Loaded {
    let mut warnings = Vec::new();
    let mut notes = Vec::new();
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let doc: DocumentMut = match text.parse() {
                Ok(d) => d,
                Err(e) => {
                    let first = e.to_string();
                    let first = first.lines().next().unwrap_or("").to_string();
                    warnings.push(format!(
                        "Config file {} is not valid TOML ({first}). Using defaults; the file will not be changed",
                        path.display()
                    ));
                    return Loaded {
                        settings: Settings::default(),
                        keys: KeyMap::default(),
                        warnings,
                        notes,
                        save: None,
                    };
                }
            };
            let table: toml::Table = toml::from_str(&text).unwrap_or_default();
            let (settings, w) = settings_from_table(&table);
            warnings.extend(w);
            let (keys, w) = keys_table(&doc);
            warnings.extend(w);
            let (keys, w) = KeyMap::from_table(&keys);
            warnings.extend(w);
            let base = sanitized(&settings);
            Loaded {
                settings,
                keys,
                warnings,
                notes,
                save: Some(SaveTarget {
                    path: path.to_path_buf(),
                    base,
                }),
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut settings = Settings::default();
            if let Some(old) = old.filter(|p| p.exists()) {
                match std::fs::read_to_string(old)
                    .map_err(anyhow::Error::from)
                    .and_then(|t| toml::from_str::<toml::Table>(&t).map_err(Into::into))
                    .and_then(|mut t| {
                        axis_from_old_grid(&mut t);
                        Settings::deserialize(t).map_err(Into::into)
                    }) {
                    Ok(s) => {
                        settings = s;
                        notes.push(format!(
                            "Took the settings from {} (left as it is)",
                            old.display()
                        ));
                    }
                    Err(e) => {
                        warnings.push(format!(
                            "Cannot read the old settings file {}: {e}. Using defaults; no config file written",
                            old.display()
                        ));
                        return Loaded {
                            settings,
                            keys: KeyMap::default(),
                            warnings,
                            notes,
                            save: None,
                        };
                    }
                }
            }
            let base = sanitized(&settings);
            match write_atomic(path, &generate(&base)) {
                Ok(()) => notes.push(format!("Wrote the config file {}", path.display())),
                Err(e) => warnings.push(format!("Cannot write {}: {e:#}", path.display())),
            }
            Loaded {
                settings,
                keys: KeyMap::default(),
                warnings,
                notes,
                save: Some(SaveTarget {
                    path: path.to_path_buf(),
                    base,
                }),
            }
        }
        Err(e) => {
            warnings.push(format!(
                "Cannot read {}: {e}. Using defaults; the file will not be changed",
                path.display()
            ));
            Loaded {
                settings: Settings::default(),
                keys: KeyMap::default(),
                warnings,
                notes,
                save: None,
            }
        }
    }
}

/// Find or make the item at `path` in a `toml_edit` table, for setting a
/// value. Missing tables are made; a missing array-of-tables entry is
/// made when it is the next one. A key or table that is new to the file
/// (like a setting added after the file was made) gets its comment.
fn item_mut<'a>(t: &'a mut dyn toml_edit::TableLike, path: &[Seg]) -> Option<&'a mut Item> {
    item_mut_at(t, path, &mut Vec::new(), true)
}

/// [`item_mut`] below `done` (the keys walked so far). `comments`: the
/// table may hold comments (not an inline table).
fn item_mut_at<'a>(
    t: &'a mut dyn toml_edit::TableLike,
    path: &[Seg],
    done: &mut Vec<String>,
    comments: bool,
) -> Option<&'a mut Item> {
    let (first, rest) = path.split_first()?;
    let Seg::Key(k) = first else { return None };
    done.push(k.clone());
    let full = done.join(".");
    if rest.is_empty() {
        if !t.contains_key(k) {
            t.insert(k, Item::None);
            if let (true, Some(c), Some(mut key)) = (comments, comment_for(&full), t.key_mut(k)) {
                key.leaf_decor_mut().set_prefix(format!("# {c}\n"));
            }
        }
        // Not `get_mut`: it hides the `Item::None` of a new key.
        return Some(t.entry(k).or_insert(Item::None));
    }
    if !t.contains_key(k) {
        let new = match rest[0] {
            Seg::Index(_) => Item::ArrayOfTables(toml_edit::ArrayOfTables::new()),
            Seg::Key(_) => {
                let mut sub = Table::new();
                match TABLE_COMMENTS.iter().find(|(n, _)| *n == full) {
                    Some((_, c)) => sub.decor_mut().set_prefix(format!("\n# {c}\n")),
                    None => sub.decor_mut().set_prefix("\n"),
                }
                Item::Table(sub)
            }
        };
        t.insert(k, new);
    }
    let item = t.get_mut(k)?;
    match &rest[0] {
        Seg::Key(_) => {
            let comments = comments && !item.is_inline_table();
            item_mut_at(item.as_table_like_mut()?, rest, done, comments)
        }
        Seg::Index(i) => {
            let tail = &rest[1..];
            let (sub, comments): (&mut dyn toml_edit::TableLike, bool) = match item {
                Item::ArrayOfTables(a) => {
                    while a.len() <= *i {
                        let mut sub = Table::new();
                        sub.decor_mut().set_prefix("\n");
                        a.push(sub);
                    }
                    (a.get_mut(*i)?, comments)
                }
                Item::Value(toml_edit::Value::Array(a)) => {
                    (a.get_mut(*i)?.as_inline_table_mut()?, false)
                }
                _ => return None,
            };
            item_mut_at(sub, tail, done, comments)
        }
    }
}

/// Set the value at `path` in the file, made with its comment when it is
/// new. False: the file has no place for it.
fn write_value(doc: &mut DocumentMut, path: &[Seg], v: &toml::Value) -> bool {
    let Some(item) = item_mut(doc.as_table_mut(), path) else {
        return false;
    };
    let mut value = edit_value(v);
    if let Item::Value(old) = item {
        // Keep the spaces and a comment after the value.
        *value.decor_mut() = old.decor().clone();
    }
    *item = Item::Value(value);
    true
}

/// What [`save`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Saved {
    /// Nothing changed since loading: the file was not touched.
    Unchanged,
    /// The changed values were written.
    Written,
}

/// Save the settings that changed since loading into the config file, in
/// place: the rest of the file (comments, order, other values, also bad
/// ones) stays. A file that no longer parses is not written.
pub fn save(target: &SaveTarget, now: &Settings) -> Result<Saved> {
    let now = sanitized(now);
    if now == target.base {
        return Ok(Saved::Unchanged);
    }
    let path = &target.path;
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            write_atomic(path, &generate(&now))?;
            return Ok(Saved::Written);
        }
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    let mut doc: DocumentMut = match text.parse() {
        Ok(d) => d,
        Err(e) => bail!(
            "{} is not valid TOML any more ({}); settings not saved",
            path.display(),
            e.to_string().lines().next().unwrap_or("")
        ),
    };
    let base = to_table(&target.base);
    let new = to_table(&now);
    let mut paths: Vec<Vec<Seg>> = leaves(&new).into_iter().map(|(p, _)| p).collect();
    for (p, _) in leaves(&base) {
        if !paths.contains(&p) {
            paths.push(p);
        }
    }
    let mut failed = Vec::new();
    for p in paths {
        let (a, b) = (get(&base, &p), get(&new, &p));
        if a == b {
            continue;
        }
        match b {
            Some(v) => {
                if !write_value(&mut doc, &p, v) {
                    // Like a table in the file that is a plain value.
                    failed.push(path_text(&p));
                }
            }
            None => {
                // The value is gone (like log_dir set back to none).
                if let Some((Seg::Key(k), parent)) = p.split_last() {
                    if parent.is_empty() {
                        doc.remove(k);
                    } else if let Some(t) =
                        item_mut(doc.as_table_mut(), parent).and_then(Item::as_table_like_mut)
                    {
                        t.remove(k);
                    }
                }
            }
        }
    }
    write_atomic(path, &doc.to_string())?;
    if !failed.is_empty() {
        bail!(
            "{} has no place for {} (the other settings are saved)",
            path.display(),
            failed.join(", ")
        );
    }
    Ok(Saved::Written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oht-cfg-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn generated_file_has_every_setting_and_action_with_comments() {
        let text = generate(&Settings::default());
        let doc: DocumentMut = text.parse().unwrap();
        // Every setting of the defaults, by serializing them.
        let t: toml::Table = toml::from_str(&text).unwrap();
        for (p, v) in leaves(&to_table(&Settings::default())) {
            assert_eq!(get(&t, &p), Some(&v), "{}", path_text(&p));
            let key = path_key(&p);
            let c = comment_for(&key).unwrap_or_else(|| panic!("no comment for {key}"));
            assert!(text.contains(&format!("# {c}\n")), "{key}");
        }
        // No comment for a setting that does not exist.
        let known: Vec<String> = leaves(&to_table(&Settings::default()))
            .iter()
            .map(|(p, _)| path_key(p))
            .chain(OPTIONAL.iter().map(|s| s.to_string()))
            .collect();
        for (k, _) in COMMENTS {
            assert!(known.contains(&k.to_string()), "{k}");
        }
        assert!(text.contains("# log_dir = "), "{text}");
        // Every action with its default keys.
        let keys = doc["keys"].as_table().unwrap();
        for d in ACTIONS {
            let v = keys.get(d.name).unwrap_or_else(|| panic!("{}", d.name));
            let got: Vec<String> = match v.as_value().unwrap() {
                toml_edit::Value::String(s) => vec![s.value().clone()],
                toml_edit::Value::Array(a) => {
                    a.iter().map(|x| x.as_str().unwrap().to_string()).collect()
                }
                _ => panic!(),
            };
            assert_eq!(
                got,
                d.keys.iter().map(|s| s.to_string()).collect::<Vec<_>>()
            );
            assert!(
                text.contains(&format!("# {}\n{} = ", d.help, d.name)),
                "{}",
                d.name
            );
        }
        assert!(
            text.contains("settings = [\"s\", \"Enter\", \"F2\"]"),
            "{text}"
        );
        assert!(text.contains("run_stop = \"Space\""), "{text}");
        // It loads back to the defaults, with no warning.
        let (s, w) = settings_from_table(&t);
        assert_eq!(s, Settings::default());
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn round_trip_and_comments_stay_after_a_change() {
        let dir = temp_dir("rt");
        let path = dir.join("sub/config.toml");
        let l = load(&path, None);
        assert!(l.warnings.is_empty(), "{:?}", l.warnings);
        assert!(path.exists());
        // The user edits the file: a comment of their own and a value.
        let text = std::fs::read_to_string(&path).unwrap();
        let text = text.replace("cal_freq = 1000", "cal_freq = 2000   # my own note");
        let text = format!("# my header\n{text}");
        std::fs::write(&path, &text).unwrap();
        let l = load(&path, None);
        assert_eq!(l.settings.cal_freq, 2000);
        let target = l.save.unwrap();
        // No change in the app: the file is not touched.
        assert_eq!(save(&target, &l.settings).unwrap(), Saved::Unchanged);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        // A change in the app: only that value changes.
        let mut s = l.settings.clone();
        s.channels[1].volt_idx = 2;
        s.cursors.volt_a.style = CursorStyle::Dotted;
        s.display.trace_style[2] = TraceStyle::Box;
        s.cal_freq = 5000;
        assert_eq!(save(&target, &s).unwrap(), Saved::Written);
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.starts_with("# my header\n"), "{after}");
        assert!(after.contains("cal_freq = 5000   # my own note"), "{after}");
        let diff: Vec<(&str, &str)> = text
            .lines()
            .zip(after.lines())
            .filter(|(a, b)| a != b)
            .collect();
        assert_eq!(diff.len(), 4, "{diff:?}");
        assert_eq!(text.lines().count(), after.lines().count());
        let back = load(&path, None);
        assert_eq!(back.settings, s);
        assert!(back.warnings.is_empty(), "{:?}", back.warnings);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn calibration_is_saved_into_a_file_made_before_it() {
        let dir = temp_dir("cal");
        let path = dir.join("config.toml");
        // A file of an older version: no [calibration] table.
        let text = generate(&Settings::default());
        let cut = text.find("\n# The app's own calibration").unwrap();
        let keys = text.find("\n# Key bindings").unwrap();
        assert!(cut < keys);
        let old = format!("{}{}", &text[..cut], &text[keys..]);
        assert!(!old.contains("[calibration]"));
        std::fs::write(&path, &old).unwrap();
        let l = load(&path, None);
        assert!(l.warnings.is_empty(), "{:?}", l.warnings);
        let target = l.save.unwrap();
        let mut s = l.settings.clone();
        s.calibration.zero_ch1 = [128.5; 8];
        s.calibration.zero_ch1_30m[3] = 131.25;
        assert_eq!(save(&target, &s).unwrap(), Saved::Written);
        let after = std::fs::read_to_string(&path).unwrap();
        let c = comment_for("calibration.zero_ch1").unwrap();
        assert!(after.contains(&format!("# {c}\nzero_ch1 = [")), "{after}");
        let back = load(&path, None);
        assert!(back.warnings.is_empty(), "{:?}", back.warnings);
        assert_eq!(back.settings.calibration, s.calibration);
        assert_eq!(back.settings, s);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn every_setting_goes_into_an_old_minimal_file() {
        let old = "# my old file\ncal_freq = 1000\n\n[keys]\nquit = \"q\"\n";
        let mut doc: DocumentMut = old.parse().unwrap();
        let defaults = to_table(&Settings::default());
        for (p, v) in leaves(&defaults) {
            assert!(write_value(&mut doc, &p, &v), "{}", path_text(&p));
        }
        let text = doc.to_string();
        assert!(
            text.starts_with("# my old file\ncal_freq = 1000\n"),
            "{text}"
        );
        let t: toml::Table = toml::from_str(&text).unwrap();
        for (p, v) in leaves(&defaults) {
            assert_eq!(get(&t, &p), Some(&v), "{}", path_text(&p));
            let key = path_key(&p);
            if key != "cal_freq" {
                if let Some(c) = comment_for(&key) {
                    assert!(text.contains(&format!("# {c}\n")), "{key}: {text}");
                }
            }
        }
        let (s, w) = settings_from_table(&t);
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(s, Settings::default());
        assert!(keys_table(&doc).1.is_empty());
    }

    #[test]
    fn a_value_with_no_place_in_the_file_is_an_error() {
        let dir = temp_dir("noplace");
        let path = dir.join("config.toml");
        std::fs::write(&path, "calibration = 5\n").unwrap();
        let target = SaveTarget {
            path: path.clone(),
            base: Settings::default(),
        };
        let mut s = Settings::default();
        s.calibration.zero_ch2 = [127.0; 8];
        s.cal_freq = 2000;
        let e = save(&target, &s).unwrap_err().to_string();
        assert!(e.contains("calibration.zero_ch2"), "{e}");
        // The other values are saved.
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("cal_freq = 2000"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_config_takes_the_old_settings_once() {
        let dir = temp_dir("mig");
        let old = dir.join("settings.toml");
        let old_text = "selected = 1\n[cursors]\nmode = \"Time\"\n[display]\ncursor_style = \"Fine\"\ndots = true\n";
        std::fs::write(&old, old_text).unwrap();
        let path = dir.join("config.toml");
        let l = load(&path, Some(&old));
        assert!(l.warnings.is_empty(), "{:?}", l.warnings);
        assert!(
            l.notes.iter().any(|n| n.contains("settings.toml")),
            "{:?}",
            l.notes
        );
        assert_eq!(l.settings.selected, 1);
        assert!(l.settings.display.dots);
        // The old file is left as it was.
        assert_eq!(std::fs::read_to_string(&old).unwrap(), old_text);
        // The new file has the values, the new cursor format and the keys.
        let l2 = load(&path, Some(&old));
        assert!(l2.notes.iter().all(|n| !n.contains("settings.toml")));
        let s = &l2.settings;
        assert_eq!(s.selected, 1);
        assert!(s.display.dots && s.cursors.time_a.enabled && s.cursors.time_b.enabled);
        assert_eq!(s.cursors.time_a.style, CursorStyle::Fine);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[keys]") && text.contains("[cursors.time_a]"));
        // A broken old file: defaults, and no config file.
        let dir2 = temp_dir("mig2");
        std::fs::write(dir2.join("settings.toml"), "selected = [").unwrap();
        let l = load(&dir2.join("config.toml"), Some(&dir2.join("settings.toml")));
        assert!(!l.warnings.is_empty() && l.save.is_none());
        assert!(!dir2.join("config.toml").exists());
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&dir2).unwrap();
    }

    #[test]
    fn broken_file_is_never_overwritten() {
        let dir = temp_dir("bad");
        let path = dir.join("config.toml");
        let text = "selected = [\n";
        std::fs::write(&path, text).unwrap();
        let l = load(&path, None);
        assert!(l.warnings[0].contains("not valid TOML"), "{:?}", l.warnings);
        assert!(l.save.is_none());
        assert_eq!(l.settings, Settings::default());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn old_files_keep_their_center_cross() {
        use crate::settings::{GridExtent, GridStyle};
        let load = |text: &str| settings_from_table(&toml::from_str(text).unwrap()).0;
        // No axis settings: the center cross of the first versions.
        let s = load("[display]\ndots = true\n");
        assert_eq!(
            (
                s.display.axis_style,
                s.display.axis_extent,
                s.display.axis_brightness
            ),
            (GridStyle::Dashed, GridExtent::Full, 30)
        );
        // It went with the grid style: solid with solid, none with none.
        let s = load("[display]\ngrid_style = \"Solid\"\n");
        assert_eq!(s.display.axis_style, GridStyle::Solid);
        let s = load("[display]\ngrid_style = \"None\"\n");
        assert_eq!(s.display.axis_style, GridStyle::None);
        let s = load("[display]\ngrid_style = \"Crosses\"\n");
        assert_eq!(s.display.axis_style, GridStyle::Dashed);
        // A file with axis settings keeps them, apart from the grid.
        let s = load(
            "[display]\ngrid_style = \"None\"\naxis_style = \"Solid\"\naxis_extent = \"Points\"\naxis_brightness = 72\n",
        );
        assert_eq!(s.display.grid_style, GridStyle::None);
        assert_eq!(s.display.axis_style, GridStyle::Solid);
        assert_eq!(s.display.axis_extent, GridExtent::Points);
        let mut s = s;
        s.sanitize();
        assert_eq!(s.display.axis_brightness, 70);
    }

    #[test]
    fn bad_entries_fall_back_one_by_one() {
        let dir = temp_dir("entries");
        let path = dir.join("config.toml");
        let text = "\
# mine
selected = 2
cal_freq = \"fast\"
wobble = 3

[trigger]
mode = \"Sometimes\"
level = 0.5

[[channels]]
volt_idx = 3

[[channels]]
probe = 10

[keys]
settings = \"o\"
autoset = \"Hyper-a\"
fly = \"y\"
xy = 5
";
        std::fs::write(&path, text).unwrap();
        let l = load(&path, None);
        let s = &l.settings;
        assert_eq!(s.selected, 2);
        assert_eq!(s.cal_freq, 1000);
        assert_eq!(s.trigger.mode, TriggerMode::Auto);
        assert_eq!(s.trigger.level, 0.5);
        assert_eq!(s.channels[0].volt_idx, 3);
        assert_eq!(s.channels[1].probe, 10);
        let w = l.warnings.join("\n");
        for part in [
            "cal_freq",
            "unknown setting wobble",
            "trigger.mode",
            "Hyper-a",
            "keys.fly",
            "keys.xy",
        ] {
            assert!(w.contains(part), "{part}: {w}");
        }
        assert!(w.contains("key o is bound to settings and single"), "{w}");
        // Nothing changed: the file stays as it is.
        let target = l.save.unwrap();
        assert_eq!(save(&target, &l.settings).unwrap(), Saved::Unchanged);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        // A change: only that value is written; the bad ones stay.
        let mut s2 = l.settings.clone();
        s2.trigger.level = 0.7;
        save(&target, &s2).unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after, text.replace("level = 0.5", "level = 0.7"));
        // A file that stops parsing is not written.
        std::fs::write(&path, "selected = [").unwrap();
        s2.trigger.level = 0.9;
        assert!(save(&target, &s2).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "selected = [");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
