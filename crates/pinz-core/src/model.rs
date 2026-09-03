//! The domain: what a bulletin board is made of. No rendering, no I/O.

use crate::geometry::WorldPoint;

/// Fixed world-space size of every post-it. Notes are uniform on purpose - it
/// keeps layout, hit-testing, and stacking simple. Zoom changes how big a note
/// *looks*, never its world size.
pub const NOTE_W: f64 = 200.0;
pub const NOTE_H: f64 = 150.0;

/// Minimum world-space gap between two pins' top-left corners, and the step a
/// blocked placement cascades by.
///
/// Two pins closer than this on *both* axes are one pin as far as the eye is
/// concerned, and pin files store x and y rounded to whole units, so a near
/// miss becomes an exact overlap on the next reload and the pin underneath is
/// lost for good. One eighth of a note keeps the cascade parallel to the
/// note's diagonal, so an offset pin's border and the start of its title stay
/// visible at every zoom you can read text at.
pub const CASCADE_X: f64 = NOTE_W / 8.0;
pub const CASCADE_Y: f64 = NOTE_H / 8.0;

/// A note's background, from the Catppuccin Mocha accents. A closed enum, so a
/// renderer maps it to whatever its medium needs (a hex string, an ANSI color).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Yellow,
    Green,
    Blue,
    Pink,
    Peach,
    Mauve,
    Teal,
    Red,
}

impl Color {
    pub const ALL: [Color; 8] = [
        Color::Yellow,
        Color::Green,
        Color::Blue,
        Color::Pink,
        Color::Peach,
        Color::Mauve,
        Color::Teal,
        Color::Red,
    ];

    /// Stable lowercase name, used when a note round-trips through a notez2
    /// frontmatter field.
    pub fn as_str(self) -> &'static str {
        match self {
            Color::Yellow => "yellow",
            Color::Green => "green",
            Color::Blue => "blue",
            Color::Pink => "pink",
            Color::Peach => "peach",
            Color::Mauve => "mauve",
            Color::Teal => "teal",
            Color::Red => "red",
        }
    }
}

/// Returned when a string doesn't name one of the eight note colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseColorError;

impl std::fmt::Display for ParseColorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "not a note color")
    }
}

impl std::error::Error for ParseColorError {}

impl std::str::FromStr for Color {
    type Err = ParseColorError;

    /// The inverse of [`Color::as_str`], so a color survives a round trip
    /// through a pin file's frontmatter.
    fn from_str(s: &str) -> std::result::Result<Color, ParseColorError> {
        Ok(match s {
            "yellow" => Color::Yellow,
            "green" => Color::Green,
            "blue" => Color::Blue,
            "pink" => Color::Pink,
            "peach" => Color::Peach,
            "mauve" => Color::Mauve,
            "teal" => Color::Teal,
            "red" => Color::Red,
            _ => return Err(ParseColorError),
        })
    }
}

/// A single post-it.
///
/// This is a notez2 note (title + body) plus the spatial metadata that living
/// on a board adds: position, stack order, color. Those extra fields are
/// exactly what would serialize into a notez2 file's frontmatter, so a note can
/// round-trip between pinz and a notez2 workspace.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub id: u64,
    pub title: String,
    pub body: String,
    /// Top-left corner in world space.
    pub x: f64,
    pub y: f64,
    /// Stack order; higher sits on top. Dragging a note to the front raises it.
    pub z: u32,
    pub color: Color,
}

impl Note {
    /// Top-left corner as a point.
    pub fn position(&self) -> WorldPoint {
        WorldPoint {
            x: self.x,
            y: self.y,
        }
    }

    /// Center point, handy for "zoom in and focus this note".
    pub fn center(&self) -> WorldPoint {
        WorldPoint {
            x: self.x + NOTE_W / 2.0,
            y: self.y + NOTE_H / 2.0,
        }
    }

    /// Whether `p` (world space) lands on this note. The basis of hit-testing
    /// once a click has been un-projected to world coordinates. Left/top edges
    /// are inclusive, right/bottom exclusive.
    pub fn contains(&self, p: WorldPoint) -> bool {
        p.x >= self.x && p.x < self.x + NOTE_W && p.y >= self.y && p.y < self.y + NOTE_H
    }
}

/// A named board - the "world" you switch between in the header tabs.
#[derive(Debug, Clone, PartialEq)]
pub struct Board {
    pub name: String,
    pub notes: Vec<Note>,
}

impl Board {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            notes: Vec::new(),
        }
    }

    /// The topmost note under a world-space point, respecting stack order.
    /// This is what a click resolves to.
    pub fn note_at(&self, p: WorldPoint) -> Option<&Note> {
        self.notes
            .iter()
            .filter(|n| n.contains(p))
            .max_by_key(|n| n.z)
    }

    /// Bounding box of every pin on the board (top-left min, bottom-right
    /// max), or `None` when the board is empty.
    pub fn bounds(&self) -> Option<(WorldPoint, WorldPoint)> {
        let first = self.notes.first()?;
        let mut min = WorldPoint {
            x: first.x,
            y: first.y,
        };
        let mut max = WorldPoint {
            x: first.x + NOTE_W,
            y: first.y + NOTE_H,
        };
        for n in &self.notes {
            min.x = min.x.min(n.x);
            min.y = min.y.min(n.y);
            max.x = max.x.max(n.x + NOTE_W);
            max.y = max.y.max(n.y + NOTE_H);
        }
        Some((min, max))
    }

    /// The middle of the board's content: the point a camera arriving here
    /// frames. An empty board has nothing to frame, so it looks at the origin.
    pub fn content_center(&self) -> WorldPoint {
        match self.bounds() {
            Some((min, max)) => WorldPoint {
                x: (min.x + max.x) / 2.0,
                y: (min.y + max.y) / 2.0,
            },
            None => WorldPoint { x: 0.0, y: 0.0 },
        }
    }

    /// A free spot for a pin that wants to sit at `wanted`, cascading
    /// down-right past anything already close enough to hide it. `exclude` is
    /// the pin being placed, so a pin dropped back where it already was does
    /// not run away from itself.
    ///
    /// This terminates: successive candidates sit exactly one gap apart, so a
    /// single pin can block at most two of them, and the board has finitely
    /// many pins. The loop bound is that argument written down, not a guess.
    pub fn free_spot(&self, wanted: WorldPoint, exclude: Option<u64>) -> WorldPoint {
        let mut spot = wanted;
        for _ in 0..=self.notes.len() * 2 {
            let taken = self.notes.iter().any(|n| {
                Some(n.id) != exclude
                    && (n.x - spot.x).abs() < CASCADE_X
                    && (n.y - spot.y).abs() < CASCADE_Y
            });
            if !taken {
                break;
            }
            spot.x += CASCADE_X;
            spot.y += CASCADE_Y;
        }
        spot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note_at(x: f64, y: f64, z: u32, title: &str) -> Note {
        Note {
            id: 1,
            title: title.into(),
            body: String::new(),
            x,
            y,
            z,
            color: Color::Yellow,
        }
    }

    #[test]
    fn note_contains_respects_its_edges() {
        let n = note_at(10.0, 20.0, 0, "t");
        assert!(
            n.contains(WorldPoint { x: 10.0, y: 20.0 }),
            "top-left is inclusive"
        );
        assert!(n.contains(WorldPoint {
            x: 10.0 + NOTE_W - 0.1,
            y: 20.0
        }));
        assert!(
            !n.contains(WorldPoint {
                x: 10.0 + NOTE_W,
                y: 20.0
            }),
            "right edge is exclusive"
        );
        assert!(!n.contains(WorldPoint { x: 9.0, y: 20.0 }));
    }

    #[test]
    fn note_at_returns_the_topmost_note() {
        let mut b = Board::new("t");
        b.notes.push(note_at(0.0, 0.0, 0, "under"));
        b.notes.push(note_at(0.0, 0.0, 5, "over"));
        assert_eq!(
            b.note_at(WorldPoint { x: 1.0, y: 1.0 }).unwrap().title,
            "over"
        );
        assert!(b.note_at(WorldPoint { x: 9999.0, y: 0.0 }).is_none());
    }

    #[test]
    fn color_round_trips_through_str() {
        for c in Color::ALL {
            assert_eq!(c.as_str().parse::<Color>(), Ok(c));
        }
        assert_eq!("chartreuse".parse::<Color>(), Err(ParseColorError));
    }

    fn board_with(positions: &[(f64, f64)]) -> Board {
        let mut b = Board::new("t");
        for (i, (x, y)) in positions.iter().enumerate() {
            let mut n = note_at(*x, *y, i as u32, "pin");
            n.id = i as u64 + 1;
            b.notes.push(n);
        }
        b
    }

    #[test]
    fn free_spot_leaves_a_clear_position_alone() {
        let b = board_with(&[(0.0, 0.0)]);
        let want = WorldPoint { x: 900.0, y: 900.0 };
        assert_eq!(b.free_spot(want, None), want);
    }

    #[test]
    fn free_spot_on_an_empty_board_is_the_wanted_spot() {
        let b = Board::new("empty");
        let want = WorldPoint { x: 12.0, y: 34.0 };
        assert_eq!(b.free_spot(want, None), want);
    }

    #[test]
    fn free_spot_cascades_past_a_pin_already_there() {
        let b = board_with(&[(0.0, 0.0)]);
        assert_eq!(
            b.free_spot(WorldPoint { x: 0.0, y: 0.0 }, None),
            WorldPoint {
                x: CASCADE_X,
                y: CASCADE_Y
            }
        );
    }

    #[test]
    fn free_spot_cascades_past_a_whole_pile() {
        let b = board_with(&[
            (0.0, 0.0),
            (CASCADE_X, CASCADE_Y),
            (CASCADE_X * 2.0, CASCADE_Y * 2.0),
        ]);
        assert_eq!(
            b.free_spot(WorldPoint { x: 0.0, y: 0.0 }, None),
            WorldPoint {
                x: CASCADE_X * 3.0,
                y: CASCADE_Y * 3.0
            }
        );
    }

    #[test]
    fn free_spot_ignores_the_pin_being_moved() {
        let b = board_with(&[(50.0, 60.0)]);
        let here = WorldPoint { x: 50.0, y: 60.0 };
        assert_eq!(
            b.free_spot(here, Some(1)),
            here,
            "a pin dropped back where it already was must not run from itself"
        );
    }

    /// Positions are written to disk rounded to whole units, so a near miss is
    /// an exact overlap after the next reload.
    #[test]
    fn free_spot_treats_a_near_miss_as_taken() {
        let b = board_with(&[(0.0, 0.0)]);
        let want = WorldPoint { x: 0.4, y: 0.4 };
        assert_eq!(
            b.free_spot(want, None),
            WorldPoint {
                x: 0.4 + CASCADE_X,
                y: 0.4 + CASCADE_Y
            }
        );
    }

    #[test]
    fn bounds_cover_every_pin_on_the_board() {
        let b = board_with(&[(0.0, 0.0), (-40.0, 25.0)]);
        let (min, max) = b.bounds().expect("a board with pins has bounds");
        assert_eq!(min, WorldPoint { x: -40.0, y: 0.0 });
        assert_eq!(
            max,
            WorldPoint {
                x: NOTE_W,
                y: 25.0 + NOTE_H
            }
        );
    }

    #[test]
    fn an_empty_board_has_no_bounds_and_is_centered_on_the_origin() {
        let b = Board::new("empty");
        assert_eq!(b.bounds(), None);
        assert_eq!(b.content_center(), WorldPoint { x: 0.0, y: 0.0 });
    }

    #[test]
    fn content_center_is_the_middle_of_the_bounding_box() {
        let b = board_with(&[(0.0, 0.0), (NOTE_W, NOTE_H)]);
        assert_eq!(
            b.content_center(),
            WorldPoint {
                x: NOTE_W,
                y: NOTE_H
            }
        );
    }
}
