//! Drawing the screen with ratatui.

use std::sync::OnceLock;

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols::Marker,
    text::{Line, Span},
    widgets::{
        canvas::{Canvas, Context, Line as CLine, Points},
        Block, Clear, Paragraph, Wrap,
    },
    Frame as TermFrame,
};

use crate::app::App;
use crate::dsp::fft::{spectrum, Spectrum};
use crate::dsp::measure::Measurements;
use crate::dsp::sinc_interpolate;
use crate::scope::{Frame, TrigState, MATH};
use crate::settings::*;
use crate::si::{fmt_opt, fmt_percent, fmt_si};

/// Trace colors: CH1, CH2, MATH.
pub const COLORS: [Color; 3] = [Color::LightYellow, Color::LightCyan, Color::LightMagenta];
/// Dimmed trace colors, for persistence.
const DIM: [Color; 3] = [Color::Yellow, Color::Cyan, Color::Magenta];
const NAMES: [&str; 3] = ["CH1", "CH2", "MATH"];

/// Spectrum scale: top line and dB per division.
pub const SPECTRUM_TOP_DBV: f64 = 20.0;
pub const SPECTRUM_DB_PER_DIV: f64 = 15.0;

pub fn draw(f: &mut TermFrame, app: &App) {
    let [main, status1, status2, status3] = Layout::vertical([
        Constraint::Min(6),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(f.area());
    let s = &app.scope.settings;

    // Measurement panel on the right, if there is room.
    let panel_w = measurement_width(app, main.width);
    let [view, panel] =
        Layout::horizontal([Constraint::Min(20), Constraint::Length(panel_w)]).areas(main);
    if panel_w > 0 {
        draw_measurements(f, app, panel);
    }
    if s.display.spectrum && !s.display.xy {
        let [time, spec] =
            Layout::vertical([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(view);
        draw_time(f, app, time);
        draw_spectrum(f, app, spec);
    } else if s.display.xy {
        draw_xy(f, app, view);
    } else {
        draw_time(f, app, view);
    }
    draw_channel_line(f, app, status1);
    draw_time_line(f, app, status2);
    draw_message_line(f, app, status3);
    if app.help {
        draw_help(f, f.area());
    }
}

/// Traces in drawing order: the selected trace last, so it is on top.
fn draw_order(s: &Settings) -> Vec<usize> {
    let mut order: Vec<usize> = (0..3).filter(|&t| t != s.selected).collect();
    order.push(s.selected);
    order
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

fn grid() -> &'static [(f64, f64)] {
    static GRID: OnceLock<Vec<(f64, f64)>> = OnceLock::new();
    GRID.get_or_init(|| {
        let mut points = Vec::new();
        let (half_h, half_v) = (H_DIVS / 2.0, V_DIVS / 2.0);
        for i in 0..=H_DIVS as i32 {
            let x = i as f64 - half_h;
            for j in 0..=(V_DIVS * 5.0) as i32 {
                points.push((x, j as f64 / 5.0 - half_v));
            }
        }
        for j in 0..=V_DIVS as i32 {
            let y = j as f64 - half_v;
            for i in 0..=(H_DIVS * 5.0) as i32 {
                points.push((i as f64 / 5.0 - half_h, y));
            }
        }
        points
    })
}

/// Screen points of a trace, in divisions. Many samples are cut down to a
/// min/max pair per dot column; few samples get sin(x)/x points between.
pub fn trace_points(frame: &Frame, t: usize, s: &Settings, dots_x: usize) -> Vec<(f64, f64)> {
    let Some(tr) = frame.traces[t].as_ref() else {
        return Vec::new();
    };
    let (scale, offset) = trace_scale(s, t);
    let half_h = H_DIVS / 2.0;
    let half_v = V_DIVS / 2.0;
    let y = |v: f64| (v / scale + offset).clamp(-half_v, half_v);
    let r = frame.visible(s, tr.len());
    let n = r.len();
    let dots_x = dots_x.max(2);
    if n == 0 {
        return Vec::new();
    }
    if n > 2 * dots_x {
        let mut cols: Vec<Option<(f64, f64)>> = vec![None; dots_x];
        for i in r {
            let v = tr[i];
            if !v.is_finite() {
                continue;
            }
            let x = frame.x(i as f64, s);
            let px = (((x + half_h) / H_DIVS) * dots_x as f64) as usize;
            let c = &mut cols[px.min(dots_x - 1)];
            *c = Some(c.map_or((v, v), |(lo, hi)| (lo.min(v), hi.max(v))));
        }
        let mut out = Vec::with_capacity(dots_x * 2);
        for (px, c) in cols.iter().enumerate() {
            if let Some((lo, hi)) = c {
                let x = (px as f64 + 0.5) / dots_x as f64 * H_DIVS - half_h;
                out.push((x, y(*lo)));
                out.push((x, y(*hi)));
            }
        }
        return out;
    }
    if s.display.interpolation == Interpolation::Sinc && n * 4 < dots_x {
        let factor = (dots_x / n).clamp(2, 32);
        let lo = r.start.saturating_sub(6);
        let hi = (r.end + 6).min(tr.len());
        let fine = sinc_interpolate(&tr[lo..hi], factor);
        return fine
            .iter()
            .enumerate()
            .filter_map(|(j, v)| {
                let x = frame.x(lo as f64 + j as f64 / factor as f64, s);
                (x >= -half_h - 1e-9 && x <= half_h + 1e-9 && v.is_finite()).then(|| (x, y(*v)))
            })
            .collect();
    }
    r.filter(|&i| tr[i].is_finite())
        .map(|i| (frame.x(i as f64, s), y(tr[i])))
        .collect()
}

fn draw_points(ctx: &mut Context, points: &[(f64, f64)], color: Color, dots: bool) {
    if dots || points.len() < 2 {
        ctx.draw(&Points {
            coords: points,
            color,
        });
        return;
    }
    for w in points.windows(2) {
        ctx.draw(&CLine {
            x1: w[0].0,
            y1: w[0].1,
            x2: w[1].0,
            y2: w[1].1,
            color,
        });
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
    spans.push(Span::raw(" "));
    Line::from(spans)
}

fn draw_time(f: &mut TermFrame, app: &App, area: Rect) {
    let sc = &app.scope;
    let s = &sc.settings;
    let block = Block::bordered().title(title(app));
    let inner = block.inner(area);
    let dots_x = inner.width as usize * 2;
    let half_h = H_DIVS / 2.0;
    let half_v = V_DIVS / 2.0;
    let order = draw_order(s);
    let mut layers: Vec<(Vec<(f64, f64)>, Color)> = Vec::new();
    // Old frames first, dimmed.
    for old in &sc.history {
        for &t in &order {
            if trace_enabled(s, t) {
                layers.push((trace_points(old, t, s, dots_x), DIM[t]));
            }
        }
    }
    if let Some(frame) = &sc.frame {
        for &t in &order {
            if trace_enabled(s, t) {
                layers.push((trace_points(frame, t, s, dots_x), COLORS[t]));
            }
        }
    }
    let dots = s.display.dots;
    let canvas = Canvas::default()
        .block(block)
        .marker(Marker::Braille)
        .x_bounds([-half_h, half_h])
        .y_bounds([-half_v, half_v])
        .paint(|ctx| {
            ctx.draw(&Points {
                coords: grid(),
                color: Color::DarkGray,
            });
            draw_cursors(ctx, s);
            ctx.layer();
            for (points, color) in &layers {
                draw_points(ctx, points, *color, dots);
            }
            draw_markers(ctx, app);
            if sc.frame.is_none() {
                let text = app
                    .status
                    .clone()
                    .unwrap_or_else(|| "Waiting for data...".to_string());
                ctx.print(
                    -half_h + 0.3,
                    0.0,
                    Line::styled(text, Style::default().fg(Color::Gray)),
                );
            }
        });
    f.render_widget(canvas, area);
}

fn draw_cursors(ctx: &mut Context, s: &Settings) {
    let (half_h, half_v) = (H_DIVS / 2.0, V_DIVS / 2.0);
    let color = Color::Gray;
    match s.cursors.mode {
        CursorMode::Off => {}
        CursorMode::Time => {
            for &x in &s.cursors.t {
                dashed(ctx, (x, -half_v), (x, half_v), color);
            }
        }
        CursorMode::Volt => {
            for &y in &s.cursors.v {
                dashed(ctx, (-half_h, y), (half_h, y), color);
            }
        }
    }
}

fn dashed(ctx: &mut Context, a: (f64, f64), b: (f64, f64), color: Color) {
    let steps = 80;
    let coords: Vec<(f64, f64)> = (0..=steps)
        .filter(|i| i % 4 < 2)
        .map(|i| {
            let k = i as f64 / steps as f64;
            (a.0 + (b.0 - a.0) * k, a.1 + (b.1 - a.1) * k)
        })
        .collect();
    ctx.draw(&Points {
        coords: &coords,
        color,
    });
}

/// Ground markers, trigger level and trigger position, cursor labels.
fn draw_markers(ctx: &mut Context, app: &App) {
    let sc = &app.scope;
    let s = &sc.settings;
    let (half_h, half_v) = (H_DIVS / 2.0, V_DIVS / 2.0);
    for t in 0..3 {
        if !trace_enabled(s, t) {
            continue;
        }
        let (_, offset) = trace_scale(s, t);
        let label = ["1", "2", "M"][t];
        ctx.print(
            -half_h,
            offset.clamp(-half_v + 0.2, half_v),
            Line::styled(format!("{label}▸"), Style::default().fg(COLORS[t])),
        );
    }
    let roll = sc.frame.as_ref().is_some_and(|f| f.roll) || s.roll();
    if !roll && !s.display.xy {
        let src = s.trigger.source;
        let (scale, offset) = trace_scale(s, src);
        let y = (s.trigger.level / scale + offset).clamp(-half_v + 0.2, half_v);
        ctx.print(
            half_h - 0.25,
            y,
            Line::styled("◂", Style::default().fg(COLORS[src])),
        );
        let x = -half_h + s.trigger.position * H_DIVS;
        ctx.print(
            (x - 0.1).clamp(-half_h, half_h - 0.2),
            half_v,
            Line::styled("▾", Style::default().fg(Color::LightRed)),
        );
    }
    match s.cursors.mode {
        CursorMode::Time => {
            for (i, &x) in s.cursors.t.iter().enumerate() {
                ctx.print(
                    (x + 0.05).min(half_h - 0.2),
                    half_v - 0.3,
                    Line::styled(["A", "B"][i], Style::default().fg(Color::White)),
                );
            }
        }
        CursorMode::Volt => {
            for (i, &y) in s.cursors.v.iter().enumerate() {
                ctx.print(
                    half_h - 0.6,
                    (y + 0.2).min(half_v),
                    Line::styled(["A", "B"][i], Style::default().fg(Color::White)),
                );
            }
        }
        CursorMode::Off => {}
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

fn draw_spectrum(f: &mut TermFrame, app: &App, area: Rect) {
    let s = &app.scope.settings;
    let rate = app.scope.frame.as_ref().map_or(1.0, |f| f.rate);
    let span = rate / 2.0 / 2f64.powi(s.display.spectrum_zoom as i32);
    let bottom = SPECTRUM_TOP_DBV - SPECTRUM_DB_PER_DIV * V_DIVS;
    let main = main_trace(app);
    let mut title = format!(
        " Spectrum · {} · {} dB/div · 0..{} ",
        s.display.window.name(),
        SPECTRUM_DB_PER_DIV,
        fmt_si(span, "Hz")
    );
    let block_inner = Block::bordered().inner(area);
    let dots_x = block_inner.width as usize * 2;
    let mut layers = Vec::new();
    for t in draw_order(s) {
        if !trace_enabled(s, t) {
            continue;
        }
        if let Some(sp) = trace_spectrum(app, t) {
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
            layers.push((spectrum_points(&sp, span, dots_x), COLORS[t]));
        }
    }
    let grid: Vec<(f64, f64)> = grid()
        .iter()
        .map(|&(x, y)| {
            (
                (x + H_DIVS / 2.0) / H_DIVS * span,
                SPECTRUM_TOP_DBV - (V_DIVS / 2.0 - y) * SPECTRUM_DB_PER_DIV,
            )
        })
        .collect();
    let canvas = Canvas::default()
        .block(Block::bordered().title(title))
        .marker(Marker::Braille)
        .x_bounds([0.0, span])
        .y_bounds([bottom, SPECTRUM_TOP_DBV])
        .paint(|ctx| {
            ctx.draw(&Points {
                coords: &grid,
                color: Color::DarkGray,
            });
            ctx.layer();
            for (points, color) in &layers {
                draw_points(ctx, points, *color, false);
            }
            ctx.print(
                0.0,
                SPECTRUM_TOP_DBV,
                Line::styled(
                    format!("{SPECTRUM_TOP_DBV:+.0} dBV"),
                    Style::default().fg(Color::Gray),
                ),
            );
        });
    f.render_widget(canvas, area);
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

fn draw_xy(f: &mut TermFrame, app: &App, area: Rect) {
    let points = xy_points(app);
    let mut t = title(app);
    t.spans.push(Span::raw("· XY: CH1 → x, CH2 → y "));
    let dots = app.scope.settings.display.dots;
    let canvas = Canvas::default()
        .block(Block::bordered().title(t))
        .marker(Marker::Braille)
        .x_bounds([-H_DIVS / 2.0, H_DIVS / 2.0])
        .y_bounds([-V_DIVS / 2.0, V_DIVS / 2.0])
        .paint(|ctx| {
            ctx.draw(&Points {
                coords: grid(),
                color: Color::DarkGray,
            });
            ctx.layer();
            draw_points(ctx, &points, Color::LightGreen, dots);
        });
    f.render_widget(canvas, area);
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

const LABEL_W: u16 = 7;
const VALUE_W: u16 = 11;

/// Traces shown in the panel. Narrow screens show only the main trace.
fn panel_traces(app: &App, width: u16) -> Vec<usize> {
    let s = &app.scope.settings;
    let all: Vec<usize> = (0..3).filter(|&t| trace_enabled(s, t)).collect();
    let full = LABEL_W + VALUE_W * all.len() as u16 + 2;
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
    let w = LABEL_W + VALUE_W * n + 2;
    if n == 0 || width < w + 40 {
        0
    } else {
        w
    }
}

fn draw_measurements(f: &mut TermFrame, app: &App, area: Rect) {
    let s = &app.scope.settings;
    let traces = panel_traces(app, area.width + 60);
    let measured: Vec<Option<Measurements>> =
        traces.iter().map(|&t| app.scope.measure(t)).collect();
    let mut lines = Vec::new();
    let mut head = vec![Span::raw(format!("{:<w$}", "", w = LABEL_W as usize))];
    for &t in &traces {
        head.push(Span::styled(
            format!("{:>w$}", NAMES[t], w = VALUE_W as usize),
            Style::default().fg(COLORS[t]).add_modifier(Modifier::BOLD),
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
            format!("{label:<w$}", w = LABEL_W as usize),
            Style::default().fg(Color::Gray),
        )];
        for (k, &t) in traces.iter().enumerate() {
            let text = rows[k].get(r).map_or("--".to_string(), |(_, v)| v.clone());
            spans.push(Span::styled(
                format!("{text:>w$}", w = VALUE_W as usize),
                Style::default().fg(COLORS[t]),
            ));
        }
        lines.push(Line::from(spans));
    }
    f.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" Measure ")),
        area,
    );
}

fn channel_span(app: &App, t: usize) -> Span<'static> {
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
    let mut style = Style::default().fg(COLORS[t]);
    if t == s.selected {
        style = style.add_modifier(Modifier::REVERSED);
    }
    if !trace_enabled(s, t) {
        style = style.add_modifier(Modifier::DIM | Modifier::CROSSED_OUT);
    }
    Span::styled(text, style)
}

fn draw_channel_line(f: &mut TermFrame, app: &App, area: Rect) {
    let mut spans = Vec::new();
    for t in 0..3 {
        spans.push(channel_span(app, t));
        spans.push(Span::raw(" "));
    }
    if let Some(text) = cursor_text(app) {
        spans.push(Span::styled(text, Style::default().fg(Color::White)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Cursor readout, like `ΔT 1 ms  1/ΔT 1 kHz`.
pub fn cursor_text(app: &App) -> Option<String> {
    let s = &app.scope.settings;
    match s.cursors.mode {
        CursorMode::Off => None,
        CursorMode::Time => {
            let tdiv = s.time_per_div();
            let (a, b) = (s.cursors.t[0] * tdiv, s.cursors.t[1] * tdiv);
            let d = (b - a).abs();
            let inv = if d > 0.0 {
                fmt_si(1.0 / d, "Hz")
            } else {
                "--".into()
            };
            Some(format!(
                "│ A {} B {} ΔT {} 1/ΔT {}",
                fmt_si(a, "s"),
                fmt_si(b, "s"),
                fmt_si(d, "s"),
                inv
            ))
        }
        CursorMode::Volt => {
            let t = main_trace(app).unwrap_or(0);
            let (scale, offset) = trace_scale(s, t);
            let unit = trace_unit(s, t);
            let v = |y: f64| (y - offset) * scale;
            let (a, b) = (v(s.cursors.v[0]), v(s.cursors.v[1]));
            Some(format!(
                "│ {} A {} B {} ΔV {}",
                NAMES[t],
                fmt_si(a, unit),
                fmt_si(b, unit),
                fmt_si((b - a).abs(), unit)
            ))
        }
    }
}

fn draw_time_line(f: &mut TermFrame, app: &App, area: Rect) {
    let s = &app.scope.settings;
    let mut parts = vec![format!(" {}/div", fmt_si(s.time_per_div(), "s"))];
    if let Some(hw) = &app.scope.hw {
        parts.push(fmt_si(hw.rate.rate, "S/s"));
    }
    if s.roll() {
        parts.push("Roll".into());
    } else {
        let src = s.trigger.source;
        parts.push(format!(
            "Trig {} {} {} {} @{:.0}%",
            s.trigger.mode.name(),
            NAMES[src],
            s.trigger.slope.symbol(),
            fmt_si(s.trigger.level, "V"),
            s.trigger.position * 100.0
        ));
    }
    if s.display.averaging > 1 {
        parts.push(format!("Avg {}", s.display.averaging));
    }
    if s.display.persistence > 0 {
        parts.push(format!("Pers {}", s.display.persistence));
    }
    if s.display.interpolation == Interpolation::Linear {
        parts.push("Linear".into());
    }
    if s.display.dots {
        parts.push("Dots".into());
    }
    parts.push(format!("Cal out {}", fmt_si(s.cal_freq as f64, "Hz")));
    f.render_widget(
        Paragraph::new(parts.join(" │ ")).style(Style::default().fg(Color::Gray)),
        area,
    );
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
            " ? help · q quit · space run/stop · a autoset · ←→ time · ↑↓ volts · tab channel",
            Style::default().fg(Color::DarkGray),
        ),
    };
    f.render_widget(Paragraph::new(line), area);
}

/// Every key, for the help screen and the README.
pub const KEYS: [(&str, &str); 44] = [
    ("q Esc", "quit (saves settings)"),
    ("? F1", "this help"),
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
    ("v", "cursors: off / time / voltage"),
    ("h l", "move time cursor A"),
    ("H L", "move time cursor B"),
    ("j k", "move voltage cursor A"),
    ("J K", "move voltage cursor B"),
    ("A", "averaging: 1 2 4 8 16 32 64"),
    ("P", "persistence: 0 2 4 8 16 traces"),
    ("d", "dots / lines"),
    ("z", "sin(x)/x / linear interpolation"),
    ("g G", "calibration output frequency up / down"),
    ("E", "export the screen as CSV"),
    ("W", "export the screen as JSON"),
    ("R", "reset all settings"),
    ("Ctrl-C", "quit"),
    ("", ""),
    ("", "The calibration output pins give a"),
    ("", "2 Vpp square wave at this frequency."),
];

fn draw_help(f: &mut TermFrame, area: Rect) {
    let rows: Vec<Line> = KEYS
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
