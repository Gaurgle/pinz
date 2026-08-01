//! The domain: what a bulletin board is made of. No rendering, no I/O.

use crate::geometry::WorldPoint;

/// Fixed world-space size of every post-it. Notes are uniform on purpose - it
/// keeps layout, hit-testing, and stacking simple. Zoom changes how big a note
/// *looks*, never its world size.
pub const NOTE_W: f64 = 200.0;
pub const NOTE_H: f64 = 150.0;

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
        WorldPoint { x: self.x, y: self.y }
    }

    /// Center point, handy for "zoom in and focus this note".
    pub fn center(&self) -> WorldPoint {
        WorldPoint { x: self.x + NOTE_W / 2.0, y: self.y + NOTE_H / 2.0 }
    }

    /// Whether `p` (world space) lands on this note. The basis of hit-testing
    /// once a click has been un-projected to world coordinates. Left/top edges
    /// are inclusive, right/bottom exclusive.
    pub fn contains(&self, p: WorldPoint) -> bool {
        p.x >= self.x
            && p.x < self.x + NOTE_W
            && p.y >= self.y
            && p.y < self.y + NOTE_H
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
        Self { name: name.into(), notes: Vec::new() }
    }

    /// The topmost note under a world-space point, respecting stack order.
    /// This is what a click resolves to.
    pub fn note_at(&self, p: WorldPoint) -> Option<&Note> {
        self.notes.iter().filter(|n| n.contains(p)).max_by_key(|n| n.z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note_at(x: f64, y: f64, z: u32, title: &str) -> Note {
        Note { id: 1, title: title.into(), body: String::new(), x, y, z, color: Color::Yellow }
    }

    #[test]
    fn note_contains_respects_its_edges() {
        let n = note_at(10.0, 20.0, 0, "t");
        assert!(n.contains(WorldPoint { x: 10.0, y: 20.0 }), "top-left is inclusive");
        assert!(n.contains(WorldPoint { x: 10.0 + NOTE_W - 0.1, y: 20.0 }));
        assert!(!n.contains(WorldPoint { x: 10.0 + NOTE_W, y: 20.0 }), "right edge is exclusive");
        assert!(!n.contains(WorldPoint { x: 9.0, y: 20.0 }));
    }

    #[test]
    fn note_at_returns_the_topmost_note() {
        let mut b = Board::new("t");
        b.notes.push(note_at(0.0, 0.0, 0, "under"));
        b.notes.push(note_at(0.0, 0.0, 5, "over"));
        assert_eq!(b.note_at(WorldPoint { x: 1.0, y: 1.0 }).unwrap().title, "over");
        assert!(b.note_at(WorldPoint { x: 9999.0, y: 0.0 }).is_none());
    }

    #[test]
    fn color_round_trips_through_str() {
        for c in Color::ALL {
            assert_eq!(c.as_str().parse::<Color>(), Ok(c));
        }
        assert_eq!("chartreuse".parse::<Color>(), Err(ParseColorError));
    }
}
