//! Pele (Mermaid to SVG) running in an embedded QuickJS on its own thread.
//!
//! QuickJS contexts are not `Send`, so one worker thread owns the runtime and
//! answers requests over a channel. Loading the bundle happens on that thread
//! too, so it overlaps with startup instead of delaying the first paint.

use crate::fonts;
use crossbeam_channel::{unbounded, Receiver, Sender};
use rquickjs::{Context, Function, Object, Runtime, Value};
use std::thread;

const BUNDLE: &str = include_str!("../assets/pele.iife.js");

/// Pele's default font size. Diagrams are laid out at this size in CSS pixels
/// and scaled to the terminal's cell height when rasterized.
pub const FONT_SIZE: f64 = 16.0;

/// How much a diagram may shrink to keep its direction. Past this, Pele
/// lays it out vertically instead, so labels stay readable in narrow splits.
const MIN_SHRINK: f64 = 0.75;

/// The width, in Pele's CSS pixels, to lay a diagram out in, given the
/// device pixels available and the rasterizing scale.
pub fn layout_width(avail_px: f64, scale: f64) -> u32 {
    (avail_px / scale / MIN_SHRINK) as u32
}

pub struct Request {
    pub tag: u64,
    pub source: String,
    pub max_width: f64,
}

#[derive(Clone, Debug)]
pub struct Svg {
    pub svg: String,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug)]
pub struct PeleError {
    pub message: String,
    pub line: u32,
    pub column: u32,
}

pub struct Response {
    pub tag: u64,
    pub result: Result<Svg, PeleError>,
}

pub struct Pele {
    tx: Sender<Request>,
}

impl Pele {
    /// Starts the worker. Responses arrive on `out`.
    pub fn spawn(out: Sender<Response>) -> Pele {
        let (tx, rx) = unbounded::<Request>();
        thread::Builder::new()
            .name("pele".into())
            .spawn(move || worker(rx, out))
            .expect("spawn pele thread");
        Pele { tx }
    }

    pub fn request(&self, req: Request) {
        let _ = self.tx.send(req);
    }
}

fn worker(rx: Receiver<Request>, out: Sender<Response>) {
    let engine = match Engine::new() {
        Ok(e) => e,
        Err(msg) => {
            // Without the engine every diagram falls back to its source.
            for req in rx {
                let err = PeleError { message: msg.clone(), line: 0, column: 0 };
                let _ = out.send(Response { tag: req.tag, result: Err(err) });
            }
            return;
        }
    };
    for req in rx {
        let result = engine.render(&req.source, req.max_width);
        if out.send(Response { tag: req.tag, result }).is_err() {
            return;
        }
    }
}

pub struct Engine {
    _rt: Runtime,
    ctx: Context,
}

impl Engine {
    pub fn new() -> Result<Engine, String> {
        let rt = Runtime::new().map_err(|e| e.to_string())?;
        let ctx = Context::full(&rt).map_err(|e| e.to_string())?;
        ctx.with(|ctx| -> rquickjs::Result<()> {
            let measure = Function::new(ctx.clone(), |text: String, size: f64, style: i32| {
                fonts::measure(&text, size as f32, style) as f64
            })?;
            ctx.globals().set("__tkmpMeasure", measure)?;
            ctx.eval::<(), _>(BUNDLE)?;
            ctx.eval::<(), _>(
                "globalThis.__tkmpRender = (src, maxWidth) => {\n\
                   const r = peleRender(src, {\n\
                     maxWidth, responsive: false, fontSize: 16,\n\
                     autoDirection: true, directionBreakpoint: maxWidth + 1,\n\
                     fontFamily: 'Inter', fontFamilyMono: 'JetBrains Mono',\n\
                     links: false, images: false,\n\
                     measurer: { width: (t, s, st) => __tkmpMeasure(t, s, st) },\n\
                   });\n\
                   return { svg: r.svg, width: r.width, height: r.height };\n\
                 };",
            )?;
            Ok(())
        })
        .map_err(|e| format!("loading Pele: {e}"))?;
        Ok(Engine { _rt: rt, ctx })
    }

    pub fn render(&self, source: &str, max_width: f64) -> Result<Svg, PeleError> {
        self.ctx.with(|ctx| {
            let call = || -> rquickjs::Result<Svg> {
                let f: Function = ctx.globals().get("__tkmpRender")?;
                let r: Object = f.call((source, max_width))?;
                Ok(Svg { svg: r.get("svg")?, width: r.get("width")?, height: r.get("height")? })
            };
            call().map_err(|e| match e {
                rquickjs::Error::Exception => exception(ctx.catch()),
                other => PeleError { message: other.to_string(), line: 0, column: 0 },
            })
        })
    }
}

fn exception(v: Value) -> PeleError {
    let Some(obj) = v.as_object() else {
        return PeleError { message: "diagram failed to render".into(), line: 0, column: 0 };
    };
    let message: String = obj.get("message").unwrap_or_else(|_| "diagram failed to render".into());
    PeleError {
        message,
        line: obj.get::<_, Option<u32>>("line").ok().flatten().unwrap_or(0),
        column: obj.get::<_, Option<u32>>("column").ok().flatten().unwrap_or(0),
    }
}
