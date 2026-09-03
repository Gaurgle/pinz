# CLAUDE.md - pinz

## What this is

A spatial bulletin board in the terminal: fixed-size post-it notes on a big
pannable, zoomable board, grouped into switchable "worlds". Pins live in their
own git repo (`~/pinz-board`, `$PINZ_HOME` to move it) and sync between machines
with `pinz sync`.

pinz is built to run as a standalone Ratatui TUI **and** as a tab inside epoz, a
Rust + Svelte desktop app. That is why the logic lives in a UI-agnostic core with
renderers on top, and it is the constraint that most shapes the code.

<!-- house-rules:start v1 -->
## House rules

These mirror the global config at `~/claude-config`, which is machine-local and
therefore invisible to cloud and mobile sessions. They apply here regardless of
where the session runs.

- **Never use em-dashes or en-dashes** in any output: chat, files, or code. Use
  a hyphen, a colon, parentheses, or rewrite the sentence.
- **Conventional Commits** (`feat:`, `fix:`, `refactor:`, `docs:`, `test:`,
  `cleanup:`), first line under 72 characters. Body is for the why, not the what.
- **No `Co-Authored-By` lines.** Ever.
- **Present the full `git commit -m "..."` command for review; do not commit.**
  The human commits and pushes. The one exception is **"ship it"**: commit and
  push, plus a PR where this repo's flow calls for one. It never merges, tags,
  releases or deploys, and never ships a red build. The **Ship policy** line
  below says what it does here; with no such line, push the current branch and
  never `main`.
- **Never edit, delete or disable a test to make code pass.** Fix the code. If a
  test looks wrong, stop and say so rather than changing it.
- **Ask before** adding or removing dependencies, changing schema or API
  contracts, touching CI config, or deleting files.
<!-- house-rules:end -->

## Current state

Usable. A pannable, zoomable board with a four-level zoom ladder, switchable
worlds, mouse move and select, in-place editing through one word-wrapped editor,
text selection with copy and paste, a moving window over a note whose text
outgrew it, and git sync at the edges.

There is no `epoz` renderer yet - `pinz-core` has exactly one
consumer today, but its API is designed as though it had two.

## Where things are written down

| Path | What it holds |
|---|---|
| `design/DESIGN.md` | Why the model, the zoom ladder, and the storage seam are the way they are |
| `design/specs/` | One dated file per feature, written and approved before the feature was built |
| `design/pinz-demo.html` | Interactive look-and-feel prototype; open in a browser to compare against the intended look |
| `TODO.md` | The running backlog of what is next |
| `RELEASING.md` | The steps a release takes, in order, and why that order |
| `README.md` | Install, run, keys, sync commands |

`design/DESIGN.md` holds standing decisions; a file in `design/specs/` describes
one feature and is dated. Where they disagree, the newer spec wins and DESIGN.md
should be updated to match. Read the relevant spec before changing a feature it
covers.

## Working conventions

- **Feature branch and a PR into `main`.** Not direct to main, even solo.
- **Ship policy:** commit, push the branch, open or update the PR. Never push
  to `main` and never merge; the PR is the review surface and you merge it.
- Test-first. Every new function gets a failing test before it gets an
  implementation.
- Formatting is **hand-maintained, not `rustfmt`**. `cargo fmt` reformats every
  file in the repo, including ones you did not touch. Do not run it. Match the
  surrounding style instead.

## Shipping conventions

- **The package in `crates/pinz-tui` is published as `pinz`.** The directory
  keeps the renderer suffix because the core is built for several renderers;
  the package takes the plain name so `cargo install pinz` matches the binary.
  Use `-p pinz` with cargo, not `-p pinz-tui`.
- **CI runs tests on Linux and macOS, clippy with `-D warnings`, and a build on
  the declared `rust-version`.** No `cargo fmt` check: formatting here is
  hand-maintained. Raising `rust-version` means editing the msrv job too.
- **Releases go out from a tag.** `RELEASING.md` has the order, which matters:
  `pinz-core` reaches crates.io before `pinz`, because the binary crate depends
  on a published version of the core.

## Build

```
cargo test --workspace          # the whole suite
cargo test -p pinz editor::   # one module (the package in crates/pinz-tui)
cargo clippy --workspace --all-targets
cargo run --bin pinz            # the app, against your real ~/pinz-board
PINZ_HOME=/tmp/scratch cargo run --bin pinz   # against a throwaway board
```

The TUI cannot be driven from a non-interactive shell: it fails with `Device not
configured` without a TTY. Assertions about rendering go through Ratatui's
`TestBackend`, which needs no terminal.

## Domain invariants - do not violate

- **`pinz-core` depends on no renderer.** Renderers depend on the core, never
  the reverse. That one-way arrow is the whole reason a terminal app and a
  desktop app can share one brain.
- **`pinz-core` does no I/O except through `Store`.** The trait is deliberately
  coarse - whole-workspace `load`/`save`, plus `delete_board`, which exists
  because a save cannot tell a deleted world from one that was never there -
  and widens only when a real caller needs it, so it does not grow speculative
  methods nobody uses.
- **One file per pin, one directory per board.** Pins live in a git repo synced
  between machines; a single board file would conflict whenever both ends
  touched anything on that board. Per-pin files mean a drag does not churn the
  whole board's history.
- **A note's world size is fixed** (`NOTE_W`, `NOTE_H`). Zoom changes how big a
  note looks, never how big it is. Layout, hit-testing and stacking all assume
  uniform notes.
- **No two pins on a board share a spot.** Every placement - a new pin, a
  dropped pin, a pin arriving from another world - goes through
  `Board::free_spot`, which cascades down-right past anything within
  `CASCADE_X`/`CASCADE_Y`. The threshold is a *gap*, not equality: pin files
  store x and y rounded to whole units, so a near miss becomes an exact overlap
  on the next reload and buries the pin underneath.
- **Every spatial operation goes through `View`** - pan, zoom, drag, hit-test.
  It is the single projection between world and screen, so what you click is
  what the math says is under the cursor.
- **Rendering and hit-testing must run the same layout.** The tab strip is laid
  out in `app.rs` and drawn from that same list; the editor's wrap lives in
  `wrap.rs` and is used by both the renderer and the mouse. Computing either one
  twice is how they drift.
- **`App` performs no I/O and no drawing.** It is a state machine: `ui.rs` reads
  it to render, `main.rs` does the terminal work. A copy is queued in
  `pending_copy` for the runner to deliver, not written from `App`. This is what
  keeps input logic testable with no terminal attached.
- **Clipboard copy sends plain OSC 52, never tmux's DCS passthrough.**
  Passthrough needs `allow-passthrough on`, off by default, so wrapping it makes
  tmux discard the copy silently. tmux also needs `set-clipboard on`; the README
  says so.
