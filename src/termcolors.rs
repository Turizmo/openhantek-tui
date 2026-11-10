//! Ask the terminal for its colours: the text colour (OSC 10), the
//! background (OSC 11) and the 16 ANSI colours (OSC 4). The `Terminal`
//! colour theme blends the old traces, the grid and the labels from them,
//! so they fit a light theme as well as a dark one.
//!
//! The query runs once at start, before the screen is set up. It writes
//! all questions at once and ends with a DA1 question (`CSI c`), which
//! every terminal answers. So a terminal that knows no colour questions
//! only costs the time of one answer. The wait has a fixed limit.

use std::sync::RwLock;
use std::time::{Duration, Instant};

use crate::theme::Rgb;

/// Colours the terminal told us.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermColors {
    /// Text colour.
    pub fg: Option<Rgb>,
    /// Background colour.
    pub bg: Option<Rgb>,
    /// ANSI colours 0 to 15.
    pub ansi: [Option<Rgb>; 16],
}

impl TermColors {
    /// True when the text and background colours are known: enough to
    /// blend colours that fit the theme.
    pub fn usable(&self) -> bool {
        self.fg.is_some() && self.bg.is_some()
    }
}

/// The detected colours, set once at start (and by tests).
static DETECTED: RwLock<Option<TermColors>> = RwLock::new(None);

/// The colours found by [`query`], if any.
pub fn detected() -> Option<TermColors> {
    DETECTED.read().ok().and_then(|d| *d)
}

/// Set the detected colours. The app does this once at start; tests use
/// it to draw with the colours of a given theme.
pub fn set_detected(c: Option<TermColors>) {
    if let Ok(mut d) = DETECTED.write() {
        *d = c;
    }
}

/// The text of all questions: OSC 10, OSC 11, OSC 4 for colours 0 to 15,
/// then DA1.
pub fn query_text() -> String {
    let mut q = String::from("\x1b]10;?\x07\x1b]11;?\x07");
    for i in 0..16 {
        q.push_str(&format!("\x1b]4;{i};?\x07"));
    }
    q.push_str("\x1b[c");
    q
}

/// Parse one colour value like `rgb:ffff/8080/0000` (1 to 4 hex digits
/// per part) or `#rrggbb`.
pub fn parse_color(text: &str) -> Option<Rgb> {
    let text = text.trim();
    if let Some(hex) = text.strip_prefix('#') {
        return crate::theme::parse_hex(hex);
    }
    let body = text
        .strip_prefix("rgb:")
        .or_else(|| text.strip_prefix("rgba:"))?;
    let parts: Vec<&str> = body.split('/').collect();
    if parts.len() < 3 {
        return None;
    }
    let one = |p: &str| -> Option<u8> {
        if p.is_empty() || p.len() > 4 || !p.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let v = u32::from_str_radix(p, 16).ok()?;
        let max = (1u32 << (4 * p.len())) - 1;
        Some(((v * 255 + max / 2) / max) as u8)
    };
    Some((one(parts[0])?, one(parts[1])?, one(parts[2])?))
}

/// What the answers so far hold.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Replies {
    pub colors: TermColors,
    /// The DA1 answer came: the terminal has answered everything.
    pub done: bool,
}

/// Parse the answers in `bytes`: OSC answers ended by BEL or ST
/// (`ESC \`), and the DA1 answer (`ESC [ ? ... c`). Anything else is
/// skipped.
pub fn parse_replies(bytes: &[u8]) -> Replies {
    let mut r = Replies::default();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != 0x1b || i + 1 >= bytes.len() {
            i += 1;
            continue;
        }
        match bytes[i + 1] {
            b']' => {
                // The body runs to BEL or ST.
                let start = i + 2;
                let mut j = start;
                let mut end = None;
                while j < bytes.len() {
                    if bytes[j] == 0x07 {
                        end = Some((j, j + 1));
                        break;
                    }
                    if bytes[j] == 0x1b && bytes.get(j + 1) == Some(&b'\\') {
                        end = Some((j, j + 2));
                        break;
                    }
                    // A new escape before the end: this answer is broken.
                    if bytes[j] == 0x1b {
                        break;
                    }
                    j += 1;
                }
                match end {
                    Some((stop, next)) => {
                        if let Ok(body) = std::str::from_utf8(&bytes[start..stop]) {
                            osc_body(body, &mut r.colors);
                        }
                        i = next;
                    }
                    None => i = j.max(i + 1),
                }
            }
            b'[' => {
                // A CSI: parameters, then one final byte.
                let mut j = i + 2;
                while j < bytes.len() && (0x20..0x40).contains(&bytes[j]) {
                    j += 1;
                }
                if j < bytes.len() {
                    if bytes[j] == b'c' && bytes.get(i + 2) == Some(&b'?') {
                        r.done = true;
                    }
                    i = j + 1;
                } else {
                    i = j;
                }
            }
            _ => i += 1,
        }
    }
    r
}

/// One OSC answer body: `10;rgb:...`, `11;rgb:...` or `4;n;rgb:...`.
fn osc_body(body: &str, c: &mut TermColors) {
    let mut parts = body.splitn(3, ';');
    match (parts.next(), parts.next(), parts.next()) {
        (Some("10"), Some(v), None) => c.fg = parse_color(v).or(c.fg),
        (Some("11"), Some(v), None) => c.bg = parse_color(v).or(c.bg),
        (Some("4"), Some(n), Some(v)) => {
            if let (Ok(n), Some(rgb)) = (n.parse::<usize>(), parse_color(v)) {
                if n < 16 {
                    c.ansi[n] = Some(rgb);
                }
            }
        }
        _ => {}
    }
}

/// Longest wait for all answers.
pub const QUERY_TIMEOUT: Duration = Duration::from_millis(150);

/// Ask the terminal for its colours. `None` when stdin or stdout is not a
/// terminal, the terminal cannot be opened, or nothing usable came back.
/// The terminal is in raw mode with no echo while it answers, so the
/// answers never show on screen and are all read from the input.
pub fn query() -> Option<TermColors> {
    // SAFETY: isatty only looks at the file descriptors.
    if unsafe { libc::isatty(0) == 0 || libc::isatty(1) == 0 } {
        return None;
    }
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::os::fd::AsRawFd;
    let mut tty = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;
    let fd = tty.as_raw_fd();
    // SAFETY: termios is plain data; tcgetattr fills it.
    let mut old: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(fd, &mut old) } != 0 {
        return None;
    }
    let mut raw = old;
    // SAFETY: cfmakeraw only changes the struct.
    unsafe { libc::cfmakeraw(&mut raw) };
    // Keep the output processing, so nothing else on screen changes.
    raw.c_oflag = old.c_oflag;
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
        return None;
    }
    let mut buf = Vec::new();
    let mut done = false;
    if tty.write_all(query_text().as_bytes()).is_ok() && tty.flush().is_ok() {
        let end = Instant::now() + QUERY_TIMEOUT;
        let mut chunk = [0u8; 512];
        while !done {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let mut p = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one valid pollfd.
            let n = unsafe { libc::poll(&mut p, 1, left.as_millis().max(1) as i32) };
            if n <= 0 {
                if n < 0
                    && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
                {
                    continue;
                }
                break;
            }
            // SAFETY: reads into a buffer of its length.
            let got = unsafe { libc::read(fd, chunk.as_mut_ptr().cast(), chunk.len()) };
            if got <= 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..got as usize]);
            done = parse_replies(&buf).done;
        }
    }
    // A slow terminal may still answer: drop what came, so it does not
    // turn into key presses.
    // SAFETY: plain calls on a valid fd.
    unsafe {
        if !done {
            libc::tcflush(fd, libc::TCIFLUSH);
        }
        libc::tcsetattr(fd, libc::TCSANOW, &old);
    }
    let colors = parse_replies(&buf).colors;
    colors.usable().then_some(colors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_values_with_1_to_4_digits() {
        assert_eq!(parse_color("rgb:ffff/8080/0000"), Some((255, 128, 0)));
        assert_eq!(parse_color("rgb:fb/f1/c7"), Some((0xfb, 0xf1, 0xc7)));
        assert_eq!(parse_color("rgb:f/0/8"), Some((255, 0, 136)));
        assert_eq!(parse_color("rgb:fff/000/800"), Some((255, 0, 128)));
        assert_eq!(parse_color("rgba:ffff/0000/0000/ffff"), Some((255, 0, 0)));
        assert_eq!(parse_color("#282828"), Some((40, 40, 40)));
        assert_eq!(parse_color("rgb:fffff/0/0"), None);
        assert_eq!(parse_color("rgb:gg/00/00"), None);
        assert_eq!(parse_color("rgb:ff/00"), None);
        assert_eq!(parse_color("red"), None);
        assert_eq!(parse_color("rgb://"), None);
    }

    #[test]
    fn answers_ended_by_bel_and_by_st() {
        let text = b"\x1b]10;rgb:5050/4949/4545\x07\
            \x1b]11;rgb:fbfb/f1f1/c7c7\x1b\\\
            \x1b]4;3;rgb:b5/76/14\x1b\\\
            \x1b]4;11;rgb:b5b5/7676/1414\x07\
            \x1b[?62;22c";
        let r = parse_replies(text);
        assert!(r.done);
        assert_eq!(r.colors.fg, Some((0x50, 0x49, 0x45)));
        assert_eq!(r.colors.bg, Some((0xfb, 0xf1, 0xc7)));
        assert_eq!(r.colors.ansi[3], Some((0xb5, 0x76, 0x14)));
        assert_eq!(r.colors.ansi[11], Some((0xb5, 0x76, 0x14)));
        assert_eq!(r.colors.ansi[0], None);
        assert!(r.colors.usable());
    }

    #[test]
    fn garbage_and_cut_answers_are_skipped() {
        // Only DA1: a terminal that knows no colour questions.
        let r = parse_replies(b"\x1b[?1;2c");
        assert!(r.done);
        assert!(!r.colors.usable());
        // Keys typed in between, a broken answer, a bad index, a cut end.
        let text = b"abc\x1b]10;rgb:zz/00/00\x07x\x1b]4;99;rgb:ff/ff/ff\x07\
            \x1b]11;rgb:00/00\x1b]11;rgb:0000/0000/0000\x07\x1b]10;rgb:ff";
        let r = parse_replies(text);
        assert!(!r.done);
        assert_eq!(r.colors.fg, None);
        assert_eq!(r.colors.bg, Some((0, 0, 0)));
        assert!(r.colors.ansi.iter().all(Option::is_none));
        // A CSI that is not DA1 does not end the wait.
        assert!(!parse_replies(b"\x1b[0n").done);
        assert_eq!(parse_replies(b""), Replies::default());
        assert_eq!(parse_replies(b"\x1b").colors, TermColors::default());
    }

    #[test]
    fn query_asks_for_everything_and_ends_with_da1() {
        let q = query_text();
        assert!(q.starts_with("\x1b]10;?\x07\x1b]11;?\x07"));
        assert!(q.contains("\x1b]4;0;?\x07") && q.contains("\x1b]4;15;?\x07"));
        assert!(q.ends_with("\x1b[c"));
    }
}
