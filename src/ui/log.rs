//! Drawing the log view (strip chart of a log) and the REC status line.

use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
    Frame as TermFrame,
};

use super::{
    bottom_labels, channel_span, compact, draw_order, grid, label_width, labels, layer_of,
    left_labels, put, trace_scale, NAMES,
};
use crate::app::App;
use crate::dsp::sinc_interpolate;
use crate::logger::{self as lg, writer::LogState};
use crate::plot::{self, Geometry, Plot, MARKER_W};
use crate::settings::*;
use crate::si::fmt_si;
use crate::theme::Palette;

/// Value labels of the log view: volts of the selected channel.
fn log_labels(s: &Settings) -> Vec<String> {
    let sel = s.selected.min(1);
    let (scale, offset) = trace_scale(s, sel);
    labels(|j| {
        compact(
            (V_DIVS / 2.0 - j as f64 - offset) * scale,
            scale / 100.0,
            "V",
        )
    })
}

/// Seconds since 1970 as `HH:MM:SS` (UTC), with `digits` decimals, and
/// with the date (`MM-DD`) when `date`.
pub fn clock_text(secs: f64, digits: usize, date: bool) -> String {
    let ns = (secs * 1e9).round() as i64;
    let iso = lg::csv::iso_utc(ns, digits);
    // 2026-11-13T12:34:56.123Z
    let time = &iso[11..iso.len() - 1];
    if date {
        format!("{} {}", &iso[5..10], time)
    } else {
        time.to_string()
    }
}

/// Decimals that show a time step of `div` seconds. A step that is not
/// a round number (zoomed in past one log step per dot column, like
/// 1.1 ms or 2.75 µs) gets up to two more, so the labels keep even
/// steps.
fn digits_for(div: f64) -> usize {
    let d = match div {
        d if d >= 0.999 => 0,
        d if d >= 0.0999 => 1,
        d if d >= 0.00999 => 2,
        d if d >= 0.000999 => 3,
        _ => 6,
    };
    (d..=d + 2)
        .find(|&n| {
            let x = div * 10f64.powi(n as i32);
            (x - x.round()).abs() < 0.01 * x.max(1.0)
        })
        .unwrap_or(d + 2)
}

/// A time on the time axis: step `i` of the open log.
fn time_label(app: &App, i: f64, div: f64, date: bool) -> String {
    let Some(v) = &app.log_view else {
        return String::new();
    };
    let h = &v.file.header;
    let t = v.file.time_of(i);
    match app.scope.settings.logger.time_axis {
        TimeAxis::Clock => clock_text(h.start_secs() + t, digits_for(div), date),
        TimeAxis::Seconds => format!("{:.*}s", digits_for(div), t),
    }
}

pub fn draw_log(f: &mut TermFrame, app: &App, area: Rect, pal: &Palette) {
    let s = &app.scope.settings;
    let Some(v) = &app.log_view else {
        let block = Block::bordered().title(" Log ");
        let inner = block.inner(area);
        f.render_widget(block, area);
        let text = if app.recording() {
            "Waiting for data from the scope..."
        } else {
            "No log open. Press r to record, or pick one in the settings: S › Logger › Open log"
        };
        f.render_widget(
            Paragraph::new(text).style(Style::default().fg(Color::Gray)),
            inner,
        );
        return;
    };
    let h = &v.file.header;
    let name = v
        .file
        .path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let mut title = vec![
        Span::raw(" Log · "),
        Span::styled(name, Style::default().fg(Color::White)),
        Span::raw(format!(
            " · {} · step {} ",
            h.device,
            lg::fmt_step(h.step())
        )),
    ];
    if v.live {
        title.push(Span::styled(
            " ● REC ",
            Style::default().fg(Color::White).bg(Color::Red),
        ));
        title.push(Span::raw(" "));
    }
    let p = v.summary_progress();
    if p < 1.0 {
        title.push(Span::styled(
            format!(" summary {:.0}% ", p * 100.0),
            Style::default().fg(Color::Yellow),
        ));
    }
    let block = Block::bordered().title(Line::from(title));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let volts = log_labels(s);
    let Some(g) = Geometry::fit(inner, label_width(&volts), 1) else {
        return;
    };
    let mut plot = Plot::new(g.cx, g.ry);
    grid(&mut plot, s, pal);
    let w = plot.dots_w();
    v.width.set(w);
    let start = v.left(w);
    // Dot columns per step: above 1 the log is zoomed in past one step
    // per dot column, and `cols` has one entry per step.
    let k = v.k() as usize;
    let cols = v.columns(w).unwrap_or_default();
    let first = v.first_entry(w);
    let per_div = 2 * g.cx;
    let x_of = |dc: f64| dc / per_div as f64 - H_DIVS / 2.0;
    let half_v = V_DIVS / 2.0;

    // Gaps: a dashed red line in each column with lost data.
    let end = start + v.span(w);
    for (a, b) in v.file.gaps() {
        if b < start || a >= end {
            continue;
        }
        let c0 = v.col_of(w, a.max(start) as f64).floor() as u64;
        let c1 = (v.col_of(w, b.max(a + 1) as f64).ceil() as u64).saturating_sub(1);
        let mut last = usize::MAX;
        for dc in c0..=c1.min(w as u64 - 1) {
            let cell = dc as usize / 2;
            if cell == last {
                continue;
            }
            last = cell;
            for row in 0..plot.rows {
                plot.put_glyph(cell, row, '┊', plot::LAYER_CURSOR, pal.trigger);
            }
        }
    }

    // Cursors: A and B in the style and colour of time cursors A and B.
    let cursor_x: Vec<Option<f64>> = v
        .cur
        .iter()
        .map(|&c| {
            // Zoomed in past one step per dot column: the middle of the
            // step, where its mean point is.
            let at = if k > 1 { c as f64 + 0.5 } else { c as f64 };
            let dc = v.col_of(w, at);
            (v.cursors && dc >= 0.0 && dc <= w as f64).then(|| x_of(dc))
        })
        .collect();
    for (i, x) in cursor_x.iter().enumerate() {
        if let Some(x) = x {
            let c = Cursor {
                position: *x,
                ..*s.cursors.get(i.min(1))
            };
            super::draw_cursor_line(&mut plot, &c, true, pal);
        }
    }

    // Envelopes in the trace mode and interpolation of the scope view:
    // the mean as a line, the min..max area, or the line over the muted
    // area.
    let mode = s.display.trace_mode;
    let dots = s.display.interpolation == Interpolation::Samples;
    // Dot column of entry `j` (fractional between entries): its column,
    // or (zoomed in past one step per dot column) the middle of its
    // step, where its mean point is. The area goes through the same
    // points, so the line stays inside it.
    let dc_of = |j: f64| {
        if k > 1 {
            v.col_of(w, first as f64 + j + 0.5)
        } else {
            j
        }
    };
    // sin(x)/x only when the screen has few steps, as in the scope view.
    let factor = if k > 1 {
        super::sinc_factor(s, v.span(w) as usize, w)
    } else {
        None
    };
    for t in draw_order(s) {
        if t > 1 || !h.channels[t].enabled || !s.channels[t].enabled {
            continue;
        }
        let style = s.display.trace_style[t];
        let (scale, offset) = trace_scale(s, t);
        let yd = |x: f64| (x / scale + offset).clamp(-half_v, half_v);
        let layer = layer_of(s, t);
        let band_layer = plot::LAYER_BAND + if t == s.selected { 3 } else { 2 - t as u8 };
        super::trace_layer_modifier(&mut plot, pal, t, &[layer, band_layer]);
        let runs = data_runs(&cols, t);
        // Single samples (raw logs, zoomed in) have no area: the peak is
        // the line through the samples.
        let band = mode.peak() && !(k > 1 && h.single());
        if mode.line() || !band {
            // The mean: a point per entry, joined, or the sin(x)/x curve
            // through them. A gap breaks the line.
            let mut pts: Vec<(f64, f64)> = Vec::new();
            for r in &runs {
                if !pts.is_empty() {
                    pts.push((f64::NAN, f64::NAN));
                }
                let means: Vec<f64> = r.clone().map(|j| cols[j][t].mean as f64).collect();
                match factor.filter(|_| !dots) {
                    Some(f) => {
                        let fine = sinc_interpolate(&means, f);
                        pts.extend(fine.iter().enumerate().map(|(i, m)| {
                            let j = r.start as f64 + i as f64 / f as f64;
                            (x_of(dc_of(j)), yd(*m))
                        }));
                    }
                    None => pts.extend(
                        r.clone()
                            .zip(&means)
                            .map(|(j, m)| (x_of(dc_of(j as f64)), yd(*m))),
                    ),
                }
            }
            plot.styled(&pts, style, dots, layer, pal.line[t]);
        }
        if !band {
            continue;
        }
        // The area per dot column, through the same points as the mean,
        // straight between them (as in the scope view).
        let mut ranges: Vec<Option<(f64, f64)>> = vec![None; w];
        for r in &runs {
            let pts: Vec<(f64, f64, f64)> = r
                .clone()
                .map(|j| {
                    let a = cols[j][t];
                    (dc_of(j as f64), yd(a.max as f64), yd(a.min as f64))
                })
                .collect();
            super::band_columns(&mut ranges, &pts);
        }
        match mode {
            TraceMode::Peak => plot.band(&ranges, style, layer, pal.line[t]),
            // Under the lines: the line owns its cells and looks the same
            // as the line alone; the muted area fills around it.
            _ => plot.band(&ranges, style, band_layer, pal.muted(t)),
        }
    }

    let buf = f.buffer_mut();
    plot.render(buf, g.plot);
    let sel = s.selected.min(1);
    left_labels(buf, &g, &volts, Style::default().fg(pal.trace[sel]));
    // Time labels; the date too when the screen spans two days.
    let div = per_div as f64 * v.steps_per_col(w) * h.step();
    let day = |i: u64| ((h.start_secs() + v.file.time_of(i as f64)) / 86_400.0).floor();
    let date = day(start) != day(end);
    bottom_labels(
        buf,
        &g,
        |n| time_label(app, v.step_at(w, (n * per_div) as f64), div, date),
        Style::default().fg(pal.label),
    );
    if g.margins.left >= MARKER_W {
        for t in draw_order(s) {
            if t > 1 || !h.channels[t].enabled || !s.channels[t].enabled {
                continue;
            }
            let (_, off) = trace_scale(s, t);
            let y = g.plot.y + plot.row_of(off.clamp(-half_v, half_v)) as u16;
            put(
                buf,
                g.plot.x - MARKER_W,
                y,
                ["1▸", "2▸"][t],
                Style::default().fg(pal.trace[t]),
                g.plot.x,
            );
        }
    }
    let right_x = g.plot.x + g.plot.width;
    for (i, x) in cursor_x.iter().enumerate() {
        let Some(x) = x else { continue };
        let name = ["A", "B"][i];
        let c = s.cursors.get(i.min(1));
        let style = Style::default().fg(super::cursor_colors(c, pal).1);
        if c.style != CursorStyle::Fine {
            let y = g.plot.y + plot.row_of(half_v - 0.3) as u16;
            let cx = g.plot.x + plot.col_of((x + 0.05).min(H_DIVS / 2.0 - 0.2)) as u16;
            put(buf, cx, y, name, style, right_x);
        } else if g.margins.top >= 1 {
            let cx = g.plot.x + plot.col_of(*x) as u16 + 1;
            put(buf, cx, g.plot.y - 1, name, style, right_x);
        }
    }
}

/// Runs of entries of channel `t` with data, in order. A gap (an entry
/// with no data) ends a run.
fn data_runs(cols: &[lg::pyramid::Aggs], t: usize) -> Vec<std::ops::Range<usize>> {
    let mut runs = Vec::new();
    let mut start = None;
    for (j, a) in cols.iter().enumerate() {
        match (a[t].is_empty(), start) {
            (false, None) => start = Some(j),
            (true, Some(s)) => {
                runs.push(s..j);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        runs.push(s..cols.len());
    }
    runs
}

/// The REC status: `● REC  412 MB / 1 GB   0:41:10, about 0:58:40 left
///  free 120 GB`. Red when the disk would fill before the limit.
pub fn rec_status(app: &App) -> Option<Line<'static>> {
    let st = app.rec.as_ref()?.lock().unwrap().clone();
    let rec = Span::styled(" ● REC ", Style::default().fg(Color::White).bg(Color::Red));
    let text = match st.state {
        LogState::Done(_) => return None,
        LogState::Waiting => {
            return Some(Line::from(vec![
                rec,
                Span::raw(" waiting for data from the scope"),
            ]))
        }
        LogState::Recording => {
            let elapsed = st.started.elapsed().as_secs_f64();
            // Bytes per second of this log, from its step and channels.
            let rate = st
                .header
                .as_ref()
                .map_or(1.0, |h| h.record_bytes() as f64 / h.step());
            let mut t = format!(" {}", lg::fmt_bytes(st.bytes));
            if st.limit > 0 {
                let left = st.limit.saturating_sub(st.bytes) as f64 / rate;
                t.push_str(&format!(
                    " / {}   {}, about {} left",
                    lg::fmt_bytes(st.limit),
                    lg::fmt_hms(elapsed),
                    lg::fmt_hms(left)
                ));
            } else {
                t.push_str(&format!("   {}, no limit", lg::fmt_hms(elapsed)));
            }
            t
        }
    };
    let mut spans = vec![rec, Span::raw(text)];
    if let Some(free) = app.log_free {
        let need = if st.limit > 0 {
            st.limit.saturating_sub(st.bytes)
        } else {
            u64::MAX
        };
        let short = free < need;
        let rate = st
            .header
            .as_ref()
            .map_or(1.0, |h| h.record_bytes() as f64 / h.step());
        let mut text = format!("   free {}", lg::fmt_bytes(free));
        if short {
            text.push_str(&format!(
                ", disk full in about {}",
                lg::fmt_hms(free as f64 / rate)
            ));
        }
        let style = if short && st.limit > 0 {
            Style::default().fg(Color::Black).bg(Color::Yellow)
        } else {
            Style::default().fg(Color::Gray)
        };
        spans.push(Span::styled(text, style));
    }
    if st.gaps > 0 {
        spans.push(Span::styled(
            format!("   {} gap{}", st.gaps, if st.gaps == 1 { "" } else { "s" }),
            Style::default().fg(Color::LightRed),
        ));
    }
    if st.clipped {
        spans.push(Span::styled(
            "  CLIP",
            Style::default().fg(Color::White).bg(Color::Red),
        ));
    }
    Some(Line::from(spans))
}

/// Cursor readout of the log view: times, ΔT, and min/max/mean of each
/// channel between A and B.
pub fn log_cursor_text(app: &App) -> Option<String> {
    let v = app.log_view.as_ref().filter(|v| v.cursors)?;
    let r = v.cursor_range();
    let step = v.file.header.step();
    let div = step * v.steps_per_col(v.width.get()) * 20.0;
    let [a, b] = v.cur;
    let mut t = format!(
        "│ A {} B {} ΔT {}",
        time_label(app, a as f64, div, false),
        time_label(app, b as f64, div, false),
        fmt_si((r.end - r.start) as f64 * step, "s")
    );
    if let Ok(aggs) = v.cursor_aggs() {
        for c in v.file.header.logged() {
            let x = aggs[c];
            if x.is_empty() {
                t.push_str(&format!(" │ {} no data", NAMES[c]));
            } else {
                t.push_str(&format!(
                    " │ {} min {} max {} mean {}",
                    NAMES[c],
                    fmt_si(x.min as f64, "V"),
                    fmt_si(x.max as f64, "V"),
                    fmt_si(x.mean as f64, "V")
                ));
            }
        }
    }
    Some(t)
}

/// The three status lines of the log view.
pub fn draw_log_status(f: &mut TermFrame, app: &App, areas: [Rect; 3], pal: &Palette) {
    let [l1, l2, l3] = areas;
    let mut spans = Vec::new();
    for t in 0..2 {
        spans.push(channel_span(app, t, pal));
        spans.push(Span::raw(" "));
    }
    if let Some(text) = log_cursor_text(app) {
        spans.push(Span::styled(text, Style::default().fg(pal.cursor)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), l1);
    let line2 = rec_status(app).unwrap_or_else(|| match &app.log_view {
        Some(v) => {
            let h = &v.file.header;
            let w = v.width.get();
            let div = v.steps_per_col(w) * (w as u64 / 10).max(1) as f64 * h.step();
            let size = std::fs::metadata(&v.file.path).map_or(0, |m| m.len());
            let gaps = v.file.gaps().len();
            Line::styled(
                format!(
                    " {} long · {} · {} step · {}/div · {} · {} gap{}",
                    lg::fmt_hms(v.file.time_of(v.end() as f64)),
                    lg::fmt_bytes(size),
                    lg::fmt_step(h.step()),
                    lg::fmt_step(div),
                    if v.zoom.is_none() {
                        "whole log".to_string()
                    } else if v.follow {
                        "newest".to_string()
                    } else {
                        "scrolled".to_string()
                    },
                    gaps,
                    if gaps == 1 { "" } else { "s" }
                ),
                Style::default().fg(Color::Gray),
            )
        }
        None => Line::raw(""),
    });
    f.render_widget(Paragraph::new(line2), l2);
    let line3 = match app.current_message() {
        Some(m) => Line::styled(
            format!(" {}", m.text),
            Style::default().fg(if m.error {
                Color::LightRed
            } else {
                Color::LightGreen
            }),
        ),
        None => Line::styled(
            " V scope · r rec · ←→ PgUp PgDn scroll · +- zoom · F whole · Home End · v hlHL cursors · E export · t time · ? help",
            Style::default().fg(pal.hint),
        ),
    };
    f.render_widget(Paragraph::new(line3), l3);
}
