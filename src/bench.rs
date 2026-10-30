//! `tkmp bench FILE`: times each pipeline stage without a terminal.
//!
//! It runs the same parse, layout, Pele, flatten and raster code the viewer
//! does, and replays two kinds of edit. It leaves out the file-event latency
//! and the terminal's own drawing, which the viewer's status line measures.

use crate::doc::{self, Node};
use crate::pele::Engine;
use crate::raster::{self, Cell};
use crate::svgflat;
use crate::text::Ctx;
use crate::theme::Theme;
use rayon::prelude::*;
use std::time::{Duration, Instant};

pub fn run(path: &std::path::Path, cols: usize, cell: Cell) -> anyhow::Result<()> {
    let src = std::fs::read_to_string(path)?;
    let theme = Theme::dark_default();
    let ctx = Ctx { theme: &theme };
    let width = cols.saturating_sub(4).clamp(10, 110);
    let scale = raster::diagram_scale(cell);
    let max_css = crate::pele::layout_width((width as u32 * cell.w) as f64, scale) as f64;

    let t = Instant::now();
    let engine = Engine::new().map_err(anyhow::Error::msg)?;
    let pele_load = t.elapsed();

    let (parse_t, d) = time(|| doc::parse(&src));
    let (layout_t, _) = time(|| {
        d.blocks.iter().filter(|b| !is_pixel(&b.node)).map(|b| ctx.node(&b.node, width).len()).sum::<usize>()
    });

    let diagrams: Vec<&String> = d.blocks.iter().filter_map(|b| match &b.node {
        Node::Mermaid(s) => Some(s),
        _ => None,
    }).collect();
    let headings: Vec<(u8, String)> = d.blocks.iter().filter_map(|b| match &b.node {
        Node::Heading { level, spans } if *level <= 2 => Some((*level, doc::plain(spans))),
        _ => None,
    }).collect();

    // Warm the engine the way the viewer does after its first diagram.
    if let Some(s) = diagrams.first() {
        let _ = engine.render(s, max_css);
    }
    let mut pele_times = Vec::new();
    let mut flats = Vec::new();
    for s in &diagrams {
        let t = Instant::now();
        match engine.render(s, max_css) {
            Ok(svg) => {
                let p = t.elapsed();
                let t2 = Instant::now();
                let flat = svgflat::flatten(&svg.svg, &theme);
                pele_times.push((p, t2.elapsed()));
                flats.push((flat, svg.width, svg.height));
            }
            Err(e) => println!("  diagram error: {} (line {}, col {})", e.message.lines().next().unwrap_or(""), e.line, e.column),
        }
    }

    let raster_one = |f: &(String, f64, f64)| {
        let t = Instant::now();
        let r = raster::diagram(&f.0, f.1, f.2, cell, width as u16).expect("raster");
        (t.elapsed(), r.png.len(), r.cols, r.rows)
    };
    let serial: Vec<_> = flats.iter().map(raster_one).collect();
    if let Some(dir) = std::env::var_os("TKMP_DUMP") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir)?;
        for (i, f) in flats.iter().enumerate() {
            let r = raster::diagram(&f.0, f.1, f.2, cell, width as u16).map_err(anyhow::Error::msg)?;
            std::fs::write(dir.join(format!("diagram-{}.png", i + 1)), &r.png)?;
            std::fs::write(dir.join(format!("diagram-{}.svg", i + 1)), &f.0)?;
        }
        for (i, (l, text)) in headings.iter().enumerate() {
            let spec = raster::layout_heading(text, *l, cell, width as u16);
            let r = raster::heading(&spec, theme.fg, cell).map_err(anyhow::Error::msg)?;
            std::fs::write(dir.join(format!("heading-{}.png", i + 1)), &r.png)?;
        }
    }
    let heading_serial: Vec<Duration> = headings.iter().map(|(l, text)| {
        let t = Instant::now();
        let spec = raster::layout_heading(text, *l, cell, width as u16);
        raster::heading(&spec, theme.fg, cell).expect("heading");
        t.elapsed()
    }).collect();
    let (cold_t, _) = time(|| {
        rayon::join(
            || flats.par_iter().for_each(|f| { raster_one(f); }),
            || headings.par_iter().for_each(|(l, text)| {
                let spec = raster::layout_heading(text, *l, cell, width as u16);
                raster::heading(&spec, theme.fg, cell).expect("heading");
            }),
        )
    });

    // Edit replays: a paragraph edit re-parses and lays out text only; a
    // diagram edit adds one Pele render, flatten and raster.
    let edited = src.replacen("\n\n", "\n\nAn inserted sentence.\n\n", 1);
    let (para_edit, _) = time(|| {
        let d = doc::parse(&edited);
        d.blocks.iter().filter(|b| !is_pixel(&b.node)).map(|b| ctx.node(&b.node, width).len()).sum::<usize>()
    });
    let diagram_edit = diagrams.iter().find(|s| engine.render(s, max_css).is_ok()).map(|s| {
        let s2 = format!("{s}\n%% edit");
        let t = Instant::now();
        let _ = doc::parse(&src);
        if let Ok(svg) = engine.render(&s2, max_css) {
            let flat = svgflat::flatten(&svg.svg, &theme);
            let _ = raster::diagram(&flat, svg.width, svg.height, cell, width as u16);
        }
        t.elapsed()
    });

    println!("tkmp bench: {} ({} bytes, {} blocks)", path.display(), src.len(), d.blocks.len());
    println!("  terminal model   {cols} cols, cell {}x{} px, content width {width}", cell.w, cell.h);
    println!("  pele engine load {}", ms(pele_load));
    println!("  parse            {}", ms(parse_t));
    println!("  text layout      {}", ms(layout_t));
    for (i, ((p, f), (r, bytes, c, rows))) in pele_times.iter().zip(&serial).enumerate() {
        println!(
            "  diagram {:<2}       pele {}  flatten {}  raster {}  ({}x{} cells, {} KB png)",
            i + 1, ms(*p), ms(*f), ms(*r), c, rows, bytes / 1024
        );
    }
    if !heading_serial.is_empty() {
        let total: Duration = heading_serial.iter().sum();
        println!("  headings         {} total for {} ({} avg)", ms(total), heading_serial.len(), ms(total / heading_serial.len() as u32));
    }
    println!("  cold raster      {} on {} threads (all images in parallel)", ms(cold_t), rayon::current_num_threads());
    println!("  edit: paragraph  {}", ms(para_edit));
    if let Some(t) = diagram_edit {
        println!("  edit: diagram    {}", ms(t));
    }
    Ok(())
}

fn is_pixel(n: &Node) -> bool {
    matches!(n, Node::Mermaid(_) | Node::Heading { level: 1 | 2, .. })
}

fn time<T>(f: impl FnOnce() -> T) -> (Duration, T) {
    let t = Instant::now();
    let v = f();
    (t.elapsed(), v)
}

fn ms(d: Duration) -> String {
    format!("{:>7.2}ms", d.as_secs_f64() * 1000.0)
}
