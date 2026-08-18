//! Spatial math: the world/screen projection every renderer shares.
//!
//! pinz is a tiny 2D engine. The board is an unbounded **world** in `f64`
//! coordinates; a **camera** looks at a rectangular slice of it. Getting the
//! projection and its inverse right is the whole ballgame - panning, zooming,
//! and mouse hit-testing are all applications of these two functions.

/// A point in board (world) space. Independent of zoom and of the medium.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldPoint {
    pub x: f64,
    pub y: f64,
}

/// A point in the visible surface. Units are terminal cells (TUI) or pixels
/// (GUI); the caller rounds to whole cells only at the very edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenPoint {
    pub x: f64,
    pub y: f64,
}

/// Discrete zoom steps. There is no continuous zoom: a terminal is a grid of
/// cells, so we snap to four levels of detail. The variant also decides how a
/// note is *rendered* (the LOD ladder in DESIGN.md), not merely its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomLevel {
    /// Solid colored blocks, no text: the whole-board overview.
    Cluster,
    /// Title only.
    Titles,
    /// Title plus a body preview.
    Preview,
    /// Full note, editable.
    Document,
}

impl ZoomLevel {
    /// Every level, ordered from most zoomed out to most zoomed in.
    pub const ALL: [ZoomLevel; 4] = [
        ZoomLevel::Cluster,
        ZoomLevel::Titles,
        ZoomLevel::Preview,
        ZoomLevel::Document,
    ];

    /// World-units-to-screen scale for this level. Larger means more zoomed in.
    /// These are tuned constants, not a formula; the ratios are what matter.
    pub fn scale(self) -> f64 {
        match self {
            ZoomLevel::Cluster => 0.24,
            ZoomLevel::Titles => 0.42,
            ZoomLevel::Preview => 0.68,
            ZoomLevel::Document => 1.0,
        }
    }

    /// Short label for the zoom indicator in the header.
    pub fn label(self) -> &'static str {
        match self {
            ZoomLevel::Cluster => "cluster",
            ZoomLevel::Titles => "titles",
            ZoomLevel::Preview => "preview",
            ZoomLevel::Document => "document",
        }
    }

    /// Position in `ALL`, i.e. 0 for `Cluster` up to 3 for `Document`.
    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|&z| z == self)
            .expect("every variant is in ALL")
    }

    fn from_index(i: isize) -> ZoomLevel {
        let last = (Self::ALL.len() - 1) as isize;
        Self::ALL[i.clamp(0, last) as usize]
    }

    /// One step more zoomed in, clamped at `Document`.
    pub fn zoomed_in(self) -> ZoomLevel {
        Self::from_index(self.index() as isize + 1)
    }

    /// One step more zoomed out, clamped at `Cluster`.
    pub fn zoomed_out(self) -> ZoomLevel {
        Self::from_index(self.index() as isize - 1)
    }
}

/// Where the camera is and how far it is zoomed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// World coordinate shown at the viewport's top-left corner.
    pub origin: WorldPoint,
    pub zoom: ZoomLevel,
}

/// A camera plus the renderer's cell aspect ratio: a concrete projection.
///
/// `cell_aspect` is `cell_width / cell_height` in the target medium. Terminal
/// cells are roughly twice as tall as wide (~0.5); square pixels (GUI, the HTML
/// demo) use 1.0. Without this correction a note that is square in world space
/// would look stretched in the terminal.
#[derive(Debug, Clone, Copy)]
pub struct Projection {
    pub camera: Camera,
    pub cell_aspect: f64,
}

impl Projection {
    pub fn new(camera: Camera, cell_aspect: f64) -> Self {
        Self {
            camera,
            cell_aspect,
        }
    }

    fn scale_x(&self) -> f64 {
        self.camera.zoom.scale()
    }

    fn scale_y(&self) -> f64 {
        self.camera.zoom.scale() * self.cell_aspect
    }

    /// World -> screen.
    pub fn to_screen(&self, w: WorldPoint) -> ScreenPoint {
        ScreenPoint {
            x: (w.x - self.camera.origin.x) * self.scale_x(),
            y: (w.y - self.camera.origin.y) * self.scale_y(),
        }
    }

    /// Screen -> world. The inverse mouse hit-testing depends on: given a click
    /// at a screen cell, this returns the world point (and thus the note) under
    /// it.
    pub fn to_world(&self, s: ScreenPoint) -> WorldPoint {
        WorldPoint {
            x: self.camera.origin.x + s.x / self.scale_x(),
            y: self.camera.origin.y + s.y / self.scale_y(),
        }
    }

    /// Screen size of something with fixed world-space dimensions (e.g. a note).
    pub fn scale_size(&self, world_w: f64, world_h: f64) -> (f64, f64) {
        (world_w * self.scale_x(), world_h * self.scale_y())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn projection_round_trips_across_cameras_and_media() {
        let cameras = [
            Camera {
                origin: WorldPoint { x: 0.0, y: 0.0 },
                zoom: ZoomLevel::Document,
            },
            Camera {
                origin: WorldPoint { x: 137.5, y: -42.0 },
                zoom: ZoomLevel::Titles,
            },
            Camera {
                origin: WorldPoint { x: 900.0, y: 610.0 },
                zoom: ZoomLevel::Cluster,
            },
        ];
        let w = WorldPoint { x: 321.0, y: 654.0 };
        for cam in cameras {
            for aspect in [1.0, 0.5] {
                let p = Projection::new(cam, aspect);
                let back = p.to_world(p.to_screen(w));
                assert!(
                    approx(back.x, w.x) && approx(back.y, w.y),
                    "round trip failed for {cam:?} aspect {aspect}: got {back:?}"
                );
            }
        }
    }

    #[test]
    fn zoom_steps_and_clamps_at_both_ends() {
        assert_eq!(ZoomLevel::Cluster.zoomed_out(), ZoomLevel::Cluster);
        assert_eq!(ZoomLevel::Document.zoomed_in(), ZoomLevel::Document);
        assert_eq!(ZoomLevel::Titles.zoomed_in(), ZoomLevel::Preview);
        assert_eq!(ZoomLevel::Titles.zoomed_out(), ZoomLevel::Cluster);
    }

    #[test]
    fn scale_is_monotonic_with_zoom() {
        let mut prev = 0.0;
        for z in ZoomLevel::ALL {
            assert!(z.scale() > prev, "{z:?} broke monotonic scale");
            prev = z.scale();
        }
    }
}
