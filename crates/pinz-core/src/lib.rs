//! `pinz-core`: the UI-agnostic heart of pinz, a spatial bulletin board of
//! post-it notes.
//!
//! This crate holds the domain model, the world/screen projection, and the
//! storage seam - and nothing about how any of it is drawn. Renderers
//! (`pinz-tui`, and later an Epoz Svelte tab) depend on this crate; it depends
//! on none of them. Keeping that arrow pointing one way is what lets a terminal
//! app and a desktop app share one brain. See `design/DESIGN.md`.

pub mod geometry;
pub mod model;
pub mod store;

pub use geometry::{Camera, Projection, ScreenPoint, WorldPoint, ZoomLevel};
pub use model::{Board, Color, Note, NOTE_H, NOTE_W};
pub use store::{MemoryStore, Store, StoreError};
