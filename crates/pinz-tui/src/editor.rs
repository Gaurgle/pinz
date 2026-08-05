//! A minimal multi-line text editor for the document-level note body.
//!
//! Hand-rolled rather than pulling `tui-textarea`, which still targets
//! ratatui 0.29 and would drag a second, incompatible ratatui into the tree.
//! Scope is deliberately small - insert, delete, newline, cursor movement -
//! enough to write a note, not a code editor. Char-indexed throughout so it
//! stays correct for non-ASCII input.

/// A cursor position, in (row, column) of characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
}

/// A cursor movement. Bundling these into one enum lets [`TextEditor::step`] be
/// the single door movement goes through, so a motion cannot move the caret and
/// forget to update the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    LeftWord,
    RightWord,
}

/// An editable buffer of text lines with a single cursor and an optional
/// selection.
#[derive(Debug, Clone)]
pub struct TextEditor {
    /// Never empty: an empty buffer is one empty line.
    lines: Vec<String>,
    row: usize,
    /// Character index within the current line (0..=char_len).
    col: usize,
    /// The fixed end of a selection. The cursor is the moving end, so the pair
    /// is in no particular order - [`Self::selection`] sorts them.
    anchor: Option<Cursor>,
}

impl TextEditor {
    /// Load `text`, placing the cursor at the very end (where you'd resume
    /// writing).
    pub fn new(text: &str) -> Self {
        let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        let row = lines.len() - 1;
        let col = char_len(&lines[row]);
        Self { lines, row, col, anchor: None }
    }

    /// The buffer as a single string, lines joined by `\n`.
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn cursor(&self) -> Cursor {
        Cursor { row: self.row, col: self.col }
    }

    // ---- selection ----

    /// The selection as `(start, end)` in document order, or `None` when
    /// nothing is selected. An anchor sitting on the cursor is not a selection.
    pub fn selection(&self) -> Option<(Cursor, Cursor)> {
        let anchor = self.anchor?;
        let cursor = self.cursor();
        if anchor == cursor {
            return None;
        }
        Some(if (anchor.row, anchor.col) < (cursor.row, cursor.col) {
            (anchor, cursor)
        } else {
            (cursor, anchor)
        })
    }

    /// The selected text, lines joined by `\n`.
    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection()?;
        if start.row == end.row {
            return Some(slice(&self.lines[start.row], start.col, end.col));
        }
        let mut out = slice(&self.lines[start.row], start.col, usize::MAX);
        for line in &self.lines[start.row + 1..end.row] {
            out.push('\n');
            out.push_str(line);
        }
        out.push('\n');
        out.push_str(&slice(&self.lines[end.row], 0, end.col));
        Some(out)
    }

    /// Select the whole buffer.
    pub fn select_all(&mut self) {
        self.anchor = Some(Cursor { row: 0, col: 0 });
        self.row = self.lines.len() - 1;
        self.col = char_len(&self.lines[self.row]);
    }

    /// Remove the selected range, leaving the cursor where it started. Returns
    /// whether anything was removed, so callers can fall through to their
    /// single-character behaviour when there was no selection.
    pub fn delete_selection(&mut self) -> bool {
        let Some((start, end)) = self.selection() else {
            return false;
        };
        if start.row == end.row {
            let from = byte_at(&self.lines[start.row], start.col);
            let to = byte_at(&self.lines[start.row], end.col);
            self.lines[start.row].replace_range(from..to, "");
        } else {
            // Keep what is left of the last line, staple it to what is left of
            // the first, and drop everything in between.
            let cut = byte_at(&self.lines[end.row], end.col);
            let tail = self.lines[end.row].split_off(cut);
            let keep = byte_at(&self.lines[start.row], start.col);
            self.lines[start.row].truncate(keep);
            self.lines[start.row].push_str(&tail);
            self.lines.drain(start.row + 1..=end.row);
        }
        self.row = start.row;
        self.col = start.col;
        self.anchor = None;
        true
    }

    /// Move the cursor. `extend` grows the selection from the existing anchor
    /// (dropping one there if the selection is new); without it the selection
    /// collapses, as an unmodified arrow key should.
    pub fn step(&mut self, m: Motion, extend: bool) {
        self.reanchor(extend);
        match m {
            Motion::Left => self.left(),
            Motion::Right => self.right(),
            Motion::Up => self.up(),
            Motion::Down => self.down(),
            Motion::Home => self.home(),
            Motion::End => self.end(),
            Motion::LeftWord => self.left_word(),
            Motion::RightWord => self.right_word(),
        }
    }

    /// Put the cursor at an arbitrary position, clamped into the buffer. For the
    /// mouse, which can point anywhere.
    pub fn set_cursor(&mut self, at: Cursor, extend: bool) {
        self.reanchor(extend);
        self.row = at.row.min(self.lines.len() - 1);
        self.col = at.col.min(char_len(&self.lines[self.row]));
    }

    /// Leave the anchor where it is when extending, drop one at the cursor when
    /// starting a fresh selection, and clear it when not extending at all.
    fn reanchor(&mut self, extend: bool) {
        let here = self.cursor();
        if extend {
            self.anchor.get_or_insert(here);
        } else {
            self.anchor = None;
        }
    }

    // ---- editing ----

    /// Insert text that may span lines, replacing any selection. Carriage
    /// returns are dropped so a CRLF paste does not litter the note with them.
    pub fn insert_str(&mut self, s: &str) {
        self.delete_selection();
        for (i, part) in s.replace('\r', "").split('\n').enumerate() {
            if i > 0 {
                self.insert_newline();
            }
            for c in part.chars() {
                self.insert_char(c);
            }
        }
    }

    pub fn insert_char(&mut self, c: char) {
        self.delete_selection();
        let at = byte_at(&self.lines[self.row], self.col);
        self.lines[self.row].insert(at, c);
        self.col += 1;
    }

    pub fn insert_newline(&mut self) {
        self.delete_selection();
        let at = byte_at(&self.lines[self.row], self.col);
        let tail = self.lines[self.row].split_off(at);
        self.lines.insert(self.row + 1, tail);
        self.row += 1;
        self.col = 0;
    }

    /// Delete the character before the cursor, joining lines at column 0. With
    /// a selection this removes exactly the selection and no extra character.
    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.col > 0 {
            let start = byte_at(&self.lines[self.row], self.col - 1);
            let end = byte_at(&self.lines[self.row], self.col);
            self.lines[self.row].replace_range(start..end, "");
            self.col -= 1;
        } else if self.row > 0 {
            let current = self.lines.remove(self.row);
            self.row -= 1;
            self.col = char_len(&self.lines[self.row]);
            self.lines[self.row].push_str(&current);
        }
    }

    /// Delete the character at the cursor, pulling up the next line at line end.
    /// With a selection this removes exactly the selection.
    pub fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.col < char_len(&self.lines[self.row]) {
            let start = byte_at(&self.lines[self.row], self.col);
            let end = byte_at(&self.lines[self.row], self.col + 1);
            self.lines[self.row].replace_range(start..end, "");
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].push_str(&next);
        }
    }

    // ---- movement ----

    fn left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = char_len(&self.lines[self.row]);
        }
    }

    fn right(&mut self) {
        if self.col < char_len(&self.lines[self.row]) {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    fn up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            self.col = self.col.min(char_len(&self.lines[self.row]));
        }
    }

    fn down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = self.col.min(char_len(&self.lines[self.row]));
        }
    }

    fn home(&mut self) {
        self.col = 0;
    }

    fn end(&mut self) {
        self.col = char_len(&self.lines[self.row]);
    }

    /// Column where the word before the cursor starts: back over any run of
    /// spaces, then over the word itself. Shared by [`Self::left_word`] and
    /// [`Self::delete_word`] so moving and deleting always agree on where a word
    /// begins.
    fn prev_word_col(&self) -> usize {
        let chars: Vec<char> = self.lines[self.row].chars().collect();
        let mut i = self.col;
        while i > 0 && chars[i - 1] == ' ' {
            i -= 1;
        }
        while i > 0 && chars[i - 1] != ' ' {
            i -= 1;
        }
        i
    }

    /// Jump to the start of the word before the cursor. At column 0 this steps
    /// to the end of the previous line, like a plain left.
    fn left_word(&mut self) {
        if self.col == 0 {
            self.left();
            return;
        }
        self.col = self.prev_word_col();
    }

    /// Jump past the word after the cursor. At the end of a line this steps to
    /// the start of the next one, like a plain right.
    fn right_word(&mut self) {
        let chars: Vec<char> = self.lines[self.row].chars().collect();
        if self.col >= chars.len() {
            self.right();
            return;
        }
        let mut i = self.col;
        while i < chars.len() && chars[i] == ' ' {
            i += 1;
        }
        while i < chars.len() && chars[i] != ' ' {
            i += 1;
        }
        self.col = i;
    }

    /// Delete the word before the cursor: any run of spaces, then the word
    /// itself. At column 0 this falls back to a plain backspace (merging lines).
    /// A selection takes precedence and is removed instead.
    pub fn delete_word(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.col == 0 {
            self.backspace();
            return;
        }
        let i = self.prev_word_col();
        let start = byte_at(&self.lines[self.row], i);
        let end = byte_at(&self.lines[self.row], self.col);
        self.lines[self.row].replace_range(start..end, "");
        self.col = i;
    }

    /// Clear the current line's text, leaving an empty line with the cursor at
    /// its start. A selection takes precedence and is removed instead.
    pub fn kill_line(&mut self) {
        if self.delete_selection() {
            return;
        }
        self.lines[self.row].clear();
        self.col = 0;
    }
}

/// The characters of `s` from `from` up to `to`, both character indices. `to`
/// past the end means "to the end of the line".
fn slice(s: &str, from: usize, to: usize) -> String {
    s[byte_at(s, from)..byte_at(s, to)].to_string()
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// Byte offset of the `col`-th character, or the string's length if `col` is at
/// or past the end. Keeps edits on UTF-8 boundaries.
fn byte_at(s: &str, col: usize) -> usize {
    s.char_indices().nth(col).map(|(i, _)| i).unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_places_cursor_at_the_end() {
        let e = TextEditor::new("hi\nthere");
        assert_eq!(e.cursor(), Cursor { row: 1, col: 5 });
        assert_eq!(e.text(), "hi\nthere");
    }

    #[test]
    fn empty_input_is_one_empty_line() {
        let e = TextEditor::new("");
        assert_eq!(e.lines(), &[String::new()]);
        assert_eq!(e.cursor(), Cursor { row: 0, col: 0 });
    }

    #[test]
    fn newline_splits_and_backspace_merges() {
        let mut e = TextEditor::new("abcd");
        e.left();
        e.left(); // between b and c
        e.insert_newline();
        assert_eq!(e.text(), "ab\ncd");
        assert_eq!(e.cursor(), Cursor { row: 1, col: 0 });
        e.backspace(); // merge the two lines back
        assert_eq!(e.text(), "abcd");
        assert_eq!(e.cursor(), Cursor { row: 0, col: 2 });
    }

    #[test]
    fn insert_and_delete_within_a_line() {
        let mut e = TextEditor::new("ac");
        e.left(); // between a and c
        e.insert_char('b');
        assert_eq!(e.text(), "abc");
        e.left(); // before b
        e.delete(); // removes b
        assert_eq!(e.text(), "ac");
    }

    #[test]
    fn delete_at_line_end_pulls_up_next_line() {
        let mut e = TextEditor::new("ab\ncd");
        e.up();
        e.end(); // end of "ab"
        e.delete();
        assert_eq!(e.text(), "abcd");
    }

    #[test]
    fn movement_clamps_at_edges() {
        let mut e = TextEditor::new("hi\nthere");
        e.up(); // row 0, col clamped to len("hi") == 2
        assert_eq!(e.cursor(), Cursor { row: 0, col: 2 });
        e.up(); // already at top, stays
        assert_eq!(e.cursor().row, 0);
        e.home();
        e.left(); // at (0,0), nowhere to go
        assert_eq!(e.cursor(), Cursor { row: 0, col: 0 });
    }

    #[test]
    fn handles_non_ascii() {
        let mut e = TextEditor::new("åä");
        e.insert_char('ö');
        assert_eq!(e.text(), "åäö");
        e.backspace();
        e.backspace();
        assert_eq!(e.text(), "å");
    }

    #[test]
    fn delete_word_removes_a_word_and_its_leading_spaces() {
        let mut e = TextEditor::new("foo bar baz");
        e.delete_word(); // cursor at end -> "baz"
        assert_eq!(e.text(), "foo bar ");
        e.delete_word(); // trailing space then "bar"
        assert_eq!(e.text(), "foo ");
    }

    #[test]
    fn delete_word_at_column_zero_merges_with_previous_line() {
        let mut e = TextEditor::new("ab\ncd");
        e.home(); // column 0 of "cd"
        e.delete_word();
        assert_eq!(e.text(), "abcd");
    }

    #[test]
    fn word_movement_steps_over_words_and_across_lines() {
        let mut e = TextEditor::new("foo bar baz");
        e.left_word();
        assert_eq!(e.cursor(), Cursor { row: 0, col: 8 }, "start of \"baz\"");
        e.left_word();
        assert_eq!(e.cursor(), Cursor { row: 0, col: 4 }, "start of \"bar\"");
        e.right_word();
        assert_eq!(e.cursor(), Cursor { row: 0, col: 7 }, "end of \"bar\"");

        // At an edge, word movement degrades to a plain step across the break.
        let mut e = TextEditor::new("ab\ncd");
        e.home();
        e.left_word();
        assert_eq!(e.cursor(), Cursor { row: 0, col: 2 });
        e.right_word();
        assert_eq!(e.cursor(), Cursor { row: 1, col: 0 });
    }

    #[test]
    fn kill_line_clears_the_current_line() {
        let mut e = TextEditor::new("hello");
        e.kill_line();
        assert_eq!(e.text(), "");
        assert_eq!(e.cursor(), Cursor { row: 0, col: 0 });
    }

    // ---- selection ----

    #[test]
    fn a_fresh_editor_has_no_selection() {
        let e = TextEditor::new("hello");
        assert_eq!(e.selection(), None);
        assert_eq!(e.selected_text(), None);
    }

    #[test]
    fn moving_without_extend_selects_nothing() {
        let mut e = TextEditor::new("hello");
        e.step(Motion::Left, false);
        e.step(Motion::Left, false);
        assert_eq!(e.selection(), None);
        assert_eq!(e.cursor(), Cursor { row: 0, col: 3 });
    }

    #[test]
    fn extending_left_selects_back_from_the_anchor() {
        let mut e = TextEditor::new("hello"); // cursor at the end
        e.step(Motion::Left, true);
        e.step(Motion::Left, true);
        assert_eq!(e.selected_text().as_deref(), Some("lo"));
    }

    #[test]
    fn selection_is_ordered_even_when_the_anchor_is_after_the_cursor() {
        let mut e = TextEditor::new("hello");
        e.step(Motion::Left, true); // anchor 5, cursor 4
        let (start, end) = e.selection().unwrap();
        assert_eq!(start, Cursor { row: 0, col: 4 });
        assert_eq!(end, Cursor { row: 0, col: 5 });
    }

    #[test]
    fn a_selection_spans_lines() {
        let mut e = TextEditor::new("ab\ncd"); // cursor at (1, 2)
        e.step(Motion::Up, true); // (0, 2)
        assert_eq!(e.selected_text().as_deref(), Some("\ncd"));
        e.step(Motion::Home, true); // (0, 0)
        assert_eq!(e.selected_text().as_deref(), Some("ab\ncd"));
    }

    #[test]
    fn moving_without_extend_collapses_an_existing_selection() {
        let mut e = TextEditor::new("hello");
        e.step(Motion::Left, true);
        e.step(Motion::Left, true);
        assert!(e.selection().is_some());
        e.step(Motion::Left, false);
        assert_eq!(e.selection(), None);
    }

    #[test]
    fn an_anchor_on_the_cursor_is_not_a_selection() {
        let mut e = TextEditor::new("hello");
        e.step(Motion::Left, true);
        e.step(Motion::Right, true); // back where it started
        assert_eq!(e.selection(), None);
    }

    #[test]
    fn select_all_covers_the_whole_buffer() {
        let mut e = TextEditor::new("ab\ncd");
        e.select_all();
        assert_eq!(e.selected_text().as_deref(), Some("ab\ncd"));
    }

    #[test]
    fn delete_selection_removes_the_range_and_reports_it() {
        let mut e = TextEditor::new("hello");
        e.step(Motion::Left, true);
        e.step(Motion::Left, true);
        assert!(e.delete_selection());
        assert_eq!(e.text(), "hel");
        assert_eq!(e.cursor(), Cursor { row: 0, col: 3 });
        assert_eq!(e.selection(), None);
        assert!(!e.delete_selection(), "nothing selected, nothing removed");
        assert_eq!(e.text(), "hel");
    }

    #[test]
    fn delete_selection_joins_the_lines_it_spanned() {
        let mut e = TextEditor::new("ab\ncd");
        e.set_cursor(Cursor { row: 1, col: 1 }, false); // between c and d
        e.step(Motion::Up, true); // anchor (1,1), cursor (0,1)
        assert_eq!(e.selected_text().as_deref(), Some("b\nc"));
        assert!(e.delete_selection());
        assert_eq!(e.text(), "ad", "the head of line 0 keeps the tail of line 1");
        assert_eq!(e.cursor(), Cursor { row: 0, col: 1 });
    }

    #[test]
    fn typing_replaces_the_selection() {
        let mut e = TextEditor::new("hello");
        e.step(Motion::Left, true);
        e.step(Motion::Left, true);
        e.insert_char('p');
        assert_eq!(e.text(), "help");
        assert_eq!(e.selection(), None);
    }

    #[test]
    fn backspace_over_a_selection_removes_it_and_nothing_more() {
        let mut e = TextEditor::new("hello");
        e.step(Motion::Left, true);
        e.step(Motion::Left, true);
        e.backspace();
        assert_eq!(e.text(), "hel", "the l before the selection survives");
    }

    #[test]
    fn delete_over_a_selection_removes_it_and_nothing_more() {
        let mut e = TextEditor::new("hello");
        e.home();
        e.step(Motion::Right, true);
        e.step(Motion::Right, true);
        e.delete();
        assert_eq!(e.text(), "llo");
    }

    #[test]
    fn newline_replaces_the_selection() {
        let mut e = TextEditor::new("hello");
        e.step(Motion::Left, true);
        e.step(Motion::Left, true);
        e.insert_newline();
        assert_eq!(e.text(), "hel\n");
    }

    #[test]
    fn word_motions_extend_too() {
        let mut e = TextEditor::new("foo bar baz");
        e.step(Motion::LeftWord, true);
        assert_eq!(e.selected_text().as_deref(), Some("baz"));
    }

    #[test]
    fn set_cursor_clamps_a_position_past_the_buffer() {
        let mut e = TextEditor::new("ab\ncd");
        e.set_cursor(Cursor { row: 99, col: 99 }, false);
        assert_eq!(e.cursor(), Cursor { row: 1, col: 2 });
        e.set_cursor(Cursor { row: 0, col: 99 }, false);
        assert_eq!(e.cursor(), Cursor { row: 0, col: 2 });
    }

    #[test]
    fn set_cursor_with_extend_selects_from_where_it_was() {
        let mut e = TextEditor::new("hello");
        e.set_cursor(Cursor { row: 0, col: 1 }, false);
        e.set_cursor(Cursor { row: 0, col: 4 }, true);
        assert_eq!(e.selected_text().as_deref(), Some("ell"));
    }

    #[test]
    fn insert_str_splits_on_newlines_and_drops_carriage_returns() {
        let mut e = TextEditor::new("");
        e.insert_str("one\r\ntwo");
        assert_eq!(e.text(), "one\ntwo");
        assert_eq!(e.cursor(), Cursor { row: 1, col: 3 });
    }

    #[test]
    fn insert_str_replaces_a_selection() {
        let mut e = TextEditor::new("hello");
        e.select_all();
        e.insert_str("bye");
        assert_eq!(e.text(), "bye");
    }

    #[test]
    fn selection_counts_characters_not_bytes() {
        let mut e = TextEditor::new("åäö");
        e.step(Motion::Left, true);
        e.step(Motion::Left, true);
        assert_eq!(e.selected_text().as_deref(), Some("äö"));
        e.delete_selection();
        assert_eq!(e.text(), "å");
    }
}
