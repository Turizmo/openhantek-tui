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
use ratatui::style::Modifier;
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
        self.app.handle_event(Event::Block(Box::new(b)));
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
    let keys: Vec<KeyCode> = "oa123[]{}0cpiMtSe+-T,.APdzmfFwxgGR"
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
    r.char('S');
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

/// Turn cursors on: 0 time A, 1 time B, 2 voltage A, 3 voltage B.
fn cursors_on(r: &mut Rig, which: &[usize]) {
    for &i in which {
        r.app.scope.settings.cursors.get_mut(i).enabled = true;
    }
}

/// The value right of `label` in the measurement panel, like `ΔT 4 ms`.
fn meas_row(text: &str, label: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let i = l.find(&format!("│{label} "))?;
        let rest = &l[i + "│".len()..];
        let end = rest.find('│').unwrap_or(rest.len());
        Some(rest[..end].split_whitespace().collect::<Vec<_>>().join(" "))
    })
}

#[test]
fn cursor_readout() {
    let mut r = rig();
    // All cursors are off by default: no Cursors section.
    let t = r.text(150, 40);
    assert!(!t.contains("Cursors"), "{t}");
    cursors_on(&mut r, &[0, 1]);
    let t = r.text(150, 40);
    // Cursors at -2 and +2 div at 1 ms/div.
    assert_eq!(meas_row(&t, "A t").as_deref(), Some("A t -2 ms"), "{t}");
    assert_eq!(meas_row(&t, "ΔT").as_deref(), Some("ΔT 4 ms"), "{t}");
    assert_eq!(meas_row(&t, "1/ΔT").as_deref(), Some("1/ΔT 250 Hz"), "{t}");
    // No cursor text in the status lines any more.
    let lines: Vec<&str> = t.lines().collect();
    assert!(lines[lines.len() - 3..].iter().all(|l| !l.contains("ΔT")));
    // The value of each channel at each time cursor.
    assert!(meas_row(&t, "V@A").is_some(), "{t}");
    r.char('l');
    assert_eq!(
        meas_row(&r.text(150, 40), "ΔT").as_deref(),
        Some("ΔT 3.9 ms")
    );
    // Voltage cursors: ΔV only with both on.
    cursors_on(&mut r, &[3]);
    let t = r.text(150, 40);
    assert!(
        meas_row(&t, "B v").is_some() && meas_row(&t, "ΔV").is_none(),
        "{t}"
    );
    cursors_on(&mut r, &[2]);
    // CH1 at 1 V/div, 0 V line at +2 div: -1 and +1 div are -3 V and -1 V.
    assert_eq!(meas_row(&r.text(150, 40), "ΔV").as_deref(), Some("ΔV 2 V"));
    // One time cursor: no ΔT.
    r.app.scope.settings.cursors.time_b.enabled = false;
    let t = r.text(150, 40);
    assert!(meas_row(&t, "ΔT").is_none() && meas_row(&t, "A t").is_some());
}

#[test]
fn v_hides_and_shows_the_cursors_and_moves_only_enabled_ones() {
    let mut r = rig();
    cursors_on(&mut r, &[0, 3]);
    let shown = r.text(150, 40);
    assert!(shown.contains("Cursors"));
    r.char('v');
    assert!(r.app.scope.settings.cursors.hidden);
    let hidden = r.text(150, 40);
    assert!(!hidden.contains("Cursors"), "{hidden}");
    // Hidden cursors do not move.
    r.char('l');
    assert_eq!(r.app.scope.settings.cursors.time_a.position, -2.0);
    r.char('v');
    assert_eq!(r.text(150, 40), shown);
    // Each cursor keeps its own enabled state.
    let c = &r.app.scope.settings.cursors;
    assert!(c.time_a.enabled && !c.time_b.enabled && !c.volt_a.enabled && c.volt_b.enabled);
    r.char('l');
    r.char('H');
    r.char('j');
    r.char('K');
    let c = &r.app.scope.settings.cursors;
    assert!((c.time_a.position + 1.9).abs() < 1e-9);
    assert_eq!(c.time_b.position, 2.0); // off: does not move
    assert_eq!(c.volt_a.position, -1.0); // off
    assert!((c.volt_b.position - 1.1).abs() < 1e-9);
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
    r.char('M');
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

/// Colour of the trigger mark `◂` right of the plot.
fn trigger_mark_colour(r: &Rig) -> ratatui::style::Color {
    let buf = r.buffer(120, 40);
    for y in 0..40 {
        for x in 0..120 {
            if buf[(x, y)].symbol() == "◂" {
                return buf[(x, y)].fg;
            }
        }
    }
    panic!("no trigger mark");
}

#[test]
fn trigger_level_out_of_range_is_red_and_explained() {
    let mut r = rig();
    r.app.scope.settings.trigger.level = 1.0;
    r.feed();
    let normal = trigger_mark_colour(&r);
    assert_ne!(normal, ui::TRIGGER_OUT_OF_RANGE);
    assert!(!r.text(120, 40).contains("out of range"));
    // 20 mV/div: the input covers only about ±0.5 V.
    r.app.scope.settings.channels[0].volt_idx = 0;
    r.feed();
    r.feed();
    assert_eq!(trigger_mark_colour(&r), ui::TRIGGER_OUT_OF_RANGE);
    let text = r.text(120, 40);
    let line = text
        .lines()
        .find(|l| l.contains("Trig "))
        .expect("trigger status");
    // The fake EEPROM calibration gives a range a little off the nominal.
    assert!(
        line.contains("Trig Auto CH1 ↑ 1 V (out of range, using 4"),
        "{line}"
    );
    assert!(line.contains(" mV) @50%"), "{line}");
    // The clipped sine still triggers on the level inside the range.
    assert_eq!(r.app.scope.state, TrigState::Triggered);
    assert_eq!(r.app.scope.settings.trigger.level, 1.0);
    // Back to 1 V/div: the real level again.
    r.app.scope.settings.channels[0].volt_idx = 5;
    r.feed();
    assert_eq!(trigger_mark_colour(&r), normal);
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
        lines
            .iter()
            .map(|l| l.chars().filter(|x| *x == c).count())
            .sum()
    };
    let before = r.screen(120, 40);
    assert!(count(&before, '·') > 50);
    // Display › Grid style: solid; Grid lines: full.
    r.char('s');
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
    r.char('s');
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
        assert!(
            trig.contains("◂│") || trig.contains("◂ │"),
            "{w}x{h}: {trig}"
        );
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
    for i in [2, 3] {
        let c = r.app.scope.settings.cursors.get_mut(i);
        c.style = CursorStyle::Fine;
        c.enabled = true;
    }
    let lines = r.screen(120, 40);
    for name in [" A ", " B "] {
        let row = lines
            .iter()
            .find(|l| l.starts_with('│') && l.contains(name));
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
    r.char('s');
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
    r.char('s');
    r.app
        .handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(r.app.quit);
}

#[test]
fn settings_panel_changes_coupling_at_once_and_keeps_it() {
    use openhantek_tui::panel::Focus;
    let mut r = rig();
    r.char('s');
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
    r.char('s');
    assert!(!r.app.panel_open);
    assert_eq!(r.app.scope.settings.channels[0].coupling, Coupling::Ac);
}

#[test]
fn settings_panel_search_and_typed_value() {
    let mut r = rig();
    r.char('s');
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
    r.char('s');
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
fn panel_help_line_stands_apart_from_rows_and_title() {
    use openhantek_tui::theme::{ColorMode, Palette};
    for mode in [ColorMode::TrueColor, ColorMode::Ansi256, ColorMode::Ansi16] {
        let mut r = rig();
        r.app.scope.settings.display.colors = mode;
        r.char('s');
        r.char('l');
        let buf = r.buffer(120, 40);
        let lines = r.screen(120, 40);
        // Cell (column, row) where `needle` starts.
        let find = |needle: &str| -> (u16, u16) {
            let y = lines.iter().position(|l| l.contains(needle)).unwrap();
            let col = lines[y]
                .char_indices()
                .position(|(i, _)| lines[y][i..].starts_with(needle))
                .unwrap();
            (col as u16, y as u16)
        };
        let (tx, ty) = find("Settings · Channel 1");
        let (hx, hy) = find("Show this channel");
        let (rx, ry) = find("Volts/div");
        let (sx, sy) = find("Probe");
        let title = buf[(tx, ty)].style();
        let help = buf[(hx, hy)].style();
        let row = buf[(rx, ry)].style();
        let selected = buf[(sx, sy)].style();
        let want = ui::panel_help(&Palette::new(mode));
        assert_eq!((help.fg, help.add_modifier), (want.fg, want.add_modifier));
        assert!(help.add_modifier.contains(Modifier::ITALIC), "{mode:?}");
        // Not like a setting row, the selected row or the title.
        for other in [title, row, selected] {
            assert_ne!(other.fg, help.fg, "{mode:?}");
            assert_ne!(other.add_modifier, help.add_modifier, "{mode:?}");
        }
        // A thin rule on the help row sets it apart.
        let line = &lines[hy as usize];
        assert!(line.starts_with("├── Show this channel"), "{line}");
        assert!(line.ends_with("─┤"), "{line}");
    }
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
    r.char('s');
    let first = panel_rows(&r.screen(120, 40));
    // Not more than the old 40 % of the screen (16 rows at 120x40): 13
    // categories (with the Logger) and 3 rows around them.
    assert!(first <= 16, "{first}");
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
    r.char('s');
    while r.app.panel.groups()[r.app.panel.cat].name != "Acquisition" {
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
    assert!(t.contains("q close"), "{t}");
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
    r.char('s');
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
    assert!((0..4).all(|i| s.cursors.get(i).style == CursorStyle::Classic));
    assert!((0..4).all(|i| s.cursors.get(i).color == CursorColor::Grey));
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

fn set_cursor_style(r: &mut Rig, st: CursorStyle) {
    for i in 0..4 {
        r.app.scope.settings.cursors.get_mut(i).style = st;
    }
}

#[test]
fn cursor_styles() {
    let mut r = rig();
    cursors_on(&mut r, &[0, 1]);
    let classic = r.screen(140, 40);
    let marker = classic.iter().position(|l| l.contains('▾')).unwrap();
    // Classic: the names are inside the plot, below the trigger mark row.
    assert!(
        cursor_name_line(&classic) > marker,
        "{}",
        classic.join("\n")
    );
    set_cursor_style(&mut r, CursorStyle::Fine);
    let fine = r.screen(140, 40);
    // Fine: in the margin row above the plot, with the trigger mark.
    assert_eq!(cursor_name_line(&fine), marker, "{}", fine.join("\n"));
    assert_ne!(classic, fine);
    // Solid and dotted look different from both, names inside.
    set_cursor_style(&mut r, CursorStyle::Solid);
    let solid = r.screen(140, 40);
    set_cursor_style(&mut r, CursorStyle::Dotted);
    let dotted = r.screen(140, 40);
    for other in [&solid, &dotted] {
        assert!(cursor_name_line(other) > marker);
        assert_ne!(other, &classic);
        assert_ne!(other, &fine);
    }
    assert_ne!(solid, dotted);
    // Voltage cursors draw in every style.
    r.app.scope.settings.cursors.time_a.enabled = false;
    r.app.scope.settings.cursors.time_b.enabled = false;
    cursors_on(&mut r, &[2, 3]);
    let mut seen = Vec::new();
    for st in CursorStyle::ALL {
        set_cursor_style(&mut r, st);
        let t = r.text(140, 40);
        assert!(t.contains("ΔV"));
        assert!(!seen.contains(&t), "{st:?}");
        seen.push(t);
    }
}

#[test]
fn each_cursor_has_its_own_style_and_colour() {
    use ratatui::style::Color;
    let mut r = rig();
    cursors_on(&mut r, &[0, 1]);
    r.app.scope.settings.cursors.time_b.color = CursorColor::Green;
    r.app.scope.settings.cursors.time_b.style = CursorStyle::Solid;
    let buf = r.buffer(140, 40);
    // The name B is green, A has the white of the classic grey cursor.
    let mut colours = std::collections::HashMap::new();
    for y in 1..30 {
        for x in 0..100 {
            let c = &buf[(x, y)];
            if c.symbol() == "A" || c.symbol() == "B" {
                colours.entry(c.symbol().to_string()).or_insert(c.fg);
            }
        }
    }
    assert_eq!(colours.get("B"), Some(&Color::LightGreen), "{colours:?}");
    assert_eq!(colours.get("A"), Some(&Color::White), "{colours:?}");
}

#[test]
fn settings_panel_changes_trace_style_live() {
    let mut r = rig();
    let before = r.screen(140, 40);
    r.char('s');
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
    r.char('s');
    assert!(!r.app.panel_open);
    assert_eq!(
        r.app.scope.settings.display.trace_style,
        [
            TraceStyle::BrailleBold,
            TraceStyle::BrailleThin,
            TraceStyle::BrailleThin
        ]
    );
    assert_ne!(r.screen(140, 40), before);
}

/// The measurement panel layout on a `w` x `h` screen (the main area is
/// the screen without the 3 status lines).
fn layout(r: &Rig, w: u16, h: u16) -> ui::MeasLayout {
    ui::meas_layout(&r.app, w, h - 3).expect("a measure panel")
}

/// The text of the measure panel: the cells right of its left border.
fn meas_text(r: &Rig, w: u16, h: u16) -> String {
    let l = layout(r, w, h);
    let x0 = (w - l.width()) as usize;
    r.screen(w, h)
        .iter()
        .map(|line| line.chars().skip(x0).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn measure_panel_picks_the_narrowest_layout_that_fits() {
    let mut r = rig();
    // 120x40, CH1 and CH2: stacked, one value column.
    let l = layout(&r, 120, 40);
    assert_eq!(
        (l.bands.clone(), l.columns, l.fits),
        (vec![vec![0], vec![1]], 1, true)
    );
    let t = meas_text(&r, 120, 40);
    let ch1 = t.find("CH1").unwrap();
    let ch2 = t.find("CH2").unwrap();
    assert!(ch1 < ch2, "{t}");
    assert_eq!(t.matches("Vpp").count(), 2, "{t}");
    assert_eq!(t.matches("Fall").count(), 2, "{t}");
    // The panel is one column wide, so the plot gets the rest.
    assert_eq!(l.width(), 20);
    // With MATH: CH1 and CH2 side by side, MATH below.
    r.char('3');
    r.feed();
    let l = layout(&r, 120, 40);
    assert_eq!(l.bands, vec![vec![0, 1], vec![2]]);
    assert!(l.fits);
    let t = meas_text(&r, 120, 40);
    assert_eq!(t.matches("Fall").count(), 2, "{t}");
    assert!(t.contains("MATH"), "{t}");
    // 160x40 with the time cursors: all three as columns.
    cursors_on(&mut r, &[0, 1]);
    let l = layout(&r, 160, 40);
    assert_eq!(
        (l.bands.clone(), l.columns, l.fits),
        (vec![vec![0, 1, 2]], 3, true)
    );
    let t = meas_text(&r, 160, 40);
    assert!(
        t.contains("V@A") && t.contains("V@B") && t.contains("1/ΔT"),
        "{t}"
    );
    // Without cursors 160x40 needs only two columns.
    r.app.scope.settings.cursors.hidden = true;
    assert_eq!(layout(&r, 160, 40).bands, vec![vec![0, 1], vec![2]]);
    // 200x60: three stacked sections fit in the height.
    let l = layout(&r, 200, 60);
    assert_eq!(
        (l.bands.clone(), l.fits),
        (vec![vec![0], vec![1], vec![2]], true)
    );
    // Tall and narrow, 100x60: stacked too.
    assert_eq!(layout(&r, 100, 60).bands, vec![vec![0], vec![1], vec![2]]);
    // With cursors 100x60 is too low for three stacked sections and too
    // narrow for columns: the selected trace and the cursors come first.
    r.app.scope.settings.cursors.hidden = false;
    let l = layout(&r, 100, 60);
    assert_eq!(
        (l.bands.clone(), l.cursors_at, l.fits),
        (vec![vec![0], vec![1], vec![2]], 1, false)
    );
    // The layout does not depend on the values.
    let before = layout(&r, 120, 40);
    r.app.scope.settings.channels[0].volt_idx = 0;
    r.feed();
    assert_eq!(layout(&r, 120, 40), before);
}

#[test]
fn measure_panel_shows_the_selected_trace_first_when_short() {
    let mut r = rig();
    r.char('3');
    r.feed();
    // 80x24: too low for every section, too narrow for columns.
    let l = layout(&r, 80, 24);
    assert!(!l.fits);
    assert_eq!(l.columns, 1);
    assert_eq!(l.bands, vec![vec![0], vec![1], vec![2]]);
    let t = meas_text(&r, 80, 24);
    assert!(t.contains("CH1") && !t.contains("MATH"), "{t}");
    // Select MATH: its section comes first, in full.
    r.key(KeyCode::Tab);
    r.key(KeyCode::Tab);
    assert_eq!(r.app.scope.settings.selected, 2);
    let l = layout(&r, 80, 24);
    assert_eq!(l.bands, vec![vec![2], vec![0], vec![1]]);
    let t = meas_text(&r, 80, 24);
    assert!(t.contains("MATH"), "{t}");
    assert!(t.contains("Fall"), "{t}");
    // Select CH2.
    r.key(KeyCode::BackTab);
    let t = meas_text(&r, 80, 24);
    let first = t.lines().find(|l| l.contains("CH")).unwrap();
    assert!(first.contains("CH2"), "{t}");
}

#[test]
fn measure_panel_shows_cursors_before_the_other_traces_when_short() {
    let mut r = rig();
    cursors_on(&mut r, &[0, 1]);
    r.key(KeyCode::Tab); // CH2 selected
    let t = meas_text(&r, 80, 30);
    let ch2 = t.find("CH2").expect("CH2");
    let cur = t.find("Cursors").expect("Cursors section");
    assert!(ch2 < cur, "{t}");
    // CH1 is after the cursors, or cut.
    if let Some(ch1) = t.find("CH1") {
        assert!(ch1 > cur, "{t}");
    }
    assert!(t.contains("ΔT"), "{t}");
}

#[test]
fn settings_open_with_s_enter_and_f2_and_trigger_source_is_capital_s() {
    for k in [KeyCode::Char('s'), KeyCode::Enter, KeyCode::F(2)] {
        let mut r = rig();
        r.key(k);
        assert!(r.app.panel_open, "{k:?}");
        // Inside the panel Enter goes into the column, it does not close.
        r.key(KeyCode::Enter);
        assert!(r.app.panel_open);
        assert_eq!(r.app.panel.focus, openhantek_tui::panel::Focus::Items);
        r.char('s');
        assert!(!r.app.panel_open);
    }
    let mut r = rig();
    r.char('S');
    assert_eq!(r.app.scope.settings.trigger.source, 1);
    // m is the measurement panel now, M the math function.
    r.char('m');
    assert!(!r.app.scope.settings.display.measurements);
    r.char('M');
    assert!(r.app.scope.settings.math.enabled);
}

#[test]
fn rebound_keys_work_and_show_in_the_help() {
    use openhantek_tui::keys::KeyMap;
    let mut r = rig();
    let (keys, warnings) = KeyMap::from_table(&[
        ("settings".into(), vec!["o".into()]),
        ("help".into(), vec!["F10".into()]),
        ("run_stop".into(), vec!["Ctrl-r".into()]),
    ]);
    // `o` was single shot: settings is first and keeps it.
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("key o is bound to settings and single"));
    r.app.keys = keys;
    r.char('s');
    assert!(!r.app.panel_open);
    r.char('o');
    assert!(r.app.panel_open);
    r.char('q');
    assert!(!r.app.panel_open);
    r.app
        .handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    assert!(!r.app.scope.running);
    r.char(' ');
    assert!(!r.app.scope.running, "space is not run/stop any more");
    r.key(KeyCode::F(10));
    assert!(r.app.help);
    let t = r.text(120, 40);
    assert!(t.contains("F10") && t.contains("Ctrl-r"), "{t}");
    assert!(t.contains("o Enter F2") || t.contains("o "), "{t}");
    r.key(KeyCode::Esc);
    assert!(!r.app.help);
    // The hint line uses the bindings too.
    r.app.message = None;
    let t = r.text(120, 40);
    let hint = t.lines().last().unwrap();
    assert!(hint.contains("F10 help · o settings"), "{hint}");
}

#[test]
fn fine_cursor_keys_move_a_hundredth_of_a_division() {
    let mut r = rig();
    cursors_on(&mut r, &[0, 3]);
    r.app
        .handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::ALT));
    assert!((r.app.scope.settings.cursors.time_a.position + 1.99).abs() < 1e-9);
    // Alt-Shift-k arrives as Alt with a capital K (or with SHIFT).
    r.app
        .handle_key(KeyEvent::new(KeyCode::Char('K'), KeyModifiers::ALT));
    r.app.handle_key(KeyEvent::new(
        KeyCode::Char('k'),
        KeyModifiers::ALT | KeyModifiers::SHIFT,
    ));
    assert!((r.app.scope.settings.cursors.volt_b.position - 1.02).abs() < 1e-9);
}

fn alt(r: &mut Rig, code: KeyCode) {
    r.app.handle_key(KeyEvent::new(code, KeyModifiers::ALT));
}

#[test]
fn fine_volts_div_keeps_the_gain_of_the_bigger_step() {
    let mut r = rig();
    let gain_1v = r.app.scope.hw.unwrap().gains[0];
    // Alt-↓ from 1 V/div: 1.1 V/div, on the 2 V step.
    alt(&mut r, KeyCode::Down);
    let ch = &r.app.scope.settings.channels[0];
    assert!((ch.volts_per_div() - 1.1).abs() < 1e-12);
    assert_eq!(VOLT_DIVS[ch.volt_idx], 2.0);
    r.feed();
    let hw = r.app.scope.hw.unwrap();
    assert_eq!(
        hw.gains[0],
        openhantek_tui::device::protocol::gain_index(2.0)
    );
    assert_ne!(hw.gains[0], gain_1v);
    let t = r.text(160, 40);
    assert!(t.contains("CH1 1.1 V/div"), "{t}");
    // The plot labels use the exact value: 2 divisions above 0 V is 2.2 V.
    assert!(t.contains("2.2V"), "{t}");
    // Measurements are in volts, not divisions: the same sine.
    let vpp = r.app.scope.measure(0).unwrap().pk_pk;
    assert!((vpp - 3.0).abs() < 0.1, "{vpp}");
    for _ in 0..3 {
        alt(&mut r, KeyCode::Down);
    }
    assert!((r.app.scope.settings.channels[0].volts_per_div() - 1.4).abs() < 1e-12);
    alt(&mut r, KeyCode::Up);
    assert!((r.app.scope.settings.channels[0].volts_per_div() - 1.3).abs() < 1e-12);
    // A normal ↑ snaps to the step below, ↓ to the step above.
    r.key(KeyCode::Up);
    assert_eq!(r.app.scope.settings.channels[0].volts_per_div(), 1.0);
    alt(&mut r, KeyCode::Down);
    r.key(KeyCode::Down);
    assert_eq!(r.app.scope.settings.channels[0].volts_per_div(), 2.0);
    assert_eq!(r.app.scope.settings.channels[0].volts_fine, 1.0);
}

#[test]
fn fine_time_div_keeps_the_rate_of_the_bigger_step() {
    let mut r = rig();
    // 1 ms/div; Alt-→ gives 1.1 ms/div on the 2 ms step.
    alt(&mut r, KeyCode::Right);
    let s = &r.app.scope.settings;
    assert!((s.time_per_div() - 1.1e-3).abs() < 1e-15);
    assert_eq!(s.coarse_time_per_div(), 2e-3);
    r.feed();
    let mut want = openhantek_tui::scope::Scope::new(Settings::default());
    want.settings.time_idx = time_index(2e-3);
    assert_eq!(r.app.scope.hw.unwrap().rate, want.hw_config().rate);
    let t = r.text(160, 40);
    assert!(t.contains("1.1 ms/div"), "{t}");
    // The screen shows 11 ms: the labels end at 5.5 ms.
    assert!(t.contains("5.5ms"), "{t}");
    // Frequency is still right.
    let f = r.app.scope.measure(0).unwrap().freq.unwrap();
    assert!((f - 1000.0).abs() < 5.0, "{f}");
    alt(&mut r, KeyCode::Left);
    assert_eq!(r.app.scope.settings.time_per_div(), 1e-3);
    alt(&mut r, KeyCode::Left);
    assert!((r.app.scope.settings.time_per_div() - 0.95e-3).abs() < 1e-15);
    // Normal ←/→ snap back to the list.
    r.key(KeyCode::Right);
    assert_eq!(r.app.scope.settings.time_per_div(), 1e-3);
    assert_eq!(r.app.scope.settings.time_fine, 1.0);
}

#[test]
fn alt_gives_a_tenth_of_the_position_and_trigger_steps() {
    let mut r = rig();
    alt(&mut r, KeyCode::Char(']'));
    assert!((r.app.scope.settings.channels[0].offset - 2.05).abs() < 1e-9);
    alt(&mut r, KeyCode::Char('{'));
    assert!((r.app.scope.settings.channels[0].offset - 2.04).abs() < 1e-9);
    alt(&mut r, KeyCode::Char('+'));
    assert!((r.app.scope.settings.trigger.level - 0.01).abs() < 1e-9);
    alt(&mut r, KeyCode::Char('.'));
    assert!((r.app.scope.settings.trigger.position - 0.505).abs() < 1e-9);
    // In the value box of a number, Alt-k steps a tenth.
    r.char('s');
    while r.app.panel.groups()[r.app.panel.cat].name != "Trigger" {
        r.char('j');
    }
    r.char('l');
    while r.app.panel.current() != Some(openhantek_tui::panel::Field::TrigLevel) {
        r.char('j');
    }
    r.char('l');
    alt(&mut r, KeyCode::Char('k'));
    assert!((r.app.scope.settings.trigger.level - 0.02).abs() < 1e-9);
    r.char('k');
    assert!((r.app.scope.settings.trigger.level - 0.12).abs() < 1e-9);
}

#[test]
fn measure_cursors_heading_is_right_aligned_in_the_value_colour() {
    use openhantek_tui::settings::CursorColor;
    use openhantek_tui::theme::Palette;
    let check = |r: &Rig, w: u16, h: u16| {
        let lines = r.screen(w, h);
        let buf = r.buffer(w, h);
        let l = layout(r, w, h);
        let x0 = (w - l.width()) as usize;
        // Right edge (column of the last character) of `needle` in a row.
        let right = |y: usize, needle: &str| -> usize {
            let cells: Vec<char> = lines[y].chars().collect();
            let s: String = cells[x0..].iter().collect();
            let i = s.find(needle).unwrap();
            x0 + s[..i].chars().count() + needle.chars().count() - 1
        };
        let ch1 = lines.iter().position(|s| s.contains("CH1 ") || s.contains("CH1│")).unwrap();
        let hy = lines.iter().position(|s| s.contains("Cursors")).unwrap_or_else(|| panic!("{}", lines.join("\n")));
        assert_eq!(right(hy, "Cursors"), right(ch1, "CH1"), "{w}x{h}\n{}", lines.join("\n"));
        let vy = lines.iter().position(|s| s.contains("│ΔT ") || s.contains("ΔT ")).unwrap();
        let head = buf[(right(hy, "Cursors") as u16, hy as u16)].style().fg;
        let value = buf[(right(vy, "ms") as u16, vy as u16)].style().fg;
        let pal = Palette::for_setting(r.app.scope.settings.display.colors);
        assert_eq!(head, value, "{w}x{h}");
        assert_eq!(head, Some(ui::cursor_section_color(&r.app.scope.settings, &pal)));
        l.columns
    };
    let mut r = rig();
    cursors_on(&mut r, &[0, 1]);
    // Stacked and side by side.
    assert_eq!(check(&r, 150, 60), 1);
    assert!(check(&r, 200, 40) > 1);
    // One colour for all cursors: the heading takes it.
    for i in 0..2 {
        r.app.scope.settings.cursors.get_mut(i).color = CursorColor::Green;
    }
    check(&r, 150, 60);
    let pal = Palette::for_setting(r.app.scope.settings.display.colors);
    assert_eq!(
        ui::cursor_section_color(&r.app.scope.settings, &pal),
        ratatui::style::Color::LightGreen
    );
    // Two colours: the palette cursor colour for heading and values.
    r.app.scope.settings.cursors.get_mut(1).color = CursorColor::Red;
    check(&r, 200, 40);
    assert_eq!(ui::cursor_section_color(&r.app.scope.settings, &pal), pal.cursor);
}

/// Lines inside the help popup on a `w` x `h` screen, all of them: the
/// help is scrolled with `j` to its end. Each line is the text between
/// the popup's borders.
fn help_popup_lines(r: &mut Rig, w: u16, h: u16) -> Vec<String> {
    let mut out = std::collections::BTreeMap::new();
    let mut last_top = None;
    loop {
        let lines = r.screen(w, h);
        let first = r.app.help_scroll.min(r.app.help_max.get());
        let top = lines.iter().position(|l| l.contains("┌ Keys")).unwrap();
        let x0 = lines[top].chars().position(|c| c == '┌').unwrap();
        let bottom = top + lines[top..].iter().position(|l| l.chars().nth(x0) == Some('└')).unwrap();
        let x1 = x0 + lines[top].chars().skip(x0).position(|c| c == '┐').unwrap();
        let title = lines[top].clone();
        for (i, l) in lines[top + 1..bottom].iter().enumerate() {
            let inner: String = l.chars().skip(x0 + 1).take(x1 - x0 - 1).collect();
            out.insert(first + i, inner);
        }
        if !title.contains("scroll") || last_top.as_ref() == Some(&title) {
            break;
        }
        last_top = Some(title);
        // One page on.
        for _ in 0..(bottom - top - 1) {
            r.char('j');
        }
    }
    out.into_values().collect()
}

#[test]
fn help_fits_at_80x24_and_120x40_without_alt_rows_or_cut_words() {
    for (w, h) in [(80u16, 24u16), (120, 40)] {
        let mut r = rig();
        r.char('?');
        let lines = help_popup_lines(&mut r, w, h);
        assert!(r.app.help, "{w}x{h}: scrolling closed the help");
        let alt: Vec<&String> = lines.iter().filter(|l| l.contains("Alt")).collect();
        // One Alt row (it may wrap onto a second line without "Alt").
        assert_eq!(alt.len(), 1, "{w}x{h}: {alt:?}");
        assert!(alt[0].contains("Alt + key") && alt[0].contains("fine step"), "{alt:?}");
        assert!(lines.iter().all(|l| !l.contains("Alt-")), "{w}x{h}");
        // Every word of every row is on the screen whole.
        let words: std::collections::HashSet<&str> =
            lines.iter().flat_map(|l| l.split_whitespace()).collect();
        for (k, d) in ui::scope_help_rows(&r.app) {
            for word in k.split_whitespace().chain(d.split_whitespace()) {
                assert!(words.contains(word), "{w}x{h}: {word:?} of {d:?} cut\n{}", lines.join("\n"));
            }
        }
        // Lines end inside the popup with a free column (no text runs
        // into the border or the next column).
        for l in &lines {
            assert!(l.ends_with(' '), "{w}x{h}: {l:?}");
            // Two columns: the left one ends with a free column too.
            let n = l.chars().count();
            if n >= 80 {
                assert_eq!(l.chars().nth(n / 2 - 1), Some(' '), "{w}x{h}: {l:?}");
            }
        }
    }
    // 120x40 shows it all at once, 80x24 scrolls.
    let mut r = rig();
    r.char('?');
    assert!(r.text(120, 40).contains("Keys · any key closes"));
    assert!(r.text(80, 24).contains("j k PgUp PgDn scroll"));
    // Scrolling keys do not close the help, other keys do.
    r.char('j');
    r.key(KeyCode::PageDown);
    assert!(r.app.help && r.app.help_scroll > 1);
    r.char('x');
    assert!(!r.app.help && !r.app.scope.settings.display.xy);
}

#[test]
fn log_help_has_one_alt_row() {
    let mut r = rig();
    r.app.show_log = true;
    r.char('?');
    let lines = help_popup_lines(&mut r, 120, 40);
    assert_eq!(lines.iter().filter(|l| l.contains("Alt")).count(), 1, "{lines:?}");
}

/// Braille cells with at least 3 dots in the screen text.
fn dense_cells(lines: &[String]) -> usize {
    lines
        .iter()
        .flat_map(|l| l.chars())
        .filter(|c| ('\u{2800}'..='\u{28FF}').contains(c) && (*c as u32 - 0x2800).count_ones() >= 3)
        .count()
}

/// A 1 MHz square of ±1 V at 1 ms/div: 16 raw samples per point at
/// 16 MS/s see both levels, so the mean is a flat line at 0 V and the
/// peak area covers -1 V .. 1 V.
fn square_rig() -> Rig {
    let sq = Wave::Square {
        freq: 1e6,
        low: -1.0,
        high: 1.0,
        duty: 0.5,
    };
    let mut r = Rig::new([sq, Wave::Dc(0.0)]);
    r.app.scope.settings.channels[1].enabled = false;
    r.app.scope.settings.channels[0].offset = 0.0;
    r.feed();
    r.feed();
    r
}

#[test]
fn trace_mode_draws_the_peak_area() {
    let mut r = square_rig();
    assert_eq!(r.app.scope.settings.display.trace_mode, TraceMode::Average);
    let line = dense_cells(&r.screen(120, 30));
    // D: peak.
    r.char('D');
    assert_eq!(r.app.scope.settings.display.trace_mode, TraceMode::Peak);
    let peak = dense_cells(&r.screen(120, 30));
    // The area is 2 divisions high across the screen; the line is thin.
    assert!(peak > 4 * line.max(10), "line {line} peak {peak}");
    // D: both. The line is bright, the area muted.
    r.char('D');
    assert_eq!(r.app.scope.settings.display.trace_mode, TraceMode::Both);
    let buf = r.buffer(120, 30);
    let pal = openhantek_tui::theme::Palette::for_setting(r.app.scope.settings.display.colors);
    let count = |c: ratatui::style::Color| {
        buf.content()
            .iter()
            .filter(|x| x.fg == c && x.symbol().chars().any(|ch| ('\u{2801}'..='\u{28FF}').contains(&ch)))
            .count()
    };
    assert!(count(pal.muted(0)) > 50, "muted {}", count(pal.muted(0)));
    assert!(count(pal.trace[0]) > 10, "bright {}", count(pal.trace[0]));
    // Every style and the persistence draw it.
    r.app.scope.settings.display.persistence = 4;
    for style in TraceStyle::ALL {
        r.app.scope.settings.display.trace_style[0] = style;
        for mode in TraceMode::ALL {
            r.app.scope.settings.display.trace_mode = mode;
            r.feed();
            let t = r.text(120, 30);
            if mode.peak() {
                match style {
                    TraceStyle::Line => assert!(t.contains(':'), "{t}"),
                    TraceStyle::Box => assert!(t.contains('░'), "{t}"),
                    _ => assert!(dense_cells(&r.screen(120, 30)) > 4 * line.max(10)),
                }
            }
        }
    }
    // D cycles back to average.
    r.app.scope.settings.display.trace_mode = TraceMode::Both;
    r.char('D');
    assert_eq!(r.app.scope.settings.display.trace_mode, TraceMode::Average);
}

#[test]
fn max_adc_rate_setting_caps_the_scope() {
    let mut r = rig();
    assert_eq!(r.app.scope.hw.unwrap().rate.raw, 16e6);
    r.app.scope.settings.acquisition.max_adc_rate = 2e6;
    r.feed();
    let hw = r.app.scope.hw.unwrap();
    assert_eq!((hw.rate.raw, hw.rate.rate), (2e6, 1e6));
    assert!(r.app.scope.frame.is_some());
    // The panel shows the choices with the CH1 rate.
    let s = &r.app.scope.settings;
    let opts = openhantek_tui::panel::Field::MaxAdc.options(s, &Default::default());
    assert_eq!(opts[0], "16 MS/s · CH1 alone 30 MS/s");
    assert_eq!(openhantek_tui::panel::Field::MaxAdc.value(s, &Default::default()), "2 MS/s");
}
