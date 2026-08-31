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

`App` gains one map, `scroll`: how many wrapped rows sit above the top of each
note's text area, by id. A note with no entry is at the top.

One window per note rather than one for the app, so reading down a long pin
and then looking at another does not cost you your place in the first. It is
not stored with the note: where you are reading is this session's business,
not something to carry between machines.

It lives in `App` and not in the renderer because `App` already computes the
wrap - `edit_layout` is what a click on text goes through - and the repo's
standing rule is that rendering and hit-testing run the same layout. A scroll
offset the renderer kept to itself would put clicks on the wrong row the
moment the text moved.

A note that is not being edited now draws through `wrap.rs` too, rather than
ratatui's own `Wrap`. That is what makes the offset mean the same thing in
both states: `note_lines` is the one place a note becomes lines, `App` wraps
them to know how far it can scroll, and `ui` wraps them to draw it.

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

**The wheel picks its target by what is under it.** Over a pin holding more
text than it can show, it moves that pin's window. Anywhere else - bare
board, or a pin with nothing hidden - it zooms, exactly as before.

That last clause is what keeps the gesture honest. A pin that fits is not a
scroll target, so the wheel does not go dead over the majority of the board,
and it only changes meaning where there is something to reveal.

While a note is open in the editor the wheel belongs to it wherever the
pointer is: you are inside its text, not on the board. Keys are already text
rather than commands there - `-` types a hyphen, it does not zoom out - so a
wheel that still zoomed would have been the odd one out.

**`page up` and `page down` are the keyboard's answer to the wheel**, on the
selected pin, one screen at a time keeping a row of overlap so there is
always a line you have already read to land on. Keys have no pointer, so the
selection is what hovering is for the mouse.

## Saying there is more

A thumb is drawn on the note's right border, sized to the fraction of the
text on screen and positioned by the offset, in the same glyph and colour as
the board's own scrollbars. It is absent whenever the text fits, so a normal
note is unchanged, and it is drawn whether or not the note is being edited:
a pin hiding something should say so before you open it.

## What this does not do

- **Up and down still move by logical line, not by visual row.** In a
  paragraph wrapped over eight rows, one `down` crosses all eight. That is
  the editor's existing behaviour; scrolling follows the caret wherever it
  lands, so nothing here depends on it. Worth revisiting separately.
- No change to how notes are stored, sized, or laid out.
- Nothing is remembered between sessions. A window sits where you left it
  until pinz closes, and every note opens at its top the next time.
