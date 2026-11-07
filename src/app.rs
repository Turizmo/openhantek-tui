//! App state: the scope, the link to the acquisition thread, and the keys.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::acquire::{Acquisition, Command, Event};
use crate::export;
use crate::scope::{Scope, MATH};
use crate::settings::*;

/// Text on the message line.
#[derive(Clone, Debug)]
pub struct Message {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

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
    sent_running: Option<bool>,
}

/// Averaging steps for the `A` key.
const AVERAGING: [usize; 7] = [1, 2, 4, 8, 16, 32, 64];
/// Persistence steps for the `P` key.
const PERSISTENCE: [usize; 5] = [0, 2, 4, 8, 16];

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
            sent_running: None,
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
        changed | self.sync()
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
        let sel = self.scope.settings.selected;
        let s = &mut self.scope.settings;
        match key.code {
            KeyCode::Char('c') if ctrl => self.quit = true,
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('?') | KeyCode::F(1) => self.help = true,

            // Run control.
            KeyCode::Char(' ') => {
                let run = !self.scope.running;
                self.scope.set_running(run);
            }
            KeyCode::Char('o') => {
                self.scope.arm_single();
                self.notify("Single: waiting for a trigger", false);
            }
            KeyCode::Char('a') => self.scope.start_autoset(),

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
            KeyCode::Char('T') => self.trigger_to_middle(),
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
            KeyCode::Char('v') => s.cursors.mode = s.cursors.mode.next(),
            KeyCode::Char(c @ ('h' | 'l' | 'H' | 'L' | 'j' | 'k' | 'J' | 'K')) => move_cursor(s, c),

            // Calibration output.
            KeyCode::Char('g') => s.cal_freq = next_in(&CAL_FREQS, s.cal_freq),
            KeyCode::Char('G') => {
                let i = CAL_FREQS.iter().position(|f| *f == s.cal_freq).unwrap_or(0);
                s.cal_freq = CAL_FREQS[(i + CAL_FREQS.len() - 1) % CAL_FREQS.len()];
            }

            // Files.
            KeyCode::Char('E') => self.export(false),
            KeyCode::Char('W') => self.export(true),
            KeyCode::Char('R') => {
                let keep = self.scope.settings.selected;
                self.scope.settings = Settings::default();
                self.scope.settings.selected = keep;
                self.notify("Settings reset to defaults", false);
            }
            _ => return,
        }
        self.changed();
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
fn move_cursor(s: &mut Settings, key: char) {
    let c = &mut s.cursors;
    let (list, i, by, limit) = match key {
        'h' => (&mut c.t, 0, -0.1, H_DIVS / 2.0),
        'l' => (&mut c.t, 0, 0.1, H_DIVS / 2.0),
        'H' => (&mut c.t, 1, -0.1, H_DIVS / 2.0),
        'L' => (&mut c.t, 1, 0.1, H_DIVS / 2.0),
        'j' => (&mut c.v, 0, -0.1, V_DIVS / 2.0),
        'k' => (&mut c.v, 0, 0.1, V_DIVS / 2.0),
        'J' => (&mut c.v, 1, -0.1, V_DIVS / 2.0),
        _ => (&mut c.v, 1, 0.1, V_DIVS / 2.0),
    };
    list[i] = (((list[i] + by) * 10.0).round() / 10.0).clamp(-limit, limit);
    // Moving a cursor turns its kind on.
    c.mode = if matches!(key, 'h' | 'l' | 'H' | 'L') {
        CursorMode::Time
    } else {
        CursorMode::Volt
    };
}
