//! Screen colours. True colour when the terminal has it, else the
//! 256-colour or 16-colour palette.

use ratatui::style::Color;
use serde::{Deserialize, Serialize};

use crate::settings::GRID_BRIGHTNESS_DEFAULT;

/// Which colours to use. `Auto` looks at `COLORTERM`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ColorMode {
    #[default]
    Auto,
    TrueColor,
    Ansi256,
    Ansi16,
}

impl ColorMode {
    pub const ALL: [ColorMode; 4] = [
        ColorMode::Auto,
        ColorMode::TrueColor,
        ColorMode::Ansi256,
        ColorMode::Ansi16,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ColorMode::Auto => "auto",
            ColorMode::TrueColor => "true colour",
            ColorMode::Ansi256 => "256 colours",
            ColorMode::Ansi16 => "16 colours",
        }
    }

    /// The mode to draw with: `Auto` becomes true colour or 256 colours.
    pub fn resolve(self, colorterm: Option<&str>) -> ColorMode {
        match self {
            ColorMode::Auto => {
                let c = colorterm.unwrap_or("").to_ascii_lowercase();
                if c.contains("truecolor") || c.contains("24bit") {
                    ColorMode::TrueColor
                } else {
                    ColorMode::Ansi256
                }
            }
            m => m,
        }
    }
}

/// An RGB colour.
pub type Rgb = (u8, u8, u8);

/// CH1, CH2, MATH in true colour.
const TRACE_RGB: [Rgb; 3] = [(255, 214, 10), (0, 200, 255), (220, 90, 255)];
const BACKGROUND: Rgb = (0, 0, 0);

/// Brightness of the newest and the oldest old trace of the persistence,
/// as a share of the way from the background (0) to the trace colour (1).
pub const FADE_NEWEST: f64 = 0.85;
pub const FADE_OLDEST: f64 = 0.12;

/// Brightness of the old trace `age` (0 = newest old frame) of `depth`
/// old frames: even steps from [`FADE_NEWEST`] to [`FADE_OLDEST`].
pub fn fade_level(age: usize, depth: usize) -> f64 {
    if depth <= 1 {
        return FADE_NEWEST;
    }
    let k = age.min(depth - 1) as f64 / (depth - 1) as f64;
    FADE_NEWEST + (FADE_OLDEST - FADE_NEWEST) * k
}

fn to_linear(c: u8) -> f64 {
    let c = c as f64 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn to_srgb(l: f64) -> u8 {
    let l = l.clamp(0.0, 1.0);
    let c = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (c * 255.0).round() as u8
}

/// `trace` faded toward `bg`: `level` 1 is the trace, 0 the background.
/// The level is perceptual (CIE lightness): it is turned into a share of
/// light, and the colours are mixed in linear light, so even levels look
/// like even steps.
pub fn fade(trace: Rgb, bg: Rgb, level: f64) -> Rgb {
    let level = level.clamp(0.0, 1.0);
    // CIE L* (0..1) to relative luminance.
    let light = if level > 0.08 {
        ((level + 0.16) / 1.16).powi(3)
    } else {
        level / 9.033
    };
    let mix = |t: u8, b: u8| {
        let (t, b) = (to_linear(t), to_linear(b));
        to_srgb(b + (t - b) * light)
    };
    (mix(trace.0, bg.0), mix(trace.1, bg.1), mix(trace.2, bg.2))
}

/// Perceptual lightness (CIE L*, 0..1) of a gray `v`: the inverse of
/// [`fade`] from black to white.
fn gray_lightness(v: u8) -> f64 {
    let y = to_linear(v);
    if y > 0.008_856 {
        1.16 * y.cbrt() - 0.16
    } else {
        9.033 * y
    }
}

/// The xterm 256-colour index nearest to `c`: the 6 x 6 x 6 cube or the
/// gray ramp.
pub fn xterm_index(c: Rgb) -> u8 {
    const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let near = |v: u8| {
        (0..6)
            .min_by_key(|&i| (STEPS[i] as i32 - v as i32).abs())
            .unwrap_or(0)
    };
    let dist = |a: Rgb, b: Rgb| {
        let d = |x: u8, y: u8| (x as i32 - y as i32).pow(2);
        d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)
    };
    let (r, g, b) = (near(c.0), near(c.1), near(c.2));
    let cube = (STEPS[r], STEPS[g], STEPS[b]);
    let cube_i = 16 + 36 * r as u8 + 6 * g as u8 + b as u8;
    let avg = (c.0 as i32 + c.1 as i32 + c.2 as i32) / 3;
    let gi = ((avg - 8 + 5) / 10).clamp(0, 23) as u8;
    let gv = 8 + 10 * gi;
    if dist(c, (gv, gv, gv)) < dist(c, cube) {
        232 + gi
    } else {
        cube_i
    }
}

/// The xterm 256-colour index nearest to `c` that keeps its hue: dark
/// colours get the darkest cube colour of their hue instead of black or a
/// gray, so a faded trace keeps the colour of its channel.
pub fn xterm_tint(c: Rgb) -> u8 {
    let i = xterm_index(c);
    let max = c.0.max(c.1).max(c.2);
    if max == 0 || (17..232).contains(&i) {
        return i;
    }
    // Each channel: nearest step, but the brightest ones at least 95.
    const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let step = |v: u8| {
        let n = (0..6)
            .min_by_key(|&k| (STEPS[k] as i32 - v as i32).abs())
            .unwrap_or(0) as u8;
        if v as u16 * 2 >= max as u16 {
            n.max(1)
        } else {
            n
        }
    };
    16 + 36 * step(c.0) + 6 * step(c.1) + step(c.2)
}

/// All colours the screen uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// CH1, CH2, MATH.
    pub trace: [Color; 3],
    /// Colour kind of this palette: true colour, 256 or 16 colours.
    pub mode: ColorMode,
    /// The trace colours as RGB, the source of the fading old traces.
    pub trace_rgb: [Rgb; 3],
    /// The plot background the old traces fade toward. The plot does not
    /// paint its background, so this is the dark terminal background the
    /// grid colours are made for.
    pub background: Rgb,
    /// Grid dots at the division points.
    pub grid: Color,
    /// The center cross.
    pub axis: Color,
    pub cursor: Color,
    pub xy: Color,
    pub trigger: Color,
    /// Labels in the plot margins.
    pub label: Color,
    /// The help line of the settings panel: a blue gray that is neither
    /// the gray of the labels and the panel title nor the plain text of
    /// the setting rows.
    pub help: Color,
}

impl Palette {
    pub fn new(mode: ColorMode) -> Self {
        match mode {
            ColorMode::TrueColor | ColorMode::Auto => Self {
                trace: [
                    Color::Rgb(255, 214, 10),
                    Color::Rgb(0, 200, 255),
                    Color::Rgb(220, 90, 255),
                ],
                mode: ColorMode::TrueColor,
                trace_rgb: TRACE_RGB,
                background: BACKGROUND,
                grid: Color::Rgb(60, 60, 60),
                axis: Color::Rgb(105, 105, 105),
                cursor: Color::Rgb(210, 210, 210),
                xy: Color::Rgb(80, 250, 120),
                trigger: Color::Rgb(255, 95, 95),
                label: Color::Rgb(150, 150, 150),
                help: Color::Rgb(120, 165, 205),
            },
            ColorMode::Ansi256 => Self {
                trace: [Color::Indexed(220), Color::Indexed(45), Color::Indexed(171)],
                mode: ColorMode::Ansi256,
                trace_rgb: TRACE_RGB,
                background: BACKGROUND,
                grid: Color::Indexed(237),
                axis: Color::Indexed(242),
                cursor: Color::Indexed(252),
                xy: Color::Indexed(84),
                trigger: Color::Indexed(203),
                label: Color::Indexed(246),
                help: Color::Indexed(110),
            },
            ColorMode::Ansi16 => Self {
                trace: [Color::LightYellow, Color::LightCyan, Color::LightMagenta],
                mode: ColorMode::Ansi16,
                trace_rgb: TRACE_RGB,
                background: BACKGROUND,
                grid: Color::DarkGray,
                axis: Color::Gray,
                cursor: Color::White,
                xy: Color::LightGreen,
                trigger: Color::LightRed,
                label: Color::Gray,
                help: Color::Cyan,
            },
        }
    }

    /// Colour of old trace `age` (0 = newest old frame) of trace `t`, with
    /// `depth` old frames. The newest is close to the trace colour, the
    /// oldest close to the background. True colour fades in even steps,
    /// 256 colours take the nearest xterm colour. 16 colours have two
    /// steps: the normal colour for the newer half, and the same colour
    /// with `true` (draw it with the DIM modifier) for the older half.
    pub fn persist(&self, t: usize, age: usize, depth: usize) -> (Color, bool) {
        let rgb = fade(self.trace_rgb[t], self.background, fade_level(age, depth));
        match self.mode {
            ColorMode::TrueColor | ColorMode::Auto => (Color::Rgb(rgb.0, rgb.1, rgb.2), false),
            ColorMode::Ansi256 => (Color::Indexed(xterm_tint(rgb)), false),
            ColorMode::Ansi16 => {
                let normal = [Color::Yellow, Color::Cyan, Color::Magenta][t];
                (normal, depth > 1 && age * 2 >= depth)
            }
        }
    }

    /// Grid and center cross colours at grid brightness `percent` (10 to
    /// 100), and whether the grid needs the DIM modifier. At
    /// [`GRID_BRIGHTNESS_DEFAULT`] they are [`Palette::grid`] and
    /// [`Palette::axis`]. True colour scales their perceptual lightness
    /// (toward the background below the default, toward white above),
    /// 256 colours take the nearest xterm gray, and 16 colours have four
    /// steps: dim dark gray, dark gray (the default), gray and white.
    pub fn grid_colors(&self, percent: u8) -> (Color, Color, bool) {
        let k = percent.clamp(1, 100) as f64 / GRID_BRIGHTNESS_DEFAULT as f64;
        let white = (255, 255, 255);
        let gray = |base: u8| fade(white, self.background, gray_lightness(base) * k);
        let (g, a) = (gray(60), gray(105));
        match self.mode {
            ColorMode::TrueColor | ColorMode::Auto => {
                (Color::Rgb(g.0, g.1, g.2), Color::Rgb(a.0, a.1, a.2), false)
            }
            ColorMode::Ansi256 => (
                Color::Indexed(xterm_index(g)),
                Color::Indexed(xterm_index(a)),
                false,
            ),
            ColorMode::Ansi16 => match percent {
                0..=20 => (Color::DarkGray, Color::DarkGray, true),
                21..=50 => (Color::DarkGray, Color::Gray, false),
                51..=80 => (Color::Gray, Color::White, false),
                _ => (Color::White, Color::White, false),
            },
        }
    }

    /// Palette for a setting, using the `COLORTERM` of this process.
    pub fn for_setting(mode: ColorMode) -> Self {
        Self::new(mode.resolve(colorterm()))
    }
}

/// `COLORTERM`, read once.
fn colorterm() -> Option<&'static str> {
    static TERM: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    TERM.get_or_init(|| std::env::var("COLORTERM").ok())
        .as_deref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_follows_colorterm() {
        assert_eq!(
            ColorMode::Auto.resolve(Some("truecolor")),
            ColorMode::TrueColor
        );
        assert_eq!(ColorMode::Auto.resolve(Some("24bit")), ColorMode::TrueColor);
        assert_eq!(ColorMode::Auto.resolve(None), ColorMode::Ansi256);
        assert_eq!(ColorMode::Auto.resolve(Some("")), ColorMode::Ansi256);
        assert_eq!(
            ColorMode::Ansi16.resolve(Some("truecolor")),
            ColorMode::Ansi16
        );
    }

    #[test]
    fn palettes_use_their_colour_kind() {
        let t = Palette::new(ColorMode::TrueColor);
        assert!(matches!(t.trace[0], Color::Rgb(..)));
        assert_eq!(t.trace[0], Color::Rgb(255, 214, 10));
        let i = Palette::new(ColorMode::Ansi256);
        assert!(i.trace.iter().all(|c| matches!(c, Color::Indexed(_))));
        let s = Palette::new(ColorMode::Ansi16);
        assert_eq!(s.trace[0], Color::LightYellow);
    }

    fn luma(c: Rgb) -> f64 {
        0.2126 * to_linear(c.0) + 0.7152 * to_linear(c.1) + 0.0722 * to_linear(c.2)
    }

    #[test]
    fn fade_endpoints() {
        let (t, bg) = ((255, 214, 10), (0, 0, 0));
        assert_eq!(fade(t, bg, 1.0), t);
        assert_eq!(fade(t, bg, 0.0), bg);
        let g = (30, 40, 50);
        assert_eq!(fade(t, g, 0.0), g);
        assert_eq!(fade(t, g, 1.0), t);
        assert_eq!(fade_level(0, 8), FADE_NEWEST);
        assert_eq!(fade_level(7, 8), FADE_OLDEST);
        assert_eq!(fade_level(0, 1), FADE_NEWEST);
        assert_eq!(fade_level(1, 2), FADE_OLDEST);
    }

    #[test]
    fn fade_is_monotonic_by_age() {
        for depth in [2, 4, 8, 16] {
            for &t in &TRACE_RGB {
                let mut last = f64::INFINITY;
                for age in 0..depth {
                    let c = fade(t, BACKGROUND, fade_level(age, depth));
                    let l = luma(c);
                    assert!(l < last, "depth {depth} age {age}: {c:?}");
                    last = l;
                }
                // Newest close to the trace, oldest close to the background.
                assert!(luma(fade(t, BACKGROUND, FADE_NEWEST)) > luma(t) * 0.6);
                assert!(luma(fade(t, BACKGROUND, FADE_OLDEST)) < luma(t) * 0.03);
            }
        }
    }

    #[test]
    fn persist_colours_per_mode() {
        let t = Palette::new(ColorMode::TrueColor);
        let all: Vec<Color> = (0..8).map(|a| t.persist(0, a, 8).0).collect();
        for w in all.windows(2) {
            assert_ne!(w[0], w[1]);
        }
        let i = Palette::new(ColorMode::Ansi256);
        let mut idx: Vec<Color> = (0..8).map(|a| i.persist(0, a, 8).0).collect();
        assert!(idx.iter().all(|c| matches!(c, Color::Indexed(_))));
        idx.dedup();
        assert!(idx.len() >= 4, "{idx:?}");
        let s = Palette::new(ColorMode::Ansi16);
        assert_eq!(s.persist(1, 0, 8), (Color::Cyan, false));
        assert_eq!(s.persist(1, 3, 8), (Color::Cyan, false));
        assert_eq!(s.persist(1, 4, 8), (Color::Cyan, true));
        assert_eq!(s.persist(1, 7, 8), (Color::Cyan, true));
    }

    #[test]
    fn grid_colours_by_brightness() {
        use crate::settings::GRID_BRIGHTNESS;
        // The default brightness gives the palette's own grid colours.
        for mode in [ColorMode::TrueColor, ColorMode::Ansi256, ColorMode::Ansi16] {
            let p = Palette::new(mode);
            assert_eq!(
                p.grid_colors(GRID_BRIGHTNESS_DEFAULT),
                (p.grid, p.axis, false),
                "{mode:?}"
            );
        }
        // True colour: each step brighter, and the center cross brighter
        // than the grid (until both are white).
        let p = Palette::new(ColorMode::TrueColor);
        let level = |c: Color| match c {
            Color::Rgb(r, g, b) => {
                assert!(r == g && g == b);
                r
            }
            _ => panic!("{c:?}"),
        };
        let mut last = 0;
        for b in GRID_BRIGHTNESS {
            let (g, a, dim) = p.grid_colors(b);
            assert!(!dim);
            assert!(level(g) > last, "{b}");
            assert!(level(a) > level(g) || level(a) == 255, "{b}");
            last = level(g);
        }
        // 256 colours: grays, different at the ends.
        let p = Palette::new(ColorMode::Ansi256);
        assert_ne!(p.grid_colors(10).0, p.grid_colors(100).0);
        // 16 colours: dim at the lowest steps.
        let p = Palette::new(ColorMode::Ansi16);
        assert!(p.grid_colors(10).2);
        assert_eq!(p.grid_colors(100).0, Color::White);
    }

    #[test]
    fn xterm_index_finds_exact_colours() {
        assert_eq!(xterm_index((0, 0, 0)), 16);
        assert_eq!(xterm_index((255, 255, 255)), 231);
        assert_eq!(xterm_index((255, 215, 0)), 220);
        assert_eq!(xterm_index((128, 128, 128)), 244);
        // Dark colours keep their hue.
        assert_eq!(xterm_tint((31, 25, 0)), 58);
        assert_eq!(xterm_tint((0, 22, 31)), 23);
        assert_eq!(xterm_tint((255, 215, 0)), 220);
        assert_eq!(xterm_tint((0, 0, 0)), 16);
    }
}
