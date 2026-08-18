# Sync conflict handling: visible stops, and position-only auto-merge

Date: 2026-08-18. Status: approved in conversation, this file records it.

## The incident that motivated this

On 2026-08-07 one machine rewrote a pin's body while the other machine had
only bumped the same pin's `z` (a stacking-order nudge from moving pins
around). Two things then went wrong at once:

1. The pull stopped, correctly, but the warning was printed to stderr
   immediately before the TUI entered the alternate screen, so it was wiped
   within milliseconds. The user never saw it.
2. The stop itself was avoidable. A `z` bump is not a judgement call; git
   just cannot know that. The repo then sat diverged for eleven days until
   the next sync surfaced it as a manual conflict.

Two changes, one per failure.

## Change 1: a stopped sync is impossible to miss

- `App` gains a sticky warning, separate from the one-off footer status. It
  is rendered in the footer with warning styling and stays for the whole
  session (a one-off status temporarily takes precedence, then the warning
  returns).
- `run_app` routes a stopped pull into that warning instead of the doomed
  pre-TUI stderr line, and prints it once more to stderr after the terminal
  is restored, so it also lands in scrollback.

## Change 2: pin-aware auto-merge of pull conflicts

`Sync::pull` currently aborts the rebase on any conflict. It now first tries
to resolve each conflicted file itself, at pin granularity, and only aborts
when a real judgement call remains.

A pin file splits into two layers:

- **content**: the title and body (everything below the frontmatter)
- **cosmetics**: the frontmatter fields `x`, `y`, `z`, `color`

Merge rules, three-way against the merge base ("local" is this machine's
commit being replayed, "remote" is the upstream commit):

| Layer | Only one side changed | Both changed, same result | Both changed, differently |
|---|---|---|---|
| content | take the changed side | take it | **stop, human resolves** |
| each cosmetic field | take the changed side | take it | take local |

Local wins cosmetic ties because the person in front of this board arranged
it this way most recently in their own view; the alternative is stopping the
sync over a pin position, which is exactly what this change exists to end.

Guardrails:

- Only both-modified (`UU`) conflicts on `.md` files are eligible. Any other
  conflict shape (delete vs edit, both added with different content, a
  non-pin file) aborts exactly as today.
- If any conflicted file fails its pin merge, the whole rebase is aborted;
  the repo is left exactly as before the pull, same as today. No partial
  resolutions survive.
- A resolved file is re-rendered through the normal pin renderer, so the
  result is always a well-formed pin file.
- The governing rule from `sync.rs` stands: stop rather than guess. This
  change narrows what counts as a guess; it does not remove the stop.

## What does not change

- Body-vs-body conflicts still stop with the same message.
- `push`, `fetch`, `status`, and the sync subcommand flow are untouched.
- No new dependencies; the merge uses the existing pin parser and renderer.
