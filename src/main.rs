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
    /// Settings file [default: ~/.config/openhantek-tui/settings.toml].
    #[arg(long, value_name = "FILE")]
    settings: Option<PathBuf>,
    /// Do not load or save the settings file.
    #[arg(long)]
    no_settings: bool,
    /// Folder for exported CSV and JSON files [default: current folder].
    #[arg(long, value_name = "DIR")]
    export_dir: Option<PathBuf>,
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
    let settings_path = if args.no_settings {
        None
    } else {
        args.settings.clone().or_else(config::settings_path)
    };
    let (settings, load_error) = match &settings_path {
        Some(p) => config::load::<Settings>(p),
        None => (Settings::default(), None),
    };
    // Do not overwrite a settings file we could not read: keep the
    // user's file so they can fix it.
    let save_path = settings_path.filter(|_| load_error.is_none());
    let acq = Acquisition::spawn(opener(&args));
    let mut app = App::new(settings, Some(acq));
    if let Some(dir) = &args.export_dir {
        app.export_dir = dir.clone();
    }
    app.settings_path = save_path.clone();
    if let Some(e) = &load_error {
        app.notify(
            format!("{e}. Using defaults; the file will not be changed"),
            true,
        );
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
    let result = run(&mut terminal, &mut app);
    ratatui::restore();
    if let Some(p) = &save_path {
        if let Err(e) = config::save(p, &app.scope.settings) {
            eprintln!("Could not save settings to {}: {e:#}", p.display());
        }
    }
    result.map_err(Into::into)
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> io::Result<()> {
    let frame_time = Duration::from_millis(33);
    let mut last_draw = Instant::now() - frame_time;
    let mut dirty = true;
    loop {
        dirty |= app.tick();
        if dirty || last_draw.elapsed() >= Duration::from_millis(500) {
            terminal.draw(|f| ui::draw(f, app))?;
            last_draw = Instant::now();
            dirty = false;
        }
        if app.quit {
            return Ok(());
        }
        let wait = frame_time.saturating_sub(last_draw.elapsed());
        if event::poll(wait)? {
            match event::read()? {
                TermEvent::Key(key) if key.kind == KeyEventKind::Press => {
                    app.handle_key(key);
                    dirty = true;
                }
                TermEvent::Resize(_, _) => dirty = true,
                _ => {}
            }
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
