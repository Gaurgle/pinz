# pinz

A spatial **bulletin board in your terminal**. Quick ideas and todos as
fixed-size post-it notes on a big, pannable, zoomable board - like a real cork
board, but in a TUI. Notes are movable, stackable, editable, and grouped into
switchable "worlds".

> Status: **early scaffold.** The look is prototyped (see `design/`), the core
> model and projection math exist and are tested, and the terminal UI is not
> drawn yet. Nothing here is usable as an app yet - this repo is the skeleton we
> build onto.

## Why it is built this way

pinz is meant to run as a standalone Ratatui TUI **and** as a tab inside
[epoz](https://github.com/Gaurgle) (a Rust + Svelte desktop app). So the logic
lives in one UI-agnostic crate and the renderers sit on top:

```
crates/
  pinz-core/   domain model + world/screen projection + storage seam (no UI, no I/O beyond the seam)
  pinz-tui/    terminal renderer (Ratatui) - currently a stub binary named `pinz`
design/
  pinz-demo.html   interactive look-and-feel prototype (open in a browser)
  DESIGN.md        the decisions behind the model, the zoom ladder, and the seam
```

The core depends on no renderer; renderers depend on the core. That one-way
arrow is what lets a terminal app and a desktop app share one brain.

## Run it

```sh
cargo run --bin pinz     # prints the seeded boards + a projection check (stub)
cargo test               # runs the core's unit tests
```

To see the intended look and interactions, open `design/pinz-demo.html` in a
browser: scroll to zoom through the levels of detail, drag notes to move them,
drag the board to pan, and switch worlds via the header tabs.

## Data model, briefly

A post-it is a note (title + body) plus spatial metadata (`x`, `y`, `z`,
`board`, `color`). That extra metadata is exactly what would live in a notez2
note's frontmatter, so a note can round-trip between pinz and a notez2
workspace. Today the only store is in-memory; a git-backed store and, later, a
shared backend are planned implementations behind the same `Store` trait.
