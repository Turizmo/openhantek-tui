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

    /// Draw and return the screen buffer, with colours.
    fn buffer(&self, w: u16, h: u16) -> ratatui::buffer::Buffer {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| ui::draw(f, &self.app)).unwrap();
        term.backend().buffer().clone()
    }

    /// Draw and return the screen as lines of text.
    fn screen(&self, w: u16, h: u16) -> Vec<String> {
        let buf = self.buffer(w, h);
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
fn grid_settings_in_the_panel_change_the_grid() {
    use openhantek_tui::theme::{ColorMode, Palette};
    let mut r = rig();
    r.app.scope.settings.display.colors = ColorMode::TrueColor;
    // Traces off, so only the grid is in the plot.
    r.char('1');
    r.char('2');
    let count = |lines: &[String], c: char| -> usize {
        lines.iter().map(|l| l.chars().filter(|x| *x == c).count()).sum()
    };
    let before = r.screen(120, 40);
    assert!(count(&before, '·') > 50);
    // Display › Grid style: solid; Grid lines: full.
    r.char('S');
    r.char('/');
    for c in "grid style".chars() {
        r.char(c);
    }
    r.key(KeyCode::Enter);
    r.char('l');
    r.char('j');
    r.char('j');
    assert_eq!(r.app.scope.settings.display.grid_style, GridStyle::Solid);
    // Back to the categories and clear the search.
    r.key(KeyCode::Esc);
    r.key(KeyCode::Esc);
    r.key(KeyCode::Esc);
    assert!(r.app.panel_open);
    r.char('/');
    for c in "grid lines".chars() {
        r.char(c);
    }
    r.key(KeyCode::Enter);
    r.char('l');
    r.char('j');
    assert_eq!(r.app.scope.settings.display.grid_extent, GridExtent::Full);
    r.char('S');
    let full = r.screen(120, 40);
    // Whole lines: many more line cells than the center cross alone.
    assert!(count(&full, '─') > 400, "{}", full.join("\n"));
    assert!(count(&full, '│') > 200);
    assert!(full.iter().any(|l| l.contains("┌──") && l.contains("──┐")));
    // Every style and extent draws. Grid characters on top of what the
    // borders and labels have with no grid.
    let grid_chars = |r: &Rig| -> usize {
        let lines = r.screen(120, 40);
        ['·', '─', '│', '╌', '╎', '┼', '+', '┌', '┴']
            .iter()
            .map(|c| count(&lines, *c))
            .sum()
    };
    r.app.scope.settings.display.grid_style = GridStyle::None;
    let none = grid_chars(&r);
    for style in GridStyle::ALL {
        for extent in GridExtent::ALL {
            r.app.scope.settings.display.grid_style = style;
            r.app.scope.settings.display.grid_extent = extent;
            let grid = grid_chars(&r) - none;
            match (style, extent) {
                (GridStyle::None, _) => assert_eq!(grid, 0),
                (_, GridExtent::Full) => assert!(grid > 500, "{style:?} full: {grid}"),
                _ => assert!(grid > 50, "{style:?} points: {grid}"),
            }
            r.screen(80, 24);
        }
    }
    // Brightness changes the grid colour.
    r.app.scope.settings.display.grid_style = GridStyle::Solid;
    let pal = Palette::new(ColorMode::TrueColor);
    let colour_of = |r: &Rig, c: char| {
        let buf = r.buffer(120, 40);
        buf.content()
            .iter()
            .find(|cell| cell.symbol() == c.to_string())
            .map(|cell| cell.fg)
            .unwrap()
    };
    let mut seen = Vec::new();
    for b in [10, 30, 100] {
        r.app.scope.settings.display.grid_brightness = b;
        let (grid, _, _) = pal.grid_colors(b);
        assert_eq!(colour_of(&r, '┬'), grid, "{b}");
        seen.push(grid);
    }
    assert_eq!(seen[1], pal.grid);
    assert!(seen[0] != seen[1] && seen[1] != seen[2]);
}

#[test]
fn margins_are_small_and_nothing_is_cut() {
    // Plot widths before the margins got smaller.
    for (w, h, old) in [(120u16, 40u16, 71usize), (80, 24, 41)] {
        let mut r = rig();
        r.char('1');
        r.char('2');
        let cols = cross_columns(&r.screen(w, h));
        let width = cols[10] - cols[0] + 1;
        assert!(width > old, "{w}x{h}: plot {width} wide, was {old}");
        r.char('1');
        r.char('2');
        let lines = r.screen(w, h);
        let text = lines.join("\n");
        // Labels as wide as the longest one, the ground marker right
        // after it, and the plot right after the marker.
        let ground = lines.iter().find(|l| l.contains("1▸")).unwrap();
        assert!(ground.starts_with("│ 0V1▸"), "{w}x{h}:\n{text}");
        assert!(text.contains("│-6V  "), "{w}x{h}:\n{text}");
        assert!(lines.iter().any(|l| l.contains("│-4V2▸")), "{w}x{h}");
        // The trigger mark is in the one column right of the plot.
        let trig = lines.iter().find(|l| l.contains('◂')).unwrap();
        // (At most one spare cell after it, from centering the plot.)
        assert!(trig.contains("◂│") || trig.contains("◂ │"), "{w}x{h}: {trig}");
        // The time labels are whole and do not touch.
        let time = lines.iter().find(|l| l.contains("-5ms")).unwrap();
        assert!(time.contains("5ms│") || time.contains("5ms "), "{time}");
        assert!(!time.contains("ms-") && !time.contains("s0s"), "{time}");
    }
    // Long labels make the margin wider, and still fit.
    let mut r = rig();
    r.app.scope.settings.channels[0].volt_idx = 0; // 20 mV/div
    r.app.scope.settings.channels[0].offset = 0.5;
    let text = r.text(120, 40);
    assert!(text.contains("-90mV"), "{text}");
    // Fine voltage cursors put their names in a second right column.
    r.app.scope.settings.display.cursor_style = CursorStyle::Fine;
    r.app.scope.settings.cursors.mode = CursorMode::Volt;
    let lines = r.screen(120, 40);
    for name in [" A ", " B "] {
        let row = lines.iter().find(|l| l.starts_with('│') && l.contains(name));
        assert!(row.is_some(), "{name}\n{}", lines.join("\n"));
    }
}

#[test]
fn persistence_fades_old_frames_by_age() {
    use openhantek_tui::theme::{ColorMode, Palette};
    use ratatui::style::Color;
    for style in TraceStyle::ALL {
        let mut r = rig();
        let s = &mut r.app.scope.settings;
        s.display.colors = ColorMode::TrueColor;
        s.display.persistence = 8;
        s.display.trace_style = [style; 3];
        s.channels[1].enabled = false;
        s.channels[0].offset = 3.0;
        // Old frames, oldest first, each lower on the screen than the
        // newer one.
        let live = r.app.scope.frame.clone().unwrap();
        r.app.scope.history.clear();
        for k in 0..8 {
            let mut f = live.clone();
            let shift = -0.8 * (8 - k) as f64;
            if let Some(tr) = f.traces[0].as_mut() {
                tr.iter_mut().for_each(|v| *v = *v * 0.2 + shift);
            }
            r.app.scope.history.push_back(f);
        }
        let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        term.draw(|f| ui::draw(f, &r.app)).unwrap();
        let buf = term.backend().buffer().clone();
        let pal = Palette::new(ColorMode::TrueColor);
        let colours: Vec<Color> = (0..8).map(|age| pal.persist(0, age, 8).0).collect();
        for w in colours.windows(2) {
            assert_ne!(w[0], w[1]);
        }
        // Mean row of each age's colour: older frames are lower.
        let mut last = 0.0;
        for (age, c) in colours.iter().enumerate() {
            let rows: Vec<u16> = (0..40)
                .flat_map(|y| (0..120).map(move |x| (x, y)))
                .filter(|&p| buf[p].fg == *c && buf[p].symbol() != " ")
                .map(|(_, y)| y)
                .collect();
            assert!(
                rows.len() > 5,
                "{style:?}: age {age} {c:?} has {} cells",
                rows.len()
            );
            let mean = rows.iter().map(|&y| y as f64).sum::<f64>() / rows.len() as f64;
            assert!(
                mean > last,
                "{style:?}: age {age} at row {mean}, newer at {last}"
            );
            last = mean;
        }
        // The live trace keeps the full colour.
        assert!(buf.content().iter().any(|c| c.fg == pal.trace[0]));
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
fn settings_panel_changes_coupling_at_once_and_keeps_it() {
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
    // Esc goes back a column and keeps AC; no Enter needed.
    r.key(KeyCode::Esc);
    assert_eq!(r.app.panel.focus, Focus::Items);
    assert_eq!(r.app.scope.settings.channels[0].coupling, Coupling::Ac);
    assert!(r.text(140, 40).contains("CH1 1 V/div AC x1"));
    // The key hints no longer talk about keeping a value.
    assert!(!r.text(140, 40).contains("keep"));
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
    // The value column shows the number in a box, not a list.
    let t = r.text(140, 40);
    assert!(t.contains("│ 0 V │"), "{t}");
    assert!(t.contains("j k  ± 100 mV"), "{t}");
    // Into the value column: j/k step it at once.
    r.char('l');
    r.char('k');
    assert!((r.app.scope.settings.trigger.level - 0.1).abs() < 1e-12);
    assert!(r.text(140, 40).contains("│ 100 mV │"));
    assert!(r.text(140, 40).contains("Trig Auto CH1 ↑ 100 mV"));
    r.char('j');
    r.key(KeyCode::Down);
    assert!((r.app.scope.settings.trigger.level + 0.1).abs() < 1e-12);
    // Typing digits edits the value in the box.
    for c in "750m".chars() {
        r.char(c);
    }
    let t = r.text(140, 40);
    assert!(t.contains("│ 750m▏ │"), "{t}");
    assert!(t.contains("⏎ apply · esc cancel"), "{t}");
    r.key(KeyCode::Enter);
    assert!((r.app.scope.settings.trigger.level - 0.75).abs() < 1e-12);
    let t = r.text(140, 40);
    assert!(t.contains("Trig Auto CH1 ↑ 750 mV"));
    assert!(t.contains("│ 750 mV │"), "{t}");
    // A bad value shows an error in the column; Esc drops it.
    for c in "5q".chars() {
        r.char(c);
    }
    r.key(KeyCode::Enter);
    assert!(r.text(140, 40).contains("unknown unit: q"));
    r.key(KeyCode::Esc);
    assert!(!r.text(140, 40).contains("unknown unit"));
    assert!((r.app.scope.settings.trigger.level - 0.75).abs() < 1e-12);
    // Small screens still show the box.
    let t = r.text(80, 24);
    assert!(t.contains("│ 750 mV │"), "{t}");
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

/// Rows of the open settings panel on the screen, borders included.
fn panel_rows(lines: &[String]) -> usize {
    let top = lines
        .iter()
        .position(|l| l.starts_with("┌ Settings"))
        .expect("no panel");
    let bottom = lines[top..]
        .iter()
        .position(|l| l.starts_with('└'))
        .expect("no panel bottom");
    bottom + 1
}

#[test]
fn settings_panel_height_is_fixed_and_fits_its_content() {
    use openhantek_tui::panel::Focus;
    let mut r = rig();
    r.char('S');
    let first = panel_rows(&r.screen(120, 40));
    // Smaller than the old 40 % of the screen (16 rows at 120x40).
    assert!(first < 16, "{first}");
    let cats = r.app.panel.groups().len();
    let mut seen = 0;
    for c in 0..cats {
        assert_eq!(r.app.panel.cat, c);
        assert_eq!(panel_rows(&r.screen(120, 40)), first, "category {c}");
        r.char('l');
        assert_eq!(r.app.panel.focus, Focus::Items);
        let items = r.app.panel.groups()[c].items.len();
        for i in 0..items {
            assert_eq!(r.app.panel.item, i);
            let lines = r.screen(120, 40);
            assert_eq!(panel_rows(&lines), first, "category {c} item {i}");
            seen += 1;
            r.char('j');
        }
        r.char('h');
        r.char('j');
    }
    assert!(seen > 40, "{seen}");
    // A long option list (time/div) scrolls inside the same panel, and
    // the help line and key hints stay.
    let mut r = rig();
    r.char('S');
    while r.app.panel.groups()[r.app.panel.cat].name != "Timebase" {
        r.char('j');
    }
    r.char('l');
    r.char('l');
    assert_eq!(r.app.panel.focus, Focus::Values);
    let opts = r.app.panel.current().unwrap();
    assert!(
        opts.options(&r.app.scope.settings, &r.app.device_info())
            .len()
            > 20
    );
    let lines = r.screen(120, 40);
    assert_eq!(panel_rows(&lines), first);
    let t = lines.join("\n");
    assert!(t.contains("●"), "{t}");
    assert!(t.contains("S close"), "{t}");
    // At 80x24 the panel shrinks so a small plot stays above it, and it
    // too keeps its size.
    r.key(KeyCode::Esc);
    let small = panel_rows(&r.screen(80, 24));
    assert!(small <= first);
    r.char('j');
    assert_eq!(panel_rows(&r.screen(80, 24)), small);
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

/// Braille dots on the screen, counted dot by dot.
fn braille_dots(lines: &[String]) -> u32 {
    lines
        .iter()
        .flat_map(|l| l.chars())
        .filter(|c| ('\u{2801}'..='\u{28FF}').contains(c))
        .map(|c| (c as u32 - 0x2800).count_ones())
        .sum()
}

/// Characters the line and box styles draw with. The box characters are
/// also in the frame and grid: [`trace_ink`] takes those off.
const LINE_CHARS: &str = "|/\\_‾'.─│╭╮╰╯╵╷╴╶├┤┬┴┼•";

/// Trace ink on the screen: braille dots, and four per line style
/// character (a cell of braille has up to eight).
fn ink(lines: &[String]) -> u32 {
    let glyphs = lines
        .iter()
        .flat_map(|l| l.chars())
        .filter(|c| LINE_CHARS.contains(*c))
        .count() as u32;
    braille_dots(lines) + 4 * glyphs
}

/// Ink of the traces at 120 x 30: the ink less that of a screen with no
/// traces yet.
fn trace_ink(r: &Rig) -> u32 {
    let blank = ink(&Rig::new([SINE_1K, Wave::CalOut]).screen(120, 30));
    ink(&r.screen(120, 30)).saturating_sub(blank)
}

fn styled_rig(style: TraceStyle) -> Rig {
    let mut r = Rig::new([SINE_1K, Wave::CalOut]);
    r.app.scope.settings.display.trace_style = [style; 3];
    r.feed();
    r
}

#[test]
fn classic_styles_are_the_default() {
    let s = Settings::default();
    assert_eq!(s.display.trace_style, [TraceStyle::BrailleThin; 3]);
    assert_eq!(s.display.cursor_style, CursorStyle::Classic);
}

#[test]
fn every_trace_style_draws_with_dots_persistence_and_interpolation() {
    for style in TraceStyle::ALL {
        let mut r = styled_rig(style);
        let base = trace_ink(&r);
        assert!(base > 300, "{style:?}: only {base} dots");
        // Dots only.
        r.char('d');
        r.feed();
        assert!(r.app.scope.settings.display.dots);
        let dots = trace_ink(&r);
        assert!(dots > 100, "{style:?} dots: only {dots}");
        r.char('d');
        // Persistence: old traces too.
        r.char('P');
        for _ in 0..4 {
            r.feed();
        }
        assert!(!r.app.scope.history.is_empty(), "{style:?}");
        assert!(trace_ink(&r) >= base * 9 / 10, "{style:?} persistence");
        r.char('P');
        // Few samples: sin(x)/x and linear both draw a line.
        for _ in 0..8 {
            r.key(KeyCode::Left);
        }
        r.feed();
        let sinc = trace_ink(&r);
        r.char('z');
        r.feed();
        assert_eq!(
            r.app.scope.settings.display.interpolation,
            Interpolation::Linear
        );
        let linear = trace_ink(&r);
        assert!(sinc > 50 && linear > 50, "{style:?}: {sinc} {linear}");
        // The spectrum uses the style too, and small screens do not panic.
        r.char('f');
        r.feed();
        assert!(r.text(140, 50).contains("Spectrum"));
        for (w, h) in [(20, 8), (40, 10), (80, 24), (300, 100)] {
            r.screen(w, h);
        }
    }
}

#[test]
fn trace_styles_look_different() {
    let thin = styled_rig(TraceStyle::BrailleThin).screen(120, 30);
    let bold = styled_rig(TraceStyle::BrailleBold).screen(120, 30);
    // Bold is two dots wide, so it has more dots than the thin line.
    assert_ne!(thin, bold);
    assert!(
        braille_dots(&bold) > braille_dots(&thin),
        "bold {} thin {}",
        braille_dots(&bold),
        braille_dots(&thin)
    );
}

#[test]
fn line_style_draws_without_braille() {
    let r = styled_rig(TraceStyle::Line);
    let lines = r.screen(120, 36);
    let text = lines.join("\n");
    assert_eq!(braille_dots(&lines), 0, "{text}");
    // The square wave: `|` edges, flat tops and bottoms (`-` on the
    // division lines). The sine: slopes, `_` in the dips.
    let count = |c: char| text.chars().filter(|&x| x == c).count();
    assert!(count('|') >= 10, "{text}");
    assert!(count('_') + count('‾') + count('-') >= 30, "{text}");
    assert!(count('/') >= 4 && count('\\') >= 4, "{text}");
    // Mixed with a braille style, each trace keeps its own look.
    let mut r = styled_rig(TraceStyle::Line);
    r.app.scope.settings.display.trace_style[1] = TraceStyle::BrailleThin;
    let mixed = r.screen(120, 36);
    assert!(braille_dots(&mixed) > 100);
    assert!(mixed.join("").contains('/'));
}

#[test]
fn box_style_draws_connected_box_lines() {
    let r = styled_rig(TraceStyle::Box);
    let lines = r.screen(120, 36);
    let text = lines.join("\n");
    assert_eq!(braille_dots(&lines), 0, "{text}");
    let count = |c: char| text.chars().filter(|&x| x == c).count();
    // No ascii line characters (`/` is in the units).
    for c in ['\\', '‾'] {
        assert_eq!(count(c), 0, "{c}\n{text}");
    }
    // The square wave and the sine: corners where they turn, ten periods
    // each, and flat tops and bottoms.
    for c in ['╭', '╮', '╰', '╯'] {
        assert!(count(c) >= 20, "{c}: {}\n{text}", count(c));
    }
    assert!(text.contains("╭───╮") && text.contains("╰───╯"), "{text}");
    assert!(text.contains("╰─╯"), "{text}");
    // Mixed with a braille style, each trace keeps its own look.
    let mut r = styled_rig(TraceStyle::Box);
    r.app.scope.settings.display.trace_style[1] = TraceStyle::BrailleThin;
    let mixed = r.screen(120, 36);
    assert!(braille_dots(&mixed) > 100);
    assert!(mixed.join("").contains('╭'));
    // Dots mode: a dot per sample.
    let mut r = styled_rig(TraceStyle::Box);
    r.char('d');
    r.feed();
    let dots = r.screen(120, 36).join("\n");
    assert!(dots.matches('•').count() > 100, "{dots}");
    assert!(!dots.contains('╭'), "{dots}");
}

/// Line of the screen where the cursor name `A` is, above the status lines.
fn cursor_name_line(lines: &[String]) -> usize {
    lines[..lines.len() - 3]
        .iter()
        .position(|l| l.contains(" A "))
        .expect("no cursor name")
}

#[test]
fn cursor_styles() {
    let mut r = rig();
    r.char('v');
    let classic = r.screen(140, 40);
    let marker = classic.iter().position(|l| l.contains('▾')).unwrap();
    // Classic: the names are inside the plot, below the trigger mark row.
    assert!(
        cursor_name_line(&classic) > marker,
        "{}",
        classic.join("\n")
    );
    r.app.scope.settings.display.cursor_style = CursorStyle::Fine;
    let fine = r.screen(140, 40);
    // Fine: in the margin row above the plot, with the trigger mark.
    assert_eq!(cursor_name_line(&fine), marker, "{}", fine.join("\n"));
    assert_ne!(classic, fine);
    // Voltage cursors draw in both styles.
    r.char('v');
    let v_fine = r.text(140, 40);
    r.app.scope.settings.display.cursor_style = CursorStyle::Classic;
    let v_classic = r.text(140, 40);
    assert!(v_fine.contains("ΔV") && v_classic.contains("ΔV"));
    assert_ne!(v_fine, v_classic);
}

#[test]
fn settings_panel_changes_trace_style_live() {
    let mut r = rig();
    let before = r.screen(140, 40);
    r.char('S');
    // Channel 1 › Style is the last item.
    r.char('l');
    r.char('G');
    assert!(r.text(140, 40).contains("Settings · Channel 1 › Style"));
    r.char('l');
    r.char('j');
    assert_eq!(
        r.app.scope.settings.display.trace_style[0],
        TraceStyle::BrailleBold
    );
    assert!(r.text(140, 40).contains("braille bold"));
    r.key(KeyCode::Enter);
    r.char('S');
    assert!(!r.app.panel_open);
    assert_eq!(
        r.app.scope.settings.display.trace_style,
        [TraceStyle::BrailleBold, TraceStyle::BrailleThin, TraceStyle::BrailleThin]
    );
    assert_ne!(r.screen(140, 40), before);
}
