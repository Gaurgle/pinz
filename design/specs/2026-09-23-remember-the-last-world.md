# Remember the last world: start where you left off

Date: 2026-09-23. Status: **built** the same day. Andreas's idea.

## The problem

pinz always opens on the first world by name. If you spend the day in one
world, every launch lands somewhere else and costs a Tab or two first.

## The design

**pinz opens on the world that was open when it last closed, on whichever
machine that was.**

- The open world is recorded in `.pinz-world` in the pin root:

  ```text
  world: ideas
  at: 1790000000
  ```

  `at` is when that world became the recorded one, in Unix seconds.
- It goes through `Store` as two methods with defaults (`load_last_world`,
  `save_last_world`), so a store that keeps no such thing needs no change and
  simply opens on the first world. `FileStore` implements them.
- It is written with every save (`persist`), so the exit save and the commit
  that follows carry it, and `pinz sync` pushes it like any pin. Writing the
  same world again leaves the file untouched, so staying in one world adds no
  commits.
- A read-only session never writes it, like everything else.
- A recorded world that no longer exists (deleted, or renamed on disk) falls
  back to the first world. A missing or unreadable file does the same.

Hidden, so the loader does not read it as a board. Not gitignored, unlike
`.pinz-lock`: the whole point is that it syncs.

## Conflicts

Unlike a pin, this one file changes whenever you move between worlds, so two
machines that both switched worlds between syncs will conflict on it. A sync
that stopped over that would block the pins from syncing too.

`Sync::pull` therefore settles a conflict on `.pinz-world` itself: both-modified
(`UU`) or both-added (`AA`, the first time two machines each create it), keep
the side with the later `at`, local on a tie or an unreadable side. Any other
conflict still stops the sync as before.

## Rejected

- **Machine-local state (gitignored, like `.pinz-lock`).** Simpler and
  conflict-free, but the request is that the world follows you across
  machines.
- **Local always wins, as for pin cosmetics.** For a pin, local wins because
  you arranged it most recently in your own view. For the open world,
  "most recent" is a fact the file can record, so it does.

## Testing

- `FileStore` round-trips the last world; a missing or garbled file loads as
  none; saving the same world again does not rewrite the file.
- The app opens on the recorded world, and on the first world when the name is
  unknown.
- `persist` records the open world; a read-only session does not.
- A pull where both machines changed `.pinz-world` settles to the newer side
  and completes, and the pins from both sides survive.
