# pinz

A spatial **bulletin board in your terminal**. Quick ideas and todos as
fixed-size post-it notes on a big, pannable, zoomable board - like a real cork
board, but in a TUI. Notes are movable, stackable, editable, and grouped into
switchable "worlds".

> Status: **usable.** A pannable, zoomable board of post-it notes with a
> four-level detail ladder, switchable worlds, and mouse move/select. Notes edit
> in place through one word-wrapped editor - the first line is the title, the
> rest the body. Pins persist to their own git repo as you work and sync between
> machines with `pinz sync`.

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

## Install

Needs a Rust toolchain and git. There are two repos and you want both: this one
is the **program**, and a second private one holds your **pins**. Keeping your
notes out of the source repo is the point - see [Where pins live](#where-pins-live).

**First machine**, from scratch:

```sh
git clone git@github.com:<you>/pinz.git ~/repos/pinz
cd ~/repos/pinz
cargo install --path crates/pinz-tui                 # puts `pinz` on your PATH

pinz sync                                            # creates ~/pinz-board + first commit
cd ~/pinz-board
gh repo create pinz-board --private --source=. --push
```

Order matters: `~/pinz-board` does not exist until `pinz sync` makes it, so running
`gh repo create --source=.` first leaves you in the wrong directory, and if that
one already has an `origin` you get `Unable to add remote "origin"`.

**Every machine after that** - clone the program, install it, clone the pins:

```sh
git clone git@github.com:<you>/pinz.git ~/repos/pinz
cargo install --path ~/repos/pinz/crates/pinz-tui
git clone git@github.com:<you>/pinz-board.git ~/pinz-board
pinz status                                          # should say: in sync
```

That is the whole setup. `pinz` deliberately does not rely on `dotsync` or any
other machine-setup tool; it syncs itself.

To upgrade later, pull this repo and re-run `cargo install --path
crates/pinz-tui`.

## Run it

```sh
pinz                            # opens your board
pinz nord                       # ...in a chosen theme

# a throwaway board, to try things without touching your real pins:
PINZ_HOME=/tmp/pinz-scratch pinz

# from a checkout, without installing:
cargo run --bin pinz
cargo test                      # runs the core + renderer unit tests
```

Sync subcommands, each looking at the repo's state first and doing only what
that state calls for:

| Command | Does |
| ------- | ---- |
| `pinz sync` | whatever is needed: pull what's waiting, commit what changed, push what's ahead |
| `pinz status` (`st`) | reports what is waiting and changes nothing |
| `pinz pull` | only brings the other machine's pins in |
| `pinz push` | only commits and sends this machine's pins |

`st` is the only abbreviation, because it is the only one where guessing wrong
is free - the worst a misread `st` can do is print a report. Anything that moves
commits has to be typed in full. Short aliases were tried and dropped: `s` reads
as *status* to anyone whose git is set up that way but would have committed and
pushed, and `up` reads as *update*, meaning pull, while it pushed.

Inside the app: **scroll** or `+`/`-` to zoom through the levels of detail,
**drag a note** to move it, **drag the board** to pan (arrow keys too), `Tab` or
`1`-`9` to switch worlds (or click a tab), `w` or the `+` in the tab strip for a
new world, `n` for a new note, `e` or `enter` to edit the selected
note (first line is the title, the rest the body; `enter` adds a line,
`alt`/`ctrl`+`←`/`→` jumps by word, `ctrl`/`alt`+`backspace` deletes a word,
`ctrl`+`u` clears the line, `esc` saves),
`c` cycles its color (`C` backwards), `d` to delete, `t` to cycle the theme (`T`
backwards), `q` to quit.

There are a few built-in themes - Catppuccin Mocha (default), Tokyo Night,
Gruvbox, Nord, and Solarized Light. Cycle them live with `t`, or start in one by
name: `pinz -- gruvbox` (the match is loose, so `pinz -- light` works too).

To compare against the intended look, open `design/pinz-demo.html` in a browser -
the TUI mirrors its interactions in cells.

## Where pins live

Pins are pinz's own, not notez2 notes. They sit in **`~/pinz-board`** (override with
`$PINZ_HOME`): one directory per board, one markdown file per pin.

```
~/pinz-board/
  ideas/
    2026-08-01-143022-buy-a-new-lamp.md
  sketches/
```

```markdown
---
x: 720
y: 380
z: 4
color: green
---
# buy a new lamp

The one by the desk flickers.
```

One file per pin is a sync decision: these files live in a git repo shared
between machines, and per-pin files conflict only when the *same pin* was edited
on both ends, where a single board file would conflict on any change at all.
Saves are incremental - only files whose bytes actually changed get rewritten, so
dragging a note doesn't churn the history.

## Syncing your machines

Setup lives under [Install](#install); this is what sync does once it is running.

`~/pinz-board` is an ordinary git repo. pinz pulls when it opens and commits and pushes when you quit, and
`pinz sync` does both on demand. It only ever touches its own repo, so it can
never sweep up or be blocked by work in progress anywhere else. If the same pin
changed on both machines, pinz **stops**, leaves the repo exactly as it was and
tells you - resolving that is a human's call, not a corkboard's.

Pins are deliberately separate from your notez2 notes. A pin that turns out to
matter is meant to graduate into a real note later; nothing here reads or writes
a notez2 workspace.
