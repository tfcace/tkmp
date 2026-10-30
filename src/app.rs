//! The live viewer: state, layout, painting and the event loop.

use crate::doc::{self, Doc, Node};
use crate::kitty;
use crate::pele::{self, Pele, PeleError};
use crate::raster::{self, Cell, Heading};
use crate::svgflat;
use crate::term;
use crate::text::{Attrs, Ctx, Line};
use crate::theme::Theme;
use crossbeam_channel::{select, unbounded, Receiver, Sender};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};
use notify::Watcher;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthStr;
use xxhash_rust::xxh3::{xxh3_64, Xxh3};

const DEBOUNCE: Duration = Duration::from_millis(15);
const SETTLE: Duration = Duration::from_millis(80);
const MARK: Duration = Duration::from_millis(1500);
const MAX_WIDTH: usize = 110;
const GUTTER: usize = 2;

#[derive(Clone, Debug, PartialEq)]
pub enum RowKind {
    Text(Line),
    Image { id: u32, row: u16, cols: u16 },
}

#[derive(Clone, Debug)]
pub struct Row {
    pub block: usize,
    pub kind: RowKind,
}

struct Flat {
    svg: Arc<String>,
    w: f64,
    h: f64,
    hash: u64,
}

enum Diagram {
    Pending,
    Ready(Arc<Flat>),
    Failed(PeleError),
}

struct Img {
    id: u32,
    cols: u16,
    rows: u16,
    placed: (u16, u16),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Ident {
    Mermaid(usize),
    Heading(u64),
}

enum Job {
    Heading(Heading),
    Diagram(Arc<Flat>, u16),
}

struct Done {
    key: u64,
    result: Result<raster::Raster, String>,
}

pub struct App {
    path: PathBuf,
    name: String,
    src_hash: Option<u64>,
    doc: Doc,
    theme: Theme,
    graphics: bool,
    cell: Cell,
    cols: u16,
    rows: u16,

    top: usize,
    follow: bool,
    layout: Vec<Row>,
    starts: Vec<usize>,
    text_cache: HashMap<(u64, usize), Arc<Vec<Line>>>,

    pele: Pele,
    pele_tags: HashMap<u64, (u64, u32)>,
    diagrams: HashMap<(u64, u32), Diagram>,
    images: HashMap<u64, Img>,
    pending: HashSet<u64>,
    wanted: HashSet<u64>,
    stale: HashMap<Ident, u64>,
    next_id: u32,
    done_tx: Sender<Done>,

    changed: HashMap<usize, Instant>,
    settle_until: Option<Instant>,
    edit_started: Option<Instant>,
    latency: Option<Duration>,
    message: Option<String>,

    frame: Vec<String>,
    out: Vec<u8>,
}

pub fn run(path: &Path) -> anyhow::Result<()> {
    let path = std::fs::canonicalize(path)?;
    crate::fonts::warm_system();
    let (pele_tx, pele_rx) = unbounded();
    let pele = Pele::spawn(pele_tx);

    term::enter()?;
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        term::leave();
        prev_hook(info);
    }));
    let caps = term::query();
    let (cols, rows, ioctl_cell) = term::size();
    let cell = ioctl_cell.or(caps.cell).unwrap_or(Cell { w: 8, h: 16 });

    let (done_tx, done_rx) = unbounded();
    let mut app = App {
        name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        path: path.clone(),
        src_hash: None,
        doc: Doc { blocks: Vec::new() },
        theme: caps.theme,
        graphics: caps.graphics && std::env::var_os("TKMP_NO_GRAPHICS").is_none(),
        cell,
        cols,
        rows,
        top: 0,
        follow: false,
        layout: Vec::new(),
        starts: Vec::new(),
        text_cache: HashMap::new(),
        pele,
        pele_tags: HashMap::new(),
        diagrams: HashMap::new(),
        images: HashMap::new(),
        pending: HashSet::new(),
        wanted: HashSet::new(),
        stale: HashMap::new(),
        next_id: 1 + (std::process::id() % 1000) * 1000,
        done_tx,
        changed: HashMap::new(),
        settle_until: None,
        edit_started: Some(Instant::now()),
        latency: None,
        message: None,
        frame: Vec::new(),
        out: Vec::new(),
    };

    let (input_tx, input_rx) = unbounded();
    std::thread::Builder::new().name("input".into()).spawn(move || {
        while let Ok(ev) = crossterm::event::read() {
            if input_tx.send(ev).is_err() {
                return;
            }
        }
    })?;

    let (fs_tx, fs_rx) = unbounded::<()>();
    let file = path.clone();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            if ev.paths.iter().any(|p| p == &file || p.file_name() == file.file_name()) {
                let _ = fs_tx.send(());
            }
        }
    })?;
    watcher.watch(path.parent().unwrap_or(Path::new(".")), notify::RecursiveMode::NonRecursive)?;

    log(format_args!("start {cols}x{rows} cell={cell:?} ioctl={ioctl_cell:?} csi={:?} graphics={}", caps.cell, app.graphics));
    app.reload();
    let result = app.event_loop(input_rx, fs_rx, pele_rx, done_rx);
    app.cleanup();
    term::leave();
    result
}

impl App {
    fn event_loop(
        &mut self,
        input: Receiver<Event>,
        fs: Receiver<()>,
        pele: Receiver<pele::Response>,
        done: Receiver<Done>,
    ) -> anyhow::Result<()> {
        let mut reload_at: Option<Instant> = None;
        loop {
            let now = Instant::now();
            let deadline = [reload_at, self.settle_until, self.next_mark_expiry()].into_iter().flatten().min();
            let timeout = deadline.map(|d| d.saturating_duration_since(now)).unwrap_or(Duration::from_secs(3600));
            let mut relayout = false;
            let mut repaint = false;
            let mut quit = false;

            let on_input = |app: &mut App, ev: Event, relayout: &mut bool, repaint: &mut bool| -> bool {
                match ev {
                    Event::Key(k) if k.kind != KeyEventKind::Release => return app.key(k, repaint),
                    Event::Mouse(m) => match m.kind {
                        MouseEventKind::ScrollDown => app.scroll(3, repaint),
                        MouseEventKind::ScrollUp => app.scroll(-3, repaint),
                        _ => {}
                    },
                    Event::Resize(c, r) => {
                        app.resize(c, r);
                        *relayout = true;
                    }
                    _ => {}
                }
                false
            };

            select! {
                recv(input) -> ev => match ev {
                    Ok(ev) => quit |= on_input(self, ev, &mut relayout, &mut repaint),
                    Err(_) => return Ok(()),
                },
                recv(fs) -> _ => {
                    reload_at = Some(Instant::now() + DEBOUNCE);
                    self.edit_started.get_or_insert_with(Instant::now);
                },
                recv(pele) -> r => if let Ok(r) = r { self.on_pele(r); relayout = true; },
                recv(done) -> d => if let Ok(d) = d { relayout |= self.on_done(d); },
                default(timeout) => {},
            }
            // Batch whatever else is already waiting into the same frame.
            while let Ok(ev) = input.try_recv() {
                quit |= on_input(self, ev, &mut relayout, &mut repaint);
            }
            while fs.try_recv().is_ok() {
                reload_at = Some(Instant::now() + DEBOUNCE);
            }
            while let Ok(r) = pele.try_recv() {
                self.on_pele(r);
                relayout = true;
            }
            while let Ok(d) = done.try_recv() {
                relayout |= self.on_done(d);
            }
            if quit {
                return Ok(());
            }

            let now = Instant::now();
            if reload_at.is_some_and(|t| t <= now) {
                reload_at = None;
                self.reload();
            }
            if self.settle_until.is_some_and(|t| t <= now) {
                self.settle_until = None;
                relayout = true;
            }
            let before = self.changed.len();
            self.changed.retain(|_, t| now.duration_since(*t) < MARK);
            repaint |= self.changed.len() != before;

            if relayout {
                self.relayout_keep_anchor();
                repaint = true;
            }
            if repaint {
                self.paint();
            }
        }
    }

    fn next_mark_expiry(&self) -> Option<Instant> {
        self.changed.values().min().map(|t| *t + MARK)
    }

    fn content_width(&self) -> usize {
        (self.cols as usize).saturating_sub(GUTTER + 2).clamp(10, MAX_WIDTH)
    }

    fn view_height(&self) -> usize {
        (self.rows as usize).saturating_sub(1).max(1)
    }

    fn max_top(&self) -> usize {
        self.layout.len().saturating_sub(self.view_height())
    }

    // ---- input ----

    fn key(&mut self, k: KeyEvent, repaint: &mut bool) -> bool {
        let page = self.view_height() as isize;
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Char('c') if ctrl => return true,
            KeyCode::Char('j') | KeyCode::Down | KeyCode::Enter => self.scroll(1, repaint),
            KeyCode::Char('k') | KeyCode::Up => self.scroll(-1, repaint),
            KeyCode::Char('d') if ctrl => self.scroll(page / 2, repaint),
            KeyCode::Char('u') if ctrl => self.scroll(-page / 2, repaint),
            KeyCode::Char('f') if ctrl => self.scroll(page - 2, repaint),
            KeyCode::Char('b') if ctrl => self.scroll(-(page - 2), repaint),
            KeyCode::Char('d') => self.scroll(page / 2, repaint),
            KeyCode::Char('u') => self.scroll(-page / 2, repaint),
            KeyCode::Char(' ') | KeyCode::PageDown => self.scroll(page - 2, repaint),
            KeyCode::Char('b') | KeyCode::PageUp => self.scroll(-(page - 2), repaint),
            KeyCode::Char('g') | KeyCode::Home => {
                self.top = 0;
                self.follow = false;
                *repaint = true;
            }
            KeyCode::Char('G') | KeyCode::End => {
                self.top = self.max_top();
                self.follow = true;
                *repaint = true;
            }
            KeyCode::Char(']') => self.jump_heading(true, repaint),
            KeyCode::Char('[') => self.jump_heading(false, repaint),
            KeyCode::Char('r') if ctrl => {
                self.frame.clear();
                self.out.extend_from_slice(b"\x1b[2J");
                *repaint = true;
            }
            _ => {}
        }
        false
    }

    fn scroll(&mut self, by: isize, repaint: &mut bool) {
        let max = self.max_top();
        let top = (self.top as isize + by).clamp(0, max as isize) as usize;
        if top != self.top {
            self.top = top;
            *repaint = true;
        }
        self.follow = self.top == max && by > 0;
    }

    fn jump_heading(&mut self, forward: bool, repaint: &mut bool) {
        let heads: Vec<usize> = self
            .doc
            .blocks
            .iter()
            .enumerate()
            .filter(|(_, b)| matches!(b.node, Node::Heading { .. }))
            .filter_map(|(i, _)| self.starts.get(i).copied())
            .collect();
        let target = if forward {
            heads.into_iter().find(|&r| r > self.top)
        } else {
            heads.into_iter().rev().find(|&r| r < self.top)
        };
        if let Some(t) = target {
            self.top = t.min(self.max_top());
            self.follow = false;
            *repaint = true;
        }
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        let (_, _, cell) = term::size();
        let cell = cell.unwrap_or(self.cell);
        if cols != self.cols || cell != self.cell {
            self.settle_until = Some(Instant::now() + SETTLE);
        }
        log(format_args!("resize {cols}x{rows} cell={cell:?}"));
        self.cols = cols;
        self.rows = rows;
        self.cell = cell;
        self.frame.clear();
        self.out.extend_from_slice(b"\x1b[2J");
    }

    // ---- document ----

    fn reload(&mut self) {
        let src = match std::fs::read_to_string(&self.path) {
            Ok(s) => s,
            Err(e) => {
                self.message = Some(format!("can't read: {e}"));
                self.edit_started = None;
                self.paint();
                return;
            }
        };
        self.message = None;
        let hash = xxh3_64(src.as_bytes());
        if self.src_hash == Some(hash) {
            self.edit_started = None;
            return;
        }
        let first = self.src_hash.is_none();
        self.src_hash = Some(hash);
        let new = doc::parse(&src);

        let at_bottom = !first && (self.follow || (self.top > 0 && self.top >= self.max_top()));
        let (anchor_block, offset) = self.anchor();
        let anchor_hash = self.doc.blocks.get(anchor_block).map(|b| b.hash);
        let old: HashSet<u64> = self.doc.blocks.iter().map(|b| b.hash).collect();

        let now = Instant::now();
        self.changed.clear();
        if !first {
            for (i, b) in new.blocks.iter().enumerate() {
                if !old.contains(&b.hash) {
                    self.changed.insert(i, now);
                }
            }
        }
        let new_anchor = anchor_hash
            .and_then(|h| {
                new.blocks
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| b.hash == h)
                    .min_by_key(|(i, _)| i.abs_diff(anchor_block))
                    .map(|(i, _)| i)
            })
            .unwrap_or(anchor_block.min(new.blocks.len().saturating_sub(1)));
        self.doc = new;
        self.relayout();
        self.restore_anchor(new_anchor, offset);
        if at_bottom {
            self.top = self.max_top();
            self.follow = true;
        }
        self.paint();
    }

    fn anchor(&self) -> (usize, usize) {
        match self.layout.get(self.top) {
            Some(row) => {
                let start = self.starts.get(row.block).copied().unwrap_or(0);
                (row.block, self.top.saturating_sub(start))
            }
            None => (0, 0),
        }
    }

    fn restore_anchor(&mut self, block: usize, offset: usize) {
        let start = self.starts.get(block).copied().unwrap_or(0);
        let end = self.starts.get(block + 1).copied().unwrap_or(self.layout.len());
        let top = start + offset.min(end.saturating_sub(start + 1));
        self.top = top.min(self.max_top());
    }

    fn relayout_keep_anchor(&mut self) {
        let follow = self.follow;
        let (b, o) = self.anchor();
        self.relayout();
        self.restore_anchor(b, o);
        if follow {
            self.top = self.max_top();
        }
    }

    // ---- layout ----

    fn relayout(&mut self) {
        let width = self.content_width();
        let mut rows = Vec::with_capacity(self.layout.len());
        let mut starts = Vec::with_capacity(self.doc.blocks.len());
        let mut new_stale = HashMap::new();
        self.wanted.clear();
        let mut used_text = HashSet::new();
        let mut used_diagrams = HashSet::new();
        let mut ordinal = 0;
        for i in 0..self.doc.blocks.len() {
            if i > 0 {
                rows.push(Row { block: i - 1, kind: RowKind::Text(Line::default()) });
            }
            starts.push(rows.len());
            for kind in self.block_rows(i, width, &mut ordinal, &mut new_stale, &mut used_text, &mut used_diagrams) {
                rows.push(Row { block: i, kind });
            }
        }
        self.layout = rows;
        self.starts = starts;
        self.text_cache.retain(|k, _| used_text.contains(k));
        self.diagrams.retain(|k, d| used_diagrams.contains(k) || matches!(d, Diagram::Pending));
        // Images neither shown nor kept as a stand-in are freed in the terminal.
        let keep: HashSet<u64> = new_stale.values().copied().collect();
        let mut freed = Vec::new();
        self.images.retain(|k, img| {
            let k = keep.contains(k);
            if !k {
                freed.push(img.id);
            }
            k
        });
        for id in freed {
            log(format_args!("delete id={id}"));
            kitty::delete(&mut self.out, id);
        }
        self.stale = new_stale;
    }

    fn block_rows(
        &mut self,
        i: usize,
        width: usize,
        ordinal: &mut usize,
        stale: &mut HashMap<Ident, u64>,
        used_text: &mut HashSet<(u64, usize)>,
        used_diagrams: &mut HashSet<(u64, u32)>,
    ) -> Vec<RowKind> {
        enum Kind {
            Heading(u8, String),
            Mermaid(String),
            Text,
        }
        let block = &self.doc.blocks[i];
        let hash = block.hash;
        let kind = match &block.node {
            Node::Heading { level, spans } if *level <= 2 && self.graphics => Kind::Heading(*level, doc::plain(spans)),
            Node::Mermaid(src) if self.graphics => Kind::Mermaid(src.clone()),
            _ => Kind::Text,
        };
        match kind {
            Kind::Heading(level, text) => {
                if !crate::fonts::covers(&text) || text.trim().is_empty() {
                    return self.text_rows(i, width, used_text);
                }
                let spec = raster::layout_heading(&text, level, self.cell, width as u16);
                let mut h = Xxh3::new();
                ("heading", &spec.lines, spec.level, self.cell, self.theme.fg).hash(&mut h);
                let key = h.digest();
                let ident = Ident::Heading(hash);
                if self.images.contains_key(&key) {
                    stale.insert(ident, key);
                    return self.image_rows(key, width);
                }
                let rows = spec.rows();
                self.want(key, Job::Heading(spec));
                if let Some(&old) = self.stale.get(&ident).filter(|k| self.images.contains_key(k)) {
                    stale.insert(ident, old);
                    return self.image_rows(old, width);
                }
                // Reserve the final height so the image lands without a jump.
                let mut out = self.text_rows(i, width, used_text);
                out.truncate(rows as usize);
                while out.len() < rows as usize {
                    out.push(RowKind::Text(Line::default()));
                }
                out
            }
            Kind::Mermaid(src) => {
                let ident = Ident::Mermaid(*ordinal);
                *ordinal += 1;
                let scale = raster::diagram_scale(self.cell);
                let max_css = pele::layout_width((width as u32 * self.cell.w) as f64, scale);
                let dkey = (hash, max_css);
                used_diagrams.insert(dkey);
                let mut error = None;
                match self.diagrams.get(&dkey) {
                    None => {
                        if self.settle_until.is_none() {
                            self.request_pele(dkey, src);
                        }
                    }
                    Some(Diagram::Pending) => {}
                    Some(Diagram::Ready(flat)) => {
                        let flat = flat.clone();
                        let (cols, rows, _) = raster::diagram_cells(flat.w, flat.h, self.cell, width as u16);
                        let mut h = Xxh3::new();
                        ("diagram", flat.hash, cols, rows, self.cell).hash(&mut h);
                        let key = h.digest();
                        if self.images.contains_key(&key) {
                            stale.insert(ident, key);
                            return self.image_rows(key, width);
                        }
                        self.want(key, Job::Diagram(flat, width as u16));
                    }
                    Some(Diagram::Failed(e)) => error = Some(e.clone()),
                }
                let mut out = match self.stale.get(&ident) {
                    Some(&old) if self.images.contains_key(&old) => {
                        stale.insert(ident, old);
                        self.image_rows(old, width)
                    }
                    _ => {
                        let mut rows = self.text_rows(i, width, used_text);
                        for r in &mut rows {
                            if let RowKind::Text(l) = r {
                                for c in &mut l.cells {
                                    c.attrs.fg = Some(self.theme.muted());
                                }
                            }
                        }
                        rows
                    }
                };
                if let Some(e) = error {
                    let mut l = Line::default();
                    l.cells.push(crate::text::Cell {
                        text: format!("✗ {}", error_text(&e)),
                        attrs: Attrs { fg: Some(self.theme.red()), ..Default::default() },
                        url: None,
                    });
                    out.push(RowKind::Text(l));
                }
                out
            }
            Kind::Text => self.text_rows(i, width, used_text),
        }
    }

    fn text_rows(&mut self, i: usize, width: usize, used: &mut HashSet<(u64, usize)>) -> Vec<RowKind> {
        let block = &self.doc.blocks[i];
        let key = (block.hash, width);
        used.insert(key);
        let lines = match self.text_cache.get(&key) {
            Some(l) => l.clone(),
            None => {
                let ctx = Ctx { theme: &self.theme };
                let l = Arc::new(ctx.node(&block.node, width));
                self.text_cache.insert(key, l.clone());
                l
            }
        };
        lines.iter().cloned().map(RowKind::Text).collect()
    }

    /// Rows for a transmitted image, rescaling its placement in the terminal
    /// when it is wider than the space it has now.
    fn image_rows(&mut self, key: u64, width: usize) -> Vec<RowKind> {
        let Some(img) = self.images.get_mut(&key) else { return Vec::new() };
        let cols = img.cols.min(width as u16).max(1);
        let rows = if cols == img.cols {
            img.rows
        } else {
            ((img.rows as u32 * cols as u32).div_ceil(img.cols as u32)).max(1) as u16
        };
        if img.placed != (cols, rows) {
            log(format_args!("place id={} {cols}x{rows} (image {}x{})", img.id, img.cols, img.rows));
            kitty::replace_placement(&mut self.out, img.id, cols, rows);
            img.placed = (cols, rows);
        }
        let id = img.id;
        (0..rows).map(|row| RowKind::Image { id, row, cols }).collect()
    }

    fn request_pele(&mut self, dkey: (u64, u32), src: String) {
        let mut h = Xxh3::new();
        dkey.hash(&mut h);
        let tag = h.digest();
        self.pele_tags.insert(tag, dkey);
        self.diagrams.insert(dkey, Diagram::Pending);
        self.pele.request(pele::Request { tag, source: src, max_width: dkey.1 as f64 });
    }

    fn on_pele(&mut self, r: pele::Response) {
        let Some(dkey) = self.pele_tags.remove(&r.tag) else { return };
        let state = match r.result {
            Ok(svg) => {
                let flat = svgflat::flatten(&svg.svg, &self.theme);
                let hash = xxh3_64(flat.as_bytes());
                Diagram::Ready(Arc::new(Flat { svg: Arc::new(flat), w: svg.width, h: svg.height, hash }))
            }
            Err(e) => Diagram::Failed(e),
        };
        self.diagrams.insert(dkey, state);
    }

    fn want(&mut self, key: u64, job: Job) {
        self.wanted.insert(key);
        if self.pending.contains(&key) || self.settle_until.is_some() {
            return;
        }
        self.pending.insert(key);
        let tx = self.done_tx.clone();
        let cell = self.cell;
        let fg = self.theme.fg;
        rayon::spawn(move || {
            let result = match job {
                Job::Heading(spec) => raster::heading(&spec, fg, cell),
                Job::Diagram(flat, cols) => raster::diagram(&flat.svg, flat.w, flat.h, cell, cols),
            };
            let _ = tx.send(Done { key, result });
        });
    }

    /// Returns true when the layout needs to change.
    fn on_done(&mut self, d: Done) -> bool {
        self.pending.remove(&d.key);
        if !self.wanted.contains(&d.key) {
            return false;
        }
        match d.result {
            Ok(r) => {
                let id = self.next_id;
                // IDs travel in a 24-bit color, so they wrap within 1..2^24.
                self.next_id = self.next_id % 0x00ff_ffff + 1;
                log(format_args!("transmit id={id} key={:x} {}x{} {} bytes cell={:?}", d.key, r.cols, r.rows, r.png.len(), self.cell));
                kitty::transmit(&mut self.out, id, &r.png, r.cols, r.rows);
                self.images.insert(d.key, Img { id, cols: r.cols, rows: r.rows, placed: (r.cols, r.rows) });
            }
            Err(e) => self.message = Some(format!("render failed: {e}")),
        }
        true
    }

    // ---- painting ----

    fn paint(&mut self) {
        let h = self.view_height();
        self.top = self.top.min(self.max_top());
        let width = self.content_width();
        let now = Instant::now();
        let mut frame = Vec::with_capacity(h + 1);
        for y in 0..h {
            let mut s = String::new();
            if let Some(row) = self.layout.get(self.top + y) {
                let marked = self.changed.get(&row.block).is_some_and(|t| now.duration_since(*t) < MARK);
                if marked {
                    sgr(&mut s, &Attrs { fg: Some(self.theme.accent()), ..Default::default() });
                    s.push_str("▎ \x1b[0m");
                } else {
                    s.push_str("  ");
                }
                match &row.kind {
                    RowKind::Text(line) => encode_line(&mut s, line, width),
                    RowKind::Image { id, row, cols } => kitty::placeholder_row(&mut s, *id, *row, *cols),
                }
            }
            s.push_str("\x1b[0m\x1b[K");
            frame.push(s);
        }
        frame.push(self.status_line());

        let busy = !self.pending.is_empty() || self.diagrams.values().any(|d| matches!(d, Diagram::Pending));
        let mut out = std::mem::take(&mut self.out);
        out.extend_from_slice(b"\x1b[?2026h");
        for (y, row) in frame.iter().enumerate() {
            if self.frame.get(y) != Some(row) {
                out.extend_from_slice(format!("\x1b[{};1H", y + 1).as_bytes());
                out.extend_from_slice(row.as_bytes());
            }
        }
        out.extend_from_slice(b"\x1b[?2026l");
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(&out);
        let _ = stdout.flush();
        out.clear();
        self.out = out;
        self.frame = frame;
        if !busy && self.settle_until.is_none() {
            if let Some(t) = self.edit_started.take() {
                self.latency = Some(t.elapsed());
                // The status line shows the new number on the next paint;
                // repaint only that row now.
                let status = self.status_line();
                let y = self.frame.len();
                let _ = write!(stdout, "\x1b[{y};1H{status}");
                let _ = stdout.flush();
                if let Some(last) = self.frame.last_mut() {
                    *last = status;
                }
            }
        }
    }

    fn status_line(&self) -> String {
        let t = &self.theme;
        let bar = Attrs { fg: Some(t.muted()), bg: Some(t.bg.mix(t.fg, 0.06)), ..Default::default() };
        let total = self.layout.len().max(1);
        let pct = if self.layout.len() <= self.view_height() {
            100
        } else {
            (self.top + self.view_height()).min(total) * 100 / total
        };
        let mut right = format!("{pct}%");
        if self.follow {
            right.push_str(" · follow");
        }
        if !self.graphics {
            right.push_str(" · text only");
        }
        if let Some(l) = self.latency {
            right.push_str(&format!(" · {:.1}ms", l.as_secs_f64() * 1000.0));
        }
        let left = match &self.message {
            Some(m) => format!(" {} · {m}", self.name),
            None => format!(" {}", self.name),
        };
        let pad = (self.cols as usize).saturating_sub(left.width() + right.width() + 1);
        let mut s = String::new();
        sgr(&mut s, &bar);
        s.push_str(&left);
        s.push_str(&" ".repeat(pad));
        s.push_str(&right);
        s.push(' ');
        s.push_str("\x1b[0m\x1b[K");
        s
    }

    fn cleanup(&mut self) {
        let mut out = Vec::new();
        for img in self.images.values() {
            kitty::delete(&mut out, img.id);
        }
        let _ = std::io::stdout().write_all(&out);
    }
}

/// Appends to the file named by `TKMP_LOG`, for debugging image traffic.
fn log(args: std::fmt::Arguments) {
    static FILE: std::sync::LazyLock<Option<std::sync::Mutex<std::fs::File>>> = std::sync::LazyLock::new(|| {
        let path = std::env::var_os("TKMP_LOG")?;
        std::fs::OpenOptions::new().create(true).append(true).open(path).ok().map(std::sync::Mutex::new)
    });
    if let Some(f) = FILE.as_ref() {
        if let Ok(mut f) = f.lock() {
            let _ = writeln!(f, "{args}");
        }
    }
}

/// Pele's parse errors repeat the position and draw a caret on separate
/// lines; one row needs the position and what the parser expected.
fn error_text(e: &PeleError) -> String {
    let lines: Vec<&str> = e.message.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let detail = match lines.as_slice() {
        [first, .., last] if first.starts_with("Parse error") => last,
        [first, ..] => first,
        [] => "diagram failed to render",
    };
    if e.line > 0 {
        format!("line {}, col {}: {detail}", e.line, e.column)
    } else {
        detail.to_string()
    }
}

fn sgr(s: &mut String, a: &Attrs) {
    s.push_str("\x1b[0");
    if a.bold {
        s.push_str(";1");
    }
    if a.italic {
        s.push_str(";3");
    }
    if a.underline {
        s.push_str(";4");
    }
    if a.strike {
        s.push_str(";9");
    }
    if let Some(c) = a.fg {
        s.push_str(&format!(";38;2;{};{};{}", c.0, c.1, c.2));
    }
    if let Some(c) = a.bg {
        s.push_str(&format!(";48;2;{};{};{}", c.0, c.1, c.2));
    }
    s.push('m');
}

fn encode_line(s: &mut String, line: &Line, width: usize) {
    let mut link: Option<&str> = None;
    for c in &line.cells {
        if c.url.as_deref() != link {
            s.push_str(&format!("\x1b]8;;{}\x1b\\", c.url.as_deref().unwrap_or("")));
            link = c.url.as_deref();
        }
        sgr(s, &c.attrs);
        s.push_str(&c.text);
    }
    if link.is_some() {
        s.push_str("\x1b]8;;\x1b\\");
    }
    if let Some(bg) = line.fill {
        let pad = width.saturating_sub(line.width());
        sgr(s, &Attrs { bg: Some(bg), ..Default::default() });
        s.push_str(&" ".repeat(pad));
    }
}
