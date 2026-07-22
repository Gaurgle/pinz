# pinz

A spatial **bulletin board in your terminal**. Quick ideas and todos as
fixed-size post-it notes on a big, pannable, zoomable board - like a real cork
board, but in a TUI. Notes are movable, stackable, editable, and grouped into
switchable "worlds".

> Status: **playable.** The core model and projection math are tested, and the
> terminal UI now draws: a pannable, zoomable board of post-it notes with the
> five-level detail ladder, switchable worlds, mouse move/select, and full
> **title + body editing** (a small multi-line text editor with cursor movement).
> Storage is still in-memory, so nothing persists across runs yet - a git-backed
> store is the next step.

## Why it is built this way

pinz is meant to run as a standalone Ratatui TUI **and** as a tab inside
[epoz](https://github.com/Gaurgle) (a Rust + Svelte desktop app). So the logic
lives in one UI-agnostic crate and the renderers sit on top:

```
crates/
  pinz-core/   domain model + world/screen projection + storage seam (no UI, no I/O beyond the seam)
  pinz-tui/    terminal renderer (Ratatui) - the `pinz` binary
design/
  pinz-demo.html   interactive look-and-feel prototype (open in a browser)
  DESIGN.md        the decisions behind the model, the zoom ladder, and the seam
```

The core depends on no renderer; renderers depend on the core. That one-way
arrow is what lets a terminal app and a desktop app share one brain.

## Run it

```sh
cargo run --bin pinz            # launches the terminal app (seeded with demo boards)
cargo run --bin pinz -- nord    # ...starting in a chosen theme
cargo test                      # runs the core + renderer unit tests
```

Inside the app: **scroll** or `+`/`-` to zoom through the levels of detail,
**drag a note** to move it, **drag the board** to pan (arrow keys too), `Tab` or
`1`-`9` to switch worlds, `n` for a new note, `e` to edit it, `d` to delete, `t`
to cycle the theme (`T` backwards), `q` to quit.

While editing a note: type to insert at the cursor, arrow keys / `Home` / `End`
to move, `Tab` to switch between the **title** and the **body**, `Enter` for a
new line in the body (from the title it drops you into the body), and `Esc` when
you're done. Edits apply live.

There are a few built-in themes - Catppuccin Mocha (default), Tokyo Night,
Gruvbox, Nord, and Solarized Light. Cycle them live with `t`, or start in one by
name: `pinz -- gruvbox` (the match is loose, so `pinz -- light` works too).

To compare against the intended look, open `design/pinz-demo.html` in a browser -
the TUI mirrors its interactions in cells.

## Data model, briefly

A post-it is a note (title + body) plus spatial metadata (`x`, `y`, `z`,
`board`, `color`). That extra metadata is exactly what would live in a notez2
note's frontmatter, so a note can round-trip between pinz and a notez2
workspace. Today the only store is in-memory; a git-backed store and, later, a
shared backend are planned implementations behind the same `Store` trait.
