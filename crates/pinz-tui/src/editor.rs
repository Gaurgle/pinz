//! A minimal text editor: some text plus a cursor, and the operations a
//! cell-based editor needs. UI-agnostic and single-purpose - it knows nothing
//! about ratatui or notes - so it is easy to test and the renderer only has to
//! ask it where the caret sits.
//!
//! Text is kept as a `Vec<String>`, one entry per line, and the cursor is a
//! `(row, col)` pair where `col` counts **characters**, not bytes - so a
//! multi-byte glyph is one step, and nothing ever lands mid-codepoint.
//! A single-line editor (a note title) simply refuses newlines.

/// Character count of a string. Cheap for the short strings a note holds.
fn chars(s: &str) -> usize {
    s.chars().count()
}

/// Byte offset of the `col`-th character (or the string's end).
fn byte_of(s: &str, col: usize) -> usize {
    s.char_indices().nth(col).map(|(b, _)| b).unwrap_or(s.len())
}

#[derive(Debug, Clone)]
pub struct TextEditor {
    lines: Vec<String>,
    row: usize,
    col: usize,
    multiline: bool,
}

impl TextEditor {
    /// Load `text`, cursor at the very end (natural for picking up where the
    /// note left off). A single-line editor flattens any newlines to spaces.
    pub fn new(text: &str, multiline: bool) -> Self {
        let lines: Vec<String> = if multiline {
            if text.is_empty() {
                vec![String::new()]
            } else {
                text.split('\n').map(str::to_string).collect()
            }
        } else {
            vec![text.replace('\n', " ")]
        };
        let row = lines.len() - 1;
        let col = chars(&lines[row]);
        Self {
            lines,
            row,
            col,
            multiline,
        }
    }

    pub fn single_line(text: &str) -> Self {
        Self::new(text, false)
    }

    pub fn multi_line(text: &str) -> Self {
        Self::new(text, true)
    }

    /// The full text, lines rejoined with `\n`.
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// The cursor as `(row, col)` in characters. Part of the editor's API and
    /// exercised by the tests; the renderer currently shows the caret via
    /// [`TextEditor::with_caret`] instead of positioning by coordinate.
    #[allow(dead_code)]
    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    fn line(&self) -> &str {
        &self.lines[self.row]
    }

    fn last_row(&self) -> usize {
        self.lines.len() - 1
    }

    // ---- edits ----

    pub fn insert(&mut self, c: char) {
        // A single-line field takes any char but never a line break.
        if c == '\n' {
            if self.multiline {
                self.newline();
            }
            return;
        }
        let b = byte_of(&self.lines[self.row], self.col);
        self.lines[self.row].insert(b, c);
        self.col += 1;
    }

    pub fn newline(&mut self) {
        if !self.multiline {
            return;
        }
        let b = byte_of(&self.lines[self.row], self.col);
        let tail = self.lines[self.row].split_off(b);
        self.lines.insert(self.row + 1, tail);
        self.row += 1;
        self.col = 0;
    }

    pub fn backspace(&mut self) {
        if self.col > 0 {
            let b0 = byte_of(&self.lines[self.row], self.col - 1);
            let b1 = byte_of(&self.lines[self.row], self.col);
            self.lines[self.row].replace_range(b0..b1, "");
            self.col -= 1;
        } else if self.row > 0 {
            // Join this line onto the end of the previous one.
            let cur = self.lines.remove(self.row);
            self.row -= 1;
            self.col = chars(&self.lines[self.row]);
            self.lines[self.row].push_str(&cur);
        }
    }

    pub fn delete(&mut self) {
        let len = chars(self.line());
        if self.col < len {
            let b0 = byte_of(&self.lines[self.row], self.col);
            let b1 = byte_of(&self.lines[self.row], self.col + 1);
            self.lines[self.row].replace_range(b0..b1, "");
        } else if self.row < self.last_row() {
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
            self.col = chars(self.line());
        }
    }

    pub fn right(&mut self) {
        if self.col < chars(self.line()) {
            self.col += 1;
        } else if self.row < self.last_row() {
            self.row += 1;
            self.col = 0;
        }
    }

    pub fn up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            self.col = self.col.min(chars(self.line()));
        }
    }

    pub fn down(&mut self) {
        if self.row < self.last_row() {
            self.row += 1;
            self.col = self.col.min(chars(self.line()));
        }
    }

    pub fn home(&mut self) {
        self.col = 0;
    }

    pub fn end(&mut self) {
        self.col = chars(self.line());
    }

    /// The text with `caret` spliced in at the cursor, ready to render. The
    /// caret rides in the character flow, so it stays put through wrapping.
    pub fn with_caret(&self, caret: &str) -> String {
        let mut lines = self.lines.clone();
        let b = byte_of(&lines[self.row], self.col);
        lines[self.row].insert_str(b, caret);
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_puts_cursor_at_the_end() {
        let e = TextEditor::single_line("hello");
        assert_eq!(e.cursor(), (0, 5));
        assert_eq!(e.text(), "hello");
    }

    #[test]
    fn insert_happens_at_the_cursor_not_the_end() {
        let mut e = TextEditor::single_line("ac");
        e.left(); // between a and c
        e.insert('b');
        assert_eq!(e.text(), "abc");
        assert_eq!(e.cursor(), (0, 2));
    }

    #[test]
    fn single_line_editor_refuses_newlines() {
        let mut e = TextEditor::single_line("ab");
        e.newline();
        e.insert('\n');
        assert_eq!(e.text(), "ab");
        assert_eq!(e.cursor(), (0, 2));
    }

    #[test]
    fn newline_splits_the_current_line() {
        let mut e = TextEditor::multi_line("hello world");
        for _ in 0.."world".len() {
            e.left();
        }
        e.newline();
        assert_eq!(e.text(), "hello \nworld");
        assert_eq!(e.cursor(), (1, 0));
    }

    #[test]
    fn backspace_at_line_start_joins_the_previous_line() {
        let mut e = TextEditor::multi_line("ab\ncd");
        // cursor at end of "cd"; walk to the start of line 2
        e.home();
        assert_eq!(e.cursor(), (1, 0));
        e.backspace();
        assert_eq!(e.text(), "abcd");
        assert_eq!(e.cursor(), (0, 2));
    }

    #[test]
    fn delete_at_line_end_pulls_up_the_next_line() {
        let mut e = TextEditor::multi_line("ab\ncd");
        e.up(); // row 0, col clamped to 2 (end of "ab")
        assert_eq!(e.cursor(), (0, 2));
        e.delete();
        assert_eq!(e.text(), "abcd");
    }

    #[test]
    fn vertical_moves_clamp_the_column() {
        let mut e = TextEditor::multi_line("longline\nhi");
        // cursor ends on row 1 col 2; go up -> col stays 2 (fits "longline")
        e.up();
        assert_eq!(e.cursor(), (0, 2));
        e.end(); // col 8
        e.down(); // row 1, col clamped to 2
        assert_eq!(e.cursor(), (1, 2));
    }

    #[test]
    fn editing_is_char_accurate_across_utf8() {
        let mut e = TextEditor::single_line("café");
        assert_eq!(e.cursor(), (0, 4));
        e.backspace(); // removes é, not a stray byte
        assert_eq!(e.text(), "caf");
        e.left();
        e.insert('t'); // between a and f
        assert_eq!(e.text(), "catf");
    }

    #[test]
    fn caret_is_spliced_at_the_cursor() {
        let mut e = TextEditor::single_line("ab");
        e.left();
        assert_eq!(e.with_caret("|"), "a|b");
    }

    #[test]
    fn home_and_end_jump_within_the_line() {
        let mut e = TextEditor::multi_line("one\ntwo");
        e.home();
        assert_eq!(e.cursor(), (1, 0));
        e.end();
        assert_eq!(e.cursor(), (1, 3));
    }
}
