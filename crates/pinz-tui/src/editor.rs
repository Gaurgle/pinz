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

/// An editable buffer of text lines with a single cursor.
#[derive(Debug, Clone)]
pub struct TextEditor {
    /// Never empty: an empty buffer is one empty line.
    lines: Vec<String>,
    row: usize,
    /// Character index within the current line (0..=char_len).
    col: usize,
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
        Self { lines, row, col }
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

    // ---- editing ----

    pub fn insert_char(&mut self, c: char) {
        let at = byte_at(&self.lines[self.row], self.col);
        self.lines[self.row].insert(at, c);
        self.col += 1;
    }

    pub fn insert_newline(&mut self) {
        let at = byte_at(&self.lines[self.row], self.col);
        let tail = self.lines[self.row].split_off(at);
        self.lines.insert(self.row + 1, tail);
        self.row += 1;
        self.col = 0;
    }

    /// Delete the character before the cursor, joining lines at column 0.
    pub fn backspace(&mut self) {
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
    pub fn delete(&mut self) {
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

    pub fn left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = char_len(&self.lines[self.row]);
        }
    }

    pub fn right(&mut self) {
        if self.col < char_len(&self.lines[self.row]) {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    pub fn up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            self.col = self.col.min(char_len(&self.lines[self.row]));
        }
    }

    pub fn down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = self.col.min(char_len(&self.lines[self.row]));
        }
    }

    pub fn home(&mut self) {
        self.col = 0;
    }

    pub fn end(&mut self) {
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
    pub fn left_word(&mut self) {
        if self.col == 0 {
            self.left();
            return;
        }
        self.col = self.prev_word_col();
    }

    /// Jump past the word after the cursor. At the end of a line this steps to
    /// the start of the next one, like a plain right.
    pub fn right_word(&mut self) {
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
    pub fn delete_word(&mut self) {
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
    /// its start.
    pub fn kill_line(&mut self) {
        self.lines[self.row].clear();
        self.col = 0;
    }
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
}
