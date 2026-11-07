//! Screen colours. True colour when the terminal has it, else the
//! 256-colour or 16-colour palette.

use ratatui::style::Color;
use serde::{Deserialize, Serialize};

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

/// All colours the screen uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// CH1, CH2, MATH.
    pub trace: [Color; 3],
    /// Old traces (persistence): the trace colours dimmed.
    pub dim: [Color; 3],
    /// Grid dots at the division points.
    pub grid: Color,
    /// The center cross.
    pub axis: Color,
    pub cursor: Color,
    pub xy: Color,
    pub trigger: Color,
    /// Labels in the plot margins.
    pub label: Color,
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
                dim: [
                    Color::Rgb(95, 80, 10),
                    Color::Rgb(0, 72, 92),
                    Color::Rgb(80, 34, 92),
                ],
                grid: Color::Rgb(60, 60, 60),
                axis: Color::Rgb(105, 105, 105),
                cursor: Color::Rgb(210, 210, 210),
                xy: Color::Rgb(80, 250, 120),
                trigger: Color::Rgb(255, 95, 95),
                label: Color::Rgb(150, 150, 150),
            },
            ColorMode::Ansi256 => Self {
                trace: [Color::Indexed(220), Color::Indexed(45), Color::Indexed(171)],
                dim: [Color::Indexed(58), Color::Indexed(23), Color::Indexed(53)],
                grid: Color::Indexed(237),
                axis: Color::Indexed(242),
                cursor: Color::Indexed(252),
                xy: Color::Indexed(84),
                trigger: Color::Indexed(203),
                label: Color::Indexed(246),
            },
            ColorMode::Ansi16 => Self {
                trace: [Color::LightYellow, Color::LightCyan, Color::LightMagenta],
                dim: [Color::Yellow, Color::Cyan, Color::Magenta],
                grid: Color::DarkGray,
                axis: Color::Gray,
                cursor: Color::White,
                xy: Color::LightGreen,
                trigger: Color::LightRed,
                label: Color::Gray,
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

    #[test]
    fn dim_is_darker_than_trace() {
        let t = Palette::new(ColorMode::TrueColor);
        for (a, b) in t.trace.iter().zip(&t.dim) {
            let (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) = (a, b) else {
                panic!("not rgb");
            };
            let sum = |r: &u8, g: &u8, b: &u8| *r as u32 + *g as u32 + *b as u32;
            let (bright, dim) = (sum(r1, g1, b1), sum(r2, g2, b2));
            assert!(dim * 100 < bright * 45, "{dim} vs {bright}");
        }
    }
}
