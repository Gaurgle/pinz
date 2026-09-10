# External edit detection

Status: approved, implemented. Date: 2026-09-10.

Stop a running pinz session from silently overwriting a pin that something
else changed on disk underneath it. Warn instead. The wider "why" behind pinz
lives in `design/DESIGN.md`; this file is the spec for one feature.

## The problem

A pin file has two writers: pinz, and anything else with a text editor. Today
the second one always loses.

`FileStore::save` writes every note the board holds, through
`write_if_changed`, which compares what it is about to write against the file
that is there now:

```rust
fn write_if_changed(path: &Path, contents: &str) -> Result<()> {
    if let Ok(existing) = fs::read_to_string(path) {
        if existing == contents {
            return Ok(());
        }
    }
    fs::write(path, contents).map_err(|e| backend("writing a pin", e))
}
```

That comparison cannot tell "this file is stale and I hold the new version"
from "this file is newer than the copy I loaded". Both look like a difference,
and both end in a write, so pinz's in-memory copy always wins.

Observed 2026-09-10: a pin was edited on disk while a pinz session was open.
The edit was verified present. On the session's next save, pinz wrote its
loaded copy back over it and the change was gone, with a clean git tree and
`pinz sync` correctly reporting nothing to sync. Nothing malfunctioned. The
file simply had two writers.

The existing `BoardLock` does not help here. It is a one-writer-per-board lock
that stops a *second pinz*; it says nothing about editors, scripts, or an
agent working in the directory, none of which take it.

## What this does not try to be

Not a file watcher. pinz does not learn about an external edit the moment it
happens, and the board on screen can be out of date until the next save. That
is a bigger feature (a watch loop, live reload, reconciling an edit against an
open editor buffer) and it is not needed to stop data loss.

Not a merge. When a pin has diverged, pinz keeps its own copy in memory and
leaves the disk alone. Reconciling the two is the user's call.

## The rule

At save time, per pin:

1. If the file's current content equals what pinz would write, do nothing.
   Not a conflict: the two writers agree.
2. Otherwise, if the file's current content differs from what pinz **loaded**
   from that path, the file changed underneath the session. Skip it, and
   report it.
3. Otherwise, write.

Case 2 is the new one. Cases 1 and 3 are today's behavior.

Every successful write updates the remembered content for that path, so the
next save compares against what pinz last put there rather than against the
original load. Without that, a session's second save would flag every pin it
had itself edited.

A pin with no remembered content is new (created this session, or moved to a
different world) and is written normally. There is nothing on disk to lose.

## Why skip rather than reload or abort

**Skip** never destroys an external edit and never blocks unrelated work. The
cost is that memory and disk stay divergent for that one pin until the user
reloads, which the warning tells them to do.

**Reload** (external edit wins, re-read into the board) would silently discard
an in-pinz edit to the same pin. That trades one silent data loss for another.

**Abort the whole save** would let one externally-touched pin block every other
edit in the session, which is a worse failure than the one being fixed.

## Interface

`Store::save` gains a return value. It is currently `Result<()>`, and there is
no way for a partially-skipped save to say so.

```rust
/// What a save did, beyond succeeding.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SaveReport {
    /// Pins left alone because they changed on disk since they were loaded.
    pub skipped: Vec<PathBuf>,
}

pub trait Store {
    fn save(&mut self, boards: &[Board]) -> Result<SaveReport>;
    // load, delete_board unchanged
}
```

`MemoryStore` returns `SaveReport::default()`; it has no disk to diverge from.

This widens the storage seam, which `store.rs` says to do "only when a real
backend needs it". A real backend needs it: the condition is only detectable
below the seam, and only actionable above it.

## What the user sees

The TUI sets a sticky warning, the same mechanism the sync-conflict path uses,
so it survives keystrokes rather than being wiped by the next one-off status:

```
! 1 pin changed on disk and was not overwritten: sync/small-machine.md
```

More than three pins are counted rather than listed, to keep it in the footer.

## Testing

- An external edit between load and save is not overwritten, and comes back in
  `skipped`.
- A pin pinz itself edited still writes. No false positive.
- A second save in one session does not flag pins the first save wrote.
- A pin whose content matches what pinz would write is not flagged, even when
  it was touched externally to reach that content.
- A new pin, with nothing on disk, writes normally.
- An unchanged board still produces an empty report.
