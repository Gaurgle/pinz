# Text selection and copy

Status: approved, not yet implemented. Date: 2026-08-05.

Mark text in a note and get it onto the system clipboard, by mouse drag or by
shift + arrows. Paste back in. The wider "why" behind pinz lives in
`design/DESIGN.md`; this file is the spec for one feature.

## The problem

pinz turns on `EnableMouseCapture` so the board can be dragged and panned. That
kills the terminal's own click-drag selection, so there is currently no way to
get a note's text out of pinz other than reading it off the screen and retyping
it.

## Constraints that shaped the design

Two of them are hard, and both rule out the obvious answer:

- **`shift+hjkl` cannot work in Edit mode.** Edit mode is insert-only, so `H`,
  `J`, `K` and `L` are literal characters. Binding them to selection would make
  capital letters untypeable. Shift + arrows are free.
- **Cmd+C never reaches the process.** On macOS the terminal emulator claims
  Cmd+C for its own copy and does not forward it. Crossterm can only observe a
  `SUPER` modifier where the terminal speaks the Kitty keyboard protocol, and
  the terminals that do still bind Cmd+C themselves by default. It is accepted
  as an alias where it happens to arrive, but it cannot be the only binding.

Likewise `yy`: `y` is free in Nav mode but is literal text in Edit mode, and
pinz has no multi-key sequence state. A bare `y` in Nav costs nothing and there
is no `yw`/`y$` on a board to disambiguate it from.

## Selection lives in `TextEditor`

`editor.rs` gains an `anchor: Option<Cursor>`. A selection exists when `anchor`
is `Some` and differs from the cursor.

```rust
pub enum Motion { Left, Right, Up, Down, Home, End, LeftWord, RightWord }

/// Move the cursor. `extend` keeps (or drops) the anchor, so one call site
/// cannot move the caret and forget the selection.
pub fn step(&mut self, m: Motion, extend: bool)
/// The selection as (start, end) in document order, if any.
pub fn selection(&self) -> Option<(Cursor, Cursor)>
pub fn selected_text(&self) -> Option<String>
pub fn select_all(&mut self)
/// Remove the selected range, leaving the cursor at its start. True if
/// anything was removed.
pub fn delete_selection(&mut self) -> bool
/// Place the cursor at an arbitrary position, clamped into the buffer.
pub fn set_cursor(&mut self, c: Cursor, extend: bool)
/// Insert text that may contain newlines, for paste.
pub fn insert_str(&mut self, s: &str)
```

`step` subsumes the eight movement arms in `App::edit_key`. The existing
`left`/`right`/`up`/`down`/`home`/`end`/`left_word`/`right_word` become its
private guts and stop being public.

Every editing operation - `insert_char`, `insert_newline`, `backspace`,
`delete`, `delete_word`, `kill_line`, `insert_str` - calls `delete_selection`
first. So typing over a selection replaces it, and backspace with a selection
removes exactly the selection and no extra character.

Everything stays char-indexed, as the editor already is, so non-ASCII text
selects and copies correctly.

## The wrap layout moves to `wrap.rs` and becomes bidirectional

`wrap_rows` and `EditWrap` move out of `ui.rs` into a new
`crates/pinz-tui/src/wrap.rs`, and stop being caret-specific. Each visual row
carries `(text, logical_line, start_col)`, which is enough to map either way:

```rust
/// Logical (row, col) to visual (row, col). Used for the caret and for the
/// selection bounds.
pub fn place(&self, c: Cursor) -> (usize, usize)
/// Visual (row, col) back to a logical cursor. Used for the mouse.
pub fn locate(&self, vrow: usize, vcol: usize) -> Cursor
```

This is the pivot of the change. It is the principle already stated at
`app.rs:43` - the app owns the tab strip's layout so that clicking a tab and
drawing it cannot drift apart - applied to the editor: rendering and hit-testing
run the identical wrap. It also removes the single-cursor tracking that is
currently threaded through `wrap_rows` as a special case.

`Cursor` moves to `wrap.rs`'s dependency set unchanged; it stays defined in
`editor.rs` and `wrap.rs` imports it.

## Rendering

`editor_lines` splits each visual row into up to three spans at the selection
bounds and styles the selected run `bg(theme.accent).fg(theme.mantle)`. No new
`Theme` field: the accent is already "the one highlight color".

When a selection is live the separate reversed-cell caret is not drawn. The
selection's moving edge is the caret, as in every other editor, and drawing both
would mean a reversed cell inside a reversed run - which cancels out and reads
as a hole.

## Mouse

`app.rs` computes the edited note's cell rect itself, from
`self.view().note_cells(note.position())` inset by one cell for the border, and
runs the same `wrap`. Nothing is fed back from the renderer, so this stays
testable without a terminal - which is `app.rs`'s stated contract.

- `mouse_down` in Edit mode **inside** the edited note: place the caret there,
  drop the anchor, set `drag = Some(Drag::Text)`. **Outside** it: commit the
  edit, exactly as today.
- `mouse_drag` while `Drag::Text`: move the caret with `extend: true`, clamped
  to the note's inner rect, so dragging past an edge runs to the line's start or
  end rather than doing nothing.
- `mouse_up`: a zero-width result clears the anchor, so a plain click just
  repositions the caret rather than leaving an invisible empty selection.

Clicking into text to move the caret is a side effect worth naming: it is not
possible at all today.

## Clipboard: OSC 52, no new dependencies

A new `clipboard.rs`. The escape sequence `\x1b]52;c;<base64>\x07` hands the
text to the terminal, which puts it on the system clipboard. It works over SSH,
and it needs no crate: about twenty lines of base64 (RFC 4648, with the spec's
own test vectors as unit tests).

Two details that are cheap now and painful later:

- **tmux.** When `$TMUX` is set the sequence is wrapped in tmux's passthrough,
  `\x1bPtmux;\x1b<seq>\x1b\\`. Requires `set-clipboard on` in the user's tmux
  config; that is documented in the README rather than worked around.
- **Size.** Terminals drop oversized OSC 52 payloads. Anything over 100 KiB is
  refused with a reported error rather than silently truncated.

Not supported by macOS Terminal.app. iTerm2, Ghostty, kitty, WezTerm and
Alacritty all support it. This is called out in the README.

**`App` performs no I/O.** It sets `pending_copy: Option<String>`; `run()` in
`main.rs` drains it after each event and writes the escape. That keeps `App` a
pure state machine, matching how the core keeps storage behind a seam, and makes
"did this copy?" assertable in a test with no terminal attached.

## Keys

Edit mode:

| Key | Effect |
| --- | --- |
| `Shift` + `←` `→` `↑` `↓` `Home` `End` | extend the selection |
| `Ctrl`/`Alt` + `Shift` + `←` `→` | extend by word |
| `Ctrl-A` | select all |
| `Ctrl-C`, `SUPER+C` | copy, **when a selection exists** |
| `Ctrl-X` | cut |

With no selection, `Ctrl-X` and `SUPER+C` do nothing.

Nav mode: `y` copies the selected note as `title\nbody`, or the title alone when
the body is empty. With nothing selected it does nothing.

The global "Ctrl-C quits in any mode" check at `app.rs:315` moves to run *after*
the Edit-with-a-selection case. In every other mode, and in Edit with no
selection, Ctrl-C still quits. The only cost is that quitting mid-selection
takes an `Esc` first.

## Paste

`EnableBracketedPaste` in `setup()`, `DisableBracketedPaste` in `restore()`.
`Event::Paste(String)` reaches `App::on_paste`, which inserts the text in Edit
mode, appends the first line in Prompt mode (a world name is one line), and is
ignored in Nav. OSC 52 is write-only, so bracketed paste is the only way text
gets back in.

## Feedback

A copy is otherwise completely invisible. `App` gains `status: Option<String>`,
shown in the footer as e.g. `copied 42 chars` and cleared by the next event.
Footer hints gain `shift+←→ select` and `ctrl+c copy` in Edit, and `y copy` in
Nav.

## Tests

- `editor.rs`: selection normalization when the anchor is after the cursor and
  when it spans lines; `selected_text`; `delete_selection`; typing, backspace
  and paste over a selection; `step` with `extend` true vs false; non-ASCII.
- `wrap.rs`: `place`/`locate` round-trip; a line long enough to wrap; empty
  lines; a click past the end of a row clamping to that row's end.
- `clipboard.rs`: the RFC 4648 base64 vectors; tmux wrapping on and off; the
  size cap refusing rather than truncating.
- `app.rs`: shift+arrow builds a selection; `Ctrl-C` with a selection sets
  `pending_copy` and does *not* quit; `Ctrl-C` without one quits; `y` in Nav
  copies title and body; mouse down then drag inside the edited note selects;
  mouse down outside it commits the edit.
- `ui.rs`: a render test asserting the selected cells carry the accent
  background.

## Out of scope

Double-click word select, triple-click line select, selection spanning several
notes, and horizontal scroll. Each is a separate change; none is needed to get
text out of pinz.
