//! openhantek-tui: a terminal oscilloscope for the Hantek DSO-6022BE.

use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use clap::Parser;
use ratatui::crossterm::event::{self, Event as TermEvent, KeyEventKind};

use openhantek_tui::acquire::{Acquisition, Opener};
use openhantek_tui::app::App;
use openhantek_tui::device::{open_demo, open_hardware};
use openhantek_tui::settings::Settings;
use openhantek_tui::{config, ui};

#[derive(Parser, Debug)]
#[command(version, about = "Terminal oscilloscope for the Hantek DSO-6022BE")]
struct Args {
    /// Use a fake scope with made-up signals (no hardware needed).
    #[arg(long)]
    demo: bool,
    /// Firmware file (Intel HEX) for the scope.
    #[arg(long, value_name = "FILE")]
    firmware: Option<PathBuf>,
    /// Config file with the settings and keys [default:
    /// ~/.config/openhantek-tui/config.toml].
    #[arg(long, value_name = "FILE", alias = "settings")]
    config: Option<PathBuf>,
    /// Do not load or save the config file (default settings and keys).
    #[arg(long, alias = "no-config")]
    no_settings: bool,
    /// Folder for exported CSV and JSON files [default: current folder].
    #[arg(long, value_name = "DIR")]
    export_dir: Option<PathBuf>,
    /// Folder for envelope logs [default: ~/.local/share/openhantek-tui/logs].
    #[arg(long, value_name = "DIR")]
    log_dir: Option<PathBuf>,
    /// Open this log file in the log view.
    #[arg(long, value_name = "FILE")]
    open_log: Option<PathBuf>,
    /// Run autoset at start.
    #[arg(long)]
    autoset: bool,
    /// No screen: capture once, print the measurements, then quit.
    #[arg(long)]
    measure: bool,
    /// With --measure: also save the capture to this CSV file.
    #[arg(long, value_name = "FILE", requires = "measure")]
    csv: Option<PathBuf>,
}

fn opener(args: &Args) -> Opener {
    if args.demo {
        Box::new(|_| Ok(open_demo()))
    } else {
        let fw = args.firmware.clone();
        Box::new(move |status| open_hardware(fw.as_deref(), status))
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    // The config file: --config, or the default one (which takes the
    // values of the old settings.toml once, when it is new).
    let loaded = if args.no_settings {
        None
    } else if let Some(p) = &args.config {
        Some(config::load(p, None))
    } else {
        config::config_path().map(|p| config::load(&p, config::old_settings_path().as_deref()))
    };
    let (settings, keys, warnings, notes, save) = match loaded {
        Some(l) => (l.settings, l.keys, l.warnings, l.notes, l.save),
        None => (
            Settings::default(),
            Default::default(),
            Vec::new(),
            Vec::new(),
            None,
        ),
    };
    let acq = Acquisition::spawn(opener(&args));
    let mut app = App::new(settings, Some(acq));
    if let Some(dir) = &args.export_dir {
        app.export_dir = dir.clone();
    }
    app.keys = keys;
    app.settings_path = save.as_ref().map(|t| t.path.clone());
    app.save_target = save;
    app.log_dir_override = args.log_dir.clone();
    if let Some(p) = &args.open_log {
        app.open_log(p);
    }
    if !notes.is_empty() {
        app.notify(notes.join(" · "), false);
    }
    if !warnings.is_empty() {
        let n = warnings.len();
        let more = if n > 1 {
            format!(" (and {} more)", n - 1)
        } else {
            String::new()
        };
        app.notify(format!("Config: {}{more}", warnings[0]), true);
        app.config_warnings = warnings.clone();
    }
    if args.autoset {
        app.scope.start_autoset();
    }

    if args.measure {
        return match measure_once(&mut app, args.csv.as_deref()) {
            // A closed pipe (like `| head`) is not an error.
            Err(e)
                if e.downcast_ref::<io::Error>()
                    .is_some_and(|e| e.kind() == io::ErrorKind::BrokenPipe) =>
            {
                Ok(())
            }
            r => r,
        };
    }

    let mut terminal = ratatui::init();
    let enhanced = push_keyboard_enhancement();
    let result = run(&mut terminal, &mut app);
    if enhanced {
        pop_keyboard_enhancement();
    }
    ratatui::restore();
    app.close_helpers();
    if let Some(t) = &app.save_target {
        if let Err(e) = config::save(t, &app.scope.settings) {
            eprintln!("Could not save the settings: {e:#}");
        }
    }
    result.map_err(Into::into)
}

/// Ask the terminal for unambiguous key codes (the kitty keyboard
/// protocol), so Ctrl-h is not Backspace and Ctrl-Shift-h is not Ctrl-h.
/// Only when the terminal says it supports it (kitty, foot, WezTerm,
/// Ghostty, Alacritty; not VTE terminals like xfce4-terminal or GNOME
/// Terminal, not tmux). Undone on exit and on a panic.
fn push_keyboard_enhancement() -> bool {
    use ratatui::crossterm::event::{KeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
    use ratatui::crossterm::{execute, terminal};
    if !terminal::supports_keyboard_enhancement().unwrap_or(false) {
        return false;
    }
    let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES;
    if execute!(io::stdout(), PushKeyboardEnhancementFlags(flags)).is_err() {
        return false;
    }
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        pop_keyboard_enhancement();
        hook(info);
    }));
    true
}

fn pop_keyboard_enhancement() {
    use ratatui::crossterm::event::PopKeyboardEnhancementFlags;
    let _ = ratatui::crossterm::execute!(io::stdout(), PopKeyboardEnhancementFlags);
}

/// Shortest time between two frames: at most 60 frames per second (the
/// stream processing publishes no faster either).
use openhantek_tui::stream::pipeline::FRAME_TIME;
/// Draw at least this often, also when nothing changed (the clock, old
/// messages going away).
const IDLE_REDRAW: Duration = Duration::from_millis(500);

/// Should the loop draw now, `since` the last frame? A change waits for
/// the frame time, so fast keys and blocks never draw more than 60
/// frames per second; with no change, the idle redraw.
fn draw_due(dirty: bool, since: Duration) -> bool {
    (dirty && since >= FRAME_TIME) || since >= IDLE_REDRAW
}

/// How long to wait for a key before the next look at the scope: until
/// the next frame may be drawn, at least 1 ms, at most a frame time.
fn wait_time(dirty: bool, since: Duration) -> Duration {
    let wait = if dirty {
        FRAME_TIME.saturating_sub(since)
    } else {
        FRAME_TIME
    };
    wait.max(Duration::from_millis(1))
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> io::Result<()> {
    let mut last_draw = Instant::now() - IDLE_REDRAW;
    let mut dirty = true;
    loop {
        dirty |= app.tick();
        if draw_due(dirty, last_draw.elapsed()) {
            terminal.draw(|f| ui::draw(f, app))?;
            last_draw = Instant::now();
            dirty = false;
        }
        if app.quit {
            return Ok(());
        }
        // Take every waiting key before the next frame: a burst of keys
        // (a held key) makes one frame, not one per key.
        let mut wait = wait_time(dirty, last_draw.elapsed());
        while event::poll(wait)? {
            match event::read()? {
                TermEvent::Key(key) if key.kind == KeyEventKind::Press => {
                    app.handle_key(key);
                    dirty = true;
                }
                TermEvent::Resize(_, _) => dirty = true,
                _ => {}
            }
            if app.quit {
                return Ok(());
            }
            wait = Duration::ZERO;
        }
    }
}

/// Capture until a frame comes (triggered if the trigger can), then print
/// the measurements of the enabled channels.
fn measure_once(app: &mut App, csv: Option<&std::path::Path>) -> Result<()> {
    let t0 = Instant::now();
    let mut frames = 0;
    loop {
        if app.tick() && app.scope.frame.is_some() && !app.scope.autoset_running() {
            frames += 1;
            let triggered = app.scope.frame.as_ref().is_some_and(|f| f.triggered);
            // Skip the first frame: it may come from the old settings.
            if frames >= 2 && (triggered || t0.elapsed() > Duration::from_secs(3)) {
                break;
            }
        }
        if let Some(m) = app.current_message().filter(|m| m.error) {
            // Opening failed (no scope, no permission): stop soon.
            if !app.connected && t0.elapsed() > Duration::from_secs(2) {
                bail!("{}", m.text);
            }
        }
        if t0.elapsed() > Duration::from_secs(30) {
            bail!("no data from the scope within 30 s");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let f = app.scope.frame.as_ref().expect("frame");
    let s = &app.scope.settings;
    let mut out = io::stdout().lock();
    writeln!(
        out,
        "{} · {}/div · {} · {}",
        app.device_name,
        openhantek_tui::si::fmt_si(s.time_per_div(), "s"),
        openhantek_tui::si::fmt_si(f.rate, "S/s"),
        if f.triggered {
            "triggered"
        } else {
            "not triggered"
        }
    )?;
    for t in 0..3 {
        if !ui::trace_enabled(s, t) {
            continue;
        }
        let Some(m) = app.scope.measure(t) else {
            continue;
        };
        let unit = if t == 2 { s.math.mode.unit() } else { "V" };
        let rows = ui::measurement_rows(&m, unit);
        let text: Vec<String> = rows.iter().map(|(k, v)| format!("{k} {v}")).collect();
        writeln!(out, "{}: {}", ["CH1", "CH2", "MATH"][t], text.join(", "))?;
    }
    if let Some(path) = csv {
        let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
        let saved = app.export_to(dir.unwrap_or(std::path::Path::new(".")), false)?;
        std::fs::rename(&saved, path)?;
        writeln!(out, "Saved {}", path.display())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_most_60_frames_per_second() {
        let ms = Duration::from_millis;
        // A change right after a frame waits for the frame time.
        assert!(!draw_due(true, ms(0)));
        assert!(!draw_due(true, ms(16)));
        assert!(draw_due(true, ms(17)));
        // No change: only the idle redraw.
        assert!(!draw_due(false, ms(100)));
        assert!(draw_due(false, ms(500)));
        // A pending change waits only until the frame is due.
        assert_eq!(wait_time(true, ms(10)), FRAME_TIME - ms(10));
        assert_eq!(wait_time(true, ms(40)), ms(1));
        assert_eq!(wait_time(false, ms(0)), FRAME_TIME);
        assert!(FRAME_TIME.as_secs_f64() * 60.0 >= 0.99999);
    }
}
