//! Lays out non-pixel blocks as styled terminal lines at a given width.

use crate::doc::{Alert, Align, Item, Node, Span, Style};
use crate::theme::{Rgb, Theme};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Hash)]
pub struct Attrs {
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

#[derive(Clone, PartialEq, Eq, Debug, Hash)]
pub struct Cell {
    pub text: String,
    pub attrs: Attrs,
    pub url: Option<String>,
}

#[derive(Clone, Default, PartialEq, Eq, Debug, Hash)]
pub struct Line {
    pub cells: Vec<Cell>,
    /// Paints the rest of the row in this color (code blocks).
    pub fill: Option<Rgb>,
}

impl Line {
    pub fn width(&self) -> usize {
        self.cells.iter().map(|c| c.text.width()).sum()
    }

    #[cfg(test)]
    pub fn plain(&self) -> String {
        self.cells.iter().map(|c| c.text.as_str()).collect()
    }

    fn push(&mut self, text: impl Into<String>, attrs: Attrs) {
        self.cells.push(Cell { text: text.into(), attrs, url: None });
    }

    fn prefixed(prefix: &[Cell], mut rest: Line) -> Line {
        let mut cells = prefix.to_vec();
        cells.append(&mut rest.cells);
        Line { cells, fill: rest.fill }
    }
}

pub struct Ctx<'a> {
    pub theme: &'a Theme,
}

impl Ctx<'_> {
    fn span_attrs(&self, s: &Style) -> Attrs {
        let t = self.theme;
        Attrs {
            fg: if s.code {
                Some(t.palette[11])
            } else if s.link {
                Some(t.accent())
            } else {
                None
            },
            bg: s.code.then(|| t.code_bg()),
            bold: s.bold,
            italic: s.italic,
            underline: s.link,
            strike: s.strike,
        }
    }

    pub fn node(&self, node: &Node, width: usize) -> Vec<Line> {
        let width = width.max(4);
        let t = self.theme;
        match node {
            Node::Para(spans) => self.wrap(spans, width, Attrs::default()),
            Node::Heading { level, spans } => {
                let attrs = match level {
                    1 | 2 | 3 => Attrs { fg: Some(t.accent()), bold: true, ..Default::default() },
                    _ => Attrs { bold: true, ..Default::default() },
                };
                let mut lines = Vec::new();
                if *level <= 2 {
                    // Pixel headings fall back to this when graphics are off.
                    let mark = if *level == 1 { "█ " } else { "▌ " };
                    let mut inner = self.wrap(spans, width - 2, attrs);
                    for (i, l) in inner.drain(..).enumerate() {
                        let mut p = Line::default();
                        p.push(if i == 0 { mark } else { "  " }, Attrs { fg: Some(t.accent()), ..Default::default() });
                        lines.push(Line::prefixed(&p.cells, l));
                    }
                } else {
                    lines = self.wrap(spans, width, attrs);
                }
                lines
            }
            Node::Code { text, .. } | Node::Mermaid(text) => self.code(text, width),
            Node::Html(html) => {
                let attrs = Attrs { fg: Some(t.muted()), ..Default::default() };
                html.trim_end().lines().flat_map(|l| hard_wrap(l, width, attrs)).collect()
            }
            Node::Rule => {
                let mut l = Line::default();
                l.push("─".repeat(width), Attrs { fg: Some(t.faint()), ..Default::default() });
                vec![l]
            }
            Node::Quote { alert, children } => self.quote(*alert, children, width),
            Node::List { start, items } => self.list(*start, items, width),
            Node::Table { aligns, head, rows } => self.table(aligns, head, rows, width),
        }
    }

    fn children(&self, nodes: &[Node], width: usize, tight: bool) -> Vec<Line> {
        let mut out = Vec::new();
        for (i, n) in nodes.iter().enumerate() {
            if i > 0 && !(tight && matches!(n, Node::List { .. })) {
                out.push(Line::default());
            }
            out.extend(self.node(n, width));
        }
        out
    }

    fn quote(&self, alert: Option<Alert>, children: &[Node], width: usize) -> Vec<Line> {
        let t = self.theme;
        let (color, title) = match alert {
            Some(Alert::Note) => (t.accent(), Some("Note")),
            Some(Alert::Tip) => (t.green(), Some("Tip")),
            Some(Alert::Important) => (t.palette[13], Some("Important")),
            Some(Alert::Warning) => (t.yellow(), Some("Warning")),
            Some(Alert::Caution) => (t.red(), Some("Caution")),
            None => (t.faint(), None),
        };
        let bar = Cell { text: "▎ ".into(), attrs: Attrs { fg: Some(color), ..Default::default() }, url: None };
        let mut out = Vec::new();
        if let Some(title) = title {
            let mut l = Line::default();
            l.push(title, Attrs { fg: Some(color), bold: true, ..Default::default() });
            out.push(Line::prefixed(std::slice::from_ref(&bar), l));
        }
        for l in self.children(children, width - 2, false) {
            let mut l = l;
            if alert.is_none() {
                for c in &mut l.cells {
                    c.attrs.italic = true;
                    c.attrs.fg.get_or_insert(t.muted());
                }
            }
            out.push(Line::prefixed(std::slice::from_ref(&bar), l));
        }
        out
    }

    fn list(&self, start: Option<u64>, items: &[Item], width: usize) -> Vec<Line> {
        let t = self.theme;
        let tight = items.iter().all(|i| i.children.iter().filter(|c| !matches!(c, Node::List { .. })).count() <= 1);
        let last = start.map(|s| s + items.len() as u64 - 1).unwrap_or(0);
        let marker_w = match start {
            Some(_) => last.to_string().len() + 2,
            None => 2,
        };
        let mut out = Vec::new();
        for (i, item) in items.iter().enumerate() {
            if i > 0 && !tight {
                out.push(Line::default());
            }
            let marker = match (start, item.task) {
                (_, Some(true)) => "☑ ".to_string(),
                (_, Some(false)) => "☐ ".to_string(),
                (Some(s), None) => format!("{:>w$}. ", s + i as u64, w = marker_w - 2),
                (None, None) => "• ".to_string(),
            };
            let marker_w = marker_w.max(marker.width());
            let attrs = Attrs { fg: Some(if item.task == Some(true) { t.green() } else { t.accent() }), ..Default::default() };
            let mut body = self.children(&item.children, width.saturating_sub(marker_w), tight);
            if item.task == Some(true) {
                for l in &mut body {
                    for c in &mut l.cells {
                        c.attrs.fg = Some(t.muted());
                    }
                }
            }
            if body.is_empty() {
                body.push(Line::default());
            }
            for (j, l) in body.into_iter().enumerate() {
                let mut p = Line::default();
                if j == 0 {
                    p.push(format!("{marker:<marker_w$}"), attrs);
                } else {
                    p.push(" ".repeat(marker_w), Attrs::default());
                }
                out.push(Line::prefixed(&p.cells, l));
            }
        }
        out
    }

    fn code(&self, text: &str, width: usize) -> Vec<Line> {
        let bg = self.theme.code_bg();
        let attrs = Attrs { bg: Some(bg), ..Default::default() };
        let mut out = Vec::new();
        for raw in text.trim_end_matches('\n').split('\n') {
            let raw = raw.replace('\t', "    ");
            for mut l in hard_wrap(&raw, width - 2, attrs) {
                l.cells.insert(0, Cell { text: " ".into(), attrs, url: None });
                l.fill = Some(bg);
                out.push(l);
            }
        }
        out
    }

    fn table(&self, aligns: &[Align], head: &[Vec<Span>], rows: &[Vec<Vec<Span>>], width: usize) -> Vec<Line> {
        let t = self.theme;
        let ncols = head.len().max(rows.iter().map(|r| r.len()).max().unwrap_or(0));
        if ncols == 0 {
            return Vec::new();
        }
        let cell_w = |c: &Vec<Span>| c.iter().map(|s| s.text.width()).sum::<usize>();
        let mut want: Vec<usize> = (0..ncols)
            .map(|i| {
                let h = head.get(i).map(cell_w).unwrap_or(0);
                rows.iter().filter_map(|r| r.get(i)).map(cell_w).fold(h, usize::max).max(1)
            })
            .collect();
        let sep = 3;
        let avail = width.saturating_sub(sep * (ncols - 1)).max(ncols);
        // Shrink the widest columns first until the table fits.
        while want.iter().sum::<usize>() > avail {
            let (i, _) = want.iter().enumerate().max_by_key(|(_, w)| **w).unwrap();
            if want[i] <= 3 {
                break;
            }
            want[i] -= 1;
        }
        let border = Attrs { fg: Some(t.faint()), ..Default::default() };
        let mut out = Vec::new();
        let emit_row = |cells: &[Vec<Span>], base: Attrs, out: &mut Vec<Line>| {
            let wrapped: Vec<Vec<Line>> = (0..ncols)
                .map(|i| cells.get(i).map(|c| self.wrap(c, want[i], base)).unwrap_or_default())
                .collect();
            let height = wrapped.iter().map(|w| w.len()).max().unwrap_or(0).max(1);
            for r in 0..height {
                let mut line = Line::default();
                for i in 0..ncols {
                    if i > 0 {
                        line.push(" │ ", border);
                    }
                    let content = wrapped[i].get(r).cloned().unwrap_or_default();
                    let pad = want[i].saturating_sub(content.width());
                    let (l, rgt) = match aligns.get(i).copied().unwrap_or(Align::Left) {
                        Align::Left => (0, pad),
                        Align::Right => (pad, 0),
                        Align::Center => (pad / 2, pad - pad / 2),
                    };
                    if l > 0 {
                        line.push(" ".repeat(l), Attrs::default());
                    }
                    line.cells.extend(content.cells);
                    if rgt > 0 {
                        line.push(" ".repeat(rgt), Attrs::default());
                    }
                }
                out.push(line);
            }
        };
        emit_row(head, Attrs { bold: true, ..Default::default() }, &mut out);
        let mut rule = Line::default();
        let parts: Vec<String> = want.iter().map(|w| "─".repeat(*w)).collect();
        rule.push(parts.join("─┼─"), border);
        out.push(rule);
        for row in rows {
            emit_row(row, Attrs::default(), &mut out);
        }
        out
    }

    /// Greedy word wrap that keeps span styles.
    pub fn wrap(&self, spans: &[Span], width: usize, base: Attrs) -> Vec<Line> {
        let mut lines = vec![Line::default()];
        let mut col = 0usize;
        // Whitespace waits here until the next word decides whether it fits.
        let mut pending_space: Option<Cell> = None;
        for span in spans {
            let mut attrs = self.span_attrs(&span.style);
            attrs.fg = attrs.fg.or(base.fg);
            attrs.bold |= base.bold;
            attrs.italic |= base.italic;
            for piece in split_words(&span.text) {
                match piece {
                    Piece::Newline => {
                        lines.push(Line::default());
                        col = 0;
                        pending_space = None;
                    }
                    Piece::Space => {
                        if col > 0 {
                            pending_space = Some(Cell { text: " ".into(), attrs: if span.style.code { attrs } else { Attrs { underline: false, strike: false, ..attrs } }, url: span.url.clone() });
                        }
                    }
                    Piece::Word(w) => {
                        let ww = w.width();
                        let space = pending_space.as_ref().map_or(0, |_| 1);
                        if col > 0 && col + space + ww > width {
                            lines.push(Line::default());
                            col = 0;
                            pending_space = None;
                        }
                        let line = lines.last_mut().unwrap();
                        if let Some(sp) = pending_space.take() {
                            line.cells.push(sp);
                            col += 1;
                        }
                        if ww > width {
                            // A word longer than the line: break it by cells.
                            let mut chunk = String::new();
                            for ch in w.chars() {
                                let cw = ch.width().unwrap_or(0);
                                if col + cw > width {
                                    let line = lines.last_mut().unwrap();
                                    line.cells.push(Cell { text: std::mem::take(&mut chunk), attrs, url: span.url.clone() });
                                    lines.push(Line::default());
                                    col = 0;
                                }
                                chunk.push(ch);
                                col += cw;
                            }
                            lines.last_mut().unwrap().cells.push(Cell { text: chunk, attrs, url: span.url.clone() });
                        } else {
                            let line = lines.last_mut().unwrap();
                            match line.cells.last_mut() {
                                Some(last) if last.attrs == attrs && last.url == span.url => last.text.push_str(w),
                                _ => line.cells.push(Cell { text: w.to_string(), attrs, url: span.url.clone() }),
                            }
                            col += ww;
                        }
                    }
                }
            }
        }
        lines
    }
}

enum Piece<'a> {
    Word(&'a str),
    Space,
    Newline,
}

fn split_words(s: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in s.char_indices() {
        if c == ' ' || c == '\n' || c == '\t' {
            if let Some(st) = start.take() {
                out.push(Piece::Word(&s[st..i]));
            }
            out.push(if c == '\n' { Piece::Newline } else { Piece::Space });
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(st) = start {
        out.push(Piece::Word(&s[st..]));
    }
    out
}

fn hard_wrap(s: &str, width: usize, attrs: Attrs) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut col = 0;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if col + cw > width && !cur.is_empty() {
            lines.push(Line { cells: vec![Cell { text: std::mem::take(&mut cur), attrs, url: None }], fill: None });
            col = 0;
        }
        cur.push(ch);
        col += cw;
    }
    lines.push(Line { cells: vec![Cell { text: cur, attrs, url: None }], fill: None });
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc;

    fn render(md: &str, width: usize) -> Vec<String> {
        let theme = Theme::dark_default();
        let ctx = Ctx { theme: &theme };
        let d = doc::parse(md);
        let mut out = Vec::new();
        for (i, b) in d.blocks.iter().enumerate() {
            if i > 0 {
                out.push(String::new());
            }
            out.extend(ctx.node(&b.node, width).iter().map(|l| l.plain().trim_end().to_string()));
        }
        out
    }

    #[test]
    fn wraps_paragraphs() {
        let out = render("the quick brown fox jumps over the lazy dog", 15);
        assert_eq!(out, vec!["the quick brown", "fox jumps over", "the lazy dog"]);
        assert!(out.iter().all(|l| l.width() <= 15));
    }

    #[test]
    fn lists_nest_and_number() {
        let out = render("1. one\n2. two\n   - inner item\n3. three\n", 40);
        assert_eq!(out, vec!["1. one", "2. two", "   • inner item", "3. three"]);
    }

    #[test]
    fn tasks_and_quotes() {
        let out = render("- [x] shipped\n- [ ] next\n\n> quoted\n", 40);
        assert_eq!(out, vec!["☑ shipped", "☐ next", "", "▎ quoted"]);
    }

    #[test]
    fn tables_fit_width() {
        let out = render("| name | value |\n|---|--:|\n| alpha | 1 |\n| beta | 22 |\n", 40);
        assert_eq!(out[0], "name  │ value");
        assert_eq!(out[1], "──────┼──────");
        assert_eq!(out[2], "alpha │     1");
        let narrow = render("| a long header | another long header |\n|---|---|\n| x | y |\n", 20);
        assert!(narrow.iter().all(|l| l.width() <= 20), "{narrow:?}");
    }

    #[test]
    fn code_blocks_hard_wrap() {
        let out = render("```\nabcdefghij\n```\n", 8);
        assert_eq!(out, vec![" abcdef", " ghij"]);
    }
}
