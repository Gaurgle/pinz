# Scrolling a note that outgrew itself

Date: 2026-08-31. Status: approved in conversation, this file records it.

## The problem

A note is a fixed size in world units (`NOTE_W` x `NOTE_H`), which at document
zoom is about 40x15 cells, so 13 rows of text inside the border. That is a
deliberate invariant: uniform notes are what make layout, hit-testing and
stacking simple.

Nothing stopped anyone writing more than 13 rows. `Paragraph` renders what
fits and drops the rest on the floor, so the surplus was not merely
off-screen, it was unreachable: keep typing past the last visible row and the
caret goes with it. You are writing blind into a note you cannot read back.

Fixed-size notes are not the bug. The missing piece is a window that moves.

## The window

`App` gains one number, `edit_scroll`: how many wrapped rows of the note being
edited sit above the top of its text area. Zero unless a note is open.

It lives in `App` and not in the renderer because `App` already computes the
wrap - `edit_layout` is what a click on text goes through - and the repo's
standing rule is that rendering and hit-testing run the same layout. A scroll
offset the renderer kept to itself would put clicks on the wrong row the
moment the text moved.

## Moving it

**Keys move the caret; the window follows.** After any key that reaches the
editor, the window moves only if the caret has left it, and then only far
enough to catch it - up to the caret's row when it went off the top, down by
the overshoot when it went off the bottom. Recentring on every keystroke
would make the text jump under a caret that stepped one row.

`on_key` and `on_paste` are the two choke points, so no editing path can
forget: a paste that adds ten lines, a `ctrl+u` that removes one, and an
arrow key all land in the same place.

**The wheel moves the window and leaves the caret alone**, three rows a
notch, clamped so the last row cannot scroll past the bottom edge. It does
not chase the caret back, which is what makes it useful: you can look up at
what you wrote without losing where you are typing. The next keystroke brings
the caret back into view, which is what every editor does.

**In edit mode the wheel scrolls instead of zooming.** Keys are already text
rather than commands while a note is open - `-` types a hyphen, it does not
zoom out - so a wheel that still zoomed was the odd one out. Outside edit
mode the wheel zooms exactly as before.

## Saying there is more

While editing, a thumb is drawn on the note's right border, sized to the
fraction of the text on screen and positioned by the offset, in the same
glyph and colour as the board's own scrollbars. It is absent whenever the
text fits, so a normal note is unchanged.

## What this does not do

- **The read-only note is untouched.** A note you have not opened still
  clips, with no scrollbar and nothing to scroll it. The read-only body is
  laid out by ratatui's own `Wrap`, not by `wrap.rs`, so pinz cannot say
  where its rows fall without moving that rendering onto the same wrap
  first. That is a change worth making on its own, not as a rider here.
  Press `e` to read a long note through.
- **Up and down still move by logical line, not by visual row.** In a
  paragraph wrapped over eight rows, one `down` crosses all eight. That is
  the editor's existing behaviour; scrolling follows the caret wherever it
  lands, so nothing here depends on it. Worth revisiting separately.
- No change to how notes are stored, sized, or laid out.
