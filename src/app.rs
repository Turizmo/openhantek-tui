//! App state: the scope, the link to the acquisition thread, and the keys.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::acquire::{Acquisition, Block, Command, Event, LogRequest};
use crate::calib::{ZeroCal, ZeroOutcome};
use crate::config::SaveTarget;
use crate::export;
use crate::keys::{Act, KeyMap, View};
use crate::logger::view::LogView;
use crate::logger::view::Move;
use crate::logger::writer::{self as log_writer, LogState, Shared, StopReason};
use crate::logger::{self, csv as log_csv};
use crate::panel::{Action, DeviceInfo, Outcome, Panel};
use crate::probecal::ProbeCal;
use crate::scope::{Scope, MATH};
use crate::settings::*;
use crate::stream::pipeline::Published;

/// What a logger key does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LogKey {
    /// Start or stop recording (everywhere).
    Record,
    /// Switch between the scope and the log view (everywhere).
    ToggleView,
    /// Scroll or zoom the log view.
    Go(Move),
    /// Time cursors on / off.
    Cursors,
    /// Move cursor A (0) or B (1) by dot columns.
    Cursor(usize, i64),
    /// Time labels: clock / seconds since start.
    Axis,
    /// Export the log as CSV (between the cursors when they are on).
    Export,
}

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

/// A log to delete once the user says yes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeleteLog {
    pub path: PathBuf,
    /// Size of the log and its .idx file, in bytes.
    pub bytes: u64,
}

impl DeleteLog {
    /// The question, like `Delete log-20261107-185153.ohlog (2.3 MB)? y / n`.
    pub fn question(&self) -> String {
        let name = self
            .path
            .file_name()
            .map_or_else(|| self.path.display().to_string(), |n| n.to_string_lossy().into());
        format!("Delete {name} ({})? y / n", logger::fmt_bytes(self.bytes))
    }
}

pub struct App {
    pub scope: Scope,
    acq: Option<Acquisition>,
    pub device_name: String,
    pub connected: bool,
    /// Progress text while connecting (firmware upload and such).
    pub status: Option<String>,
    pub message: Option<Message>,
    pub help: bool,
    /// A log waiting for `y` / `n` before it is deleted.
    pub confirm_delete: Option<DeleteLog>,
    /// First help line on screen, when the help scrolls.
    pub help_scroll: usize,
    /// Largest `help_scroll` for the screen, set when the help is drawn.
    pub help_max: std::cell::Cell<usize>,
    pub quit: bool,
    pub export_dir: PathBuf,
    /// Settings file, when settings are saved on quit.
    pub settings_path: Option<PathBuf>,
    /// Notes from connecting, like where the calibration came from.
    pub device_notes: Vec<String>,
    pub panel: Panel,
    pub panel_open: bool,
    /// The key bindings.
    pub keys: KeyMap,
    /// Problems found in the config file, for Settings › Files.
    pub config_warnings: Vec<String>,
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
    /// The config file to save to (on quit, and right after a
    /// calibration). `None`: never save.
    pub save_target: Option<SaveTarget>,
    /// The zero calibration waits for `y` (the inputs grounded).
    pub ask_zero_cal: bool,
    /// The running zero calibration.
    pub zero_cal: Option<ZeroCal>,
    /// The open probe helper (calibration output).
    pub probe_cal: Option<ProbeCal>,
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
            confirm_delete: None,
            help_scroll: 0,
            help_max: std::cell::Cell::new(0),
            quit: false,
            export_dir: PathBuf::from("."),
            settings_path: None,
            device_notes: Vec::new(),
            panel: Panel::new(),
            panel_open: false,
            keys: KeyMap::default(),
            config_warnings: Vec::new(),
            sent_running: None,
            rec: None,
            log_view: None,
            show_log: false,
            log_dir_override: None,
            log_free: None,
            free_at: None,
            save_target: None,
            ask_zero_cal: false,
            zero_cal: None,
            probe_cal: None,
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
        if let Some(p) = self.acq.as_ref().and_then(|a| a.latest()) {
            changed |= self.handle_published(*p);
        }
        if self.zero_cal.as_ref().is_some_and(|z| z.timed_out()) {
            self.end_zero_cal(Err("no data from the scope".into()));
            changed = true;
        }
        if changed {
            if let (Some(p), Some(f)) = (&mut self.probe_cal, &self.scope.frame) {
                p.frame(f);
            }
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
            step_ns: logger::step_ns(logger::fit_step(
                s.logger.step,
                channels,
                s.acquisition.max_adc_rate,
            )),
            channels,
            probe: [0, 1].map(|c| s.channels[c].probe),
            invert: [0, 1].map(|c| s.channels[c].invert),
            volts_per_div: [0, 1].map(|c| s.channels[c].volts_per_div()),
            gains: hw.gains,
            dc: hw.dc,
            cal_freq: hw.cal_freq,
            max_adc: s.acquisition.max_adc_rate,
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

    /// True when `path` is the log being recorded (or waiting to start).
    fn is_recording(&self, path: &std::path::Path) -> bool {
        self.recording()
            && self
                .rec
                .as_ref()
                .is_some_and(|r| r.lock().unwrap().path == path)
    }

    /// Ask (y / n, in the message box) before deleting the log at `path`.
    /// The log being recorded is never deleted.
    pub fn ask_delete_log(&mut self, path: &std::path::Path) {
        if self.is_recording(path) {
            self.notify("That log is being recorded: stop it (r) before deleting it", true);
            return;
        }
        let size = |p: &std::path::Path| std::fs::metadata(p).map_or(0, |m| m.len());
        let bytes = size(path) + size(&logger::pyramid::Pyramid::sidecar(path));
        self.confirm_delete = Some(DeleteLog {
            path: path.to_path_buf(),
            bytes,
        });
    }

    /// Delete the log at `path` and its `.idx` summary file. A log open
    /// in the log view is closed first; the log being recorded and files
    /// that are not logs are never deleted.
    pub fn delete_log(&mut self, path: &std::path::Path) {
        if self.is_recording(path) {
            self.notify("That log is being recorded: stop it (r) before deleting it", true);
            return;
        }
        if path.extension().is_none_or(|e| e != logger::EXT) {
            self.notify(format!("Not a log: {}", path.display()), true);
            return;
        }
        if self.log_view.as_ref().is_some_and(|v| v.file.path == path) {
            self.log_view = None;
            self.show_log = false;
        }
        let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into());
        if let Err(e) = std::fs::remove_file(path) {
            self.notify(format!("Could not delete {name}: {e}"), true);
            return;
        }
        let idx = logger::pyramid::Pyramid::sidecar(path);
        match std::fs::remove_file(&idx) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                self.notify(format!("Deleted {name}, but not its .idx: {e}"), true);
            }
            _ => self.notify(format!("Deleted {name}"), false),
        }
        // Keep the cursor of the panel's log list on a log.
        let n = self.recent_logs().len();
        self.panel.value = self.panel.value.min(n.saturating_sub(1));
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

    /// A logger action.
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

    /// Take the newest frame of the stream processing.
    pub fn handle_published(&mut self, p: Published) -> bool {
        if self.zero_cal.is_some() && p.gen == self.scope.gen {
            if let Some(b) = &p.snapshot {
                self.zero_cal_block(b);
            }
        }
        self.scope.take(p)
    }

    /// Take one block (in volts at the BNC) as if the stream processing
    /// made a frame of it. The UI tests feed blocks this way.
    pub fn handle_block(&mut self, b: Block) -> bool {
        if self.zero_cal.is_some() && b.gen == self.scope.gen {
            self.zero_cal_block(&b);
        }
        self.scope.push(b)
    }

    /// Handle one event from the acquisition thread.
    pub fn handle_event(&mut self, ev: Event) -> bool {
        match ev {
            Event::Restart(why) => {
                self.notify(why.message(), true);
                true
            }
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
        if let Some((pgen, p)) = self.scope.sync_proc() {
            if let Some(a) = &self.acq {
                a.send(Command::Process {
                    pgen,
                    p: Box::new(p),
                });
            }
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
        // Ctrl-C always quits, whatever the bindings are.
        let ctrl_c = key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'));
        if ctrl_c {
            self.quit = true;
            return;
        }
        if std::mem::take(&mut self.ask_zero_cal) {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.start_zero_cal(),
                _ => self.notify("No zero calibration", false),
            }
            return;
        }
        if self.zero_cal.is_some() {
            if key.code == KeyCode::Esc || self.keys.is(Act::Quit, &key) {
                self.end_zero_cal(Err("stopped".into()));
            } else {
                self.notify("Zero calibration runs: Esc stops it", false);
            }
            return;
        }
        if self.probe_cal.is_some() {
            self.probe_cal_key(key);
            return;
        }
        if let Some(d) = self.confirm_delete.take() {
            // Only y deletes; n, Esc and any other key keep the log.
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.delete_log(&d.path),
                _ => self.notify("Kept the log", false),
            }
            return;
        }
        if self.help {
            // A long help scrolls; any other key closes it, and the quit
            // keys still quit.
            let max = self.help_max.get();
            if max > 0 {
                let page = 10;
                let s = self.help_scroll.min(max);
                let to = if self.keys.is(Act::PanelDown, &key) {
                    Some(s + 1)
                } else if self.keys.is(Act::PanelUp, &key) {
                    Some(s.saturating_sub(1))
                } else {
                    match key.code {
                        KeyCode::PageDown => Some(s + page),
                        KeyCode::PageUp => Some(s.saturating_sub(page)),
                        KeyCode::Home => Some(0),
                        KeyCode::End => Some(max),
                        _ => None,
                    }
                };
                if let Some(to) = to {
                    self.help_scroll = to.min(max);
                    return;
                }
            }
            if self.keys.is(Act::Quit, &key) {
                self.quit = true;
            }
            self.help = false;
            return;
        }
        if self.panel_open {
            self.panel_key(key);
            return;
        }
        let views: &[View] = if self.show_log {
            &[View::Global, View::Log, View::Scope]
        } else {
            &[View::Global, View::Scope]
        };
        if let Some(a) = self.keys.action(views, &key) {
            self.act(a);
        }
    }

    /// Do what a key action of the scope or log view does.
    pub fn act(&mut self, a: Act) {
        let log_move = match a {
            Act::LogBack => Some(LogKey::Go(Move::Divs(-1))),
            Act::LogOn => Some(LogKey::Go(Move::Divs(1))),
            Act::LogPageBack => Some(LogKey::Go(Move::Pages(-1))),
            Act::LogPageOn => Some(LogKey::Go(Move::Pages(1))),
            Act::LogStart => Some(LogKey::Go(Move::Home)),
            Act::LogEnd => Some(LogKey::Go(Move::End)),
            Act::LogZoomIn => Some(LogKey::Go(Move::ZoomIn)),
            Act::LogZoomOut => Some(LogKey::Go(Move::ZoomOut)),
            Act::LogFit => Some(LogKey::Go(Move::Fit)),
            Act::LogNative => Some(LogKey::Go(Move::Native)),
            Act::LogCursors => Some(LogKey::Cursors),
            Act::LogCursor(i, on) => Some(LogKey::Cursor(i, if on { 10 } else { -10 })),
            Act::LogCursorFine(i, on) => Some(LogKey::Cursor(i, if on { 1 } else { -1 })),
            Act::LogTimeLabels => Some(LogKey::Axis),
            Act::LogExport => Some(LogKey::Export),
            Act::Record => Some(LogKey::Record),
            Act::LogView => Some(LogKey::ToggleView),
            _ => None,
        };
        if let Some(k) = log_move {
            self.log_key(k);
            return;
        }
        let sel = self.scope.settings.selected;
        let s = &mut self.scope.settings;
        match a {
            Act::Quit => self.quit = true,
            Act::Help => {
                self.help = true;
                self.help_scroll = 0;
            }
            Act::Settings => {
                self.panel_open = true;
                return;
            }

            // Run control.
            Act::RunStop => self.action(Action::RunStop),
            Act::Single => self.action(Action::Single),
            Act::Autoset => self.action(Action::Autoset),

            // Channels.
            Act::NextTrace => s.selected = (sel + 1) % 3,
            Act::PrevTrace => s.selected = (sel + 2) % 3,
            Act::Ch1 => s.channels[0].enabled ^= true,
            Act::Ch2 => s.channels[1].enabled ^= true,
            Act::Math => s.math.enabled ^= true,
            Act::VoltsSmaller | Act::VoltsBigger => {
                let (idx, fine) = volt_idx(s, sel);
                *idx = coarse_move(VOLT_DIVS.len(), *idx, *fine, a == Act::VoltsBigger);
                *fine = 1.0;
            }
            Act::VoltsSmallerFine | Act::VoltsBiggerFine => {
                let (idx, fine) = volt_idx(s, sel);
                let v = VOLT_DIVS[*idx] * *fine;
                (*idx, *fine) = fine_move(&VOLT_DIVS, v, a == Act::VoltsBiggerFine);
            }
            Act::TraceUp => move_offset(s, sel, 0.5),
            Act::TraceDown => move_offset(s, sel, -0.5),
            Act::TraceUpFine => move_offset(s, sel, 0.1),
            Act::TraceDownFine => move_offset(s, sel, -0.1),
            Act::TraceUpAlt => move_offset(s, sel, 0.05),
            Act::TraceDownAlt => move_offset(s, sel, -0.05),
            Act::TraceUpFineAlt => move_offset(s, sel, 0.01),
            Act::TraceDownFineAlt => move_offset(s, sel, -0.01),
            Act::TraceCenter => *offset(s, sel) = 0.0,
            Act::Coupling if sel < 2 => {
                s.channels[sel].coupling = s.channels[sel].coupling.toggle()
            }
            Act::Probe if sel < 2 => {
                let is_src = s.trigger_source() == sel;
                let ch = &mut s.channels[sel];
                let old = ch.probe as f64;
                ch.probe = if ch.probe == 1 { 10 } else { 1 };
                // Keep the trigger level where it is on the screen.
                if is_src {
                    s.trigger.level *= ch.probe as f64 / old;
                }
            }
            Act::Invert if sel < 2 => s.channels[sel].invert ^= true,
            Act::MathFunction => {
                s.math.mode = s.math.mode.next();
                s.math.enabled = true;
            }

            // Time base.
            Act::TimeSmaller | Act::TimeBigger => {
                s.time_idx = coarse_move(
                    TIME_DIVS.len(),
                    s.time_idx,
                    s.time_fine,
                    a == Act::TimeBigger,
                );
                s.time_fine = 1.0;
            }
            Act::TimeSmallerFine | Act::TimeBiggerFine => {
                let up = a == Act::TimeBiggerFine;
                (s.time_idx, s.time_fine) = fine_move(&TIME_DIVS, s.time_per_div(), up);
            }

            // Trigger.
            Act::TriggerMode => s.trigger.mode = s.trigger.mode.next(),
            Act::TriggerSource => s.trigger.source = 1 - s.trigger.source.min(1),
            Act::TriggerEdge => s.trigger.slope = s.trigger.slope.next(),
            Act::TriggerUp => s.trigger.level += 0.1 * s.channels[s.trigger_source()].volts_per_div(),
            Act::TriggerDown => {
                s.trigger.level -= 0.1 * s.channels[s.trigger_source()].volts_per_div()
            }
            Act::TriggerToMiddle => self.action(Action::TriggerToMiddle),
            Act::TriggerLeft => s.trigger.position = (s.trigger.position - 0.05).max(0.0),
            Act::TriggerRight => s.trigger.position = (s.trigger.position + 0.05).min(1.0),
            Act::TriggerUpFine => {
                s.trigger.level += 0.01 * s.channels[s.trigger_source()].volts_per_div()
            }
            Act::TriggerDownFine => {
                s.trigger.level -= 0.01 * s.channels[s.trigger_source()].volts_per_div()
            }
            Act::TriggerLeftFine => s.trigger.position = (s.trigger.position - 0.005).max(0.0),
            Act::TriggerRightFine => s.trigger.position = (s.trigger.position + 0.005).min(1.0),

            // Display.
            Act::Averaging => s.display.averaging = next_in(&AVERAGING, s.display.averaging),
            Act::Persistence => {
                s.display.persistence = next_in(&PERSISTENCE, s.display.persistence);
                self.scope.history.clear();
            }
            Act::TraceMode => s.display.trace_mode = s.display.trace_mode.next(),
            Act::LogTraceMode => s.logger.trace_mode = s.logger.trace_mode.next(),
            Act::Interpolation => s.display.interpolation = s.display.interpolation.next(),
            Act::MeasurePanel => s.display.measurements ^= true,
            Act::Spectrum => s.display.spectrum ^= true,
            Act::SpectrumZoom => {
                s.display.spectrum_zoom = (s.display.spectrum_zoom + 1) % (MAX_SPECTRUM_ZOOM + 1)
            }
            Act::FftWindow => s.display.window = s.display.window.next(),
            Act::Xy => s.display.xy ^= true,

            // Cursors.
            Act::Cursors => {
                s.cursors.hidden ^= true;
                if !s.cursors.hidden && !(0..4).any(|i| s.cursors.get(i).enabled) {
                    self.notify("No cursor is on: turn them on in Settings › Cursors", false);
                }
            }
            Act::CursorMove(i, up) => move_cursor(s, i, if up { 0.1 } else { -0.1 }),
            Act::CursorFine(i, up) => move_cursor(s, i, if up { 0.01 } else { -0.01 }),

            // Calibration output.
            Act::CalFreqUp => s.cal_freq = next_in(&CAL_FREQS, s.cal_freq),
            Act::CalFreqDown => {
                let i = CAL_FREQS.iter().position(|f| *f == s.cal_freq).unwrap_or(0);
                s.cal_freq = CAL_FREQS[(i + CAL_FREQS.len() - 1) % CAL_FREQS.len()];
            }

            // Files.
            Act::ExportCsv => self.action(Action::ExportCsv),
            Act::ExportJson => self.action(Action::ExportJson),
            Act::Reset => self.action(Action::Reset),
            Act::ZeroCal => self.action(Action::ZeroCal),
            Act::ProbeCal => self.action(Action::ProbeCal),
            _ => return,
        }
        self.changed();
    }

    /// A key while the settings panel is open.
    fn panel_key(&mut self, key: KeyEvent) {
        let d = self.device_info();
        let persistence = self.scope.settings.display.persistence;
        match self
            .panel
            .handle_key(key, &mut self.scope.settings, &d, &self.keys)
        {
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
            Action::DeleteLog(i) => {
                if let Some(p) = self.recent_logs().get(i).cloned() {
                    self.ask_delete_log(&p);
                }
            }
            Action::ZeroCal => self.ask_zero_cal(),
            Action::ProbeCal => self.open_probe_cal(),
            Action::ClearGainCal => {
                self.scope.settings.calibration.gain = [1.0; 2];
                let saved = self.save_calibration();
                self.notify(format!("Gain corrections cleared{saved}"), false);
            }
            Action::ClearZeroCal => {
                self.scope.settings.calibration.clear_zero();
                let saved = self.save_calibration();
                self.notify(
                    format!("Zero calibration cleared: the EEPROM values again{saved}"),
                    false,
                );
            }
            Action::Reset => {
                let keep = self.scope.settings.selected;
                // The calibration is not a setting to reset.
                let cal = self.scope.settings.calibration.clone();
                self.scope.settings = Settings::default();
                self.scope.settings.selected = keep;
                self.scope.settings.calibration = cal;
                self.scope.history.clear();
                self.notify("Settings reset to defaults", false);
            }
        }
    }

    /// Ask for the grounded inputs before the zero calibration.
    fn ask_zero_cal(&mut self) {
        if self.recording() {
            self.notify("Stop the logger before the zero calibration", true);
        } else if !self.connected {
            self.notify("Zero calibration needs the scope", true);
        } else {
            self.panel_open = false;
            self.show_log = false;
            self.ask_zero_cal = true;
        }
    }

    /// Start the zero calibration (the inputs are grounded).
    pub fn start_zero_cal(&mut self) {
        let z = ZeroCal::new(self.scope.settings.clone(), self.scope.running);
        self.scope.settings = z.settings();
        self.scope.set_running(true);
        self.notify(z.progress(), false);
        self.zero_cal = Some(z);
        self.sync();
    }

    /// A block for the zero calibration.
    fn zero_cal_block(&mut self, b: &crate::acquire::Block) {
        let Some(z) = &mut self.zero_cal else {
            return;
        };
        match z.block(b) {
            ZeroOutcome::Busy => {}
            ZeroOutcome::Next => {
                self.scope.settings = z.settings();
                let text = z.progress();
                self.notify(text, false);
            }
            ZeroOutcome::Done(cal) => self.end_zero_cal(Ok(cal)),
            ZeroOutcome::Failed(e) => self.end_zero_cal(Err(e)),
        }
    }

    /// End the zero calibration: the user's settings back, with the new
    /// zero values when it worked (saved in the config file at once).
    fn end_zero_cal(&mut self, result: Result<crate::settings::CalibrationSettings, String>) {
        let Some(z) = self.zero_cal.take() else {
            return;
        };
        let cal = self.scope.settings.calibration.clone();
        self.scope.settings = z.saved.clone();
        self.scope.settings.calibration = cal;
        self.scope.set_running(z.was_running);
        match result {
            Ok(cal) => {
                self.scope.settings.calibration = cal;
                let saved = self.save_calibration();
                let (d, at) = &z.biggest;
                self.notify(
                    format!(
                        "Zero calibration done: {} values{saved}. Biggest change {d:+.1} counts ({at})",
                        self.scope.settings.calibration.zero_count()
                    ),
                    false,
                );
            }
            Err(e) => self.notify(format!("Zero calibration failed: {e}"), true),
        }
        self.sync();
    }

    /// Put the user's settings back if a helper has the scope (before
    /// the settings are saved on quit).
    pub fn close_helpers(&mut self) {
        if self.zero_cal.is_some() {
            self.end_zero_cal(Err("stopped".into()));
        }
        self.close_probe_cal();
    }

    /// Open the probe helper on the selected channel.
    pub fn open_probe_cal(&mut self) {
        if self.recording() {
            self.notify("Stop the logger before the probe helper", true);
            return;
        }
        self.panel_open = false;
        self.show_log = false;
        let ch = self.scope.settings.selected.min(1);
        let p = ProbeCal::new(self.scope.settings.clone(), self.scope.running, ch);
        self.scope.settings = p.settings();
        self.scope.set_running(true);
        self.probe_cal = Some(p);
        self.changed();
    }

    /// Close the probe helper: the user's settings back (with a new gain
    /// correction, if one was saved).
    pub fn close_probe_cal(&mut self) {
        let Some(p) = self.probe_cal.take() else {
            return;
        };
        let cal = self.scope.settings.calibration.clone();
        self.scope.settings = p.saved;
        self.scope.settings.calibration = cal;
        self.scope.set_running(p.was_running);
        self.changed();
    }

    /// A key while the probe helper is open.
    fn probe_cal_key(&mut self, key: KeyEvent) {
        let Some(p) = &mut self.probe_cal else {
            return;
        };
        let c = p.channel;
        let gain = self.scope.settings.calibration.gain[c];
        let mut note: Option<(String, bool)> = None;
        let mut apply = true;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('C') => {
                self.close_probe_cal();
                self.notify("Probe helper closed: your settings are back", false);
                return;
            }
            KeyCode::Tab | KeyCode::BackTab => p.channel = 1 - c,
            KeyCode::Char('1') => p.channel = 0,
            KeyCode::Char('2') => p.channel = 1,
            KeyCode::Right | KeyCode::Char('g') | KeyCode::Char('+') => p.step_freq(true),
            KeyCode::Left | KeyCode::Char('G') | KeyCode::Char('-') => p.step_freq(false),
            KeyCode::Char('p') => {
                // The probe switch is the user's setting: it stays.
                let ch = &mut p.saved.channels[c];
                ch.probe = if ch.probe == 1 { 10 } else { 1 };
            }
            KeyCode::Enter => {
                apply = false;
                match p.new_gain(gain) {
                    Ok(g) => {
                        self.scope.settings.calibration.gain[c] = g;
                        p.changed();
                        let saved = self.save_calibration();
                        note = Some((
                            format!(
                                "CH{} gain correction x{g:.4} ({:+.2} %){saved}",
                                c + 1,
                                (g - 1.0) * 100.0
                            ),
                            false,
                        ));
                    }
                    Err(e) => note = Some((format!("No gain correction: {e}"), true)),
                }
            }
            KeyCode::Delete | KeyCode::Backspace | KeyCode::Char('x') => {
                apply = false;
                self.scope.settings.calibration.gain[c] = 1.0;
                p.changed();
                let saved = self.save_calibration();
                note = Some((format!("CH{} gain correction cleared{saved}", c + 1), false));
            }
            _ => {
                apply = false;
                note = Some((
                    format!("Probe helper: {}", crate::probecal::PROBE_KEYS),
                    false,
                ));
            }
        }
        if apply {
            if let Some(p) = &mut self.probe_cal {
                p.changed();
                let cal = self.scope.settings.calibration.clone();
                self.scope.settings = p.settings();
                self.scope.settings.calibration = cal;
            }
        }
        if let Some((text, error)) = note {
            self.notify(text, error);
        }
        self.changed();
    }

    /// Save the calibration in the config file now (only it: the other
    /// settings are saved on quit). Returns a note for the message.
    fn save_calibration(&mut self) -> String {
        let Some(t) = &mut self.save_target else {
            return " (not saved: no config file)".into();
        };
        let mut s = t.base.clone();
        s.calibration = self.scope.settings.calibration.clone();
        s.sanitize();
        match crate::config::save(t, &s) {
            Ok(_) => {
                t.base = s;
                ", saved in the config file".into()
            }
            Err(e) => format!(" (not saved: {e:#})"),
        }
    }

    /// Facts for the Device and Files parts of the settings panel.
    pub fn device_info(&self) -> DeviceInfo {
        DeviceInfo {
            name: self.device_name.clone(),
            connected: self.connected,
            notes: self.device_notes.clone(),
            rate: self.scope.hw.as_ref().map(|h| h.rate.rate),
            adc_rate: self.scope.hw.as_ref().map(|h| h.rate.raw),
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
            config_warnings: self.config_warnings.clone(),
            settings_file: self
                .settings_path
                .as_ref()
                .map_or_else(|| "not saved".to_string(), |p| p.display().to_string()),
        }
    }

    /// Set the trigger level to the middle of the source signal.
    fn trigger_to_middle(&mut self) {
        let src = self.scope.settings.trigger_source();
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

/// The volts/div step and fine factor of trace `sel`.
fn volt_idx(s: &mut Settings, sel: usize) -> (&mut usize, &mut f64) {
    if sel == MATH {
        (&mut s.math.volt_idx, &mut s.math.units_fine)
    } else {
        let c = &mut s.channels[sel];
        (&mut c.volt_idx, &mut c.volts_fine)
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
    *o = ((*o + by) * 100.0).round() / 100.0;
    *o = o.clamp(-V_DIVS / 2.0, V_DIVS / 2.0);
}

/// Move cursor `i` (0 time A, 1 time B, 2 voltage A, 3 voltage B) by
/// `by` divisions (0.1, or 0.01 for the fine keys: less than one braille
/// dot below 50 cells per division). Only cursors that are on screen move.
fn move_cursor(s: &mut Settings, i: usize, by: f64) {
    if !s.cursors.shown(i) {
        return;
    }
    let limit = if CursorSettings::is_time(i) {
        H_DIVS / 2.0
    } else {
        V_DIVS / 2.0
    };
    let c = s.cursors.get_mut(i);
    c.position = (((c.position + by) * 100.0).round() / 100.0).clamp(-limit, limit);
}
