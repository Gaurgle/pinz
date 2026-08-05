# Undo, and dragging pins into other worlds

Status: approved, not yet implemented. Date: 2026-08-05.

Two features that share one observation: `App` owns the whole workspace and
`Store::save` takes all of it, so board state is cheap to snapshot and a pin
moving between boards is just a move between two `Vec<Note>`.

## Undo

### What counts as a step

Anything that changes board state: a new pin, a delete, a recolor, a finished
edit, a finished drag, a new world, a pin moved to another world.

Not: theme, zoom, pan, selection, switching worlds. None of them touch board
state, and undoing them would be surprising rather than useful.

### Snapshots, not inverse operations

```rust
struct Snapshot {
    boards: Vec<Board>,
    active: usize,
    selected: Option<u64>,
}
```

A `VecDeque` of these, capped at 50, plus a redo stack. Undo pops the undo
stack, pushes the current state onto redo, and restores what it popped. Redo
mirrors it. Any new action clears redo.

Inverse operations would use less memory and cost far more code, with a new way
to be wrong per action type. A board is a few hundred small notes at most, so a
snapshot is tens of kilobytes - less than the frame pinz already redraws on
every keystroke.

`active` and `selected` ride along so undoing a cross-world move puts you back
where it happened rather than leaving you staring at an unchanged board.

### When a snapshot is taken

Not by a `checkpoint()` call at each mutation site - that is one forgotten call
away from a silent hole. Instead the event entry points become wrappers:

```rust
pub fn on_key(&mut self, key: KeyEvent) {
    let rev = self.begin_step();
    self.key(key);          // the old body, now private
    self.end_step(rev);
}
```

- `begin_step` stashes the current state in `pending` if nothing is stashed yet,
  and returns the current `revision`.
- `end_step` commits `pending` to the undo stack **only if** `revision` moved
  *and* `is_dragging()` is false. Mid-drag it keeps the stash and gathers.
- A step that changed nothing drops the stash.

The granularity we want falls straight out of machinery that already exists for
another reason:

| Gesture | Steps | Why |
| --- | --- | --- |
| A drag across the board | 1 | `pending` is the pre-drag stash; commit waits for `is_dragging()` to go false |
| Typing a note and pressing esc | 1 | `edit_key` never bumps `revision`; only `commit_edit` does |
| An arrow key | 0 | `revision` did not move |

`is_dragging()` is the same predicate the runner already uses to avoid writing a
file per mouse-move. Undo granularity and save granularity are the same
question, so they get the same answer.

Undo and redo clear `pending` before returning, so they cannot record
themselves.

### Keys

`u` undoes, `ctrl+r` redoes. Nav mode only: `u` is literal text in the editor,
and there is no text-level undo (see Out of scope). Undo with an empty stack
does nothing and says so in the footer.

## Dropping a pin on another world's tab

### The gesture

`Drag::Note` already exists. Three things are added to it.

**While the cursor is over the tab strip**, `mouse_drag` stops moving the note
and sets `drop_target: Option<usize>` instead. Without this the note would chase
the cursor up into the header.

**On release over a world tab**, the pin moves to that board keeping its world
coordinates, with `z` set to the top of the target. Selection clears - the pin
is not on this board any more - the footer reports `moved to reading`, and the
view stays where it is, because the common case is clearing several pins off one
board in a row.

Dropping on its own tab, on the `+`, or anywhere outside the strip is a no-op.

`click_tab`'s hit-test extracts into `tab_at(col, row) -> Option<usize>`, so
clicking a tab and dropping on one resolve it identically.

### Feedback

The note stops following the cursor over the strip, which without feedback reads
as the drag having broken. Three signals restore it:

1. **The armed tab** takes the accent background and a bold label.
2. **A `📌` rides the cursor**, painted at the cursor's cell in the strip. This
   is what keeps the gesture continuous once the note stops tracking. It
   overwrites one cell of the armed tab's label, which reads correctly because
   that tab is already highlighted.
3. **The footer states the outcome**: `release to move "buy milk" to reading`,
   naming both pin and destination so a mis-aimed drop is caught before release.

Dragging back off the strip disarms all three and the note resumes following the
cursor. Nothing commits until release.

A floating indicator box near the cursor was considered and rejected: the strip
is one row tall, so a box would have to overlap the header or the board, and it
would only repeat what the armed tab says.

### Persistence

Nothing to do. `FileStore::save` already relocates a pin's file when its board
changes and removes the old one (`file_store.rs:188-196`).

## Tests

**Undo:** each action type undone and redone; a multi-event drag collapses to one
step; a non-mutating key records nothing; undo of a cross-world move restores
both boards and the active tab; redo cleared by a new action; the depth cap
evicts the oldest; undo and redo on empty stacks are no-ops; undo does not
record itself.

**Drop:** dragging a pin onto another world's tab moves it and leaves the view in
place; the pin keeps its coordinates and lands on top of the target; dropping on
its own tab and on the `+` are no-ops; the note does not move while the cursor
is over the strip; `drop_target` is set while over the strip and cleared on
release; a render test asserts the armed tab carries the accent background and
the pin glyph lands on the cursor's column.

## Out of scope

Per-keystroke text undo inside the editor (a second stack at a different
granularity, on a mode-dependent key). A keyboard command to move a pin between
worlds, which stays on the TODO list. Undo of theme, zoom or pan.
