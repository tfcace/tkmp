# tkmp

> [!NOTE]
> This project is vibe-coded with ❤️ Guided by a human.

A live markdown previewer for Rex and other terminals that speak the kitty graphics protocol. Run it in a split next to whatever is writing the file (you, an editor or an agent) and it re-renders on every save.

```sh
rex split -- tkmp plan.md
```

What makes it different from glow and mdcat:

- **Mermaid diagrams as real images.** [Pele](https://pele.run) lays them out (its bundle is embedded in the binary and runs in QuickJS), and resvg rasterizes them.
- **Big-type headings.** H1 and H2 are drawn in Inter Display, exactly 3 and 2 rows tall, so a long plan has visible structure.
- **Colors from your terminal.** tkmp asks the terminal for its foreground, background and palette at startup, and draws diagrams with those colors on a transparent background.
- **Built to keep up with agents.** Blocks are hashed, so a save only redraws what changed. Changed blocks get a mark in the gutter for 1.5 s. `G` follows the end of the file like `tail -f`, and scroll position stays put when text is inserted above you.

## Install

```sh
cargo install --git https://github.com/tfcace/tkmp
```

Or `cargo install --path .` from a clone.

A single binary of about 6 MB, with the fonts and Pele embedded. It needs no Node, no browser and no system fonts (it uses system fonts only for text the embedded fonts can't draw, such as Hebrew or emoji in diagram labels).

## Open from Rex with a key

`rex/tkmp.lua` binds a key that finds the `.md` path nearest the cursor in the focused block (in `ls` output, an agent's transcript, a link's target) and opens it in tkmp beside that block. Pressing it again replaces that split, and `q` closes it.

```sh
cp rex/tkmp.lua ~/.config/rex/
echo 'require("tkmp").setup{ key = "cmd+shift+m" }' >> ~/.config/rex/init.lua
rex config reload
```

To pick a file when several are on screen, right-click its link, choose Copy Link, then press the key. The copied path wins if it's also on screen, and each copy counts once, so the next press goes back to the nearest path. Copy Link only appears on text Rex treats as a link, such as absolute paths and OSC 8 links, not a bare name like `notes.md`.

The action is also in the command palette as "tkmp: preview the markdown file on screen". Clicking a link can't open tkmp yet: Rex opens clicked file links in the macOS default app, and scripts can't intercept them.

## Keys

| Key | Action |
|---|---|
| `j` / `k`, arrows, mouse wheel | scroll |
| `space` / `b` | page down / up |
| `d` / `u` | half page |
| `g` / `G` | top / bottom (bottom turns on follow) |
| `]` / `[` | next / previous heading |
| `ctrl-r` | redraw |
| `q` | quit |

The status line shows the scroll position, follow mode, and how long the last change took from file event to finished frame.

## Speed

`tkmp bench FILE` times each stage without a terminal. For [`examples/tour.md`](examples/tour.md) on an M-series Mac, with a 16×28 px cell:

```
pele engine load     50.67ms   (once, on its own thread)
parse                 0.16ms
text layout           0.10ms
diagram              0.3-1.7ms Pele, 1.1-2.8ms raster each
headings              0.33ms each
cold raster           2.74ms   (all 10 images in parallel)
```

Measured live in Rex from file event to painted frame: about 19 ms for a paragraph edit (15 ms of that is the debounce that keeps half-written files off screen), 27 ms for a diagram edit, and 56 ms to open the tour cold.

How it stays fast:

- **Images are keyed by content.** A diagram image's key is the hash of its themed SVG plus its size in cells, so typing in a paragraph never redraws an image, and a resize that doesn't change the diagram's layout reuses it.
- **Pixels go to the terminal once.** Images use kitty's Unicode placeholders, so they live in the text grid. Scrolling only rewrites placeholder characters, and the painter only rewrites rows that changed, inside a synchronized update.
- **Resizes rescale first.** Existing images are resized in the terminal right away, and crisp ones are rendered once the size has been stable for 80 ms.

## Notes

- `TKMP_LOG=FILE` logs every image transmit, placement and delete, plus resizes, for debugging.
- `TKMP_NO_GRAPHICS=1` forces text-only output. Without kitty graphics, tkmp falls back on its own: headings become bold text and diagrams show their source.
- In a narrow split, a diagram that would have to shrink by more than 25% is laid out vertically instead, so its labels stay readable.
- While a diagram is broken, its last good render stays on screen with the parse error and its position underneath.
- Inter and JetBrains Mono are under the SIL Open Font License; Pele is MIT. Their license files are in [`assets/`](assets).
- [`assets/rowcolumn-diacritics.txt`](assets/rowcolumn-diacritics.txt) is the table of placeholder diacritics from the [kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders).
