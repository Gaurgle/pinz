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
use std::collections::{HashMap, VecDeque};
use std::time::Duration;

/// A note's text as logical lines, the way it reads when it is not being
/// edited: the title, a blank spacer, then the body from preview zoom up.
///
/// The one place a note becomes lines. `App` wraps these to know how far the
/// note can scroll and a click can reach; `ui` wraps them to draw it. The
/// repo's standing rule is that those are the same layout, computed once.
pub fn note_lines(note: &Note, lod: ZoomLevel) -> Vec<String> {
    let mut lines = vec![note.title.clone()];
    if matches!(lod, ZoomLevel::Preview | ZoomLevel::Document) && !note.body.is_empty() {
        lines.push(String::new());
        lines.extend(note.body.split('\n').map(str::to_string));
    }
    lines
}

/// Arrow-key pan step, in cells.
const PAN_CELLS: f64 = 4.0;
/// How far past the note cloud you may pan before hitting the soft wall, in
/// world units. Enough to breathe; not enough to lose the board.
const PAN_MARGIN: f64 = 80.0;
/// Width of the `+` tab, drawn as " + ".
const NEW_TAB_WIDTH: u16 = 3;
/// Rows of note text one wheel notch moves. Three is what a terminal sends a
/// pager, and a note is only thirteen rows tall: a full page a notch would
/// leave nothing on screen to read the new position against.
const WHEEL_ROWS: isize = 3;

/// Longest world name we will take. A world is a directory, so this is about
/// keeping paths sane rather than anything deeper.
const BOARD_NAME_MAX: usize = 40;

/// How many worlds you may have.
///
/// The number is not arbitrary: `1`-`9` are how you reach a world, so a tenth
/// could only be got to by tabbing past the others. A limit you can see in the
/// tab strip beats a world you can only reach the long way round.
const MAX_WORLDS: usize = 9;

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
    /// Which question is being asked. The prompt looks the same either way;
    /// what differs is what a confirmed answer means, so the kind rides on the
    /// prompt rather than being inferred from its title.
    pub kind: PromptKind,
    pub title: &'static str,
    pub hint: &'static str,
    /// What has been typed so far.
    pub input: String,
    /// Set when the last attempt to confirm was refused, so the reason can be
    /// shown without throwing away what was typed.
    pub error: Option<String>,
}

/// What an answered prompt does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    /// Make a world with the name that was typed.
    NewWorld,
    /// Delete the world you are on, if the name that was typed is its own.
    DeleteWorld,
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

/// Where a dragged pin would land if the button were released now.
///
/// The cursor column rides along with the world so the renderer can put the
/// pin glyph under the cursor without a second, separately-updated field to
/// keep in step with this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DropTarget {
    pub world: usize,
    pub col: u16,
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

/// How long the camera takes to travel to a jump's destination.
///
/// A guess to be tuned in use, including over SSH where every frame is a round
/// trip. See `design/specs/2026-08-19-camera-glide.md`.
const GLIDE: Duration = Duration::from_millis(140);

/// A camera glide in flight: where the view was when it started, and how far
/// through it we are. Only the origin travels; zoom cuts.
struct Glide {
    from: WorldPoint,
    elapsed: Duration,
}

/// One step of keyboard selection, in board directions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Left,
    Right,
    Up,
    Down,
}

pub struct App {
    boards: Vec<Board>,
    active: usize,
    camera: Camera,
    /// Currently selected note (by id), if any.
    selected: Option<u64>,
    mode: Mode,
    /// A camera glide in flight, if any. `self.camera` is always where we are
    /// *going*; this is what stands between that and what is on screen.
    glide: Option<Glide>,
    /// Whether the key list is up.
    ///
    /// A flag beside the mode rather than a fourth [`Mode`], because help is
    /// something you put *over* what you were doing: closing it has to put you
    /// back in the note you were editing, and a mode you replaced cannot do
    /// that.
    help: bool,
    /// The live note editor, present only while in [`Mode::Edit`].
    editor: Option<TextEditor>,
    /// How far each note's text is scrolled: the wrapped rows sitting above its
    /// text area, by note id. A note with no entry is at the top.
    ///
    /// A note is a fixed size in world units, so text longer than it fits is
    /// not a layout to fix but a window to move, and this is where those
    /// windows sit. One per note rather than one for the app, so reading down
    /// a long note and then looking at another does not cost you your place in
    /// the first.
    ///
    /// Kept here rather than in the renderer because `App` already owns the
    /// wrap that a click on text goes through: an offset only the renderer knew
    /// would put clicks on the wrong row the moment the text moved. Kept out of
    /// the note itself because where you are reading is this session's
    /// business, not something to carry between machines.
    scroll: HashMap<u64, usize>,
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
    /// Worlds whose directories are waiting to be removed. As with
    /// `pending_copy`, the app names the work and the runner does it: nothing
    /// here touches the filesystem, so a delete is testable with no board on
    /// disk.
    pending_deletes: Vec<String>,
    /// A one-off message for the footer, cleared by the next event. A copy is
    /// otherwise completely invisible.
    status: Option<String>,
    /// A sticky warning for the footer, set once at startup and never cleared:
    /// a stopped sync must stay visible for the whole session, because the
    /// alternate screen already ate one such warning for eleven days.
    warning: Option<String>,
    /// Another pinz owns this board, so this session may look but not touch.
    /// Enforced in [`Self::end_step`], where every event's changes are undone
    /// rather than kept - a rule at one choke point rather than a list of
    /// forbidden keys that a new feature could quietly fall outside of.
    read_only: bool,
    /// Board states to go back to, oldest first. Capped at [`UNDO_DEPTH`].
    undo: VecDeque<Snapshot>,
    /// States undone past, newest last. Cleared by any fresh change.
    redo: Vec<Snapshot>,
    /// The state as it was before the event being handled. Held across a whole
    /// drag so a gesture becomes one undo step rather than one per mouse-move.
    pending: Option<Snapshot>,
    /// Where a pin would land if released now, set while a note drag hovers the
    /// tab strip. The renderer arms that tab.
    drop_target: Option<DropTarget>,
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
            glide: None,
            help: false,
            editor: None,
            scroll: HashMap::new(),
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
            pending_deletes: Vec::new(),
            status: None,
            warning: None,
            read_only: false,
            undo: VecDeque::new(),
            redo: Vec::new(),
            pending: None,
            drop_target: None,
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
    /// The camera **as it appears on screen**, which is the target camera only
    /// when nothing is travelling.
    ///
    /// Drawing and hit-testing both read this, so a click during a glide lands
    /// on the pin you can see rather than one that has not arrived yet.
    pub fn camera(&self) -> Camera {
        let Some(glide) = &self.glide else {
            return self.camera;
        };
        let t = (glide.elapsed.as_secs_f64() / GLIDE.as_secs_f64()).clamp(0.0, 1.0);
        // Ease out: the board decelerates into place rather than stopping dead.
        let eased = 1.0 - (1.0 - t).powi(3);
        Camera {
            origin: WorldPoint {
                x: glide.from.x + (self.camera.origin.x - glide.from.x) * eased,
                y: glide.from.y + (self.camera.origin.y - glide.from.y) * eased,
            },
            zoom: self.camera.zoom,
        }
    }

    /// Whether anything is moving. The runner blocks for input unless this is
    /// true, which is what keeps pinz at zero CPU sitting open on a desk.
    pub fn animating(&self) -> bool {
        self.glide.is_some()
    }

    /// Advance any animation by `dt`.
    ///
    /// Elapsed time is handed in rather than read, so `App` keeps its promise
    /// to be a state machine with no I/O, and tests advance time explicitly
    /// instead of sleeping. Deliberately does not touch `revision`: the runner
    /// writes pins to disk when that moves, and a glide changes no board data.
    pub fn tick(&mut self, dt: Duration) {
        let Some(glide) = self.glide.as_mut() else {
            return;
        };
        glide.elapsed += dt;
        if glide.elapsed >= GLIDE {
            self.glide = None;
        }
    }

    /// Send the camera to a new origin as a jump: it travels rather than cuts.
    ///
    /// The glide starts from what is currently *on screen*, so a second jump
    /// mid-flight carries on from where the eye is instead of lurching back to
    /// where the last one began.
    fn glide_to(&mut self, origin: WorldPoint) {
        let shown = self.camera().origin;
        self.camera.origin = origin;
        self.clamp_origin();
        self.glide = (self.camera.origin != shown).then_some(Glide {
            from: shown,
            elapsed: Duration::ZERO,
        });
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
    /// How many wrapped rows sit above the top of this note's text area. The
    /// renderer offsets the text by this much, and clamping happens here so a
    /// note whose text shrank cannot leave its window stranded past the end.
    pub fn scroll_of(&self, id: u64) -> usize {
        self.scroll
            .get(&id)
            .copied()
            .unwrap_or(0)
            .min(self.max_scroll_of(id))
    }

    pub fn editor(&self) -> Option<&TextEditor> {
        self.editor.as_ref()
    }
    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    /// Where a dragged pin would land, for the renderer to arm that tab and put
    /// the pin under the cursor.
    pub fn drop_target(&self) -> Option<DropTarget> {
        self.drop_target
    }

    /// The note currently being dragged, so the renderer can name it.
    pub fn dragging_note(&self) -> Option<&Note> {
        let Some(Drag::Note { id, .. }) = self.drag else {
            return None;
        };
        self.active_board().notes.iter().find(|n| n.id == id)
    }

    /// The board viewport from the last render. Here for tests and for a
    /// renderer that needs to un-project a point, as [`App::active_index`] is.
    #[allow(dead_code)]
    pub fn viewport(&self) -> Rect {
        self.viewport
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

    /// Take the worlds waiting to be removed from the store. The runner calls
    /// this immediately before a save, so the save that follows is the one that
    /// puts back anything an undo has since brought back.
    pub fn take_pending_deletes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_deletes)
    }

    /// Say something in the footer. For the runner, which finds out whether a
    /// copy actually reached the terminal after the app has stopped looking.
    pub fn set_status(&mut self, message: String) {
        self.status = Some(message);
    }

    /// The sticky warning, if this session has one.
    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    /// Put a warning in the footer for the rest of the session. For the
    /// runner, when a sync stops and the board is running local-only.
    pub fn set_warning(&mut self, message: String) {
        self.warning = Some(message);
    }

    /// Is this session forbidden from changing the board?
    pub fn read_only(&self) -> bool {
        self.read_only
    }

    /// Say no to an action that would need write access, reporting whether it
    /// was refused. Used by the two doors into an input mode: `end_step` would
    /// undo the result anyway, but only after the typing, and an editor whose
    /// work is discarded on close is a worse answer than never opening one.
    fn refuse_if_read_only(&mut self) -> bool {
        if self.read_only {
            self.status = Some("read-only: another pinz owns this board".into());
        }
        self.read_only
    }

    /// Refuse every change for the rest of the session. For the runner, when
    /// another pinz already owns this board.
    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
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

    /// Whether the key list is up, for the renderer to draw over everything.
    pub fn help(&self) -> bool {
        self.help
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
        // At the limit the + could only ever refuse, so the strip simply ends
        // after the ninth world. Both the renderer and hit-testing read this
        // list, so the button and its click target disappear together.
        if self.boards.len() < MAX_WORLDS {
            out.push(Tab {
                kind: TabKind::New,
                x,
                width: NEW_TAB_WIDTH,
            });
        }
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
        View::new(self.camera(), self.viewport)
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
        self.follow_caret();
        self.end_step();
    }

    fn key(&mut self, key: KeyEvent) {
        // Whatever the last event had to say, this one supersedes it.
        self.status = None;
        // The key that dismisses the list is spent doing so: pressing ? and
        // then n should not leave you holding a note you never asked for.
        // Checked before everything else, ctrl-c included, so there is exactly
        // one thing any key can mean while the list is up.
        if self.help {
            self.help = false;
            return;
        }
        // F1 rather than only `?`, because in a note `?` is a character you are
        // trying to type. No terminal sends F1 as text, so it is the one key
        // that can mean the same thing in every mode.
        if key.code == KeyCode::F(1) {
            self.help = true;
            return;
        }
        if self.copy_chord(key) {
            return;
        }
        match self.mode {
            Mode::Edit => return self.edit_key(key),
            Mode::Prompt => return self.prompt_key(key),
            Mode::Nav => {}
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Char('r') if ctrl => self.redo_step(),
            KeyCode::Char('u') => self.undo_step(),
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('q') => self.quit(),
            KeyCode::Esc => self.selected = None,
            KeyCode::PageDown => self.page_selected(true),
            KeyCode::PageUp => self.page_selected(false),
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
            KeyCode::Char('W') => self.begin_delete_world(),
            KeyCode::Char('y') => self.yank_note(),
            KeyCode::Tab => self.switch_world(self.active + 1),
            KeyCode::BackTab => self.switch_world(self.active + self.boards.len() - 1),
            KeyCode::Char(c @ '1'..='9') => self.switch_world((c as usize) - ('1' as usize)),
            // Selection before panning: shift means "select" here exactly as it
            // does inside a note, and hjkl are the same four steps for a hand
            // already on the home row.
            KeyCode::Left if shift => self.select_toward(Step::Left),
            KeyCode::Right if shift => self.select_toward(Step::Right),
            KeyCode::Up if shift => self.select_toward(Step::Up),
            KeyCode::Down if shift => self.select_toward(Step::Down),
            KeyCode::Char('H') => self.select_toward(Step::Left),
            KeyCode::Char('L') => self.select_toward(Step::Right),
            KeyCode::Char('K') => self.select_toward(Step::Up),
            KeyCode::Char('J') => self.select_toward(Step::Down),
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
        // Not our board to change: put it back exactly as it was, and say so.
        // Reverting here rather than refusing each key means a change can only
        // land if it survives this one check, whatever produced it.
        if self.read_only {
            self.restore(snap);
            self.status = Some("read-only: another pinz owns this board".into());
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
            self.quit();
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
        self.follow_caret();
        self.end_step();
    }

    fn paste(&mut self, text: String) {
        self.status = None;
        if self.help {
            return;
        }
        match self.mode {
            Mode::Edit => {
                if let Some(editor) = self.editor.as_mut() {
                    editor.insert_str(&text);
                }
            }
            Mode::Prompt => {
                let Some(prompt) = self.prompt.as_mut() else {
                    return;
                };
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
        if self.refuse_if_read_only() {
            return;
        }
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
        // The editor lays the note out as one buffer and the read-only view as
        // title, spacer, body, so a window measured against one does not point
        // at the same row in the other. `follow_caret` sets it from the caret.
        self.scroll.remove(&id);
        self.mode = Mode::Edit;
        self.camera.zoom = ZoomLevel::Document;
        // Centre rather than merely clamp: `e` from a zoomed-out board used to
        // leave you looking wherever you already were, with the note you just
        // opened somewhere off to the side.
        self.center_on_note(id);
    }

    /// Put one pin in the middle of the viewport.
    ///
    /// `clamp_origin` can pull it off-centre for a pin near the edge of the
    /// board; that is the same pan margin every other camera move obeys, not a
    /// rule of its own.
    fn center_on_note(&mut self, id: u64) {
        let Some(origin) = self.centered_origin(id) else {
            return;
        };
        self.glide_to(origin);
    }

    /// The origin that puts one pin in the middle of the viewport, at the
    /// current zoom. Unclamped; every caller runs it through `clamp_origin`.
    fn centered_origin(&self, id: u64) -> Option<WorldPoint> {
        let (cx, cy) = self.note_center(id)?;
        let (sx, sy) = self.view().scale();
        Some(WorldPoint {
            x: cx - (self.viewport.width as f64 / 2.0) / sx,
            y: cy - (self.viewport.height as f64 / 2.0) / sy,
        })
    }

    fn note_center(&self, id: u64) -> Option<(f64, f64)> {
        self.active_board()
            .notes
            .iter()
            .find(|n| n.id == id)
            .map(center_of)
    }

    // ---- keyboard selection ----

    /// Move the selection one pin in a direction, and fetch it into view.
    ///
    /// With nothing selected the direction is ignored and the pin nearest the
    /// middle of the screen is taken: the first press should land where you are
    /// already looking rather than at some edge of the board.
    fn select_toward(&mut self, step: Step) {
        let Some(target) = self.step_target(step) else {
            return;
        };
        self.selected = Some(target);
        self.bring_into_view(target);
        // At cluster zoom no title is drawn at all, so this is the only thing
        // that says which pin you just landed on.
        if let Some(note) = self.active_board().notes.iter().find(|n| n.id == target) {
            self.status = Some(if note.title.trim().is_empty() {
                "untitled pin".to_string()
            } else {
                note.title.clone()
            });
        }
    }

    /// The pin one step away, if there is one.
    ///
    /// Candidates are the pins strictly beyond this one along the axis, scored
    /// by how far along they are plus twice how far off it - so a pin straight
    /// ahead beats a nearer one away to the side. There is no wrap-around: a
    /// selection that leaps to the opposite edge costs more than stopping does.
    fn step_target(&self, step: Step) -> Option<u64> {
        let notes = &self.active_board().notes;
        let Some(current) = self.selected.and_then(|id| notes.iter().find(|n| n.id == id)) else {
            return self.nearest_to_view_center();
        };
        let (cx, cy) = center_of(current);
        notes
            .iter()
            .filter(|n| n.id != current.id)
            .filter_map(|n| {
                let (nx, ny) = center_of(n);
                let (along, off) = match step {
                    Step::Left => (cx - nx, ny - cy),
                    Step::Right => (nx - cx, ny - cy),
                    Step::Up => (cy - ny, nx - cx),
                    Step::Down => (ny - cy, nx - cx),
                };
                if along > 0.0 {
                    Some((along + 2.0 * off.abs(), n.id))
                } else {
                    None
                }
            })
            // Ties broken by id, so the same board always steps the same way.
            .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
            .map(|(_, id)| id)
    }

    fn nearest_to_view_center(&self) -> Option<u64> {
        let (sx, sy) = self.view().scale();
        let mid = (
            self.camera.origin.x + (self.viewport.width as f64 / 2.0) / sx,
            self.camera.origin.y + (self.viewport.height as f64 / 2.0) / sy,
        );
        self.active_board()
            .notes
            .iter()
            .map(|n| {
                let (x, y) = center_of(n);
                ((x - mid.0).hypot(y - mid.1), n.id)
            })
            .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
            .map(|(_, id)| id)
    }

    /// Put a pin on screen if it is not already, by centring it.
    ///
    /// A pin you can already see does not move the board at all: stepping
    /// between neighbours should stay calm, and you keep your sense of where
    /// you are.
    ///
    /// When the board *does* have to move it centres, rather than scrolling the
    /// least that makes the pin fit. Minimum scroll leaves the pin flush against
    /// whichever edge it came in from, so where a pin ended up was a function of
    /// which direction you approached it - arriving at the same pin from the
    /// left and from the right put it in opposite corners.
    fn bring_into_view(&mut self, id: u64) {
        let Some((x, y)) = self
            .active_board()
            .notes
            .iter()
            .find(|n| n.id == id)
            .map(|n| (n.x, n.y))
        else {
            return;
        };
        let (sx, sy) = self.view().scale();
        let view_w = self.viewport.width as f64 / sx;
        let view_h = self.viewport.height as f64 / sy;
        let origin = self.camera.origin;
        let on_screen = x >= origin.x
            && x + NOTE_W <= origin.x + view_w
            && y >= origin.y
            && y + NOTE_H <= origin.y + view_h;
        if on_screen {
            return;
        }
        self.center_on_note(id);
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
        if let Some(id) = self.selected {
            self.scroll.remove(&id);
        }
        self.mode = Mode::Nav;
    }

    /// Leave, saving whatever is open on the way out.
    ///
    /// Every quit key goes through here so none of them can forget the commit.
    /// In nav there is no editor and this is just a flag; in edit it is the
    /// difference between keeping the note you were typing and losing it. The
    /// reason is the one `commit_edit` already gives for Esc: a whole note is
    /// too much to lose to a single key. Ctrl-C is the only way out of edit
    /// mode - `q` there is a letter - so without this the exit key was also
    /// the discard key.
    fn quit(&mut self) {
        self.commit_edit();
        self.should_quit = true;
    }

    /// Open the prompt that names a new world.
    fn begin_new_world(&mut self) {
        if self.refuse_if_read_only() {
            return;
        }
        // Refused here rather than on confirm: being asked for a name and only
        // then told the world cannot exist is the wrong order to find out.
        if self.boards.len() >= MAX_WORLDS {
            self.status = Some(format!("{MAX_WORLDS} worlds is the limit"));
            return;
        }
        self.prompt = Some(Prompt {
            kind: PromptKind::NewWorld,
            title: "new world",
            hint: "enter to create · esc to cancel",
            input: String::new(),
            error: None,
        });
        self.mode = Mode::Prompt;
    }

    /// Delete the world you are on: outright if it is empty, on a typed
    /// confirmation if there are pins to lose.
    ///
    /// Both refusals happen here rather than on confirm, for the reason
    /// [`Self::begin_new_world`] gives: being asked for a name and only then
    /// told it cannot happen is the wrong order to find out.
    ///
    /// The last world stays. Everything from [`Self::active_board`] to the tab
    /// strip assumes there is a board to be on, and an empty state for every
    /// renderer is a poor trade for a thing nobody wants to do.
    fn begin_delete_world(&mut self) {
        if self.refuse_if_read_only() {
            return;
        }
        if self.boards.len() == 1 {
            self.status = Some(format!("{} is the last world", self.active_board().name));
            return;
        }
        // Nothing to lose, nothing to ask about.
        if self.active_board().notes.is_empty() {
            self.delete_active_world();
            return;
        }
        self.prompt = Some(Prompt {
            kind: PromptKind::DeleteWorld,
            title: "delete world",
            hint: "type its name to confirm · esc to cancel",
            input: String::new(),
            error: None,
        });
        self.mode = Mode::Prompt;
    }

    /// Drop the active world and queue its directory for the runner.
    ///
    /// The pins go with it: moving them out first is what dragging onto another
    /// world's tab is for. Undo restores both, because the snapshot
    /// [`Self::begin_step`] already took holds the whole workspace and the
    /// store still knows which file each pin came from.
    fn delete_active_world(&mut self) {
        let gone = self.boards.remove(self.active);
        self.pending_deletes.push(gone.name.clone());
        // The tab that slid into this slot is the one to show; deleting the
        // last tab falls back one.
        self.active = self.active.min(self.boards.len() - 1);
        self.selected = None;
        self.centered = false; // re-center on whatever board we landed on
        self.revision += 1;
        self.status = Some(format!("deleted {}", gone.name));
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
        let (kind, answer) = {
            let Some(prompt) = self.prompt.as_ref() else {
                return;
            };
            (prompt.kind, prompt.input.trim().to_string())
        };
        let outcome = match kind {
            PromptKind::NewWorld => self.create_world(&answer),
            PromptKind::DeleteWorld => self.confirm_delete_world(&answer),
        };
        if let Err(why) = outcome {
            if let Some(prompt) = self.prompt.as_mut() {
                prompt.error = Some(why);
            }
            return;
        }
        self.prompt = None;
        self.mode = Mode::Nav;
    }

    /// Make a world, or say why not.
    ///
    /// Creating a world does not switch to it. Naming one is something you do
    /// *while* working on another board, and being carried off the board you
    /// were looking at is a worse surprise than having to press its number.
    /// A name already on a tab is refused for the same reason: it would be the
    /// one way `w` could still move you.
    fn create_world(&mut self, name: &str) -> Result<(), String> {
        validate_board_name(name)?;
        // Two directories cannot share a name anyway.
        if self.boards.iter().any(|b| b.name == name) {
            return Err(format!("{name} already exists"));
        }
        self.boards.push(Board::new(name.to_string()));
        self.revision += 1;
        // Staying put means a growing tab strip is the only sign anything
        // happened, so say what was made and which key opens it.
        self.status = Some(format!(
            "created {name} · press {} to open it",
            self.boards.len()
        ));
        Ok(())
    }

    /// Delete the world you are on, if what was typed is its name.
    ///
    /// Its name rather than a yes/no: the pins on a world are worth more than
    /// one keystroke of protection, and a name you have to read off the tab
    /// strip to type is a name you have looked at.
    fn confirm_delete_world(&mut self, answer: &str) -> Result<(), String> {
        let name = self.active_board().name.clone();
        if answer != name {
            return Err(format!("type {name} to delete it"));
        }
        self.delete_active_world();
        Ok(())
    }

    // ---- mouse ----

    pub fn on_mouse(&mut self, m: MouseEvent) {
        self.begin_step();
        self.mouse(m);
        self.end_step();
    }

    fn mouse(&mut self, m: MouseEvent) {
        self.status = None;
        // The list covers the board, so a click lands on something you cannot
        // see. Closing it is a key press.
        if self.help {
            return;
        }
        match m.kind {
            MouseEventKind::ScrollUp => self.wheel(false, m.column, m.row),
            MouseEventKind::ScrollDown => self.wheel(true, m.column, m.row),
            MouseEventKind::Down(MouseButton::Left) => self.mouse_down(m.column, m.row),
            MouseEventKind::Drag(MouseButton::Left) => self.mouse_drag(m.column, m.row),
            MouseEventKind::Up(MouseButton::Left) => self.mouse_up(m.column, m.row),
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

    /// The tab under a column of the strip, if any. Shared by clicking and by
    /// dropping a pin, so the two cannot resolve a column differently.
    fn tab_at(&self, col: u16) -> Option<TabKind> {
        let offset = col.checked_sub(self.tabs.x)?;
        self.tabs()
            .into_iter()
            .find(|t| offset >= t.x && offset < t.x + t.width)
            .map(|t| t.kind)
    }

    /// The world under a point, if that point is on the tab strip and lands on a
    /// world rather than the `+`.
    fn world_at_point(&self, col: u16, row: u16) -> Option<usize> {
        if !self.tabs.contains((col, row).into()) {
            return None;
        }
        match self.tab_at(col)? {
            TabKind::World { index, .. } => Some(index),
            TabKind::New => None,
        }
    }

    /// Resolve a click on the tab strip: a world switches to it, the `+` opens
    /// the new-world prompt, and a gap does nothing.
    fn click_tab(&mut self, col: u16) {
        let Some(kind) = self.tab_at(col) else {
            return;
        };
        // A click anywhere is also an answer of "not now" to an open prompt.
        if self.mode == Mode::Prompt {
            self.prompt = None;
            self.mode = Mode::Nav;
        }
        match kind {
            TabKind::World { index, .. } => {
                if self.mode == Mode::Edit {
                    self.commit_edit();
                }
                self.switch_world(index);
            }
            TabKind::New => self.begin_new_world(),
        }
    }

    /// Move a pin to another world, keeping where it sits in world space and
    /// putting it on top of whatever is already there.
    ///
    /// The view deliberately stays where it is: the common case is clearing
    /// several pins off one board in a row, and following each one would cost
    /// you your place every time. `FileStore` relocates the pin's file on the
    /// next save, so there is nothing to do here but move it between boards.
    fn move_note_to_board(&mut self, id: u64, target: usize) {
        if target == self.active || target >= self.boards.len() {
            return;
        }
        let Some(at) = self.active_board().notes.iter().position(|n| n.id == id) else {
            return;
        };
        let mut note = self.boards[self.active].notes.remove(at);
        // Worlds do not share a coordinate space, so the pin's old x and y mean
        // nothing here: keeping them can strand it far outside the cloud this
        // board frames. Land it in the middle, clear of what is already there.
        let center = self.boards[target].content_center();
        let spot = self.boards[target].free_spot(
            WorldPoint {
                x: center.x - NOTE_W / 2.0,
                y: center.y - NOTE_H / 2.0,
            },
            None,
        );
        note.x = spot.x;
        note.y = spot.y;
        note.z = self.boards[target]
            .notes
            .iter()
            .map(|n| n.z)
            .max()
            .unwrap_or(0)
            + 1;
        self.boards[target].notes.push(note);
        self.selected = None;
        self.revision += 1;
        self.status = Some(format!("moved to {}", self.boards[target].name));
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

    /// Is this the note currently open in the editor?
    fn editing(&self, id: u64) -> bool {
        self.mode == Mode::Edit && self.selected == Some(id) && self.editor.is_some()
    }

    /// The wrap a note's window moves over: the live editor buffer when it is
    /// the note being edited, its saved text otherwise. One door, so scrolling,
    /// hit-testing and drawing cannot end up reading different layouts.
    fn text_layout(&self, id: u64) -> Option<(CellRect, Wrapped)> {
        if self.editing(id) {
            self.edit_layout()
        } else {
            self.note_layout(id)
        }
    }

    /// A note's saved text, wrapped to its width at the current zoom.
    fn note_layout(&self, id: u64) -> Option<(CellRect, Wrapped)> {
        let note = self.active_board().notes.iter().find(|n| n.id == id)?;
        let cells = self.view().note_cells(note.position());
        // The text sits inside the note's one-cell border.
        let width = cells.width.checked_sub(2)?;
        let lines = note_lines(note, self.zoom());
        Some((cells, wrap::wrap(&lines, width as usize)))
    }

    /// How many rows of text a note shows at once, and how many it has.
    /// `None` when there is no room to show anything.
    fn text_extent(&self, id: u64) -> Option<(usize, usize)> {
        let (cells, wrapped) = self.text_layout(id)?;
        let height = cells.height.checked_sub(2)? as usize;
        (height > 0).then_some((height, wrapped.rows.len()))
    }

    /// The furthest a note can scroll: far enough to bring its last row to the
    /// bottom of the window and no further, so scrolling cannot run off into
    /// blank space under the text. Zero when everything already fits, which is
    /// also what makes it the test for "is there anything to scroll here".
    fn max_scroll_of(&self, id: u64) -> usize {
        match self.text_extent(id) {
            Some((height, rows)) => rows.saturating_sub(height),
            None => 0,
        }
    }

    /// Move a note's window, clamped to its text.
    fn scroll_note(&mut self, id: u64, rows: isize) {
        let at = self.scroll_of(id) as isize;
        let to = at.saturating_add(rows).clamp(0, self.max_scroll_of(id) as isize);
        self.scroll.insert(id, to as usize);
    }

    /// Bring the caret back into view after something moved it.
    ///
    /// The window moves only when the caret has left it, and then only far
    /// enough to catch it. Recentring on every keystroke would make the text
    /// jump under a caret that stepped a single row.
    fn follow_caret(&mut self) {
        let (Some(id), Some((cells, wrapped)), Some(editor)) =
            (self.selected, self.edit_layout(), self.editor.as_ref())
        else {
            return;
        };
        let Some(height) = cells.height.checked_sub(2).map(usize::from) else {
            return;
        };
        if height == 0 {
            return;
        }
        let (row, _) = wrapped.place(editor.cursor());
        let mut at = self.scroll_of(id);
        if row < at {
            at = row;
        } else if row >= at + height {
            at = row + 1 - height;
        }
        // Text can shrink under the window - a cut, a kill, an undone paste -
        // and leave it parked past the end.
        self.scroll
            .insert(id, at.min(wrapped.rows.len().saturating_sub(height)));
    }

    /// The wheel: on a note it moves that note's text, on the board it zooms.
    ///
    /// A note that is not hiding anything absorbs the notch and does nothing.
    /// That is the point rather than an oversight: which gesture you get is
    /// decided by what is under the pointer, never by how much someone happened
    /// to write. A wheel that zoomed the world out from under a short pin and
    /// scrolled a long one is a gesture you cannot predict before you turn it.
    fn wheel(&mut self, down: bool, col: u16, row: u16) {
        match self.wheel_target(col, row) {
            // `scroll_note` clamps, so this is a no-op on a note that fits.
            Some(id) => self.scroll_note(id, if down { WHEEL_ROWS } else { -WHEEL_ROWS }),
            None => self.zoom_at(!down, col, row),
        }
    }

    /// The note the wheel belongs to, or `None` when it belongs to the board.
    ///
    /// While a note is open the wheel is its own wherever the pointer is: you
    /// are inside its text, not on the board.
    ///
    /// Zoomed out past the body the wheel is the board's, whatever it is over.
    /// A note at cluster or titles zoom is a block or a headline rather than
    /// something you read down, and those are the levels where notes cover the
    /// screen - a wheel that died on every one of them would leave you unable
    /// to zoom out of a full board.
    fn wheel_target(&self, col: u16, row: u16) -> Option<u64> {
        if self.mode == Mode::Edit {
            return self.selected;
        }
        if !matches!(self.zoom(), ZoomLevel::Preview | ZoomLevel::Document) {
            return None;
        }
        let world = self.view().world_at(col, row);
        Some(self.active_board().note_at(world)?.id)
    }

    /// Page the selected note's text, the keyboard's answer to the wheel.
    ///
    /// A page keeps one row of overlap, so there is always a line you have
    /// already read to land on rather than a wall of new text.
    fn page_selected(&mut self, down: bool) {
        let Some(id) = self.selected else { return };
        let Some((height, _)) = self.text_extent(id) else {
            return;
        };
        let page = height.saturating_sub(1).max(1) as isize;
        self.scroll_note(id, if down { page } else { -page });
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
        let scrolled = dy as usize + self.selected.map_or(0, |id| self.scroll_of(id));
        Some(wrapped.locate(scrolled, dx as usize))
    }

    /// Release: a pin let go over another world's tab lands there. Anything
    /// else just ends the gesture.
    fn mouse_up(&mut self, col: u16, row: u16) {
        if let Some(Drag::Note { id, .. }) = self.drag {
            match self.world_at_point(col, row) {
                Some(world) => self.move_note_to_board(id, world),
                None => self.settle_note(id),
            }
        }
        self.drag = None;
        self.drop_target = None;
    }

    /// Nudge a just-dropped pin clear of anything it would hide.
    ///
    /// Done on release rather than inside [`Self::mouse_drag`] on purpose: a
    /// dragged pin tracks the cursor 1:1, and a cascade running mid-gesture
    /// would have it squirming out from under the pointer. It settles once,
    /// when you let go, inside the same undo step as the drag.
    fn settle_note(&mut self, id: u64) {
        let Some(at) = self
            .active_board()
            .notes
            .iter()
            .find(|n| n.id == id)
            .map(|n| n.position())
        else {
            return;
        };
        let spot = self.active_board().free_spot(at, Some(id));
        if spot == at {
            return; // clear already; don't bump `revision` for a no-op
        }
        if let Some(note) = self.note_mut(id) {
            note.x = spot.x;
            note.y = spot.y;
        }
    }

    fn mouse_drag(&mut self, col: u16, row: u16) {
        // A note dragged over the strip stops tracking the cursor and arms a
        // tab instead. Without this the pin would chase the cursor up into the
        // header, which is neither where it lives nor where it would land.
        //
        // Tracking stops over the *whole* strip, not just over the world tabs:
        // the `+` is not a drop target, but it is still no place for a pin.
        if matches!(self.drag, Some(Drag::Note { .. })) {
            let over_strip = self.tabs.contains((col, row).into());
            self.drop_target = over_strip
                .then(|| self.world_at_point(col, row))
                .flatten()
                .map(|world| DropTarget { world, col });
            if over_strip {
                return;
            }
        }
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
                self.glide = None; // the board follows the pointer, 1:1
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
        // Zoom cuts, so the origin shift that comes with it cuts too: gliding
        // the pan while the scale jumps under it looks worse than either.
        self.glide = None;
        let anchor = self.view().world_at(col, row);
        self.camera.zoom = target;
        // Shift the origin so `anchor` lands back under the same cell.
        let now = self.view().world_at(col, row);
        self.camera.origin.x += anchor.x - now.x;
        self.camera.origin.y += anchor.y - now.y;
        self.clamp_origin();
    }

    /// One zoom step from the keyboard.
    ///
    /// With a pin selected the zoom is *about that pin*: it ends up in the
    /// middle of the viewport, because `+` on a selection means "look closer at
    /// this one" and not "look closer at whatever the middle of the screen
    /// happened to hold". With nothing selected there is no such subject, so
    /// the middle of the view is the focal point.
    fn zoom_at_center(&mut self, zoom_in: bool) {
        match self.selected {
            Some(id) => self.zoom_onto_note(zoom_in, id),
            None => {
                let (col, row) = self.viewport_center();
                self.zoom_at(zoom_in, col, row);
            }
        }
    }

    /// Zoom one step and leave `id` centred. The pan cuts along with the zoom
    /// for the same reason `zoom_at` kills a glide: a board still travelling
    /// while the scale jumps under it reads as a lurch.
    fn zoom_onto_note(&mut self, zoom_in: bool, id: u64) {
        let target = if zoom_in {
            self.camera.zoom.zoomed_in()
        } else {
            self.camera.zoom.zoomed_out()
        };
        if target == self.camera.zoom {
            return;
        }
        self.glide = None;
        self.camera.zoom = target;
        // After the zoom: the centred origin depends on the new scale.
        let Some(origin) = self.centered_origin(id) else {
            return;
        };
        self.camera.origin = origin;
        self.clamp_origin();
    }

    fn viewport_center(&self) -> (u16, u16) {
        (
            self.viewport.x + self.viewport.width / 2,
            self.viewport.y + self.viewport.height / 2,
        )
    }

    // ---- pan ----

    fn pan_cells(&mut self, dx: f64, dy: f64) {
        self.glide = None; // a manipulation, not a jump: 1:1 or it reads as lag
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
        self.active_board().bounds()
    }

    fn center_on_content(&mut self) {
        // Opening a board, or arriving in a new world: there is no continuous
        // space between where you were and here to travel through.
        self.glide = None;
        let (sx, sy) = self.view().scale();
        let center = self.active_board().content_center();
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
        // Two notes made without panning would otherwise share one spot
        // exactly, and the older one would be gone behind the newer.
        let center = self.active_board().free_spot(center, None);
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

/// A pin's middle. Direction and distance between pins are measured from here,
/// not from the top-left corner, so a step lands where it looks like it should.
fn center_of(note: &Note) -> (f64, f64) {
    (note.x + NOTE_W / 2.0, note.y + NOTE_H / 2.0)
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
    use pinz_core::{CASCADE_X, CASCADE_Y};
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    /// Pins at known world positions, for the spatial tests. A cross around the
    /// origin plus one decoy far to the right and well below, which is nearer
    /// on the x axis than the pin due right but badly off it.
    const PINS: &[(f64, f64)] = &[
        (0.0, 0.0),      // 1, the middle
        (1200.0, 0.0),   // 2, due right
        (0.0, 900.0),    // 3, due down
        (-900.0, 0.0),   // 4, due left
        (0.0, -900.0),   // 5, due up
        (1800.0, 700.0), // 6, the decoy
    ];

    fn app_with_pins(positions: &[(f64, f64)]) -> App {
        let mut board = Board::new("spatial");
        for (i, (x, y)) in positions.iter().enumerate() {
            board.notes.push(Note {
                id: i as u64 + 1,
                title: format!("pin {}", i + 1),
                body: String::new(),
                x: *x,
                y: *y,
                z: i as u32 + 1,
                color: Color::Yellow,
            });
        }
        let mut a = App::new(vec![board]);
        a.set_viewport(VIEWPORT);
        a
    }

    const VIEWPORT: Rect = Rect {
        x: 0,
        y: 2,
        width: 100,
        height: 30,
    };

    /// Where a pin's centre lands on screen, in cells from the viewport corner.
    fn pin_cell(a: &App, id: u64) -> (f64, f64) {
        let n = a
            .active_board()
            .notes
            .iter()
            .find(|n| n.id == id)
            .expect("pin");
        View::new(a.camera(), VIEWPORT).cell_of(WorldPoint {
            x: n.x + NOTE_W / 2.0,
            y: n.y + NOTE_H / 2.0,
        })
    }

    fn shift(code: KeyCode) -> KeyEvent {
        chord(code, KeyModifiers::SHIFT)
    }

    #[test]
    fn the_first_step_takes_the_pin_nearest_the_view() {
        let mut a = app_with_pins(PINS);
        assert!(a.selected().is_none());
        a.on_key(shift(KeyCode::Right));
        assert_eq!(a.selected(), Some(1), "the middle pin is the closest one");
    }

    #[test]
    fn shift_arrows_step_to_the_nearest_pin_that_way() {
        let mut a = app_with_pins(PINS);
        a.on_key(shift(KeyCode::Right)); // onto pin 1
        for (code, want, why) in [
            (KeyCode::Right, 2, "the decoy is nearer on x but far off the axis"),
            (KeyCode::Left, 1, "and back"),
            (KeyCode::Down, 3, "straight down beats the decoy"),
            (KeyCode::Up, 1, "and back again"),
            (KeyCode::Left, 4, "due left"),
            (KeyCode::Right, 1, "returns"),
            (KeyCode::Up, 5, "due up"),
        ] {
            a.on_key(shift(code));
            assert_eq!(a.selected(), Some(want), "{why}");
        }
    }

    #[test]
    fn shift_hjkl_steps_the_same_way_as_the_arrows() {
        let mut a = app_with_pins(PINS);
        a.on_key(shift(KeyCode::Char('L'))); // onto pin 1
        a.on_key(shift(KeyCode::Char('L')));
        assert_eq!(a.selected(), Some(2), "l goes right");
        a.on_key(shift(KeyCode::Char('H')));
        assert_eq!(a.selected(), Some(1), "h goes left");
        a.on_key(shift(KeyCode::Char('J')));
        assert_eq!(a.selected(), Some(3), "j goes down");
        a.on_key(shift(KeyCode::Char('K')));
        assert_eq!(a.selected(), Some(1), "k goes up");
    }

    #[test]
    fn stepping_past_the_last_pin_stays_put() {
        let mut a = app_with_pins(PINS);
        a.on_key(shift(KeyCode::Right)); // pin 1
        a.on_key(shift(KeyCode::Left)); // pin 4, the leftmost
        assert_eq!(a.selected(), Some(4));
        a.on_key(shift(KeyCode::Left));
        assert_eq!(a.selected(), Some(4), "no wrap-around off the edge");
    }

    #[test]
    fn the_camera_moves_only_for_a_pin_it_cannot_show() {
        let mut a = app_with_pins(PINS);
        let still = a.camera().origin;
        a.on_key(shift(KeyCode::Right)); // pin 1, already on screen
        assert_eq!(
            a.camera().origin,
            still,
            "a visible pin must not lurch the board"
        );

        a.on_key(shift(KeyCode::Right)); // pin 2, off to the right
        a.tick(GLIDE); // it travels rather than jumping; let it arrive
        assert_ne!(a.camera().origin, still, "an off-screen pin must be fetched");
        let (cx, cy) = pin_cell(&a, 2);
        assert!(
            cx > 0.0 && cx < VIEWPORT.width as f64,
            "pin 2 still off screen at {cx}"
        );
        assert!(cy > 0.0 && cy < VIEWPORT.height as f64, "off vertically: {cy}");
    }

    #[test]
    fn stepping_onto_a_pin_names_it() {
        // At cluster zoom no title is drawn, so this is the only thing telling
        // you which pin you just landed on.
        let mut a = app_with_pins(PINS);
        a.on_key(shift(KeyCode::Right));
        a.on_key(shift(KeyCode::Right));
        assert!(
            a.status().is_some_and(|s| s.contains("pin 2")),
            "got {:?}",
            a.status()
        );
    }

    // ---- camera glide (design/specs/2026-08-19-camera-glide.md) ----

    #[test]
    fn a_step_to_an_off_screen_pin_glides_rather_than_jumping() {
        let mut a = app_with_pins(PINS);
        a.on_key(shift(KeyCode::Right)); // pin 1, already on screen
        let before = a.camera().origin;

        a.on_key(shift(KeyCode::Right)); // pin 2, off to the right
        assert!(a.animating(), "an off-screen pin starts a glide");
        assert_eq!(a.camera().origin, before, "the board has not moved yet");

        a.tick(GLIDE);
        assert!(!a.animating(), "and the glide finishes");
        let (cx, _) = pin_cell(&a, 2);
        assert!(
            cx > 0.0 && cx < VIEWPORT.width as f64,
            "pin 2 never arrived: {cx}"
        );
    }

    #[test]
    fn half_a_glide_is_partway_there() {
        // Otherwise a "glide" that snapped straight to the end would pass any
        // test that only checked where it finished.
        let mut a = app_with_pins(PINS);
        a.on_key(shift(KeyCode::Right));
        let from = a.camera().origin;
        a.on_key(shift(KeyCode::Right));

        a.tick(GLIDE / 2);
        let mid = a.camera().origin;
        a.tick(GLIDE);
        let to = a.camera().origin;
        assert!(
            mid.x > from.x && mid.x < to.x,
            "not between: {} .. {} .. {}",
            from.x,
            mid.x,
            to.x
        );
    }

    #[test]
    fn a_step_mid_glide_carries_on_from_what_is_on_screen() {
        let mut a = app_with_pins(PINS);
        a.on_key(shift(KeyCode::Right)); // pin 1
        a.on_key(shift(KeyCode::Right)); // pin 2, glide starts
        a.tick(GLIDE / 2);
        let shown = a.camera().origin;

        a.on_key(shift(KeyCode::Left)); // back to pin 1, mid-flight
        assert!(a.animating(), "still travelling");
        assert_eq!(
            a.camera().origin,
            shown,
            "a retarget must not lurch back to where the last one began"
        );
    }

    #[test]
    fn a_pin_fetched_from_off_screen_lands_in_the_middle() {
        // Scrolling the least that made a pin fit put it flush against the edge
        // it came in from, so where a pin ended up depended on which way you
        // approached it - and it was never the middle.
        let mut a = app_with_pins(PINS);
        for _ in 0..3 {
            a.on_key(key(KeyCode::Char('+'))); // document zoom, so pins do not all fit
        }
        a.on_key(shift(KeyCode::Right)); // land on pin 1
        a.tick(GLIDE);

        let (mid_x, mid_y) = (VIEWPORT.width as f64 / 2.0, VIEWPORT.height as f64 / 2.0);
        for (step, from) in [
            (KeyCode::Right, "the left"),
            (KeyCode::Left, "the right"),
            (KeyCode::Down, "above"),
            (KeyCode::Up, "below"),
        ] {
            a.on_key(shift(step));
            a.tick(GLIDE);
            let id = a.selected().unwrap();
            let (cx, cy) = pin_cell(&a, id);
            assert!(
                (cx - mid_x).abs() < 1.0,
                "pin {id} approached from {from} landed at x={cx}, wanted {mid_x}"
            );
            assert!(
                (cy - mid_y).abs() < 1.0,
                "pin {id} approached from {from} landed at y={cy}, wanted {mid_y}"
            );
        }
    }

    #[test]
    fn a_glide_writes_no_pins() {
        // The runner saves when `revision` moves. An animation that bumped it
        // would rewrite every pin file at frame rate.
        let mut a = app_with_pins(PINS);
        a.on_key(shift(KeyCode::Right));
        a.on_key(shift(KeyCode::Right));
        let rev = a.revision();
        a.tick(GLIDE / 2);
        a.tick(GLIDE);
        assert_eq!(a.revision(), rev);
    }

    #[test]
    fn a_manipulation_cuts_and_cancels_a_glide() {
        // Your hand is on these, so anything but 1:1 reads as lag.
        let arrow_pan = |a: &mut App| a.on_key(key(KeyCode::Right));
        let zoom = |a: &mut App| a.on_key(key(KeyCode::Char('+')));
        let drag = |a: &mut App| {
            a.on_mouse(mouse_down(5, 5)); // empty board, above every pin
            a.on_mouse(mouse_drag(20, 5));
        };
        for (act, what) in [
            (&arrow_pan as &dyn Fn(&mut App), "an arrow pan"),
            (&zoom, "a zoom"),
            (&drag, "a drag"),
        ] {
            let mut a = app_with_pins(PINS);
            a.on_key(shift(KeyCode::Right));
            a.on_key(shift(KeyCode::Right));
            assert!(a.animating(), "setup: a glide should be in flight");
            act(&mut a);
            assert!(!a.animating(), "{what} must cut, not glide");
        }
    }

    #[test]
    fn e_centres_the_pin_it_opens() {
        let mut a = app_with_pins(PINS);
        a.on_key(shift(KeyCode::Right)); // pin 1
        a.on_key(key(KeyCode::Char('e')));
        a.tick(GLIDE); // e glides there like any other jump
        assert_eq!(a.zoom(), ZoomLevel::Document);
        let (cx, cy) = pin_cell(&a, 1);
        let (want_x, want_y) = (VIEWPORT.width as f64 / 2.0, VIEWPORT.height as f64 / 2.0);
        assert!((cx - want_x).abs() < 1.0, "off centre horizontally: {cx}");
        assert!((cy - want_y).abs() < 1.0, "off centre vertically: {cy}");
    }

    #[test]
    fn zooming_with_a_pin_selected_centres_it() {
        let mut a = app_with_pins(PINS);
        a.on_key(shift(KeyCode::Right)); // pin 1
        a.tick(GLIDE);
        for _ in 0..6 {
            a.on_key(key(KeyCode::Right)); // shove it well off centre
        }
        let (before, _) = pin_cell(&a, 1);
        assert!(before < 20.0, "setup: the pin should be off centre, at {before}");

        a.on_key(key(KeyCode::Char('+')));
        let (cx, cy) = pin_cell(&a, 1);
        let (want_x, want_y) = (VIEWPORT.width as f64 / 2.0, VIEWPORT.height as f64 / 2.0);
        assert!((cx - want_x).abs() < 1.0, "off centre horizontally: {cx}");
        assert!((cy - want_y).abs() < 1.0, "off centre vertically: {cy}");
    }

    #[test]
    fn zooming_out_keeps_the_selected_pin_centred() {
        let mut a = app_with_pins(PINS);
        a.on_key(shift(KeyCode::Right)); // pin 1
        a.tick(GLIDE);
        for _ in 0..6 {
            a.on_key(key(KeyCode::Down));
        }
        a.on_key(key(KeyCode::Char('-')));
        let (cx, cy) = pin_cell(&a, 1);
        let (want_x, want_y) = (VIEWPORT.width as f64 / 2.0, VIEWPORT.height as f64 / 2.0);
        assert!((cx - want_x).abs() < 1.0, "off centre horizontally: {cx}");
        assert!((cy - want_y).abs() < 1.0, "off centre vertically: {cy}");
    }

    #[test]
    fn zooming_with_nothing_selected_holds_the_middle_of_the_view() {
        let mut a = app_with_pins(PINS);
        for _ in 0..6 {
            a.on_key(key(KeyCode::Right));
        }
        assert!(a.selected().is_none());
        let (col, row) = (VIEWPORT.x + VIEWPORT.width / 2, VIEWPORT.y + VIEWPORT.height / 2);
        let before = View::new(a.camera(), VIEWPORT).world_at(col, row);
        a.on_key(key(KeyCode::Char('+')));
        let after = View::new(a.camera(), VIEWPORT).world_at(col, row);
        assert!((after.x - before.x).abs() < 1.0, "the focal point moved: {after:?}");
        assert!((after.y - before.y).abs() < 1.0, "the focal point moved: {after:?}");
    }

    /// A board list already at the world limit, for testing the cap.
    fn filled_to_the_limit() -> App {
        let mut a = app();
        for n in a.boards().len()..MAX_WORLDS {
            a.on_key(key(KeyCode::Char('w')));
            for c in format!("world{n}").chars() {
                a.on_key(key(KeyCode::Char(c)));
            }
            a.on_key(key(KeyCode::Enter));
        }
        a
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
    fn question_mark_opens_the_key_list_and_any_key_closes_it() {
        let mut a = app();
        a.on_key(key(KeyCode::Char('?')));
        assert!(a.help(), "? opens the key list");

        let before = a.boards().to_vec();
        a.on_key(key(KeyCode::Char('n')));
        assert!(!a.help(), "any key closes it");
        assert_eq!(
            a.boards(),
            before.as_slice(),
            "and that key does nothing else"
        );
    }

    #[test]
    fn f1_opens_the_key_list_without_disturbing_the_note() {
        let mut a = app();
        a.on_key(key(KeyCode::Char('n')));
        for c in "hi".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        a.on_key(key(KeyCode::F(1)));
        assert!(a.help(), "f1 reaches the list from inside a note");

        a.on_key(key(KeyCode::Esc));
        assert!(!a.help());
        assert_eq!(a.mode(), Mode::Edit, "back in the note you were writing");
        assert_eq!(
            a.editor().unwrap().text(),
            "new notehi",
            "with the text untouched"
        );
    }

    #[test]
    fn a_question_mark_while_editing_is_just_a_character() {
        let mut a = app();
        a.on_key(key(KeyCode::Char('n')));
        a.on_key(key(KeyCode::Char('?')));
        assert!(!a.help());
        assert_eq!(a.editor().unwrap().text(), "new note?");
    }

    #[test]
    fn the_mouse_does_nothing_while_the_key_list_is_up() {
        let mut a = app();
        a.on_key(key(KeyCode::Char('?')));
        let before = a.boards().to_vec();
        a.on_mouse(mouse_down(10, 10));
        assert!(a.help(), "a click is not a way out either");
        assert_eq!(a.boards(), before.as_slice());
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

    /// Two pins closer than one cascade step on *both* axes hide each other,
    /// and become the same pin outright once positions round on the way to
    /// disk. No board may ever hold such a pair.
    fn assert_nothing_is_stacked(board: &Board) {
        for (i, a) in board.notes.iter().enumerate() {
            for b in &board.notes[i + 1..] {
                assert!(
                    (a.x - b.x).abs() >= CASCADE_X || (a.y - b.y).abs() >= CASCADE_Y,
                    "{:?} and {:?} are stacked at ({}, {}) and ({}, {})",
                    a.title,
                    b.title,
                    a.x,
                    a.y,
                    b.x,
                    b.y
                );
            }
        }
    }

    #[test]
    fn two_fresh_notes_do_not_land_on_the_same_spot() {
        let mut a = app();
        a.on_key(key(KeyCode::Char('n')));
        a.on_key(key(KeyCode::Esc));
        a.on_key(key(KeyCode::Char('n')));
        a.on_key(key(KeyCode::Esc));
        assert_nothing_is_stacked(a.active_board());
    }

    #[test]
    fn a_pin_dropped_on_another_settles_clear_of_it() {
        let mut a = app();
        let target = a.active_board().notes[1].clone();
        let (tc, tr) = cell_of_note(&a, target.id);
        let id = drag_first_pin_to(&mut a, tc, tr);
        a.on_mouse(mouse_up(tc, tr));

        assert_ne!(id, target.id, "the drag must grab a different pin");
        assert_nothing_is_stacked(a.active_board());
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
    fn quitting_mid_edit_saves_the_note() {
        let mut a = app();
        a.on_key(key(KeyCode::Char('n'))); // edit, editor holds "new note"
        a.on_key(key(KeyCode::Enter));
        for c in "body".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        a.on_key(chord(KeyCode::Char('c'), KeyModifiers::CONTROL)); // the only way out of edit
        assert!(a.should_quit());
        let id = a.selected().unwrap();
        let note = a.active_board().notes.iter().find(|n| n.id == id).unwrap();
        assert_eq!(note.title, "new note");
        assert_eq!(note.body, "body", "quitting must not drop the open edit");
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
        let color_of = |a: &App| {
            a.active_board()
                .notes
                .iter()
                .find(|n| n.id == id)
                .unwrap()
                .color
        };
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
    fn confirming_the_prompt_creates_the_world_without_leaving_this_one() {
        let mut a = app();
        // A note, selected, so there is something to be disturbed.
        a.on_key(key(KeyCode::Char('n')));
        a.on_key(key(KeyCode::Esc));
        let (here, selected, before) = (a.active_index(), a.selected(), a.boards().len());

        a.on_key(key(KeyCode::Char('w')));
        for c in "reading".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        a.on_key(key(KeyCode::Enter));

        assert_eq!(a.mode(), Mode::Nav);
        assert_eq!(a.boards().len(), before + 1);
        assert_eq!(a.boards().last().unwrap().name, "reading");
        assert!(a.boards().last().unwrap().notes.is_empty());
        assert_eq!(a.active_index(), here, "creating a world must not move you");
        assert_eq!(a.selected(), selected, "nor drop what you had selected");
    }

    #[test]
    fn creating_a_world_says_where_it_went() {
        // Staying put means the only sign is the tab strip growing, so the
        // footer names the world and the key that opens it.
        let mut a = app();
        a.on_key(key(KeyCode::Char('w')));
        for c in "reading".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        a.on_key(key(KeyCode::Enter));

        let status = a.status().unwrap_or_default().to_string();
        assert!(status.contains("reading"), "got {status:?}");
        assert!(
            status.contains('4'),
            "the tab number is how you get there, got {status:?}"
        );
    }

    #[test]
    fn the_tenth_world_is_refused_before_the_prompt_opens() {
        let mut a = filled_to_the_limit();
        assert_eq!(a.boards().len(), MAX_WORLDS);

        a.on_key(key(KeyCode::Char('w')));
        assert_eq!(a.mode(), Mode::Nav, "the prompt does not even open");
        assert!(a.prompt().is_none());
        assert!(
            a.status().is_some_and(|s| s.contains('9')),
            "a refusal must say what the limit is, got {:?}",
            a.status()
        );
        assert_eq!(a.boards().len(), MAX_WORLDS);
    }

    /// A world with nothing on it, made the way you would make one, and left
    /// as the world you are looking at.
    fn app_with_an_empty_world() -> App {
        let mut a = app();
        a.on_key(key(KeyCode::Char('w')));
        type_into(&mut a, "scratch");
        a.on_key(key(KeyCode::Enter));
        let tab = char::from_digit(a.boards().len() as u32, 10).unwrap();
        a.on_key(key(KeyCode::Char(tab)));
        a
    }

    fn type_into(a: &mut App, text: &str) {
        for c in text.chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn shift_w_will_not_delete_the_last_world() {
        let mut a = App::new(vec![Board::new("ideas")]);
        a.on_key(shift(KeyCode::Char('W')));

        assert_eq!(a.boards().len(), 1, "the last world stays");
        assert_eq!(a.mode(), Mode::Nav, "and is not even asked about");
        assert!(
            a.status().is_some_and(|s| s.contains("ideas")),
            "a refusal should name the world, got {:?}",
            a.status()
        );
    }

    #[test]
    fn a_read_only_board_refuses_to_delete_a_world() {
        let mut a = app();
        a.set_read_only(true);
        let before = a.boards().len();
        a.on_key(shift(KeyCode::Char('W')));

        assert_eq!(a.boards().len(), before);
        assert_eq!(a.mode(), Mode::Nav);
        assert!(a.take_pending_deletes().is_empty());
    }

    #[test]
    fn an_empty_world_goes_without_being_asked_about() {
        let mut a = app_with_an_empty_world();
        let before = a.boards().len();
        assert!(a.active_board().notes.is_empty());

        a.on_key(shift(KeyCode::Char('W')));

        assert_eq!(a.mode(), Mode::Nav, "nothing to confirm on an empty world");
        assert_eq!(a.boards().len(), before - 1);
        assert!(!a.boards().iter().any(|b| b.name == "scratch"));
        assert!(
            a.status().is_some_and(|s| s.contains("scratch")),
            "got {:?}",
            a.status()
        );
    }

    #[test]
    fn a_world_with_pins_on_it_asks_for_its_name_first() {
        let mut a = app();
        let (before, name) = (a.boards().len(), a.active_board().name.clone());

        a.on_key(shift(KeyCode::Char('W')));
        assert_eq!(a.mode(), Mode::Prompt);
        assert!(a.prompt().is_some());
        assert_eq!(a.boards().len(), before, "nothing is gone yet");

        type_into(&mut a, &name);
        a.on_key(key(KeyCode::Enter));

        assert_eq!(a.mode(), Mode::Nav);
        assert_eq!(a.boards().len(), before - 1);
        assert!(!a.boards().iter().any(|b| b.name == name));
    }

    #[test]
    fn the_wrong_name_keeps_the_world_and_what_you_typed() {
        let mut a = app();
        let before = a.boards().len();
        a.on_key(shift(KeyCode::Char('W')));
        type_into(&mut a, "ideaz");
        a.on_key(key(KeyCode::Enter));

        assert_eq!(a.mode(), Mode::Prompt, "the prompt stays open");
        let prompt = a.prompt().unwrap();
        assert_eq!(prompt.input, "ideaz", "and keeps what was typed");
        assert!(prompt.error.is_some(), "with the reason shown");
        assert_eq!(a.boards().len(), before);
    }

    #[test]
    fn esc_out_of_the_delete_prompt_leaves_the_worlds_alone() {
        let mut a = app();
        let before = a.boards().len();
        a.on_key(shift(KeyCode::Char('W')));
        a.on_key(key(KeyCode::Esc));

        assert_eq!(a.mode(), Mode::Nav);
        assert!(a.prompt().is_none());
        assert_eq!(a.boards().len(), before);
        assert!(a.take_pending_deletes().is_empty());
    }

    #[test]
    fn deleting_the_last_tab_falls_back_one() {
        let mut a = app();
        let last = a.boards().len() - 1;
        a.on_key(key(KeyCode::Char(
            char::from_digit(a.boards().len() as u32, 10).unwrap(),
        )));
        assert_eq!(a.active_index(), last);
        let name = a.active_board().name.clone();

        a.on_key(shift(KeyCode::Char('W')));
        type_into(&mut a, &name);
        a.on_key(key(KeyCode::Enter));

        assert_eq!(
            a.active_index(),
            last - 1,
            "the tab beside it is the one to show"
        );
        assert!(a.selected().is_none());
    }

    #[test]
    fn undo_brings_a_deleted_world_back_with_its_pins() {
        let mut a = app();
        let before = a.boards().to_vec();
        let name = a.active_board().name.clone();

        a.on_key(shift(KeyCode::Char('W')));
        type_into(&mut a, &name);
        a.on_key(key(KeyCode::Enter));
        assert_eq!(a.boards().len(), before.len() - 1);

        a.on_key(key(KeyCode::Char('u')));
        assert_eq!(a.boards(), before, "pins and all");
        assert_eq!(a.active_index(), 0);
    }

    #[test]
    fn a_deleted_world_is_handed_over_exactly_once() {
        let mut a = app_with_an_empty_world();
        a.on_key(shift(KeyCode::Char('W')));

        assert_eq!(a.take_pending_deletes(), vec!["scratch".to_string()]);
        assert!(
            a.take_pending_deletes().is_empty(),
            "a delete is delivered once"
        );
    }

    #[test]
    fn deleting_a_world_is_a_change_worth_saving() {
        let mut a = app_with_an_empty_world();
        let before = a.revision();
        a.on_key(shift(KeyCode::Char('W')));
        assert!(a.revision() > before);
    }

    #[test]
    fn the_tab_strip_drops_the_plus_at_the_limit() {
        // A + that can only ever refuse is worse than no +.
        let a = filled_to_the_limit();
        let tabs = a.tabs();
        assert_eq!(tabs.len(), MAX_WORLDS, "nine worlds and nothing else");
        assert!(!tabs.iter().any(|t| matches!(t.kind, TabKind::New)));
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

        assert_eq!(
            a.mode(),
            Mode::Prompt,
            "a bad name does not close the prompt"
        );
        let prompt = a.prompt().unwrap();
        assert_eq!(prompt.input, "a/b", "the typed name survives");
        assert!(prompt.error.is_some(), "and it says why");
        assert_eq!(a.boards().len(), before);

        // Fixing it in place works.
        a.on_key(key(KeyCode::Backspace));
        a.on_key(key(KeyCode::Backspace));
        a.on_key(key(KeyCode::Char('b')));
        a.on_key(key(KeyCode::Enter));
        assert_eq!(a.mode(), Mode::Nav, "the fixed name was accepted");
        assert_eq!(a.boards().last().unwrap().name, "ab");
    }

    #[test]
    fn naming_an_existing_world_is_refused_rather_than_switched_to() {
        let mut a = app();
        let before = a.boards().len();
        let existing = a.boards()[1].name.clone();
        a.on_key(key(KeyCode::Char('w')));
        for c in existing.chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        a.on_key(key(KeyCode::Enter));
        assert_eq!(
            a.boards().len(),
            before,
            "two directories cannot share a name"
        );
        assert_eq!(a.mode(), Mode::Prompt, "the prompt stays open to be fixed");
        assert!(a.prompt().unwrap().error.is_some(), "and says why");
        assert_eq!(a.active_index(), 0, "a refusal does not move you either");
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
            assert_eq!(
                pair[0].x + pair[0].width,
                pair[1].x,
                "gap or overlap in the strip"
            );
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
        a.set_tabs_area(Rect {
            x: 0,
            y: 1,
            width: 100,
            height: 1,
        });
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
        a.on_key(chord(
            KeyCode::Left,
            KeyModifiers::ALT | KeyModifiers::SHIFT,
        ));
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
        a.on_key(chord(
            KeyCode::Left,
            KeyModifiers::SUPER | KeyModifiers::SHIFT,
        ));
        assert_eq!(
            a.editor().unwrap().selected_text().as_deref(),
            Some("hello")
        );
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
        assert_eq!(
            a.editor().unwrap().selected_text().as_deref(),
            Some("one\ntwo")
        );
    }

    #[test]
    fn ctrl_c_with_a_selection_copies_instead_of_quitting() {
        let mut a = editing("hello");
        a.on_key(chord(KeyCode::Left, KeyModifiers::SHIFT));
        a.on_key(chord(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(!a.should_quit(), "a copy must not quit the app");
        assert_eq!(a.take_pending_copy().as_deref(), Some("o"));
        assert_eq!(
            a.editor().unwrap().text(),
            "hello",
            "copy does not remove text"
        );
    }

    #[test]
    fn a_read_only_board_refuses_every_change() {
        let mut a = app();
        a.set_read_only(true);
        let before = a.boards().to_vec();

        // A new pin, an edit, a delete and a recolor: none may land.
        a.on_key(key(KeyCode::Char('n')));
        a.on_key(key(KeyCode::Char('d')));
        a.on_key(key(KeyCode::Char('c')));
        assert_eq!(a.boards(), before.as_slice(), "the board must be untouched");
    }

    #[test]
    fn a_read_only_board_does_not_open_an_editor() {
        let mut a = app();
        a.set_read_only(true);
        a.on_key(key(KeyCode::Char('e')));
        assert_eq!(a.mode(), Mode::Nav, "editing is refused outright");
        assert!(a.editor().is_none());
        a.on_key(key(KeyCode::Char('w')));
        assert_eq!(a.mode(), Mode::Nav, "so is naming a new world");
    }

    #[test]
    fn a_read_only_board_says_why_it_refused() {
        let mut a = app();
        a.set_read_only(true);
        a.on_key(key(KeyCode::Char('n')));
        assert!(
            a.status().is_some_and(|s| s.contains("read-only")),
            "a refusal must explain itself, got {:?}",
            a.status()
        );
    }

    #[test]
    fn a_writable_board_still_accepts_changes() {
        let mut a = app();
        let before = a.boards().to_vec();
        a.on_key(key(KeyCode::Char('n')));
        assert_ne!(a.boards(), before.as_slice(), "the guard must not misfire");
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
        assert_eq!(
            a.take_pending_copy(),
            None,
            "nothing selected, nothing copied"
        );
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
    fn a_sync_warning_stays_up_while_one_off_statuses_come_and_go() {
        let mut a = app();
        a.set_warning("sync stopped: the same pin changed on both machines".into());
        a.on_key(key(KeyCode::Left));
        assert!(
            a.warning().is_some_and(|w| w.contains("sync stopped")),
            "a keystroke must not clear the warning"
        );
        a.set_status("copied 3 chars".into());
        a.on_key(key(KeyCode::Left));
        assert!(
            a.warning().is_some(),
            "a one-off status clearing must not take the warning with it"
        );
    }

    #[test]
    fn a_copy_reports_a_status_that_the_next_key_clears() {
        let mut a = editing("hello");
        a.on_key(chord(KeyCode::Left, KeyModifiers::SHIFT));
        a.on_key(chord(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(
            a.status().is_some_and(|s| s.contains("copied")),
            "{:?}",
            a.status()
        );
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
        assert_eq!(a.boards().last().unwrap().name, "world");
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
        assert_eq!(
            a.editor().unwrap().selected_text().as_deref(),
            Some("hello")
        );
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
        assert!(
            selected.starts_with("hello"),
            "clamped to the row's end: {selected:?}"
        );
    }

    #[test]
    fn clicking_outside_the_edited_note_still_commits_the_edit() {
        let mut a = editing("hello");
        a.on_mouse(mouse_down(a.viewport.x, a.viewport.y));
        assert_eq!(
            a.mode(),
            Mode::Nav,
            "a click on the board saves and leaves edit"
        );
    }

    // ---- undo / redo ----

    fn titles(a: &App) -> Vec<String> {
        a.active_board()
            .notes
            .iter()
            .map(|n| n.title.clone())
            .collect()
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
        let color_of = |a: &App| {
            a.active_board()
                .notes
                .iter()
                .find(|n| n.id == id)
                .unwrap()
                .color
        };
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
        assert_eq!(
            contents(&a),
            before,
            "one undo should take back the whole edit"
        );
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
        assert_ne!(
            contents(&a),
            deleted,
            "the no-op edit must not eat the undo"
        );
    }

    #[test]
    fn a_drag_is_one_undo_step() {
        let mut a = app();
        let note = &a.active_board().notes[0];
        let (id, x0, y0) = (note.id, note.x, note.y);
        // Aim at the note's centre: a corner rounds outside it at this zoom.
        let (sc, sr) = cell_of_note(&a, id);
        a.on_mouse(mouse_down(sc, sr));
        assert_eq!(
            a.selected(),
            Some(id),
            "the drag should have grabbed the note"
        );
        for step in 1..=5 {
            a.on_mouse(mouse_drag(sc + step, sr + step));
        }
        a.on_mouse(mouse_up(sc + 5, sr + 5));
        let moved = a.active_board().notes.iter().find(|n| n.id == id).unwrap();
        assert_ne!((moved.x, moved.y), (x0, y0), "the note should have moved");

        a.on_key(key(KeyCode::Char('u')));
        let back = a.active_board().notes.iter().find(|n| n.id == id).unwrap();
        assert_eq!(
            (back.x, back.y),
            (x0, y0),
            "one undo should take back the whole drag"
        );
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
        assert_eq!(
            titles(&a),
            after,
            "redo must not resurrect a discarded future"
        );
    }

    #[test]
    fn undo_and_redo_on_empty_stacks_do_nothing() {
        let mut a = app();
        let before = titles(&a);
        a.on_key(key(KeyCode::Char('u')));
        a.on_key(chord(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert_eq!(titles(&a), before);
        assert!(
            a.status().is_some_and(|s| s.contains("nothing")),
            "{:?}",
            a.status()
        );
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
        let color_of = |a: &App| {
            a.active_board()
                .notes
                .iter()
                .find(|n| n.id == id)
                .unwrap()
                .color
        };
        let oldest = color_of(&a);
        for _ in 0..(UNDO_DEPTH + 10) {
            a.on_key(key(KeyCode::Char('c')));
        }
        for _ in 0..(UNDO_DEPTH + 10) {
            a.on_key(key(KeyCode::Char('u')));
        }
        assert_ne!(
            color_of(&a),
            oldest,
            "the oldest steps should have been evicted"
        );
    }

    // ---- dropping a pin on another world ----

    /// An app with both a viewport and a tab strip placed, so a gesture can
    /// travel from the board to the tabs.
    fn with_tabs() -> App {
        let mut a = app();
        a.set_tabs_area(Rect {
            x: 0,
            y: 1,
            width: 100,
            height: 1,
        });
        a
    }

    /// The cell in the middle of world `index`'s tab.
    fn cell_of_tab(a: &App, index: usize) -> (u16, u16) {
        let tab = a
            .tabs()
            .into_iter()
            .find(|t| matches!(t.kind, TabKind::World { index: i, .. } if i == index))
            .expect("world tab");
        (a.tabs.x + tab.x + tab.width / 2, a.tabs.y)
    }

    /// Pick up the first pin on the active board and drag the cursor to `(col, row)`.
    fn drag_first_pin_to(a: &mut App, col: u16, row: u16) -> u64 {
        let id = a.active_board().notes[0].id;
        let (sc, sr) = cell_of_note(a, id);
        a.on_mouse(mouse_down(sc, sr));
        assert_eq!(
            a.selected(),
            Some(id),
            "the drag should have grabbed the pin"
        );
        a.on_mouse(mouse_drag(col, row));
        id
    }

    #[test]
    fn dropping_a_pin_on_another_worlds_tab_moves_it() {
        let mut a = with_tabs();
        let before_here = a.active_board().notes.len();
        let before_there = a.boards()[1].notes.len();
        let (tc, tr) = cell_of_tab(&a, 1);
        let id = drag_first_pin_to(&mut a, tc, tr);
        a.on_mouse(mouse_up(tc, tr));

        assert_eq!(
            a.active_board().notes.len(),
            before_here - 1,
            "left this board"
        );
        assert_eq!(
            a.boards()[1].notes.len(),
            before_there + 1,
            "landed on that one"
        );
        assert!(a.boards()[1].notes.iter().any(|n| n.id == id));
        assert_eq!(
            a.active_index(),
            0,
            "you stay on the board you were clearing"
        );
        assert_eq!(a.selected(), None, "the pin is not on this board any more");
        assert!(
            a.status().is_some_and(|s| s.contains("moved to")),
            "{:?}",
            a.status()
        );
    }

    /// Worlds do not share a coordinate space: a pin keeping its old x and y
    /// can land arbitrarily far from the board it arrives on, and since the
    /// camera frames the *union* of a board's pins, that strands both the pin
    /// and everything already there. It lands in the middle instead, cascaded
    /// clear of whatever the middle already holds.
    #[test]
    fn a_dropped_pin_lands_in_the_middle_of_the_world_it_arrives_in() {
        let mut a = with_tabs();
        let note = a.active_board().notes[0].clone();
        let top_before = a.boards()[1].notes.iter().map(|n| n.z).max().unwrap_or(0);
        let center = a.boards()[1].content_center();
        let wanted = WorldPoint {
            x: center.x - NOTE_W / 2.0,
            y: center.y - NOTE_H / 2.0,
        };
        let expected = a.boards()[1].free_spot(wanted, None);

        let (tc, tr) = cell_of_tab(&a, 1);
        drag_first_pin_to(&mut a, tc, tr);
        a.on_mouse(mouse_up(tc, tr));

        let moved = a.boards()[1]
            .notes
            .iter()
            .find(|n| n.id == note.id)
            .unwrap();
        assert_eq!(
            (moved.x, moved.y),
            (expected.x, expected.y),
            "should land at the target board's centre, not keep its old spot"
        );
        assert!(
            moved.z > top_before,
            "should sit on top of the target board"
        );
        assert_nothing_is_stacked(&a.boards()[1]);
    }

    #[test]
    fn a_note_does_not_move_while_the_cursor_is_over_the_tab_strip() {
        let mut a = with_tabs();
        let note = a.active_board().notes[0].clone();
        let (tc, tr) = cell_of_tab(&a, 1);
        drag_first_pin_to(&mut a, tc, tr);
        let held = a
            .active_board()
            .notes
            .iter()
            .find(|n| n.id == note.id)
            .unwrap();
        assert_eq!(
            (held.x, held.y),
            (note.x, note.y),
            "the pin must not fly up into the header"
        );
    }

    #[test]
    fn the_drop_target_arms_over_the_strip_and_clears_on_release() {
        let mut a = with_tabs();
        let (tc, tr) = cell_of_tab(&a, 1);
        drag_first_pin_to(&mut a, tc, tr);
        assert_eq!(
            a.drop_target().map(|d| d.world),
            Some(1),
            "the tab under the cursor is armed"
        );

        // Back onto the board: disarmed, and the pin follows the cursor again.
        let (bc, br) = (v_area(&a).x + 30, v_area(&a).y + 10);
        a.on_mouse(mouse_drag(bc, br));
        assert_eq!(a.drop_target(), None, "leaving the strip disarms it");
        a.on_mouse(mouse_up(bc, br));
        assert_eq!(a.drop_target(), None);
    }

    #[test]
    fn dropping_a_pin_on_its_own_tab_leaves_it_alone() {
        let mut a = with_tabs();
        let (tc, tr) = cell_of_tab(&a, 0); // the board we are already on
        let id = drag_first_pin_to(&mut a, tc, tr);
        // Captured after the grab, so this measures the drop and nothing else.
        let before = a.active_board().notes.clone();
        a.on_mouse(mouse_up(tc, tr));
        assert_eq!(
            a.active_board().notes,
            before,
            "a self-drop must change nothing at all, not even stack order"
        );
        assert_eq!(
            a.selected(),
            Some(id),
            "the pin is still here and still selected"
        );
    }

    #[test]
    fn the_plus_is_never_a_drop_target() {
        let mut a = with_tabs();
        let plus = a.tabs().last().unwrap().clone();
        let (tc, tr) = (a.tabs.x + plus.x + 1, a.tabs.y);
        let note = a.active_board().notes[0].clone();
        drag_first_pin_to(&mut a, tc, tr);
        assert_eq!(a.drop_target(), None, "the + is not a world to drop into");

        // It is still part of the strip, so the pin must not follow the cursor
        // up there either.
        let held = a
            .active_board()
            .notes
            .iter()
            .find(|n| n.id == note.id)
            .unwrap();
        assert_eq!(
            (held.x, held.y),
            (note.x, note.y),
            "the pin chased the cursor"
        );

        let before = a.active_board().notes.clone();
        a.on_mouse(mouse_up(tc, tr));
        assert_eq!(a.active_board().notes, before);
        assert_eq!(
            a.mode(),
            Mode::Nav,
            "a drop must not open the new-world prompt"
        );
    }

    #[test]
    fn moving_a_pin_to_another_world_is_one_undo_step() {
        let mut a = with_tabs();
        let before_here = titles(&a);
        let before_there = a.boards()[1].notes.len();
        let (tc, tr) = cell_of_tab(&a, 1);
        drag_first_pin_to(&mut a, tc, tr);
        a.on_mouse(mouse_up(tc, tr));
        assert_ne!(titles(&a), before_here);

        a.on_key(key(KeyCode::Char('u')));
        assert_eq!(titles(&a), before_here, "the pin should be back");
        assert_eq!(
            a.boards()[1].notes.len(),
            before_there,
            "and gone from the target"
        );
    }

    // ---- scrolling a note longer than itself ----

    /// One pin carrying `rows` short body lines, on a board of its own.
    fn board_with_note_of(rows: usize) -> App {
        let body = (1..=rows)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut board = Board::new("long");
        board.notes.push(Note {
            id: 1,
            title: "a long note".into(),
            body,
            x: 0.0,
            y: 0.0,
            z: 1,
            color: Color::Yellow,
        });
        let mut a = App::new(vec![board]);
        a.set_viewport(VIEWPORT);
        a
    }

    /// That pin, open in the editor. The caret lands where `TextEditor::new`
    /// puts it: at the very end of the text.
    fn editing_a_note_of(rows: usize) -> App {
        let mut a = board_with_note_of(rows);
        let (col, row) = cell_of_note(&a, 1);
        a.on_mouse(mouse_down(col, row));
        a.on_mouse(mouse_up(col, row));
        a.on_key(key(KeyCode::Char('e')));
        assert_eq!(a.mode(), Mode::Edit, "the note should be open");
        a
    }

    /// The window the note shows: (first visible row, rows that fit).
    fn window(a: &App) -> (usize, usize) {
        let (height, _) = a.text_extent(1).expect("a note is open");
        (a.scroll_of(1), height)
    }

    /// Which wrapped row the caret sits on.
    fn caret_row(a: &App) -> usize {
        let (_, wrapped) = a.edit_layout().expect("a note is open");
        wrapped.place(a.editor().unwrap().cursor()).0
    }

    fn wheel(down: bool, col: u16, row: u16) -> MouseEvent {
        let kind = if down {
            MouseEventKind::ScrollDown
        } else {
            MouseEventKind::ScrollUp
        };
        mouse_at(kind, col, row)
    }

    #[test]
    fn a_note_that_fits_never_scrolls() {
        let a = editing_a_note_of(2);
        assert_eq!(a.scroll_of(1), 0);
        assert_eq!(a.max_scroll_of(1), 0, "there is nothing below to reach");
    }

    #[test]
    fn opening_a_long_note_shows_the_end_the_caret_is_at() {
        // Without this the caret opens below the note and you type blind.
        let a = editing_a_note_of(40);
        let (top, height) = window(&a);
        assert!(top > 0, "a 41-row note cannot start at the top and show its end");
        let caret = caret_row(&a);
        assert!(
            (top..top + height).contains(&caret),
            "caret on row {caret}, window {top}..{}",
            top + height
        );
    }

    #[test]
    fn the_window_holds_still_while_the_caret_moves_inside_it() {
        let mut a = editing_a_note_of(40);
        let before = a.scroll_of(1);
        a.on_key(key(KeyCode::Up));
        assert_eq!(
            a.scroll_of(1),
            before,
            "a step the window already shows must not move the text"
        );
    }

    #[test]
    fn the_caret_walking_off_the_top_pulls_the_window_with_it() {
        let mut a = editing_a_note_of(40);
        let (_, height) = window(&a);
        for _ in 0..height + 5 {
            a.on_key(key(KeyCode::Up));
        }
        let (top, height) = window(&a);
        let caret = caret_row(&a);
        assert!(
            (top..top + height).contains(&caret),
            "caret on row {caret}, window {top}..{}",
            top + height
        );
    }

    #[test]
    fn typing_past_the_last_visible_row_follows_the_caret_down() {
        let mut a = editing_a_note_of(40);
        let before = a.scroll_of(1);
        for _ in 0..5 {
            a.on_key(key(KeyCode::Enter));
        }
        assert!(a.scroll_of(1) > before, "new rows must not push the caret out of sight");
        let (top, height) = window(&a);
        assert!((top..top + height).contains(&caret_row(&a)));
    }

    #[test]
    fn the_wheel_moves_the_text_and_leaves_the_caret_alone() {
        let mut a = editing_a_note_of(40);
        let before = a.scroll_of(1);
        let caret = a.editor().unwrap().cursor();
        let (col, row) = cell_of_note(&a, 1);
        a.on_mouse(wheel(false, col, row));
        assert_eq!(a.scroll_of(1), before - WHEEL_ROWS as usize);
        assert_eq!(
            a.editor().unwrap().cursor(),
            caret,
            "looking up the note is not moving the cursor"
        );
    }

    #[test]
    fn the_next_keystroke_brings_the_caret_back_into_view() {
        let mut a = editing_a_note_of(40);
        let (col, row) = cell_of_note(&a, 1);
        for _ in 0..10 {
            a.on_mouse(wheel(false, col, row));
        }
        a.on_key(key(KeyCode::Char('x')));
        let (top, height) = window(&a);
        assert!((top..top + height).contains(&caret_row(&a)));
    }

    #[test]
    fn scrolling_stops_at_both_ends_of_the_text() {
        let mut a = editing_a_note_of(40);
        let (col, row) = cell_of_note(&a, 1);
        for _ in 0..50 {
            a.on_mouse(wheel(false, col, row));
        }
        assert_eq!(a.scroll_of(1), 0, "the top is as far up as it goes");
        for _ in 0..50 {
            a.on_mouse(wheel(true, col, row));
        }
        let (top, height) = window(&a);
        let (_, rows) = a.text_extent(1).unwrap();
        assert_eq!(
            top,
            rows - height,
            "the last row belongs at the bottom edge, not scrolled past it"
        );
    }

    #[test]
    fn the_wheel_scrolls_instead_of_zooming_while_a_note_is_open() {
        let mut a = editing_a_note_of(40);
        let (col, row) = cell_of_note(&a, 1);
        a.on_mouse(wheel(true, col, row));
        assert_eq!(
            a.camera().zoom,
            ZoomLevel::Document,
            "a wheel inside a note is text movement, not a zoom"
        );
    }

    #[test]
    fn the_wheel_still_zooms_when_no_note_is_open() {
        let mut a = app_with_pins(&[(0.0, 0.0)]);
        let before = a.camera().zoom;
        a.on_mouse(wheel(false, 10, 10));
        assert_ne!(a.camera().zoom, before, "the board still zooms on the wheel");
    }

    #[test]
    fn a_click_lands_on_the_row_the_scroll_put_under_it() {
        let mut a = editing_a_note_of(40);
        let (cells, _) = a.edit_layout().unwrap();
        let top = a.scroll_of(1);
        assert!(top > 0, "the point of the test is a scrolled note");
        // The first row inside the border, which is row `top` of the text.
        a.on_mouse(mouse_down(
            (cells.x + 1) as u16,
            (cells.y + 1) as u16,
        ));
        assert_eq!(
            caret_row(&a),
            top,
            "a click must resolve against the same window the text is drawn in"
        );
    }

    #[test]
    fn closing_a_note_puts_the_window_back_to_the_top() {
        let mut a = editing_a_note_of(40);
        assert!(a.scroll_of(1) > 0);
        a.on_key(key(KeyCode::Esc));
        assert_eq!(a.mode(), Mode::Nav);
        assert_eq!(a.scroll_of(1), 0, "the next note opens at its own top");
    }

    // ---- the wheel picks its target by what is under it ----

    /// One pin of `rows` body lines, at document zoom, nothing selected and
    /// nothing open: the reading case.
    fn viewing_a_note_of(rows: usize) -> App {
        let mut a = board_with_note_of(rows);
        a.on_key(key(KeyCode::Char('+')));
        a.on_key(key(KeyCode::Char('+')));
        assert_eq!(a.zoom(), ZoomLevel::Document, "the body has to be drawn");
        a
    }

    /// A cell on the board with no note under it.
    fn empty_cell(a: &App) -> (u16, u16) {
        let (col, row) = (a.viewport.x + 1, a.viewport.y + 1);
        let world = a.view().world_at(col, row);
        assert!(
            a.active_board().note_at(world).is_none(),
            "this corner should be bare board"
        );
        (col, row)
    }

    #[test]
    fn the_wheel_over_a_long_note_scrolls_it_and_leaves_the_zoom_alone() {
        let mut a = viewing_a_note_of(40);
        let (col, row) = cell_of_note(&a, 1);
        a.on_mouse(wheel(true, col, row));
        assert_eq!(a.scroll_of(1), WHEEL_ROWS as usize, "the note moved");
        assert_eq!(a.zoom(), ZoomLevel::Document, "the board did not");
    }

    #[test]
    fn the_wheel_over_the_board_still_zooms() {
        let mut a = viewing_a_note_of(40);
        let (col, row) = empty_cell(&a);
        a.on_mouse(wheel(true, col, row));
        assert_eq!(a.zoom(), ZoomLevel::Preview, "bare board is the camera's");
        assert_eq!(a.scroll_of(1), 0, "and no note moved");
    }

    #[test]
    fn the_wheel_over_a_short_note_does_nothing_rather_than_zooming() {
        // Which gesture you get is decided by what is under the pointer, not
        // by how much someone wrote. A pin that zoomed the world out while its
        // longer neighbour scrolled is a wheel you cannot predict.
        let mut a = viewing_a_note_of(1);
        let (col, row) = cell_of_note(&a, 1);
        a.on_mouse(wheel(true, col, row));
        assert_eq!(a.zoom(), ZoomLevel::Document, "the board held still");
        assert_eq!(a.scroll_of(1), 0, "and there was nothing to move");
    }

    #[test]
    fn a_note_holds_the_wheel_from_preview_zoom_up() {
        // Preview is the first level that draws a body, so it is the first
        // where the wheel can mean "read this pin".
        let mut a = board_with_note_of(40);
        a.on_key(key(KeyCode::Char('+')));
        assert_eq!(a.zoom(), ZoomLevel::Preview);
        let (col, row) = cell_of_note(&a, 1);
        a.on_mouse(wheel(true, col, row));
        assert_eq!(a.zoom(), ZoomLevel::Preview, "the board held still");
        assert_eq!(a.scroll_of(1), WHEEL_ROWS as usize, "the pin moved");
    }

    #[test]
    fn a_short_note_being_edited_holds_the_wheel_too() {
        let mut a = editing_a_note_of(1);
        let (col, row) = cell_of_note(&a, 1);
        a.on_mouse(wheel(true, col, row));
        assert_eq!(
            a.zoom(),
            ZoomLevel::Document,
            "the board must not move under a note you are writing in"
        );
    }

    #[test]
    fn zoomed_out_past_the_body_the_wheel_is_the_boards() {
        // Cluster and titles are where notes cover the screen. A wheel that
        // died on every one of them would leave a full board impossible to
        // zoom out of, and a note there is a block, not something you read.
        let mut a = board_with_note_of(40);
        assert_eq!(a.zoom(), ZoomLevel::Titles);
        let (col, row) = cell_of_note(&a, 1);
        a.on_mouse(wheel(false, col, row));
        assert_eq!(a.zoom(), ZoomLevel::Preview);
        assert_eq!(a.scroll_of(1), 0);
    }

    #[test]
    fn each_note_keeps_its_own_place() {
        let mut a = viewing_a_note_of(40);
        // A second long pin, well clear of the first.
        let body = (1..=40).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        a.boards[0].notes.push(Note {
            id: 2,
            title: "another".into(),
            body,
            x: 400.0,
            y: 0.0,
            z: 2,
            color: Color::Yellow,
        });
        let first = cell_of_note(&a, 1);
        let second = cell_of_note(&a, 2);
        a.on_mouse(wheel(true, first.0, first.1));
        a.on_mouse(wheel(true, second.0, second.1));
        a.on_mouse(wheel(true, second.0, second.1));
        assert_eq!(a.scroll_of(1), WHEEL_ROWS as usize);
        assert_eq!(a.scroll_of(2), 2 * WHEEL_ROWS as usize, "one window each");
    }

    #[test]
    fn a_shrunken_note_does_not_stay_scrolled_past_its_end() {
        let mut a = viewing_a_note_of(40);
        let (col, row) = cell_of_note(&a, 1);
        for _ in 0..20 {
            a.on_mouse(wheel(true, col, row));
        }
        assert!(a.scroll_of(1) > 0);
        a.boards[0].notes[0].body = "one line".into();
        assert_eq!(a.scroll_of(1), 0, "the window follows the text back up");
    }

    #[test]
    fn page_keys_move_the_selected_note_a_screen_at_a_time() {
        let mut a = viewing_a_note_of(40);
        let (col, row) = cell_of_note(&a, 1);
        a.on_mouse(mouse_down(col, row));
        a.on_mouse(mouse_up(col, row));
        assert_eq!(a.selected(), Some(1));
        let (height, _) = a.text_extent(1).unwrap();
        a.on_key(key(KeyCode::PageDown));
        assert_eq!(
            a.scroll_of(1),
            height - 1,
            "a page keeps one row of overlap to land on"
        );
        a.on_key(key(KeyCode::PageUp));
        assert_eq!(a.scroll_of(1), 0);
    }

    #[test]
    fn page_keys_do_nothing_with_no_pin_selected() {
        let mut a = viewing_a_note_of(40);
        a.on_key(key(KeyCode::PageDown));
        assert_eq!(a.scroll_of(1), 0);
        assert_eq!(a.zoom(), ZoomLevel::Document, "and they are not a zoom");
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
