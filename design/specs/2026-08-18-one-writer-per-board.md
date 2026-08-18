# One writer per board: later instances open read-only

Date: 2026-08-18. Status: **built** the same day. Andreas's idea.

## The hazard

Two pinz instances open on the same machine share one directory, so git never
sees the problem and cannot help. Reading `FileStore`:

- **Both edit the same pin: silent loss.** The second instance holds that pin
  as it was when it started; when it saves, it rewrites the file and the other
  instance's edit is gone. No conflict, no warning.
- **One deletes a pin the other still holds: the pin comes back.** The stale
  instance rewrites the file it still remembers.
- **One creates a pin: safe.** The other never loaded that file, and `save`
  only deletes files it loaded, so a new pin survives. The existing "never
  delete what we did not load" rule already covers this case.

There is no lock and no instance guard anywhere in the codebase today.

## The design

**The first instance to open a board owns it and may write. Later instances
open read-only and say so.**

- On startup, after resolving the pin root, try to take a lock. Taking it
  means: create `.pinz-lock` in the pin root, exclusively (`create_new`, which
  fails if the file exists), holding the pid and a start timestamp.
- **Lock taken:** normal session. Sync runs as it does now. The lock file is
  removed on quit, including on the panic path, so a crash does not strand it.
- **Lock held by someone else:** the board still opens, fully navigable and
  readable, but in **read-only mode**: edits, new pins, deletes, moves and
  color changes are refused, no save is ever written, and no sync runs (no
  commit, no pull, no push). A sticky footer warning states plainly that
  another pinz owns this board and that changes will not be saved. Reuse the
  warning surface added on 2026-08-18 for stopped syncs.
- **Stale locks:** a lock whose pid is no longer alive is stale and gets
  taken over, so a hard kill never locks you out permanently. `.pinz-lock` is
  gitignored: it is machine-local state and must never sync.

Read-only rather than refusing to start, because a second window is usually
someone wanting to *look* at the board, and losing that is a worse trade than
allowing it safely.

## What this deliberately does not do

- **No cross-machine locking.** The lock is one directory on one machine. Two
  machines are git's problem and are handled by the pin-aware merge.
- **No live reload.** A read-only instance shows the board as it was when it
  opened; it does not follow the writer's edits. That is the separate
  sync-while-running feature on the backlog, and the two should be designed
  together when that one comes up.
- **No lock promotion.** A read-only instance stays read-only for its whole
  session, even if the writer quits. Quitting and reopening is the way back to
  writing; anything smarter needs live reload first.

## Testing

- Taking a lock in a fresh root succeeds; a second attempt in the same root
  fails and reports who holds it.
- A lock whose pid is dead is taken over.
- The lock file is removed on a normal quit, and on the panic path.
- A read-only app refuses an edit, a delete and a new pin, and its store is
  never written.
- A read-only session runs no git operations at all.
