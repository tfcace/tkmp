//! Terminal setup and the one-time startup queries: colors, cell size and
//! graphics support, answered before the input reader thread starts.

use crate::kitty;
use crate::raster::Cell;
use crate::theme::{Rgb, Theme};
use std::io::Write;
use std::time::{Duration, Instant};

pub struct Caps {
    pub theme: Theme,
    pub graphics: bool,
    /// From `CSI 16 t`, used when the window size ioctl reports no pixels.
    pub cell: Option<Cell>,
}

pub fn enter() -> std::io::Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    let mut out = std::io::stdout();
    // Alternate screen, hidden cursor, button and wheel mouse in SGR mode.
    out.write_all(b"\x1b[?1049h\x1b[?25l\x1b[?1000h\x1b[?1006h\x1b[2J")?;
    out.flush()
}

pub fn leave() {
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x1b[?1006l\x1b[?1000l\x1b[0m\x1b[?25h\x1b[?1049l");
    let _ = out.flush();
    let _ = crossterm::terminal::disable_raw_mode();
}

/// Sends every query followed by Primary Device Attributes. Terminals
/// answer in order, so the DA1 reply marks the end of everything they
/// were going to say.
pub fn query() -> Caps {
    let mut q = String::from("\x1b]10;?\x1b\\\x1b]11;?\x1b\\");
    for i in 0..16 {
        q.push_str(&format!("\x1b]4;{i};?\x1b\\"));
    }
    q.push_str(kitty::QUERY);
    q.push_str("\x1b[16t\x1b[c");
    let mut out = std::io::stdout();
    let _ = out.write_all(q.as_bytes());
    let _ = out.flush();
    let reply = read_until_da1(Duration::from_millis(400));
    parse_reply(&reply)
}

fn read_until_da1(timeout: Duration) -> Vec<u8> {
    let deadline = Instant::now() + timeout;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let mut pfd = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
        let ready = unsafe { libc::poll(&mut pfd, 1, left.as_millis().max(1) as i32) };
        if ready <= 0 {
            break;
        }
        let n = unsafe { libc::read(0, chunk.as_mut_ptr().cast(), chunk.len()) };
        if n <= 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n as usize]);
        if has_da1(&buf) {
            break;
        }
    }
    buf
}

fn has_da1(buf: &[u8]) -> bool {
    let s = String::from_utf8_lossy(buf);
    s.match_indices("\x1b[?").any(|(i, _)| {
        let rest = &s[i + 3..];
        let end = rest.find(|c: char| !(c.is_ascii_digit() || c == ';'));
        matches!(end, Some(e) if rest[e..].starts_with('c'))
    })
}

pub fn parse_reply(buf: &[u8]) -> Caps {
    let s = String::from_utf8_lossy(buf);
    let mut theme = Theme::dark_default();
    let mut fg = None;
    let mut bg = None;
    // OSC replies end with ST (ESC \) or BEL.
    for part in s.split(['\x07']).flat_map(|p| p.split("\x1b\\")) {
        let Some(osc) = part.rfind("\x1b]").map(|i| &part[i + 2..]) else { continue };
        if let Some(v) = osc.strip_prefix("10;") {
            fg = Rgb::parse(v);
        } else if let Some(v) = osc.strip_prefix("11;") {
            bg = Rgb::parse(v);
        } else if let Some(v) = osc.strip_prefix("4;") {
            if let Some((idx, color)) = v.split_once(';') {
                if let (Ok(i), Some(c)) = (idx.parse::<usize>(), Rgb::parse(color)) {
                    if i < 16 {
                        theme.palette[i] = c;
                    }
                }
            }
        }
    }
    if let Some(fg) = fg {
        theme.fg = fg;
    }
    if let Some(bg) = bg {
        theme.bg = bg;
    }
    let graphics = s.contains("\x1b_Gi=31;OK");
    let cell = s.find("\x1b[6;").and_then(|i| {
        let rest = &s[i + 4..];
        let end = rest.find('t')?;
        let (h, w) = rest[..end].split_once(';')?;
        let (h, w) = (h.parse().ok()?, w.parse().ok()?);
        (h > 0 && w > 0).then_some(Cell { w, h })
    });
    Caps { theme, graphics, cell }
}

/// Terminal size in cells, and the cell size in pixels when the ioctl knows.
pub fn size() -> (u16, u16, Option<Cell>) {
    match crossterm::terminal::window_size() {
        Ok(ws) => {
            let cell = (ws.width > 0 && ws.height > 0 && ws.columns > 0 && ws.rows > 0).then(|| Cell {
                w: ws.width as u32 / ws.columns as u32,
                h: ws.height as u32 / ws.rows as u32,
            });
            (ws.columns, ws.rows, cell)
        }
        Err(_) => {
            let (c, r) = crossterm::terminal::size().unwrap_or((80, 24));
            (c, r, None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rex_reply() {
        // Captured from Rex during the spike.
        let reply = b"\x1b[6;28;16t\x1b]10;rgb:e0e0/f0f0/efef\x1b\\\x1b]11;rgb:0f0f/1b1b/1d1d\x1b\\\x1b]4;4;rgb:6a6a/7474/8a8a\x1b\\\x1b_Gi=31;OK\x1b\\\x1b[?62;22c";
        let caps = parse_reply(reply);
        assert!(caps.graphics);
        assert_eq!(caps.cell, Some(Cell { w: 16, h: 28 }));
        assert_eq!(caps.theme.bg, Rgb(0x0f, 0x1b, 0x1d));
        assert_eq!(caps.theme.fg, Rgb(0xe0, 0xf0, 0xef));
        assert_eq!(caps.theme.palette[4], Rgb(0x6a, 0x74, 0x8a));
        assert!(has_da1(reply));
    }

    #[test]
    fn no_graphics_without_ok() {
        let caps = parse_reply(b"\x1b[?62;22c");
        assert!(!caps.graphics);
        assert_eq!(caps.cell, None);
    }
}
