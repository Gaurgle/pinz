//! Word wrap for the note editor, and the map between logical and visual
//! positions that goes with it.
//!
//! The map has to go both ways. Rendering turns a cursor and a selection into
//! screen cells; the mouse turns a screen cell back into a cursor. Both run the
//! *same* wrap here, for the reason the tab strip is laid out in `app.rs`: if
//! hit-testing and drawing computed the layout separately they would eventually
//! disagree, and the note you click would stop being the text you see.

use crate::editor::Cursor;

/// One visual row: its text, which logical line it came from, and the character
/// index within that line where it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub text: String,
    pub line: usize,
    pub start: usize,
}

/// Logical lines laid out at a width.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wrapped {
    pub rows: Vec<Row>,
}

/// Greedy word wrap: break after spaces, hard-break words longer than the width.
///
/// Spaces are kept at the end of a wrapped row so every character maps to
/// exactly one visual cell. That one-to-one property is what makes [`Wrapped::place`]
/// and [`Wrapped::locate`] exact rather than approximate.
pub fn wrap(logical: &[String], width: usize) -> Wrapped {
    let width = width.max(1);
    let mut rows: Vec<Row> = Vec::new();

    for (line, text) in logical.iter().enumerate() {
        let chars: Vec<char> = text.chars().collect();
        if chars.is_empty() {
            rows.push(Row { text: String::new(), line, start: 0 });
            continue;
        }
        let mut start = 0;
        while start < chars.len() {
            let hard_end = (start + width).min(chars.len());
            let end = if hard_end == chars.len() {
                chars.len()
            } else {
                // Break after the last space that fits; a word wider than the
                // row has nowhere to break, so cut it.
                match chars[start..hard_end].iter().rposition(|&c| c == ' ') {
                    Some(rel) if rel > 0 => start + rel + 1,
                    _ => hard_end,
                }
            };
            rows.push(Row { text: chars[start..end].iter().collect(), line, start });
            start = end;
        }
    }

    // A caret needs somewhere to sit even when there is nothing to show.
    if rows.is_empty() {
        rows.push(Row { text: String::new(), line: 0, start: 0 });
    }
    Wrapped { rows }
}

impl Wrapped {
    /// Where a logical cursor lands on screen, as `(visual row, column)`.
    ///
    /// A cursor exactly on a wrap boundary belongs to the *start of the next
    /// row*, not the end of the previous one - otherwise the caret would sit in
    /// the phantom column past the right edge.
    pub fn place(&self, c: Cursor) -> (usize, usize) {
        let mut last = None;
        for (i, row) in self.rows.iter().enumerate() {
            if row.line != c.row {
                continue;
            }
            let len = row.text.chars().count();
            if c.col < row.start + len {
                return (i, c.col - row.start);
            }
            last = Some((i, row.start, len));
        }
        match last {
            // Past the end of every row for this line: the line's last row.
            Some((i, start, len)) => (i, c.col.min(start + len) - start),
            // A line we never laid out. Clamp to the very end of the text.
            None => {
                let i = self.rows.len() - 1;
                (i, self.rows[i].text.chars().count())
            }
        }
    }

    /// Where a screen cell points in the buffer. Clamps, so a click past the end
    /// of a row lands at that row's end and a click below the text lands at the
    /// end of the last row.
    pub fn locate(&self, vrow: usize, vcol: usize) -> Cursor {
        let row = &self.rows[vrow.min(self.rows.len() - 1)];
        Cursor { row: row.line, col: row.start + vcol.min(row.text.chars().count()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_keeps_all_characters_and_bounds_the_width() {
        let lines = vec!["hello world foo".to_string()];
        let w = wrap(&lines, 8);
        let joined: String = w.rows.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(joined, "hello world foo", "no characters lost to wrapping");
        assert!(w.rows.iter().all(|r| r.text.chars().count() <= 8));
    }

    #[test]
    fn wrap_labels_title_vs_body_rows_and_keeps_blanks() {
        let lines = vec!["title".to_string(), String::new(), "body".to_string()];
        let w = wrap(&lines, 20);
        assert_eq!(w.rows.len(), 3, "blank line is preserved");
        assert_eq!(w.rows[0].line, 0, "row 0 is the title line");
        assert_eq!(w.rows[2].line, 2);
    }

    #[test]
    fn wrap_hard_breaks_a_word_longer_than_the_width() {
        let lines = vec!["antidisestablishmentarianism".to_string()];
        let w = wrap(&lines, 10);
        assert_eq!(w.rows[0].text, "antidisest");
        assert_eq!(w.rows[1].start, 10);
    }

    #[test]
    fn wrap_counts_characters_not_bytes() {
        let lines = vec!["ååååå ööööö".to_string()];
        let w = wrap(&lines, 6);
        assert!(w.rows.iter().all(|r| r.text.chars().count() <= 6));
        let joined: String = w.rows.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(joined, "ååååå ööööö");
    }

    #[test]
    fn place_maps_a_cursor_inside_the_first_row() {
        let lines = vec!["hello world foo".to_string()];
        let w = wrap(&lines, 8);
        assert_eq!(w.place(Cursor { row: 0, col: 3 }), (0, 3));
    }

    #[test]
    fn place_tracks_a_cursor_at_the_very_end() {
        let lines = vec!["hello world foo".to_string()];
        let w = wrap(&lines, 8);
        assert_eq!(w.place(Cursor { row: 0, col: 15 }), (2, 3));
    }

    #[test]
    fn place_finds_a_cursor_on_a_later_logical_line() {
        let lines = vec!["title".to_string(), String::new(), "body".to_string()];
        let w = wrap(&lines, 20);
        assert_eq!(w.place(Cursor { row: 2, col: 4 }), (2, 4));
        assert_eq!(w.place(Cursor { row: 1, col: 0 }), (1, 0), "the blank line");
    }

    #[test]
    fn place_at_a_wrap_boundary_lands_on_the_start_of_the_next_row() {
        let lines = vec!["hello world foo".to_string()];
        let w = wrap(&lines, 8);
        // "hello " is row 0, so column 6 is the first character of row 1.
        assert_eq!(w.place(Cursor { row: 0, col: 6 }), (1, 0));
    }

    #[test]
    fn locate_inverts_place() {
        let lines = vec!["hello world foo".to_string(), "second line".to_string()];
        let w = wrap(&lines, 8);
        for (row, line) in lines.iter().enumerate() {
            for col in 0..=line.chars().count() {
                let c = Cursor { row, col };
                let (vr, vc) = w.place(c);
                assert_eq!(w.locate(vr, vc), c, "round trip failed for {c:?}");
            }
        }
    }

    #[test]
    fn locate_clamps_a_click_past_the_end_of_a_row() {
        let lines = vec!["ab".to_string()];
        let w = wrap(&lines, 8);
        assert_eq!(w.locate(0, 99), Cursor { row: 0, col: 2 });
    }

    #[test]
    fn locate_clamps_a_click_below_the_last_row() {
        let lines = vec!["ab".to_string(), "cd".to_string()];
        let w = wrap(&lines, 8);
        assert_eq!(w.locate(99, 0), Cursor { row: 1, col: 0 });
    }

    #[test]
    fn a_zero_width_does_not_panic_or_lose_the_row() {
        let w = wrap(&["ab".to_string()], 0);
        assert!(!w.rows.is_empty());
        assert_eq!(w.locate(0, 0), Cursor { row: 0, col: 0 });
    }

    #[test]
    fn empty_input_still_has_one_row_to_put_a_caret_on() {
        let w = wrap(&[], 8);
        assert_eq!(w.rows.len(), 1);
        assert_eq!(w.place(Cursor { row: 0, col: 0 }), (0, 0));
    }
}
