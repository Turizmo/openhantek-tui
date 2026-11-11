//! Screen colours. The traces are ANSI colours by name (the terminal
//! theme picks the shade) or RGB, each with its own brightness and
//! saturation. The rest (old traces, grid, labels) is blended from the
//! colours the terminal tells us. The colour modes only differ in how
//! many colours they write. With no answer from the terminal every mode
//! uses the 16 colours by name.

use ratatui::style::{Color, Modifier};
use serde::{Deserialize, Serialize};

use crate::settings::{DisplaySettings, GRID_BRIGHTNESS_DEFAULT};
use crate::termcolors::TermColors;

/// How many colours to write. `Auto` looks at `COLORTERM`: true colour
/// when it says so, else 256 colours.
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

/// Where the colours come from. `Terminal` uses the colours the
/// terminal tells us. `Light` and `Dark` are our own sets, written as RGB, so the
/// terminal theme does not matter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ColorTheme {
    #[default]
    Terminal,
    Light,
    Dark,
}

impl ColorTheme {
    pub const ALL: [ColorTheme; 3] = [ColorTheme::Terminal, ColorTheme::Light, ColorTheme::Dark];

    pub fn name(self) -> &'static str {
        match self {
            ColorTheme::Terminal => "terminal",
            ColorTheme::Light => "light",
            ColorTheme::Dark => "dark",
        }
    }
}

/// A full set of colours: text, background and the 16 ANSI colours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scheme {
    pub fg: Rgb,
    pub bg: Rgb,
    pub ansi: [Rgb; 16],
    /// The colours are the terminal's own: plain ANSI trace colours are
    /// sent by name. False for a preset, which is always sent as RGB.
    pub named: bool,
}

/// The dark preset, for a dark terminal background.
pub const DARK: Scheme = Scheme {
    fg: (0xd0, 0xd0, 0xd0),
    bg: (0x1a, 0x1b, 0x1e),
    ansi: [
        (0x2e, 0x30, 0x34),
        (0xd0, 0x50, 0x50),
        (0x5a, 0xb4, 0x5a),
        (0xc8, 0xa0, 0x30),
        (0x4c, 0x82, 0xc8),
        (0xb0, 0x60, 0xc0),
        (0x40, 0xaa, 0xaa),
        (0xb4, 0xb4, 0xb4),
        (0x6c, 0x6e, 0x72),
        (0xff, 0x6e, 0x6e),
        (0x7c, 0xe0, 0x7c),
        (0xff, 0xd8, 0x4a),
        (0x6a, 0xae, 0xff),
        (0xe8, 0x7c, 0xf4),
        (0x60, 0xe0, 0xe0),
        (0xf4, 0xf4, 0xf4),
    ],
    named: false,
};

/// The light preset, for a light terminal background. The yellow is a
/// dark amber, so it stands out on white. Like most light themes, black
/// is the light colour and white the dark one, so text meant to stand
/// out on a dark screen stands out here too.
pub const LIGHT: Scheme = Scheme {
    fg: (0x1c, 0x1c, 0x1c),
    bg: (0xf8, 0xf8, 0xf6),
    ansi: [
        (0xf0, 0xee, 0xe8),
        (0xa8, 0x22, 0x22),
        (0x1a, 0x6e, 0x1a),
        (0x80, 0x58, 0x00),
        (0x1e, 0x4e, 0x9e),
        (0x88, 0x2a, 0x96),
        (0x0a, 0x6c, 0x76),
        (0x4a, 0x4a, 0x4a),
        (0x9a, 0x9a, 0x9a),
        (0xd2, 0x2e, 0x2e),
        (0x1e, 0x88, 0x1e),
        (0xa0, 0x6a, 0x00),
        (0x14, 0x58, 0xc8),
        (0xa0, 0x28, 0xb4),
        (0x0c, 0x8c, 0x98),
        (0x1e, 0x1e, 0x1e),
    ],
    named: false,
};

impl Scheme {
    /// The colours of `theme`. `term` is the terminal's answer and
    /// `colorfgbg` the `COLORFGBG` value. `Terminal` with no usable answer
    /// falls back to a preset: light when `COLORFGBG` names background 7
    /// or 15, else dark. The flag is true for that fallback.
    pub fn of(
        theme: ColorTheme,
        term: Option<&TermColors>,
        colorfgbg: Option<&str>,
    ) -> (Scheme, bool) {
        match theme {
            ColorTheme::Light => (LIGHT, false),
            ColorTheme::Dark => (DARK, false),
            ColorTheme::Terminal => match term.filter(|t| t.usable()) {
                Some(t) => {
                    let mut ansi = XTERM16;
                    for (a, d) in ansi.iter_mut().zip(t.ansi) {
                        if let Some(d) = d {
                            *a = d;
                        }
                    }
                    let s = Scheme {
                        fg: t.fg.unwrap_or((255, 255, 255)),
                        bg: t.bg.unwrap_or((0, 0, 0)),
                        ansi,
                        named: true,
                    };
                    (s, false)
                }
                None => (fallback(colorfgbg), true),
            },
        }
    }
}

/// The preset for a terminal that does not tell its colours: light when
/// `COLORFGBG` (like `0;15`) names background 7 or 15, else dark.
pub fn fallback(colorfgbg: Option<&str>) -> Scheme {
    let bg = colorfgbg.and_then(|v| v.rsplit(';').next()).map(str::trim);
    match bg {
        Some("7") | Some("15") => LIGHT,
        _ => DARK,
    }
}

/// The note for the message line when `theme` is `Terminal` but the
/// terminal did not tell its colours.
pub fn fallback_note(
    theme: ColorTheme,
    term: Option<&TermColors>,
    colorfgbg: Option<&str>,
) -> Option<String> {
    let (s, fell_back) = Scheme::of(theme, term, colorfgbg);
    let name = if s == LIGHT { "light" } else { "dark" };
    fell_back.then(|| format!("No colour answer from the terminal; using the {name} preset"))
}

/// An RGB colour.
pub type Rgb = (u8, u8, u8);

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

/// Colour `t` of the way from `bg` (0) to `c` (1), mixed in Oklab: even
/// steps look even on a light background as well as on a dark one.
pub fn blend(c: Rgb, bg: Rgb, t: f64) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let (a, b) = (oklab(c), oklab(bg));
    from_oklab([
        b[0] + (a[0] - b[0]) * t,
        b[1] + (a[1] - b[1]) * t,
        b[2] + (a[2] - b[2]) * t,
    ])
}

fn oklab(c: Rgb) -> [f64; 3] {
    let (r, g, b) = (to_linear(c.0), to_linear(c.1), to_linear(c.2));
    let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
    let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
    let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
    [
        0.210_454_255_3 * l + 0.793_617_785 * m - 0.004_072_046_8 * s,
        1.977_998_495_1 * l - 2.428_592_205 * m + 0.450_593_709_9 * s,
        0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766 * s,
    ]
}

/// Linear RGB of an Oklab colour. Out of the sRGB gamut it can be below
/// 0 or above 1.
fn oklab_to_linear(c: [f64; 3]) -> [f64; 3] {
    let l = (c[0] + 0.396_337_777_4 * c[1] + 0.215_803_757_3 * c[2]).powi(3);
    let m = (c[0] - 0.105_561_345_8 * c[1] - 0.063_854_172_8 * c[2]).powi(3);
    let s = (c[0] - 0.089_484_177_5 * c[1] - 1.291_485_548 * c[2]).powi(3);
    [
        4.076_741_662_1 * l - 3.307_711_591_3 * m + 0.230_969_929_2 * s,
        -1.268_438_004_6 * l + 2.609_757_401_1 * m - 0.341_319_396_5 * s,
        -0.004_196_086_3 * l - 0.703_418_614_7 * m + 1.707_614_701 * s,
    ]
}

fn from_oklab(c: [f64; 3]) -> Rgb {
    let [r, g, b] = oklab_to_linear(c);
    (to_srgb(r), to_srgb(g), to_srgb(b))
}

/// `c` with its Oklab chroma times `k`, at the same lightness and hue:
/// 0 is the gray of the same lightness, above 1 more colourful. A boost
/// stops at the edge of the sRGB gamut, so the hue stays.
pub fn saturate(c: Rgb, k: f64) -> Rgb {
    let k = k.max(0.0);
    if k == 1.0 {
        return c;
    }
    let [l, a, b] = oklab(c);
    let k = gamut_chroma(l, a, b, k);
    from_oklab([l, a * k, b * k])
}

/// True when an Oklab colour is inside the sRGB gamut.
fn in_gamut(lab: [f64; 3]) -> bool {
    oklab_to_linear(lab)
        .iter()
        .all(|v| (-1e-4..=1.0 + 1e-4).contains(v))
}

/// The chroma factor to use for `k` times the chroma of Oklab `l a b`:
/// `k` itself, or the largest factor below it that is still inside the
/// sRGB gamut. So the hue and the lightness stay.
fn gamut_chroma(l: f64, a: f64, b: f64, k: f64) -> f64 {
    let inside = |k: f64| in_gamut([l, a * k, b * k]);
    if inside(k) {
        return k;
    }
    let (mut lo, mut hi) = (0.0, k);
    for _ in 0..30 {
        let mid = (lo + hi) / 2.0;
        if inside(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

/// `c` with its Oklab lightness moved `share` (0 to 1) of the way to the
/// lightness of `fg`, away from `bg`. A colour that already stands out
/// from `bg` as much as `fg` goes toward white instead (black on a light
/// background). The hue and chroma stay as far as the sRGB gamut allows.
fn toward_contrast(c: Rgb, bg: Rgb, fg: Rgb, share: f64) -> Rgb {
    let [l, a, b] = oklab(c);
    let (lb, lf) = (oklab(bg)[0], oklab(fg)[0]);
    let target = if (lf - lb).abs() > (l - lb).abs() {
        lf
    } else if l >= lb {
        1.0
    } else {
        0.0
    };
    let l = l + (target - l) * share.clamp(0.0, 1.0);
    let k = gamut_chroma(l, a, b, 1.0);
    from_oklab([l, a * k, b * k])
}

/// A trace colour with brightness `brightness` and saturation
/// `saturation` (percent of the chroma), both in Oklab. Brightness is
/// percent, -100 to 100: below 0 the share of the way to `bg`, above 0
/// the share of the way to the lightness of the text colour `fg` (more
/// contrast; see [`toward_contrast`]). Brightness 0 and saturation 100
/// give `c`.
pub fn adjust(c: Rgb, bg: Rgb, fg: Rgb, brightness: i16, saturation: u16) -> Rgb {
    let c = saturate(c, saturation as f64 / 100.0);
    let share = brightness.clamp(-100, 100) as f64 / 100.0;
    match brightness {
        0 => c,
        b if b < 0 => blend(c, bg, 1.0 + share),
        _ => toward_contrast(c, bg, fg, share),
    }
}

/// Distance of two colours, in Oklab.
fn distance(a: Rgb, b: Rgb) -> f64 {
    let (a, b) = (oklab(a), oklab(b));
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

/// The xterm default RGB of the 16 ANSI colours, for when the terminal
/// does not say its own.
pub const XTERM16: [Rgb; 16] = [
    (0, 0, 0),
    (205, 0, 0),
    (0, 205, 0),
    (205, 205, 0),
    (0, 0, 238),
    (205, 0, 205),
    (0, 205, 205),
    (229, 229, 229),
    (127, 127, 127),
    (255, 0, 0),
    (0, 255, 0),
    (255, 255, 0),
    (92, 92, 255),
    (255, 0, 255),
    (0, 255, 255),
    (255, 255, 255),
];

/// Names of the 16 ANSI colours, as in the settings. The terminal theme
/// decides the real shade of each.
pub const ANSI_NAMES: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "gray",
    "dark-gray",
    "bright-red",
    "bright-green",
    "bright-yellow",
    "bright-blue",
    "bright-magenta",
    "bright-cyan",
    "white",
];

/// ANSI colour `n` (0 to 15) as a ratatui colour.
pub fn ansi_color(n: u8) -> Color {
    const ALL: [Color; 16] = [
        Color::Black,
        Color::Red,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
        Color::Cyan,
        Color::Gray,
        Color::DarkGray,
        Color::LightRed,
        Color::LightGreen,
        Color::LightYellow,
        Color::LightBlue,
        Color::LightMagenta,
        Color::LightCyan,
        Color::White,
    ];
    ALL[n as usize & 15]
}

/// The plain colour of ANSI colour `n` for a faded trace in 16 colours:
/// a bright colour becomes its normal colour, white becomes gray. Dark
/// gray stays, so it does not turn black.
fn plain16(n: u8) -> u8 {
    match n {
        9..=14 => n - 8,
        15 => 7,
        n => n,
    }
}

/// The index in `table` of the colour nearest to `c`.
pub fn nearest16(c: Rgb, table: &[Rgb; 16]) -> u8 {
    (0..16u8)
        .min_by(|&a, &b| {
            distance(c, table[a as usize]).total_cmp(&distance(c, table[b as usize]))
        })
        .unwrap_or(15)
}

/// Six hex digits, like `ff8800`.
pub fn parse_hex(hex: &str) -> Option<Rgb> {
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some((p(0)?, p(2)?, p(4)?))
}

/// The colour of one trace, a setting: one of the 16 ANSI colours (the
/// terminal theme decides the shade), or an exact RGB colour. Written as
/// a name of [`ANSI_NAMES`] or `#rrggbb`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum TraceColor {
    /// ANSI colour 0 to 15.
    Ansi(u8),
    Rgb(Rgb),
}

impl TraceColor {
    /// CH1, CH2, MATH: bright yellow, bright blue, bright magenta.
    pub const DEFAULTS: [TraceColor; 3] = [
        TraceColor::Ansi(11),
        TraceColor::Ansi(12),
        TraceColor::Ansi(13),
    ];

    /// The 16 names, in the order of the settings panel.
    pub fn choices() -> Vec<TraceColor> {
        (0..16).map(TraceColor::Ansi).collect()
    }

    pub fn name(self) -> String {
        match self {
            TraceColor::Ansi(n) => ANSI_NAMES[n as usize & 15].into(),
            TraceColor::Rgb((r, g, b)) => format!("#{r:02x}{g:02x}{b:02x}"),
        }
    }

    /// Parse a colour name or `#rrggbb`. Case and `_` for `-` do not
    /// matter; `grey` is `gray`. (`auto` of older files is changed to
    /// the default colour when the config file loads.)
    pub fn parse(text: &str) -> Result<TraceColor, String> {
        let t = text.trim().to_ascii_lowercase().replace(['_', ' '], "-");
        let t = t.replace("grey", "gray");
        if let Some(hex) = t.strip_prefix('#') {
            return parse_hex(hex)
                .map(TraceColor::Rgb)
                .ok_or_else(|| format!("not a colour: {text} (use #rrggbb)"));
        }
        ANSI_NAMES
            .iter()
            .position(|n| *n == t)
            .map(|n| TraceColor::Ansi(n as u8))
            .ok_or_else(|| format!("not a colour: {text} (use a colour name or #rrggbb)"))
    }
}

impl TryFrom<String> for TraceColor {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        TraceColor::parse(&s)
    }
}

impl From<TraceColor> for String {
    fn from(c: TraceColor) -> String {
        c.name()
    }
}

/// In 16 colours a trace cannot be blended. A brightness of this much or
/// more draws it with the BOLD modifier (many terminals draw bold text
/// brighter), and minus this much or less with the DIM modifier.
pub const BRIGHTNESS16_FROM: i16 = 50;

/// How one trace looks: its colour, brightness and saturation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraceLook {
    pub color: TraceColor,
    /// Percent, -100 to 100: below 0 the share of the way to the
    /// background, above 0 the share of the way to the lightness of the
    /// text colour.
    pub brightness: i16,
    /// Percent of the colour's chroma, 0 to 400.
    pub saturation: u16,
}

impl TraceLook {
    /// `color` with brightness 0 and full saturation.
    pub const fn plain(color: TraceColor) -> Self {
        Self {
            color,
            brightness: 0,
            saturation: 100,
        }
    }

    /// The looks of CH1, CH2 and MATH in the display settings.
    pub fn of(d: &DisplaySettings) -> [TraceLook; 3] {
        std::array::from_fn(|t| Self {
            color: d.trace_colors[t],
            brightness: d.trace_brightness[t],
            saturation: d.trace_saturation[t],
        })
    }

    /// True when brightness and saturation change nothing.
    pub fn is_plain(self) -> bool {
        self.brightness == 0 && self.saturation == 100
    }

    /// The modifier that stands in for the brightness in 16 colours:
    /// BOLD for a high brightness, DIM for a low one, else none.
    pub fn modifier16(self) -> Modifier {
        if self.brightness >= BRIGHTNESS16_FROM {
            Modifier::BOLD
        } else if self.brightness <= -BRIGHTNESS16_FROM {
            Modifier::DIM
        } else {
            Modifier::empty()
        }
    }
}

/// Levels of the colours made from the text and background colours: the
/// share of the way from the background to the text colour.
pub const THEMED_GRID: f64 = 0.3;
pub const THEMED_AXIS: f64 = 0.48;
pub const THEMED_LABEL: f64 = 0.75;
pub const THEMED_HINT: f64 = 0.6;
pub const THEMED_CURSOR: f64 = 0.9;

/// All colours the screen uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// CH1, CH2, MATH: the chosen colour with no brightness or
    /// saturation. For text and markers: the channel line, the
    /// measurements, the labels and the arrows.
    pub trace: [Color; 3],
    /// The drawn waveform of each trace: the chosen colour with its
    /// brightness and saturation.
    pub line: [Color; 3],
    /// How colours are written: true colour, 256 or 16 colours. Auto is
    /// true colour or 256 colours here, from `COLORTERM`.
    pub mode: ColorMode,
    /// The colours everything is made from: the terminal's or a preset.
    pub scheme: Scheme,
    /// The text colours of the traces as RGB.
    pub trace_rgb: [Rgb; 3],
    /// The waveform colours as RGB, the source of the fading old traces.
    pub line_rgb: [Rgb; 3],
    /// The ANSI colour (0 to 15) nearest to each trace, for the faded
    /// traces in 16 colours.
    pub trace16: [u8; 3],
    /// The modifier to draw each trace with: in 16 colours, where a
    /// trace cannot be blended, BOLD or DIM for a brightness of at least
    /// [`BRIGHTNESS16_FROM`] up or down. Else none.
    pub trace_modifier: [Modifier; 3],
    /// The looks the traces were made from, for the swatches.
    pub looks: [TraceLook; 3],
    /// The plot background the old traces fade toward. The plot does not
    /// paint its background, so this is the terminal background (or the
    /// preset's).
    pub background: Rgb,
    /// The text colour.
    pub foreground: Rgb,
    /// RGB of the 16 ANSI colours.
    pub ansi: [Rgb; 16],
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
    /// Key hints: quieter than the labels.
    pub hint: Color,
}

impl Palette {
    /// The palette of a colour mode with the dark preset and the default
    /// trace colours, with no `COLORTERM`.
    pub fn new(mode: ColorMode) -> Self {
        Self::build(mode, None, &DARK, &DEFAULT_LOOKS)
    }

    /// The palette of colour `mode` with colours `scheme` and trace looks
    /// `traces`. `colorterm` is the `COLORTERM` value: it picks the depth
    /// of auto mode. The modes only differ in how many colours they
    /// write, so auto looks the same as the mode it picks.
    pub fn build(
        mode: ColorMode,
        colorterm: Option<&str>,
        scheme: &Scheme,
        traces: &[TraceLook; 3],
    ) -> Self {
        let depth = mode.resolve(colorterm);
        let (fg, bg) = (scheme.fg, scheme.bg);
        let mut p = Self {
            trace: [Color::Reset; 3],
            line: [Color::Reset; 3],
            mode: depth,
            scheme: *scheme,
            trace_rgb: [bg; 3],
            line_rgb: [bg; 3],
            trace16: [11, 12, 13],
            trace_modifier: [Modifier::empty(); 3],
            looks: *traces,
            background: bg,
            foreground: fg,
            ansi: scheme.ansi,
            grid: Color::Reset,
            axis: Color::Reset,
            cursor: Color::Reset,
            xy: Color::Reset,
            trigger: Color::Reset,
            label: Color::Reset,
            help: Color::Reset,
            hint: Color::Reset,
        };
        p.grid = p.made(blend(fg, bg, THEMED_GRID));
        p.axis = p.made(blend(fg, bg, THEMED_AXIS));
        p.cursor = p.made(blend(fg, bg, THEMED_CURSOR));
        p.label = p.made(blend(fg, bg, THEMED_LABEL));
        p.help = p.made(blend(blend(scheme.ansi[4], fg, 0.5), bg, 0.85));
        p.hint = p.made(blend(fg, bg, THEMED_HINT));
        p.xy = p.slot(10);
        p.trigger = p.slot(9);
        for (t, look) in traces.iter().enumerate() {
            let (c, rgb, n, m) = p.trace_of(*look);
            p.line[t] = c;
            p.line_rgb[t] = rgb;
            p.trace16[t] = n;
            p.trace_modifier[t] = m;
            // The text keeps the plain colour.
            let (c, rgb, _, _) = p.trace_of(TraceLook::plain(look.color));
            p.trace[t] = c;
            p.trace_rgb[t] = rgb;
        }
        p
    }

    /// ANSI colour `n`: by name for the terminal's own colours and in 16
    /// colours, else the preset's RGB.
    fn slot(&self, n: u8) -> Color {
        self.named(ansi_color(n))
    }

    /// True when colours may be sent by name: the terminal's own colours,
    /// or 16 colours. A preset in true colour or 256 colours sends RGB.
    pub fn sends_names(&self) -> bool {
        self.scheme.named || self.mode == ColorMode::Ansi16
    }

    /// `c` as this palette sends it: a named ANSI colour becomes the
    /// scheme's RGB for it when names are not sent. Other colours stay.
    pub fn named(&self, c: Color) -> Color {
        if self.sends_names() {
            return c;
        }
        match (0..16u8).find(|&n| ansi_color(n) == c) {
            Some(n) => self.emit(self.ansi[n as usize]),
            None => c,
        }
    }

    /// A colour made from the scheme (grid, labels, ...): RGB, the
    /// nearest 256-colour index, or in 16 colours the nearest ANSI colour
    /// by name. That one is never the colour of the background itself.
    fn made(&self, c: Rgb) -> Color {
        if self.mode != ColorMode::Ansi16 {
            return self.emit(c);
        }
        let bg = nearest16(self.background, &self.ansi);
        let n = (0..16u8)
            .filter(|&n| n != bg)
            .min_by(|&a, &b| {
                distance(c, self.ansi[a as usize]).total_cmp(&distance(c, self.ansi[b as usize]))
            })
            .unwrap_or(7);
        ansi_color(n)
    }

    /// A trace that looks like `look`: the colour to draw, its RGB (for
    /// the fades), the nearest ANSI colour, and the modifier to draw it
    /// with. With the terminal's own colours, a name with brightness 0
    /// and full saturation stays the name, so the terminal theme picks
    /// the shade. Else the colour is made from the RGB of the name or the
    /// RGB colour, and written as RGB or the nearest 256-colour index. 16
    /// colours can only send names: an RGB colour is the nearest name,
    /// saturation does nothing, and a high or low brightness is the BOLD
    /// or DIM modifier.
    fn trace_of(&self, look: TraceLook) -> (Color, Rgb, u8, Modifier) {
        let rgb = match look.color {
            TraceColor::Ansi(n) => self.ansi[(n & 15) as usize],
            TraceColor::Rgb(c) => c,
        };
        if self.mode == ColorMode::Ansi16 {
            let n = match look.color {
                TraceColor::Ansi(n) => n & 15,
                TraceColor::Rgb(c) => nearest16(c, &self.ansi),
            };
            return (ansi_color(n), rgb, n, look.modifier16());
        }
        if let (TraceColor::Ansi(n), true) = (look.color, look.is_plain() && self.scheme.named) {
            return (ansi_color(n & 15), rgb, n & 15, Modifier::empty());
        }
        let c = adjust(
            rgb,
            self.background,
            self.foreground,
            look.brightness,
            look.saturation,
        );
        (self.emit(c), c, nearest16(c, &self.ansi), Modifier::empty())
    }

    /// The colour trace `t` would have with colour `tc` and its own
    /// brightness and saturation: for the swatches in the settings panel.
    pub fn trace_color(&self, t: usize, tc: TraceColor) -> Color {
        let look = TraceLook {
            color: tc,
            ..self.looks[t.min(2)]
        };
        self.trace_of(look).0
    }

    /// `c` written the way of this palette: true colour or the nearest
    /// 256-colour index.
    fn emit(&self, c: Rgb) -> Color {
        emit(self.mode, c)
    }

    /// Colour of old trace `age` (0 = newest old frame) of trace `t`, with
    /// `depth` old frames. The newest is close to the trace colour, the
    /// oldest close to the background. True colour fades in even steps,
    /// 256 colours take the nearest xterm colour. 16 colours have two
    /// steps: the plain colour for the newer half, and the same colour
    /// with `true` (draw it with the DIM modifier) for the older half, or
    /// for all of them when the trace itself is dim. A bold trace's old
    /// traces are not bold.
    pub fn persist(&self, t: usize, age: usize, depth: usize) -> (Color, bool) {
        let t = t.min(2);
        if self.mode == ColorMode::Ansi16 {
            let plain = ansi_color(plain16(self.trace16[t]));
            let dim = self.trace_modifier[t].contains(Modifier::DIM);
            return (plain, dim || (depth > 1 && age * 2 >= depth));
        }
        let rgb = blend(self.line_rgb[t], self.background, fade_level(age, depth));
        match self.mode {
            ColorMode::TrueColor => (Color::Rgb(rgb.0, rgb.1, rgb.2), false),
            _ => (Color::Indexed(xterm_tint(rgb)), false),
        }
    }

    /// The muted colour of trace `t`: its peak area under the bright mean
    /// line (trace mode both). Halfway to the background; in 16 colours
    /// the plain (not light) colour.
    pub fn muted(&self, t: usize) -> Color {
        if self.mode == ColorMode::Ansi16 {
            return ansi_color(plain16(self.trace16[t.min(2)]));
        }
        self.persist(t, 2, 4).0
    }

    /// Grid and center cross colours at grid brightness `percent` (10 to
    /// 100), and whether the grid needs the DIM modifier. At
    /// [`GRID_BRIGHTNESS_DEFAULT`] they are [`Palette::grid`] and
    /// [`Palette::axis`]. They go from the background toward the text
    /// colour. In 16 colours each is the nearest name, and the lowest
    /// steps are dim.
    pub fn grid_colors(&self, percent: u8) -> (Color, Color, bool) {
        let k = percent.clamp(1, 100) as f64 / GRID_BRIGHTNESS_DEFAULT as f64;
        let (fg, bg) = (self.foreground, self.background);
        (
            self.made(blend(fg, bg, THEMED_GRID * k)),
            self.made(blend(fg, bg, THEMED_AXIS * k)),
            self.mode == ColorMode::Ansi16 && percent <= 20,
        )
    }

    /// The palette of the display settings `d`, with the `COLORTERM` of
    /// this process and the colours the terminal told us at start.
    pub fn of(d: &DisplaySettings) -> Self {
        let term = crate::termcolors::detected();
        let (scheme, _) = Scheme::of(d.colortheme, term.as_ref(), colorfgbg());
        Self::build(d.colors, colorterm(), &scheme, &TraceLook::of(d))
    }
}

/// The default trace looks: [`TraceColor::DEFAULTS`], plain.
pub const DEFAULT_LOOKS: [TraceLook; 3] = [
    TraceLook::plain(TraceColor::DEFAULTS[0]),
    TraceLook::plain(TraceColor::DEFAULTS[1]),
    TraceLook::plain(TraceColor::DEFAULTS[2]),
];

/// `c` as true colour, or as the nearest 256-colour index for any other
/// mode.
fn emit(mode: ColorMode, c: Rgb) -> Color {
    match mode {
        ColorMode::TrueColor => Color::Rgb(c.0, c.1, c.2),
        _ => Color::Indexed(xterm_index(c)),
    }
}

/// `COLORFGBG`, read once.
pub fn colorfgbg() -> Option<&'static str> {
    static V: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    V.get_or_init(|| std::env::var("COLORFGBG").ok()).as_deref()
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

    /// Gruvbox light and dark, as a terminal would tell them.
    fn gruvbox(light: bool) -> TermColors {
        let hex = |h: &str| parse_hex(h).unwrap();
        let (fg, bg, ansi) = if light {
            (
                "504945",
                "fbf1c7",
                [
                    "fbf1c7", "9d0006", "79740e", "b57614", "076678", "8f3f71", "427b58", "504945",
                    "d5c4a1", "9d0006", "79740e", "b57614", "076678", "8f3f71", "427b58", "282828",
                ],
            )
        } else {
            (
                "d5c4a1",
                "282828",
                [
                    "282828", "fb4934", "b8bb26", "fabd2f", "83a598", "d3869b", "8ec07c", "d5c4a1",
                    "504945", "fb4934", "b8bb26", "fabd2f", "83a598", "d3869b", "8ec07c", "fbf1c7",
                ],
            )
        };
        TermColors {
            fg: Some(hex(fg)),
            bg: Some(hex(bg)),
            ansi: ansi.map(|a| Some(hex(a))),
        }
    }

    fn rgb_of(c: Color) -> Rgb {
        match c {
            Color::Rgb(r, g, b) => (r, g, b),
            _ => panic!("{c:?}"),
        }
    }

    /// Perceptual lightness, 0..1.
    fn light(c: Rgb) -> f64 {
        oklab(c)[0]
    }

    const AUTO3: [TraceLook; 3] = DEFAULT_LOOKS;

    fn plain3(c: [TraceColor; 3]) -> [TraceLook; 3] {
        c.map(TraceLook::plain)
    }

    #[test]
    fn trace_colour_names_parse_and_print() {
        for gone in ["auto", "", "default"] {
            assert!(TraceColor::parse(gone).is_err(), "{gone}");
        }
        assert_eq!(TraceColor::parse("yellow"), Ok(TraceColor::Ansi(3)));
        assert_eq!(TraceColor::parse("Bright_Yellow"), Ok(TraceColor::Ansi(11)));
        assert_eq!(TraceColor::parse("dark-grey"), Ok(TraceColor::Ansi(8)));
        assert_eq!(TraceColor::parse("white"), Ok(TraceColor::Ansi(15)));
        assert_eq!(TraceColor::parse("#FF8800"), Ok(TraceColor::Rgb((255, 136, 0))));
        for bad in ["#ff88", "#gg0000", "orange", "#ff880000", "light yellow"] {
            assert!(TraceColor::parse(bad).is_err(), "{bad}");
        }
        for c in TraceColor::choices()
            .into_iter()
            .chain([TraceColor::Rgb((1, 2, 254))])
        {
            assert_eq!(TraceColor::parse(&c.name()), Ok(c));
        }
        assert_eq!(TraceColor::choices().len(), 16);
        let names: Vec<String> = TraceColor::DEFAULTS.iter().map(|c| c.name()).collect();
        assert_eq!(names, ["bright-yellow", "bright-blue", "bright-magenta"]);
        assert_eq!(TraceColor::Rgb((1, 2, 254)).name(), "#0102fe");
    }

    #[test]
    fn brightness_and_saturation_maths() {
        let bg = (0x28, 0x28, 0x28);
        let fg = (0xeb, 0xdb, 0xb2);
        let k = |c: Rgb| {
            let o = oklab(c);
            o[1].hypot(o[2])
        };
        let in_gamut = |c: Rgb| {
            let lin = oklab_to_linear(oklab(c));
            lin.iter().all(|v| (-0.01..=1.01).contains(v))
        };
        for c in [(0xfa, 0xbd, 0x2f), (0x83, 0xa5, 0x98), (255, 0, 0), (10, 200, 90)] {
            // Brightness 0 and saturation 100: the colour itself.
            assert_eq!(adjust(c, bg, fg, 0, 100), c);
            // Brightness -100: the background.
            assert_eq!(adjust(c, bg, fg, -100, 100), bg);
            assert_eq!(adjust(c, bg, fg, -100, 400), bg);
            // Brightness -40: between the colour and the background, as
            // the old dimness 40.
            let d = adjust(c, bg, fg, -40, 100);
            let (lc, lb, ld) = (light(c), light(bg), light(d));
            assert!(ld < lc && ld > lb, "{c:?} {d:?}");
            assert_eq!(d, blend(c, bg, 0.6));
            // Above 0: lighter on a dark background, up to the lightness
            // of the text colour at 100, with the hue kept.
            let lf = light(fg);
            let up = adjust(c, bg, fg, 60, 100);
            let full = adjust(c, bg, fg, 100, 100);
            if lc < lf {
                assert!(light(up) > lc && light(up) < lf, "{c:?} {up:?}");
                assert!((light(full) - lf).abs() < 0.01, "{c:?} {full:?}");
            } else {
                // As light as the text already: toward white.
                assert!(light(up) > lc, "{c:?} {up:?}");
                assert!(light(full) > 0.99, "{c:?} {full:?}");
            }
            assert!(in_gamut(up) && in_gamut(full), "{up:?} {full:?}");
            let hue = |c: Rgb| {
                let o = oklab(c);
                o[2].atan2(o[1])
            };
            if k(c) > 0.05 && k(full) > 0.02 {
                assert!((hue(full) - hue(c)).abs() < 0.1, "{c:?} {full:?}");
            }
            // A light background: darker, toward the dark text colour.
            let (lbg, lfg) = ((0xfb, 0xf1, 0xc7), (0x3c, 0x38, 0x36));
            let dark = adjust(c, lbg, lfg, 100, 100);
            if light(c) > light(lfg) {
                assert!((light(dark) - light(lfg)).abs() < 0.01, "{c:?} {dark:?}");
            }
            assert!(light(adjust(c, lbg, lfg, 50, 100)) <= lc + 1e-3, "{c:?}");
            // Saturation 0: a gray of the same Oklab lightness.
            let (r, g, b) = saturate(c, 0.0);
            assert!(r.abs_diff(g) <= 1 && g.abs_diff(b) <= 1, "{c:?} {r} {g} {b}");
            assert!((light((r, g, b)) - lc).abs() < 0.005, "{c:?}");
            // Saturation 200 and 400: more chroma, same lightness, in the
            // gamut.
            for f in [2.0, 4.0] {
                let more = saturate(c, f);
                assert!(k(more) >= k(c) - 1e-3, "{c:?} {more:?}");
                assert!((light(more) - lc).abs() < 0.01, "{c:?} {more:?}");
                assert!(in_gamut(more), "{more:?}");
            }
        }
        // Pure red is at the gamut edge: a boost stays red.
        assert_eq!(saturate((255, 0, 0), 2.0), (255, 0, 0));
        assert_eq!(saturate((255, 0, 0), 4.0), (255, 0, 0));
        // A muted colour gets more colourful, with the same hue, and stops
        // at the edge of the gamut.
        let m = (150, 120, 100);
        let more = saturate(m, 1.8);
        assert!(more.0 > m.0 && more.2 < m.2, "{more:?}");
        assert!(k(saturate(m, 4.0)) >= k(more));
        // White cannot get any lighter.
        let w = adjust((255, 255, 255), (0, 0, 0), (255, 255, 255), 100, 100);
        assert_eq!(w, (255, 255, 255));
        // Near white the chroma goes, as the gamut has no room for it.
        let y = adjust((0xfa, 0xbd, 0x2f), bg, (0x80, 0x80, 0x80), 90, 100);
        assert!(light(y) > 0.95 && k(y) < k((0xfa, 0xbd, 0x2f)), "{y:?}");
        assert!(in_gamut(y), "{y:?}");
    }

    #[test]
    fn fade_levels() {
        assert_eq!(fade_level(0, 8), FADE_NEWEST);
        assert_eq!(fade_level(7, 8), FADE_OLDEST);
        assert_eq!(fade_level(0, 1), FADE_NEWEST);
        assert_eq!(fade_level(1, 2), FADE_OLDEST);
        // Older traces are ever closer to the background.
        for depth in [2, 4, 8, 16] {
            for t in [(255, 214, 10), (0, 200, 255), (220, 90, 255)] {
                let mut last = f64::INFINITY;
                for age in 0..depth {
                    let l = light(blend(t, DARK.bg, fade_level(age, depth)));
                    assert!(l < last, "depth {depth} age {age}");
                    last = l;
                }
            }
        }
    }

    /// Every colour a palette hands out, with what it is for.
    fn all_colours(p: &Palette) -> Vec<(String, Color)> {
        let mut v = vec![
            ("grid".into(), p.grid),
            ("axis".into(), p.axis),
            ("cursor".into(), p.cursor),
            ("xy".into(), p.xy),
            ("trigger".into(), p.trigger),
            ("label".into(), p.label),
            ("help".into(), p.help),
            ("hint".into(), p.hint),
        ];
        for t in 0..3 {
            v.push((format!("trace {t}"), p.trace[t]));
            v.push((format!("line {t}"), p.line[t]));
            v.push((format!("muted {t}"), p.muted(t)));
            for age in 0..4 {
                v.push((format!("persist {t} {age}"), p.persist(t, age, 4).0));
            }
            for n in 0..16 {
                v.push((format!("swatch {t} {n}"), p.trace_color(t, TraceColor::Ansi(n))));
            }
        }
        for b in crate::settings::GRID_BRIGHTNESS {
            let (g, a, _) = p.grid_colors(b);
            v.push((format!("grid {b}"), g));
            v.push((format!("axis {b}"), a));
        }
        v
    }

    fn is_named(c: Color) -> bool {
        !matches!(c, Color::Rgb(..) | Color::Indexed(_) | Color::Reset)
    }

    #[test]
    fn presets_and_the_fallback() {
        let term = gruvbox(false);
        for t in [None, Some(&term)] {
            assert_eq!(Scheme::of(ColorTheme::Light, t, None), (LIGHT, false));
            assert_eq!(Scheme::of(ColorTheme::Dark, t, Some("0;15")), (DARK, false));
        }
        // The terminal's colours, sent by name.
        let (s, fell) = Scheme::of(ColorTheme::Terminal, Some(&term), Some("0;15"));
        assert!(!fell && s.named);
        assert_eq!((s.fg, s.bg), (term.fg.unwrap(), term.bg.unwrap()));
        assert_eq!(s.ansi[11], term.ansi[11].unwrap());
        // No answer: a preset, picked from COLORFGBG.
        let half = TermColors {
            bg: Some((255, 255, 255)),
            ..TermColors::default()
        };
        for t in [None, Some(&half)] {
            for (v, want) in [
                (None, DARK),
                (Some("0;15"), LIGHT),
                (Some("0;7"), LIGHT),
                (Some("default;default;15"), LIGHT),
                (Some("15;0"), DARK),
                (Some("7;8"), DARK),
                (Some("junk"), DARK),
            ] {
                assert_eq!(Scheme::of(ColorTheme::Terminal, t, v), (want, true), "{v:?}");
            }
        }
        // The note on the message line.
        let note = fallback_note(ColorTheme::Terminal, None, None).unwrap();
        assert_eq!(note, "No colour answer from the terminal; using the dark preset");
        assert!(fallback_note(ColorTheme::Terminal, None, Some("0;15"))
            .unwrap()
            .contains("light preset"));
        assert_eq!(fallback_note(ColorTheme::Terminal, Some(&term), None), None);
        assert_eq!(fallback_note(ColorTheme::Dark, None, None), None);
        assert_eq!(fallback_note(ColorTheme::Light, None, None), None);
    }

    #[test]
    fn presets_are_readable() {
        for s in [DARK, LIGHT] {
            let bg = light(s.bg);
            // Text, labels and the default traces stand out.
            assert!((light(s.fg) - bg).abs() > 0.6, "{s:?}");
            for n in [9, 10, 11, 12, 13, 14] {
                assert!((light(s.ansi[n]) - bg).abs() > 0.3, "{n} {s:?}");
            }
            // The traces are colourful.
            for n in [11, 12, 13] {
                let o = oklab(s.ansi[n]);
                assert!(o[1].hypot(o[2]) > 0.1, "{n} {s:?}");
            }
        }
    }

    #[test]
    fn auto_looks_like_the_mode_it_picks() {
        let term = gruvbox(false);
        let mut looks = AUTO3;
        looks[0].brightness = -30;
        looks[1].color = TraceColor::Rgb((255, 136, 0));
        looks[2].saturation = 150;
        let schemes = [
            Scheme::of(ColorTheme::Terminal, Some(&term), None).0,
            Scheme::of(ColorTheme::Terminal, None, None).0,
            LIGHT,
            DARK,
        ];
        for s in schemes {
            for l in [AUTO3, looks] {
                for ct in [None, Some("truecolor"), Some("24bit")] {
                    let a = Palette::build(ColorMode::Auto, ct, &s, &l);
                    let m = Palette::build(ColorMode::Auto.resolve(ct), ct, &s, &l);
                    assert_eq!(a, m, "{ct:?}");
                    assert_eq!(all_colours(&a), all_colours(&m));
                }
            }
        }
    }

    #[test]
    fn presets_send_no_names() {
        let mut looks = AUTO3;
        looks[0].brightness = -60;
        looks[1].brightness = 60;
        for s in [DARK, LIGHT] {
            for l in [AUTO3, looks] {
                let t = Palette::build(ColorMode::TrueColor, None, &s, &l);
                for (what, c) in all_colours(&t) {
                    assert!(matches!(c, Color::Rgb(..)), "{what}: {c:?}");
                }
                let i = Palette::build(ColorMode::Ansi256, None, &s, &l);
                for (what, c) in all_colours(&i) {
                    assert!(matches!(c, Color::Indexed(_)), "{what}: {c:?}");
                }
                assert_eq!(t.trace_modifier, [Modifier::empty(); 3]);
            }
            // A trace name is the preset's colour for it.
            let t = Palette::build(ColorMode::TrueColor, None, &s, &AUTO3);
            // So is any other name the screen draws.
            let w = s.ansi[15];
            assert!(!t.sends_names());
            assert_eq!(t.named(Color::White), Color::Rgb(w.0, w.1, w.2));
            assert_eq!(t.named(Color::Rgb(1, 2, 3)), Color::Rgb(1, 2, 3));
            assert_eq!(t.named(Color::Reset), Color::Reset);
            let n = Palette::build(ColorMode::Ansi16, None, &s, &AUTO3);
            assert!(n.sends_names());
            assert_eq!(n.named(Color::White), Color::White);
            let y = s.ansi[11];
            assert_eq!(t.trace[0], Color::Rgb(y.0, y.1, y.2));
            assert_eq!(t.trace_rgb, [s.ansi[11], s.ansi[12], s.ansi[13]]);
            let i = Palette::build(ColorMode::Ansi256, None, &s, &AUTO3);
            assert_eq!(i.trace[0], Color::Indexed(xterm_index(y)));
        }
    }

    #[test]
    fn terminal_colours() {
        for is_light in [true, false] {
            let term = gruvbox(is_light);
            let (s, _) = Scheme::of(ColorTheme::Terminal, Some(&term), None);
            let p = Palette::build(ColorMode::TrueColor, None, &s, &AUTO3);
            // Traces: the terminal's own bright colours, by name.
            assert_eq!(
                p.trace,
                [Color::LightYellow, Color::LightBlue, Color::LightMagenta]
            );
            assert_eq!((p.xy, p.trigger), (Color::LightGreen, Color::LightRed));
            assert!(p.sends_names());
            assert_eq!(p.named(Color::White), Color::White);
            assert_eq!(p.trace_rgb[0], term.ansi[11].unwrap());
            let (fg, bg) = (term.fg.unwrap(), term.bg.unwrap());
            assert_eq!(p.background, bg);
            // Grid, center cross, labels and the cursor: ever closer to
            // the text colour, all between the background and the text.
            let steps: Vec<f64> = [p.grid, p.axis, p.hint, p.label, p.cursor]
                .iter()
                .map(|c| (light(rgb_of(*c)) - light(bg)).abs())
                .collect();
            for w in steps.windows(2) {
                assert!(w[0] < w[1], "{steps:?}");
            }
            let full = (light(fg) - light(bg)).abs();
            assert!(steps[0] > 0.1 * full && steps[4] < full, "{steps:?}");
            // Labels stand out from the background.
            assert!(steps[3] > 0.6 * full);
            // Old traces fade toward the background: lighter on a light
            // theme, darker on a dark one.
            let fades: Vec<f64> = (0..4).map(|a| light(rgb_of(p.persist(0, a, 4).0))).collect();
            for w in fades.windows(2) {
                assert_eq!(w[0] < w[1], is_light, "{fades:?}");
            }
            let m = light(rgb_of(p.muted(0)));
            assert!((m - light(bg)).abs() < (light(p.trace_rgb[0]) - light(bg)).abs());
            // Grid brightness: closer to the text at higher settings.
            let g = |b| light(rgb_of(p.grid_colors(b).0));
            assert_eq!(g(10) < g(100), !is_light);
            assert_eq!(p.grid_colors(GRID_BRIGHTNESS_DEFAULT), (p.grid, p.axis, false));
            // 256 colours: the same names, the rest as indexes.
            let q = Palette::build(ColorMode::Ansi256, None, &s, &AUTO3);
            assert_eq!(q.trace, p.trace);
            assert!(matches!(q.label, Color::Indexed(_)));
            assert!(matches!(q.persist(1, 1, 4).0, Color::Indexed(_)));
        }
    }

    #[test]
    fn trace_colour_overrides() {
        let pink = TraceColor::Rgb((255, 0, 255));
        let over = plain3([pink, TraceColor::Ansi(9), TraceColor::Ansi(13)]);
        let term = Scheme::of(ColorTheme::Terminal, Some(&gruvbox(true)), None).0;
        // True colour: exact, with any colour source.
        for s in [term, DARK, LIGHT] {
            let t = Palette::build(ColorMode::TrueColor, None, &s, &over);
            assert_eq!(t.trace[0], Color::Rgb(255, 0, 255));
            // The fades of an override come from its colour.
            assert_eq!(t.trace_rgb[0], (255, 0, 255));
            let i = Palette::build(ColorMode::Ansi256, None, &s, &over);
            assert_eq!(i.trace[0], Color::Indexed(201));
        }
        let t = Palette::build(ColorMode::TrueColor, None, &DARK, &over);
        let (r, g, b) = rgb_of(t.persist(0, 1, 4).0);
        assert!(r.abs_diff(b) < 40 && g < r / 2 && r < 255, "{r} {g} {b}");
        // A name: by name with the terminal's colours, else the preset's.
        let a = Palette::build(ColorMode::TrueColor, None, &term, &over);
        assert_eq!(a.trace[1], Color::LightRed);
        assert_eq!(a.trace_rgb[1], (0x9d, 0x00, 0x06));
        let r = DARK.ansi[9];
        assert_eq!(t.trace[1], Color::Rgb(r.0, r.1, r.2));
        // Swatches show what a choice would draw.
        assert_eq!(a.trace_color(0, TraceColor::Ansi(2)), Color::Green);
        let gr = DARK.ansi[2];
        assert_eq!(t.trace_color(0, TraceColor::Ansi(2)), Color::Rgb(gr.0, gr.1, gr.2));
    }

    #[test]
    fn brightness_and_saturation_of_traces() {
        let term = gruvbox(false);
        let s = Scheme::of(ColorTheme::Terminal, Some(&term), None).0;
        let mut looks = AUTO3;
        looks[0].brightness = -40;
        looks[1].saturation = 180;
        // RGB from the theme's own colour.
        let p = Palette::build(ColorMode::TrueColor, None, &s, &looks);
        let (bg, fg) = (term.bg.unwrap(), term.fg.unwrap());
        let want0 = adjust(term.ansi[11].unwrap(), bg, fg, -40, 100);
        let want1 = adjust(term.ansi[12].unwrap(), bg, fg, 0, 180);
        assert_eq!(p.line[0], Color::Rgb(want0.0, want0.1, want0.2));
        assert_eq!(p.line[1], Color::Rgb(want1.0, want1.1, want1.2));
        // MATH has neither: it stays the name.
        assert_eq!(p.line[2], Color::LightMagenta);
        assert_eq!(p.line_rgb[0], want0);
        assert_eq!(p.trace_modifier, [Modifier::empty(); 3]);
        // The fades and the muted area come from the dimmed colour.
        let plain = Palette::build(ColorMode::TrueColor, None, &s, &AUTO3);
        assert_ne!(p.persist(0, 1, 4), plain.persist(0, 1, 4));
        assert_eq!(
            p.persist(0, 1, 4).0,
            emit(ColorMode::TrueColor, blend(want0, term.bg.unwrap(), fade_level(1, 4)))
        );
        assert_ne!(p.muted(0), plain.muted(0));
        // The swatch of another name uses the same brightness.
        let sw = rgb_of(p.trace_color(0, TraceColor::Ansi(9)));
        assert_eq!(sw, adjust(term.ansi[9].unwrap(), bg, fg, -40, 100));
        // A brighter trace: toward the text colour, and so are its fades.
        let mut up = looks;
        up[0].brightness = 60;
        let u = Palette::build(ColorMode::TrueColor, None, &s, &up);
        let want = adjust(term.ansi[11].unwrap(), bg, fg, 60, 100);
        assert_eq!(u.line[0], Color::Rgb(want.0, want.1, want.2));
        assert!(light(want) > light(term.ansi[11].unwrap()));
        assert_eq!(u.line_rgb[0], want);
        // 256 colours: the nearest index.
        let q = Palette::build(ColorMode::Ansi256, None, &s, &looks);
        assert_eq!(q.line[0], Color::Indexed(xterm_index(want0)));
        // A preset: toward its own background.
        let t = Palette::build(ColorMode::TrueColor, None, &LIGHT, &looks);
        let w = adjust(LIGHT.ansi[11], LIGHT.bg, LIGHT.fg, -40, 100);
        assert_eq!(t.line[0], Color::Rgb(w.0, w.1, w.2));
        // An RGB colour is dimmed too.
        let mut rgb = looks;
        rgb[2] = TraceLook {
            color: TraceColor::Rgb((255, 0, 255)),
            brightness: -100,
            saturation: 100,
        };
        let r = Palette::build(ColorMode::TrueColor, None, &DARK, &rgb);
        assert_eq!(r.line[2], Color::Rgb(DARK.bg.0, DARK.bg.1, DARK.bg.2));
    }

    #[test]
    fn brightness_and_saturation_change_only_the_line() {
        let term = gruvbox(false);
        let s = Scheme::of(ColorTheme::Terminal, Some(&term), None).0;
        let mut looks = AUTO3;
        looks[0].brightness = -60;
        looks[1].saturation = 40;
        for scheme in [s, DARK, LIGHT] {
            for mode in [ColorMode::TrueColor, ColorMode::Ansi256] {
                let plain = Palette::build(mode, None, &scheme, &AUTO3);
                // At the defaults the text and the line are the same.
                assert_eq!(plain.trace, plain.line);
                assert_eq!(plain.trace_rgb, plain.line_rgb);
                let p = Palette::build(mode, None, &scheme, &looks);
                // The text keeps the plain colour.
                assert_eq!(p.trace, plain.trace);
                assert_eq!(p.trace_rgb, plain.trace_rgb);
                // The line has the brightness and the saturation.
                assert_ne!(p.line[0], p.trace[0]);
                assert_ne!(p.line[1], p.trace[1]);
                assert_eq!(p.line[2], p.trace[2]);
                assert_ne!(p.line_rgb[0], p.trace_rgb[0]);
            }
        }
        // The terminal theme: the text stays the name.
        let p = Palette::build(ColorMode::TrueColor, None, &s, &looks);
        assert_eq!(p.trace, [Color::LightYellow, Color::LightBlue, Color::LightMagenta]);
        assert!(matches!(p.line[0], Color::Rgb(..)));
    }

    #[test]
    fn sixteen_colours_send_names_only() {
        let mut looks = AUTO3;
        looks[0].brightness = -49;
        looks[1].brightness = -50;
        looks[2].saturation = 0;
        let term = Scheme::of(ColorTheme::Terminal, Some(&gruvbox(false)), None).0;
        for s in [term, DARK, LIGHT] {
            for l in [AUTO3, looks] {
                let p = Palette::build(ColorMode::Ansi16, Some("truecolor"), &s, &l);
                for (what, c) in all_colours(&p) {
                    assert!(is_named(c), "{what}: {c:?}");
                }
                // Nothing made from the scheme is the background itself.
                let bg = ansi_color(nearest16(s.bg, &s.ansi));
                for c in [p.grid, p.axis, p.label, p.cursor, p.hint, p.help] {
                    assert_ne!(c, bg, "{s:?}");
                }
            }
            let p = Palette::build(ColorMode::Ansi16, Some("truecolor"), &s, &looks);
            // The names stay; saturation does nothing.
            assert_eq!(
                p.trace,
                [Color::LightYellow, Color::LightBlue, Color::LightMagenta]
            );
            // Brightness -50 and down is the DIM modifier.
            assert_eq!(
                p.trace_modifier,
                [Modifier::empty(), Modifier::DIM, Modifier::empty()]
            );
            // Old traces: the plain colour, dim for the older half, and
            // every old trace of a dim trace.
            assert_eq!(p.persist(1, 0, 8), (Color::Blue, true));
            assert_eq!(p.persist(0, 0, 8), (Color::Yellow, false));
            assert_eq!(p.persist(0, 4, 8), (Color::Yellow, true));
            assert_eq!(p.muted(0), Color::Yellow);
            // The lowest grid steps are dim.
            assert!(p.grid_colors(10).2);
            assert!(!p.grid_colors(100).2);
            assert_eq!(p.grid_colors(GRID_BRIGHTNESS_DEFAULT), (p.grid, p.axis, false));
        }
        // Brightness 50 and up is the BOLD modifier, below that nothing.
        // Old traces of a bold trace are not bold.
        let mut up = AUTO3;
        up[0].brightness = 50;
        up[1].brightness = 49;
        up[2].brightness = 100;
        let p = Palette::build(ColorMode::Ansi16, None, &DARK, &up);
        assert_eq!(
            p.trace_modifier,
            [Modifier::BOLD, Modifier::empty(), Modifier::BOLD]
        );
        assert_eq!(p.trace, [Color::LightYellow, Color::LightBlue, Color::LightMagenta]);
        assert_eq!(p.persist(0, 0, 8), (Color::Yellow, false));
        // In other modes the brightness is in the colour, with no modifier.
        let t = Palette::build(ColorMode::TrueColor, None, &DARK, &up);
        assert_eq!(t.trace_modifier, [Modifier::empty(); 3]);
        // An RGB colour is the nearest name of the colours in use.
        let orange = plain3([
            TraceColor::Rgb((0xb8, 0x88, 0x20)),
            TraceColor::Ansi(12),
            TraceColor::Ansi(13),
        ]);
        let l = Palette::build(ColorMode::Ansi16, None, &LIGHT, &orange);
        assert_eq!(l.trace[0], Color::LightYellow);
        let d = Palette::build(ColorMode::Ansi16, None, &DARK, &orange);
        assert_eq!(d.trace[0], Color::Yellow);
        // Dark gray fades as itself, not black.
        let g = Palette::build(ColorMode::Ansi16, None, &DARK, &plain3([TraceColor::Ansi(8); 3]));
        assert_eq!(g.muted(0), Color::DarkGray);
    }

    #[test]
    fn blend_endpoints_and_nearest() {
        let (a, b) = ((255, 214, 10), (251, 241, 199));
        assert_eq!(blend(a, b, 1.0), a);
        assert_eq!(blend(a, b, 0.0), b);
        assert_eq!(nearest16((250, 250, 250), &XTERM16), 15);
        assert_eq!(nearest16((10, 0, 0), &XTERM16), 0);
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
