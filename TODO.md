# pinz - TODO / backlog

Scratch list of things to build or decide. The "why" behind shipped decisions
lives in `design/DESIGN.md`; this is the running list of what's next.

## Editing

- **Sideways (horizontal) scroll.** Scroll the content left/right rather than
  relying only on wrap. (Clarify scope: the board view, or long lines while
  editing a note? We added word-wrap in edit mode, so this is likely the board
  canvas or an opt-out of wrap.)
- Vertical scroll when a note's body grows taller than the post-it.
- Double-click to select a word, triple-click to select a line. Selection and
  copy are done (`design/specs/2026-08-05-text-selection-and-copy.md`); these
  are the two mouse shortcuts that got left out of it.
- Select across several notes on the board and copy them together. Needs a
  modifier to tell it apart from pan and move.

## Boards and pins

- Rename and delete boards from inside the app. Creating one is done (`w`, or
  the `+` in the tab strip); the other two still mean touching `~/pinz-board` by hand.
- Move a pin to another board *from the keyboard*. Dragging it onto the target
  world's tab is done; there is no equivalent for a keyboard-only session.
- Re-center the board on terminal resize.
- Pile / stacking cascade when notes are dropped together.

## Sync

- Surface sync state in the UI. It currently only prints around the TUI, so a
  conflict reported at startup scrolls past before you reach the board.
- Sync while running, for a board left open for days. Pins are written to disk
  immediately, but only pushed on quit.
- Consider a merge driver for pin files, so two machines editing the *same* pin
  merge on position instead of stopping. Only worth it if stopping turns out to
  be annoying in practice.

## Graduating a pin into a note

- One-way promote: turn a pin into a real notez2 note in a chosen scope, leaving
  notez2 the owner of its own format. Shell out to `notez` from the TUI; call
  `notez-core` in-process from the epoz tab.

## notez2, separately

- `notez sync`. notez2 has no sync command at all today, so `~/notez` is synced
  by hand and drifts. That is notez2's problem rather than pinz's, but it is the
  reason pins got their own repo.
