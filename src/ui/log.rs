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
    left_labels, put, trace_scale, CLASSIC_CURSOR, CLASSIC_CURSOR_NAME, NAMES,
};
use crate::app::App;
use crate::logger::{self as lg, writer::LogState};
use crate::plot::{self, Geometry, Plot, MARKER_W};
use crate::settings::*;
use crate::si::fmt_si;
use crate::theme::Palette;

/// Fill character between min and max in the text styles.
fn fill_char(style: TraceStyle) -> char {
    match style {
        TraceStyle::Line => ':',
        _ => '░',
    }
}

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

/// Decimals that show a time step of `div` seconds.
fn digits_for(div: f64) -> usize {
    match div {
        d if d >= 0.999 => 0,
        d if d >= 0.0999 => 1,
        d if d >= 0.00999 => 2,
        d if d >= 0.000999 => 3,
        _ => 6,
    }
}

/// A time on the time axis: step `i` of the open log.
fn time_label(app: &App, i: u64, div: f64, date: bool) -> String {
    let Some(v) = &app.log_view else {
        return String::new();
    };
    let h = &v.file.header;
    let t = v.file.time_of(i as f64);
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
    let z = v.z(w);
    let start = v.left(w);
    let cols = v.columns(w).unwrap_or_default();
    let per_div = 2 * g.cx;
    let x_of = |dc: f64| dc / per_div as f64 - H_DIVS / 2.0;
    let half_v = V_DIVS / 2.0;

    // Gaps: a dashed red line in each column with lost data.
    let end = start + z * w as u64;
    for (a, b) in v.file.gaps() {
        if b < start || a >= end {
            continue;
        }
        let c0 = (a.max(start) - start) / z;
        let c1 = if b > a {
            (b - 1).max(start) - start
        } else {
            a - start
        } / z;
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

    // Cursors.
    let classic = s.display.cursor_style == CursorStyle::Classic;
    let cursor_x: Vec<Option<f64>> = v
        .cur
        .iter()
        .map(|&c| {
            let dc = (c as f64 - start as f64) / z as f64;
            (v.cursors && dc >= 0.0 && dc <= w as f64).then(|| x_of(dc))
        })
        .collect();
    for x in cursor_x.iter().flatten() {
        if classic {
            plot.dashed_classic(
                (*x, -half_v),
                (*x, half_v),
                plot::LAYER_CURSOR,
                CLASSIC_CURSOR,
            );
        } else {
            plot.dashed_v(*x, plot::LAYER_CURSOR, pal.cursor);
        }
    }

    // Envelopes: filled from min to max, the mean as a line.
    for t in draw_order(s) {
        if t > 1 || !h.channels[t].enabled || !s.channels[t].enabled {
            continue;
        }
        let style = s.display.trace_style[t];
        let (scale, offset) = trace_scale(s, t);
        let yd = |x: f32| (x as f64 / scale + offset).clamp(-half_v, half_v);
        let (fill, dim) = pal.persist(t, 2, 4);
        let layer = layer_of(s, t);
        let fill_layer = plot::LAYER_PERSIST + t as u8;
        if dim {
            plot.dim_upto(fill_layer);
        }
        let pts: Vec<(f64, f64)> = cols
            .iter()
            .enumerate()
            .map(|(dc, a)| {
                let a = a[t];
                let x = x_of(dc as f64);
                (x, if a.is_empty() { f64::NAN } else { yd(a.mean) })
            })
            .collect();
        let braille = matches!(style, TraceStyle::BrailleThin | TraceStyle::BrailleBold);
        if braille {
            // The mean first: the fill on the same layer joins its cells
            // (one colour per braille cell), so the band has no holes.
            plot.styled(&pts, style, false, layer, pal.trace[t]);
        }
        for (dc, a) in cols.iter().enumerate() {
            let a = a[t];
            if a.is_empty() {
                continue;
            }
            if braille {
                let y0 = plot.py(yd(a.max)).floor() as i64;
                let y1 = plot.py(yd(a.min)).floor() as i64;
                plot.vrun(dc as i64, y0, y1, layer, fill);
            } else {
                let r0 = plot.row_of(yd(a.max));
                let r1 = plot.row_of(yd(a.min));
                for row in r0..=r1 {
                    plot.put_glyph(dc / 2, row, fill_char(style), fill_layer, fill);
                }
            }
        }
        if !braille {
            plot.styled(&pts, style, false, layer, pal.trace[t]);
        }
    }

    let buf = f.buffer_mut();
    plot.render(buf, g.plot);
    let sel = s.selected.min(1);
    left_labels(buf, &g, &volts, Style::default().fg(pal.trace[sel]));
    // Time labels; the date too when the screen spans two days.
    let div = (per_div as u64 * z) as f64 * h.step();
    let day = |i: u64| ((h.start_secs() + v.file.time_of(i as f64)) / 86_400.0).floor();
    let date = day(start) != day(end);
    bottom_labels(
        buf,
        &g,
        |k| time_label(app, start + (k * per_div) as u64 * z, div, date),
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
        if classic {
            let y = g.plot.y + plot.row_of(half_v - 0.3) as u16;
            let cx = g.plot.x + plot.col_of((x + 0.05).min(H_DIVS / 2.0 - 0.2)) as u16;
            put(
                buf,
                cx,
                y,
                name,
                Style::default().fg(CLASSIC_CURSOR_NAME),
                right_x,
            );
        } else if g.margins.top >= 1 {
            let cx = g.plot.x + plot.col_of(*x) as u16 + 1;
            put(
                buf,
                cx,
                g.plot.y - 1,
                name,
                Style::default().fg(pal.cursor),
                right_x,
            );
        }
    }
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
    let div = step * (v.z(v.width.get()) * 20) as f64;
    let [a, b] = v.cur;
    let mut t = format!(
        "│ A {} B {} ΔT {}",
        time_label(app, a, div, false),
        time_label(app, b, div, false),
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
            let div = (v.z(w) * (w as u64 / 10).max(1)) as f64 * h.step();
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
            Style::default().fg(Color::DarkGray),
        ),
    };
    f.render_widget(Paragraph::new(line3), l3);
}
