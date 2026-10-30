//! Pixel jobs: Mermaid SVG and big-type headings, drawn by resvg onto a
//! canvas that is an exact multiple of the cell size so the terminal never
//! rescales them.

use crate::fonts::{self, Weight};
use crate::theme::Rgb;
use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Cell {
    pub w: u32,
    pub h: u32,
}

pub struct Raster {
    pub png: Vec<u8>,
    pub cols: u16,
    pub rows: u16,
}

/// Scale from Pele's CSS pixels to device pixels, chosen so diagram labels
/// come out about as tall as the terminal's own text.
pub fn diagram_scale(cell: Cell) -> f64 {
    cell.h as f64 * 0.8 / crate::pele::FONT_SIZE
}

/// Size a diagram takes in cells, given Pele's reported size.
pub fn diagram_cells(w: f64, h: f64, cell: Cell, avail_cols: u16) -> (u16, u16, f64) {
    let max_px = avail_cols as f64 * cell.w as f64;
    // Placeholder rows are numbered by diacritics, which caps an image's height.
    let max_py = crate::kitty::max_rows() as f64 * cell.h as f64;
    let s = diagram_scale(cell).min(max_px / w.max(1.0)).min(max_py / h.max(1.0));
    let cols = ((w * s) / cell.w as f64).ceil().max(1.0) as u16;
    let rows = ((h * s) / cell.h as f64).ceil().max(1.0) as u16;
    (cols.min(avail_cols.max(1)), rows, s)
}

pub fn diagram(svg: &str, w: f64, h: f64, cell: Cell, avail_cols: u16) -> Result<Raster, String> {
    let db = if needs_system_fonts(svg) { fonts::with_system() } else { fonts::embedded() };
    let (cols, rows, s) = diagram_cells(w, h, cell, avail_cols);
    draw(svg, db, cols, rows, cell, Transform::from_scale(s as f32, s as f32))
}

/// A heading laid out into lines that fit the available width.
#[derive(Clone, Debug, PartialEq)]
pub struct Heading {
    pub lines: Vec<String>,
    pub level: u8,
    pub rows_per_line: u16,
    pub cols: u16,
    pub font_size: f32,
    pub weight: Weight,
}

impl Heading {
    pub fn rows(&self) -> u16 {
        self.rows_per_line * self.lines.len() as u16
    }
}

pub fn layout_heading(text: &str, level: u8, cell: Cell, avail_cols: u16) -> Heading {
    let rows_per_line: u16 = if level == 1 { 3 } else { 2 };
    let weight = if level == 1 { Weight::DisplayBold } else { Weight::DisplaySemiBold };
    let font_size = (rows_per_line as u32 * cell.h) as f32 / 1.32;
    let max_px = (avail_cols as u32 * cell.w) as f32;
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let candidate = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
        if !cur.is_empty() && fonts::width(&candidate, font_size, weight) > max_px {
            lines.push(std::mem::replace(&mut cur, word.to_string()));
        } else {
            cur = candidate;
        }
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }
    let widest = lines.iter().map(|l| fonts::width(l, font_size, weight)).fold(0.0, f32::max);
    let cols = ((widest / cell.w as f32).ceil() as u16 + 1).min(avail_cols.max(1));
    Heading { lines, level, rows_per_line, cols, font_size, weight }
}

pub fn heading(h: &Heading, color: Rgb, cell: Cell) -> Result<Raster, String> {
    let m = fonts::metrics(h.weight);
    let line_px = (h.rows_per_line as u32 * cell.h) as f32;
    let fs = h.font_size;
    // Center the capitals in each line box; descenders fit below.
    let baseline = (line_px + m.cap_height * fs) / 2.0;
    let weight = if h.weight == Weight::DisplayBold { 700 } else { 600 };
    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}">"#,
        h.cols as u32 * cell.w,
        h.rows() as u32 * cell.h
    );
    for (i, line) in h.lines.iter().enumerate() {
        svg.push_str(&format!(
            r#"<text x="0" y="{:.1}" font-family="{}" font-weight="{}" font-size="{:.1}" fill="{}">{}</text>"#,
            baseline + i as f32 * line_px,
            fonts::DISPLAY,
            weight,
            fs,
            color.hex(),
            escape(line)
        ));
    }
    svg.push_str("</svg>");
    draw(&svg, fonts::embedded(), h.cols, h.rows(), cell, Transform::identity())
}

fn draw(
    svg: &str,
    fontdb: Arc<usvg::fontdb::Database>,
    cols: u16,
    rows: u16,
    cell: Cell,
    transform: Transform,
) -> Result<Raster, String> {
    let opts = usvg::Options { fontdb, font_family: fonts::SANS.into(), ..Default::default() };
    let tree = usvg::Tree::from_str(svg, &opts).map_err(|e| e.to_string())?;
    let (pw, ph) = (cols as u32 * cell.w, rows as u32 * cell.h);
    let mut pixmap = Pixmap::new(pw.max(1), ph.max(1)).ok_or("image too large")?;
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Ok(Raster { png: encode_png(&pixmap)?, cols, rows })
}

/// PNG with fast compression: the bytes only travel over a local pty.
fn encode_png(p: &Pixmap) -> Result<Vec<u8>, String> {
    // tiny-skia stores premultiplied alpha; PNG wants straight alpha.
    let mut data = p.data().to_vec();
    for px in data.chunks_exact_mut(4) {
        let a = px[3];
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((*c as u16 * 255 + a as u16 / 2) / a as u16) as u8;
            }
        }
    }
    let mut out = Vec::with_capacity(data.len() / 8);
    let mut enc = png::Encoder::new(&mut out, p.width(), p.height());
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    w.write_image_data(&data).map_err(|e| e.to_string())?;
    drop(w);
    Ok(out)
}

fn needs_system_fonts(svg: &str) -> bool {
    // Only text content matters; markup is ASCII.
    !svg.is_ascii() && !fonts::covers(&text_content(svg))
}

fn text_content(svg: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in svg.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL: Cell = Cell { w: 16, h: 28 };

    #[test]
    fn heading_wraps_to_width() {
        let h = layout_heading("A fairly long heading that cannot fit on one narrow line", 1, CELL, 30);
        assert!(h.lines.len() > 1);
        assert!(h.cols <= 30);
        assert_eq!(h.rows(), 3 * h.lines.len() as u16);
    }

    #[test]
    fn heading_rasterizes_at_cell_multiple() {
        let h = layout_heading("Plan", 2, CELL, 80);
        let r = heading(&h, Rgb(255, 255, 255), CELL).unwrap();
        assert_eq!(r.rows, 2);
        let img = png::Decoder::new(std::io::Cursor::new(&r.png)).read_info().unwrap();
        assert_eq!(img.info().width, r.cols as u32 * CELL.w);
        assert_eq!(img.info().height, 2 * CELL.h);
    }

    #[test]
    fn diagram_shrinks_to_fit() {
        let (cols, _, s) = diagram_cells(2000.0, 100.0, CELL, 50);
        assert_eq!(cols, 50);
        assert!(s < diagram_scale(CELL));
    }
}
