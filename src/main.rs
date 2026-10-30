mod app;
mod bench;
mod doc;
mod fonts;
mod kitty;
mod pele;
mod raster;
mod svgflat;
mod term;
mod text;
mod theme;

use std::path::PathBuf;

const USAGE: &str = "\
tkmp: a live markdown previewer with Mermaid diagrams and big-type headings

usage:
  tkmp FILE                          watch FILE and re-render on every save
  tkmp bench FILE [--cols N] [--cell WxH]
                                    time each pipeline stage, no terminal needed

keys: j/k scroll, space/b page, d/u half page, g/G top/bottom (G follows),
      ]/[ next/previous heading, ctrl-r redraw, q quit

TKMP_NO_GRAPHICS=1 forces text-only rendering.";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        None | Some("-h" | "--help") => {
            println!("{USAGE}");
            return;
        }
        Some("-V" | "--version") => {
            println!("tkmp {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Some("bench") => bench_args(&args[1..]),
        Some(file) => {
            if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
                eprintln!("tkmp: stdout is not a terminal");
                std::process::exit(2);
            }
            app::run(&PathBuf::from(file))
        }
    };
    if let Err(e) = result {
        eprintln!("tkmp: {e}");
        std::process::exit(1);
    }
}

fn bench_args(args: &[String]) -> anyhow::Result<()> {
    let mut file = None;
    let mut cols = 120;
    let mut cell = raster::Cell { w: 16, h: 28 };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--cols" => cols = it.next().ok_or_else(|| anyhow::anyhow!("--cols needs a value"))?.parse()?,
            "--cell" => {
                let v = it.next().ok_or_else(|| anyhow::anyhow!("--cell needs WxH"))?;
                let (w, h) = v.split_once('x').ok_or_else(|| anyhow::anyhow!("--cell needs WxH"))?;
                cell = raster::Cell { w: w.parse()?, h: h.parse()? };
            }
            f => file = Some(PathBuf::from(f)),
        }
    }
    bench::run(&file.ok_or_else(|| anyhow::anyhow!("bench needs a FILE"))?, cols, cell)
}
