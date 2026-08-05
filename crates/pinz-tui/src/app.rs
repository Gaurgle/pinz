//! Application state and the input -> state transitions that drive it.
//!
//! Kept free of drawing so the logic is testable without a terminal: `ui.rs`
//! reads this state to render, and feeds crossterm events back in through
//! [`App::on_key`] / [`App::on_mouse`]. Every spatial operation - pan, zoom,
//! drag, hit-test - goes through [`View`], the projection spine, so what you
//! click is exactly what the math says is under the cursor.

use pinz_core::{Board, Camera, Color, Note, WorldPoint, ZoomLevel, NOTE_H, NOTE_W};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;

use crate::editor::{Cursor, Motion, TextEditor};
use crate::theme::{self, Theme};
use crate::view::{CellRect, View};
use crate::wrap::{self, Wrapped};
use std::collections::VecDeque;

/// Arrow-key pan step, in cells.
const PAN_CELLS: f64 = 4.0;
/// How far past the note cloud you may pan before hitting the soft wall, in
/// world units. Enough to breathe; not enough to lose the board.
const PAN_MARGIN: f64 = 80.0;
/// Width of the `+` tab, drawn as " + ".
const NEW_TAB_WIDTH: u16 = 3;
/// Longest world name we will take. A world is a directory, so this is about
/// keeping paths sane rather than anything deeper.
const BOARD_NAME_MAX: usize = 40;
/// How many board states undo remembers. A snapshot is the whole workspace,
/// which for a corkboard of text notes is tens of kilobytes - less than the
/// frame pinz already redraws on every keystroke - so this can be generous.
const UNDO_DEPTH: usize = 50;

/// What the keyboard is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Navigating the board.
    Nav,
    /// Editing the selected note as one buffer: line 1 is the title, the rest
    /// is the body.
    Edit,
    /// Answering a prompt. Not everything should be one keystroke away - naming
    /// a world is worth a moment's thought and an escape hatch.
    Prompt,
}

/// One cell of the world tab strip. The app owns the layout so that clicking a
/// tab and drawing it cannot drift apart: `ui` renders exactly these spans, and
/// [`App::on_mouse`] hit-tests the same ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    pub kind: TabKind,
    /// Column offset from the left of the tab strip, and how wide it is.
    pub x: u16,
    pub width: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabKind {
    World {
        index: usize,
        name: String,
        notes: usize,
        active: bool,
    },
    /// The `+` that opens the new-world prompt.
    New,
}

/// A question the app is waiting on an answer to.
#[derive(Debug, Clone)]
pub struct Prompt {
    pub title: &'static str,
    pub hint: &'static str,
    /// What has been typed so far.
    pub input: String,
    /// Set when the last attempt to confirm was refused, so the reason can be
    /// shown without throwing away what was typed.
    pub error: Option<String>,
}

/// The cursor movement a key means while editing, if it means one at all.
///
/// Terminals disagree about what Option/Alt + arrow sends: some report
/// Alt+Left, others the readline escapes Alt-b / Alt-f. Both spellings are
/// accepted, and Ctrl+arrow with them.
///
/// SUPER is Cmd on macOS, where Cmd + arrow is line-wise movement. Most
/// terminals claim that chord before it reaches an application - it is bound
/// here so it works in the ones that can be told to forward it, not because it
/// can be relied on.
fn motion_for(code: KeyCode, ctrl: bool, alt: bool, sup: bool) -> Option<Motion> {
    Some(match code {
        KeyCode::Left if ctrl || alt => Motion::LeftWord,
        KeyCode::Right if ctrl || alt => Motion::RightWord,
        KeyCode::Left if sup => Motion::Home,
        KeyCode::Right if sup => Motion::End,
        KeyCode::Char('b' | 'B') if alt => Motion::LeftWord,
        KeyCode::Char('f' | 'F') if alt => Motion::RightWord,
        KeyCode::Left => Motion::Left,
        KeyCode::Right => Motion::Right,
        KeyCode::Up => Motion::Up,
        KeyCode::Down => Motion::Down,
        KeyCode::Home => Motion::Home,
        KeyCode::End => Motion::End,
        _ => return None,
    })
}

/// Board state as undo remembers it.
///
/// A whole-workspace copy rather than an inverse operation per action. The
/// store already saves whole workspaces, so this needs no new machinery and has
/// no per-action way to be wrong; the cost is a clone of some text.
///
/// `active` and `selected` ride along so undoing something that happened on
/// another world puts you back where it happened, rather than leaving you
/// staring at a board that did not change.
#[derive(Debug, Clone)]
struct Snapshot {
    boards: Vec<Board>,
    active: usize,
    selected: Option<u64>,
}

/// A drag in progress, started on mouse-down.
#[derive(Debug, Clone, Copy)]
enum Drag {
    /// Moving a note: its id plus the grab point's offset from the note's
    /// top-left, in world units, so the note tracks the cursor without jumping.
    Note { id: u64, off_x: f64, off_y: f64 },
    /// Panning the board: the anchor cell and the camera origin at grab time.
    Pan {
        col: u16,
        row: u16,
        origin: WorldPoint,
    },
    /// Sweeping a text selection inside the note being edited. The anchor lives
    /// in the editor, so there is nothing to carry here.
    Text,
}

pub struct App {
    boards: Vec<Board>,
    active: usize,
    camera: Camera,
    /// Currently selected note (by id), if any.
    selected: Option<u64>,
    mode: Mode,
    /// The live note editor, present only while in [`Mode::Edit`].
    editor: Option<TextEditor>,
    drag: Option<Drag>,
    /// The board viewport from the last render, needed to interpret mouse
    /// positions and to center content. Zero until the first draw.
    viewport: Rect,
    /// The tab strip from the last render, so a click can land on a world.
    tabs: Rect,
    /// The open prompt, if any. Present exactly when the mode is [`Mode::Prompt`].
    prompt: Option<Prompt>,
    /// Have we centered the current board on screen yet?
    centered: bool,
    next_id: u64,
    color_tick: usize,
    /// Index into [`theme::THEMES`] of the active theme.
    theme_index: usize,
    /// Bumped by every change to note content or placement. The runner watches
    /// it to know when the board is worth writing to disk, so a crash costs at
    /// most the pin you were mid-drag on.
    revision: u64,
    /// Text waiting to go to the system clipboard. The app never writes to the
    /// terminal itself - the runner drains this and does the I/O - which keeps
    /// this module a pure state machine, testable with no terminal attached.
    pending_copy: Option<String>,
    /// A one-off message for the footer, cleared by the next event. A copy is
    /// otherwise completely invisible.
    status: Option<String>,
    /// Board states to go back to, oldest first. Capped at [`UNDO_DEPTH`].
    undo: VecDeque<Snapshot>,
    /// States undone past, newest last. Cleared by any fresh change.
    redo: Vec<Snapshot>,
    /// The state as it was before the event being handled. Held across a whole
    /// drag so a gesture becomes one undo step rather than one per mouse-move.
    pending: Option<Snapshot>,
    should_quit: bool,
}

impl App {
    pub fn new(boards: Vec<Board>) -> Self {
        let next_id = boards
            .iter()
            .flat_map(|b| b.notes.iter())
            .map(|n| n.id)
            .max()
            .unwrap_or(0)
            + 1;
        App {
            boards,
            active: 0,
            camera: Camera {
                origin: WorldPoint { x: 0.0, y: 0.0 },
                // Start at titles: enough notes on screen to read the board's
                // shape, close enough to know what each one is.
                zoom: ZoomLevel::Titles,
            },
            selected: None,
            mode: Mode::Nav,
            editor: None,
            drag: None,
            viewport: Rect::default(),
            tabs: Rect::default(),
            prompt: None,
            centered: false,
            next_id,
            color_tick: 0,
            theme_index: 0,
            revision: 0,
            pending_copy: None,
            status: None,
            undo: VecDeque::new(),
            redo: Vec::new(),
            pending: None,
            should_quit: false,
        }
    }

    // ---- read access for the renderer ----

    pub fn boards(&self) -> &[Board] {
        &self.boards
    }
    /// Index of the active world. The tab strip carries its own `active` flag,
    /// so this is here for tests and for symmetry with [`App::active_board`].
    #[allow(dead_code)]
    pub fn active_index(&self) -> usize {
        self.active
    }
    pub fn active_board(&self) -> &Board {
        &self.boards[self.active]
    }
    pub fn camera(&self) -> Camera {
        self.camera
    }
    pub fn zoom(&self) -> ZoomLevel {
        self.camera.zoom
    }
    pub fn selected(&self) -> Option<u64> {
        self.selected
    }
    pub fn mode(&self) -> Mode {
        self.mode
    }
    /// The live note editor, for the renderer to draw with a cursor. `Some`
    /// only while editing.
    pub fn editor(&self) -> Option<&TextEditor> {
        self.editor.as_ref()
    }
    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    /// A one-off message for the footer, if the last event produced one.
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    /// Take the text waiting to go to the clipboard, if any. The runner calls
    /// this after each event; a copy is delivered exactly once.
    pub fn take_pending_copy(&mut self) -> Option<String> {
        self.pending_copy.take()
    }

    /// Say something in the footer. For the runner, which finds out whether a
    /// copy actually reached the terminal after the app has stopped looking.
    pub fn set_status(&mut self, message: String) {
        self.status = Some(message);
    }

    /// Counter of changes to the boards. Compare it across events to know
    /// whether anything worth persisting happened.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Whether a drag is in flight. Saving mid-drag would write a file per
    /// mouse-move; the runner waits for the gesture to finish.
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The open prompt, for the renderer to draw. `Some` only in [`Mode::Prompt`].
    pub fn prompt(&self) -> Option<&Prompt> {
        self.prompt.as_ref()
    }

    /// Record where the tab strip was drawn, so clicks on it can be resolved.
    pub fn set_tabs_area(&mut self, area: Rect) {
        self.tabs = area;
    }

    /// The tab strip: one span per world, then the `+`. Widths here are what the
    /// renderer must draw, so hit-testing stays exact.
    pub fn tabs(&self) -> Vec<Tab> {
        let mut out = Vec::with_capacity(self.boards.len() + 1);
        let mut x = 1; // the strip opens with a space
        for (index, board) in self.boards.iter().enumerate() {
            // marker + "name " + "count  "
            let width = 1 + board.name.chars().count() as u16 + 1 + digits(board.notes.len()) + 2;
            out.push(Tab {
                kind: TabKind::World {
                    index,
                    name: board.name.clone(),
                    notes: board.notes.len(),
                    active: index == self.active,
                },
                x,
                width,
            });
            x += width;
        }
        out.push(Tab {
            kind: TabKind::New,
            x,
            width: NEW_TAB_WIDTH,
        });
        out
    }

    /// The active theme (a small `Copy` palette).
    pub fn theme(&self) -> Theme {
        theme::THEMES[self.theme_index]
    }

    /// Select a theme by (loose, case-insensitive) name; ignored if no match.
    /// Used at launch to honor a `--theme` argument. Returns whether it stuck.
    pub fn set_theme_by_name(&mut self, name: &str) -> bool {
        match theme::index_by_name(name) {
            Some(i) => {
                self.theme_index = i;
                true
            }
            None => false,
        }
    }

    fn cycle_theme(&mut self, forward: bool) {
        let n = theme::THEMES.len();
        self.theme_index = if forward {
            (self.theme_index + 1) % n
        } else {
            (self.theme_index + n - 1) % n
        };
    }

    /// Step the selected note through the note palette. No-op without a
    /// selection.
    fn cycle_note_color(&mut self, forward: bool) {
        let Some(id) = self.selected else { return };
        let palette = Color::ALL;
        let n = palette.len();
        if let Some(note) = self.note_mut(id) {
            let cur = palette.iter().position(|&c| c == note.color).unwrap_or(0);
            note.color = if forward {
                palette[(cur + 1) % n]
            } else {
                palette[(cur + n - 1) % n]
            };
        }
    }

    /// Record the viewport the board was last drawn into, and center the board
    /// the first time we know how big it is.
    pub fn set_viewport(&mut self, area: Rect) {
        self.viewport = area;
        if !self.centered && area.width > 0 && area.height > 0 {
            self.center_on_content();
            self.centered = true;
        }
    }

    fn view(&self) -> View {
        View::new(self.camera, self.viewport)
    }

    fn active_board_mut(&mut self) -> &mut Board {
        &mut self.boards[self.active]
    }

    /// Mutable access to a note. Every caller here is about to change the note,
    /// so this is the one place that has to record it.
    fn note_mut(&mut self, id: u64) -> Option<&mut Note> {
        self.revision += 1;
        self.active_board_mut()
            .notes
            .iter_mut()
            .find(|n| n.id == id)
    }

    // ---- keyboard ----

    pub fn on_key(&mut self, key: KeyEvent) {
        self.begin_step();
        self.key(key);
        self.end_step();
    }

    fn key(&mut self, key: KeyEvent) {
        // Whatever the last event had to say, this one supersedes it.
        self.status = None;
        if self.copy_chord(key) {
            return;
        }
        match self.mode {
            Mode::Edit => return self.edit_key(key),
            Mode::Prompt => return self.prompt_key(key),
            Mode::Nav => {}
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('r') if ctrl => self.redo_step(),
            KeyCode::Char('u') => self.undo_step(),
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc => self.selected = None,
            KeyCode::Char('+') | KeyCode::Char('=') => self.zoom_at_center(true),
            KeyCode::Char('-') | KeyCode::Char('_') => self.zoom_at_center(false),
            KeyCode::Char('n') => self.new_note(),
            KeyCode::Char('d') | KeyCode::Char('x') | KeyCode::Delete => self.delete_selected(),
            KeyCode::Char('e') | KeyCode::Enter => self.begin_edit(),
            KeyCode::Char('t') => self.cycle_theme(true),
            KeyCode::Char('T') => self.cycle_theme(false),
            KeyCode::Char('c') => self.cycle_note_color(true),
            KeyCode::Char('C') => self.cycle_note_color(false),
            KeyCode::Char('w') => self.begin_new_world(),
            KeyCode::Char('y') => self.yank_note(),
            KeyCode::Tab => self.switch_world(self.active + 1),
            KeyCode::BackTab => self.switch_world(self.active + self.boards.len() - 1),
            KeyCode::Char(c @ '1'..='9') => self.switch_world((c as usize) - ('1' as usize)),
            KeyCode::Left => self.pan_cells(-PAN_CELLS, 0.0),
            KeyCode::Right => self.pan_cells(PAN_CELLS, 0.0),
            KeyCode::Up => self.pan_cells(0.0, -PAN_CELLS),
            KeyCode::Down => self.pan_cells(0.0, PAN_CELLS),
            _ => {}
        }
    }

    // ---- undo ----

    /// Stash the state this event is about to change.
    ///
    /// Done unconditionally rather than at each mutation site: a `checkpoint()`
    /// call per action is one forgotten call away from a silent hole in the
    /// history. Most events change nothing and the stash is dropped again in
    /// [`Self::end_step`].
    fn begin_step(&mut self) {
        if self.pending.is_none() {
            self.pending = Some(self.snapshot());
        }
    }

    /// Commit the stash as an undo step, once the gesture it belongs to is over
    /// and only if the boards actually differ.
    ///
    /// Comparing the boards rather than watching `revision` is deliberate.
    /// `revision` is bumped by `note_mut` on *access*, because it cannot know
    /// whether the caller will change anything - which is the right trade for
    /// deciding when to save, but would record an undo step for opening a note
    /// and closing it untouched.
    ///
    /// Holding the stash across a drag is what makes a whole drag one step
    /// instead of one per mouse-move, the same predicate the runner uses to
    /// avoid saving mid-gesture. Typing collapses the same way, because a note
    /// is only written back on `commit_edit`.
    fn end_step(&mut self) {
        if self.is_dragging() {
            return; // still gathering; keep the pre-gesture stash
        }
        let Some(snap) = self.pending.take() else {
            return;
        };
        if snap.boards == self.boards {
            return;
        }
        if self.undo.len() == UNDO_DEPTH {
            self.undo.pop_front();
        }
        self.undo.push_back(snap);
        // A fresh change makes the undone future unreachable.
        self.redo.clear();
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            boards: self.boards.clone(),
            active: self.active,
            selected: self.selected,
        }
    }

    /// Put a snapshot back, clamping the active world in case the board list
    /// shrank since it was taken.
    fn restore(&mut self, snap: Snapshot) {
        self.boards = snap.boards;
        self.active = snap.active.min(self.boards.len().saturating_sub(1));
        self.selected = snap.selected;
        self.revision += 1;
        self.clamp_origin();
    }

    fn undo_step(&mut self) {
        let Some(snap) = self.undo.pop_back() else {
            self.status = Some("nothing to undo".into());
            return;
        };
        let current = self.snapshot();
        self.restore(snap);
        self.redo.push(current);
        // Clearing the stash is what stops an undo from becoming an undo step.
        self.pending = None;
    }

    fn redo_step(&mut self) {
        let Some(snap) = self.redo.pop() else {
            self.status = Some("nothing to redo".into());
            return;
        };
        let current = self.snapshot();
        self.restore(snap);
        if self.undo.len() == UNDO_DEPTH {
            self.undo.pop_front();
        }
        self.undo.push_back(current);
        self.pending = None;
    }

    // ---- copy ----

    /// Handle the copy and cut chords, which have to be resolved before the
    /// per-mode dispatch because Ctrl-C means two different things.
    ///
    /// Ctrl-C copies when there is a selection to copy and quits otherwise, so
    /// it keeps working as the escape hatch everywhere except the one moment
    /// you obviously meant to copy. SUPER+C (Cmd-C on macOS) only ever copies -
    /// it is never an escape hatch, and most terminals swallow it before it
    /// reaches us anyway.
    ///
    /// Returns whether the key was consumed.
    fn copy_chord(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let sup = key.modifiers.contains(KeyModifiers::SUPER);
        if !ctrl && !sup {
            return false;
        }
        let cut = match key.code {
            KeyCode::Char('c' | 'C') => false,
            KeyCode::Char('x' | 'X') => true,
            _ => return false,
        };
        if self.mode == Mode::Edit && self.has_selection() {
            self.copy_selection(cut);
            return true;
        }
        // Nothing to copy. Ctrl-C falls back to its old job; Cmd-C has none.
        if ctrl && !cut {
            self.should_quit = true;
        }
        true
    }

    fn has_selection(&self) -> bool {
        self.editor
            .as_ref()
            .is_some_and(|e| e.selection().is_some())
    }

    /// Copy the editor's selection, removing it too when `cut`.
    fn copy_selection(&mut self, cut: bool) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        let Some(text) = editor.selected_text() else {
            return;
        };
        if cut {
            editor.delete_selection();
        }
        self.set_copied(text);
    }

    /// Copy the selected note whole: the title, then the body under it.
    fn yank_note(&mut self) {
        let Some(id) = self.selected else { return };
        let Some(note) = self.active_board().notes.iter().find(|n| n.id == id) else {
            return;
        };
        let text = if note.body.is_empty() {
            note.title.clone()
        } else {
            format!("{}\n{}", note.title, note.body)
        };
        self.set_copied(text);
    }

    /// Queue text for the clipboard and say so in the footer.
    fn set_copied(&mut self, text: String) {
        self.status = Some(format!("copied {} chars", text.chars().count()));
        self.pending_copy = Some(text);
    }

    /// Text arriving as one lump from a bracketed paste. Never key-by-key, so a
    /// pasted newline cannot be mistaken for Enter.
    pub fn on_paste(&mut self, text: String) {
        self.begin_step();
        self.paste(text);
        self.end_step();
    }

    fn paste(&mut self, text: String) {
        self.status = None;
        match self.mode {
            Mode::Edit => {
                if let Some(editor) = self.editor.as_mut() {
                    editor.insert_str(&text);
                }
            }
            Mode::Prompt => {
                let Some(prompt) = self.prompt.as_mut() else { return };
                // A world name is a directory name: one line, bounded.
                for c in text.lines().next().unwrap_or_default().chars() {
                    if prompt.input.chars().count() >= BOARD_NAME_MAX {
                        break;
                    }
                    prompt.input.push(c);
                }
                prompt.error = None;
            }
            Mode::Nav => {}
        }
    }

    /// Open the selected note in the editor as one buffer: line 1 is the title,
    /// the rest the body. Bumps zoom to document so there's room to write and
    /// see the cursor. No-op without a selection.
    fn begin_edit(&mut self) {
        let Some(id) = self.selected else { return };
        let Some(note) = self.active_board().notes.iter().find(|n| n.id == id) else {
            return;
        };
        let text = if note.body.is_empty() {
            note.title.clone()
        } else {
            format!("{}\n{}", note.title, note.body)
        };
        self.editor = Some(TextEditor::new(&text));
        self.mode = Mode::Edit;
        self.camera.zoom = ZoomLevel::Document;
        self.clamp_origin();
    }

    fn edit_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc {
            self.commit_edit();
            return;
        }
        let Some(editor) = self.editor.as_mut() else {
            self.mode = Mode::Nav;
            return;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let sup = key.modifiers.contains(KeyModifiers::SUPER);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        // Movement first, because every motion is also a selection gesture:
        // holding shift extends instead of collapsing. Routing them all through
        // `step` is what keeps the two from drifting apart.
        if let Some(motion) = motion_for(key.code, ctrl, alt, sup) {
            editor.step(motion, shift);
            return;
        }

        match key.code {
            // Word / line delete. Ctrl or Alt + Backspace (and Ctrl-W) kill the
            // word before the cursor; Ctrl-U clears the current line.
            KeyCode::Backspace if ctrl || alt => editor.delete_word(),
            KeyCode::Char('w') if ctrl => editor.delete_word(),
            KeyCode::Char('u') if ctrl => editor.kill_line(),
            KeyCode::Char('a' | 'A') if ctrl || sup => editor.select_all(),
            KeyCode::Enter => editor.insert_newline(),
            KeyCode::Backspace => editor.backspace(),
            KeyCode::Delete => editor.delete(),
            // A modified key that got this far is a chord we don't bind, never
            // text. Without this guard an unbound Alt-<letter> - which is how a
            // terminal spells Option+arrow - would type its letter into the note.
            KeyCode::Char(_) if ctrl || alt || sup => {}
            KeyCode::Char(c) => editor.insert_char(c),
            _ => {}
        }
    }

    /// Write the editor back to the note - first line is the title, the rest the
    /// body - and return to nav. Esc saves rather than discards: a whole note is
    /// too much to lose to a stray key.
    fn commit_edit(&mut self) {
        if let (Some(id), Some(editor)) = (self.selected, self.editor.take()) {
            // First line is the title, everything after the first newline is the
            // body (which may itself span lines).
            let text = editor.text();
            let (title, body) = match text.split_once('\n') {
                Some((title, body)) => (title.to_string(), body.to_string()),
                None => (text, String::new()),
            };
            if let Some(note) = self.note_mut(id) {
                note.title = title;
                note.body = body;
            }
        }
        self.editor = None;
        self.mode = Mode::Nav;
    }

    /// Open the prompt that names a new world.
    fn begin_new_world(&mut self) {
        self.prompt = Some(Prompt {
            title: "new world",
            hint: "enter to create · esc to cancel",
            input: String::new(),
            error: None,
        });
        self.mode = Mode::Prompt;
    }

    fn prompt_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let Some(prompt) = self.prompt.as_mut() else {
            self.mode = Mode::Nav;
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.prompt = None;
                self.mode = Mode::Nav;
            }
            KeyCode::Enter => self.confirm_prompt(),
            KeyCode::Backspace if ctrl || alt => {
                prompt.input.clear();
                prompt.error = None;
            }
            KeyCode::Char('u') if ctrl => {
                prompt.input.clear();
                prompt.error = None;
            }
            KeyCode::Backspace => {
                prompt.input.pop();
                prompt.error = None;
            }
            // As in the note editor, a modified key that got this far is a chord
            // we do not bind, never text.
            KeyCode::Char(_) if ctrl || alt => {}
            KeyCode::Char(c) => {
                if prompt.input.chars().count() < BOARD_NAME_MAX {
                    prompt.input.push(c);
                }
                prompt.error = None;
            }
            _ => {}
        }
    }

    /// Act on the prompt's answer. A refusal keeps the prompt open with the
    /// typed text intact and the reason shown - retyping a name because of a
    /// stray slash would be its own small insult.
    fn confirm_prompt(&mut self) {
        let Some(prompt) = self.prompt.as_mut() else { return };
        let name = prompt.input.trim().to_string();
        match validate_board_name(&name) {
            Err(why) => prompt.error = Some(why),
            Ok(()) => {
                // An existing world is switched to rather than duplicated: two
                // directories cannot share a name anyway.
                match self.boards.iter().position(|b| b.name == name) {
                    Some(index) => self.switch_world(index),
                    None => {
                        self.boards.push(Board::new(name));
                        self.revision += 1;
                        self.switch_world(self.boards.len() - 1);
                    }
                }
                self.prompt = None;
                self.mode = Mode::Nav;
            }
        }
    }

    // ---- mouse ----

    pub fn on_mouse(&mut self, m: MouseEvent) {
        self.begin_step();
        self.mouse(m);
        self.end_step();
    }

    fn mouse(&mut self, m: MouseEvent) {
        self.status = None;
        match m.kind {
            MouseEventKind::ScrollUp => self.zoom_at(true, m.column, m.row),
            MouseEventKind::ScrollDown => self.zoom_at(false, m.column, m.row),
            MouseEventKind::Down(MouseButton::Left) => self.mouse_down(m.column, m.row),
            MouseEventKind::Drag(MouseButton::Left) => self.mouse_drag(m.column, m.row),
            MouseEventKind::Up(MouseButton::Left) => self.drag = None,
            _ => {}
        }
    }

    fn in_viewport(&self, col: u16, row: u16) -> bool {
        self.viewport.contains((col, row).into())
    }

    fn mouse_down(&mut self, col: u16, row: u16) {
        if self.tabs.contains((col, row).into()) {
            self.click_tab(col);
            return;
        }
        if !self.in_viewport(col, row) {
            return;
        }
        // Editing ends the moment you touch the board - unless you touched the
        // note you are editing, where a press starts a text selection instead.
        if self.mode == Mode::Edit {
            if let Some(at) = self.edit_cursor_at(col, row, false) {
                if let Some(editor) = self.editor.as_mut() {
                    editor.set_cursor(at, false);
                }
                self.drag = Some(Drag::Text);
                return;
            }
            self.commit_edit();
        }
        let world = self.view().world_at(col, row);
        // Topmost note under the cursor wins, respecting stack order.
        if let Some(note) = self.active_board().note_at(world) {
            let id = note.id;
            let (off_x, off_y) = (world.x - note.x, world.y - note.y);
            self.selected = Some(id);
            self.bring_to_front(id);
            self.drag = Some(Drag::Note { id, off_x, off_y });
        } else {
            self.selected = None;
            self.drag = Some(Drag::Pan {
                col,
                row,
                origin: self.camera.origin,
            });
        }
    }

    /// Resolve a click on the tab strip: a world switches to it, the `+` opens
    /// the new-world prompt, and a gap does nothing.
    fn click_tab(&mut self, col: u16) {
        let Some(offset) = col.checked_sub(self.tabs.x) else {
            return;
        };
        let Some(tab) = self
            .tabs()
            .into_iter()
            .find(|t| offset >= t.x && offset < t.x + t.width)
        else {
            return;
        };
        // A click anywhere is also an answer of "not now" to an open prompt.
        if self.mode == Mode::Prompt {
            self.prompt = None;
            self.mode = Mode::Nav;
        }
        match tab.kind {
            TabKind::World { index, .. } => {
                if self.mode == Mode::Edit {
                    self.commit_edit();
                }
                self.switch_world(index);
            }
            TabKind::New => self.begin_new_world(),
        }
    }

    /// The edited note's full cell footprint and the wrap laying out its text.
    /// `None` unless a note is open in the editor.
    ///
    /// Computed here rather than handed back by the renderer, so `app` stays
    /// testable with no terminal: `ui` runs the identical wrap when it draws.
    fn edit_layout(&self) -> Option<(CellRect, Wrapped)> {
        let editor = self.editor.as_ref()?;
        let id = self.selected?;
        let note = self.active_board().notes.iter().find(|n| n.id == id)?;
        let cells = self.view().note_cells(note.position());
        // The text sits inside the note's one-cell border.
        let width = cells.width.checked_sub(2)?;
        Some((cells, wrap::wrap(editor.lines(), width as usize)))
    }

    /// Where in the buffer a screen cell points, for a click or drag inside the
    /// note being edited. `clamp` is what separates the two gestures: a click
    /// outside the text area is not a click on text at all, but a drag that
    /// wanders off the edge should keep selecting to the nearest cell.
    fn edit_cursor_at(&self, col: u16, row: u16, clamp: bool) -> Option<Cursor> {
        let (cells, wrapped) = self.edit_layout()?;
        let width = cells.width.checked_sub(2)? as i64;
        let height = cells.height.checked_sub(2)? as i64;
        if width <= 0 || height <= 0 {
            return None;
        }
        let dx = col as i64 - (cells.x + 1);
        let dy = row as i64 - (cells.y + 1);
        let (dx, dy) = if clamp {
            (dx.clamp(0, width - 1), dy.clamp(0, height - 1))
        } else {
            if dx < 0 || dy < 0 || dx >= width || dy >= height {
                return None;
            }
            (dx, dy)
        };
        Some(wrapped.locate(dy as usize, dx as usize))
    }

    fn mouse_drag(&mut self, col: u16, row: u16) {
        match self.drag {
            Some(Drag::Text) => {
                if let Some(at) = self.edit_cursor_at(col, row, true) {
                    if let Some(editor) = self.editor.as_mut() {
                        editor.set_cursor(at, true);
                    }
                }
            }
            Some(Drag::Note { id, off_x, off_y }) => {
                let world = self.view().world_at(col, row);
                if let Some(note) = self.note_mut(id) {
                    note.x = world.x - off_x;
                    note.y = world.y - off_y;
                }
            }
            Some(Drag::Pan {
                col: c0,
                row: r0,
                origin,
            }) => {
                let (sx, sy) = self.view().scale();
                let dx = (col as f64 - c0 as f64) / sx;
                let dy = (row as f64 - r0 as f64) / sy;
                self.camera.origin = WorldPoint {
                    x: origin.x - dx,
                    y: origin.y - dy,
                };
                self.clamp_origin();
            }
            None => {}
        }
    }

    fn bring_to_front(&mut self, id: u64) {
        let top = self
            .active_board()
            .notes
            .iter()
            .map(|n| n.z)
            .max()
            .unwrap_or(0);
        if let Some(note) = self.note_mut(id) {
            if note.z != top {
                note.z = top + 1;
            }
        }
    }

    // ---- zoom ----

    /// One zoom step toward or away, keeping the cell under `(col,row)` fixed -
    /// the same focal-point zoom the demo uses, so the board doesn't lurch.
    fn zoom_at(&mut self, zoom_in: bool, col: u16, row: u16) {
        let target = if zoom_in {
            self.camera.zoom.zoomed_in()
        } else {
            self.camera.zoom.zoomed_out()
        };
        if target == self.camera.zoom {
            return;
        }
        let anchor = self.view().world_at(col, row);
        self.camera.zoom = target;
        // Shift the origin so `anchor` lands back under the same cell.
        let now = self.view().world_at(col, row);
        self.camera.origin.x += anchor.x - now.x;
        self.camera.origin.y += anchor.y - now.y;
        self.clamp_origin();
    }

    fn zoom_at_center(&mut self, zoom_in: bool) {
        let (col, row) = self.viewport_center();
        self.zoom_at(zoom_in, col, row);
    }

    fn viewport_center(&self) -> (u16, u16) {
        (
            self.viewport.x + self.viewport.width / 2,
            self.viewport.y + self.viewport.height / 2,
        )
    }

    // ---- pan ----

    fn pan_cells(&mut self, dx: f64, dy: f64) {
        let (sx, sy) = self.view().scale();
        self.camera.origin.x += dx / sx;
        self.camera.origin.y += dy / sy;
        self.clamp_origin();
    }

    /// Soft-clamp the camera so the note cloud can't be panned entirely
    /// off-screen. With no notes, there's nothing to hold onto - leave it be.
    fn clamp_origin(&mut self) {
        let Some((min, max)) = self.content_bounds() else {
            return;
        };
        let (sx, sy) = self.view().scale();
        let view_w = self.viewport.width as f64 / sx;
        let view_h = self.viewport.height as f64 / sy;

        let lo_x = min.x - PAN_MARGIN;
        let hi_x = (max.x + PAN_MARGIN - view_w).max(lo_x);
        let lo_y = min.y - PAN_MARGIN;
        let hi_y = (max.y + PAN_MARGIN - view_h).max(lo_y);

        self.camera.origin.x = self.camera.origin.x.clamp(lo_x, hi_x);
        self.camera.origin.y = self.camera.origin.y.clamp(lo_y, hi_y);
    }

    /// Bounding box of the active board's notes (top-left min, bottom-right
    /// max), or `None` when the board is empty.
    fn content_bounds(&self) -> Option<(WorldPoint, WorldPoint)> {
        let notes = &self.active_board().notes;
        let first = notes.first()?;
        let mut min = WorldPoint {
            x: first.x,
            y: first.y,
        };
        let mut max = WorldPoint {
            x: first.x + NOTE_W,
            y: first.y + NOTE_H,
        };
        for n in notes {
            min.x = min.x.min(n.x);
            min.y = min.y.min(n.y);
            max.x = max.x.max(n.x + NOTE_W);
            max.y = max.y.max(n.y + NOTE_H);
        }
        Some((min, max))
    }

    fn center_on_content(&mut self) {
        let (sx, sy) = self.view().scale();
        let center = match self.content_bounds() {
            Some((min, max)) => WorldPoint {
                x: (min.x + max.x) / 2.0,
                y: (min.y + max.y) / 2.0,
            },
            None => WorldPoint { x: 0.0, y: 0.0 },
        };
        self.camera.origin = WorldPoint {
            x: center.x - (self.viewport.width as f64 / 2.0) / sx,
            y: center.y - (self.viewport.height as f64 / 2.0) / sy,
        };
        self.clamp_origin();
    }

    // ---- world switching ----

    fn switch_world(&mut self, index: usize) {
        if self.boards.is_empty() {
            return;
        }
        let index = index % self.boards.len();
        if index == self.active {
            return;
        }
        self.active = index;
        self.selected = None;
        self.mode = Mode::Nav;
        self.editor = None;
        self.prompt = None;
        self.centered = false; // re-center on the new board's content
    }

    // ---- note creation / deletion ----

    fn new_note(&mut self) {
        // Zoom in to write *first*, then place the note at the view center it
        // will actually be shown at - otherwise it lands at the old zoom's
        // center and drifts off-screen once we jump to document.
        self.camera.zoom = ZoomLevel::Document;
        let (sx, sy) = self.view().scale();
        // Drop it at the middle of the view, top-left offset so its center sits
        // under the viewport center.
        let center = WorldPoint {
            x: self.camera.origin.x + (self.viewport.width as f64 / 2.0) / sx - NOTE_W / 2.0,
            y: self.camera.origin.y + (self.viewport.height as f64 / 2.0) / sy - NOTE_H / 2.0,
        };
        let color = Color::ALL[self.color_tick % Color::ALL.len()];
        self.color_tick += 1;
        let top = self
            .active_board()
            .notes
            .iter()
            .map(|n| n.z)
            .max()
            .unwrap_or(0);
        let id = self.next_id;
        self.next_id += 1;
        self.revision += 1;
        self.active_board_mut().notes.push(Note {
            id,
            title: "new note".to_string(),
            body: String::new(),
            x: center.x,
            y: center.y,
            z: top + 1,
            color,
        });
        self.selected = Some(id);
        // Already at document zoom (set above); settle the camera and open the
        // editor on the fresh note straight away.
        self.clamp_origin();
        self.begin_edit();
    }

    fn delete_selected(&mut self) {
        if let Some(id) = self.selected {
            self.revision += 1;
            self.active_board_mut().notes.retain(|n| n.id != id);
            self.selected = None;
            self.editor = None;
            self.mode = Mode::Nav;
        }
    }
}

/// Decimal digits in `n`, for laying out a tab's note count.
fn digits(n: usize) -> u16 {
    let mut n = n / 10;
    let mut d = 1;
    while n > 0 {
        n /= 10;
        d += 1;
    }
    d
}

/// A world is a directory, so its name has to survive being one.
fn validate_board_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("give it a name".into());
    }
    if name.starts_with('.') {
        return Err("cannot start with a dot".into());
    }
    if name.chars().any(|c| matches!(c, '/' | '\\')) {
        return Err("no slashes - a world is one directory".into());
    }
    if name.chars().count() > BOARD_NAME_MAX {
        return Err(format!("keep it under {BOARD_NAME_MAX} characters"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinz_core::MemoryStore;
    use pinz_core::Store;
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn app() -> App {
        let mut store = MemoryStore::seeded();
        let mut app = App::new(store.load().unwrap());
        // Give it a viewport so spatial math has something to work with.
        app.set_viewport(Rect {
            x: 0,
            y: 2,
            width: 100,
            height: 30,
        });
        app
    }

    #[test]
    fn starts_on_first_board_at_titles() {
        let app = app();
        assert_eq!(app.active_index(), 0);
        assert_eq!(app.zoom(), ZoomLevel::Titles);
        assert!(app.selected().is_none());
    }

    #[test]
    fn q_quits_and_ctrl_c_quits() {
        let mut a = app();
        a.on_key(key(KeyCode::Char('q')));
        assert!(a.should_quit());

        let mut b = app();
        b.on_key(KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        assert!(b.should_quit());
    }

    #[test]
    fn zoom_in_and_out_clamps_at_the_ends() {
        let mut a = app();
        for _ in 0..10 {
            a.on_key(key(KeyCode::Char('+')));
        }
        assert_eq!(a.zoom(), ZoomLevel::Document);
        for _ in 0..10 {
            a.on_key(key(KeyCode::Char('-')));
        }
        assert_eq!(a.zoom(), ZoomLevel::Cluster);
    }

    #[test]
    fn tab_cycles_worlds_and_numbers_jump() {
        let mut a = app();
        a.on_key(key(KeyCode::Tab));
        assert_eq!(a.active_index(), 1);
        a.on_key(key(KeyCode::Char('1')));
        assert_eq!(a.active_index(), 0);
        a.on_key(key(KeyCode::Char('3')));
        assert_eq!(a.active_index(), 2);
    }

    #[test]
    fn new_note_adds_selects_and_enters_edit() {
        let mut a = app();
        let before = a.active_board().notes.len();
        a.on_key(key(KeyCode::Char('n')));
        assert_eq!(a.active_board().notes.len(), before + 1);
        assert!(a.selected().is_some());
        assert_eq!(a.mode(), Mode::Edit);
        assert_eq!(a.zoom(), ZoomLevel::Document);
    }

    #[test]
    fn typing_then_esc_saves_the_title() {
        let mut a = app();
        a.on_key(key(KeyCode::Char('n'))); // edit, editor holds "new note"
        for c in "hi".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        a.on_key(key(KeyCode::Esc)); // esc saves
        assert_eq!(a.mode(), Mode::Nav);
        let id = a.selected().unwrap();
        let note = a.active_board().notes.iter().find(|n| n.id == id).unwrap();
        assert_eq!(note.title, "new notehi");
        assert_eq!(note.body, "");
    }

    #[test]
    fn enter_inserts_a_newline_rather_than_committing() {
        let mut a = app();
        a.on_key(key(KeyCode::Char('n')));
        a.on_key(key(KeyCode::Enter)); // a newline, not a save
        assert_eq!(a.mode(), Mode::Edit, "enter keeps editing");
        assert!(a.selected().is_some());
    }

    #[test]
    fn t_cycles_themes_forward_and_wraps() {
        let mut a = app();
        let first = a.theme().name;
        a.on_key(key(KeyCode::Char('t')));
        assert_ne!(a.theme().name, first, "cycling should change the theme");
        // Walk all the way around; it must land back on the first.
        for _ in 1..super::theme::THEMES.len() {
            a.on_key(key(KeyCode::Char('t')));
        }
        assert_eq!(a.theme().name, first, "cycling should wrap");
    }

    #[test]
    fn shift_t_cycles_backward() {
        let mut a = app();
        let first = a.theme().name;
        a.on_key(key(KeyCode::Char('T')));
        // One step back from the first theme is the last theme.
        assert_eq!(a.theme().name, super::theme::THEMES.last().unwrap().name);
        a.on_key(key(KeyCode::Char('t')));
        assert_eq!(a.theme().name, first);
    }

    #[test]
    fn set_theme_by_name_matches_loosely() {
        let mut a = app();
        assert!(a.set_theme_by_name("gruvbox"));
        assert_eq!(a.theme().name, "Gruvbox");
        assert!(!a.set_theme_by_name("nonesuch"));
        assert_eq!(a.theme().name, "Gruvbox", "a miss leaves the theme alone");
    }

    #[test]
    fn editing_splits_first_line_as_title_rest_as_body() {
        let mut a = app();
        a.on_key(key(KeyCode::Char('n'))); // edit, editor holds "new note"
        assert_eq!(a.zoom(), ZoomLevel::Document, "edit forces document zoom");
        a.on_key(key(KeyCode::Enter)); // newline -> start the body
        for c in "line1".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        a.on_key(key(KeyCode::Enter));
        for c in "line2".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        a.on_key(key(KeyCode::Esc)); // save
        assert_eq!(a.mode(), Mode::Nav);
        let id = a.selected().unwrap();
        let note = a.active_board().notes.iter().find(|n| n.id == id).unwrap();
        assert_eq!(note.title, "new note");
        assert_eq!(note.body, "line1\nline2");
        assert!(a.editor().is_none(), "editor is cleared after saving");
    }

    #[test]
    fn edit_does_nothing_without_a_selection() {
        let mut a = app();
        a.selected = None;
        a.on_key(key(KeyCode::Char('e')));
        assert_eq!(a.mode(), Mode::Nav);
        assert!(a.editor().is_none());
    }

    #[test]
    fn c_cycles_the_selected_notes_color_both_ways() {
        let mut a = app();
        a.on_key(key(KeyCode::Char('n')));
        a.on_key(key(KeyCode::Esc)); // save, still selected in Nav
        let id = a.selected().unwrap();
        let color_of = |a: &App| a.active_board().notes.iter().find(|n| n.id == id).unwrap().color;
        let before = color_of(&a);
        a.on_key(key(KeyCode::Char('c')));
        assert_ne!(color_of(&a), before, "c should change the color");
        a.on_key(key(KeyCode::Char('C')));
        assert_eq!(color_of(&a), before, "C should step back");
    }

    #[test]
    fn ctrl_backspace_deletes_a_word_while_editing() {
        let mut a = app();
        a.on_key(key(KeyCode::Char('n'))); // edit, editor holds "new note"
        a.on_key(KeyEvent {
            code: KeyCode::Backspace,
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        a.on_key(key(KeyCode::Esc));
        let id = a.selected().unwrap();
        let note = a.active_board().notes.iter().find(|n| n.id == id).unwrap();
        assert_eq!(note.title, "new ");
    }

    #[test]
    fn alt_arrows_move_by_word_instead_of_typing_letters() {
        let alt = |code| KeyEvent {
            code,
            modifiers: KeyModifiers::ALT,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        // Both spellings of Option+arrow a terminal may send, plus an unbound
        // Alt chord - none of them may reach the buffer as text.
        for code in [
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Char('b'),
            KeyCode::Char('f'),
            KeyCode::Char('z'),
        ] {
            let mut a = app();
            a.on_key(key(KeyCode::Char('n'))); // edit, editor holds "new note"
            a.on_key(alt(code));
            a.on_key(key(KeyCode::Esc));
            let id = a.selected().unwrap();
            let note = a.active_board().notes.iter().find(|n| n.id == id).unwrap();
            assert_eq!(note.title, "new note", "{code:?} typed into the note");
        }

        // And the bound ones actually move: alt+left lands before "note", so
        // typing there splits the title.
        let mut a = app();
        a.on_key(key(KeyCode::Char('n')));
        a.on_key(alt(KeyCode::Left));
        a.on_key(key(KeyCode::Char('X')));
        a.on_key(key(KeyCode::Esc));
        let id = a.selected().unwrap();
        let note = a.active_board().notes.iter().find(|n| n.id == id).unwrap();
        assert_eq!(note.title, "new Xnote");
    }

    #[test]
    fn the_revision_tracks_changes_worth_saving() {
        let mut a = app();
        let start = a.revision();

        // Looking around changes nothing on disk.
        a.on_key(key(KeyCode::Char('+')));
        a.on_key(key(KeyCode::Tab));
        a.on_key(key(KeyCode::Right));
        assert_eq!(a.revision(), start, "panning and zooming are not changes");

        a.on_key(key(KeyCode::Char('n'))); // new note
        let after_new = a.revision();
        assert!(after_new > start, "creating a pin is a change");

        for c in "hi".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        a.on_key(key(KeyCode::Esc)); // save the edit
        let after_edit = a.revision();
        assert!(after_edit > after_new, "editing a pin is a change");

        a.on_key(key(KeyCode::Char('c'))); // recolor
        assert!(a.revision() > after_edit, "recoloring is a change");

        let before_delete = a.revision();
        a.on_key(key(KeyCode::Char('d')));
        assert!(a.revision() > before_delete, "deleting a pin is a change");
    }

    #[test]
    fn w_opens_the_new_world_prompt_and_esc_backs_out() {
        let mut a = app();
        let before = a.boards().len();
        a.on_key(key(KeyCode::Char('w')));
        assert_eq!(a.mode(), Mode::Prompt);
        assert!(a.prompt().is_some());

        for c in "wavez".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        assert_eq!(a.prompt().unwrap().input, "wavez");

        a.on_key(key(KeyCode::Esc));
        assert_eq!(a.mode(), Mode::Nav, "esc backs out");
        assert!(a.prompt().is_none());
        assert_eq!(a.boards().len(), before, "nothing was created");
    }

    #[test]
    fn confirming_the_prompt_creates_the_world_and_switches_to_it() {
        let mut a = app();
        let before = a.boards().len();
        a.on_key(key(KeyCode::Char('w')));
        for c in "reading".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        a.on_key(key(KeyCode::Enter));

        assert_eq!(a.mode(), Mode::Nav);
        assert_eq!(a.boards().len(), before + 1);
        assert_eq!(a.active_board().name, "reading", "lands on the new world");
        assert!(a.active_board().notes.is_empty());
    }

    #[test]
    fn a_refused_name_keeps_the_prompt_and_what_was_typed() {
        let mut a = app();
        let before = a.boards().len();
        a.on_key(key(KeyCode::Char('w')));
        for c in "a/b".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        a.on_key(key(KeyCode::Enter));

        assert_eq!(a.mode(), Mode::Prompt, "a bad name does not close the prompt");
        let prompt = a.prompt().unwrap();
        assert_eq!(prompt.input, "a/b", "the typed name survives");
        assert!(prompt.error.is_some(), "and it says why");
        assert_eq!(a.boards().len(), before);

        // Fixing it in place works.
        a.on_key(key(KeyCode::Backspace));
        a.on_key(key(KeyCode::Backspace));
        a.on_key(key(KeyCode::Char('b')));
        a.on_key(key(KeyCode::Enter));
        assert_eq!(a.active_board().name, "ab");
    }

    #[test]
    fn naming_an_existing_world_switches_instead_of_duplicating() {
        let mut a = app();
        let before = a.boards().len();
        let existing = a.boards()[1].name.clone();
        a.on_key(key(KeyCode::Char('w')));
        for c in existing.chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        a.on_key(key(KeyCode::Enter));
        assert_eq!(a.boards().len(), before, "two directories cannot share a name");
        assert_eq!(a.active_board().name, existing);
    }

    #[test]
    fn an_empty_or_dotted_name_is_refused() {
        for bad in ["", "   ", ".hidden"] {
            let mut a = app();
            let before = a.boards().len();
            a.on_key(key(KeyCode::Char('w')));
            for c in bad.chars() {
                a.on_key(key(KeyCode::Char(c)));
            }
            a.on_key(key(KeyCode::Enter));
            assert_eq!(a.mode(), Mode::Prompt, "{bad:?} should be refused");
            assert_eq!(a.boards().len(), before);
        }
    }

    #[test]
    fn the_tab_strip_ends_with_a_plus_and_spans_do_not_overlap() {
        let a = app();
        let tabs = a.tabs();
        assert_eq!(tabs.len(), a.boards().len() + 1);
        assert!(matches!(tabs.last().unwrap().kind, TabKind::New));

        // Each span starts where the previous one ended: this is what makes a
        // click land on the tab it looks like it landed on.
        for pair in tabs.windows(2) {
            assert_eq!(pair[0].x + pair[0].width, pair[1].x, "gap or overlap in the strip");
        }
        match &tabs[0].kind {
            TabKind::World { index, active, .. } => {
                assert_eq!(*index, 0);
                assert!(active, "the first world starts active");
            }
            _ => panic!("expected a world first"),
        }
    }

    #[test]
    fn clicking_a_tab_switches_worlds_and_clicking_plus_opens_the_prompt() {
        let mut a = app();
        a.set_tabs_area(Rect { x: 0, y: 1, width: 100, height: 1 });
        let tabs = a.tabs();

        // The second world's tab.
        let second = &tabs[1];
        a.on_mouse(mouse_down(second.x, 1));
        assert_eq!(a.active_index(), 1, "clicked the second world");

        // The + at the end.
        let plus = a.tabs().last().unwrap().clone();
        a.on_mouse(mouse_down(plus.x + 1, 1));
        assert_eq!(a.mode(), Mode::Prompt, "the + opens the new-world prompt");
    }

    #[test]
    fn a_prompt_takes_text_not_board_shortcuts() {
        // In the prompt, "n" and "q" are letters, not new-note and quit.
        let mut a = app();
        let before = a.active_board().notes.len();
        a.on_key(key(KeyCode::Char('w')));
        for c in "nq".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        assert_eq!(a.prompt().unwrap().input, "nq");
        assert!(!a.should_quit(), "q must not quit while naming");
        assert_eq!(a.boards()[0].notes.len(), before, "n must not add a pin");
    }

    #[test]
    fn delete_removes_the_selected_note() {
        let mut a = app();
        a.on_key(key(KeyCode::Char('n')));
        a.on_key(key(KeyCode::Esc)); // save edit, back to Nav, still selected
        let before = a.active_board().notes.len();
        a.on_key(key(KeyCode::Char('d')));
        assert_eq!(a.active_board().notes.len(), before - 1);
        assert!(a.selected().is_none());
    }

    #[test]
    fn clicking_a_note_selects_the_topmost_and_raises_it() {
        let mut a = app();
        // Two notes stacked at the same spot; the higher z should win the click
        // and then be raised further.
        a.active_board_mut().notes.clear();
        a.active_board_mut().notes.push(Note {
            id: 100,
            title: "under".into(),
            body: String::new(),
            x: 300.0,
            y: 300.0,
            z: 1,
            color: Color::Yellow,
        });
        a.active_board_mut().notes.push(Note {
            id: 200,
            title: "over".into(),
            body: String::new(),
            x: 300.0,
            y: 300.0,
            z: 2,
            color: Color::Blue,
        });
        a.center_on_content();

        // Click the center of the stack.
        let v = a.view();
        let (cx, cy) = v.cell_of(WorldPoint {
            x: 300.0 + NOTE_W / 2.0,
            y: 300.0 + NOTE_H / 2.0,
        });
        let col = (v_area(&a).x as f64 + cx) as u16;
        let row = (v_area(&a).y as f64 + cy) as u16;
        a.on_mouse(mouse_down(col, row));

        assert_eq!(a.selected(), Some(200));
        let over = a.active_board().notes.iter().find(|n| n.id == 200).unwrap();
        let under = a.active_board().notes.iter().find(|n| n.id == 100).unwrap();
        assert!(over.z > under.z, "clicked note should be raised to the top");
    }

    #[test]
    fn clicking_empty_space_clears_selection() {
        let mut a = app();
        a.selected = Some(1);
        // Corner of the viewport, far from any seeded note once centered.
        a.on_mouse(mouse_down(a.viewport.x, a.viewport.y));
        assert!(a.selected().is_none());
    }

    // ---- selection and copy ----

    /// A note in edit mode holding `text`, with the caret at the end.
    fn editing(text: &str) -> App {
        let mut a = app();
        a.on_key(key(KeyCode::Char('n')));
        a.editor = Some(TextEditor::new(text));
        a
    }

    #[test]
    fn shift_arrow_builds_a_selection() {
        let mut a = editing("hello");
        a.on_key(chord(KeyCode::Left, KeyModifiers::SHIFT));
        a.on_key(chord(KeyCode::Left, KeyModifiers::SHIFT));
        assert_eq!(a.editor().unwrap().selected_text().as_deref(), Some("lo"));
    }

    #[test]
    fn a_plain_arrow_collapses_the_selection() {
        let mut a = editing("hello");
        a.on_key(chord(KeyCode::Left, KeyModifiers::SHIFT));
        a.on_key(key(KeyCode::Left));
        assert_eq!(a.editor().unwrap().selection(), None);
    }

    #[test]
    fn alt_shift_arrow_extends_by_word() {
        let mut a = editing("foo bar baz");
        a.on_key(chord(KeyCode::Left, KeyModifiers::ALT | KeyModifiers::SHIFT));
        assert_eq!(a.editor().unwrap().selected_text().as_deref(), Some("baz"));
    }

    #[test]
    fn cmd_arrows_jump_to_the_line_edges() {
        let mut a = editing("hello");
        a.on_key(chord(KeyCode::Left, KeyModifiers::SUPER));
        assert_eq!(a.editor().unwrap().cursor(), Cursor { row: 0, col: 0 });
        a.on_key(chord(KeyCode::Right, KeyModifiers::SUPER));
        assert_eq!(a.editor().unwrap().cursor(), Cursor { row: 0, col: 5 });
    }

    #[test]
    fn cmd_shift_arrow_selects_to_the_line_edge() {
        let mut a = editing("hello");
        a.on_key(chord(KeyCode::Left, KeyModifiers::SUPER | KeyModifiers::SHIFT));
        assert_eq!(a.editor().unwrap().selected_text().as_deref(), Some("hello"));
    }

    #[test]
    fn a_cmd_chord_never_types_its_letter() {
        let mut a = editing("hi");
        a.on_key(chord(KeyCode::Char('z'), KeyModifiers::SUPER));
        assert_eq!(a.editor().unwrap().text(), "hi");
    }

    #[test]
    fn ctrl_a_selects_the_whole_note() {
        let mut a = editing("one\ntwo");
        a.on_key(chord(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert_eq!(a.editor().unwrap().selected_text().as_deref(), Some("one\ntwo"));
    }

    #[test]
    fn ctrl_c_with_a_selection_copies_instead_of_quitting() {
        let mut a = editing("hello");
        a.on_key(chord(KeyCode::Left, KeyModifiers::SHIFT));
        a.on_key(chord(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(!a.should_quit(), "a copy must not quit the app");
        assert_eq!(a.take_pending_copy().as_deref(), Some("o"));
        assert_eq!(a.editor().unwrap().text(), "hello", "copy does not remove text");
    }

    #[test]
    fn ctrl_c_without_a_selection_still_quits() {
        let mut a = editing("hello");
        a.on_key(chord(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(a.should_quit());
        assert_eq!(a.take_pending_copy(), None);
    }

    #[test]
    fn cmd_c_copies_but_never_quits() {
        let mut a = editing("hello");
        a.on_key(chord(KeyCode::Char('c'), KeyModifiers::SUPER));
        assert!(!a.should_quit(), "cmd+c is not an escape hatch");
        assert_eq!(a.take_pending_copy(), None, "nothing selected, nothing copied");
    }

    #[test]
    fn ctrl_x_cuts_the_selection() {
        let mut a = editing("hello");
        a.on_key(chord(KeyCode::Left, KeyModifiers::SHIFT));
        a.on_key(chord(KeyCode::Left, KeyModifiers::SHIFT));
        a.on_key(chord(KeyCode::Char('x'), KeyModifiers::CONTROL));
        assert_eq!(a.take_pending_copy().as_deref(), Some("lo"));
        assert_eq!(a.editor().unwrap().text(), "hel");
    }

    #[test]
    fn typing_over_a_selection_replaces_it() {
        let mut a = editing("hello");
        a.on_key(chord(KeyCode::Left, KeyModifiers::SHIFT));
        a.on_key(chord(KeyCode::Left, KeyModifiers::SHIFT));
        a.on_key(key(KeyCode::Char('p')));
        assert_eq!(a.editor().unwrap().text(), "help");
    }

    #[test]
    fn y_in_nav_copies_the_selected_note() {
        let mut a = app();
        a.on_key(key(KeyCode::Char('n')));
        a.on_key(key(KeyCode::Enter));
        for c in "body".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        a.on_key(key(KeyCode::Esc)); // save, still selected in Nav
        a.on_key(key(KeyCode::Char('y')));
        assert_eq!(a.take_pending_copy().as_deref(), Some("new note\nbody"));
    }

    #[test]
    fn y_in_nav_copies_the_title_alone_when_there_is_no_body() {
        let mut a = app();
        a.on_key(key(KeyCode::Char('n')));
        a.on_key(key(KeyCode::Esc));
        a.on_key(key(KeyCode::Char('y')));
        assert_eq!(a.take_pending_copy().as_deref(), Some("new note"));
    }

    #[test]
    fn y_in_nav_does_nothing_with_no_note_selected() {
        let mut a = app();
        a.selected = None;
        a.on_key(key(KeyCode::Char('y')));
        assert_eq!(a.take_pending_copy(), None);
    }

    #[test]
    fn a_copy_reports_a_status_that_the_next_key_clears() {
        let mut a = editing("hello");
        a.on_key(chord(KeyCode::Left, KeyModifiers::SHIFT));
        a.on_key(chord(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(a.status().is_some_and(|s| s.contains("copied")), "{:?}", a.status());
        a.on_key(key(KeyCode::Left));
        assert_eq!(a.status(), None);
    }

    #[test]
    fn taking_the_pending_copy_leaves_nothing_behind() {
        let mut a = editing("hello");
        a.on_key(chord(KeyCode::Char('a'), KeyModifiers::CONTROL));
        a.on_key(chord(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(a.take_pending_copy().is_some());
        assert_eq!(a.take_pending_copy(), None, "a copy is delivered once");
    }

    // ---- paste ----

    #[test]
    fn paste_inserts_multi_line_text_into_the_editor() {
        let mut a = editing("hi ");
        a.on_paste("one\ntwo".to_string());
        assert_eq!(a.editor().unwrap().text(), "hi one\ntwo");
    }

    #[test]
    fn paste_replaces_a_selection() {
        let mut a = editing("hello");
        a.on_key(chord(KeyCode::Char('a'), KeyModifiers::CONTROL));
        a.on_paste("bye".to_string());
        assert_eq!(a.editor().unwrap().text(), "bye");
    }

    #[test]
    fn paste_into_a_prompt_takes_only_the_first_line() {
        let mut a = app();
        a.on_key(key(KeyCode::Char('w')));
        a.on_paste("world\nignored".to_string());
        a.on_key(key(KeyCode::Enter));
        assert_eq!(a.mode(), Mode::Nav);
        assert_eq!(a.active_board().name, "world");
    }

    #[test]
    fn paste_in_nav_does_nothing() {
        let mut a = app();
        let before = a.active_board().notes.len();
        a.on_paste("text".to_string());
        assert_eq!(a.active_board().notes.len(), before);
    }

    // ---- mouse selection ----

    /// The screen cell of a character inside the note currently being edited.
    fn cell_in_note(a: &App, vrow: u16, vcol: u16) -> (u16, u16) {
        let (cells, _) = a.edit_layout().expect("a note should be open");
        ((cells.x + 1) as u16 + vcol, (cells.y + 1) as u16 + vrow)
    }

    #[test]
    fn dragging_inside_the_edited_note_selects_text() {
        let mut a = editing("hello world");
        let (c0, r0) = cell_in_note(&a, 0, 0);
        a.on_mouse(mouse_down(c0, r0));
        assert_eq!(a.editor().unwrap().cursor(), Cursor { row: 0, col: 0 });
        a.on_mouse(mouse_drag(c0 + 5, r0));
        assert_eq!(a.editor().unwrap().selected_text().as_deref(), Some("hello"));
        assert_eq!(a.mode(), Mode::Edit, "a text drag must not end the edit");
    }

    #[test]
    fn a_plain_click_inside_the_note_just_moves_the_caret() {
        let mut a = editing("hello world");
        let (c0, r0) = cell_in_note(&a, 0, 3);
        a.on_mouse(mouse_down(c0, r0));
        a.on_mouse(mouse_up(c0, r0));
        assert_eq!(a.editor().unwrap().cursor(), Cursor { row: 0, col: 3 });
        assert_eq!(a.editor().unwrap().selection(), None);
        assert_eq!(a.mode(), Mode::Edit);
    }

    #[test]
    fn dragging_past_the_note_edge_clamps_instead_of_stopping() {
        let mut a = editing("hello world");
        let (c0, r0) = cell_in_note(&a, 0, 0);
        a.on_mouse(mouse_down(c0, r0));
        a.on_mouse(mouse_drag(c0 + 500, r0));
        let selected = a.editor().unwrap().selected_text().unwrap();
        assert!(selected.starts_with("hello"), "clamped to the row's end: {selected:?}");
    }

    #[test]
    fn clicking_outside_the_edited_note_still_commits_the_edit() {
        let mut a = editing("hello");
        a.on_mouse(mouse_down(a.viewport.x, a.viewport.y));
        assert_eq!(a.mode(), Mode::Nav, "a click on the board saves and leaves edit");
    }

    // ---- undo / redo ----

    fn titles(a: &App) -> Vec<String> {
        a.active_board().notes.iter().map(|n| n.title.clone()).collect()
    }

    /// Title and body of every note. The editor opens with the cursor at the end
    /// of the whole buffer, so typing lands in the body - titles alone would not
    /// see an edit at all.
    fn contents(a: &App) -> Vec<String> {
        a.active_board()
            .notes
            .iter()
            .map(|n| format!("{}\n{}", n.title, n.body))
            .collect()
    }

    #[test]
    fn u_undoes_a_new_note() {
        let mut a = app();
        let before = titles(&a);
        a.on_key(key(KeyCode::Char('n')));
        a.on_key(key(KeyCode::Esc)); // save
        assert_ne!(titles(&a), before);
        a.on_key(key(KeyCode::Char('u')));
        assert_eq!(titles(&a), before, "the note should be gone again");
    }

    #[test]
    fn u_undoes_a_delete() {
        let mut a = app();
        a.selected = Some(a.active_board().notes[0].id);
        let before = titles(&a);
        a.on_key(key(KeyCode::Char('d')));
        assert_ne!(titles(&a), before);
        a.on_key(key(KeyCode::Char('u')));
        assert_eq!(titles(&a), before);
    }

    #[test]
    fn u_undoes_a_recolor() {
        let mut a = app();
        let id = a.active_board().notes[0].id;
        a.selected = Some(id);
        let color_of = |a: &App| a.active_board().notes.iter().find(|n| n.id == id).unwrap().color;
        let before = color_of(&a);
        a.on_key(key(KeyCode::Char('c')));
        assert_ne!(color_of(&a), before);
        a.on_key(key(KeyCode::Char('u')));
        assert_eq!(color_of(&a), before);
    }

    #[test]
    fn a_finished_edit_is_one_undo_step() {
        let mut a = app();
        a.selected = Some(a.active_board().notes[0].id);
        let before = contents(&a);
        a.on_key(key(KeyCode::Char('e')));
        for c in "xyz".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        a.on_key(key(KeyCode::Esc)); // one edit, three keystrokes
        assert_ne!(contents(&a), before);
        a.on_key(key(KeyCode::Char('u')));
        assert_eq!(contents(&a), before, "one undo should take back the whole edit");
    }

    #[test]
    fn opening_a_note_and_closing_it_untouched_records_nothing() {
        let mut a = app();
        a.selected = Some(a.active_board().notes[0].id);
        a.on_key(key(KeyCode::Char('d'))); // one real step to undo back to
        let deleted = contents(&a);
        a.selected = Some(a.active_board().notes[0].id);
        a.on_key(key(KeyCode::Char('e')));
        a.on_key(key(KeyCode::Esc)); // opened and closed, changed nothing
        a.on_key(key(KeyCode::Char('u')));
        assert_ne!(contents(&a), deleted, "the no-op edit must not eat the undo");
    }

    #[test]
    fn a_drag_is_one_undo_step() {
        let mut a = app();
        let note = &a.active_board().notes[0];
        let (id, x0, y0) = (note.id, note.x, note.y);
        // Aim at the note's centre: a corner rounds outside it at this zoom.
        let (sc, sr) = cell_of_note(&a, id);
        a.on_mouse(mouse_down(sc, sr));
        assert_eq!(a.selected(), Some(id), "the drag should have grabbed the note");
        for step in 1..=5 {
            a.on_mouse(mouse_drag(sc + step, sr + step));
        }
        a.on_mouse(mouse_up(sc + 5, sr + 5));
        let moved = a.active_board().notes.iter().find(|n| n.id == id).unwrap();
        assert_ne!((moved.x, moved.y), (x0, y0), "the note should have moved");

        a.on_key(key(KeyCode::Char('u')));
        let back = a.active_board().notes.iter().find(|n| n.id == id).unwrap();
        assert_eq!((back.x, back.y), (x0, y0), "one undo should take back the whole drag");
    }

    #[test]
    fn a_key_that_changes_nothing_records_no_undo_step() {
        let mut a = app();
        a.selected = Some(a.active_board().notes[0].id);
        a.on_key(key(KeyCode::Char('d'))); // one real step
        let after_delete = titles(&a);
        for _ in 0..5 {
            a.on_key(key(KeyCode::Left)); // pans; no board change
        }
        a.on_key(key(KeyCode::Char('u')));
        assert_ne!(titles(&a), after_delete, "undo should reach past the pans");
    }

    #[test]
    fn ctrl_r_redoes_what_u_undid() {
        let mut a = app();
        a.on_key(key(KeyCode::Char('n')));
        a.on_key(key(KeyCode::Esc));
        let created = titles(&a);
        a.on_key(key(KeyCode::Char('u')));
        assert_ne!(titles(&a), created);
        a.on_key(chord(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert_eq!(titles(&a), created, "redo should put it back");
    }

    #[test]
    fn a_new_action_clears_the_redo_stack() {
        let mut a = app();
        a.on_key(key(KeyCode::Char('n')));
        a.on_key(key(KeyCode::Esc));
        a.on_key(key(KeyCode::Char('u')));
        a.selected = Some(a.active_board().notes[0].id);
        a.on_key(key(KeyCode::Char('c'))); // a fresh action
        let after = titles(&a);
        a.on_key(chord(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert_eq!(titles(&a), after, "redo must not resurrect a discarded future");
    }

    #[test]
    fn undo_and_redo_on_empty_stacks_do_nothing() {
        let mut a = app();
        let before = titles(&a);
        a.on_key(key(KeyCode::Char('u')));
        a.on_key(chord(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert_eq!(titles(&a), before);
        assert!(a.status().is_some_and(|s| s.contains("nothing")), "{:?}", a.status());
    }

    #[test]
    fn undo_does_not_record_itself() {
        let mut a = app();
        a.on_key(key(KeyCode::Char('n')));
        a.on_key(key(KeyCode::Esc));
        let created = titles(&a);
        let empty = titles(&app());
        a.on_key(key(KeyCode::Char('u')));
        assert_eq!(titles(&a), empty);
        // A second undo has nothing left; if undo recorded itself it would
        // bounce back to the created state here.
        a.on_key(key(KeyCode::Char('u')));
        assert_ne!(titles(&a), created, "undo recorded itself");
    }

    #[test]
    fn the_undo_depth_is_capped() {
        let mut a = app();
        let id = a.active_board().notes[0].id;
        a.selected = Some(id);
        let color_of = |a: &App| a.active_board().notes.iter().find(|n| n.id == id).unwrap().color;
        let oldest = color_of(&a);
        for _ in 0..(UNDO_DEPTH + 10) {
            a.on_key(key(KeyCode::Char('c')));
        }
        for _ in 0..(UNDO_DEPTH + 10) {
            a.on_key(key(KeyCode::Char('u')));
        }
        assert_ne!(color_of(&a), oldest, "the oldest steps should have been evicted");
    }

    // helpers that reach into private state for assertions
    fn v_area(a: &App) -> Rect {
        a.viewport
    }
    /// The screen cell at the centre of a note, for aiming a mouse gesture.
    fn cell_of_note(a: &App, id: u64) -> (u16, u16) {
        let note = a.active_board().notes.iter().find(|n| n.id == id).unwrap();
        let (cx, cy) = a.view().cell_of(note.center());
        (
            (a.viewport.x as f64 + cx).round() as u16,
            (a.viewport.y as f64 + cy).round() as u16,
        )
    }
    fn chord(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }
    fn mouse_at(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }
    fn mouse_down(col: u16, row: u16) -> MouseEvent {
        mouse_at(MouseEventKind::Down(MouseButton::Left), col, row)
    }
    fn mouse_drag(col: u16, row: u16) -> MouseEvent {
        mouse_at(MouseEventKind::Drag(MouseButton::Left), col, row)
    }
    fn mouse_up(col: u16, row: u16) -> MouseEvent {
        mouse_at(MouseEventKind::Up(MouseButton::Left), col, row)
    }
}
