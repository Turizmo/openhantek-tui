//! Drawing the screen with ratatui.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Wrap},
    Frame as TermFrame,
};

use crate::app::App;
use crate::dsp::fft::{spectrum, Spectrum};
use crate::dsp::measure::Measurements;
use crate::dsp::sinc_interpolate;
use crate::panel::{Focus, Kind};
use crate::plot::{self, Geometry, Plot, LABEL_W, MARKER_W};
use crate::scope::{Frame, TrigState, MATH};
use crate::settings::*;
use crate::si::{fmt_opt, fmt_percent, fmt_si};
use crate::theme::Palette;

mod log;

const NAMES: [&str; 3] = ["CH1", "CH2", "MATH"];

/// Spectrum scale: top line and dB per division.
pub const SPECTRUM_TOP_DBV: f64 = 20.0;
pub const SPECTRUM_DB_PER_DIV: f64 = 15.0;

/// Smallest settings panel, in rows, when the screen is short.
const PANEL_MIN_ROWS: u16 = 7;
/// Rows of the panel around its columns: the top border with the title,
/// the help line, and the bottom border with the key hints.
pub const PANEL_CHROME_ROWS: u16 = 3;
/// Rows the main view keeps under an open panel: a bordered plot of one
/// row per division with no margins.
const PLOT_MIN_ROWS: u16 = 2 + V_DIVS as u16 + 1;
/// The status lines below the panel.
const STATUS_ROWS: u16 = 3;

pub fn draw(f: &mut TermFrame, app: &App) {
    let area = f.area();
    let pal = Palette::for_setting(app.scope.settings.display.colors);
    let panel_h = if app.panel_open {
        let s = &app.scope.settings;
        let rows = app.panel.column_rows(s, &app.device_info()) as u16;
        panel_height(rows + PANEL_CHROME_ROWS, area.height)
    } else {
        0
    };
    let [main, panel, status1, status2, status3] = Layout::vertical([
        Constraint::Min(6),
        Constraint::Length(panel_h),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    let s = &app.scope.settings;

    if app.show_log {
        log::draw_log(f, app, main, &pal);
        if app.panel_open {
            draw_panel(f, app, panel, &pal);
        }
        log::draw_log_status(f, app, [status1, status2, status3], &pal);
        if app.help {
            draw_help(f, area, &log_help_rows());
        }
        return;
    }

    // Measurement panel on the right, if there is room.
    let panel_w = measurement_width(app, main.width);
    let [view, meas] =
        Layout::horizontal([Constraint::Min(20), Constraint::Length(panel_w)]).areas(main);
    if panel_w > 0 {
        draw_measurements(f, app, meas, &pal);
    }
    if s.display.spectrum && !s.display.xy {
        let time_h = split_height(view.height);
        let [time, spec] =
            Layout::vertical([Constraint::Length(time_h), Constraint::Min(0)]).areas(view);
        draw_time(f, app, time, &pal);
        draw_spectrum(f, app, spec, &pal);
    } else if s.display.xy {
        draw_xy(f, app, view, &pal);
    } else {
        draw_time(f, app, view, &pal);
    }
    if app.panel_open {
        draw_panel(f, app, panel, &pal);
    }
    draw_channel_line(f, app, status1, &pal);
    match log::rec_status(app) {
        Some(line) => f.render_widget(Paragraph::new(line), status2),
        None => draw_time_line(f, app, status2),
    }
    draw_message_line(f, app, status3);
    if app.help {
        draw_help(f, area, &KEYS);
    }
}

/// The help rows of the log view: its keys, then the scope keys.
fn log_help_rows() -> Vec<(&'static str, &'static str)> {
    let mut rows: Vec<(&str, &str)> = crate::keys::LOG_HELP.to_vec();
    rows.push(("", ""));
    rows.extend(KEYS.iter().filter(|(k, _)| {
        [
            "q Esc",
            "? F1",
            "S F2",
            "Tab S-Tab",
            "1 2 3",
            "↑ ↓",
            "[ ]",
            "{ }",
            "0",
        ]
        .contains(k)
    }));
    rows
}

/// Rows for the settings panel on a screen `h` rows high, when its
/// content needs `need` rows. It does not depend on the cursor, so the
/// panel keeps its size while moving. It shrinks (and the columns scroll)
/// only when the screen is too short for it and a small plot; then it
/// keeps at least [`PANEL_MIN_ROWS`], and the plot at least 6 rows.
pub fn panel_height(need: u16, h: u16) -> u16 {
    let room = h.saturating_sub(STATUS_ROWS + PLOT_MIN_ROWS);
    need.min(room.max(PANEL_MIN_ROWS))
        .min(h.saturating_sub(STATUS_ROWS + 6))
}

/// Height of the time view when the spectrum is below it: about 55 % of
/// the rows, rounded down to a whole number of rows per division.
fn split_height(h: u16) -> u16 {
    let target = h * 55 / 100;
    (1..=40)
        .rev()
        .map(|ry| 2 + Geometry::height_for(ry))
        .find(|need| *need <= target)
        .unwrap_or(target)
}

pub fn trace_enabled(s: &Settings, t: usize) -> bool {
    if t == MATH {
        s.math.enabled
    } else {
        s.channels[t].enabled
    }
}

/// Units per division and offset (divisions) of a trace.
pub fn trace_scale(s: &Settings, t: usize) -> (f64, f64) {
    if t == MATH {
        (s.math.units_per_div(), s.math.offset)
    } else {
        (s.channels[t].volts_per_div(), s.channels[t].offset)
    }
}

fn trace_unit(s: &Settings, t: usize) -> &'static str {
    if t == MATH {
        s.math.mode.unit()
    } else {
        "V"
    }
}

/// Traces in drawing order: the selected trace last, so it is on top.
fn draw_order(s: &Settings) -> Vec<usize> {
    let mut order: Vec<usize> = (0..3).filter(|&t| t != s.selected).collect();
    order.push(s.selected);
    order
}

/// Plot layer of a trace: the selected one is on top, then CH1, CH2, MATH.
fn layer_of(s: &Settings, t: usize) -> u8 {
    if t == s.selected {
        plot::LAYER_TOP
    } else {
        [plot::LAYER_CH1, plot::LAYER_CH2, plot::LAYER_MATH][t]
    }
}

/// Screen points of a trace, in divisions, in time order. Few samples get
/// sin(x)/x points between them. With `dots`, only the real samples.
pub fn trace_points(
    frame: &Frame,
    t: usize,
    s: &Settings,
    dots_x: usize,
    dots: bool,
) -> Vec<(f64, f64)> {
    let Some(tr) = frame.traces[t].as_ref() else {
        return Vec::new();
    };
    let (scale, offset) = trace_scale(s, t);
    let half_v = V_DIVS / 2.0;
    let y = |v: f64| {
        if v.is_finite() {
            (v / scale + offset).clamp(-half_v, half_v)
        } else {
            f64::NAN
        }
    };
    let r = frame.visible(s, tr.len());
    let n = r.len();
    if n == 0 {
        return Vec::new();
    }
    if dots {
        return r.map(|i| (frame.x(i as f64, s), y(tr[i]))).collect();
    }
    if s.display.interpolation == Interpolation::Sinc && n * 4 < dots_x {
        let factor = (dots_x / n).clamp(2, 32);
        let lo = r.start.saturating_sub(6);
        let hi = (r.end + 6).min(tr.len());
        let fine = sinc_interpolate(&tr[lo..hi], factor);
        return fine
            .iter()
            .enumerate()
            .map(|(j, v)| (frame.x(lo as f64 + j as f64 / factor as f64, s), y(*v)))
            .collect();
    }
    // One sample more on each side, so the line reaches the screen edges.
    let lo = r.start.saturating_sub(1);
    let hi = (r.end + 1).min(tr.len());
    (lo..hi).map(|i| (frame.x(i as f64, s), y(tr[i]))).collect()
}

/// The grid in the style of the display settings.
fn grid(plot: &mut Plot, s: &Settings, pal: &Palette) {
    let d = &s.display;
    plot.grid_with(pal, d.grid_style, d.grid_extent, d.grid_brightness);
}

fn draw_trace(plot: &mut Plot, frame: &Frame, t: usize, s: &Settings, layer: u8, color: Color) {
    let style = s.display.trace_style[t];
    // Braille thin shows the same points with and without dots, like the
    // first version did: sin(x)/x points too.
    let dots = s.display.dots && style != TraceStyle::BrailleThin;
    let pts = trace_points(frame, t, s, plot.dots_w(), dots);
    plot.styled(&pts, style, s.display.dots, layer, color);
}

/// Colour of the trigger mark and text when the level is outside the
/// input range.
pub const TRIGGER_OUT_OF_RANGE: Color = Color::Red;

/// Colour of the classic cursor lines and their names.
const CLASSIC_CURSOR: Color = Color::Gray;
const CLASSIC_CURSOR_NAME: Color = Color::White;

/// Colours of a cursor: the line and its name. Grey is the look of the
/// first versions: grey lines with white names in the plot, the palette
/// cursor colour for fine dashes.
pub fn cursor_colors(c: &Cursor, pal: &Palette) -> (Color, Color) {
    let fixed = match c.color {
        CursorColor::Grey if c.style == CursorStyle::Fine => return (pal.cursor, pal.cursor),
        CursorColor::Grey => return (CLASSIC_CURSOR, CLASSIC_CURSOR_NAME),
        CursorColor::White => Color::White,
        CursorColor::Yellow => Color::LightYellow,
        CursorColor::Cyan => Color::LightCyan,
        CursorColor::Magenta => Color::LightMagenta,
        CursorColor::Green => Color::LightGreen,
        CursorColor::Red => Color::LightRed,
        CursorColor::Blue => Color::LightBlue,
    };
    (fixed, fixed)
}

/// One cursor line: vertical at x (`time`) or horizontal at y, in
/// divisions, in the cursor's style and colour.
pub fn draw_cursor_line(plot: &mut Plot, c: &Cursor, time: bool, pal: &Palette) {
    let (half_h, half_v) = (H_DIVS / 2.0, V_DIVS / 2.0);
    let layer = plot::LAYER_CURSOR;
    let (color, _) = cursor_colors(c, pal);
    let p = c.position;
    match (c.style, time) {
        (CursorStyle::Classic, true) => {
            plot.dashed_classic((p, -half_v), (p, half_v), layer, color)
        }
        (CursorStyle::Classic, false) => {
            plot.dashed_classic((-half_h, p), (half_h, p), layer, color)
        }
        (CursorStyle::Fine, true) => plot.dashed_v(p, layer, color),
        (CursorStyle::Fine, false) => plot.dashed_h(p, layer, color),
        (CursorStyle::Solid, true) => plot.pattern_v(p, |_| true, layer, color),
        (CursorStyle::Solid, false) => plot.pattern_h(p, |_| true, layer, color),
        (CursorStyle::Dotted, true) => plot.pattern_v(p, |y| y % 4 == 0, layer, color),
        (CursorStyle::Dotted, false) => plot.pattern_h(p, |x| x % 4 == 0, layer, color),
    }
}

fn draw_cursors(plot: &mut Plot, s: &Settings, pal: &Palette) {
    for i in (0..4).filter(|&i| s.cursors.shown(i)) {
        draw_cursor_line(plot, s.cursors.get(i), CursorSettings::is_time(i), pal);
    }
}

fn title(app: &App) -> Line<'static> {
    let sc = &app.scope;
    let run = if sc.running {
        Span::styled(" RUN ", Style::default().fg(Color::Black).bg(Color::Green))
    } else {
        Span::styled(" STOP ", Style::default().fg(Color::White).bg(Color::Red))
    };
    let state = match sc.state {
        TrigState::Triggered => Span::styled("Trig'd", Style::default().fg(Color::Green)),
        TrigState::Waiting => Span::styled("Wait", Style::default().fg(Color::Yellow)),
        st => Span::raw(st.name()),
    };
    let mut spans = vec![
        Span::raw(" openhantek-tui · "),
        Span::styled(
            app.device_name.clone(),
            Style::default().fg(if app.connected {
                Color::White
            } else {
                Color::Red
            }),
        ),
        Span::raw(" "),
        run,
        Span::raw(" "),
        state,
    ];
    if let Some(f) = &sc.frame {
        spans.push(Span::raw(format!(" · {}", fmt_si(f.rate, "S/s"))));
        for (c, name) in NAMES.iter().enumerate().take(2) {
            if f.clipped[c] && sc.settings.channels[c].enabled {
                spans.push(Span::styled(
                    format!(" CLIP {name}"),
                    Style::default().fg(Color::White).bg(Color::Red),
                ));
            }
        }
    }
    if sc.autoset_running() {
        spans.push(Span::styled(" AUTOSET", Style::default().fg(Color::Yellow)));
    }
    if app.recording() {
        spans.push(Span::styled(
            " ● REC: live view paused, V shows the log ",
            Style::default().fg(Color::White).bg(Color::Red),
        ));
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
}

/// Short SI text with no space, like `-500mV` or `2ms`. The value is
/// rounded to a multiple of `step`, so tiny offsets do not show as `2.02mV`.
fn compact(v: f64, step: f64, unit: &str) -> String {
    let v = if step > 0.0 {
        (v / step).round() * step
    } else {
        v
    };
    let v = if v.abs() < 1e-12 { 0.0 } else { v };
    fmt_si(v, unit).replace(' ', "")
}

/// Write `text` at (x, y), cut at `max_x`.
fn put(buf: &mut Buffer, x: u16, y: u16, text: &str, style: Style, max_x: u16) {
    for (cx, ch) in (x..max_x).zip(text.chars()) {
        if let Some(cell) = buf.cell_mut((cx, y)) {
            cell.set_char(ch).set_style(style);
        }
    }
}

/// The labels left of the plot, one per division line from the top.
fn labels(label: impl Fn(usize) -> String) -> Vec<String> {
    (0..=plot::V_DIVS).map(label).collect()
}

/// Columns the longest label needs, at most [`LABEL_W`].
fn label_width(labels: &[String]) -> u16 {
    let w = labels.iter().map(|t| t.chars().count()).max().unwrap_or(0);
    (w as u16).min(LABEL_W)
}

/// Labels left of the plot, one per division line, right-aligned in the
/// label part of the left margin.
fn left_labels(buf: &mut Buffer, g: &Geometry, labels: &[String], style: Style) {
    let w = g.margins.label_w();
    if w == 0 {
        return;
    }
    let x = g.plot.x - g.margins.left;
    for (j, text) in labels.iter().enumerate() {
        let y = g.plot.y + (j * g.ry) as u16;
        let text: String = text.chars().take(w as usize).collect();
        let pad = w as usize - text.chars().count();
        put(buf, x + pad as u16, y, &text, style, x + w);
    }
}

/// Labels below the plot, one per division (fewer when they do not fit).
/// The end labels may stick out into the spare cells next to the margins;
/// a label that would touch the one before it is left out.
fn bottom_labels(buf: &mut Buffer, g: &Geometry, label: impl Fn(usize) -> String, style: Style) {
    if g.margins.bottom == 0 {
        return;
    }
    let y = g.plot.y + g.plot.height;
    let min_x = g.area.x;
    let max_x = g.area.x + g.area.width;
    let texts: Vec<String> = (0..=plot::H_DIVS).map(&label).collect();
    let widest = texts.iter().map(|t| t.chars().count()).max().unwrap_or(1);
    // Every division, every 2nd or every 5th, so labels do not touch.
    let every = [1, 2, 5, 10]
        .into_iter()
        .find(|e| e * g.cx > widest)
        .unwrap_or(10);
    let mut free_x = min_x;
    for (k, text) in texts.iter().enumerate() {
        if k % every != 0 {
            continue;
        }
        let w = text.chars().count() as u16;
        let center = g.plot.x + (k * g.cx) as u16;
        let x = center
            .saturating_sub(w / 2)
            .clamp(min_x, max_x.saturating_sub(w));
        if x < free_x {
            continue;
        }
        put(buf, x, y, text, style, max_x);
        free_x = x + w + 1;
    }
}

fn draw_time(f: &mut TermFrame, app: &App, area: Rect, pal: &Palette) {
    let sc = &app.scope;
    let s = &sc.settings;
    let block = Block::bordered().title(title(app));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let volts = time_labels(s);
    // One cell on the right for the trigger mark; two when the fine
    // voltage cursors put their names there.
    let fine_v = (2..4).any(|i| s.cursors.shown(i) && s.cursors.get(i).style == CursorStyle::Fine);
    let right = if fine_v { 2 } else { 1 };
    let Some(g) = Geometry::fit(inner, label_width(&volts), right) else {
        return;
    };
    let mut plot = Plot::new(g.cx, g.ry);
    grid(&mut plot, s, pal);
    draw_cursors(&mut plot, s, pal);
    let order = draw_order(s);
    // Old frames first, oldest first, fading by age: a newer frame is on
    // a higher layer, so it wins a cell over an older one.
    let n = sc.history.len();
    let depth = s.display.persistence.max(n);
    for (i, old) in sc.history.iter().enumerate() {
        let age = n - 1 - i;
        let layer = plot::persist_layer(i, n);
        for &t in &order {
            if trace_enabled(s, t) {
                let (color, dim) = pal.persist(t, age, depth);
                if dim {
                    plot.dim_upto(layer);
                }
                draw_trace(&mut plot, old, t, s, layer, color);
            }
        }
    }
    if let Some(frame) = &sc.frame {
        for &t in &order {
            if trace_enabled(s, t) {
                draw_trace(&mut plot, frame, t, s, layer_of(s, t), pal.trace[t]);
            }
        }
    }
    let buf = f.buffer_mut();
    plot.render(buf, g.plot);
    time_margins(buf, app, &g, &plot, &volts, pal);
    if sc.frame.is_none() {
        let text = app
            .status
            .clone()
            .unwrap_or_else(|| "Waiting for data...".to_string());
        let y = g.plot.y + g.plot.height / 2;
        put(
            buf,
            g.plot.x + 2,
            y.saturating_sub(1),
            &text,
            Style::default().fg(Color::Gray),
            g.plot.x + g.plot.width,
        );
    }
}

/// The value labels of the time view, for the selected trace.
fn time_labels(s: &Settings) -> Vec<String> {
    let sel = s.selected;
    let (scale, offset) = trace_scale(s, sel);
    let unit = trace_unit(s, sel);
    labels(|j| {
        compact(
            (V_DIVS / 2.0 - j as f64 - offset) * scale,
            scale / 100.0,
            unit,
        )
    })
}

/// Labels, ground markers, trigger marks and cursor names around the plot.
fn time_margins(
    buf: &mut Buffer,
    app: &App,
    g: &Geometry,
    plot: &Plot,
    volts: &[String],
    pal: &Palette,
) {
    let sc = &app.scope;
    let s = &sc.settings;
    let half_v = V_DIVS / 2.0;
    let sel = s.selected;
    left_labels(buf, g, volts, Style::default().fg(pal.trace[sel]));
    let roll = sc.frame.as_ref().is_some_and(|f| f.roll) || s.roll();
    let x_ref = match &sc.frame {
        Some(f) => f.x_ref(s),
        None if roll => H_DIVS / 2.0,
        None => -H_DIVS / 2.0 + s.trigger.position * H_DIVS,
    };
    let tdiv = s.time_per_div();
    bottom_labels(
        buf,
        g,
        |k| compact((k as f64 - H_DIVS / 2.0 - x_ref) * tdiv, tdiv / 100.0, "s"),
        Style::default().fg(pal.label),
    );
    let right_x = g.plot.x + g.plot.width;
    // Ground markers: where each trace has its 0 line.
    if g.margins.left >= MARKER_W {
        for t in draw_order(s) {
            if !trace_enabled(s, t) {
                continue;
            }
            let (_, off) = trace_scale(s, t);
            let y = g.plot.y + plot.row_of(off.clamp(-half_v, half_v)) as u16;
            let label = ["1▸", "2▸", "M▸"][t];
            let x = g.plot.x - MARKER_W;
            put(
                buf,
                x,
                y,
                label,
                Style::default().fg(pal.trace[t]),
                g.plot.x,
            );
        }
    }
    if !roll && !s.display.xy && g.margins.right >= 1 {
        let src = s.trigger.source;
        let (sc_, off) = trace_scale(s, src);
        let y = g.plot.y + plot.row_of((s.trigger.level / sc_ + off).clamp(-half_v, half_v)) as u16;
        // Red when the level is outside the input range (the trigger
        // uses a level inside it).
        let color = if sc.effective_trigger_level().is_some() {
            TRIGGER_OUT_OF_RANGE
        } else {
            pal.trace[src]
        };
        put(
            buf,
            right_x,
            y,
            "◂",
            Style::default().fg(color),
            right_x + 1,
        );
    }
    if !roll && g.margins.top >= 1 {
        let x = g.plot.x + plot.col_of(-H_DIVS / 2.0 + s.trigger.position * H_DIVS) as u16;
        put(
            buf,
            x,
            g.plot.y - 1,
            "▾",
            Style::default().fg(pal.trigger),
            x + 1,
        );
    }
    // Cursor names, only the letter: fine dashes put it in the margin (at
    // the top for a time line, at the right for a voltage line), the
    // other styles inside the plot (at the top right of a time line, at
    // the right on the row above a voltage line).
    let half_h = H_DIVS / 2.0;
    for i in (0..4).filter(|&i| s.cursors.shown(i)) {
        let c = s.cursors.get(i);
        let name = ["A", "B"][i % 2];
        let style = Style::default().fg(cursor_colors(c, pal).1);
        let time = CursorSettings::is_time(i);
        match (c.style == CursorStyle::Fine, time) {
            (true, true) if g.margins.top >= 1 => {
                let cx = g.plot.x + plot.col_of(c.position) as u16 + 1;
                put(buf, cx, g.plot.y - 1, name, style, right_x);
            }
            (true, false) if g.margins.right >= 2 => {
                let cy = g.plot.y + plot.row_of(c.position) as u16;
                put(buf, right_x + 1, cy, name, style, right_x + 2);
            }
            (true, _) => {}
            (false, true) => {
                let y = g.plot.y + plot.row_of(half_v - 0.3) as u16;
                let x = g.plot.x + plot.col_of((c.position + 0.05).min(half_h - 0.2)) as u16;
                put(buf, x, y, name, style, right_x);
            }
            (false, false) => {
                let x = g.plot.x + plot.col_of(half_h - 0.6) as u16;
                let y = g.plot.y + plot.row_of((c.position + 0.2).min(half_v)) as u16;
                put(buf, x, y, name, style, right_x);
            }
        }
    }
}

/// The trace the spectrum and cursors use: the selected one, if it is on.
fn main_trace(app: &App) -> Option<usize> {
    let s = &app.scope.settings;
    if trace_enabled(s, s.selected) {
        return Some(s.selected);
    }
    (0..3).find(|&t| trace_enabled(s, t))
}

/// Spectrum of trace `t` on screen.
pub fn trace_spectrum(app: &App, t: usize) -> Option<Spectrum> {
    let sc = &app.scope;
    let f = sc.frame.as_ref()?;
    let v = f.visible_samples(t, &sc.settings)?;
    let sp = spectrum(v, f.dt, sc.settings.display.window, 1 << 14);
    (!sp.rms.is_empty()).then_some(sp)
}

/// Spectrum points: x in Hz, y in dBV, the highest bin per dot column.
pub fn spectrum_points(sp: &Spectrum, span: f64, dots_x: usize) -> Vec<(f64, f64)> {
    let dots_x = dots_x.max(2);
    let bottom = SPECTRUM_TOP_DBV - SPECTRUM_DB_PER_DIV * V_DIVS;
    let mut cols: Vec<Option<f64>> = vec![None; dots_x];
    for k in 0..sp.rms.len() {
        let fr = sp.freq(k);
        if fr > span {
            break;
        }
        let px = ((fr / span) * (dots_x - 1) as f64).round() as usize;
        let db = sp.dbv(k).clamp(bottom, SPECTRUM_TOP_DBV);
        let c = &mut cols[px.min(dots_x - 1)];
        *c = Some(c.map_or(db, |old: f64| old.max(db)));
    }
    cols.iter()
        .enumerate()
        .filter_map(|(px, c)| c.map(|db| (px as f64 / (dots_x - 1) as f64 * span, db)))
        .collect()
}

fn draw_spectrum(f: &mut TermFrame, app: &App, area: Rect, pal: &Palette) {
    let s = &app.scope.settings;
    let rate = app.scope.frame.as_ref().map_or(1.0, |f| f.rate);
    let span = rate / 2.0 / 2f64.powi(s.display.spectrum_zoom as i32);
    let main = main_trace(app);
    let mut title = format!(
        " Spectrum · {} · {} dB/div · 0..{} ",
        s.display.window.name(),
        SPECTRUM_DB_PER_DIV,
        fmt_si(span, "Hz")
    );
    let block_inner = Block::bordered().inner(area);
    let db = labels(|j| format!("{:.0}dB", SPECTRUM_TOP_DBV - j as f64 * SPECTRUM_DB_PER_DIV));
    let geo = Geometry::fit(block_inner, label_width(&db), 0);
    let mut plot = geo.map(|g| Plot::new(g.cx, g.ry));
    if let Some(p) = plot.as_mut() {
        grid(p, &app.scope.settings, pal);
    }
    let to_div = |(hz, db): (f64, f64)| {
        (
            hz / span * H_DIVS - H_DIVS / 2.0,
            (db - SPECTRUM_TOP_DBV) / SPECTRUM_DB_PER_DIV + V_DIVS / 2.0,
        )
    };
    for t in draw_order(s) {
        if !trace_enabled(s, t) {
            continue;
        }
        let Some(sp) = trace_spectrum(app, t) else {
            continue;
        };
        if Some(t) == main {
            if let Some((pf, rms)) = sp.peak() {
                title.push_str(&format!(
                    "· {} peak {} {:.1} dBV ",
                    NAMES[t],
                    fmt_si(pf, "Hz"),
                    20.0 * rms.max(1e-12).log10()
                ));
            }
        }
        if let Some(p) = plot.as_mut() {
            let pts: Vec<(f64, f64)> = spectrum_points(&sp, span, p.dots_w() - 1)
                .into_iter()
                .map(to_div)
                .collect();
            p.styled(
                &pts,
                s.display.trace_style[t],
                false,
                layer_of(s, t),
                pal.trace[t],
            );
        }
    }
    f.render_widget(Block::bordered().title(title), area);
    let (Some(g), Some(p)) = (geo, plot) else {
        return;
    };
    let buf = f.buffer_mut();
    p.render(buf, g.plot);
    left_labels(buf, &g, &db, Style::default().fg(pal.label));
    bottom_labels(
        buf,
        &g,
        |k| compact(k as f64 / H_DIVS * span, span / 1000.0, "Hz"),
        Style::default().fg(pal.label),
    );
}

/// XY points: CH1 on x, CH2 on y, in divisions.
pub fn xy_points(app: &App) -> Vec<(f64, f64)> {
    let sc = &app.scope;
    let s = &sc.settings;
    let Some(f) = &sc.frame else {
        return Vec::new();
    };
    let (Some(a), Some(b)) = (f.visible_samples(0, s), f.visible_samples(1, s)) else {
        return Vec::new();
    };
    let (sx, ox) = trace_scale(s, 0);
    let (sy, oy) = trace_scale(s, 1);
    let (half_h, half_v) = (H_DIVS / 2.0, V_DIVS / 2.0);
    // Keep the drawing light: at most about 4000 points.
    let step = (a.len() / 4000).max(1);
    a.iter()
        .zip(b)
        .step_by(step)
        .map(|(x, y)| {
            (
                (x / sx + ox).clamp(-half_h, half_h),
                (y / sy + oy).clamp(-half_v, half_v),
            )
        })
        .collect()
}

fn draw_xy(f: &mut TermFrame, app: &App, area: Rect, pal: &Palette) {
    let s = &app.scope.settings;
    let points = xy_points(app);
    let mut t = title(app);
    t.spans.push(Span::raw("· XY: CH1 → x, CH2 → y "));
    let block = Block::bordered().title(t);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let (sx, ox) = trace_scale(s, 0);
    let (sy, oy) = trace_scale(s, 1);
    let volts = labels(|j| compact((V_DIVS / 2.0 - j as f64 - oy) * sy, sy / 100.0, "V"));
    let Some(g) = Geometry::fit(inner, label_width(&volts), 0) else {
        return;
    };
    let mut plot = Plot::new(g.cx, g.ry);
    grid(&mut plot, s, pal);
    if s.display.dots {
        plot.points(&points, plot::LAYER_TOP, pal.xy);
    } else {
        plot.polyline(&points, plot::LAYER_TOP, pal.xy);
    }
    let buf = f.buffer_mut();
    plot.render(buf, g.plot);
    left_labels(buf, &g, &volts, Style::default().fg(pal.trace[1]));
    bottom_labels(
        buf,
        &g,
        |k| compact((k as f64 - H_DIVS / 2.0 - ox) * sx, sx / 100.0, "V"),
        Style::default().fg(pal.trace[0]),
    );
}

/// The measurement rows: label and value text.
pub fn measurement_rows(m: &Measurements, unit: &str) -> Vec<(&'static str, String)> {
    vec![
        ("Vpp", fmt_si(m.pk_pk, unit)),
        ("Max", fmt_si(m.max, unit)),
        ("Min", fmt_si(m.min, unit)),
        ("Mean", fmt_si(m.mean, unit)),
        ("RMS", fmt_si(m.rms, unit)),
        ("AC RMS", fmt_si(m.ac_rms, unit)),
        ("Ampl", fmt_si(m.amplitude, unit)),
        ("Top", fmt_si(m.top, unit)),
        ("Base", fmt_si(m.base, unit)),
        ("Freq", fmt_opt(m.freq, "Hz")),
        ("Period", fmt_opt(m.period, "s")),
        ("Duty", fmt_percent(m.duty)),
        ("+Width", fmt_opt(m.pos_width, "s")),
        ("-Width", fmt_opt(m.neg_width, "s")),
        ("Rise", fmt_opt(m.rise, "s")),
        ("Fall", fmt_opt(m.fall, "s")),
    ]
}

const MEAS_LABEL_W: u16 = 7;
const VALUE_W: u16 = 11;

/// Traces shown in the panel. Narrow screens show only the main trace.
fn panel_traces(app: &App, width: u16) -> Vec<usize> {
    let s = &app.scope.settings;
    let all: Vec<usize> = (0..3).filter(|&t| trace_enabled(s, t)).collect();
    let full = MEAS_LABEL_W + VALUE_W * all.len() as u16 + 2;
    if width >= full + 60 {
        all
    } else {
        main_trace(app).into_iter().collect()
    }
}

fn measurement_width(app: &App, width: u16) -> u16 {
    if !app.scope.settings.display.measurements || app.scope.settings.display.xy {
        return 0;
    }
    let n = panel_traces(app, width).len() as u16;
    let w = MEAS_LABEL_W + VALUE_W * n + 2;
    if n == 0 || width < w + 40 {
        0
    } else {
        w
    }
}

fn draw_measurements(f: &mut TermFrame, app: &App, area: Rect, pal: &Palette) {
    let s = &app.scope.settings;
    let traces = panel_traces(app, area.width + 60);
    let measured: Vec<Option<Measurements>> =
        traces.iter().map(|&t| app.scope.measure(t)).collect();
    let mut lines = Vec::new();
    let mut head = vec![Span::raw(format!("{:<w$}", "", w = MEAS_LABEL_W as usize))];
    for &t in &traces {
        head.push(Span::styled(
            format!("{:>w$}", NAMES[t], w = VALUE_W as usize),
            Style::default()
                .fg(pal.trace[t])
                .add_modifier(Modifier::BOLD),
        ));
    }
    lines.push(Line::from(head));
    let rows: Vec<Vec<(&str, String)>> = traces
        .iter()
        .zip(&measured)
        .map(|(&t, m)| {
            m.as_ref()
                .map(|m| measurement_rows(m, trace_unit(s, t)))
                .unwrap_or_default()
        })
        .collect();
    let labels = measurement_rows(&Measurements::default(), "V");
    for (r, (label, _)) in labels.iter().enumerate() {
        let mut spans = vec![Span::styled(
            format!("{label:<w$}", w = MEAS_LABEL_W as usize),
            Style::default().fg(pal.label),
        )];
        for (k, &t) in traces.iter().enumerate() {
            let text = rows[k].get(r).map_or("--".to_string(), |(_, v)| v.clone());
            spans.push(Span::styled(
                format!("{text:>w$}", w = VALUE_W as usize),
                Style::default().fg(pal.trace[t]),
            ));
        }
        lines.push(Line::from(spans));
    }
    let cur = cursor_rows(app);
    let at: Vec<usize> = (0..2).filter(|&i| s.cursors.shown(i)).collect();
    if !cur.is_empty() {
        let w = MEAS_LABEL_W as usize;
        lines.push(Line::styled(
            "Cursors",
            Style::default().fg(pal.label).add_modifier(Modifier::BOLD),
        ));
        for (label, value) in &cur {
            lines.push(Line::from(vec![
                Span::styled(format!("{label:<w$}"), Style::default().fg(pal.label)),
                Span::raw(format!("{value:>v$}", v = VALUE_W as usize)),
            ]));
        }
        for &i in &at {
            let label = ["V@A", "V@B"][i];
            let mut spans = vec![Span::styled(
                format!("{label:<w$}"),
                Style::default().fg(pal.label),
            )];
            for &t in &traces {
                let text = value_at_cursor(app, t, i)
                    .map_or("--".to_string(), |v| fmt_si(v, trace_unit(s, t)));
                spans.push(Span::styled(
                    format!("{text:>v$}", v = VALUE_W as usize),
                    Style::default().fg(pal.trace[t]),
                ));
            }
            lines.push(Line::from(spans));
        }
    }
    f.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" Measure ")),
        area,
    );
}

fn channel_span(app: &App, t: usize, pal: &Palette) -> Span<'static> {
    let s = &app.scope.settings;
    let text = if t == MATH {
        format!(
            " M {} {}/div ",
            s.math.mode.name(),
            fmt_si(s.math.units_per_div(), s.math.mode.unit())
        )
    } else {
        let ch = &s.channels[t];
        format!(
            " {} {}/div {} x{}{} ",
            NAMES[t],
            fmt_si(ch.volts_per_div(), "V"),
            ch.coupling.name(),
            ch.probe,
            if ch.invert { " INV" } else { "" }
        )
    };
    let mut style = Style::default().fg(pal.trace[t]);
    if t == s.selected {
        style = style.add_modifier(Modifier::REVERSED);
    }
    if !trace_enabled(s, t) {
        style = style.add_modifier(Modifier::DIM | Modifier::CROSSED_OUT);
    }
    Span::styled(text, style)
}

fn draw_channel_line(f: &mut TermFrame, app: &App, area: Rect, pal: &Palette) {
    let mut spans = Vec::new();
    for t in 0..3 {
        spans.push(channel_span(app, t, pal));
        spans.push(Span::raw(" "));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The cursor readout for the measurement panel: rows of a label and a
/// value. Time cursors give their time, ΔT and 1/ΔT when both are on;
/// voltage cursors their voltage on the main trace, ΔV when both are on.
pub fn cursor_rows(app: &App) -> Vec<(&'static str, String)> {
    let s = &app.scope.settings;
    let c = &s.cursors;
    let mut rows = Vec::new();
    let tdiv = s.time_per_div();
    let t = |i: usize| c.get(i).position * tdiv;
    for (i, label) in [(0, "A t"), (1, "B t")] {
        if c.shown(i) {
            rows.push((label, fmt_si(t(i), "s")));
        }
    }
    if c.shown(0) && c.shown(1) {
        let d = (t(1) - t(0)).abs();
        rows.push(("ΔT", fmt_si(d, "s")));
        let inv = if d > 0.0 {
            fmt_si(1.0 / d, "Hz")
        } else {
            "--".into()
        };
        rows.push(("1/ΔT", inv));
    }
    let tr = main_trace(app).unwrap_or(0);
    let (scale, offset) = trace_scale(s, tr);
    let unit = trace_unit(s, tr);
    let v = |i: usize| (c.get(i).position - offset) * scale;
    for (i, label) in [(2, "A v"), (3, "B v")] {
        if c.shown(i) {
            rows.push((label, fmt_si(v(i), unit)));
        }
    }
    if c.shown(2) && c.shown(3) {
        rows.push(("ΔV", fmt_si((v(3) - v(2)).abs(), unit)));
    }
    rows
}

/// Value of trace `t` at time cursor `i` (0 = A, 1 = B), from the frame
/// on screen, interpolated between samples.
pub fn value_at_cursor(app: &App, t: usize, i: usize) -> Option<f64> {
    let s = &app.scope.settings;
    let f = app.scope.frame.as_ref()?;
    let tr = f.traces[t].as_ref()?;
    if tr.is_empty() || f.dt <= 0.0 {
        return None;
    }
    let k = (f.time_at(s.cursors.get(i).position, s) - f.t0) / f.dt;
    if k < 0.0 || k > (tr.len() - 1) as f64 {
        return None;
    }
    let j = (k.floor() as usize).min(tr.len() - 1);
    let next = tr[(j + 1).min(tr.len() - 1)];
    let frac = k - j as f64;
    Some(tr[j] + (next - tr[j]) * frac)
}

fn draw_time_line(f: &mut TermFrame, app: &App, area: Rect) {
    let s = &app.scope.settings;
    let gray = Style::default().fg(Color::Gray);
    let mut parts: Vec<Vec<Span>> = vec![vec![Span::raw(format!(
        " {}/div",
        fmt_si(s.time_per_div(), "s")
    ))]];
    let text = |t: String| vec![Span::raw(t)];
    if let Some(hw) = &app.scope.hw {
        parts.push(text(fmt_si(hw.rate.rate, "S/s")));
    }
    if s.roll() {
        parts.push(text("Roll".into()));
    } else {
        let src = s.trigger.source;
        let mut trig = vec![Span::raw(format!(
            "Trig {} {} {} {}",
            s.trigger.mode.name(),
            NAMES[src],
            s.trigger.slope.symbol(),
            fmt_si(s.trigger.level, "V"),
        ))];
        if let Some(used) = app.scope.effective_trigger_level() {
            trig.push(Span::styled(
                format!(" (out of range, using {})", fmt_si(used, "V")),
                Style::default().fg(Color::LightRed),
            ));
        }
        trig.push(Span::raw(format!(" @{:.0}%", s.trigger.position * 100.0)));
        parts.push(trig);
    }
    if s.display.averaging > 1 {
        parts.push(text(format!("Avg {}", s.display.averaging)));
    }
    if s.display.persistence > 0 {
        parts.push(text(format!("Pers {}", s.display.persistence)));
    }
    if s.display.interpolation == Interpolation::Linear {
        parts.push(text("Linear".into()));
    }
    if s.display.dots {
        parts.push(text("Dots".into()));
    }
    parts.push(text(format!("Cal out {}", fmt_si(s.cal_freq as f64, "Hz"))));
    let mut spans = Vec::new();
    for (i, p) in parts.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" │ "));
        }
        spans.extend(p);
    }
    f.render_widget(Paragraph::new(Line::from(spans)).style(gray), area);
}

fn draw_message_line(f: &mut TermFrame, app: &App, area: Rect) {
    let line = match (app.current_message(), &app.status) {
        (Some(m), _) => Line::styled(
            format!(" {}", m.text),
            Style::default().fg(if m.error {
                Color::LightRed
            } else {
                Color::LightGreen
            }),
        ),
        (None, Some(st)) => Line::styled(format!(" {st}"), Style::default().fg(Color::Yellow)),
        _ => Line::styled(
            " ? help · S settings · q quit · space run/stop · a autoset · ←→ time · ↑↓ volts · tab channel",
            Style::default().fg(Color::DarkGray),
        ),
    };
    f.render_widget(Paragraph::new(line), area);
}

/// First visible row of a list of `len` rows in `height` rows, so that
/// row `sel` is on screen.
fn scroll(sel: usize, len: usize, height: usize) -> usize {
    if len <= height || height == 0 {
        0
    } else {
        sel.saturating_sub(height / 2).min(len - height)
    }
}

/// Style of a list row: the cursor row is reversed in the focused column
/// and bold in the others.
fn row_style(selected: bool, focused: bool) -> Style {
    match (selected, focused) {
        (true, true) => Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
        (true, false) => Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
        _ => Style::default(),
    }
}

/// `left` and `right` on one row of `width` cells, `right` aligned right.
fn two_sided(left: &str, right: &str, width: usize) -> String {
    let lw = left.chars().count();
    let rw = right.chars().count();
    if lw + rw + 1 > width {
        let keep = width.saturating_sub(rw + 1).max(1);
        let left: String = left.chars().take(keep).collect();
        let lw = left.chars().count();
        return format!(
            "{left}{:pad$}{right}",
            "",
            pad = width.saturating_sub(lw + rw).max(1)
        );
    }
    format!("{left}{:pad$}{right}", "", pad = width - lw - rw)
}

fn draw_panel(f: &mut TermFrame, app: &App, area: Rect, pal: &Palette) {
    let p = &app.panel;
    let s = &app.scope.settings;
    let d = app.device_info();
    let current = p.current();
    // The key hints or the input line go in the bottom border.
    let block = Block::bordered()
        .title(Line::styled(
            format!(" Settings · {} ", p.breadcrumb()),
            panel_heading(pal),
        ))
        .title_bottom(panel_foot(app, current, area.width.saturating_sub(2), pal))
        .border_style(Style::default().fg(pal.axis));
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    if inner.height < 2 || inner.width < 24 {
        return;
    }
    let [cols, help] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    let cat_w = (inner.width / 4).clamp(12, 22);
    let item_w = ((inner.width - cat_w) / 2 + 4).min(46);
    let [cat_area, sep1, item_area, sep2, val_area] = Layout::horizontal([
        Constraint::Length(cat_w),
        Constraint::Length(1),
        Constraint::Length(item_w),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(cols);
    let sep_style = Style::default().fg(pal.grid);
    for sep in [sep1, sep2] {
        let lines: Vec<Line> = (0..sep.height)
            .map(|_| Line::styled("│", sep_style))
            .collect();
        f.render_widget(Paragraph::new(lines), sep);
    }
    let h = cols.height as usize;
    let groups = p.groups();

    // Categories.
    let start = scroll(p.cat, groups.len(), h);
    let lines: Vec<Line> = groups
        .iter()
        .enumerate()
        .skip(start)
        .take(h)
        .map(|(i, g)| {
            let w = cat_w as usize;
            let text = format!(" {:<w$}", g.name, w = w.saturating_sub(1));
            Line::styled(text, row_style(i == p.cat, p.focus == Focus::Categories))
        })
        .collect();
    f.render_widget(Paragraph::new(lines), cat_area);

    // Settings with their values.
    let items = groups
        .get(p.cat)
        .map(|g| g.items.clone())
        .unwrap_or_default();
    let start = scroll(p.item, items.len(), h);
    let in_items = p.focus != Focus::Categories;
    let lines: Vec<Line> = items
        .iter()
        .enumerate()
        .skip(start)
        .take(h)
        .map(|(i, (field, label))| {
            let w = item_w as usize;
            let value = field.value(s, &d);
            let text = format!(
                " {}",
                two_sided(label, &format!("{value} "), w.saturating_sub(1))
            );
            let mut style = row_style(i == p.item && in_items, p.focus == Focus::Items);
            if field.kind() == Kind::Info && !(i == p.item && in_items) {
                style = style.fg(pal.label);
            }
            Line::styled(text, style)
        })
        .collect();
    if items.is_empty() {
        f.render_widget(
            Paragraph::new(Line::styled(" no match", Style::default().fg(pal.label))),
            item_area,
        );
    } else {
        f.render_widget(Paragraph::new(lines), item_area);
    }

    // Values or help for the field under the cursor.
    let mut right: Vec<Line> = Vec::new();
    if let Some(field) = current {
        match field.kind() {
            Kind::Choice | Kind::Bool => {
                let opts = field.options(s, &d);
                let now = field.index(s);
                let cursor = if p.focus == Focus::Values {
                    p.value
                } else {
                    now
                };
                let start = scroll(cursor, opts.len(), h);
                for (i, o) in opts.iter().enumerate().skip(start).take(h) {
                    let mark = if i == now { "●" } else { " " };
                    let style = row_style(i == cursor && p.focus == Focus::Values, true);
                    let style = if i == now && p.focus != Focus::Values {
                        style.fg(pal.trace[0])
                    } else {
                        style
                    };
                    right.push(Line::styled(format!(" {mark} {o} "), style));
                }
            }
            Kind::Number => right = number_column(p, field, s, &d, pal),
            Kind::Info => {
                right.push(Line::from(format!(" {}", field.value(s, &d))));
            }
            Kind::Action => {
                right.push(Line::from(format!(" ⏎ {}", field.label())));
            }
        }
    }
    f.render_widget(Paragraph::new(right).wrap(Wrap { trim: false }), val_area);

    // Help for the field, in the colour of the heading, so it does not
    // look like one more setting row.
    let help_text = current.map_or(String::new(), |f| f.help_text(s, &d));
    f.render_widget(
        Paragraph::new(Line::styled(format!(" {help_text}"), panel_heading(pal))),
        help,
    );
}

/// Style of the panel heading and of the help line of the setting.
pub fn panel_heading(pal: &Palette) -> Style {
    Style::default().fg(pal.label).add_modifier(Modifier::BOLD)
}

/// The value column of a number setting: the value in a box, the step and
/// how to type. In the value column the box is bright; while typing it
/// holds the typed text and a cursor.
fn number_column(
    p: &crate::panel::Panel,
    field: crate::panel::Field,
    s: &Settings,
    d: &crate::panel::DeviceInfo,
    pal: &Palette,
) -> Vec<Line<'static>> {
    let here = p.focus == Focus::Values;
    let text = match (&p.input, here) {
        (Some(t), true) => format!("{t}▏"),
        _ => field.value(s, d),
    };
    let w = text.chars().count() + 2;
    let (frame, value) = if here {
        let c = Style::default().fg(pal.trace[0]);
        (c, c.add_modifier(Modifier::BOLD))
    } else {
        (Style::default().fg(pal.label), Style::default())
    };
    let bar = "─".repeat(w);
    let mut lines = vec![
        Line::styled(format!(" ╭{bar}╮"), frame),
        Line::from(vec![
            Span::styled(" │ ", frame),
            Span::styled(text, value),
            Span::styled(" │", frame),
        ]),
        Line::styled(format!(" ╰{bar}╯"), frame),
        Line::from(format!(" j k  {}", field.step_text(s))),
    ];
    let hint = Style::default().fg(pal.label);
    if here && p.input.is_some() {
        lines.push(Line::styled(" ⏎ apply · esc cancel", hint));
    } else {
        lines.push(Line::styled(" type: 500m 2k 1.5u", hint));
    }
    if let (Some(e), true) = (&p.error, here) {
        lines.push(Line::styled(
            format!(" {e}"),
            Style::default().fg(Color::LightRed),
        ));
    }
    lines
}

/// The bottom line of the panel: key hints, or the line being typed.
fn panel_foot(
    app: &App,
    current: Option<crate::panel::Field>,
    width: u16,
    pal: &Palette,
) -> Line<'static> {
    let p = &app.panel;
    let hint = Style::default().fg(Color::DarkGray);
    let foot_line = if p.input.is_some() {
        let label = current.map_or("", |f| f.label());
        Line::styled(
            format!(" typing {label} · ⏎ apply · ← h ↑ ↓ apply and move · esc cancel · ⌫ edit"),
            hint,
        )
    } else if p.on_number() && width >= 90 {
        Line::styled(
            " j/k ↑/↓ step · digits . - type a value (500m 2k 1.5u) · ⌫ edit · esc/h back · S close",
            hint,
        )
    } else if p.on_number() {
        Line::styled(" j/k step · type a value · ⌫ edit · esc back", hint)
    } else if p.searching {
        Line::from(vec![
            Span::styled(
                format!(" /{}▏", p.filter),
                Style::default()
                    .fg(pal.trace[0])
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ⏎ done · esc clear · ↑↓ move", hint),
        ])
    } else if width >= 108 {
        Line::styled(
            " j/k move · h/l out/in · esc back · space toggle · +/- step · / search · gg/G · S close",
            hint,
        )
    } else {
        Line::styled(
            " hjkl move · esc back · space toggle · +/- step · / find · S close",
            hint,
        )
    };
    foot_line
}

/// Every key, for the help screen and the README.
pub const KEYS: [(&str, &str); 47] = [
    ("q Esc", "quit (saves settings)"),
    ("? F1", "this help"),
    ("S F2", "settings panel (h j k l, / search, type numbers)"),
    ("space", "run / stop"),
    ("o", "single shot: arm the trigger once"),
    ("a", "autoset"),
    ("Tab S-Tab", "select CH1 / CH2 / MATH"),
    ("1 2 3", "CH1 / CH2 / MATH on or off"),
    ("↑ ↓", "volts/div of the selected trace"),
    ("[ ]", "move the selected trace down / up"),
    ("{ }", "move the selected trace a little"),
    ("0", "selected trace to the center"),
    ("c", "coupling DC / AC"),
    ("p", "probe x1 / x10"),
    ("i", "invert"),
    ("m", "next math function (turns MATH on)"),
    ("← →", "time/div"),
    ("t", "trigger mode: Auto / Normal / Single"),
    ("s", "trigger source CH1 / CH2"),
    ("e", "trigger edge: rising / falling / both"),
    ("+ -", "trigger level"),
    ("T", "trigger level to the middle of the signal"),
    (", .", "trigger position left / right"),
    ("f", "spectrum (FFT) view"),
    ("F", "spectrum zoom (span / 2)"),
    ("w", "FFT window"),
    ("x", "XY mode"),
    ("M", "measurement panel"),
    ("v", "hide / show the cursors (Settings › Cursors)"),
    ("h l", "move time cursor A (when on)"),
    ("H L", "move time cursor B (when on)"),
    ("j k", "move voltage cursor A (when on)"),
    ("J K", "move voltage cursor B (when on)"),
    ("A", "averaging: 1 2 4 8 16 32 64"),
    ("P", "persistence: 0 2 4 8 16 traces"),
    ("d", "dots / lines"),
    ("z", "sin(x)/x / linear interpolation"),
    ("g G", "calibration output frequency up / down"),
    ("E", "export the screen as CSV"),
    ("W", "export the screen as JSON"),
    ("R", "reset all settings"),
    ("r", "start / stop the envelope logger"),
    ("V", "log view (strip chart of the log)"),
    ("Ctrl-C", "quit"),
    ("", ""),
    ("", "The calibration output pins give a"),
    ("", "2 Vpp square wave at this frequency."),
];

fn draw_help(f: &mut TermFrame, area: Rect, keys: &[(&str, &str)]) {
    let rows: Vec<Line> = keys
        .iter()
        .map(|(k, d)| {
            Line::from(vec![
                Span::styled(format!(" {k:<10}"), Style::default().fg(Color::LightYellow)),
                Span::raw(*d),
            ])
        })
        .collect();
    let half = rows.len().div_ceil(2);
    let w = 104.min(area.width);
    let h = (half as u16 + 3).min(area.height);
    let popup = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, popup);
    let block = Block::bordered().title(" Keys · any key closes ");
    let inner = block.inner(popup);
    f.render_widget(block, popup);
    if inner.width >= 80 {
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                .areas(inner);
        f.render_widget(Paragraph::new(rows[..half].to_vec()), left);
        f.render_widget(Paragraph::new(rows[half..].to_vec()), right);
    } else {
        f.render_widget(Paragraph::new(rows).wrap(Wrap { trim: false }), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_height_rules() {
        // As big as needed when there is room.
        assert_eq!(panel_height(15, 40), 15);
        assert_eq!(panel_height(15, 60), 15);
        assert_eq!(panel_height(8, 40), 8);
        // A short screen keeps a small plot, then the panel shrinks.
        assert_eq!(panel_height(15, 24), 10);
        assert_eq!(panel_height(15, 20), PANEL_MIN_ROWS);
        // A tiny screen still leaves room for the plot and status lines.
        assert_eq!(panel_height(15, 15), 6);
    }

    #[test]
    fn split_height_is_a_whole_plot() {
        for h in [30u16, 37, 50, 60] {
            let t = split_height(h);
            assert!(t <= h * 55 / 100, "{h}: {t}");
            let g = Geometry::fit(Rect::new(0, 0, 100, t - 2), 3, 1).unwrap();
            // No spare rows inside the time view.
            assert_eq!(g.plot.height + 2, t - 2, "{h}");
        }
    }

    #[test]
    fn compact_labels_round_small_offsets() {
        assert_eq!(compact(0.00202, 0.01, "V"), "0V");
        assert_eq!(compact(-0.998, 0.01, "V"), "-1V");
        assert_eq!(compact(-0.0025, 1e-5, "s"), "-2.5ms");
        assert_eq!(compact(1.5e-3, 0.0, "s"), "1.5ms");
    }

    #[test]
    fn two_sided_rows() {
        assert_eq!(two_sided("Probe", "x1", 10), "Probe   x1");
        assert_eq!(two_sided("Long label here", "on", 10).chars().count(), 10);
    }

    #[test]
    fn scroll_keeps_the_row_visible() {
        assert_eq!(scroll(0, 5, 10), 0);
        assert_eq!(scroll(30, 38, 10), 25);
        assert_eq!(scroll(37, 38, 10), 28);
        assert_eq!(scroll(12, 38, 10), 7);
    }
}
