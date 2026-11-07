//! UI tests of the logger: the log view in each trace style, the REC
//! status line, the panel estimate, the log view keys and CSV export.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use openhantek_tui::app::App;
use openhantek_tui::logger::format::{ChannelInfo, Header};
use openhantek_tui::logger::writer::{self, LogState, Msg, Writer};
use openhantek_tui::settings::*;
use openhantek_tui::ui;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::Terminal;

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("oht-logui-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn header(step_ns: u64, n: u32) -> Header {
    let ch = ChannelInfo {
        enabled: true,
        ..Default::default()
    };
    Header {
        start_ns: 1_794_573_296_000_000_000,
        step_ns,
        adc_rate: 20e3,
        samples_per_step: n,
        channels: [ch.clone(), ch],
        device: "TESTSCOPE".into(),
    }
}

/// A 1 ms log of 20 000 steps: a slow sine with noise on CH1 (min/max
/// apart), 1 V on CH2, and a gap from step 8 000 to 9 000.
fn write_log(path: &Path) {
    let sh = writer::shared(path.to_path_buf(), 0);
    let w = Writer::spawn(sh.clone());
    w.send(Msg::Begin(header(1_000_000, 20)));
    let rec = |i: u64| {
        let v = (i as f32 / 2000.0 * std::f32::consts::TAU).sin() * 1.5;
        [v - 0.3, v + 0.3, v, 0.9, 1.1, 1.0]
    };
    let part = |r: std::ops::Range<u64>| r.flat_map(rec).collect::<Vec<f32>>();
    w.send(Msg::Records {
        first: 0,
        data: part(0..8_000),
    });
    w.send(Msg::Gap);
    w.send(Msg::Records {
        first: 9_000,
        data: part(9_000..20_000),
    });
    w.finish();
    assert!(matches!(sh.lock().unwrap().state, LogState::Done(_)));
}

fn screen(app: &App, w: u16, h: u16) -> Vec<String> {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer().clone();
    (0..h)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect())
        .collect()
}

fn key(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::from(code));
}

fn app_with_log(name: &str) -> (App, PathBuf) {
    let dir = temp_dir(name);
    let path = dir.join("log-test.ohlog");
    write_log(&path);
    let mut app = App::new(Settings::default(), None);
    app.export_dir = dir.clone();
    app.log_dir_override = Some(dir.clone());
    assert!(app.open_log(&path));
    // Sum the log up.
    for _ in 0..10 {
        app.tick();
    }
    (app, dir)
}

fn plot_rows(lines: &[String]) -> String {
    // The plot is inside the border, above the status lines.
    lines[1..lines.len() - 5].join("\n")
}

#[test]
fn log_view_draws_each_trace_style() {
    let (mut app, dir) = app_with_log("styles");
    assert!(app.show_log);
    for style in TraceStyle::ALL {
        app.scope.settings.display.trace_style = [style; 3];
        let lines = screen(&app, 120, 40);
        let all = lines.join("\n");
        assert!(
            all.contains("Log · log-test.ohlog · TESTSCOPE · step 1 ms"),
            "{all}"
        );
        // The time axis is the clock (UTC) of the log.
        assert!(all.contains("12:34:56"), "{all}");
        assert!(all.contains("0:00:20 long"), "{all}");
        assert!(all.contains("1 gap"), "{all}");
        let plot = plot_rows(&lines);
        // The gap is marked.
        assert!(plot.contains('┊'), "{style:?}\n{all}");
        let braille = plot
            .chars()
            .filter(|c| ('\u{2801}'..='\u{28FF}').contains(c))
            .count();
        match style {
            TraceStyle::BrailleThin | TraceStyle::BrailleBold => {
                assert!(braille > 300, "{style:?}: {braille}\n{all}")
            }
            TraceStyle::Line => {
                assert_eq!(braille, 0, "{all}");
                assert!(plot.matches(':').count() > 100, "{all}");
            }
            TraceStyle::Box => {
                assert_eq!(braille, 0, "{all}");
                assert!(plot.matches('░').count() > 100, "{all}");
                assert!(plot.contains('─'), "{all}");
            }
        }
    }
    // Small screens do not panic.
    for (w, h) in [(80, 24), (40, 12), (20, 8), (1, 1)] {
        screen(&app, w, h);
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn log_view_keys_zoom_scroll_cursors_and_export() {
    let (mut app, dir) = app_with_log("keys");
    screen(&app, 120, 40);
    let v = app.log_view.as_ref().unwrap();
    assert_eq!(v.zoom, None);
    let w = v.width.get();
    key(&mut app, KeyCode::Char('+'));
    assert!(app.log_view.as_ref().unwrap().zoom.is_some());
    key(&mut app, KeyCode::Home);
    let v = app.log_view.as_ref().unwrap();
    assert_eq!(v.left(w), 0);
    assert!(!v.follow);
    key(&mut app, KeyCode::Right);
    assert!(app.log_view.as_ref().unwrap().left(w) > 0);
    key(&mut app, KeyCode::End);
    assert!(app.log_view.as_ref().unwrap().follow);
    key(&mut app, KeyCode::Char('F'));
    assert_eq!(app.log_view.as_ref().unwrap().zoom, None);
    // Cursors and their readout.
    key(&mut app, KeyCode::Char('v'));
    key(&mut app, KeyCode::Char('l'));
    key(&mut app, KeyCode::Char('H'));
    let v = app.log_view.as_ref().unwrap();
    assert!(v.cursors);
    let r = v.cursor_range();
    assert!(r.end > r.start);
    let all = screen(&app, 160, 40).join("\n");
    assert!(all.contains("ΔT"), "{all}");
    assert!(all.contains("CH2 min 900 mV max 1.1 V mean 1 V"), "{all}");
    // The time labels in seconds.
    key(&mut app, KeyCode::Char('t'));
    assert_eq!(app.scope.settings.logger.time_axis, TimeAxis::Seconds);
    let all = screen(&app, 120, 40).join("\n");
    assert!(all.contains("10.0s") || all.contains("10s"), "{all}");
    // E exports between the cursors.
    key(&mut app, KeyCode::Char('E'));
    let msg = app.current_message().unwrap().text.clone();
    assert!(msg.starts_with("Saved "), "{msg}");
    let path = msg.trim_start_matches("Saved ");
    let text = std::fs::read_to_string(path).unwrap();
    let rows = text.lines().count() as u64 - 1;
    assert_eq!(
        text.lines().next().unwrap(),
        "time_utc,ch1_min,ch1_max,ch1_mean,ch2_min,ch2_max,ch2_mean"
    );
    // Rows: the steps between the cursors, minus the gap, plus a gap row.
    let gap = (8_000..9_000u64).filter(|i| r.contains(i)).count() as u64;
    let gap_row = (r.start < 8_000 && r.end > 9_000) as u64;
    assert_eq!(rows, r.end - r.start - gap + gap_row);
    // The whole log in seconds since start.
    app.scope.settings.logger.csv_time = CsvTime::Seconds;
    std::thread::sleep(Duration::from_millis(1100)); // a new file name
    app.export_log(false);
    let msg = app.current_message().unwrap().text.clone();
    let text = std::fs::read_to_string(msg.trim_start_matches("Saved ")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1 + 19_000 + 1);
    assert!(lines[1].starts_with("0.000,"));
    assert_eq!(lines[8_001], "8.000,,,,,,");
    assert!(lines[8_002].starts_with("9.000,"));
    // V goes back to the scope; the panel lists the log.
    key(&mut app, KeyCode::Char('V'));
    assert!(!app.show_log);
    let d = app.device_info();
    assert_eq!(d.recent_logs, vec!["log-test.ohlog".to_string()]);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn rec_status_line() {
    let dir = temp_dir("status");
    let mut app = App::new(Settings::default(), None);
    let sh = writer::shared(dir.join("x.ohlog"), 1_000_000_000);
    {
        let mut st = sh.lock().unwrap();
        st.state = LogState::Recording;
        st.bytes = 412_300_000;
        st.started = Instant::now() - Duration::from_secs(2470);
        st.header = Some(header(1_000_000, 20));
    }
    app.rec = Some(sh.clone());
    app.log_free = Some(120_000_000_000);
    let all = screen(&app, 120, 40).join("\n");
    // 24 kB/s: 587.7 MB left take 6:48:07.
    assert!(
        all.contains("● REC  412 MB / 1 GB   0:41:10, about 6:48:07 left   free 120 GB"),
        "{all}"
    );
    // The scope view says it is paused.
    assert!(all.contains("live view paused"), "{all}");
    // Less free space than the limit needs: a warning.
    app.log_free = Some(300_000_000);
    let all = screen(&app, 120, 40).join("\n");
    assert!(
        all.contains("free 300 MB, disk full in about 3:28:20"),
        "{all}"
    );
    // Waiting for the scope.
    sh.lock().unwrap().state = LogState::Waiting;
    let all = screen(&app, 120, 40).join("\n");
    assert!(
        all.contains("● REC  waiting for data from the scope"),
        "{all}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn panel_shows_the_limit_estimate() {
    let mut app = App::new(Settings::default(), None);
    app.log_free = Some(500_000_000_000);
    key(&mut app, KeyCode::Char('s'));
    for _ in 0..30 {
        if app.panel.groups()[app.panel.cat].name == "Logger" {
            break;
        }
        key(&mut app, KeyCode::Char('j'));
    }
    key(&mut app, KeyCode::Char('l'));
    for _ in 0..30 {
        if app.panel.current() == Some(openhantek_tui::panel::Field::LogLimit) {
            break;
        }
        key(&mut app, KeyCode::Char('j'));
    }
    let all = screen(&app, 120, 40).join("\n");
    assert!(
        all.contains("Limit 1 GB ≈ 11 h 34 min at 1 ms step"),
        "{all}"
    );
    assert!(all.contains("1 GB ≈ 11 h 34 min"), "{all}");
    // Changing the limit changes the estimate.
    key(&mut app, KeyCode::Char('l'));
    key(&mut app, KeyCode::Char('j'));
    assert_eq!(app.scope.settings.logger.log_limit, 2_000_000_000);
    let all = screen(&app, 120, 40).join("\n");
    assert!(
        all.contains("Limit 2 GB ≈ 23 h 8 min at 1 ms step"),
        "{all}"
    );
}

#[test]
fn record_key_without_scope_says_so() {
    let mut app = App::new(Settings::default(), None);
    let dir = temp_dir("norec");
    app.log_dir_override = Some(dir.clone());
    key(&mut app, KeyCode::Char('r'));
    assert!(app.rec.is_none());
    assert!(app.current_message().unwrap().error);
    std::fs::remove_dir_all(&dir).unwrap();
}
