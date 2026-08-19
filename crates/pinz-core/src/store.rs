//! The storage seam.
//!
//! Every pinz tool talks to its data through [`Store`], never to the filesystem
//! or a network directly. Today the only implementation is in-memory; a
//! git-backed-files store and, later, a remote server are further
//! implementations behind this same trait. Keeping the trait free of UI and
//! transport details is what lets the TUI, the Epoz tab, and a future backend
//! share one core. See `design/DESIGN.md` ("The seam").

use crate::model::Board;
// Only the demo fixture builds notes directly; a release build has no seed.
#[cfg(any(test, feature = "fixtures"))]
use crate::model::{Color, Note};

/// Anything that can go wrong reaching the store.
#[derive(Debug)]
pub enum StoreError {
    /// A named board or note was not found.
    NotFound(String),
    /// The backing store (files, network, ...) failed. Message is human-facing.
    Backend(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::NotFound(what) => write!(f, "not found: {what}"),
            StoreError::Backend(msg) => write!(f, "store backend error: {msg}"),
        }
    }
}

impl std::error::Error for StoreError {}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Load and persist the full set of boards.
///
/// Deliberately coarse for now - whole-workspace load/save. We widen it
/// (per-note upsert, queries, sync) only when a real backend needs it, so the
/// trait does not grow speculative methods no caller uses yet.
pub trait Store {
    fn load(&mut self) -> Result<Vec<Board>>;
    fn save(&mut self, boards: &[Board]) -> Result<()>;

    /// Remove a board and everything on it.
    ///
    /// Separate from `save` because `save` writes the boards it is handed and
    /// cannot tell a board that was deleted from one that was never there -
    /// another machine's world, arrived by sync, would look the same. Deleting
    /// is a thing you say, not something inferred from an absence.
    fn delete_board(&mut self, name: &str) -> Result<()>;
}

/// In-memory store, optionally seeded with demo content. Enough to bring a
/// renderer up and to test against; not persistent.
pub struct MemoryStore {
    boards: Vec<Board>,
}

impl MemoryStore {
    pub fn empty() -> Self {
        Self { boards: Vec::new() }
    }

    /// A store carrying the demo boards.
    ///
    /// Test fixture only, and gated so it cannot reach a release binary: the
    /// boards a renderer shows must come from someone's real pin repo, never
    /// from content baked into the program.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn seeded() -> Self {
        Self {
            boards: seed_boards(),
        }
    }
}

impl Store for MemoryStore {
    fn load(&mut self) -> Result<Vec<Board>> {
        Ok(self.boards.clone())
    }

    fn save(&mut self, boards: &[Board]) -> Result<()> {
        self.boards = boards.to_vec();
        Ok(())
    }

    fn delete_board(&mut self, name: &str) -> Result<()> {
        let before = self.boards.len();
        self.boards.retain(|b| b.name != name);
        if self.boards.len() == before {
            return Err(StoreError::NotFound(name.to_string()));
        }
        Ok(())
    }
}

/// Boards for tests and screenshots. Deliberately generic - it doubles as a
/// tutorial, and nothing anybody actually wrote belongs in the source tree.
#[cfg(any(test, feature = "fixtures"))]
fn seed_boards() -> Vec<Board> {
    let mut id = 0u64;
    let mut note = |x: f64, y: f64, color: Color, title: &str, body: &str| {
        id += 1;
        Note {
            id,
            title: title.to_string(),
            body: body.to_string(),
            x,
            y,
            z: id as u32,
            color,
        }
    };

    vec![
        Board {
            name: "ideas".to_string(),
            notes: vec![
                note(120.0, 110.0, Color::Yellow, "welcome", "Drag a pin to move it. Press e to edit, n for a new one, q to quit."),
                note(470.0, 150.0, Color::Peach, "zoom", "Scroll, or + and -, to step through the four levels of detail."),
                note(300.0, 430.0, Color::Mauve, "colors", "Press c to cycle a pin through the palette, C to go back."),
                note(720.0, 380.0, Color::Green, "worlds", "Tab switches boards. A board is just a directory of pins."),
            ],
        },
        Board {
            name: "sketches".to_string(),
            notes: vec![
                note(140.0, 130.0, Color::Blue, "stacking", "Drop pins on each other; whichever you grab comes to the front."),
                note(500.0, 200.0, Color::Teal, "long lines", "A body wider than the pin wraps to the note, and keeps the line breaks you typed."),
                note(360.0, 470.0, Color::Pink, "the edges", "Pan a pin off the screen: it gets cut, never re-flowed."),
            ],
        },
        Board {
            name: "todo".to_string(),
            notes: vec![
                note(150.0, 140.0, Color::Red, "one thing", "The first line is the title. Everything after it is the body."),
                note(520.0, 180.0, Color::Green, "and another", "Pins are markdown files, one per pin, in their own git repo."),
            ],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_store_loads_three_boards() {
        let mut store = MemoryStore::seeded();
        let boards = store.load().unwrap();
        assert_eq!(boards.len(), 3);
        assert!(boards.iter().all(|b| !b.notes.is_empty()));
    }

    #[test]
    fn deleting_a_board_drops_it_from_the_store() {
        let mut store = MemoryStore::empty();
        store
            .save(&[Board::new("ideas"), Board::new("scratch")])
            .unwrap();
        store.delete_board("scratch").unwrap();
        let names: Vec<String> = store.load().unwrap().into_iter().map(|b| b.name).collect();
        assert_eq!(names, ["ideas"]);
    }

    #[test]
    fn deleting_a_board_that_is_not_there_is_not_found() {
        let mut store = MemoryStore::empty();
        store.save(&[Board::new("ideas")]).unwrap();
        assert!(matches!(
            store.delete_board("ghost"),
            Err(StoreError::NotFound(_))
        ));
    }

    #[test]
    fn save_then_load_round_trips() {
        let mut store = MemoryStore::empty();
        let boards = vec![Board::new("scratch")];
        store.save(&boards).unwrap();
        assert_eq!(store.load().unwrap(), boards);
    }
}
