//! Colors derived from the terminal's own palette, so diagrams and headings
//! match whatever theme Rex is using.

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// `t` = 0 gives `self`, 1 gives `other`.
    pub fn mix(self, other: Rgb, t: f32) -> Rgb {
        let l = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round().clamp(0.0, 255.0) as u8;
        Rgb(l(self.0, other.0), l(self.1, other.1), l(self.2, other.2))
    }

    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }


    /// Parses `#rgb`, `#rrggbb` and xterm's `rgb:rrrr/gggg/bbbb`.
    pub fn parse(s: &str) -> Option<Rgb> {
        let s = s.trim();
        if let Some(h) = s.strip_prefix('#') {
            let v = |i: usize, n: usize| u8::from_str_radix(&h[i..i + n], 16).ok();
            return match h.len() {
                3 => Some(Rgb(v(0, 1)? * 17, v(1, 1)? * 17, v(2, 1)? * 17)),
                6 => Some(Rgb(v(0, 2)?, v(2, 2)?, v(4, 2)?)),
                _ => None,
            };
        }
        let body = s.strip_prefix("rgb:")?;
        let mut parts = body.split('/');
        let mut chan = || -> Option<u8> {
            let p = parts.next()?;
            let v = u32::from_str_radix(p, 16).ok()?;
            let max = (1u32 << (4 * p.len() as u32)) - 1;
            Some(((v * 255 + max / 2) / max) as u8)
        };
        Some(Rgb(chan()?, chan()?, chan()?))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Theme {
    pub fg: Rgb,
    pub bg: Rgb,
    /// ANSI colors 0 to 15.
    pub palette: [Rgb; 16],
}

impl Theme {
    pub fn dark_default() -> Theme {
        let p = [
            (0x1d, 0x1f, 0x21), (0xcc, 0x66, 0x66), (0xb5, 0xbd, 0x68), (0xf0, 0xc6, 0x74),
            (0x81, 0xa2, 0xbe), (0xb2, 0x94, 0xbb), (0x8a, 0xbe, 0xb7), (0xc5, 0xc8, 0xc6),
            (0x66, 0x66, 0x66), (0xd5, 0x4e, 0x53), (0xb9, 0xca, 0x4a), (0xe7, 0xc5, 0x47),
            (0x7a, 0xa6, 0xda), (0xc3, 0x97, 0xd8), (0x70, 0xc0, 0xb1), (0xea, 0xea, 0xea),
        ];
        Theme { fg: Rgb(0xe0, 0xe0, 0xe0), bg: Rgb(0x1d, 0x1f, 0x21), palette: p.map(|(r, g, b)| Rgb(r, g, b)) }
    }


    pub fn accent(&self) -> Rgb {
        self.palette[12]
    }
    pub fn red(&self) -> Rgb {
        self.palette[1]
    }
    pub fn green(&self) -> Rgb {
        self.palette[2]
    }
    pub fn yellow(&self) -> Rgb {
        self.palette[3]
    }
    pub fn muted(&self) -> Rgb {
        self.fg.mix(self.bg, 0.45)
    }
    pub fn faint(&self) -> Rgb {
        self.fg.mix(self.bg, 0.7)
    }
    pub fn code_bg(&self) -> Rgb {
        self.bg.mix(self.fg, 0.07)
    }

    /// Values for Pele's `--pele-*` CSS variables.
    pub fn pele_var(&self, name: &str) -> Option<String> {
        let c = match name {
            "--pele-bg" => self.bg,
            "--pele-fg" => self.fg,
            "--pele-muted" => self.muted(),
            "--pele-line" => self.fg.mix(self.bg, 0.25),
            "--pele-surface" => self.bg.mix(self.fg, 0.09),
            "--pele-surface-alt" => self.bg.mix(self.fg, 0.16),
            "--pele-border" => self.fg.mix(self.bg, 0.55),
            "--pele-accent" => self.accent(),
            "--pele-critical" => self.red(),
            "--pele-font" => return Some(format!("{},sans-serif", crate::fonts::SANS)),
            "--pele-font-mono" => return Some(format!("{},monospace", crate::fonts::MONO_FAMILY)),
            _ => {
                let n: usize = name.strip_prefix("--pele-series-")?.parse().ok()?;
                const SERIES: [usize; 8] = [12, 11, 10, 9, 14, 13, 3, 6];
                self.palette[SERIES[(n.max(1) - 1) % 8]]
            }
        };
        Some(c.hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xterm_and_hex() {
        assert_eq!(Rgb::parse("rgb:0f0f/1b1b/1d1d"), Some(Rgb(0x0f, 0x1b, 0x1d)));
        assert_eq!(Rgb::parse("rgb:ff/80/00"), Some(Rgb(255, 128, 0)));
        assert_eq!(Rgb::parse("#fff"), Some(Rgb(255, 255, 255)));
        assert_eq!(Rgb::parse("#5b7bd5"), Some(Rgb(0x5b, 0x7b, 0xd5)));
    }

    #[test]
    fn mixes() {
        assert_eq!(Rgb(0, 0, 0).mix(Rgb(200, 100, 50), 0.5), Rgb(100, 50, 25));
    }
}
