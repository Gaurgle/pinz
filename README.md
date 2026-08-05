# pinz

A spatial **bulletin board in your terminal**. Quick ideas and todos as
fixed-size post-it notes on a big, pannable, zoomable board - like a real cork
board, but in a TUI. Notes are movable, stackable, editable, and grouped into
switchable "worlds".

> Status: **usable.** A pannable, zoomable board of post-it notes with a
> four-level detail ladder, switchable worlds, and mouse move/select. Notes edit
> in place through one word-wrapped editor - the first line is the title, the
> rest the body - with text selection, copy and paste. Pins persist to their own
> git repo as you work and sync between machines with `pinz sync`.

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
  specs/           one file per feature, written before it was built
```

The core depends on no renderer; renderers depend on the core. That one-way
arrow is what lets a terminal app and a desktop app share one brain.

## Install

Needs a Rust toolchain and git. There are two repos and you want both: this one
is the **program**, and a second private one holds your **pins**. Keeping your
notes out of the source repo is the point - see [Where pins live](#where-pins-live).

### 1. Install the program

The same on every machine:

```sh
git clone git@github.com:<you>/pinz.git ~/repos/pinz
cargo install --path ~/repos/pinz/crates/pinz-tui    # puts `pinz` on your PATH
```

`--path` has to name the crate, not the repo root: the root is a workspace
manifest with no package in it, and cargo will say so.

### 2. Get your pins

Now answer one question, because the two answers do **not** run the same
commands:

> **Does a pin repo already exist - did any other machine ever run `pinz`?**

**No. This is the first machine, and the pins do not exist yet.** Let pinz
create the board, then give it a home:

```sh
pinz sync                                            # creates ~/pinz-board + first commit
cd ~/pinz-board
gh repo create pinz-board --private --source=. --push
```

Order matters: `~/pinz-board` does not exist until `pinz sync` makes it, so running
`gh repo create --source=.` first leaves you in the wrong directory, and if that
one already has an `origin` you get `Unable to add remote "origin"`.

**Yes. The pins are already on GitHub.** Clone them, and do not run `pinz sync`
first:

```sh
git clone git@github.com:<you>/pinz-board.git ~/pinz-board
pinz status                                          # should say: in sync
```

⚠️ **Do not mix the two.** Running the first-machine block on a machine whose
pins already exist is the one way to get properly stuck: `pinz sync` sees no
board, builds a fresh one with a single blank pin, and commits it. That repo now
has a history unrelated to your real one, so `git remote add origin` cannot
rescue it - git refuses to merge unrelated histories, and the board opens empty
while your pins sit untouched on GitHub. The way out is to throw the new board
away and clone properly:

```sh
mv ~/pinz-board ~/pinz-board.bak                     # nothing of yours is in here
git clone git@github.com:<you>/pinz-board.git ~/pinz-board
pinz status
```

`pinz status` names this state whenever the board has no remote, and prints both
routes rather than guessing which one you are in.

That is the whole setup. `pinz` deliberately does not rely on `dotsync` or any
other machine-setup tool; it syncs itself.

To upgrade later, pull this repo and re-run `cargo install --path
crates/pinz-tui`.

## Run it

The first run creates the pin repo and opens on a single blank pin, ready to be
written on: press `e` and type.

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
| `pinz version` | prints the version - worth checking that both machines match |
| `pinz help` | the above, from the tool itself |

Two flags: `--no-sync` opens the board without touching git at all, and
`--theme <name>` (or a bare theme name) picks the starting theme.

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
`y` to copy the whole note, `c` cycles its color (`C` backwards), `d` to delete,
`u` to undo and `ctrl`+`r` to redo, `t` to cycle the theme (`T` backwards), `q`
to quit.

### Moving a pin to another world

**Drag a pin onto another world's tab** and let go. The tab lights up as you
cross it, a 📌 rides the cursor, and the footer names both the pin and where it
is headed, so a mis-aimed drop is caught before you release.

The pin keeps its position and lands on top of whatever is already on the target
board. You stay on the board you were clearing, which is what you want when
taking several pins off it in a row. Dropping on the board's own tab, on the `+`,
or anywhere else does nothing.

### Undo

`u` undoes, `ctrl`+`r` redoes, and pinz remembers the last 50 changes. Undo
covers anything that changes a board: new pins, deletes, recolors, edits, moves,
new worlds, and pins sent to another world. Undoing a change made on another
world takes you back to that world so you can see it happen.

The step size is what you would expect rather than what is easy: a whole drag is
one step, and a whole editing session in a note is one step, not one per
keystroke. Opening a note and closing it without typing records nothing at all.

Theme, zoom and pan are not board changes and are not undone.

### Marking text and copying

While editing a note, **drag across the text** to select it, or hold `shift` with
any movement key:

| Key | Does |
| --- | ---- |
| `shift`+`←``→``↑``↓` | extend the selection by a character or a line |
| `alt`/`ctrl`+`shift`+`←`/`→` | extend it by a word |
| `shift`+`home`/`end` | extend it to the start or end of the line |
| `ctrl`+`a` | select the whole note |
| `ctrl`+`c` | copy the selection |
| `ctrl`+`x` | cut it |
| `cmd`+`←`/`→` | jump to the start or end of the line |
| `y` (in nav) | copy the selected note whole, title and body |

`ctrl`+`c` still quits when nothing is selected, so the escape hatch survives.
Typing over a selection replaces it, and `cmd`+`v` pastes back in.

Copying uses **OSC 52**: the text is handed to your terminal, which owns the real
clipboard. That is what makes it work over SSH with no helper process. Supported
by iTerm2, Ghostty, kitty, WezTerm and Alacritty; **not** by macOS Terminal.app.
Inside tmux it needs `set -s set-clipboard on` in your tmux config.

`cmd`+`c` and `cmd`+`←`/`→` are bound too, but most terminals claim those chords
for themselves and never forward them - they work only where you can configure
the terminal to pass them through. Ghostty is confirmed to swallow `cmd`+`c`.
`ctrl`+`c` and `home`/`end` always work.

There are a few built-in themes - Catppuccin Mocha (default), Tokyo Night,
Gruvbox, Nord, and Solarized Light. Cycle them live with `t`, or start in one by
name: `pinz gruvbox` (the match is loose, so `pinz light` works too).

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

`~/pinz-board` is an ordinary git repo. pinz pulls when it opens, and commits and
pushes when you quit; `pinz sync` does both on demand. It only ever touches its own repo, so it can
never sweep up or be blocked by work in progress anywhere else. If the same pin
changed on both machines, pinz **stops**, leaves the repo exactly as it was and
tells you - resolving that is a human's call, not a corkboard's.

Pins are deliberately separate from your notez2 notes. A pin that turns out to
matter is meant to graduate into a real note later; nothing here reads or writes
a notez2 workspace.
