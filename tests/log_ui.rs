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
        raw: false,
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

/// Put the panel on Logger › Open log, in its list, at log `i`.
fn open_log_list(app: &mut App, i: usize) {
    use openhantek_tui::panel::{Field, Focus};
    app.panel_open = true;
    let groups = app.panel.groups();
    let cat = groups.iter().position(|g| g.name == "Logger").unwrap();
    let item = groups[cat]
        .items
        .iter()
        .position(|(f, _)| *f == Field::LogOpen)
        .unwrap();
    app.panel.cat = cat;
    app.panel.item = item;
    app.panel.focus = Focus::Values;
    app.panel.value = i;
}

#[test]
fn d_deletes_a_log_and_its_idx_after_y() {
    // Only temp dirs: the log folder is the test's own.
    let (mut app, dir) = app_with_log("delete");
    let open = dir.join("log-test.ohlog");
    let idx = dir.join("log-test.ohlog.idx");
    // The summary file next to it (the view writes it once it is done).
    if !idx.exists() {
        std::fs::write(&idx, b"summary").unwrap();
    }
    let other = dir.join("log-other.ohlog");
    write_log(&other);
    let old = std::time::SystemTime::now() - Duration::from_secs(60);
    std::fs::File::options()
        .write(true)
        .open(&other)
        .unwrap()
        .set_modified(old)
        .unwrap();
    assert_eq!(app.recent_logs(), vec![open.clone(), other.clone()]);
    open_log_list(&mut app, 0);
    // d asks, in the TUI, with the name and size.
    app.handle_key(KeyEvent::from(KeyCode::Char('d')));
    let ask = app.confirm_delete.clone().expect("a question");
    assert_eq!(ask.path, open);
    let t = screen(&app, 120, 40).join("\n");
    assert!(t.contains("Delete log-test.ohlog ("), "{t}");
    assert!(t.contains("? y / n"), "{t}");
    // n keeps it, and so does Esc.
    key(&mut app, KeyCode::Char('n'));
    assert!(app.confirm_delete.is_none() && open.exists() && idx.exists());
    key(&mut app, KeyCode::Char('d'));
    key(&mut app, KeyCode::Esc);
    assert!(open.exists() && app.panel_open);
    // y deletes the log and its .idx, and closes the view on it.
    assert!(app.log_view.is_some());
    key(&mut app, KeyCode::Char('d'));
    key(&mut app, KeyCode::Char('y'));
    assert!(!open.exists() && !idx.exists());
    assert!(app.log_view.is_none() && !app.show_log);
    assert!(other.exists());
    assert_eq!(app.recent_logs(), vec![other.clone()]);
    assert!(app.message.as_ref().unwrap().text.contains("Deleted log-test.ohlog"));
    // The list still points at a log.
    assert_eq!(app.panel.value, 0);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_log_being_recorded_is_never_deleted() {
    let (mut app, dir) = app_with_log("delete-rec");
    let path = dir.join("log-test.ohlog");
    // Pretend this log is being recorded.
    app.rec = Some(writer::shared(path.clone(), 0));
    assert!(app.recording());
    open_log_list(&mut app, 0);
    key(&mut app, KeyCode::Char('d'));
    assert!(app.confirm_delete.is_none());
    assert!(app.message.as_ref().unwrap().text.contains("being recorded"));
    // Not even when asked directly.
    app.delete_log(&path);
    assert!(path.exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn delete_key_is_in_the_key_table() {
    use openhantek_tui::keys::{Act, KeyMap};
    let (m, w) = KeyMap::from_table(&[("delete_log".into(), vec!["x".into()])]);
    assert!(w.is_empty(), "{w:?}");
    assert_eq!(m.first(Act::PanelDeleteLog), "x");
    assert_eq!(KeyMap::default().first(Act::PanelDeleteLog), "d");
    assert!(openhantek_tui::config::generate(&Settings::default()).contains("delete_log = \"d\""));
}

#[test]
fn log_view_zooms_in_past_one_step_per_dot_column() {
    let (mut app, dir) = app_with_log("stretch");
    app.scope.settings.logger.time_axis = TimeAxis::Seconds;
    app.scope.settings.channels[1].enabled = false;
    screen(&app, 120, 40);
    key(&mut app, KeyCode::Char('n'));
    // Scroll into the sine, away from the gap and the newest data.
    key(&mut app, KeyCode::Home);
    for _ in 0..10 {
        key(&mut app, KeyCode::PageDown);
    }
    for _ in 0..12 {
        key(&mut app, KeyCode::Char('+'));
        screen(&app, 120, 40);
    }
    let v = app.log_view.as_ref().unwrap();
    let w = v.width.get();
    let k = v.k() as usize;
    // The biggest 1-2-5 stretch that keeps about 10 steps on screen.
    let max = openhantek_tui::logger::view::LogView::max_stretch(w);
    assert!(k as u64 <= max && k as u64 * 5 / 2 > max, "{k} {max}");
    assert!(k >= 10, "{k}");
    assert!(v.span(w) >= 10 && v.span(w) <= 12, "{}", v.span(w));
    let left = v.left(w);
    let lines = screen(&app, 120, 40);
    // Time labels: seconds with sub-step spacing, starting at the left
    // edge, in even steps.
    let bottom = lines.iter().position(|l| l.starts_with('└')).unwrap();
    let labels: Vec<f64> = lines[bottom - 1]
        .split(|c: char| c == '│' || c.is_whitespace())
        .filter_map(|t| t.strip_suffix('s')?.parse().ok())
        .collect();
    assert!(labels.len() >= 4, "{labels:?}");
    assert!((labels[0] - left as f64 * 1e-3).abs() < 1e-5, "{labels:?} at {left}");
    let d = labels[1] - labels[0];
    assert!(d > 0.0 && d < 4e-3, "{labels:?}");
    for p in labels.windows(2) {
        assert!(((p[1] - p[0]) - d).abs() < 2e-5, "{labels:?}");
    }
    // The status line gives the time per division below one step.
    let status = lines.iter().find(|l| l.contains(" step · ")).unwrap();
    assert!(status.contains("µs/div") || status.contains("1.1 ms/div"), "{status}");
    // Each step is a flat band: within the middle cells of a step all
    // rows look the same, and the band changes between steps.
    let plot: Vec<Vec<char>> = lines[1..lines.len() - 5]
        .iter()
        .map(|l| l.chars().collect())
        .collect();
    let column = |cell: usize| -> String {
        plot.iter()
            .map(|r| r.get(cell).copied().unwrap_or(' '))
            .map(|c| if ('\u{2800}'..='\u{28ff}').contains(&c) { c } else { ' ' })
            .collect()
    };
    // Plot cells start after the label margin; a step is k/2 cells.
    let first_plot_cell = lines[2].chars().position(|c| c == '·' || c == '┼').unwrap();
    let mut flat = 0;
    let mut changes = 0;
    for j in 1..v.span(w) as usize - 1 {
        let c0 = v.col_of(w, (left + j as u64) as f64) as usize / 2 + first_plot_cell;
        let a = column(c0 + 1);
        let b = column(c0 + k / 2 - 2);
        if a == b && a.trim() != "" {
            flat += 1;
        }
        if column(c0 + 1) != column(c0 + 1 + k / 2) {
            changes += 1;
        }
    }
    assert!(flat >= 6, "flat {flat}\n{}", lines.join("\n"));
    // The slow sine of the test log moves the band now and then.
    assert!(changes >= 1, "changes {changes}\n{}", lines.join("\n"));
    // Fine cursor keys land on single steps.
    key(&mut app, KeyCode::Char('v'));
    let a = app.log_view.as_ref().unwrap().cur[0];
    app.handle_key(KeyEvent::new(
        KeyCode::Char('l'),
        ratatui::crossterm::event::KeyModifiers::ALT,
    ));
    assert_eq!(app.log_view.as_ref().unwrap().cur[0], a + 1);
    let t = screen(&app, 120, 40).join("\n");
    assert!(t.contains("ΔT"), "{t}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn log_view_trace_modes() {
    let (mut app, dir) = app_with_log("modes");
    app.scope.settings.display.trace_style = [TraceStyle::Box; 3];
    // Both is the log view default: the area and the line.
    assert_eq!(app.scope.settings.logger.trace_mode, TraceMode::Both);
    let both = plot_rows(&screen(&app, 120, 40));
    assert!(both.matches('░').count() > 100 && both.contains('─'));
    // D in the log view: average, only the line.
    key(&mut app, KeyCode::Char('D'));
    assert_eq!(app.scope.settings.logger.trace_mode, TraceMode::Average);
    let avg = plot_rows(&screen(&app, 120, 40));
    assert_eq!(avg.matches('░').count(), 0);
    assert!(avg.contains('─'));
    // Peak: the area only, in the trace colour.
    key(&mut app, KeyCode::Char('D'));
    assert_eq!(app.scope.settings.logger.trace_mode, TraceMode::Peak);
    let peak = plot_rows(&screen(&app, 120, 40));
    assert!(peak.matches('░').count() > 100);
    // The scope view's mode did not change.
    assert_eq!(app.scope.settings.display.trace_mode, TraceMode::Average);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A raw CH1 log at 30 MS/s: one u8 sample per step, a 1 MHz sine.
fn write_raw_log(path: &Path, n: u64) -> Header {
    let mut h = header(33, 1);
    h.raw = true;
    h.adc_rate = 30e6;
    h.channels[1].enabled = false;
    h.channels[0].zero_count = 128.0;
    h.channels[0].volts_per_count = 0.04;
    let sh = writer::shared(path.to_path_buf(), 0);
    let w = Writer::spawn(sh.clone());
    w.send(Msg::Begin(h.clone()));
    let data: Vec<u8> = (0..n)
        .map(|i| (128.0 + 50.0 * (i as f64 / 30.0 * std::f64::consts::TAU).sin()).round() as u8)
        .collect();
    w.send(Msg::Raw { first: 0, data });
    w.finish();
    h
}

#[test]
fn raw_log_in_the_log_view_and_csv() {
    let dir = temp_dir("raw");
    let path = dir.join("log-raw.ohlog");
    write_raw_log(&path, 3_000_000);
    let mut app = App::new(Settings::default(), None);
    app.export_dir = dir.clone();
    app.log_dir_override = Some(dir.clone());
    assert!(app.open_log(&path));
    for _ in 0..20 {
        app.tick();
    }
    let v = app.log_view.as_ref().unwrap();
    assert!(v.file.header.raw);
    assert_eq!(v.end(), 3_000_000);
    assert!(v.summary_progress() >= 1.0);
    let all = screen(&app, 120, 40).join("\n");
    assert!(all.contains("step 33.3 ns"), "{all}");
    // 0.1 s long: 3 M steps of 33.3 ns.
    assert!(all.contains("0:00:00 long"), "{all}");
    // Zoomed out: the pyramid's min/max is the ±2 V of the sine.
    let cols = v.columns(100).unwrap();
    assert!((cols[50][0].max - 2.0).abs() < 0.05 && (cols[50][0].min + 2.0).abs() < 0.05);
    // Zoomed in to single samples: peak is the line through them, in
    // every mode.
    for _ in 0..30 {
        key(&mut app, KeyCode::Char('+'));
    }
    for mode in TraceMode::ALL {
        app.scope.settings.logger.trace_mode = mode;
        for style in TraceStyle::ALL {
            app.scope.settings.display.trace_style = [style; 3];
            let plot = plot_rows(&screen(&app, 120, 40));
            let ink = plot
                .chars()
                .filter(|c| ('\u{2801}'..='\u{28FF}').contains(c) || "─│╭╮╰╯|/\\_-‾".contains(*c))
                .count();
            assert!(ink > 10, "{mode:?} {style:?}\n{plot}");
        }
    }
    // CSV export converts to volts.
    let f = &app.log_view.as_ref().unwrap().file;
    let mut out = Vec::new();
    openhantek_tui::logger::csv::write_log_csv(&mut out, f, 0..31, CsvTime::Seconds).unwrap();
    let text = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "time_s,ch1");
    assert_eq!(lines[1], "0.0000000000,0.000000");
    // A quarter period (7.5 samples) on: near the top.
    assert!(lines[9].starts_with("0.0000002667,"), "{}", lines[9]);
    let v9: f64 = lines[9].split(',').nth(1).unwrap().parse().unwrap();
    assert!(v9 > 1.9, "{v9}");
    assert_eq!(lines.len(), 32);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn log_view_both_mode_line_looks_like_average_mode() {
    let (mut app, dir) = app_with_log("both-line");
    let pal = openhantek_tui::theme::Palette::for_setting(app.scope.settings.display.colors);
    let buffer = |app: &App| {
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        term.draw(|f| ui::draw(f, app)).unwrap();
        term.backend().buffer().clone()
    };
    let cells = |buf: &ratatui::buffer::Buffer, c| -> Vec<(usize, String)> {
        buf.content()
            .iter()
            .enumerate()
            .filter(|(_, x)| x.fg == c && x.symbol() != " ")
            .map(|(i, x)| (i, x.symbol().to_string()))
            .collect()
    };
    for style in TraceStyle::ALL {
        app.scope.settings.display.trace_style = [style; 3];
        app.scope.settings.logger.trace_mode = TraceMode::Average;
        let avg = buffer(&app);
        app.scope.settings.logger.trace_mode = TraceMode::Both;
        let both = buffer(&app);
        for t in 0..2 {
            let (a, b) = (cells(&avg, pal.trace[t]), cells(&both, pal.trace[t]));
            assert!(a.len() > 20, "{style:?} CH{}: {}", t + 1, a.len());
            assert_eq!(a, b, "{style:?} CH{}", t + 1);
        }
        assert!(!cells(&both, pal.muted(0)).is_empty(), "{style:?}");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
