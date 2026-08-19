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
- A note is laid out at its full size and clipped at paint time, never before.
  Widgets fit themselves to the rect they are handed, so drawing a partly
  off-screen note into a rect already trimmed to the viewport made it re-wrap its
  text as it approached an edge - the board behaved like a set of shrinking boxes
  rather than paper under a window frame. Instead `View::note_cells` returns the
  full footprint (signed, so it can start left of or above the viewport), the
  note renders into a private buffer of that size, and only the visible window is
  copied onto the frame. The layout is a function of the note, not of where the
  camera happens to be. **Done.**
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
- Storage runs through `FileStore`, the file-backed `Store`: `~/pinz-board` (or
  `$PINZ_HOME`), one directory per board, one markdown file per pin with its
  position in a small frontmatter header. The renderer did not change to gain it,
  which is what the seam was for. **Done.**
- Saving happens as you work, not only on exit. A corkboard is meant to stay
  open, so `App` counts changes and the runner writes when that count moves -
  except mid-drag, which would otherwise rewrite a pin's file on every
  mouse-move. Git sync stays at the edges (pull on open, commit and push on
  quit), so a gesture never mints a commit. **Done.**
- The loop **blocks** for input rather than running a frame clock, which is
  what keeps pinz at zero CPU sitting open on a desk. The one exception is
  while the camera is travelling to a jump's destination, when it polls at a
  frame budget: idle still means idle, because idle means nothing is moving.
  Elapsed time is handed to `App`, never read by it. **Done**, see
  `design/specs/2026-08-19-camera-glide.md`.
- Not `ratatui-3d`: the board is fundamentally 2D; 3D buys nothing here.

## The seam: storage is swappable

Every tool talks to data through the `Store` trait, never to files or a network
directly. Today: an in-memory store (demo and tests) and `FileStore`, the
git-backed files store (local-first, offline, versioned - matches the notez
philosophy). Later, **if** a real need appears, a remote backend is just another
`Store` implementation and the tools do not change.

Triggers that would justify building the backend (not before):

- two machines editing the same data and hand-resolving merge conflicts,
- a cross-tool query that otherwise forces reading everything,
- wanting instant cross-device propagation,
- access from a client that cannot `git clone` (a phone, a web view).

When that day comes, build it in Rust (e.g. axum + sqlx/Postgres) behind the
same trait - it doubles as a real backend portfolio piece.

## Pins are separate from notes (one-way, on purpose)

A pin is **not** a notez2 note. It is pinz's own markdown file in pinz's own
repo, and nothing in pinz reads or writes a notez2 workspace.

The alternative was tempting and wrong: make a board a spatial *view* over the
notez corpus, so any existing note could be pinned. It buys reach and costs the
thing that matters. notez2 promises its files round-trip byte-for-byte through
the CLI, nvim and epoz; a second tool editing those files in place has to parse
and faithfully reproduce a format it does not own, and position data - which
changes on every drag - would turn each mouse gesture into a diff in the notes
repo.

So the relationship runs one way: a pin that turns out to matter **graduates**
into a notez2 note, via `notez` (or `notez-core` in-process from epoz), leaving
notez2 the sole owner of its own format. Nothing flows back.

That separation is also why pins live in their own git repo rather than inside
`~/notez`. Auto-push in a shared repo would carry along unrelated commits, and
auto-pull would be blocked by unrelated work in progress (`~/notez` runs
`pull.rebase` with no autostash, so a rebase pull simply refuses while the tree
is dirty). Its own repo makes pinz's sync entirely pinz's business.

## Sync: stop rather than guess

`pinz sync` is pinz's own command, not a hook into another tool's. Pull on open,
commit and push on quit, and both on demand. A fetch that fails - offline, no
remote, no upstream - is not an error: pinz says so and carries on with local
files. But anything needing a judgement call about whose version of a pin wins
aborts the rebase, leaves the repo exactly as it was, and hands it back. A
corkboard has no business merging your notes.

## Dual target

- Standalone Ratatui TUI (`pinz-tui`).
- A tab inside epoz (Rust + Svelte). Epoz's Rust side hosts `pinz-core`; the
  Svelte tab renders essentially what `design/pinz-demo.html` shows and reaches
  the core through Tauri commands.

Because epoz will also grow a TUI, the same core-plus-renderers discipline
applies to it - which is the whole reason to keep logic out of the UI.
