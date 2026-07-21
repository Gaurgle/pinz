//! The storage seam.
//!
//! Every pinz tool talks to its data through [`Store`], never to the filesystem
//! or a network directly. Today the only implementation is in-memory; a
//! git-backed-files store and, later, a remote server are further
//! implementations behind this same trait. Keeping the trait free of UI and
//! transport details is what lets the TUI, the Epoz tab, and a future backend
//! share one core. See `design/DESIGN.md` ("The seam").

use crate::model::{Board, Color, Note};

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

    /// Seeded with the same three boards as the design demo.
    pub fn seeded() -> Self {
        Self { boards: seed_boards() }
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
}

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
                note(120.0, 110.0, Color::Yellow, "pinz", "Terminal bulletin board. Discrete LOD zoom ladder."),
                note(470.0, 150.0, Color::Peach, "Auracast venue audits", "Map where broadcast audio should reach. Sell the report."),
                note(300.0, 430.0, Color::Mauve, "WM leak-tracing service", "Turn the audio watermark into a leak-tracing offering."),
                note(720.0, 380.0, Color::Green, "Fortnox integrations", "Small automations for Swedish e-commerce bookkeeping."),
            ],
        },
        Board {
            name: "wavez".to_string(),
            notes: vec![
                note(140.0, 130.0, Color::Blue, "Broadcast platform", "Auracast source/sink/assistant roles as a product."),
                note(500.0, 200.0, Color::Teal, "BASS assistant role", "Scan and hand off the broadcast code to the sink."),
                note(360.0, 470.0, Color::Pink, "nRF5340 kit pairing", "Kits connect and pair. No end-to-end LC3 audio yet."),
            ],
        },
        Board {
            name: "life".to_string(),
            notes: vec![
                note(150.0, 140.0, Color::Red, "Job search", "Android > Kotlin > backend. Ship visible pieces."),
                note(520.0, 180.0, Color::Green, "zalary 2026", "Update tax tables from SKV 433. Figures are year-keyed."),
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
    fn save_then_load_round_trips() {
        let mut store = MemoryStore::empty();
        let boards = vec![Board::new("scratch")];
        store.save(&boards).unwrap();
        assert_eq!(store.load().unwrap(), boards);
    }
}
