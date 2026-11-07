//! openhantek-tui: a terminal oscilloscope for the Hantek DSO-6022BE.
//!
//! Step 1: a fake scope. The signals are made up in code, so the screen and
//! the controls can be built before we talk to the real hardware.

use std::io;
use std::time::{Duration, Instant};

use ratatui::{
    crossterm::event::{self, Event, KeyCode, KeyEventKind},
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    symbols::Marker,
    text::{Line, Span},
    widgets::{Axis, Block, Chart, Dataset, GraphType, Paragraph},
    DefaultTerminal, Frame,
};

/// Screen grid: 10 divisions wide, 8 divisions high, like a real scope.
const H_DIVS: f64 = 10.0;
const V_DIVS: f64 = 8.0;
/// Samples per screen.
const POINTS: usize = 1000;

const TIME_DIVS: [f64; 16] = [
    1e-6, 2e-6, 5e-6, 10e-6, 20e-6, 50e-6, 100e-6, 200e-6, 500e-6, 1e-3, 2e-3, 5e-3, 10e-3, 20e-3,
    50e-3, 100e-3,
];
const VOLT_DIVS: [f64; 9] = [0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0];

/// A made-up input signal.
#[derive(Clone, Copy)]
enum Signal {
    Sine { freq: f64, amp: f64 },
    Square { freq: f64, high: f64 },
}

impl Signal {
    fn at(self, t: f64) -> f64 {
        match self {
            Signal::Sine { freq, amp } => amp * (std::f64::consts::TAU * freq * t).sin(),
            Signal::Square { freq, high } => {
                if (t * freq).rem_euclid(1.0) < 0.5 {
                    high
                } else {
                    0.0
                }
            }
        }
    }

    /// Trigger level halfway up the signal.
    fn trigger_level(self) -> f64 {
        match self {
            Signal::Sine { .. } => 0.0,
            Signal::Square { high, .. } => high / 2.0,
        }
    }
}

/// Small xorshift noise source, so we need no `rand` crate.
struct Noise(u64);

impl Noise {
    /// Returns a value in -1.0..1.0.
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 52) as f64 - 1.0
    }
}

struct Channel {
    name: &'static str,
    color: Color,
    signal: Signal,
    enabled: bool,
    volt_idx: usize,
    /// Vertical position, in divisions.
    offset: f64,
    /// Last captured samples, in volts.
    samples: Vec<f64>,
}

impl Channel {
    fn volts_per_div(&self) -> f64 {
        VOLT_DIVS[self.volt_idx]
    }

    /// Samples as chart points: x and y in divisions, clipped to the screen.
    fn screen_points(&self) -> Vec<(f64, f64)> {
        let vdiv = self.volts_per_div();
        let limit = V_DIVS / 2.0;
        self.samples
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let x = i as f64 * H_DIVS / (POINTS - 1) as f64 - H_DIVS / 2.0;
                let y = (v / vdiv + self.offset).clamp(-limit, limit);
                (x, y)
            })
            .collect()
    }

    /// Peak-to-peak volts and frequency of the last capture.
    fn measure(&self, dt: f64) -> (f64, Option<f64>) {
        let min = self.samples.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = self
            .samples
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);
        let vpp = max - min;
        // Find edges through the middle, with a band so noise does not count.
        // Rising and falling edges are kept apart, so the duty cycle does not matter.
        let mid = (max + min) / 2.0;
        let band = vpp * 0.1;
        let (mut rising, mut falling) = (Vec::new(), Vec::new());
        let mut high: Option<bool> = None;
        for (i, &v) in self.samples.iter().enumerate() {
            if v > mid + band && high != Some(true) {
                if high.is_some() {
                    rising.push(i);
                }
                high = Some(true);
            } else if v < mid - band && high != Some(false) {
                if high.is_some() {
                    falling.push(i);
                }
                high = Some(false);
            }
        }
        // Mean period from each edge list that has two or more edges.
        let periods: Vec<f64> = [rising, falling]
            .iter()
            .filter(|e| e.len() >= 2)
            .map(|e| (e[e.len() - 1] - e[0]) as f64 * dt / (e.len() - 1) as f64)
            .collect();
        let freq =
            (!periods.is_empty()).then(|| periods.len() as f64 / periods.iter().sum::<f64>());
        (vpp, freq)
    }
}

struct App {
    channels: [Channel; 2],
    selected: usize,
    time_idx: usize,
    paused: bool,
    trigger: bool,
    clock: Instant,
    noise: Noise,
    /// Time between samples in the last capture, in seconds.
    dt: f64,
}

impl App {
    fn new() -> Self {
        Self {
            channels: [
                Channel {
                    name: "CH1",
                    color: Color::Yellow,
                    signal: Signal::Sine {
                        freq: 1000.0,
                        amp: 1.5,
                    },
                    enabled: true,
                    volt_idx: 6, // 1 V/div
                    offset: 1.5,
                    samples: Vec::new(),
                },
                Channel {
                    name: "CH2",
                    color: Color::Cyan,
                    signal: Signal::Square {
                        freq: 2500.0,
                        high: 3.3,
                    },
                    enabled: true,
                    volt_idx: 7, // 2 V/div
                    offset: -3.0,
                    samples: Vec::new(),
                },
            ],
            selected: 0,
            time_idx: 7, // 200 µs/div
            paused: false,
            trigger: true,
            clock: Instant::now(),
            noise: Noise(0x9E37_79B9_7F4A_7C15),
            dt: 0.0,
        }
    }

    fn time_per_div(&self) -> f64 {
        TIME_DIVS[self.time_idx]
    }

    /// Take one screen of samples from the fake signals.
    fn acquire(&mut self) {
        let span = self.time_per_div() * H_DIVS;
        self.dt = span / (POINTS - 1) as f64;
        let mut start = self.clock.elapsed().as_secs_f64();
        if self.trigger {
            let sig = self.channels[self.selected].signal;
            if let Some(edge) = find_rising_edge(sig, sig.trigger_level(), start, self.dt.max(1e-6))
            {
                // Put the trigger point in the middle of the screen.
                start = edge - span / 2.0;
            }
        }
        for ch in &mut self.channels {
            ch.samples.clear();
            for i in 0..POINTS {
                let t = start + i as f64 * self.dt;
                ch.samples.push(ch.signal.at(t) + self.noise.next() * 0.03);
            }
        }
    }

    fn run(mut self, mut terminal: DefaultTerminal) -> io::Result<()> {
        loop {
            if !self.paused {
                self.acquire();
            }
            terminal.draw(|frame| self.draw(frame))?;
            if !event::poll(Duration::from_millis(33))? {
                continue;
            }
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            let sel = self.selected;
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Char(' ') => self.paused = !self.paused,
                KeyCode::Char('t') => self.trigger = !self.trigger,
                KeyCode::Tab => self.selected = (self.selected + 1) % self.channels.len(),
                KeyCode::Char('1') => self.channels[0].enabled ^= true,
                KeyCode::Char('2') => self.channels[1].enabled ^= true,
                KeyCode::Left => self.time_idx = self.time_idx.saturating_sub(1),
                KeyCode::Right => self.time_idx = (self.time_idx + 1).min(TIME_DIVS.len() - 1),
                KeyCode::Up => {
                    self.channels[sel].volt_idx = self.channels[sel].volt_idx.saturating_sub(1)
                }
                KeyCode::Down => {
                    self.channels[sel].volt_idx =
                        (self.channels[sel].volt_idx + 1).min(VOLT_DIVS.len() - 1)
                }
                KeyCode::Char(']') => {
                    self.channels[sel].offset = (self.channels[sel].offset + 0.5).min(V_DIVS / 2.0)
                }
                KeyCode::Char('[') => {
                    self.channels[sel].offset = (self.channels[sel].offset - 0.5).max(-V_DIVS / 2.0)
                }
                _ => {}
            }
        }
    }

    fn draw(&self, frame: &mut Frame) {
        let [chart_area, status_area, help_area] = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        let grid = grid_points();
        let traces: Vec<Vec<(f64, f64)>> =
            self.channels.iter().map(Channel::screen_points).collect();

        // Grid first, so the traces draw on top of it.
        let mut datasets = vec![Dataset::default()
            .marker(Marker::Braille)
            .graph_type(GraphType::Scatter)
            .style(Style::default().fg(Color::DarkGray))
            .data(&grid)];
        for (ch, points) in self.channels.iter().zip(&traces) {
            if ch.enabled {
                datasets.push(
                    Dataset::default()
                        .marker(Marker::Braille)
                        .graph_type(GraphType::Line)
                        .style(Style::default().fg(ch.color))
                        .data(points),
                );
            }
        }

        let tdiv = self.time_per_div();
        let sel = &self.channels[self.selected];
        let vdiv = sel.volts_per_div();
        let half_h = H_DIVS / 2.0;
        let half_v = V_DIVS / 2.0;
        let y_label = |y: f64| fmt_si((y - sel.offset) * vdiv, "V");

        let state = if self.paused { "STOP" } else { "RUN" };
        let title = format!(" openhantek-tui · DEMO · {state} ");
        let chart = Chart::new(datasets)
            .block(Block::bordered().title(title))
            .legend_position(None)
            .x_axis(Axis::default().bounds([-half_h, half_h]).labels([
                fmt_si(-half_h * tdiv, "s"),
                "0".into(),
                fmt_si(half_h * tdiv, "s"),
            ]))
            .y_axis(
                Axis::default()
                    .bounds([-half_v, half_v])
                    .style(Style::default().fg(sel.color))
                    .labels([y_label(-half_v), y_label(0.0), y_label(half_v)]),
            );
        frame.render_widget(chart, chart_area);

        // Status line: one block per channel, then time base and trigger.
        let mut spans = Vec::new();
        for (i, ch) in self.channels.iter().enumerate() {
            let mut style = Style::default().fg(ch.color);
            if i == self.selected {
                style = style.add_modifier(Modifier::REVERSED);
            }
            if !ch.enabled {
                style = style.add_modifier(Modifier::DIM);
            }
            let (vpp, freq) = ch.measure(self.dt);
            let freq = freq.map_or("--".to_string(), |f| fmt_si(f, "Hz"));
            spans.push(Span::styled(
                format!(
                    " {} {}/div  Vpp {}  f {} ",
                    ch.name,
                    fmt_si(ch.volts_per_div(), "V"),
                    fmt_si(vpp, "V"),
                    freq
                ),
                style,
            ));
            spans.push(Span::raw("  "));
        }
        let trig = if self.trigger {
            format!(
                "Trig {} ↑ {}",
                sel.name,
                fmt_si(sel.signal.trigger_level(), "V")
            )
        } else {
            "Trig off".to_string()
        };
        spans.push(Span::raw(format!("{}/div  {trig}", fmt_si(tdiv, "s"))));
        frame.render_widget(Paragraph::new(Line::from(spans)), status_area);

        let help = "q quit · space run/stop · ←/→ time/div · ↑/↓ volts/div · [ ] move · tab channel · 1/2 on/off · t trigger";
        frame.render_widget(
            Paragraph::new(help).style(Style::default().fg(Color::DarkGray)),
            help_area,
        );
    }
}

/// Find the first time after `from` where the signal goes up through `level`.
fn find_rising_edge(sig: Signal, level: f64, from: f64, step: f64) -> Option<f64> {
    let mut prev = sig.at(from);
    for k in 1..=20_000 {
        let t = from + k as f64 * step;
        let v = sig.at(t);
        if prev < level && v >= level {
            // Narrow the crossing down between the two samples.
            let (mut lo, mut hi) = (t - step, t);
            for _ in 0..40 {
                let mid = (lo + hi) / 2.0;
                if sig.at(mid) < level {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            return Some(hi);
        }
        prev = v;
    }
    None
}

/// Dotted grid lines at every division.
fn grid_points() -> Vec<(f64, f64)> {
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
}

/// Format a value with an SI prefix, like `200 µs` or `1.5 kHz`.
fn fmt_si(value: f64, unit: &str) -> String {
    const PREFIXES: [(f64, &str); 6] = [
        (1e6, "M"),
        (1e3, "k"),
        (1.0, ""),
        (1e-3, "m"),
        (1e-6, "µ"),
        (1e-9, "n"),
    ];
    if value == 0.0 {
        return format!("0 {unit}");
    }
    let (scale, prefix) = PREFIXES
        .iter()
        .find(|(scale, _)| value.abs() >= *scale * 0.999)
        .copied()
        .unwrap_or((1e-9, "n"));
    let n = value / scale;
    let text = if n.abs() >= 100.0 {
        format!("{n:.0}")
    } else if n.abs() >= 10.0 {
        format!("{n:.1}")
    } else {
        format!("{n:.2}")
    };
    let text = if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    };
    format!("{text} {prefix}{unit}")
}

fn main() -> io::Result<()> {
    let terminal = ratatui::init();
    let result = App::new().run(terminal);
    ratatui::restore();
    result
}
