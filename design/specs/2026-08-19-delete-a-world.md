# Delete a world from inside the app

Date: 2026-08-19. Status: **built** the same day. Andreas's idea, from the
backlog entry of the same date.

## The gap

Worlds can be created (`w`, or the `+` in the tab strip) but never removed. A
world you made by mistake, or one whose pins have all moved elsewhere, stays on
the tab strip until you go and delete the directory under `~/pinz-board` by
hand. That is the only board operation the app cannot do to itself.

The part that needs a decision is storage, not keys. `FileStore::save` writes
the boards it is handed and deletes stale *pin* files; it never removes a board
directory, and an emptied board carries a `.gitkeep` so git does not lose the
tab. Dropping a world from the in-memory list is therefore not enough: the
directory survives, and the tab is back on the next load.

## The design

**`W` deletes the world you are on. An empty world goes without asking; a world
with pins on it wants its name typed first.**

### Guard rails, in order

Every check happens before anything is asked of you, for the reason
`begin_new_world` already gives: being asked for a name and only then told the
thing cannot happen is the wrong order to find out.

| Condition | What happens |
|---|---|
| Board is read-only | The existing `refuse_if_read_only` message |
| It is the only world | Status: `ideas is the last world` |
| The world has no pins | Deleted outright. Status: `deleted sketches` |
| The world has pins | The prompt opens: type the name to confirm |

The last-world rule keeps `boards.len() >= 1` true, which `App::active_board`
and the tab strip both assume today. Allowing zero worlds would be a new empty
state for every renderer to handle, in exchange for an action nobody wants.

### The prompt

The existing prompt does the work, so the confirmation already has its box, its
caret, and its refusal line. It gains one thing: a kind.

```rust
enum PromptKind {
    NewWorld,
    DeleteWorld { index: usize },
}
```

`confirm_prompt` dispatches on it. For `DeleteWorld` the typed text must equal
the world's name exactly; anything else is refused in place with the typed text
kept, exactly as a bad new-world name is. Esc cancels and nothing is touched.

Typing the name, rather than a y/n, because the pins on a world are worth more
than one keystroke of protection, and because a name you have to look at the
tab strip to type is a name you have looked at.

### After the delete

`active` clamps to `min(active, len - 1)`: the next tab slides into the slot you
were on, and deleting the last tab falls back one. Selection clears.

### Undo is free, and it is real

`begin_step` stashes a snapshot before every event unconditionally, and
`restore` already clamps the active index in case the board list shrank, so `u`
brings the world back with no new code in the undo path.

It restores the pins too, not just the tab. `FileStore` keeps its
`paths` map (note id -> the file it came from) across the delete, so the undo
puts the notes back in memory and the next save writes each one to its original
path, recreating the directory. Delete then undo is byte-identical, not a
rebuild. Nothing has to be taught to hold a deleted world in reserve.

### The storage seam

`Store` gains one method, its first widening since the trait was written:

```rust
fn delete_board(&mut self, name: &str) -> Result<()>;
```

`MemoryStore` retains by name. `FileStore` removes the board directory with
`remove_dir_all`. A name that is not there is a `StoreError::NotFound`, not a
silent success: the caller asked to remove something it believed existed.

**That also removes files in the directory we never loaded, which cuts against
the rule `save` follows for pins ("anything we never loaded is not ours to
remove"). It is the right call here anyway, and deliberately:** a world *is* its
directory, so a delete that left it standing would be a lie the next load
exposes; the command is explicit and confirmed by name, unlike the incidental
pruning `save` does; and the pin repo is git, with `pinz sync` running
`git add -A`, so a deleted world is recoverable from history on any machine that
has synced.

`App` performs no I/O, so it queues the names instead:

```rust
pending_deletes: Vec<String>,
pub fn take_pending_deletes(&mut self) -> Vec<String>
```

the same shape as `pending_copy`. The runner drains the queue and calls
`delete_board` for each name *before* `store.save(app.boards())`, so the save
that follows is the one that would recreate anything an undo brought back.

## What this deliberately does not do

- **No rename.** Renaming a world is its own backlog entry and its own storage
  problem (a directory move that has to keep the pins' history intact). Delete
  is not a workaround for it and this spec does not pretend otherwise.
- **No trash, no restore-later.** In-session undo covers the slip; git covers
  everything after that. A `.trash/` directory in the pin repo would be a third
  copy of the same guarantee, and one that syncs.
- **No deleting a world you are not on.** No middle-click on a tab, no delete
  from a picker. `W` acts on the active world, which is the one whose name you
  can read off the tab strip while you type it.
- **No cascade to pins.** Deleting a world deletes its pins with it. Moving
  pins out first is what dragging onto another world's tab is for.

## Testing

App level, all without a terminal:

- `W` on the only world refuses and names it; the board list is unchanged.
- `W` on a read-only board refuses with the read-only message.
- `W` on an empty world deletes it without opening a prompt, and says so.
- `W` on a world with pins opens the prompt and deletes nothing yet.
- A wrong name in that prompt is refused, keeps the typed text, and the world
  survives.
- The exact name deletes the world and queues it for the store.
- Deleting the active world clamps `active`, and deleting the last tab falls
  back one.
- Esc out of the delete prompt leaves the board list alone.
- `u` after a delete brings the world back with its pins.
- The queue hands each name out exactly once.

Store level:

- `delete_board` removes the directory, in a tempdir.
- `delete_board` on a name that is not there is a `NotFound`, not a panic.
- Delete, then save, then load: the world is gone from the loaded boards.
- Delete, then restore the board list, then save, then load: the pins are back
  at their original paths.

Renderer:

- The help panel's `worlds` group lists `W`, through `TestBackend`.
