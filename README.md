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

**Homebrew** (macOS and Linux, prebuilt, no toolchain needed):

```sh
brew install Gaurgle/tap/pinz
```

**Cargo**, if you already have Rust:

```sh
cargo install pinz
```

**From a checkout**, for hacking on it:

```sh
git clone git@github.com:<you>/pinz.git ~/repos/pinz
cargo install --path ~/repos/pinz/crates/pinz-tui    # puts `pinz` on your PATH
```

`--path` has to name the crate directory, not the repo root: the root is a
workspace manifest with no package in it, and cargo will say so. The directory
is `pinz-tui` while the package it holds is published as `pinz`, because the
core is built for several renderers and only one of them is a terminal.

Building from source needs Rust 1.88 or newer (the floor the workspace
declares) and git. Prebuilt binaries for macOS on Apple silicon and Intel, and
for x86-64 Linux, are attached to every [release][releases].

[releases]: https://github.com/Gaurgle/pinz/releases

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
| `pinz version` | prints this build and the newest release, and whether they match |
| `pinz help` | the above, from the tool itself |

`pinz version` reads the release tags off the source repo with `git ls-remote`,
so it answers three things at once: what this machine runs, what has shipped,
and whether you are level, behind, or on a build that was never released. With
no network it says `unknown` for the release and still prints your build.

Two flags: `--no-sync` opens the board without touching git at all, and
`--theme <name>` (or a bare theme name) picks the starting theme.

`st` is the only abbreviation, because it is the only one where guessing wrong
is free - the worst a misread `st` can do is print a report. Anything that moves
commits has to be typed in full. Short aliases were tried and dropped: `s` reads
as *status* to anyone whose git is set up that way but would have committed and
pushed, and `up` reads as *update*, meaning pull, while it pushed.

Inside the app, **`?` shows every key** (`F1` while you are editing a note, where
`?` is a character you might be typing). The footer stays out of the way and
carries news instead: what a drag is about to do, what was copied, why a board
is read-only.

The same list in prose: **scroll** or `+`/`-` to zoom through the levels of
detail (from preview zoom up the wheel belongs to the pin under the pointer
and only zooms over bare board; `+`/`-` about the selected pin, which
ends up centered, or the middle of the view when nothing is selected), **drag a
note** to move it, **drag the board** to pan (arrow keys too),
`Tab` or `1`-`9` to switch worlds (or click a tab), `w` or the `+` in the tab
strip for a new world (it stays out of your way: the tab appears, you do not
move to it, and nine is the limit because `1`-`9` are how you get there), `W` to
delete the world you are on, `n`
for a new note, `shift`+arrows (or `shift`+`h`/`j`/`k`/`l`) to step the
selection between pins, `e` or `enter` to edit the selected note (which zooms in
and centers it; first line is the title, the rest the body; `enter` adds a line,
`alt`/`ctrl`+`←`/`→` jumps by word, `ctrl`/`alt`+`backspace` deletes a word,
`ctrl`+`u` clears the line, `esc` saves; the note scrolls with the wheel or by
moving the caret past an edge),
`y` to copy the whole note, `c` cycles its color (`C` backwards), `d` to delete,
`u` to undo and `ctrl`+`r` to redo, `t` to cycle the theme (`T` backwards, and
the active one is named in the header), `q` to quit.

A pin holding more text than it can show scrolls where it lies, open or not:
put the pointer on it and use the wheel, or `page up`/`page down` on the
selected pin. It draws a thumb on its right border while there is more.

Which gesture the wheel is depends on what is under the pointer, never on how
much someone wrote:

| Pointer is on | Zoom level | The wheel |
| ------------- | ---------- | --------- |
| a pin | preview or document | scrolls that pin; a short one has nowhere to go |
| bare board | preview or document | zooms |
| anything | cluster or titles | zooms |

A short pin absorbing the wheel is the point rather than an oversight: the
board never lurches out from under a pin you were reading, and you do not have
to know how much text a pin holds to know what the wheel will do. The
exception is about the zoom level, not the contents. Cluster and titles draw
no body to read and are the levels where pins cover the screen, so a wheel
that died on every one of them would leave a full board impossible to zoom out
of with the mouse.

### Deleting a world

`W` deletes the world you are on, pins and all. An empty world goes straight
away; one with pins on it asks you to type its name first. The last world stays:
there is always a board to be on.

`u` brings it back, pins and all, as long as you are still in the session. After
that the pin repo is git, so a deleted world is in the history: `pinz sync`
carries the deletion as a commit like any other change.

### Moving a pin to another world

**Drag a pin onto another world's tab** and let go. The tab lights up as you
cross it, a 📌 rides the cursor, and the footer names both the pin and where it
is headed, so a mis-aimed drop is caught before you release.

The pin lands in the middle of the target board, on top of whatever is already
there, and cascades down-right if the middle is taken. Worlds do not share a
coordinate space, so carrying the old position across would drop the pin
somewhere off in the dark, far from the cloud that board frames. You stay on the
board you were clearing, which is what you want when taking several pins off it
in a row. Dropping on the board's own tab, on the `+`, or anywhere else does
nothing.

### Pins never hide each other

No two pins on a board can end up in the same spot. A new pin, a dropped pin and
a pin arriving from another world all cascade down-right until they are clear,
so a pin can overlap its neighbours but can never vanish behind one.

### Undo

`u` undoes, `ctrl`+`r` redoes, and pinz remembers the last 50 changes. Undo
covers anything that changes a board: new pins, deletes, recolors, edits, moves,
new and deleted worlds, and pins sent to another world. Undoing a change made on another
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

`ctrl`+`c` still quits when nothing is selected, so the escape hatch survives, and it
saves the note you were typing on the way out, exactly as `esc` would.
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

`~/pinz-board` is an ordinary git repo. pinz pulls when it opens, and commits
and pushes when you quit; `pinz sync` does both on demand. It only ever touches
its own repo, so it can never sweep up or be blocked by work in progress
anywhere else.

Quitting tears down the alternate screen and snaps your old scrollback back, so
whatever pinz did lands on top of whatever was already there. It arrives as one
block, under a rule drawn across the terminal:

```
─── pinz ──────────────────────────────────────────────
  ~/pinz-board - 1 uncommitted change(s)
  ✓ commit  committed local pins
  · pull    already up to date
  ✓ push    pushed to Gaurgle/pinz-board
```

The rule carries the name, so the lines under it do not have to. Everything in
the block is indented, which is what marks it as pinz's even in a screenshot or
a pipe with no color. Informational lines are blue; step lines are colored by
what happened - green `✓` done, grey `·` nothing to do, red `✗` stopped - and
the message is git's own wording, folded up from its stderr and left plain.

A push that moved something names where it went, so two machines pointed at
different remotes cannot look alike. The name is `origin` shortened the way
`gh` writes one; a remote that is a directory keeps the last two segments of
its path.

Set `NO_COLOR` to turn the color off. Color is dropped and the rule falls back
to a fixed width when the output is not a terminal.

When the same pin changed on both machines, pinz settles what it safely can:
position and color changes merge on their own (moving a pin on one machine and
editing its text on the other is not a real conflict), and a tie on position
goes to the machine you are sitting at. Only when both machines changed a pin's
*text* differently does pinz **stop**, leave the repo exactly as it was, and
tell you - in the footer if the board is open, on the terminal otherwise.
Resolving that is a human's call, not a corkboard's.

Only one pinz may write a board at a time. The first instance takes a lock and
works normally; open a second one on the same machine and it comes up fully
readable but read-only, saying so in the footer, so two windows can never
silently overwrite each other's edits. A lock left behind by a crash is taken
over automatically.

Pins are deliberately separate from your notez2 notes. A pin that turns out to
matter is meant to graduate into a real note later; nothing here reads or writes
a notez2 workspace.

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Unless you state otherwise, any contribution you intentionally
submit for inclusion in this work shall be dual-licensed as above, with no
additional terms.
