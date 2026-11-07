//! App state: the scope, the link to the acquisition thread, and the keys.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::acquire::{Acquisition, Command, Event, LogRequest};
use crate::export;
use crate::keys::{self, LogKey};
use crate::logger::view::LogView;
use crate::logger::writer::{self as log_writer, LogState, Shared, StopReason};
use crate::logger::{self, csv as log_csv};
use crate::panel::{Action, DeviceInfo, Outcome, Panel};
use crate::scope::{Scope, MATH};
use crate::settings::*;

/// Text on the message line.
#[derive(Clone, Debug)]
pub struct Message {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

/// Recent logs in the panel (few, so the panel keeps its height).
pub const RECENT_LOGS: usize = 10;

/// How long a message stays on screen.
pub const MESSAGE_TIME: Duration = Duration::from_secs(6);

pub struct App {
    pub scope: Scope,
    acq: Option<Acquisition>,
    pub device_name: String,
    pub connected: bool,
    /// Progress text while connecting (firmware upload and such).
    pub status: Option<String>,
    pub message: Option<Message>,
    pub help: bool,
    pub quit: bool,
    pub export_dir: PathBuf,
    /// Settings file, when settings are saved on quit.
    pub settings_path: Option<PathBuf>,
    /// Notes from connecting, like where the calibration came from.
    pub device_notes: Vec<String>,
    pub panel: Panel,
    pub panel_open: bool,
    sent_running: Option<bool>,
    /// The log being recorded, if any.
    pub rec: Option<Shared>,
    /// The log open in the log view.
    pub log_view: Option<LogView>,
    /// Show the log view (not the scope).
    pub show_log: bool,
    /// Log folder from the command line (it wins over the setting).
    pub log_dir_override: Option<PathBuf>,
    /// Free bytes in the log folder, checked about once a second.
    pub log_free: Option<u64>,
    free_at: Option<Instant>,
}

fn next_in<T: PartialEq + Copy>(list: &[T], v: T) -> T {
    let i = list.iter().position(|x| *x == v).map_or(0, |i| i + 1);
    list[i % list.len()]
}

impl App {
    pub fn new(settings: Settings, acq: Option<Acquisition>) -> Self {
        let mut app = Self {
            scope: Scope::new(settings),
            acq,
            device_name: "connecting...".into(),
            connected: false,
            status: None,
            message: None,
            help: false,
            quit: false,
            export_dir: PathBuf::from("."),
            settings_path: None,
            device_notes: Vec::new(),
            panel: Panel::new(),
            panel_open: false,
            sent_running: None,
            rec: None,
            log_view: None,
            show_log: false,
            log_dir_override: None,
            log_free: None,
            free_at: None,
        };
        app.sync();
        app
    }

    pub fn notify(&mut self, text: impl Into<String>, error: bool) {
        // A fresh error stays a little while: a notice does not hide it.
        if !error {
            if let Some(m) = &self.message {
                if m.error && m.at.elapsed() < Duration::from_secs(4) {
                    return;
                }
            }
        }
        self.message = Some(Message {
            text: text.into(),
            error,
            at: Instant::now(),
        });
    }

    /// The message to show now, if it is not too old.
    pub fn current_message(&self) -> Option<&Message> {
        self.message
            .as_ref()
            .filter(|m| m.error || m.at.elapsed() < MESSAGE_TIME)
    }

    /// Take events from the acquisition thread. Returns true when
    /// something changed on screen.
    pub fn tick(&mut self) -> bool {
        let mut changed = false;
        while let Some(ev) = self.acq.as_ref().and_then(|a| a.try_recv()) {
            changed |= self.handle_event(ev);
        }
        changed |= self.log_tick();
        changed | self.sync()
    }

    /// Folder for new logs.
    pub fn log_dir(&self) -> PathBuf {
        self.log_dir_override
            .clone()
            .unwrap_or_else(|| self.scope.settings.logger.dir())
    }

    /// True while a log is being recorded (or waits for the scope).
    pub fn recording(&self) -> bool {
        self.rec
            .as_ref()
            .is_some_and(|r| !matches!(r.lock().unwrap().state, LogState::Done(_)))
    }

    /// Logger upkeep: the end of a log, the free space, the log view.
    fn log_tick(&mut self) -> bool {
        let mut changed = false;
        if self
            .free_at
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(1))
        {
            self.log_free = logger::free_space(&self.log_dir());
            self.free_at = Some(Instant::now());
            changed |= self.rec.is_some();
        }
        if let Some(r) = &self.rec {
            let st = r.lock().unwrap().clone();
            match st.state {
                LogState::Done(why) => {
                    let p = st.path.display();
                    let size = logger::fmt_bytes(st.bytes);
                    match why {
                        StopReason::User => self.notify(format!("Log saved: {p} ({size})"), false),
                        StopReason::Limit => self.notify(
                            format!("Size limit reached, log stopped: {p} ({size})"),
                            true,
                        ),
                        StopReason::Error(e) => {
                            self.notify(format!("Log stopped, cannot write {p}: {e}"), true)
                        }
                    }
                    self.rec = None;
                    if let Some(v) = &mut self.log_view {
                        v.live = false;
                    }
                    changed = true;
                }
                LogState::Recording => {
                    let open = self
                        .log_view
                        .as_ref()
                        .is_some_and(|v| v.file.path == st.path);
                    if !open {
                        match LogView::open(&st.path, true) {
                            Ok(v) => self.log_view = Some(v),
                            Err(e) => self.notify(format!("{e:#}"), true),
                        }
                    }
                    changed = true;
                }
                LogState::Waiting => changed = true,
            }
        }
        if let Some(v) = &mut self.log_view {
            match v.tick() {
                Ok(c) => changed |= c && self.show_log,
                Err(e) => {
                    self.notify(format!("Reading the log failed: {e:#}"), true);
                    self.log_view = None;
                }
            }
        }
        changed
    }

    /// Start recording, or stop the running log.
    pub fn toggle_record(&mut self) {
        if self.recording() {
            if let Some(a) = &self.acq {
                a.send(Command::StopLog);
            }
            return;
        }
        let s = &self.scope.settings;
        let channels = [s.channels[0].enabled, s.channels[1].enabled];
        if !channels.iter().any(|c| *c) {
            self.notify("Turn on CH1 or CH2 to record a log", true);
            return;
        }
        let dir = self.log_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.notify(
                format!("Cannot make the log folder {}: {e}", dir.display()),
                true,
            );
            return;
        }
        let path = logger::new_log_path(&dir, std::time::SystemTime::now());
        let limit = s.logger.log_limit;
        let hw = self.scope.hw_config();
        let shared = log_writer::shared(path.clone(), limit);
        let req = LogRequest {
            shared: shared.clone(),
            step_ns: logger::step_ns(s.logger.step),
            channels,
            probe: [0, 1].map(|c| s.channels[c].probe),
            invert: [0, 1].map(|c| s.channels[c].invert),
            volts_per_div: [0, 1].map(|c| s.channels[c].volts_per_div()),
            gains: hw.gains,
            dc: hw.dc,
            cal_freq: hw.cal_freq,
        };
        let free = logger::free_space(&dir);
        let warn = free.is_some_and(|f| limit == 0 || f < limit);
        let text = match free {
            Some(f) if warn => format!(
                "Recording to {} · only {} free on the disk",
                path.display(),
                logger::fmt_bytes(f)
            ),
            _ => format!("Recording to {}", path.display()),
        };
        match &self.acq {
            Some(a) => a.send(Command::StartLog(req)),
            None => {
                self.notify("No scope: cannot record", true);
                return;
            }
        }
        self.notify(text, warn && limit > 0);
        self.rec = Some(shared);
        self.show_log = true;
    }

    /// Open a log file in the log view.
    pub fn open_log(&mut self, path: &std::path::Path) -> bool {
        let live = self
            .rec
            .as_ref()
            .is_some_and(|r| r.lock().unwrap().path == path);
        match LogView::open(path, live) {
            Ok(v) => {
                self.log_view = Some(v);
                self.show_log = true;
                true
            }
            Err(e) => {
                self.notify(format!("{e:#}"), true);
                false
            }
        }
    }

    /// Logs in the log folder, newest first, for the panel.
    pub fn recent_logs(&self) -> Vec<PathBuf> {
        logger::recent_logs(&self.log_dir(), RECENT_LOGS)
    }

    /// Export the open log as CSV: the whole log, or with `range` only
    /// between the cursors.
    pub fn export_log(&mut self, range: bool) {
        let Some(v) = &self.log_view else {
            self.notify(
                "No log open: press V or pick one in Logger › Open log",
                true,
            );
            return;
        };
        let r = if range && v.cursors {
            v.cursor_range()
        } else if range {
            self.notify(
                "Cursors are off: press v in the log view to set A and B",
                true,
            );
            return;
        } else {
            v.file.first()..v.end()
        };
        let time = self.scope.settings.logger.csv_time;
        match log_csv::save_log_csv(&self.export_dir, &v.file, r, time) {
            Ok(p) => self.notify(format!("Saved {}", p.display()), false),
            Err(e) => self.notify(format!("Export failed: {e:#}"), true),
        }
    }

    /// A logger key (see [`crate::keys`]).
    fn log_key(&mut self, k: LogKey) {
        let w = self
            .log_view
            .as_ref()
            .map_or(200, |v| v.width.get())
            .max(10);
        let div = w / 10;
        match k {
            LogKey::Record => self.toggle_record(),
            LogKey::ToggleView => {
                self.show_log = !self.show_log;
                if self.show_log && self.log_view.is_none() {
                    if let Some(p) = self.recent_logs().first() {
                        let p = p.clone();
                        self.open_log(&p);
                    } else {
                        self.notify("No logs yet: press r to record one", false);
                    }
                }
            }
            LogKey::Go(m) => {
                if let Some(v) = &mut self.log_view {
                    v.go(m, w, div);
                }
            }
            LogKey::Cursors => {
                if let Some(v) = &mut self.log_view {
                    v.toggle_cursors(w);
                }
            }
            LogKey::Cursor(i, by) => {
                if let Some(v) = &mut self.log_view {
                    v.move_cursor(i, by, w);
                }
            }
            LogKey::Axis => {
                let a = &mut self.scope.settings.logger.time_axis;
                *a = match a {
                    TimeAxis::Clock => TimeAxis::Seconds,
                    TimeAxis::Seconds => TimeAxis::Clock,
                };
            }
            LogKey::Export => {
                let range = self.log_view.as_ref().is_some_and(|v| v.cursors);
                self.export_log(range);
            }
        }
    }

    /// Handle one event from the acquisition thread.
    pub fn handle_event(&mut self, ev: Event) -> bool {
        match ev {
            Event::Block(b) => self.scope.push(b),
            Event::Status(s) => {
                self.status = Some(s);
                true
            }
            Event::Connected { name, notes } => {
                self.device_name = name;
                self.connected = true;
                self.status = None;
                let text = if notes.is_empty() {
                    "Scope connected".to_string()
                } else {
                    notes.join(" · ")
                };
                let warn = notes.iter().any(|n| n.contains("nominal"));
                self.device_notes = notes;
                self.notify(text, warn);
                true
            }
            Event::Disconnected(e) => {
                self.connected = false;
                self.notify(
                    format!("Scope lost: {e}. Waiting for it to come back"),
                    true,
                );
                true
            }
            Event::Error(e) => {
                self.status = None;
                self.notify(e, true);
                true
            }
        }
    }

    /// Send changed settings to the acquisition thread.
    fn sync(&mut self) -> bool {
        let mut changed = false;
        for n in std::mem::take(&mut self.scope.notices) {
            self.notify(n, false);
            changed = true;
        }
        if let Some((gen, cfg)) = self.scope.sync_hw() {
            if let Some(a) = &self.acq {
                a.send(Command::Configure { gen, cfg });
            }
        }
        if self.sent_running != Some(self.scope.running) {
            self.sent_running = Some(self.scope.running);
            if let Some(a) = &self.acq {
                a.send(Command::Run(self.scope.running));
            }
        }
        changed
    }

    /// Settings changed by a key: update the hardware, and redraw a
    /// stopped trace with the new settings.
    fn changed(&mut self) {
        self.sync();
        if !self.scope.running {
            self.scope.reprocess();
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.help {
            // Any key closes the help; q still quits.
            match key.code {
                KeyCode::Char('q') => self.quit = true,
                KeyCode::Char('c') if ctrl => self.quit = true,
                _ => self.help = false,
            }
            return;
        }
        if self.panel_open {
            if ctrl && key.code == KeyCode::Char('c') {
                self.quit = true;
            } else {
                self.panel_key(key);
            }
            return;
        }
        if let Some(k) = keys::log_key(&key, self.show_log) {
            self.log_key(k);
            return;
        }
        let sel = self.scope.settings.selected;
        let s = &mut self.scope.settings;
        match key.code {
            KeyCode::Char('c') if ctrl => self.quit = true,
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('?') | KeyCode::F(1) => self.help = true,
            KeyCode::Char('S') | KeyCode::F(2) => {
                self.panel_open = true;
                return;
            }

            // Run control.
            KeyCode::Char(' ') => self.action(Action::RunStop),
            KeyCode::Char('o') => self.action(Action::Single),
            KeyCode::Char('a') => self.action(Action::Autoset),

            // Channels.
            KeyCode::Tab => s.selected = (sel + 1) % 3,
            KeyCode::BackTab => s.selected = (sel + 2) % 3,
            KeyCode::Char('1') => s.channels[0].enabled ^= true,
            KeyCode::Char('2') => s.channels[1].enabled ^= true,
            KeyCode::Char('3') => s.math.enabled ^= true,
            KeyCode::Up => {
                let v = volt_idx(s, sel);
                *v = v.saturating_sub(1);
            }
            KeyCode::Down => {
                let v = volt_idx(s, sel);
                *v = (*v + 1).min(VOLT_DIVS.len() - 1);
            }
            KeyCode::Char(']') => move_offset(s, sel, 0.5),
            KeyCode::Char('[') => move_offset(s, sel, -0.5),
            KeyCode::Char('}') => move_offset(s, sel, 0.1),
            KeyCode::Char('{') => move_offset(s, sel, -0.1),
            KeyCode::Char('0') => *offset(s, sel) = 0.0,
            KeyCode::Char('c') if sel < 2 => {
                s.channels[sel].coupling = s.channels[sel].coupling.toggle()
            }
            KeyCode::Char('p') if sel < 2 => {
                let ch = &mut s.channels[sel];
                let old = ch.probe as f64;
                ch.probe = if ch.probe == 1 { 10 } else { 1 };
                // Keep the trigger level where it is on the screen.
                if s.trigger.source == sel {
                    s.trigger.level *= ch.probe as f64 / old;
                }
            }
            KeyCode::Char('i') if sel < 2 => s.channels[sel].invert ^= true,
            KeyCode::Char('m') => {
                s.math.mode = s.math.mode.next();
                s.math.enabled = true;
            }

            // Time base.
            KeyCode::Left => s.time_idx = s.time_idx.saturating_sub(1),
            KeyCode::Right => s.time_idx = (s.time_idx + 1).min(TIME_DIVS.len() - 1),

            // Trigger.
            KeyCode::Char('t') => s.trigger.mode = s.trigger.mode.next(),
            KeyCode::Char('s') => s.trigger.source = 1 - s.trigger.source.min(1),
            KeyCode::Char('e') => s.trigger.slope = s.trigger.slope.next(),
            KeyCode::Char('+') | KeyCode::Char('=') => {
                s.trigger.level += 0.1 * s.channels[s.trigger.source].volts_per_div()
            }
            KeyCode::Char('-') | KeyCode::Char('_') => {
                s.trigger.level -= 0.1 * s.channels[s.trigger.source].volts_per_div()
            }
            KeyCode::Char('T') => self.action(Action::TriggerToMiddle),
            KeyCode::Char(',') | KeyCode::Char('<') => {
                s.trigger.position = (s.trigger.position - 0.05).max(0.0)
            }
            KeyCode::Char('.') | KeyCode::Char('>') => {
                s.trigger.position = (s.trigger.position + 0.05).min(1.0)
            }

            // Display.
            KeyCode::Char('A') => s.display.averaging = next_in(&AVERAGING, s.display.averaging),
            KeyCode::Char('P') => {
                s.display.persistence = next_in(&PERSISTENCE, s.display.persistence);
                self.scope.history.clear();
            }
            KeyCode::Char('d') => s.display.dots ^= true,
            KeyCode::Char('z') => {
                s.display.interpolation = match s.display.interpolation {
                    Interpolation::Sinc => Interpolation::Linear,
                    Interpolation::Linear => Interpolation::Sinc,
                }
            }
            KeyCode::Char('M') => s.display.measurements ^= true,
            KeyCode::Char('f') => s.display.spectrum ^= true,
            KeyCode::Char('F') => {
                s.display.spectrum_zoom = (s.display.spectrum_zoom + 1) % (MAX_SPECTRUM_ZOOM + 1)
            }
            KeyCode::Char('w') => s.display.window = s.display.window.next(),
            KeyCode::Char('x') => s.display.xy ^= true,

            // Cursors.
            KeyCode::Char('v') => {
                s.cursors.hidden ^= true;
                if !s.cursors.hidden && !(0..4).any(|i| s.cursors.get(i).enabled) {
                    self.notify("No cursor is on: turn them on in Settings › Cursors", false);
                }
            }
            KeyCode::Char(c @ ('h' | 'l' | 'H' | 'L' | 'j' | 'k' | 'J' | 'K')) => move_cursor(s, c),

            // Calibration output.
            KeyCode::Char('g') => s.cal_freq = next_in(&CAL_FREQS, s.cal_freq),
            KeyCode::Char('G') => {
                let i = CAL_FREQS.iter().position(|f| *f == s.cal_freq).unwrap_or(0);
                s.cal_freq = CAL_FREQS[(i + CAL_FREQS.len() - 1) % CAL_FREQS.len()];
            }

            // Files.
            KeyCode::Char('E') => self.action(Action::ExportCsv),
            KeyCode::Char('W') => self.action(Action::ExportJson),
            KeyCode::Char('R') => self.action(Action::Reset),
            _ => return,
        }
        self.changed();
    }

    /// A key while the settings panel is open.
    fn panel_key(&mut self, key: KeyEvent) {
        let d = self.device_info();
        let persistence = self.scope.settings.display.persistence;
        match self.panel.handle_key(key, &mut self.scope.settings, &d) {
            Outcome::None => {}
            Outcome::Changed => {
                if self.scope.settings.display.persistence != persistence {
                    self.scope.history.clear();
                }
                self.changed();
            }
            Outcome::Close => {
                // Closing may apply a number that was being typed.
                self.panel_open = false;
                self.changed();
            }
            Outcome::Action(a) => {
                self.action(a);
                self.changed();
            }
            Outcome::Message(text, error) => self.notify(text, error),
        }
    }

    /// Run/stop, autoset, export and the other things that are not settings.
    pub fn action(&mut self, a: Action) {
        match a {
            Action::RunStop => {
                let run = !self.scope.running;
                self.scope.set_running(run);
            }
            Action::Single => {
                self.scope.arm_single();
                self.notify("Single: waiting for a trigger", false);
            }
            Action::Autoset => self.scope.start_autoset(),
            Action::TriggerToMiddle => self.trigger_to_middle(),
            Action::ExportCsv => self.export(false),
            Action::ExportJson => self.export(true),
            Action::Record => self.toggle_record(),
            Action::LogView => self.log_key(LogKey::ToggleView),
            Action::ExportLog => self.export_log(false),
            Action::ExportLogRange => self.export_log(true),
            Action::OpenLog(i) => {
                if let Some(p) = self.recent_logs().get(i).cloned() {
                    if self.open_log(&p) {
                        self.panel_open = false;
                    }
                }
            }
            Action::Reset => {
                let keep = self.scope.settings.selected;
                self.scope.settings = Settings::default();
                self.scope.settings.selected = keep;
                self.scope.history.clear();
                self.notify("Settings reset to defaults", false);
            }
        }
    }

    /// Facts for the Device and Files parts of the settings panel.
    pub fn device_info(&self) -> DeviceInfo {
        DeviceInfo {
            name: self.device_name.clone(),
            connected: self.connected,
            notes: self.device_notes.clone(),
            rate: self.scope.hw.as_ref().map(|h| h.rate.rate),
            running: self.scope.running,
            export_dir: self.export_dir.display().to_string(),
            recording: self.recording(),
            log_dir: self.log_dir().display().to_string(),
            log_free: self.log_free,
            recent_logs: self
                .recent_logs()
                .iter()
                .map(|p| {
                    p.file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
                })
                .collect(),
            settings_file: self
                .settings_path
                .as_ref()
                .map_or_else(|| "not saved".to_string(), |p| p.display().to_string()),
        }
    }

    /// Set the trigger level to the middle of the source signal.
    fn trigger_to_middle(&mut self) {
        let src = self.scope.settings.trigger.source;
        if let Some(m) = self.scope.measure(src) {
            self.scope.settings.trigger.level = (m.max + m.min) / 2.0;
        }
    }

    /// Save the traces on screen as CSV, or with `json` as JSON.
    pub fn export(&mut self, json: bool) {
        match self.export_to(&self.export_dir.clone(), json) {
            Ok(p) => self.notify(format!("Saved {}", p.display()), false),
            Err(e) => self.notify(format!("Export failed: {e:#}"), true),
        }
    }

    pub fn export_to(&self, dir: &std::path::Path, json: bool) -> anyhow::Result<PathBuf> {
        let s = &self.scope.settings;
        let Some(f) = &self.scope.frame else {
            anyhow::bail!("nothing captured yet");
        };
        let names = ["CH1_V", "CH2_V", "MATH"];
        let shown = [s.channels[0].enabled, s.channels[1].enabled, s.math.enabled];
        let mut start = None;
        let mut cols = Vec::new();
        for t in 0..3 {
            if !shown[t] {
                continue;
            }
            if let Some(tr) = &f.traces[t] {
                let r = f.visible(s, tr.len());
                start.get_or_insert(r.start);
                cols.push((names[t], &tr[r]));
            }
        }
        let start = start.unwrap_or(0);
        let t0 = f.t0 + start as f64 * f.dt;
        if json {
            let path = dir.join(format!(
                "openhantek-{}.json",
                export::timestamp(std::time::SystemTime::now())
            ));
            let traces: serde_json::Map<String, serde_json::Value> = cols
                .iter()
                .map(|(n, v)| (n.to_string(), serde_json::json!(v)))
                .collect();
            let doc = serde_json::json!({
                "t0_s": t0,
                "dt_s": f.dt,
                "sample_rate": f.rate,
                "triggered": f.triggered,
                "settings": s,
                "traces": traces,
            });
            std::fs::write(&path, serde_json::to_string_pretty(&doc)?)?;
            return Ok(path);
        }
        let columns: Vec<export::Column> = cols
            .iter()
            .map(|(name, samples)| export::Column { name, samples })
            .collect();
        Ok(export::save_csv(dir, t0, f.dt, &columns)?)
    }
}

fn volt_idx(s: &mut Settings, sel: usize) -> &mut usize {
    if sel == MATH {
        &mut s.math.volt_idx
    } else {
        &mut s.channels[sel].volt_idx
    }
}

fn offset(s: &mut Settings, sel: usize) -> &mut f64 {
    if sel == MATH {
        &mut s.math.offset
    } else {
        &mut s.channels[sel].offset
    }
}

fn move_offset(s: &mut Settings, sel: usize, by: f64) {
    let o = offset(s, sel);
    *o = ((*o + by) * 10.0).round() / 10.0;
    *o = o.clamp(-V_DIVS / 2.0, V_DIVS / 2.0);
}

/// h/l move time cursor A, H/L cursor B; j/k and J/K the voltage cursors.
/// Only cursors that are on screen move.
fn move_cursor(s: &mut Settings, key: char) {
    let (i, by) = match key {
        'h' => (0, -0.1),
        'l' => (0, 0.1),
        'H' => (1, -0.1),
        'L' => (1, 0.1),
        'j' => (2, -0.1),
        'k' => (2, 0.1),
        'J' => (3, -0.1),
        _ => (3, 0.1),
    };
    if !s.cursors.shown(i) {
        return;
    }
    let limit = if CursorSettings::is_time(i) {
        H_DIVS / 2.0
    } else {
        V_DIVS / 2.0
    };
    let c = s.cursors.get_mut(i);
    c.position = (((c.position + by) * 10.0).round() / 10.0).clamp(-limit, limit);
}
