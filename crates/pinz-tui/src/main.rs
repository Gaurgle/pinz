//! `pinz` terminal app - stub.
//!
//! Ratatui is not wired in yet; that is a deliberate later step. For now this
//! proves the wiring: it loads boards through the [`Store`] seam and prints a
//! summary, plus one sample projection, so `pinz-core` has a real consumer and
//! we can see the pieces fit before drawing a single cell.

use pinz_core::{Camera, MemoryStore, Projection, Store, WorldPoint, ZoomLevel};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut store = MemoryStore::seeded();
    let boards = store.load()?;

    println!("pinz - {} board(s)\n", boards.len());
    for board in &boards {
        println!("  [{}]  {} note(s)", board.name, board.notes.len());
        for note in &board.notes {
            println!("     - ({:>4.0},{:>4.0})  {}", note.x, note.y, note.title);
        }
        println!();
    }

    // Sanity-check the projection seam: camera at the origin, fully zoomed in,
    // terminal cell aspect. A note origin at world (200,150) should land
    // 200 cells right and 75 cells down (150 * 0.5 aspect).
    let projection = Projection::new(
        Camera { origin: WorldPoint { x: 0.0, y: 0.0 }, zoom: ZoomLevel::Document },
        0.5,
    );
    let world = WorldPoint { x: 200.0, y: 150.0 };
    let screen = projection.to_screen(world);
    println!(
        "projection check @ {}: world ({},{}) -> screen ({:.0},{:.0}) cells",
        projection.camera.zoom.label(),
        world.x,
        world.y,
        screen.x,
        screen.y,
    );

    Ok(())
}
