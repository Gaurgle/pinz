# Camera glide: a jump moves, a manipulation does not

Date: 2026-08-19. Status: **specified, not built**. Andreas's idea.

## Why

Keyboard stepping between pins (same day, `feat/keyboard-pin-navigation`) can
move the board a long way on one keypress. A cut leaves you re-reading the
board afterwards to work out what moved and in which direction, which is the
one thing you already knew a moment earlier and just lost.

A glide is not decoration here. It carries two facts a cut throws away:
**which way** the board moved, and **roughly how far**. That is the whole
argument for it, and it is also why the animation is on the camera rather than
on the pin: a flash on the pin you landed on says "here" but nothing about
where you came from.

## The rule

**A jump glides. A manipulation does not.**

Glides:

| Call site | Trigger |
|---|---|
| `bring_into_view` | `shift`+arrow / `shift`+`hjkl` onto an off-screen pin |
| `center_on_note` | `e` or `enter` opening a pin |

Cuts, and why:

- **Drag, and scroll-wheel pan.** These follow the pointer. Anything but 1:1
  is a bug, not a style.
- **Arrow-key panning.** A held arrow key would queue 140ms glides behind each
  other and read as lag. It is a manipulation, one step at a time, not a jump
  to a destination.
- **Zoom, and the origin change that comes with it.** See below.
- **Switching worlds, and the first centering on open.** Gliding between two
  different boards is meaningless: there is no continuous space between them
  to travel through.

Written as a rule rather than a list because the list is short today and will
not stay that way. A new camera move should be able to answer "is this a jump
to somewhere, or is the user's hand on it?" and get the right answer without
consulting this file.

## Zoom is not interpolated

`Camera.zoom` is a four-value `ZoomLevel`, and `Projection::scale_x`
(`crates/pinz-core/src/geometry.rs`) reads `camera.zoom.scale()` directly.
Interpolating it means teaching `pinz-core` - the renderer-agnostic half -
about a scale that sits between two rungs of its own ladder.

The cheapest honest version was considered and set aside: give `Camera` a
`scale: Option<f64>` meaning "the continuous scale actually in force", with
`None` falling back to the enum. Six lines in core, no behaviour change when
unset. It was rejected for now on the grounds that **the smoothness would be
partial while the cost lands in core**: the render split is chosen per level
(solid blocks at cluster, bordered widgets above it), so notes would scale
smoothly and then change character in a single frame anyway. Paying a core
change for a half-smooth transition is the wrong trade while zoom still reads
as a deliberate mode change rather than a movement.

Because zoom cuts, the origin change that *accompanies* a zoom cuts with it.
`zoom_at` holds the point under the cursor fixed while the scale changes;
gliding that pan while the scale jumps under it would look worse than either
one alone.

Revisit only if the zoom cut proves jarring in use. This is a preference to
be re-examined, not a closed door.

## The state

`App` gains one field:

```rust
/// A camera glide in flight: where the view was when it started, and how far
/// through it we are. Only the origin is interpolated; zoom cuts.
struct Glide {
    from: WorldPoint,
    elapsed: Duration,
}
```

`self.camera` keeps its current meaning throughout: **the camera we are going
to**. Every existing operation that mutates it - `pan_cells`, `zoom_at`,
`clamp_origin`, `center_on_content` - is untouched. Only two things are new:

- Starting a glide snapshots the currently *displayed* origin into
  `Glide::from` before mutating `self.camera`.
- `App::camera()` returns the eased blend from `Glide::from` toward
  `self.camera.origin` while a glide is in flight, and `self.camera` otherwise.

Easing is ease-out cubic (`1 - (1 - t)³`) over a `GLIDE` constant of about
140ms, so the board decelerates into place rather than stopping dead.

**Retargeting, not queueing.** A glide started while one is in flight
snapshots the position currently on screen. Holding `shift`+arrow therefore
produces one continuous movement across the board rather than a series of
lurches to intermediate pins.

`App::animating()` reports whether a glide is in flight. It is what the runner
branches on, so it is public API rather than an internal detail.

`camera()` is what `View` is built from, so the existing invariant holds
unchanged: every spatial operation still goes through one projection.
Hit-testing mid-glide tests against what is on screen, which is what makes a
click during a glide land on the pin you can see rather than one that has not
arrived yet.

## Time enters at the edge

```rust
pub fn tick(&mut self, dt: Duration)
```

`App` never reads a clock. It is handed an elapsed duration and advances by
it, which keeps it the pure state machine the domain invariants call for and
makes every test deterministic with no sleeping: press a key, `tick(GLIDE)`,
assert. `Instant` stays in `main.rs`, where I/O already lives.

Two alternatives were rejected:

- **`tick(now: Instant)`.** Puts a clock type in `App` and invites comparing
  instants from different sources. Nothing needs absolute time.
- **Frame counting - `tick()` advances one frame.** Simplest of all, but the
  duration then depends on how fast frames happen to arrive. Input during a
  glide produces extra frames, so the animation would speed up exactly when
  the user is busiest.

`tick` must not touch `revision`. The runner writes pins to disk when
`revision` moves, and a glide changes no board data; a ticking animation that
rewrote pin files sixty times a second would be a serious bug.

## The runner

The loop keeps blocking. It only polls while something is actually moving,
with `FRAME` a frame budget of about 16ms:

```rust
let mut last = Instant::now();
loop {
    terminal.draw(...)?;
    if app.should_quit() { return Ok(None); }
    if app.animating() {
        if event::poll(FRAME)? {
            apply(app, event::read()?);
        }
        let now = Instant::now();
        app.tick(now - last);
        last = now;
    } else {
        apply(app, event::read()?);   // blocks, possibly for hours
        last = Instant::now();        // a glide may have just started
    }
    // ... deliver_copy and the save check, unchanged
}
```

**The elapsed reset is the subtle part.** `last` must be re-stamped *after*
the blocking read, not before it. Otherwise the first tick of a glide is
handed however long the user sat looking at the board, and the animation
completes in one frame - which is a cut with extra steps, and would pass any
test that only checks the final position.

**The zero-idle claim survives, stated precisely:** idle means no glide in
flight, and with no glide in flight the loop blocks on `event::read()` exactly
as it does today. pinz still uses no CPU sitting open on a desk. What changes
is that for roughly 140ms after a keyboard step it draws at a frame budget.

## What this costs

- `crates/pinz-tui/src/main.rs` - the doc comment on `run` currently reads
  "No animation loop: the board only changes in response to input, so a redraw
  per event is enough and keeps the app idle at zero CPU." It is amended, not
  deleted: the zero-idle half is still true and still deliberate.
- `App::camera()` changes meaning, from "where we are going" to "what is on
  screen". This breaks one existing test,
  `the_camera_moves_only_for_a_pin_it_cannot_show`, which asserts the origin
  has moved immediately after a step - under a glide it has not moved yet at
  `t=0`. It becomes tick-to-completion, then assert. That is a better test:
  it proves the pin actually arrives rather than that a number changed.
- `App` gains a notion of elapsed time. It still has no clock, no I/O and no
  drawing.
- Roughly forty lines in `pinz-tui`. Nothing in `pinz-core`.
- DESIGN.md gains a bullet: the redraw model is a standing decision and is
  currently recorded only as a comment in `main.rs`.

## What was rejected

- **An always-on tick loop.** Simplest to write and it throws away the
  property `main.rs` was protecting, for an app that spends most of its life
  idle on a second monitor.
- **A pulse on the landed pin instead of a glide.** Considered, and it does
  cover the case where the camera does not move at all. It carries no
  direction, which is the information the animation exists to convey. Worth
  revisiting as an addition, never as a replacement.
- **A continuous zoom ladder.** Genuinely smooth, and it dissolves the
  four-level ladder that the render split, the level-of-detail choice and
  DESIGN.md are all built on.

## Testing

All of it without a terminal and without sleeping, since `tick(dt)` takes the
elapsed time as an argument:

- A step onto an off-screen pin leaves `camera()` where it was on the frame it
  is pressed, and puts the pin on screen once `tick(GLIDE)` has run.
- Half a glide puts the camera strictly between the two positions, so the
  easing is actually applied rather than the end state being snapped to.
- A second step mid-glide starts from the displayed position, not from the
  original one: no lurch backwards.
- `animating()` is false before any glide and false again once one completes,
  which is what the runner's blocking read depends on.
- `tick` does not change `revision`.
- A drag, a scroll-zoom and an arrow pan all move `camera()` on the same frame
  they happen: the manipulation half of the rule.
- `e` centres the pin once the glide completes.

## Open questions, to settle in use rather than on paper

- **140ms is a guess.** It wants tuning against a real board on a real
  terminal, including over SSH where each frame is a round trip.
- **Whether a short hop should glide at all.** Moving a third of a screen may
  read better as a cut. A distance threshold is easy to add later and
  impossible to pick sensibly without using it first.
