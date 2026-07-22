//! The renderer's projection spine.
//!
//! `pinz-core` owns the pure world/screen math ([`Projection`]); this wraps it
//! for the terminal medium. Two things the core deliberately leaves to a
//! renderer live here:
//!
//! - **cell aspect** - terminal cells are ~twice as tall as wide, so a
//!   world-square note must be squashed vertically to look square ([`CELL_ASPECT`]).
//! - **display unit** - the core's zoom scales are tuned as *ratios* (see
//!   `ZoomLevel::scale`); their absolute values are pixel-sized. One world unit
//!   at document zoom would be a whole cell, making a 200-wide note 200 columns
//!   across. [`CELL_UNIT`] is the single scalar that rescales the whole ladder
//!   to terminal proportions. It multiplies x and y equally, so it never
//!   disturbs the aspect correction the core already applied to y.
//!
//! Everything downstream - hit-testing, panning, note rects - goes through
//! [`View`], so the inverse ([`View::world_at`]) stays exactly consistent with
//! the forward projection.

use pinz_core::{Camera, Projection, ScreenPoint, WorldPoint, NOTE_H, NOTE_W};
use ratatui::layout::Rect;

/// `cell_width / cell_height` for a terminal. Square-pixel media (GUI, the HTML
/// demo) would use 1.0; a text cell is roughly half as wide as tall.
pub const CELL_ASPECT: f64 = 0.5;

/// Cells per world-unit at document zoom, on top of the core's ladder. Chosen so
/// a note (200x150 world) is a comfortable ~40x15 cells fully zoomed in and a
/// small block when zoomed out.
pub const CELL_UNIT: f64 = 0.2;

/// A camera bound to a concrete viewport rectangle, ready to convert between
/// world coordinates and terminal cells.
#[derive(Debug, Clone, Copy)]
pub struct View {
    proj: Projection,
    /// The viewport in terminal coordinates; cell offsets are relative to its
    /// top-left corner.
    area: Rect,
}

impl View {
    pub fn new(camera: Camera, area: Rect) -> Self {
        Self {
            proj: Projection::new(camera, CELL_ASPECT),
            area,
        }
    }

    /// Cells-per-world-unit along each axis at the current zoom. Handy for
    /// turning a screen-space drag (in cells) back into a world delta.
    pub fn scale(&self) -> (f64, f64) {
        let s = self.proj.camera.zoom.scale() * CELL_UNIT;
        (s, s * CELL_ASPECT)
    }

    /// World point -> cell offset from the viewport's top-left (fractional).
    pub fn cell_of(&self, w: WorldPoint) -> (f64, f64) {
        let s = self.proj.to_screen(w);
        (s.x * CELL_UNIT, s.y * CELL_UNIT)
    }

    /// Cell offset from the viewport's top-left -> world point. The inverse of
    /// [`View::cell_of`].
    pub fn world_of(&self, cx: f64, cy: f64) -> WorldPoint {
        self.proj.to_world(ScreenPoint {
            x: cx / CELL_UNIT,
            y: cy / CELL_UNIT,
        })
    }

    /// Absolute terminal cell (as crossterm reports mouse positions) -> world
    /// point. This is what turns a click into "which note did I grab".
    pub fn world_at(&self, col: u16, row: u16) -> WorldPoint {
        let cx = col as f64 - self.area.x as f64;
        let cy = row as f64 - self.area.y as f64;
        self.world_of(cx, cy)
    }

    /// A note's on-screen size in whole-ish cells at the current zoom.
    pub fn note_size(&self) -> (f64, f64) {
        let (w, h) = self.proj.scale_size(NOTE_W, NOTE_H);
        (w * CELL_UNIT, h * CELL_UNIT)
    }

    /// The terminal rect a note occupies, clipped to the viewport. `None` when
    /// the note is fully off-screen.
    pub fn note_rect(&self, top_left: WorldPoint) -> Option<Rect> {
        let (cx, cy) = self.cell_of(top_left);
        let (w, h) = self.note_size();
        let x0 = self.area.x as f64 + cx;
        let y0 = self.area.y as f64 + cy;
        clip(x0, y0, w, h, self.area)
    }
}

/// Intersect a fractional rect (in terminal coordinates) with the viewport,
/// rounding to whole cells. Returns `None` if nothing is left.
fn clip(x0: f64, y0: f64, w: f64, h: f64, area: Rect) -> Option<Rect> {
    let ax0 = area.x as i64;
    let ay0 = area.y as i64;
    let ax1 = ax0 + area.width as i64;
    let ay1 = ay0 + area.height as i64;

    let nx0 = x0.round() as i64;
    let ny0 = y0.round() as i64;
    let nx1 = (x0 + w).round() as i64;
    let ny1 = (y0 + h).round() as i64;

    let cx0 = nx0.max(ax0);
    let cy0 = ny0.max(ay0);
    let cx1 = nx1.min(ax1);
    let cy1 = ny1.min(ay1);

    if cx1 <= cx0 || cy1 <= cy0 {
        return None;
    }
    Some(Rect {
        x: cx0 as u16,
        y: cy0 as u16,
        width: (cx1 - cx0) as u16,
        height: (cy1 - cy0) as u16,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinz_core::{WorldPoint, ZoomLevel};

    fn view(zoom: ZoomLevel) -> View {
        let cam = Camera {
            origin: WorldPoint { x: 0.0, y: 0.0 },
            zoom,
        };
        View::new(
            cam,
            Rect {
                x: 0,
                y: 0,
                width: 120,
                height: 40,
            },
        )
    }

    #[test]
    fn cell_and_world_round_trip() {
        let v = view(ZoomLevel::Titles);
        let w = WorldPoint { x: 321.0, y: 210.0 };
        let (cx, cy) = v.cell_of(w);
        let back = v.world_of(cx, cy);
        assert!((back.x - w.x).abs() < 1e-9);
        assert!((back.y - w.y).abs() < 1e-9);
    }

    #[test]
    fn world_at_accounts_for_viewport_offset() {
        let cam = Camera {
            origin: WorldPoint { x: 0.0, y: 0.0 },
            zoom: ZoomLevel::Document,
        };
        // Same camera, viewport shifted down/right: the same world point must
        // land at a correspondingly shifted terminal cell.
        let a = View::new(
            cam,
            Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
        );
        let b = View::new(
            cam,
            Rect {
                x: 10,
                y: 5,
                width: 80,
                height: 24,
            },
        );
        let w = WorldPoint { x: 50.0, y: 60.0 };
        let (ax, ay) = a.cell_of(w);
        assert!((b.world_at((ax as u16) + 10, (ay as u16) + 5).x - w.x).abs() < 1.0);
    }

    #[test]
    fn note_at_origin_is_visible_but_far_note_clips_out() {
        let v = view(ZoomLevel::Document);
        assert!(v.note_rect(WorldPoint { x: 0.0, y: 0.0 }).is_some());
        assert!(v
            .note_rect(WorldPoint {
                x: 99_000.0,
                y: 99_000.0
            })
            .is_none());
    }

    #[test]
    fn note_looks_wider_than_tall_in_cells() {
        // 200x150 world (ratio 1.33). In cells the aspect squash makes it read
        // much wider than tall, which is what keeps it square on screen.
        let v = view(ZoomLevel::Document);
        let (w, h) = v.note_size();
        assert!(w > h * 2.0, "expected a wide cell footprint, got {w}x{h}");
    }
}
