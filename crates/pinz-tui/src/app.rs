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

use crate::editor::TextEditor;
use crate::theme::{self, Theme};
use crate::view::View;

/// Arrow-key pan step, in cells.
const PAN_CELLS: f64 = 4.0;
/// How far past the note cloud you may pan before hitting the soft wall, in
/// world units. Enough to breathe; not enough to lose the board.
const PAN_MARGIN: f64 = 80.0;

/// Which part of a note the editor is focused on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Title,
    Body,
}

impl Field {
    /// The short label the footer shows while this field is being edited.
    pub fn label(self) -> &'static str {
        match self {
            Field::Title => "title",
            Field::Body => "body",
        }
    }
}

/// An in-progress edit: which note, which field, and the live text buffer. The
/// buffer owns the text being typed; it is written back into the note on every
/// keystroke (see [`App::sync_edit`]) so the note is never stale.
struct Edit {
    id: u64,
    field: Field,
    editor: TextEditor,
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
}

pub struct App {
    boards: Vec<Board>,
    active: usize,
    camera: Camera,
    /// Currently selected note (by id), if any.
    selected: Option<u64>,
    /// The active edit session, if the user is typing into a note.
    editing: Option<Edit>,
    drag: Option<Drag>,
    /// The board viewport from the last render, needed to interpret mouse
    /// positions and to center content. Zero until the first draw.
    viewport: Rect,
    /// Have we centered the current board on screen yet?
    centered: bool,
    next_id: u64,
    color_tick: usize,
    /// Index into [`theme::THEMES`] of the active theme.
    theme_index: usize,
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
            editing: None,
            drag: None,
            viewport: Rect::default(),
            centered: false,
            next_id,
            color_tick: 0,
            theme_index: 0,
            should_quit: false,
        }
    }

    // ---- read access for the renderer ----

    pub fn boards(&self) -> &[Board] {
        &self.boards
    }
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
    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    /// Whether the user is currently typing into a note.
    pub fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    /// The field being edited right now, if any.
    pub fn edit_field(&self) -> Option<Field> {
        self.editing.as_ref().map(|e| e.field)
    }

    /// The text to render for `field` of `note`: the live buffer (with the caret
    /// spliced in) when this is the field being edited, otherwise the note's own
    /// text. The bool reports whether this field is the active edit target, so
    /// the renderer can highlight it. The caret rides in the character flow, so
    /// it stays correct through wrapping.
    pub fn field_display(&self, note: &Note, field: Field) -> (String, bool) {
        if let Some(edit) = &self.editing {
            if edit.id == note.id && edit.field == field {
                return (edit.editor.with_caret("▏"), true);
            }
        }
        let text = match field {
            Field::Title => &note.title,
            Field::Body => &note.body,
        };
        (text.clone(), false)
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

    fn note_mut(&mut self, id: u64) -> Option<&mut Note> {
        self.active_board_mut()
            .notes
            .iter_mut()
            .find(|n| n.id == id)
    }

    // ---- keyboard ----

    pub fn on_key(&mut self, key: KeyEvent) {
        // Ctrl-C always quits, in any mode.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        if self.is_editing() {
            self.edit_key(key);
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc => self.selected = None,
            KeyCode::Char('+') | KeyCode::Char('=') => self.zoom_at_center(true),
            KeyCode::Char('-') | KeyCode::Char('_') => self.zoom_at_center(false),
            KeyCode::Char('n') => self.new_note(),
            KeyCode::Char('d') | KeyCode::Char('x') | KeyCode::Delete => self.delete_selected(),
            KeyCode::Char('e') | KeyCode::Enter => self.begin_edit(Field::Title),
            KeyCode::Char('t') => self.cycle_theme(true),
            KeyCode::Char('T') => self.cycle_theme(false),
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

    /// Keystrokes while editing. Text keys drive the [`TextEditor`]; a few keys
    /// are structural (switch field, finish, newline) and need the wider `App`.
    fn edit_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.finish_edit();
                return;
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.switch_field();
                return;
            }
            KeyCode::Enter => {
                // Enter adds a line in the body; from the single-line title it
                // moves you down into the body instead.
                if self.edit_field() == Some(Field::Body) {
                    if let Some(e) = self.editing.as_mut() {
                        e.editor.newline();
                    }
                } else {
                    self.switch_field();
                    return;
                }
            }
            code => {
                if let Some(e) = self.editing.as_mut() {
                    match code {
                        KeyCode::Backspace => e.editor.backspace(),
                        KeyCode::Delete => e.editor.delete(),
                        KeyCode::Left => e.editor.left(),
                        KeyCode::Right => e.editor.right(),
                        KeyCode::Up => e.editor.up(),
                        KeyCode::Down => e.editor.down(),
                        KeyCode::Home => e.editor.home(),
                        KeyCode::End => e.editor.end(),
                        KeyCode::Char(c) => e.editor.insert(c),
                        _ => {}
                    }
                }
            }
        }
        self.sync_edit();
    }

    /// Open the editor on `field` of the selected note, zoom in so it's legible,
    /// and bring it to the middle of the view.
    fn begin_edit(&mut self, field: Field) {
        let Some(id) = self.selected else {
            return;
        };
        let Some(note) = self.active_board().notes.iter().find(|n| n.id == id) else {
            return;
        };
        let editor = match field {
            Field::Title => TextEditor::single_line(&note.title),
            Field::Body => TextEditor::multi_line(&note.body),
        };
        self.editing = Some(Edit { id, field, editor });
        self.camera.zoom = ZoomLevel::Document;
        self.center_on_note(id);
    }

    /// Write the live buffer back into the note. Called after every edit so the
    /// note is always current - quitting or saving mid-edit loses nothing.
    fn sync_edit(&mut self) {
        let Some(edit) = self.editing.as_ref() else {
            return;
        };
        let (id, field, text) = (edit.id, edit.field, edit.editor.text());
        if let Some(note) = self.note_mut(id) {
            match field {
                Field::Title => note.title = text,
                Field::Body => note.body = text,
            }
        }
    }

    /// Toggle the editor between the title and the body, loading the other
    /// field's current text.
    fn switch_field(&mut self) {
        self.sync_edit();
        let Some(edit) = self.editing.as_ref() else {
            return;
        };
        let (id, next) = (
            edit.id,
            match edit.field {
                Field::Title => Field::Body,
                Field::Body => Field::Title,
            },
        );
        let text = self
            .active_board()
            .notes
            .iter()
            .find(|n| n.id == id)
            .map(|n| match next {
                Field::Title => n.title.clone(),
                Field::Body => n.body.clone(),
            })
            .unwrap_or_default();
        if let Some(edit) = self.editing.as_mut() {
            edit.field = next;
            edit.editor = match next {
                Field::Title => TextEditor::single_line(&text),
                Field::Body => TextEditor::multi_line(&text),
            };
        }
    }

    /// Commit and leave edit mode. The buffer is already synced live, so this
    /// just drops the session.
    fn finish_edit(&mut self) {
        self.sync_edit();
        self.editing = None;
    }

    // ---- mouse ----

    pub fn on_mouse(&mut self, m: MouseEvent) {
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
        if !self.in_viewport(col, row) {
            return;
        }
        // Editing ends the moment you touch the board.
        if self.is_editing() {
            self.finish_edit();
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

    fn mouse_drag(&mut self, col: u16, row: u16) {
        match self.drag {
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

    /// Center the camera on one note - used when we drop into edit mode so the
    /// note you're typing into is in the middle of the view.
    fn center_on_note(&mut self, id: u64) {
        let Some(center) = self
            .active_board()
            .notes
            .iter()
            .find(|n| n.id == id)
            .map(|n| n.center())
        else {
            return;
        };
        let (sx, sy) = self.view().scale();
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
        self.editing = None;
        self.centered = false; // re-center on the new board's content
    }

    // ---- note creation / deletion ----

    fn new_note(&mut self) {
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
        self.active_board_mut().notes.push(Note {
            id,
            title: String::new(),
            body: String::new(),
            x: center.x,
            y: center.y,
            z: top + 1,
            color,
        });
        self.selected = Some(id);
        // Drop straight into editing the (empty) title, zoomed in to write.
        self.begin_edit(Field::Title);
    }

    fn delete_selected(&mut self) {
        if let Some(id) = self.selected {
            self.active_board_mut().notes.retain(|n| n.id != id);
            self.selected = None;
        }
    }
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
        assert_eq!(a.zoom(), ZoomLevel::Survey);
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

    fn note_by_id(a: &App, id: u64) -> &Note {
        a.active_board().notes.iter().find(|n| n.id == id).unwrap()
    }

    #[test]
    fn new_note_adds_selects_and_enters_edit() {
        let mut a = app();
        let before = a.active_board().notes.len();
        a.on_key(key(KeyCode::Char('n')));
        assert_eq!(a.active_board().notes.len(), before + 1);
        assert!(a.selected().is_some());
        assert!(a.is_editing());
        assert_eq!(a.edit_field(), Some(Field::Title));
        assert_eq!(a.zoom(), ZoomLevel::Document);
    }

    #[test]
    fn typing_edits_the_title_live_and_esc_keeps_it() {
        let mut a = app();
        a.on_key(key(KeyCode::Char('n'))); // fresh note, empty title, editing
        for c in "milk".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        let id = a.selected().unwrap();
        assert_eq!(note_by_id(&a, id).title, "milk", "edits apply live");
        a.on_key(key(KeyCode::Esc));
        assert!(!a.is_editing(), "esc finishes editing");
        assert_eq!(note_by_id(&a, id).title, "milk", "and keeps the text");
    }

    #[test]
    fn insertion_happens_at_the_cursor_not_the_end() {
        let mut a = app();
        a.on_key(key(KeyCode::Char('n')));
        for c in "ac".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        a.on_key(key(KeyCode::Left)); // between a and c
        a.on_key(key(KeyCode::Char('b')));
        let id = a.selected().unwrap();
        assert_eq!(note_by_id(&a, id).title, "abc");
    }

    #[test]
    fn tab_moves_to_the_body_and_enter_adds_lines_there() {
        let mut a = app();
        a.on_key(key(KeyCode::Char('n')));
        for c in "title".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        a.on_key(key(KeyCode::Tab)); // -> body
        assert_eq!(a.edit_field(), Some(Field::Body));
        for c in "one".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        a.on_key(key(KeyCode::Enter)); // newline in the body
        for c in "two".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        let id = a.selected().unwrap();
        assert_eq!(
            note_by_id(&a, id).title,
            "title",
            "title survived the switch"
        );
        assert_eq!(note_by_id(&a, id).body, "one\ntwo");
    }

    #[test]
    fn enter_from_the_title_drops_into_the_body() {
        let mut a = app();
        a.on_key(key(KeyCode::Char('n')));
        a.on_key(key(KeyCode::Enter)); // title -> body (no newline in a title)
        assert_eq!(a.edit_field(), Some(Field::Body));
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
    fn delete_removes_the_selected_note() {
        let mut a = app();
        a.on_key(key(KeyCode::Char('n')));
        a.on_key(key(KeyCode::Esc)); // finish editing, back to Nav, still selected
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

    // helpers that reach into private state for assertions
    fn v_area(a: &App) -> Rect {
        a.viewport
    }
    fn mouse_down(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }
}
