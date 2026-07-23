# pinz - TODO / backlog

Scratch list of things to build or decide. The "why" behind shipped decisions
lives in `design/DESIGN.md`; this is the running list of what's next.

## Editing

- **Sideways (horizontal) scroll.** Scroll the content left/right rather than
  relying only on wrap. (Clarify scope: the board view, or long lines while
  editing a note? We added word-wrap in edit mode, so this is likely the board
  canvas or an opt-out of wrap.)
- **Modifier + backspace deletes more than one char.** `Ctrl`/`Alt` + `Backspace`
  should delete the previous *word*; a chord to delete the whole *line*. The
  editor currently only removes one character at a time.

## Zoom

- **Trim the zoom ladder.** The most zoomed-out level probably isn't needed -
  proposal: the furthest-out level should be **Preview** (title + body preview),
  dropping the dot/block levels below it. Decide how many levels remain (maybe
  just Preview and Document). Ladder today, out -> in: Survey, Cluster, Titles,
  Preview, Document.

## Notes

- **Change a note's color.** Let the selected note pick/cycle among a small
  palette (~5 colors). The core already names abstract colors (`Color`, 8 of
  them); this is a key or menu to set the selected note's color, resolved per
  theme. Decide whether to narrow the palette to ~5.

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
