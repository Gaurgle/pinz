//! The palette. pinz wears Catppuccin Mocha, same as the design demo.
//!
//! Kept in one place so every widget pulls the same hexes and a [`Color`] maps
//! to exactly one terminal color. `pinz-core` names note colors abstractly (a
//! closed enum); turning those names into RGB is a renderer concern, so it lives
//! here rather than in the core.

use pinz_core::Color as NoteColor;
use ratatui::style::Color;

// Base / surfaces / text - the neutral scaffolding.
pub const CRUST: Color = Color::Rgb(0x11, 0x11, 0x1b);
pub const MANTLE: Color = Color::Rgb(0x18, 0x18, 0x25);
pub const BASE: Color = Color::Rgb(0x1e, 0x1e, 0x2e);
pub const SURFACE0: Color = Color::Rgb(0x31, 0x32, 0x44);
pub const SURFACE1: Color = Color::Rgb(0x45, 0x47, 0x5a);
pub const OVERLAY0: Color = Color::Rgb(0x6c, 0x70, 0x86);
pub const OVERLAY1: Color = Color::Rgb(0x7f, 0x84, 0x9c);
pub const TEXT: Color = Color::Rgb(0xcd, 0xd6, 0xf4);
pub const SUBTEXT: Color = Color::Rgb(0xa6, 0xad, 0xc8);
pub const LAVENDER: Color = Color::Rgb(0xb4, 0xbe, 0xfe);

/// The eight note accents, in the same order as [`NoteColor::ALL`].
pub fn note_color(c: NoteColor) -> Color {
    match c {
        NoteColor::Yellow => Color::Rgb(0xf9, 0xe2, 0xaf),
        NoteColor::Green => Color::Rgb(0xa6, 0xe3, 0xa1),
        NoteColor::Blue => Color::Rgb(0x89, 0xb4, 0xfa),
        NoteColor::Pink => Color::Rgb(0xf5, 0xc2, 0xe7),
        NoteColor::Peach => Color::Rgb(0xfa, 0xb3, 0x87),
        NoteColor::Mauve => Color::Rgb(0xcb, 0xa6, 0xf7),
        NoteColor::Teal => Color::Rgb(0x94, 0xe2, 0xd5),
        NoteColor::Red => Color::Rgb(0xf3, 0x8b, 0xa8),
    }
}
