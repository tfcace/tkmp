//! Kitty graphics protocol: transmitting images and drawing them with
//! Unicode placeholders, so images live in the text grid and scroll and
//! redraw like text without resending pixels.

use base64::Engine as _;
use std::sync::LazyLock;

const PLACEHOLDER: char = '\u{10EEEE}';

static DIACRITICS: LazyLock<Vec<char>> = LazyLock::new(|| {
    include_str!("../assets/rowcolumn-diacritics.txt")
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| u32::from_str_radix(l.split(';').next()?, 16).ok())
        .filter_map(char::from_u32)
        .collect()
});

/// Rows a single image can span (one diacritic per row index).
pub fn max_rows() -> u16 {
    DIACRITICS.len() as u16
}

/// Transmits a PNG and creates a virtual placement of `cols` x `rows`
/// cells for Unicode placeholders. Chunked as the protocol requires.
pub fn transmit(out: &mut Vec<u8>, id: u32, png: &[u8], cols: u16, rows: u16) {
    let data = base64::engine::general_purpose::STANDARD.encode(png);
    let bytes = data.as_bytes();
    let mut chunks = bytes.chunks(4096).peekable();
    let mut first = true;
    while let Some(chunk) = chunks.next() {
        let more = chunks.peek().is_some() as u8;
        out.extend_from_slice(b"\x1b_G");
        if first {
            out.extend_from_slice(
                format!("a=T,U=1,f=100,i={id},p=1,c={cols},r={rows},q=2,m={more}").as_bytes(),
            );
            first = false;
        } else {
            out.extend_from_slice(format!("m={more}").as_bytes());
        }
        out.push(b';');
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\x1b\\");
    }
}

/// Replaces an image's virtual placement with a new size. The terminal
/// rescales the pixels it already has, which is how resizes stay instant.
pub fn replace_placement(out: &mut Vec<u8>, id: u32, cols: u16, rows: u16) {
    out.extend_from_slice(format!("\x1b_Ga=p,U=1,i={id},p=1,c={cols},r={rows},q=2\x1b\\").as_bytes());
}

/// Deletes an image and frees its data.
pub fn delete(out: &mut Vec<u8>, id: u32) {
    out.extend_from_slice(format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\").as_bytes());
}

/// Probes for graphics support; the terminal answers `\x1b_Gi=31;OK\x1b\\`.
pub const QUERY: &str = "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\";

/// One row of placeholder cells for `id`. The image ID travels in the
/// foreground color; only the first cell needs diacritics, the rest inherit
/// row and column from their left neighbor.
pub fn placeholder_row(out: &mut String, id: u32, row: u16, cols: u16) {
    let d = &*DIACRITICS;
    let (r, g, b) = ((id >> 16) & 0xff, (id >> 8) & 0xff, id & 0xff);
    out.push_str(&format!("\x1b[38;2;{r};{g};{b}m"));
    out.push(PLACEHOLDER);
    out.push(d[row as usize % d.len()]);
    out.push(d[0]);
    for _ in 1..cols {
        out.push(PLACEHOLDER);
    }
    out.push_str("\x1b[39m");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diacritics_table_is_complete() {
        assert_eq!(DIACRITICS.len(), 297);
        assert_eq!(DIACRITICS[0], '\u{0305}');
        assert_eq!(DIACRITICS[1], '\u{030D}');
    }

    #[test]
    fn placeholder_encodes_id_and_row() {
        let mut s = String::new();
        placeholder_row(&mut s, 0x010203, 2, 3);
        assert!(s.starts_with("\x1b[38;2;1;2;3m"));
        let chars: Vec<char> = s.chars().filter(|c| *c == PLACEHOLDER).collect();
        assert_eq!(chars.len(), 3);
        assert!(s.contains(&format!("{PLACEHOLDER}{}{}", DIACRITICS[2], DIACRITICS[0])));
    }

    #[test]
    fn transmit_chunks() {
        let png = vec![7u8; 10_000];
        let mut out = Vec::new();
        transmit(&mut out, 5, &png, 10, 2);
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("\x1b_Ga=T,U=1,f=100,i=5,p=1,c=10,r=2,q=2,m=1;"));
        assert!(s.ends_with("\x1b\\"));
        assert_eq!(s.matches("\x1b_G").count(), 4);
        assert!(s.contains("\x1b_Gm=0;"));
    }
}
