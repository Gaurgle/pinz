//! Drawing. Reads [`App`] state and paints a frame; holds no state of its own.
//!
//! The board uses the split render path DESIGN calls for: zoomed out
//! (survey/cluster) notes are cheap shapes painted straight into the buffer;
//! zoomed in (titles/preview/document) they are real bordered widgets with text.
//! The projection layer ([`View`]) decides where everything lands; this module
//! only decides how it looks at each zoom.

use pinz_core::{Note, WorldPoint, ZoomLevel};
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::app::{App, Mode};
use crate::theme;
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

    draw_header(frame, rows[0], app);
    draw_tabs(frame, rows[1], app);
    draw_board(frame, rows[2], app);
    draw_footer(frame, rows[3], app);
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let cols = Layout::horizontal([Constraint::Min(10), Constraint::Length(28)]).split(area);

    let brand = Line::from(vec![
        Span::styled(
            "📌 pinz",
            Style::new().fg(theme::TEXT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "  — terminal bulletin board",
            Style::new().fg(theme::OVERLAY1),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(brand).style(Style::new().bg(theme::MANTLE)),
        cols[0],
    );

    // Zoom indicator: five dots filled up to the current level, then the label.
    let idx = app.zoom().index();
    let mut spans = Vec::new();
    for i in 0..ZoomLevel::ALL.len() {
        let (glyph, color) = if i <= idx {
            ("●", theme::LAVENDER)
        } else {
            ("○", theme::SURFACE1)
        };
        spans.push(Span::styled(glyph, Style::new().fg(color)));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(
        format!("{:<8}", app.zoom().label()),
        Style::new().fg(theme::OVERLAY1),
    ));
    frame.render_widget(
        Paragraph::new(Line::from(spans))
            .alignment(Alignment::Right)
            .style(Style::new().bg(theme::MANTLE)),
        cols[1],
    );
}

fn draw_tabs(frame: &mut Frame, area: Rect, app: &App) {
    let mut spans = vec![Span::raw(" ")];
    for (i, board) in app.boards().iter().enumerate() {
        let active = i == app.active_index();
        let (fg, modifier) = if active {
            (theme::TEXT, Modifier::BOLD)
        } else {
            (theme::SUBTEXT, Modifier::empty())
        };
        let marker = if active { "▎" } else { " " };
        spans.push(Span::styled(marker, Style::new().fg(theme::LAVENDER)));
        spans.push(Span::styled(
            format!("{} ", board.name),
            Style::new().fg(fg).add_modifier(modifier),
        ));
        spans.push(Span::styled(
            format!("{}  ", board.notes.len()),
            Style::new().fg(theme::OVERLAY0),
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::new().bg(theme::MANTLE)),
        area,
    );
}

fn draw_board(frame: &mut Frame, area: Rect, app: &App) {
    // Background wash.
    frame.render_widget(Block::new().style(Style::new().bg(theme::BASE)), area);
    draw_grid(frame, area, app);

    let view = View::new(app.camera(), area);

    // Notes bottom-of-stack first, so higher z overlaps lower.
    let mut notes: Vec<&Note> = app.active_board().notes.iter().collect();
    notes.sort_by_key(|n| n.z);

    match app.zoom() {
        ZoomLevel::Survey => draw_far(frame, area, &view, &notes, false),
        ZoomLevel::Cluster => draw_far(frame, area, &view, &notes, true),
        lod => {
            for note in &notes {
                if let Some(rect) = view.note_rect(note.position()) {
                    draw_note_widget(frame, rect, note, lod, app);
                }
            }
        }
    }

    draw_scrollbars(frame, area, app);
}

/// The dot grid, aligned to world coordinates so it slides under the notes as
/// you pan - a cheap sense of place. Skipped when the grid would be too dense to
/// read.
fn draw_grid(frame: &mut Frame, area: Rect, app: &App) {
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
                cell.set_symbol("·").set_fg(theme::SURFACE0);
            }
            gy += GRID_WORLD;
        }
        gx += GRID_WORLD;
    }
}

/// Zoomed-out path: a note is a shape, not a widget. `filled` distinguishes the
/// cluster level (solid colored blocks) from survey (a single colored pip).
fn draw_far(frame: &mut Frame, area: Rect, view: &View, notes: &[&Note], filled: bool) {
    for note in notes {
        let color = theme::note_color(note.color);
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
fn draw_note_widget(frame: &mut Frame, rect: Rect, note: &Note, lod: ZoomLevel, app: &App) {
    let color = theme::note_color(note.color);
    let selected = app.selected() == Some(note.id);
    let editing = selected && app.mode() == Mode::EditTitle;

    let border_style = if selected {
        Style::new()
            .fg(theme::LAVENDER)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme::CRUST)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(Span::styled("📌", Style::new().fg(theme::CRUST)))
        .style(Style::new().bg(color).fg(theme::CRUST));

    // Compute the inner area before the block is consumed by render. Clear first
    // so the note is opaque: a Block only restyles cells, it won't wipe the grid
    // dots (or a note underneath) showing through its interior.
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let title = if editing {
        format!("{}▏", app.edit_buf())
    } else {
        note.title.clone()
    };

    let mut lines = vec![Line::from(Span::styled(
        title,
        Style::new().fg(theme::CRUST).add_modifier(Modifier::BOLD),
    ))];

    // Body appears from preview level up.
    if matches!(lod, ZoomLevel::Preview | ZoomLevel::Document) && !note.body.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            note.body.clone(),
            Style::new().fg(theme::CRUST),
        )));
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

/// Thin position indicators along the right and bottom edges, sized and placed
/// like the demo's scrollbars: they show where the camera sits in the note
/// cloud. Hidden on an axis when everything already fits.
fn draw_scrollbars(frame: &mut Frame, area: Rect, app: &App) {
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
        paint_bar(frame, col, top, len, true, area);
    }

    // Horizontal bar on the bottom edge.
    if view_w < world_w && area.width > 2 {
        let track = area.width as f64;
        let frac = (view_w / world_w).clamp(0.05, 1.0);
        let pos = ((origin.x - min.x) / world_w).clamp(0.0, 1.0 - frac);
        let len = (frac * track).round().max(1.0) as u16;
        let left = area.x + (pos * track).round() as u16;
        let row = area.y + area.height - 1;
        paint_bar(frame, left, row, len, false, area);
    }
}

fn paint_bar(frame: &mut Frame, x: u16, y: u16, len: u16, vertical: bool, area: Rect) {
    let buf = frame.buffer_mut();
    for i in 0..len {
        let (cx, cy) = if vertical { (x, y + i) } else { (x + i, y) };
        if cx < area.x + area.width && cy < area.y + area.height {
            if let Some(cell) = buf.cell_mut((cx, cy)) {
                cell.set_symbol(if vertical { "▐" } else { "▄" })
                    .set_fg(theme::OVERLAY0);
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

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let hint = match app.mode() {
        Mode::EditTitle => Line::from(vec![
            Span::styled(
                "editing title",
                Style::new()
                    .fg(theme::LAVENDER)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  ", Style::new().fg(theme::OVERLAY0)),
            key_hint("enter", "save"),
            key_hint("esc", "cancel"),
        ]),
        Mode::Nav => Line::from(vec![
            key_hint("scroll/±", "zoom"),
            key_hint("drag", "move/pan"),
            key_hint("n", "new"),
            key_hint("e", "edit"),
            key_hint("d", "del"),
            key_hint("tab/1-9", "world"),
            key_hint("q", "quit"),
        ]),
    };
    frame.render_widget(
        Paragraph::new(hint).style(Style::new().bg(theme::MANTLE)),
        area,
    );
}

fn key_hint(key: &str, label: &str) -> Span<'static> {
    Span::styled(format!(" {key} {label} "), Style::new().fg(theme::OVERLAY1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinz_core::{MemoryStore, Store};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn render() -> ratatui::buffer::Buffer {
        let mut store = MemoryStore::seeded();
        let mut app = App::new(store.load().unwrap());
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
        let buf = render();
        let text = buffer_text(&buf);
        assert!(text.contains("pinz"), "brand missing:\n{text}");
        assert!(text.contains("ideas"), "world tab missing:\n{text}");
        assert!(text.contains("wavez"), "world tab missing:\n{text}");
        // A seeded note title should be on the board at titles zoom.
        assert!(text.contains("Fortnox"), "note title missing:\n{text}");
    }

    #[test]
    fn renders_footer_keybinds() {
        let buf = render();
        let text = buffer_text(&buf);
        assert!(text.contains("zoom"), "footer hint missing:\n{text}");
        assert!(text.contains("quit"), "footer hint missing:\n{text}");
    }

    #[test]
    fn tiny_terminal_does_not_panic() {
        let mut store = MemoryStore::seeded();
        let mut app = App::new(store.load().unwrap());
        let mut terminal = Terminal::new(TestBackend::new(8, 5)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
    }
}
