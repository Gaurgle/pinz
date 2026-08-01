//! Drawing. Reads [`App`] state and paints a frame; holds no state of its own.
//!
//! The board uses the split render path DESIGN calls for: zoomed out
//! (survey/cluster) notes are cheap shapes painted straight into the buffer;
//! zoomed in (titles/preview/document) they are real bordered widgets with text.
//! The projection layer ([`View`]) decides where everything lands; this module
//! only decides how it looks at each zoom.
//!
//! All color comes from the active [`Theme`], never a hardcoded constant, so a
//! theme swap re-skins every widget with no other change.

use pinz_core::{Note, WorldPoint, ZoomLevel};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget, Wrap},
    Frame,
};

use crate::app::{App, Mode};
use crate::editor::Cursor;
use crate::theme::Theme;
use crate::view::{CellRect, View};

/// Paint the whole app: header, tabs, board, footer.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let rows = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Length(1), // world tabs
        Constraint::Min(3),    // board
        Constraint::Length(1), // footer
    ])
    .split(area);

    // Let the app know how big the board is (also triggers first-time centering)
    // before we draw it.
    app.set_viewport(rows[2]);

    let theme = app.theme();
    draw_header(frame, rows[0], app, &theme);
    draw_tabs(frame, rows[1], app, &theme);
    draw_board(frame, rows[2], app, &theme);
    draw_footer(frame, rows[3], app, &theme);
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let cols = Layout::horizontal([Constraint::Min(10), Constraint::Length(10)]).split(area);

    let brand = Line::from(vec![
        Span::styled(
            "📌 pinz",
            Style::new().fg(theme.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "  — terminal bulletin board",
            Style::new().fg(theme.overlay1),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(brand).style(Style::new().bg(theme.mantle)),
        cols[0],
    );

    // Zoom indicator: one dot per level, filled up to the current one. No label -
    // the dots say how far in you are, and what the levels are called is the
    // renderer's business, not something to spend header width on.
    let idx = app.zoom().index();
    let mut spans = Vec::new();
    for i in 0..ZoomLevel::ALL.len() {
        let (glyph, color) = if i <= idx {
            ("●", theme.accent)
        } else {
            ("○", theme.surface1)
        };
        spans.push(Span::styled(glyph, Style::new().fg(color)));
        spans.push(Span::raw(" "));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans))
            .alignment(Alignment::Right)
            .style(Style::new().bg(theme.mantle)),
        cols[1],
    );
}

fn draw_tabs(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let mut spans = vec![Span::raw(" ")];
    for (i, board) in app.boards().iter().enumerate() {
        let active = i == app.active_index();
        let (fg, modifier) = if active {
            (theme.text, Modifier::BOLD)
        } else {
            (theme.subtext, Modifier::empty())
        };
        let marker = if active { "▎" } else { " " };
        spans.push(Span::styled(marker, Style::new().fg(theme.accent)));
        spans.push(Span::styled(
            format!("{} ", board.name),
            Style::new().fg(fg).add_modifier(modifier),
        ));
        spans.push(Span::styled(
            format!("{}  ", board.notes.len()),
            Style::new().fg(theme.overlay0),
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::new().bg(theme.mantle)),
        area,
    );
}

fn draw_board(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    // Background wash.
    frame.render_widget(Block::new().style(Style::new().bg(theme.base)), area);
    draw_grid(frame, area, app, theme);

    let view = View::new(app.camera(), area);

    // Notes bottom-of-stack first, so higher z overlaps lower.
    let mut notes: Vec<&Note> = app.active_board().notes.iter().collect();
    notes.sort_by_key(|n| n.z);

    match app.zoom() {
        ZoomLevel::Cluster => draw_cluster(frame, &view, &notes, theme),
        lod => {
            for note in &notes {
                let cells = view.note_cells(note.position());
                if cells.clip(area).is_some() {
                    draw_note_widget(frame, cells, area, note, lod, app, theme);
                }
            }
        }
    }

    draw_scrollbars(frame, area, app, theme);
}

/// The dot grid, aligned to world coordinates so it slides under the notes as
/// you pan - a cheap sense of place. Skipped when the grid would be too dense to
/// read.
fn draw_grid(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    const GRID_WORLD: f64 = 48.0;
    let view = View::new(app.camera(), area);
    let (sx, _sy) = view.scale();
    let step_cells = GRID_WORLD * sx;
    if step_cells < 3.0 {
        return; // too busy; let the wash speak for itself
    }

    let origin = app.camera().origin;
    // First grid line at or after the camera origin, in world space.
    let start_x = (origin.x / GRID_WORLD).ceil() * GRID_WORLD;
    let start_y = (origin.y / GRID_WORLD).ceil() * GRID_WORLD;

    let buf = frame.buffer_mut();
    let mut gx = start_x;
    loop {
        let (cx, _) = view.cell_of(WorldPoint { x: gx, y: origin.y });
        if cx >= area.width as f64 {
            break;
        }
        let mut gy = start_y;
        loop {
            let (_, cy) = view.cell_of(WorldPoint { x: gx, y: gy });
            if cy >= area.height as f64 {
                break;
            }
            let col = area.x + cx as u16;
            let row = area.y + cy as u16;
            if let Some(cell) = buf.cell_mut((col, row)) {
                cell.set_symbol("·").set_fg(theme.surface0);
            }
            gy += GRID_WORLD;
        }
        gx += GRID_WORLD;
    }
}

/// Zoomed-out path (cluster level): each note is a solid colored block, no text -
/// the whole-board overview.
fn draw_cluster(frame: &mut Frame, view: &View, notes: &[&Note], theme: &Theme) {
    for note in notes {
        if let Some(rect) = view.note_rect(note.position()) {
            let color = theme.note(note.color);
            frame.render_widget(Block::new().style(Style::new().bg(color)), rect);
        }
    }
}

/// Zoomed-in path: a real post-it - bordered, colored, with text sized to the
/// zoom level. The document level is where the selected note is editable.
///
/// `cells` is the note's *full* footprint even when it hangs off the viewport;
/// the note is laid out at that size and only then cut down to `area`. That is
/// what makes it behave like paper sliding under the window frame instead of a
/// box that reflows its text as it reaches the edge.
fn draw_note_widget(
    frame: &mut Frame,
    cells: CellRect,
    area: Rect,
    note: &Note,
    lod: ZoomLevel,
    app: &App,
    theme: &Theme,
) {
    let color = theme.note(note.color);
    let selected = app.selected() == Some(note.id);
    let editing = selected && app.mode() == Mode::Edit;

    let border_style = if selected {
        Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.note_fg)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(Span::styled("📌", Style::new().fg(theme.note_fg)))
        .style(Style::new().bg(color).fg(theme.note_fg));

    // Everything below paints into note-local coordinates: a rect the size of the
    // whole note, at (0,0). `paint_clipped` puts the visible window of it on
    // screen.
    let full = Rect {
        x: 0,
        y: 0,
        width: cells.width,
        height: cells.height,
    };
    // Compute the inner area before the block is consumed by render. Clear first
    // so the note is opaque: a Block only restyles cells, it won't wipe the grid
    // dots (or a note underneath) showing through its interior.
    let inner = block.inner(full);

    paint_clipped(frame, cells, area, |buf| {
        Clear.render(full, buf);
        block.render(full, buf);
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        // While editing, the whole note is one live buffer (line 1 = title, the
        // rest = body), word-wrapped to the inner width with the cursor mapped
        // through the wrap so text never runs off the right edge.
        if let Some(editor) = editing.then(|| app.editor()).flatten() {
            let lines =
                editor_lines(editor.lines(), editor.cursor(), inner.width as usize, theme.note_fg);
            Paragraph::new(lines).render(inner, buf);
            return;
        }

        // Not editing: the title, then (from preview level up) the body. The body
        // is split on its own newlines first - a Line renders an embedded '\n' as
        // nothing, so passing the raw body would silently glue the rows together
        // the moment the editor closes.
        let mut lines = vec![Line::from(Span::styled(
            note.title.clone(),
            Style::new().fg(theme.note_fg).add_modifier(Modifier::BOLD),
        ))];
        if matches!(lod, ZoomLevel::Preview | ZoomLevel::Document) && !note.body.is_empty() {
            lines.push(Line::raw(""));
            lines.extend(body_lines(&note.body, theme.note_fg));
        }
        Paragraph::new(lines).wrap(Wrap { trim: true }).render(inner, buf);
    });
}

/// Paint `render` into an off-screen buffer the size of `cells`, then copy the
/// part of it that falls inside `area` onto the frame.
///
/// The detour through a private buffer is the whole point: widgets lay
/// themselves out to the rect they are given, so handing one a rect already
/// trimmed to the screen edge would re-wrap its text every time the note moved.
/// Here the layout is computed once at full size and the edge only ever cuts.
fn paint_clipped(
    frame: &mut Frame,
    cells: CellRect,
    area: Rect,
    render: impl FnOnce(&mut Buffer),
) {
    let Some(visible) = cells.clip(area) else {
        return;
    };
    if cells.width == 0 || cells.height == 0 {
        return;
    }
    let mut local = Buffer::empty(Rect {
        x: 0,
        y: 0,
        width: cells.width,
        height: cells.height,
    });
    render(&mut local);

    // Offset of the visible window within the note.
    let dx = (visible.x as i64 - cells.x) as u16;
    let dy = (visible.y as i64 - cells.y) as u16;
    let buf = frame.buffer_mut();
    for row in 0..visible.height {
        for col in 0..visible.width {
            let src = local[(dx + col, dy + row)].clone();
            if let Some(dst) = buf.cell_mut((visible.x + col, visible.y + row)) {
                *dst = src;
            }
        }
    }
}

/// A note body as one [`Line`] per hard line break, so the rows a writer typed
/// survive into the read-only view. Long rows still soft-wrap: the caller's
/// [`Wrap`] handles each line on its own.
fn body_lines(body: &str, fg: ratatui::style::Color) -> Vec<Line<'static>> {
    body.split('\n')
        .map(|row| Line::from(Span::styled(row.to_string(), Style::new().fg(fg))))
        .collect()
}

/// The editor's logical lines, wrapped to `width` and rendered for the document
/// view: line 1 (the title) is bold, and the cursor shows as a reversed cell,
/// placed through the wrap so it stays on screen on long lines.
fn editor_lines(
    logical: &[String],
    cursor: Cursor,
    width: usize,
    fg: ratatui::style::Color,
) -> Vec<Line<'static>> {
    let wrapped = wrap_rows(logical, cursor, width);
    wrapped
        .rows
        .iter()
        .enumerate()
        .map(|(vr, (text, li))| {
            let caret = (vr == wrapped.caret_row).then_some(wrapped.caret_col);
            row_line(text, *li == 0, fg, caret)
        })
        .collect()
}

/// One rendered row: bold when it belongs to the title line, with an optional
/// reversed-cell caret at `caret_col` (a block caret past the last character).
fn row_line(
    text: &str,
    title: bool,
    fg: ratatui::style::Color,
    caret_col: Option<usize>,
) -> Line<'static> {
    let mut base = Style::new().fg(fg);
    if title {
        base = base.add_modifier(Modifier::BOLD);
    }
    let Some(col) = caret_col else {
        return Line::from(Span::styled(text.to_string(), base));
    };
    let chars: Vec<char> = text.chars().collect();
    let col = col.min(chars.len());
    let caret = base.add_modifier(Modifier::REVERSED);
    let before: String = chars[..col].iter().collect();
    let mut spans = vec![Span::styled(before, base)];
    if col < chars.len() {
        spans.push(Span::styled(chars[col].to_string(), caret));
        spans.push(Span::styled(chars[col + 1..].iter().collect::<String>(), base));
    } else {
        spans.push(Span::styled(" ".to_string(), caret));
    }
    Line::from(spans)
}

/// Editor lines wrapped to a width, plus where the cursor lands in that layout.
struct EditWrap {
    /// (visual row text, source logical line index).
    rows: Vec<(String, usize)>,
    caret_row: usize,
    caret_col: usize,
}

/// Greedy word wrap: break after spaces, hard-break words longer than the width,
/// and track the cursor's visual position. Spaces are kept at the end of a
/// wrapped row so every character maps to exactly one visual cell - which is what
/// lets the caret land precisely after wrapping.
fn wrap_rows(logical: &[String], cursor: Cursor, width: usize) -> EditWrap {
    let width = width.max(1);
    let mut rows: Vec<(String, usize)> = Vec::new();
    let mut caret_row = 0;
    let mut caret_col = 0;
    let mut caret_done = false;

    for (li, line) in logical.iter().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        if chars.is_empty() {
            if li == cursor.row && !caret_done {
                caret_row = rows.len();
                caret_col = 0;
                caret_done = true;
            }
            rows.push((String::new(), li));
            continue;
        }
        let mut start = 0;
        while start < chars.len() {
            let hard_end = (start + width).min(chars.len());
            let end = if hard_end == chars.len() {
                chars.len()
            } else {
                match chars[start..hard_end].iter().rposition(|&c| c == ' ') {
                    Some(rel) if rel > 0 => start + rel + 1,
                    _ => hard_end,
                }
            };
            if li == cursor.row && !caret_done {
                let last = end == chars.len();
                if cursor.col < end || (last && cursor.col <= end) {
                    caret_row = rows.len();
                    caret_col = cursor.col - start;
                    caret_done = true;
                }
            }
            rows.push((chars[start..end].iter().collect(), li));
            start = end;
        }
    }

    if rows.is_empty() {
        rows.push((String::new(), 0));
    }
    if !caret_done {
        caret_row = rows.len() - 1;
        caret_col = rows.last().map(|(t, _)| t.chars().count()).unwrap_or(0);
    }
    EditWrap { rows, caret_row, caret_col }
}

/// Thin position indicators along the right and bottom edges, sized and placed
/// like the demo's scrollbars: they show where the camera sits in the note
/// cloud. Hidden on an axis when everything already fits.
fn draw_scrollbars(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let Some((min, max)) = content_extent(app) else {
        return;
    };
    let view = View::new(app.camera(), area);
    let (sx, sy) = view.scale();
    let origin = app.camera().origin;

    let world_w = (max.x - min.x).max(1.0);
    let world_h = (max.y - min.y).max(1.0);
    let view_w = area.width as f64 / sx;
    let view_h = area.height as f64 / sy;

    // Vertical bar on the right edge.
    if view_h < world_h && area.height > 2 {
        let track = area.height as f64;
        let frac = (view_h / world_h).clamp(0.05, 1.0);
        let pos = ((origin.y - min.y) / world_h).clamp(0.0, 1.0 - frac);
        let len = (frac * track).round().max(1.0) as u16;
        let top = area.y + (pos * track).round() as u16;
        let col = area.x + area.width - 1;
        paint_bar(frame, col, top, len, true, area, theme.overlay0);
    }

    // Horizontal bar on the bottom edge.
    if view_w < world_w && area.width > 2 {
        let track = area.width as f64;
        let frac = (view_w / world_w).clamp(0.05, 1.0);
        let pos = ((origin.x - min.x) / world_w).clamp(0.0, 1.0 - frac);
        let len = (frac * track).round().max(1.0) as u16;
        let left = area.x + (pos * track).round() as u16;
        let row = area.y + area.height - 1;
        paint_bar(frame, left, row, len, false, area, theme.overlay0);
    }
}

fn paint_bar(
    frame: &mut Frame,
    x: u16,
    y: u16,
    len: u16,
    vertical: bool,
    area: Rect,
    color: ratatui::style::Color,
) {
    let buf = frame.buffer_mut();
    for i in 0..len {
        let (cx, cy) = if vertical { (x, y + i) } else { (x + i, y) };
        if cx < area.x + area.width && cy < area.y + area.height {
            if let Some(cell) = buf.cell_mut((cx, cy)) {
                cell.set_symbol(if vertical { "▐" } else { "▄" })
                    .set_fg(color);
            }
        }
    }
}

fn content_extent(app: &App) -> Option<(WorldPoint, WorldPoint)> {
    let notes = &app.active_board().notes;
    let first = notes.first()?;
    let mut min = WorldPoint {
        x: first.x,
        y: first.y,
    };
    let mut max = WorldPoint {
        x: first.x + pinz_core::NOTE_W,
        y: first.y + pinz_core::NOTE_H,
    };
    for n in notes {
        min.x = min.x.min(n.x);
        min.y = min.y.min(n.y);
        max.x = max.x.max(n.x + pinz_core::NOTE_W);
        max.y = max.y.max(n.y + pinz_core::NOTE_H);
    }
    Some((min, max))
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let hint = match app.mode() {
        Mode::Edit => Line::from(vec![
            Span::styled(
                "editing",
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  ", Style::new().fg(theme.overlay0)),
            Span::styled("line 1 is the title", Style::new().fg(theme.overlay0)),
            Span::styled("  ·  ", Style::new().fg(theme.overlay0)),
            key_hint("enter", "newline", theme.overlay1),
            key_hint("↑↓←→", "move", theme.overlay1),
            key_hint("alt+←→", "word", theme.overlay1),
            key_hint("ctrl+⌫", "del word", theme.overlay1),
            key_hint("esc", "save", theme.overlay1),
        ]),
        Mode::Nav => Line::from(vec![
            key_hint("scroll/±", "zoom", theme.overlay1),
            key_hint("drag", "move/pan", theme.overlay1),
            key_hint("n", "new", theme.overlay1),
            key_hint("e", "edit", theme.overlay1),
            key_hint("c", "color", theme.overlay1),
            key_hint("d", "del", theme.overlay1),
            key_hint("tab", "world", theme.overlay1),
            key_hint("t", &format!("theme:{}", theme.name), theme.overlay1),
            key_hint("q", "quit", theme.overlay1),
        ]),
    };
    frame.render_widget(
        Paragraph::new(hint).style(Style::new().bg(theme.mantle)),
        area,
    );
}

fn key_hint(key: &str, label: &str, color: ratatui::style::Color) -> Span<'static> {
    Span::styled(format!(" {key} {label} "), Style::new().fg(color))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinz_core::{MemoryStore, Store};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn render_with(theme_name: Option<&str>) -> ratatui::buffer::Buffer {
        let mut store = MemoryStore::seeded();
        let mut app = App::new(store.load().unwrap());
        if let Some(name) = theme_name {
            app.set_theme_by_name(name);
        }
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn renders_brand_tabs_and_a_note_title() {
        let buf = render_with(None);
        let text = buffer_text(&buf);
        assert!(text.contains("pinz"), "brand missing:\n{text}");
        assert!(text.contains("ideas"), "world tab missing:\n{text}");
        assert!(text.contains("sketches"), "world tab missing:\n{text}");
        // A seeded note title should be on the board at titles zoom.
        assert!(text.contains("welcome"), "note title missing:\n{text}");
    }

    #[test]
    fn footer_shows_the_active_theme_name() {
        let buf = render_with(Some("nord"));
        let text = buffer_text(&buf);
        assert!(text.contains("Nord"), "active theme name missing:\n{text}");
    }

    #[test]
    fn a_light_theme_paints_a_light_board_background() {
        // Solarized Light's base is near-white; the board wash should carry it,
        // proving color really flows from the theme, not a constant.
        let buf = render_with(Some("light"));
        let board_cell = &buf[(2, 10)]; // somewhere on the board area
        assert_eq!(board_cell.bg, ratatui::style::Color::Rgb(0xfd, 0xf6, 0xe3));
    }

    #[test]
    fn editing_renders_title_and_typed_body() {
        use ratatui::crossterm::event::{
            KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers,
        };
        let mut store = MemoryStore::seeded();
        let mut app = App::new(store.load().unwrap());
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap(); // establish viewport + centering

        let press = |code| KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        app.on_key(press(KeyCode::Char('n'))); // new note -> edit, "new note"
        app.on_key(press(KeyCode::Enter)); // newline -> start the body
        for ch in "hello".chars() {
            app.on_key(press(KeyCode::Char(ch)));
        }
        terminal.draw(|f| draw(f, &mut app)).unwrap();

        let text = buffer_text(&terminal.backend().buffer().clone());
        assert!(text.contains("new note"), "title should be on screen:\n{text}");
        assert!(text.contains("hello"), "typed body should be on screen:\n{text}");
    }

    #[test]
    fn a_saved_body_keeps_its_line_breaks_on_the_board() {
        let mut store = MemoryStore::seeded();
        let mut boards = store.load().unwrap();
        boards[0].notes.clear();
        boards[0].notes.push(pinz_core::Note {
            id: 900,
            title: "TITLE".into(),
            body: "AAA\n\nBBB".into(),
            x: 100.0,
            y: 100.0,
            z: 1,
            color: pinz_core::Color::Yellow,
        });
        let mut app = App::new(boards);
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap(); // establish viewport + centering

        // Zoom to document, where the body is drawn.
        for _ in 0..4 {
            app.on_key(ratatui::crossterm::event::KeyEvent::new(
                ratatui::crossterm::event::KeyCode::Char('+'),
                ratatui::crossterm::event::KeyModifiers::NONE,
            ));
        }
        terminal.draw(|f| draw(f, &mut app)).unwrap();

        let text = buffer_text(&terminal.backend().buffer().clone());
        assert!(text.contains("AAA"), "body missing:\n{text}");
        assert!(text.contains("BBB"), "body missing:\n{text}");
        assert!(
            !text.contains("AAABBB"),
            "line breaks were swallowed into one row:\n{text}"
        );
        let rows: Vec<&str> = text.lines().collect();
        let a = rows.iter().position(|r| r.contains("AAA")).unwrap();
        let b = rows.iter().position(|r| r.contains("BBB")).unwrap();
        assert_eq!(b - a, 2, "the blank line between them is preserved");
    }

    /// Draw a single wordy note at document zoom on a terminal barely wider than
    /// the note, then pan right `presses` times so the note hangs off the left
    /// edge. Returns the board's text rows.
    fn panned_board_rows(presses: usize) -> Vec<String> {
        let press = |code| {
            ratatui::crossterm::event::KeyEvent::new(
                code,
                ratatui::crossterm::event::KeyModifiers::NONE,
            )
        };
        let mut store = MemoryStore::seeded();
        let mut boards = store.load().unwrap();
        boards[0].notes.clear();
        boards[0].notes.push(pinz_core::Note {
            id: 900,
            title: "TITLE".into(),
            body: "alpha beta gamma delta epsilon zeta eta theta".into(),
            x: 0.0,
            y: 0.0,
            z: 1,
            color: pinz_core::Color::Yellow,
        });
        let mut app = App::new(boards);
        let mut terminal = Terminal::new(TestBackend::new(44, 24)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap(); // viewport + centering
        for _ in 0..4 {
            app.on_key(press(ratatui::crossterm::event::KeyCode::Char('+')));
        }
        for _ in 0..presses {
            app.on_key(press(ratatui::crossterm::event::KeyCode::Right));
        }
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        buffer_text(&terminal.backend().buffer().clone())
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// The text inside a note's borders on one screen row: between the two
    /// border glyphs when both are on screen, otherwise whatever is left of the
    /// single one. Empty for rows that aren't part of a note.
    fn note_interior(row: &str) -> String {
        let Some(right) = row.rfind('│') else {
            return String::new();
        };
        let seg = &row[..right];
        match seg.rfind('│') {
            Some(left) => seg[left + '│'.len_utf8()..].trim().to_string(),
            None => seg.trim().to_string(),
        }
    }

    #[test]
    fn a_note_at_the_edge_does_not_reflow_its_text() {
        // Same note, same camera height, panned so more of it hangs off the left
        // edge. Panning may only ever *cut* a row - never re-wrap it into a new
        // shape - so each row still on screen must be a tail of what it was.
        let whole = panned_board_rows(0);
        let cut = panned_board_rows(8);
        assert_eq!(whole.len(), cut.len(), "same terminal, same row count");

        let mut narrowed = 0;
        for (whole_row, cut_row) in whole.iter().zip(&cut) {
            let (before, after) = (note_interior(whole_row), note_interior(cut_row));
            if after.is_empty() {
                continue; // row is blank, or cut away entirely - both are fine
            }
            assert!(
                before.ends_with(&after),
                "row re-wrapped at the edge: {after:?} is not the tail of {before:?}"
            );
            if before != after {
                narrowed += 1;
            }
        }
        assert!(
            narrowed > 0,
            "the note never reached the edge; the test proves nothing:\n{cut:#?}"
        );
    }

    #[test]
    fn wrap_keeps_all_characters_and_bounds_the_width() {
        let lines = vec!["hello world foo".to_string()];
        let w = wrap_rows(&lines, Cursor { row: 0, col: 15 }, 8);
        let joined: String = w.rows.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(joined, "hello world foo", "no characters lost to wrapping");
        assert!(w.rows.iter().all(|(t, _)| t.chars().count() <= 8));
        assert_eq!((w.caret_row, w.caret_col), (2, 3), "cursor at end tracks to last row");
    }

    #[test]
    fn wrap_maps_a_cursor_inside_the_first_row() {
        let lines = vec!["hello world foo".to_string()];
        let w = wrap_rows(&lines, Cursor { row: 0, col: 3 }, 8);
        assert_eq!((w.caret_row, w.caret_col), (0, 3));
    }

    #[test]
    fn wrap_labels_title_vs_body_rows_and_keeps_blanks() {
        let lines = vec!["title".to_string(), String::new(), "body".to_string()];
        let w = wrap_rows(&lines, Cursor { row: 2, col: 4 }, 20);
        assert_eq!(w.rows.len(), 3, "blank line is preserved");
        assert_eq!(w.rows[0].1, 0, "row 0 is the title line");
        assert_eq!(w.rows[2].1, 2);
        assert_eq!((w.caret_row, w.caret_col), (2, 4));
    }

    #[test]
    fn tiny_terminal_does_not_panic() {
        let mut store = MemoryStore::seeded();
        let mut app = App::new(store.load().unwrap());
        let mut terminal = Terminal::new(TestBackend::new(8, 5)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
    }
}
