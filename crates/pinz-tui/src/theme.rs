//! Themes. A [`Theme`] is the full set of colors the renderer needs; the app
//! holds one active theme and can swap it at runtime (or pick one at launch).
//!
//! `pinz-core` names note colors abstractly (a closed enum); each theme decides
//! what those eight names actually look like, alongside the neutral scaffolding
//! (backgrounds, text, one accent for selection/highlights). Turning names into
//! RGB is a renderer concern, so it all lives here rather than in the core.

use pinz_core::Color as NoteColor;
use ratatui::style::Color;

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

/// A complete palette. Every field the UI reads comes from here, so switching
/// themes is just switching which `Theme` the app points at.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub name: &'static str,
    /// Board background.
    pub base: Color,
    /// Header / tabs / footer background.
    pub mantle: Color,
    /// Grid-dot color, and the faint "off" state generally.
    pub surface0: Color,
    /// Inactive zoom dots.
    pub surface1: Color,
    /// Scrollbars, note counts - low-emphasis marks.
    pub overlay0: Color,
    /// Secondary text (brand tagline, keybind hints).
    pub overlay1: Color,
    /// Primary text.
    pub text: Color,
    /// Inactive tab labels.
    pub subtext: Color,
    /// The one highlight color: selection border, active tab, filled zoom dots.
    pub accent: Color,
    /// Text drawn *on* a note's colored background.
    pub note_fg: Color,
    /// The eight note accents, in [`NoteColor::ALL`] order.
    notes: [Color; 8],
}

impl Theme {
    /// The RGB for one of the core's abstract note colors, in this theme.
    pub fn note(&self, c: NoteColor) -> Color {
        self.notes[note_index(c)]
    }
}

fn note_index(c: NoteColor) -> usize {
    match c {
        NoteColor::Yellow => 0,
        NoteColor::Green => 1,
        NoteColor::Blue => 2,
        NoteColor::Pink => 3,
        NoteColor::Peach => 4,
        NoteColor::Mauve => 5,
        NoteColor::Teal => 6,
        NoteColor::Red => 7,
    }
}

/// Every theme, in cycle order. The first is the default.
pub const THEMES: &[Theme] = &[MOCHA, TOKYO_NIGHT, GRUVBOX, NORD, SOLARIZED_LIGHT];

/// Resolve a user-typed theme name to an index. Loose on purpose: a substring
/// match, case-insensitive, so `nord`, `tokyo`, or `catppuccin` all land.
pub fn index_by_name(query: &str) -> Option<usize> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return None;
    }
    THEMES
        .iter()
        .position(|t| t.name.to_lowercase().contains(&q))
}

/// Catppuccin Mocha - the demo's look. Soft pastels on a deep indigo.
const MOCHA: Theme = Theme {
    name: "Catppuccin Mocha",
    base: rgb(0x1e, 0x1e, 0x2e),
    mantle: rgb(0x18, 0x18, 0x25),
    surface0: rgb(0x31, 0x32, 0x44),
    surface1: rgb(0x45, 0x47, 0x5a),
    overlay0: rgb(0x6c, 0x70, 0x86),
    overlay1: rgb(0x7f, 0x84, 0x9c),
    text: rgb(0xcd, 0xd6, 0xf4),
    subtext: rgb(0xa6, 0xad, 0xc8),
    accent: rgb(0xb4, 0xbe, 0xfe),
    note_fg: rgb(0x11, 0x11, 0x1b),
    notes: [
        rgb(0xf9, 0xe2, 0xaf), // yellow
        rgb(0xa6, 0xe3, 0xa1), // green
        rgb(0x89, 0xb4, 0xfa), // blue
        rgb(0xf5, 0xc2, 0xe7), // pink
        rgb(0xfa, 0xb3, 0x87), // peach
        rgb(0xcb, 0xa6, 0xf7), // mauve
        rgb(0x94, 0xe2, 0xd5), // teal
        rgb(0xf3, 0x8b, 0xa8), // red
    ],
};

/// Tokyo Night - cool, saturated, a touch neon.
const TOKYO_NIGHT: Theme = Theme {
    name: "Tokyo Night",
    base: rgb(0x1f, 0x23, 0x35),
    mantle: rgb(0x1a, 0x1b, 0x26),
    surface0: rgb(0x29, 0x2e, 0x42),
    surface1: rgb(0x41, 0x48, 0x68),
    overlay0: rgb(0x56, 0x5f, 0x89),
    overlay1: rgb(0x78, 0x7c, 0x99),
    text: rgb(0xc0, 0xca, 0xf5),
    subtext: rgb(0xa9, 0xb1, 0xd6),
    accent: rgb(0x7a, 0xa2, 0xf7),
    note_fg: rgb(0x16, 0x16, 0x1e),
    notes: [
        rgb(0xe0, 0xaf, 0x68), // yellow
        rgb(0x9e, 0xce, 0x6a), // green
        rgb(0x7a, 0xa2, 0xf7), // blue
        rgb(0xff, 0x75, 0xa0), // pink
        rgb(0xff, 0x9e, 0x64), // peach
        rgb(0xbb, 0x9a, 0xf7), // mauve
        rgb(0x7d, 0xcf, 0xff), // teal
        rgb(0xf7, 0x76, 0x8e), // red
    ],
};

/// Gruvbox (dark) - warm, earthy, retro.
const GRUVBOX: Theme = Theme {
    name: "Gruvbox",
    base: rgb(0x32, 0x30, 0x2f),
    mantle: rgb(0x28, 0x28, 0x28),
    surface0: rgb(0x3c, 0x38, 0x36),
    surface1: rgb(0x50, 0x49, 0x45),
    overlay0: rgb(0x66, 0x5c, 0x54),
    overlay1: rgb(0xa8, 0x99, 0x84),
    text: rgb(0xeb, 0xdb, 0xb2),
    subtext: rgb(0xd5, 0xc4, 0xa1),
    accent: rgb(0x83, 0xa5, 0x98),
    note_fg: rgb(0x1d, 0x20, 0x21),
    notes: [
        rgb(0xfa, 0xbd, 0x2f), // yellow
        rgb(0xb8, 0xbb, 0x26), // green
        rgb(0x83, 0xa5, 0x98), // blue
        rgb(0xd3, 0x86, 0x9b), // pink
        rgb(0xfe, 0x80, 0x19), // peach
        rgb(0xb1, 0x62, 0x86), // mauve
        rgb(0x8e, 0xc0, 0x7c), // teal
        rgb(0xfb, 0x49, 0x34), // red
    ],
};

/// Nord - a cold, muted arctic palette.
const NORD: Theme = Theme {
    name: "Nord",
    base: rgb(0x3b, 0x42, 0x52),
    mantle: rgb(0x2e, 0x34, 0x40),
    surface0: rgb(0x43, 0x4c, 0x5e),
    surface1: rgb(0x4c, 0x56, 0x6a),
    overlay0: rgb(0x61, 0x6e, 0x88),
    overlay1: rgb(0x8b, 0x98, 0xb5),
    text: rgb(0xec, 0xef, 0xf4),
    subtext: rgb(0xd8, 0xde, 0xe9),
    accent: rgb(0x88, 0xc0, 0xd0),
    note_fg: rgb(0x2e, 0x34, 0x40),
    notes: [
        rgb(0xeb, 0xcb, 0x8b), // yellow
        rgb(0xa3, 0xbe, 0x8c), // green
        rgb(0x81, 0xa1, 0xc1), // blue
        rgb(0xd3, 0xa0, 0xc9), // pink
        rgb(0xd0, 0x87, 0x70), // peach
        rgb(0xb4, 0x8e, 0xad), // mauve
        rgb(0x8f, 0xbc, 0xbb), // teal
        rgb(0xbf, 0x61, 0x6a), // red
    ],
};

/// Solarized Light - the odd one out: a warm-paper light theme, to prove the
/// renderer isn't wired to assume a dark background.
const SOLARIZED_LIGHT: Theme = Theme {
    name: "Solarized Light",
    base: rgb(0xfd, 0xf6, 0xe3),
    mantle: rgb(0xee, 0xe8, 0xd5),
    surface0: rgb(0xd6, 0xcf, 0xbd),
    surface1: rgb(0xcc, 0xc4, 0xb0),
    overlay0: rgb(0x93, 0xa1, 0xa1),
    overlay1: rgb(0x65, 0x7b, 0x83),
    text: rgb(0x58, 0x6e, 0x75),
    subtext: rgb(0x65, 0x7b, 0x83),
    accent: rgb(0x6c, 0x71, 0xc4),
    note_fg: rgb(0xfd, 0xf6, 0xe3),
    notes: [
        rgb(0xb5, 0x89, 0x00), // yellow
        rgb(0x85, 0x99, 0x00), // green
        rgb(0x26, 0x8b, 0xd2), // blue
        rgb(0xd3, 0x36, 0x82), // pink
        rgb(0xcb, 0x4b, 0x16), // peach
        rgb(0x6c, 0x71, 0xc4), // mauve
        rgb(0x2a, 0xa1, 0x98), // teal
        rgb(0xdc, 0x32, 0x2f), // red
    ],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_theme_maps_all_eight_note_colors() {
        for theme in THEMES {
            for c in NoteColor::ALL {
                let _ = theme.note(c); // index in range for all eight
            }
        }
    }

    #[test]
    fn name_lookup_is_loose_and_case_insensitive() {
        assert_eq!(index_by_name("nord"), Some(3));
        assert_eq!(index_by_name("TOKYO"), Some(1));
        assert_eq!(index_by_name("catppuccin"), Some(0));
        assert_eq!(index_by_name("light"), Some(4));
        assert_eq!(index_by_name("chartreuse"), None);
        assert_eq!(index_by_name(""), None);
    }
}
