# tkmp: a live markdown previewer

tkmp renders this file in a Rex split and re-renders on every save. Headings like the one above are drawn as pixels, and **Mermaid** blocks become real diagrams. Everything else is *styled text*, with `inline code`, ~~strikethrough~~ and [links](https://pele.run).

## How a save becomes pixels

```mermaid
flowchart LR
  A[File saved] -->|fs event| B(Parse blocks)
  B --> C{Block changed?}
  C -->|no| D[Reuse cached rows]
  C -->|yes| E[Pele SVG]
  E --> F[resvg PNG]
  F --> G[kitty graphics]
  D --> H[Diff painter]
  G --> H
```

## The pieces

| Stage | Library | Runs on |
|---|---|--:|
| Parse | pulldown-cmark | main thread |
| Mermaid layout | Pele in QuickJS | pele thread |
| Rasterize | resvg + tiny-skia | rayon pool |
| Paint | kitty placeholders | main thread |

> [!TIP]
> Press `]` and `[` to jump between headings, and `G` to follow the end of the file while an agent writes it.

### A sequence

```mermaid
sequenceDiagram
  participant E as Editor
  participant M as tkmp
  participant T as Terminal
  E->>M: write plan.md
  M->>M: hash blocks, diff
  M->>T: transmit changed images
  M->>T: repaint changed rows
  T-->>E: you see it
```

### State of a diagram

```mermaid
stateDiagram-v2
  [*] --> Pending
  Pending --> Ready: Pele + resvg
  Pending --> Failed: syntax error
  Failed --> Pending: you fix it
  Ready --> Pending: you edit it
```

## Checklist

- [x] Spike: Rex draws unicode placeholders
- [x] Pele runs headless in QuickJS
- [ ] Ship it to the NAS
  - cross-compile for linux-x86_64
  - copy to `~/.local/bin`

1. Open a split: `rex split -- tkmp plan.md`
2. Edit `plan.md` anywhere
3. Watch the gutter light up where it changed

```rust
fn main() {
    println!("code blocks get a background and wrap at the edge");
}
```

### Some more diagram types

```mermaid
pie title Where the milliseconds go
  "rasterize" : 70
  "Pele" : 8
  "parse + layout" : 7
  "terminal write" : 15
```

```mermaid
classDiagram
  class App {
    +reload()
    +relayout()
    +paint()
  }
  class Pele {
    +render(src, maxWidth)
  }
  App --> Pele : requests SVG
  App --> Raster : spawns jobs
```

```mermaid
gantt
  title Building tkmp
  dateFormat YYYY-MM-DD
  section Core
  Spike            :done, s1, 2026-10-23, 1d
  Viewer           :active, v1, after s1, 2d
  section Polish
  NAS build        :n1, after v1, 1d
```

---

That's the tour. Edit this file while it's open and watch it update.
