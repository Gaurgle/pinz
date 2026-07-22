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
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::app::{App, Field};
use crate::theme::Theme;
use crate::view::View;

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
    let cols = Layout::horizontal([Constraint::Min(10), Constraint::Length(28)]).split(area);

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

    // Zoom indicator: five dots filled up to the current level, then the label.
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
    spans.push(Span::styled(
        format!("{:<8}", app.zoom().label()),
        Style::new().fg(theme.overlay1),
    ));
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
        ZoomLevel::Survey => draw_far(frame, area, &view, &notes, false, theme),
        ZoomLevel::Cluster => draw_far(frame, area, &view, &notes, true, theme),
        lod => {
            for note in &notes {
                if let Some(rect) = view.note_rect(note.position()) {
                    draw_note_widget(frame, rect, note, lod, app, theme);
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

/// Zoomed-out path: a note is a shape, not a widget. `filled` distinguishes the
/// cluster level (solid colored blocks) from survey (a single colored pip).
fn draw_far(
    frame: &mut Frame,
    area: Rect,
    view: &View,
    notes: &[&Note],
    filled: bool,
    theme: &Theme,
) {
    for note in notes {
        let color = theme.note(note.color);
        if filled {
            if let Some(rect) = view.note_rect(note.position()) {
                frame.render_widget(Block::new().style(Style::new().bg(color)), rect);
            }
        } else {
            let (cx, cy) = view.cell_of(note.center());
            if cx < 0.0 || cy < 0.0 || cx >= area.width as f64 || cy >= area.height as f64 {
                continue;
            }
            let col = area.x + cx as u16;
            let row = area.y + cy as u16;
            if let Some(cell) = frame.buffer_mut().cell_mut((col, row)) {
                cell.set_symbol("●").set_fg(color);
            }
        }
    }
}

/// Zoomed-in path: a real post-it - bordered, colored, with text sized to the
/// zoom level. The document level is where the selected note is editable.
fn draw_note_widget(
    frame: &mut Frame,
    rect: Rect,
    note: &Note,
    lod: ZoomLevel,
    app: &App,
    theme: &Theme,
) {
    let color = theme.note(note.color);
    let selected = app.selected() == Some(note.id);

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

    // Compute the inner area before the block is consumed by render. Clear first
    // so the note is opaque: a Block only restyles cells, it won't wipe the grid
    // dots (or a note underneath) showing through its interior.
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    // Text comes from the app: the live edit buffer (with a caret) for whichever
    // field is being typed into, otherwise the note's own text.
    let (title, _) = app.field_display(note, Field::Title);
    let (body, body_active) = app.field_display(note, Field::Body);

    let mut lines = vec![Line::from(Span::styled(
        title,
        Style::new().fg(theme.note_fg).add_modifier(Modifier::BOLD),
    ))];

    // Body appears from preview level up (and always while it's being edited, so
    // the caret is visible even in an empty body). Split on newlines so a
    // multi-line body renders as multiple lines.
    if matches!(lod, ZoomLevel::Preview | ZoomLevel::Document) && (!body.is_empty() || body_active)
    {
        lines.push(Line::raw(""));
        for bl in body.split('\n') {
            lines.push(Line::from(Span::styled(
                bl.to_string(),
                Style::new().fg(theme.note_fg),
            )));
        }
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
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
    let hint = if let Some(field) = app.edit_field() {
        Line::from(vec![
            Span::styled(
                format!("editing {}", field.label()),
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  ", Style::new().fg(theme.overlay0)),
            key_hint("↑↓←→", "move", theme.overlay1),
            key_hint("tab", "title/body", theme.overlay1),
            key_hint("enter", "newline", theme.overlay1),
            key_hint("esc", "done", theme.overlay1),
        ])
    } else {
        Line::from(vec![
            key_hint("scroll/±", "zoom", theme.overlay1),
            key_hint("drag", "move/pan", theme.overlay1),
            key_hint("n", "new", theme.overlay1),
            key_hint("e", "edit", theme.overlay1),
            key_hint("d", "del", theme.overlay1),
            key_hint("tab", "world", theme.overlay1),
            key_hint("t", &format!("theme:{}", theme.name), theme.overlay1),
            key_hint("q", "quit", theme.overlay1),
        ])
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
        assert!(text.contains("wavez"), "world tab missing:\n{text}");
        // A seeded note title should be on the board at titles zoom.
        assert!(text.contains("Fortnox"), "note title missing:\n{text}");
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
    fn editing_a_body_draws_the_typed_text_with_a_caret() {
        use ratatui::crossterm::event::{
            KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers,
        };
        let press = |c: KeyCode| KeyEvent {
            code: c,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };

        let mut store = MemoryStore::seeded();
        let mut app = App::new(store.load().unwrap());
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap(); // center

        app.on_key(press(KeyCode::Char('n'))); // new note, editing the title
        app.on_key(press(KeyCode::Tab)); // -> body
        for c in "hello".chars() {
            app.on_key(press(KeyCode::Char(c)));
        }
        terminal.draw(|f| draw(f, &mut app)).unwrap();

        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("hello"), "typed body missing:\n{text}");
        assert!(text.contains('▏'), "edit caret missing:\n{text}");
    }

    #[test]
    fn tiny_terminal_does_not_panic() {
        let mut store = MemoryStore::seeded();
        let mut app = App::new(store.load().unwrap());
        let mut terminal = Terminal::new(TestBackend::new(8, 5)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
    }
}
