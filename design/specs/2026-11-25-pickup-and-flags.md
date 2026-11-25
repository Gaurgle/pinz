# Pickup and flags: frontmatter syncz can read

Date: 2026-11-25. Status: **approved** the same day; parts 1 and 2 built,
part 3 not started. Andreas's idea, from the
pin `ideas/2026-11-24-180000-pinz-pickup-and-flags.md`.

## The problem

A handoff pin ("pick this up tomorrow") only helps if you happen to open the
right world. syncz already watches `~/pinz-board` and shows a menu bar item on
every machine, so it is the natural place to surface "there is something to
pick up", from any world, before you even run `pinz`.

For syncz to see that, the pin file has to say it. Today it can't, and a hand
edit doesn't survive: `render_pin` writes only `x`, `y`, `z` and `color`, so
any other key is dropped the next time the pin is moved or edited.

## The design

Three parts, built in this order. Each is its own PR.

### 1. Keep unknown frontmatter keys

- `Note` gains `extra: Vec<(String, String)>`: every key pinz does not own, in
  file order.
- `parse_pin` keeps them. `render_pin` writes the owned keys first (`x`, `y`,
  `z`, `color`, then `pickup` and `flags` once they exist), then `extra` as it
  was read.
- A duplicate key keeps its last value, matching `split_frontmatter` today and
  notez's `core/frontmatter.rs`, which already reads this format in order.
- New pins start with an empty `extra`.
- A sync conflict merges `extra` as one cosmetic field: the side that
  changed it wins, a tie goes to local. So a pickup set on one machine
  survives a drag on the other.

Tests: a pin with an unknown key round-trips through a move and an edit; key
order is stable across a second save (no diff churn in the board repo).

### 2. `pickup: true`

- Own field, not a sixth flag: it is a notification ("act on this next"),
  not a priority, and syncz treats it differently (it alerts on it).
- Written only when true; absent means false, so existing pins don't change.
- `P` toggles it on the selected pin. The pin shows a mark at every zoom rung
  that draws text, and a corner tick at cluster zoom.
- Clearing it is only in pinz. syncz is read-only.

### 3. Flags, shared with notez

- `flags: prio important`, space-separated, same keys as
  `notez-core/src/tags.rs`: `important`, `prio`, `longterm`, `idea`,
  `blocked`. Unknown words are kept, in order, and ignored.
- Written only when non-empty.
- Drawn as coloured dots in the title row, in notez's colours.
- A key opens a small flag toggle (proposal: `!`, then `1` to `5`).
- `#tag` filter, like notez, matching across all worlds: later, its own spec
  once flags exist.

## What syncz does with it (after pinz, a syncz ticket)

Not built here. Recorded so the format fits it.

- Read pins from syncz's fetched ref (`git show <ref>:<world>/<pin>.md`), not
  the checkout, so a pickup written on the other machine shows before
  `pinz sync` runs.
- Menu bar: a count per tool, `3 pinz to pick up`, `1 notez to pick up`,
  and under it one row per note, title and world (or folder), flags as
  colour. A note with no `pickup` says nothing.
- A row is clickable and opens that note in its tool. pinz needs a way in
  for that (`pinz open <world>/<pin-file>`, or the `pinz://` scheme from the
  Pinz/Notez links idea); decided in the syncz ticket.
- Read-only. Opening a note does not clear it: the mark stays until it is
  cleared in pinz or notez, so a glance never loses a handoff.
- Independent of read-on-sync.

## Open

- `P` and `!` are proposals; check them against the `?` overlay.

## One format for pinz and notez

Decided 2026-11-25: pinz and notez carry note metadata the same way, so
syncz (and any later -z tool) reads one format. That format is frontmatter:

- `pickup: true` and `flags: <keys>`, same keys, same spelling, same rules
  (written only when set, unknown keys preserved, last duplicate wins).
- notez moves its flags out of the per-root `.tags` sidecar
  (`relpath:flagbyte`) into each note's frontmatter, and gains `pickup`.
  A notez ticket, after this spec; it needs a one-time migration of existing
  `.tags` files.
- Why frontmatter over `.tags` for both: the metadata travels with the file
  (rename, move, copy between tools), a change touches one file instead of a
  shared one that conflicts in sync, and pins already store everything there.

## Rejected

- **`pickup` as a sixth flag.** Mixes "alert me" with "how important", and
  notez's flag byte would need a bit it doesn't own.
- **A separate pickup index file.** Another file to conflict on in sync, and
  it drifts from the pins it describes.
- **`.tags` sidecars for pins too.** Would make the two tools match, but
  keeps the sidecar's problems (sync conflicts, metadata lost on rename) in
  both.
