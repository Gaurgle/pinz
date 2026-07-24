# pinz - design notes

The decisions behind pinz, so the "why" survives. Kept close to the code; update
it when a decision changes.

## What it is

A spatial bulletin board for the terminal. Ideas and todos are fixed-size
post-it notes on a large board you pan and zoom around. Notes are movable
(mouse), stackable (to prioritize), editable, and grouped into named "worlds"
you switch between via header tabs. Think cork board, rendered in cells.

## The mental model: a tiny 2D engine

Three pieces, and everything else falls out of them:

- **World** - an unbounded `f64` coordinate space holding notes (`x`, `y`, `z`).
- **Camera** - an origin (world point at the viewport's top-left) plus a zoom
  level.
- **Projection** - `world <-> screen`, both directions.

Get the projection **and its inverse** right first. Panning, zooming, and mouse
hit-testing are all just applications of those two functions. The inverse
(`screen -> world`) is what turns a click into "which note did I grab".

Terminal cells are about twice as tall as wide, so the projection carries a
`cell_aspect` (`cell_width / cell_height`): ~0.5 for a terminal, 1.0 for square
pixels (the GUI and the HTML demo). Without it, a world-square note looks
stretched in the terminal.

## Zoom is a discrete ladder, not a slider

A terminal is a fixed grid of cells; there is no continuous zoom. So zoom snaps
to four levels, and the level decides **how a note is rendered**, not just its
size (its "level of detail"):

| Level    | A note shows            | You can        |
| -------- | ----------------------- | -------------- |
| Cluster  | a solid colored block   | overview / move |
| Titles   | its title only          | select         |
| Preview  | title + body preview    | select         |
| Document | full note               | **edit**       |

(A fifth, more zoomed-out "dots" level was tried and dropped - a dot conveys too
little to earn a level; Cluster's colored blocks are the whole-board overview.)

This also implies a **split render path**: zoomed in (Titles..Document), notes
are real widgets (a box + text) placed at their projected rect; zoomed out
(Cluster), each note is a solid colored block painted straight into the buffer.
The projection layer chooses the path per level.

## Ratatui stack

What the renderer settled on (`crates/pinz-tui`):

- Own projection layer as the spine (not a widget). `pinz-tui/src/view.rs` wraps
  `pinz-core`'s `Projection` and adds the two terminal-only corrections the core
  leaves out: the cell aspect (~0.5) and a display-unit scalar that rescales the
  core's pixel-tuned zoom ladder to cell proportions. Everything - pan, zoom,
  hit-test - goes through it, so the mouse inverse stays exact. **Done.**
- Split render path by zoom: zoomed in (titles/preview/document), notes are
  `Block` + `Paragraph` at their projected `Rect`; zoomed out (cluster), each is
  a solid colored block painted straight into the buffer. **Done.**
- A custom tab strip for worlds and lightweight edge bars as camera-position
  indicators; a zoom indicator dot per level (not `Gauge` - too heavy). **Done.**
- Theming is swappable, not baked in. A `Theme` is the full palette (backgrounds,
  text, one accent, and the eight note accents the core names abstractly); the
  app holds one active theme and every widget reads its color from there, so a
  swap re-skins everything with no other change. Ships with a handful (Catppuccin
  Mocha, Tokyo Night, Gruvbox, Nord, Solarized Light - deliberately one light
  theme, to keep the renderer honest about not assuming a dark background); cycle
  with `t`, or pick one at launch. The core stays color-agnostic. **Done.**
- Editing: one editor for the whole note - the first line is the title, the rest
  the body. `e` or `enter` opens it, `enter` adds a line, `esc` saves. It is a
  hand-rolled multi-line buffer (`pinz-tui/src/editor.rs`) shown at the document
  level, word-wrapped to the note width with the cursor mapped through the wrap
  (`wrap_rows` in `ui.rs`) so text never runs off the edge. Not `tui-textarea` -
  it still targets ratatui 0.29 and would pull a second, incompatible ratatui
  into the tree; the editor we need is small enough to own. **Done.**
- Storage still runs through the in-memory `Store`; the app calls `save` on exit
  so a git-backed store drops in without touching the renderer. **Seam ready.**
- Not `ratatui-3d`: the board is fundamentally 2D; 3D buys nothing here.

## The seam: storage is swappable

Every tool talks to data through the `Store` trait, never to files or a network
directly. Today: an in-memory store. Next: a git-backed-files store (local-first,
offline, versioned - matches the notez philosophy). Later, **if** a real need
appears, a remote backend is just another `Store` implementation and the tools
do not change.

Triggers that would justify building the backend (not before):

- two machines editing the same data and hand-resolving merge conflicts,
- a cross-tool query that otherwise forces reading everything,
- wanting instant cross-device propagation,
- access from a client that cannot `git clone` (a phone, a web view).

When that day comes, build it in Rust (e.g. axum + sqlx/Postgres) behind the
same trait - it doubles as a real backend portfolio piece.

## notez2 compatibility (aligned, not coupled)

A post-it = a notez2 note (markdown + frontmatter) + spatial metadata (`x`, `y`,
`z`, `board`, `color`). Keeping the note shape honest to that means a note can
round-trip to a notez2 file later, without coupling pinz to notez2 now. Separate
repo for the time being.

## Dual target

- Standalone Ratatui TUI (`pinz-tui`).
- A tab inside epoz (Rust + Svelte). Epoz's Rust side hosts `pinz-core`; the
  Svelte tab renders essentially what `design/pinz-demo.html` shows and reaches
  the core through Tauri commands.

Because epoz will also grow a TUI, the same core-plus-renderers discipline
applies to it - which is the whole reason to keep logic out of the UI.
