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

## Run it

```sh
cargo run --bin pinz            # opens your board (~/pinz, created on first run)
cargo run --bin pinz -- nord    # ...starting in a chosen theme
cargo run --bin pinz -- --demo  # seeded demo boards; writes nothing to disk
cargo run --bin pinz -- sync    # pull, commit and push the pin repo, then exit
cargo test                      # runs the core + renderer unit tests
cargo install --path crates/pinz-tui   # put `pinz` on your PATH
```

Sync subcommands, each looking at the repo's state first and doing only what
that state calls for:

| Command | Alias | Does |
| ------- | ----- | ---- |
| `pinz sync` | `s` | whatever is needed: pull what's waiting, commit what changed, push what's ahead |
| `pinz status` | `st` | reports what is waiting and changes nothing |
| `pinz pull` | `down` | only brings the other machine's pins in |
| `pinz push` | `up` | only commits and sends this machine's pins |

Inside the app: **scroll** or `+`/`-` to zoom through the levels of detail,
**drag a note** to move it, **drag the board** to pan (arrow keys too), `Tab` or
`1`-`9` to switch worlds, `n` for a new note, `e` or `enter` to edit the selected
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

Pins are pinz's own, not notez2 notes. They sit in **`~/pinz`** (override with
`$PINZ_HOME`): one directory per board, one markdown file per pin.

```
~/pinz/
  ideas/
    2026-08-01-143022-fortnox-integrations.md
  wavez/
```

```markdown
---
x: 720
y: 380
z: 4
color: green
---
# Fortnox integrations

Small automations for Swedish e-commerce bookkeeping.
```

One file per pin is a sync decision: these files live in a git repo shared
between machines, and per-pin files conflict only when the *same pin* was edited
on both ends, where a single board file would conflict on any change at all.
Saves are incremental - only files whose bytes actually changed get rewritten, so
dragging a note doesn't churn the history.

## Syncing your machines

`~/pinz` is an ordinary git repo. It does not exist until pinz makes it, so
create it *first* and give it a remote second:

```sh
pinz sync                                            # creates ~/pinz + first commit
cd ~/pinz
gh repo create pinz-board --private --source=. --push
```

Running `gh repo create --source=.` before `pinz sync` will fail: with no
`~/pinz` to change into you are still in whatever directory you started in, and
if that one already has an `origin` you get `Unable to add remote "origin"`.

On the second machine, clone it into place and you are done:

```sh
git clone git@github.com:<you>/pinz-board.git ~/pinz
```

After that pinz pulls when it opens and commits and pushes when you quit, and
`pinz sync` does both on demand. It only ever touches its own repo, so it can
never sweep up or be blocked by work in progress anywhere else. If the same pin
changed on both machines, pinz **stops**, leaves the repo exactly as it was and
tells you - resolving that is a human's call, not a corkboard's.

Pins are deliberately separate from your notez2 notes. A pin that turns out to
matter is meant to graduate into a real note later; nothing here reads or writes
a notez2 workspace.
