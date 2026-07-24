# pinz - TODO / backlog

Scratch list of things to build or decide. The "why" behind shipped decisions
lives in `design/DESIGN.md`; this is the running list of what's next.

## Editing

- **Sideways (horizontal) scroll.** Scroll the content left/right rather than
  relying only on wrap. (Clarify scope: the board view, or long lines while
  editing a note? We added word-wrap in edit mode, so this is likely the board
  canvas or an opt-out of wrap.)

## Already on the radar (from earlier)

- Vertical scroll when a note's body grows taller than the post-it.
- Re-center the board on terminal resize.
- Pile / stacking cascade when notes are dropped together.
- `Color::from_str` shadows `std::str::FromStr` (clippy) - fold into the
  persistence pass.

## Parked (needs Andreas's design)

- **notez2 format + persistence.** The git-backed `Store` that writes each note
  as markdown + frontmatter. Important enough that the format is Andreas's call;
  don't start unprompted.
