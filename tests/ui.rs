//! UI tests: draw into a ratatui TestBackend and check the text, and check
//! what the keys do.
#![allow(clippy::field_reassign_with_default)]

mod common;

use common::*;
use openhantek_tui::acquire::Event;
use openhantek_tui::app::App;
use openhantek_tui::device::fake::{FakeScope, Wave};
use openhantek_tui::dsp::trigger::Slope;
use openhantek_tui::scope::{time_index, TrigState};
use openhantek_tui::settings::*;
use openhantek_tui::ui;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;

/// An app with no thread, fed with blocks from a fake scope by hand.
struct Rig {
    app: App,
    driver: openhantek_tui::device::Driver,
}

impl Rig {
    fn new(waves: [Wave; 2]) -> Self {
        let fake = FakeScope::new(waves);
        let driver = driver(&fake);
        let mut app = App::new(Settings::default(), None);
        app.handle_event(Event::Connected {
            name: "TESTSCOPE".into(),
            notes: vec![],
        });
        Self { app, driver }
    }

    /// Push one block made with the current settings.
    fn feed(&mut self) {
        self.app.tick();
        let b = block_for(&self.app, &mut self.driver);
        self.app.handle_event(Event::Block(b));
    }

    fn key(&mut self, code: KeyCode) {
        self.app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn char(&mut self, c: char) {
        self.key(KeyCode::Char(c));
    }

    /// Draw and return the screen as lines of text.
    fn screen(&self, w: u16, h: u16) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| ui::draw(f, &self.app)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect())
            .collect()
    }

    fn text(&self, w: u16, h: u16) -> String {
        self.screen(w, h).join("\n")
    }
}

fn rig() -> Rig {
    let mut r = Rig::new([SINE_1K, Wave::CalOut]);
    r.feed();
    r
}

#[test]
fn draws_title_status_and_measurements() {
    let r = rig();
    let t = r.text(150, 40);
    assert!(t.contains("openhantek-tui"), "{t}");
    assert!(t.contains("TESTSCOPE"));
    assert!(t.contains("RUN"));
    assert!(t.contains("Trig'd"));
    assert!(t.contains("CH1 1 V/div DC x1"));
    assert!(t.contains("1 ms/div"));
    assert!(t.contains("Trig Auto CH1 ↑ 0 V @50%"));
    assert!(t.contains("Measure"));
    assert!(t.contains("Vpp"));
    assert!(t.contains("1 kHz"));
    assert!(t.contains("Cal out 1 kHz"));
    // Ground markers for both channels.
    assert!(t.contains("1▸") && t.contains("2▸"));
}

#[test]
fn trace_is_drawn_in_the_chart() {
    let r = rig();
    let lines = r.screen(120, 30);
    // Braille dots that are not part of the grid: the trace has full cells.
    let dense = lines
        .iter()
        .flat_map(|l| l.chars())
        .filter(|c| ('\u{2800}'..='\u{28FF}').contains(c) && (*c as u32 - 0x2800).count_ones() >= 3)
        .count();
    assert!(dense > 50, "only {dense} dense braille cells");
}

#[test]
fn small_terminal_does_not_panic() {
    let r = rig();
    for (w, h) in [(20, 8), (40, 10), (80, 24), (1, 1), (300, 100)] {
        r.screen(w, h);
    }
}

#[test]
fn help_overlay_lists_keys_and_closes() {
    let mut r = rig();
    r.char('?');
    assert!(r.app.help);
    let t = r.text(120, 40);
    assert!(t.contains("Keys · any key closes"));
    assert!(t.contains("autoset"));
    assert!(t.contains("trigger mode"));
    r.char('x'); // any key closes, and does nothing else
    assert!(!r.app.help);
    assert!(!r.app.scope.settings.display.xy);
}

#[test]
fn every_key_in_the_help_does_something_or_is_documented() {
    // Each listed key changes state (or quits/opens help); none panics.
    let keys: Vec<KeyCode> = "oa123[]{}0cpimtse+-T,.APdzMfFwxvhlHLjkJKgGR"
        .chars()
        .map(KeyCode::Char)
        .chain([
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Tab,
            KeyCode::BackTab,
        ])
        .collect();
    for k in keys {
        let mut r = rig();
        let before = format!("{:?}", r.app.scope.settings);
        let running = r.app.scope.running;
        r.key(k);
        r.feed();
        r.screen(120, 40);
        let after = format!("{:?}", r.app.scope.settings);
        let changed = before != after || running != r.app.scope.running || r.app.message.is_some();
        assert!(changed, "key {k:?} did nothing");
    }
}

#[test]
fn keys_change_scales_and_trigger() {
    let mut r = rig();
    let s0 = r.app.scope.settings.clone();
    r.key(KeyCode::Up);
    assert_eq!(r.app.scope.settings.channels[0].volts_per_div(), 0.5);
    r.key(KeyCode::Tab);
    r.key(KeyCode::Down);
    assert_eq!(r.app.scope.settings.channels[1].volts_per_div(), 2.0);
    r.key(KeyCode::Right);
    assert_eq!(r.app.scope.settings.time_idx, s0.time_idx + 1);
    r.char(']');
    assert_eq!(
        r.app.scope.settings.channels[1].offset,
        s0.channels[1].offset + 0.5
    );
    r.char('t');
    assert_eq!(r.app.scope.settings.trigger.mode, TriggerMode::Normal);
    r.char('e');
    assert_eq!(r.app.scope.settings.trigger.slope, Slope::Falling);
    r.char('s');
    assert_eq!(r.app.scope.settings.trigger.source, 1);
    r.char('+');
    // 0.1 div of CH2 (2 V/div).
    assert!((r.app.scope.settings.trigger.level - 0.2).abs() < 1e-9);
    r.char('.');
    assert!((r.app.scope.settings.trigger.position - 0.55).abs() < 1e-9);
}

#[test]
fn space_stops_and_redraws_with_new_settings() {
    let mut r = rig();
    r.char(' ');
    assert!(!r.app.scope.running);
    assert_eq!(r.app.scope.state, TrigState::Stopped);
    assert!(r.text(150, 40).contains("STOP"));
    // A stopped trace follows display changes, like invert.
    let before = r.app.scope.measure(0).unwrap().max;
    r.char('i');
    let after = r.app.scope.measure(0).unwrap().max;
    assert!((before - after).abs() < 0.1); // a sine: max stays
    r.char(' ');
    assert!(r.app.scope.running);
}

#[test]
fn spectrum_view_shows_the_peak() {
    let mut r = rig();
    r.char('f');
    r.feed();
    let t = r.text(150, 44);
    assert!(t.contains("Spectrum · Hann"), "{t}");
    assert!(t.contains("CH1 peak 1 kHz"));
    r.char('w');
    assert!(r.text(150, 44).contains("Spectrum · Hamming"));
    r.char('F');
    assert!(r.text(150, 44).contains("0..250 kHz"));
}

#[test]
fn xy_view() {
    let mut r = rig();
    r.char('x');
    r.feed();
    let t = r.text(120, 30);
    assert!(t.contains("XY: CH1 → x, CH2 → y"));
    assert!(!ui::xy_points(&r.app).is_empty());
}

#[test]
fn cursor_readout() {
    let mut r = rig();
    r.char('v');
    let t = r.text(150, 40);
    // Cursors at -2 and +2 div at 1 ms/div.
    assert!(t.contains("A -2 ms B 2 ms ΔT 4 ms 1/ΔT 250 Hz"), "{t}");
    r.char('l');
    assert!(r.text(150, 40).contains("ΔT 3.9 ms"));
    r.char('v');
    let t = r.text(150, 40);
    assert!(t.contains("ΔV"));
}

#[test]
fn math_channel_is_drawn_and_measured() {
    let mut r = rig();
    r.char('3');
    r.feed();
    let t = r.text(160, 40);
    assert!(t.contains("M CH1+CH2 2 V/div"));
    assert!(t.contains("MATH"));
    assert!(r.app.scope.measure(2).is_some());
    r.char('m');
    assert!(r.text(160, 40).contains("M CH1-CH2"));
}

#[test]
fn roll_mode_status() {
    let mut r = rig();
    r.app.scope.settings.time_idx = time_index(500e-3) - 1;
    r.key(KeyCode::Right);
    r.feed();
    r.feed();
    let t = r.text(150, 40);
    assert!(t.contains("500 ms/div"));
    assert!(t.contains("│ Roll │"), "{t}");
    assert_eq!(r.app.scope.state, TrigState::Roll);
}

#[test]
fn errors_show_on_the_message_line() {
    let mut r = rig();
    r.app
        .handle_event(Event::Disconnected("the scope was unplugged".into()));
    let t = r.text(150, 40);
    assert!(t.contains("Scope lost: the scope was unplugged"));
    r.app
        .handle_event(Event::Error("permission denied. Add udev rules".into()));
    assert!(r.text(150, 40).contains("permission denied"));
}

#[test]
fn quit_keys() {
    for k in [KeyCode::Char('q'), KeyCode::Esc] {
        let mut r = rig();
        r.key(k);
        assert!(r.app.quit);
    }
    let mut r = rig();
    r.app
        .handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(r.app.quit);
}

#[test]
fn clipping_is_shown() {
    let mut r = Rig::new([Wave::Dc(4.0), Wave::Dc(0.0)]);
    // 4 V at 100 mV/div is far out of range.
    for _ in 0..3 {
        r.key(KeyCode::Up);
    }
    r.feed();
    assert!(r.text(150, 40).contains("CLIP CH1"));
}

/// Cells of the plot grid row that has the center cross: the columns of
/// its `┼` signs.
fn cross_columns(lines: &[String]) -> Vec<usize> {
    let row = lines
        .iter()
        .find(|l| l.contains("┼╌") || l.contains("┼┴"))
        .expect("no center row");
    row.chars()
        .enumerate()
        .filter(|(_, c)| *c == '┼')
        .map(|(i, _)| i)
        .collect()
}

#[test]
fn grid_divisions_are_even_at_every_size() {
    let mut r = rig();
    // Traces off, so they do not cover the center row.
    r.char('1');
    r.char('2');
    for (w, h) in [(80, 24), (100, 30), (140, 40), (200, 60), (123, 37)] {
        let lines = r.screen(w, h);
        let cols = cross_columns(&lines);
        assert_eq!(cols.len(), 11, "{w}x{h}: {cols:?}");
        let gaps: Vec<usize> = cols.windows(2).map(|p| p[1] - p[0]).collect();
        assert!(gaps.iter().all(|g| *g == gaps[0]), "{w}x{h}: {gaps:?}");
    }
}

#[test]
fn trace_colours_follow_the_colour_setting() {
    use openhantek_tui::theme::ColorMode;
    use ratatui::style::Color;
    let mut r = rig();
    for (mode, want) in [
        (ColorMode::TrueColor, Color::Rgb(255, 214, 10)),
        (ColorMode::Ansi256, Color::Indexed(220)),
        (ColorMode::Ansi16, Color::LightYellow),
    ] {
        r.app.scope.settings.display.colors = mode;
        let mut term = Terminal::new(TestBackend::new(140, 40)).unwrap();
        term.draw(|f| ui::draw(f, &r.app)).unwrap();
        let buf = term.backend().buffer();
        let found = buf.content().iter().any(|c| {
            c.fg == want
                && c.symbol()
                    .chars()
                    .all(|ch| ('\u{2801}'..='\u{28FF}').contains(&ch))
        });
        assert!(found, "{mode:?}: no CH1 braille in {want:?}");
    }
}

#[test]
fn settings_panel_opens_and_closes() {
    let mut r = rig();
    r.char('S');
    assert!(r.app.panel_open);
    let t = r.text(140, 40);
    assert!(t.contains("Settings · Channel 1"), "{t}");
    assert!(t.contains("Calibration output"));
    assert!(t.contains("hjkl") || t.contains("j/k move"));
    // The plot is still drawn above the panel.
    assert!(t.contains("1▸"));
    r.key(KeyCode::Esc);
    assert!(!r.app.panel_open);
    assert!(!r.app.quit);
    r.key(KeyCode::F(2));
    assert!(r.app.panel_open);
    r.char('q');
    assert!(!r.app.panel_open);
    assert!(!r.app.quit);
    // Ctrl-C still quits from the panel.
    r.char('S');
    r.app
        .handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(r.app.quit);
}

#[test]
fn settings_panel_changes_coupling_live_and_reverts() {
    use openhantek_tui::panel::Focus;
    let mut r = rig();
    r.char('S');
    // Channel 1 › Coupling is the 4th item.
    r.char('l');
    for _ in 0..3 {
        r.char('j');
    }
    assert!(r.text(140, 40).contains("Settings · Channel 1 › Coupling"));
    r.char('l');
    assert_eq!(r.app.panel.focus, Focus::Values);
    r.char('j');
    // Live: the channel line shows AC at once.
    assert_eq!(r.app.scope.settings.channels[0].coupling, Coupling::Ac);
    assert!(r.text(140, 40).contains("CH1 1 V/div AC x1"));
    // Esc puts DC back.
    r.key(KeyCode::Esc);
    assert_eq!(r.app.scope.settings.channels[0].coupling, Coupling::Dc);
    // Enter keeps the new value.
    r.char('l');
    r.char('j');
    r.key(KeyCode::Enter);
    assert_eq!(r.app.scope.settings.channels[0].coupling, Coupling::Ac);
    r.char('S');
    assert!(!r.app.panel_open);
    assert_eq!(r.app.scope.settings.channels[0].coupling, Coupling::Ac);
}

#[test]
fn settings_panel_search_and_typed_value() {
    let mut r = rig();
    r.char('S');
    r.char('/');
    for c in "level".chars() {
        r.char(c);
    }
    let t = r.text(140, 40);
    assert!(t.contains("Trigger › Level"), "{t}");
    assert!(t.contains("/level"));
    r.key(KeyCode::Enter);
    // Type an exact trigger level.
    r.char('=');
    for _ in 0..8 {
        r.key(KeyCode::Backspace);
    }
    for c in "750m".chars() {
        r.char(c);
    }
    assert!(r.text(140, 40).contains("Level = 750m"));
    r.key(KeyCode::Enter);
    assert!((r.app.scope.settings.trigger.level - 0.75).abs() < 1e-12);
    assert!(r.text(140, 40).contains("Trig Auto CH1 ↑ 750 mV"));
}

#[test]
fn settings_panel_runs_actions_and_shows_device() {
    let mut r = rig();
    r.char('S');
    r.char('/');
    for c in "run".chars() {
        r.char(c);
    }
    r.key(KeyCode::Enter);
    assert!(r.app.scope.running);
    r.key(KeyCode::Enter);
    assert!(!r.app.scope.running);
    // Device info is read-only.
    r.key(KeyCode::Esc);
    r.key(KeyCode::Esc);
    r.char('/');
    for c in "device".chars() {
        r.char(c);
    }
    let t = r.text(140, 40);
    assert!(t.contains("Device › Model"), "{t}");
    assert!(t.contains("TESTSCOPE"));
    assert!(t.contains("connected"));
}

#[test]
fn settings_panel_fits_small_screens() {
    let mut r = rig();
    r.char('S');
    for (w, h) in [(80, 24), (60, 20), (40, 12), (20, 8), (300, 100)] {
        let t = r.text(w, h);
        if w >= 60 {
            assert!(t.contains("Settings"), "{w}x{h}");
        }
    }
}
