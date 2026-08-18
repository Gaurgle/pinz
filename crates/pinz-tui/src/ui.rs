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

use pinz_core::{Color as NoteColor, Note, WorldPoint, ZoomLevel};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget, Wrap},
    Frame,
};

use crate::app::{App, Mode, Prompt, TabKind};
use crate::editor::TextEditor;
use crate::theme::Theme;
use crate::view::{CellRect, View};
use crate::wrap;

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

    // Let the app know where things landed (this also triggers first-time
    // centering) before drawing them.
    app.set_viewport(rows[2]);
    app.set_tabs_area(rows[1]);

    let theme = app.theme();
    draw_header(frame, rows[0], app, &theme);
    draw_tabs(frame, rows[1], app, &theme);
    draw_board(frame, rows[2], app, &theme);
    draw_footer(frame, rows[3], app, &theme);
    if let Some(prompt) = app.prompt() {
        draw_prompt(frame, rows[2], prompt, &theme);
    }
}

/// A small centered dialog. Deliberately a stop-and-answer moment rather than a
/// keystroke that acts instantly: naming a world deserves an escape hatch.
fn draw_prompt(frame: &mut Frame, area: Rect, prompt: &Prompt, theme: &Theme) {
    let width = area.width.clamp(12, 44);
    let height = 6u16.min(area.height);
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.accent))
        .title(Span::styled(
            format!(" {} ", prompt.title),
            Style::new().fg(theme.text).add_modifier(Modifier::BOLD),
        ))
        .style(Style::new().bg(theme.mantle));
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    // The typed name with a block caret, then either the error or the hint.
    let mut lines = vec![
        Line::from(vec![
            Span::styled(prompt.input.clone(), Style::new().fg(theme.text)),
            Span::styled(
                " ",
                Style::new().fg(theme.text).add_modifier(Modifier::REVERSED),
            ),
        ]),
        Line::raw(""),
    ];
    lines.push(match &prompt.error {
        Some(error) => Line::from(Span::styled(
            error.clone(),
            Style::new().fg(theme.note(pinz_core::Color::Red)),
        )),
        None => Line::from(Span::styled(prompt.hint, Style::new().fg(theme.overlay0))),
    });
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
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

/// The world tab strip, plus the `+` that opens the new-world prompt.
///
/// Every span comes from [`App::tabs`], which is also what a click is tested
/// against - so what you see and what you can hit cannot drift apart.
fn draw_tabs(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let armed = app.drop_target();
    let mut spans = vec![Span::raw(" ")];
    for tab in app.tabs() {
        match tab.kind {
            TabKind::World {
                index,
                name,
                notes,
                active,
            } => {
                // An armed tab is the one a released pin would land on. It takes
                // the accent as a *background* rather than a brighter foreground,
                // because the active tab already owns the foreground emphasis and
                // the two must not be mistakable for each other.
                if armed.is_some_and(|d| d.world == index) {
                    let style = Style::new()
                        .bg(theme.accent)
                        .fg(theme.mantle)
                        .add_modifier(Modifier::BOLD);
                    spans.push(Span::styled(" ", style));
                    spans.push(Span::styled(format!("{name} "), style));
                    spans.push(Span::styled(format!("{notes}  "), style));
                    continue;
                }
                let (fg, modifier) = if active {
                    (theme.text, Modifier::BOLD)
                } else {
                    (theme.subtext, Modifier::empty())
                };
                let marker = if active { "▎" } else { " " };
                spans.push(Span::styled(marker, Style::new().fg(theme.accent)));
                spans.push(Span::styled(
                    format!("{name} "),
                    Style::new().fg(fg).add_modifier(modifier),
                ));
                spans.push(Span::styled(
                    format!("{notes}  "),
                    Style::new().fg(theme.overlay0),
                ));
            }
            TabKind::New => spans.push(Span::styled(" + ", Style::new().fg(theme.overlay1))),
        }
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::new().bg(theme.mantle)),
        area,
    );

    // The pin rides the cursor. Painted last, over the strip, because it is what
    // keeps the gesture feeling continuous once the note stops tracking the
    // mouse - without it a drag over the tabs reads as having broken.
    if let Some(target) = armed {
        let buf = frame.buffer_mut();
        if let Some(cell) = buf.cell_mut((target.col, area.y)) {
            cell.set_symbol("📌");
        }
        // A pin is two cells wide; the second is its continuation, which is how
        // ratatui represents a wide grapheme.
        if let Some(next) = buf.cell_mut((target.col + 1, area.y)) {
            next.set_symbol("");
        }
    }
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
            let lines = editor_lines(editor, inner.width as usize, theme);
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
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .render(inner, buf);
    });
}

/// Paint `render` into an off-screen buffer the size of `cells`, then copy the
/// part of it that falls inside `area` onto the frame.
///
/// The detour through a private buffer is the whole point: widgets lay
/// themselves out to the rect they are given, so handing one a rect already
/// trimmed to the screen edge would re-wrap its text every time the note moved.
/// Here the layout is computed once at full size and the edge only ever cuts.
fn paint_clipped(frame: &mut Frame, cells: CellRect, area: Rect, render: impl FnOnce(&mut Buffer)) {
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
/// view: line 1 (the title) is bold, the selection is highlighted, and the
/// cursor shows as a reversed cell - all placed through the wrap so they stay on
/// screen on long lines.
///
/// The caret is drawn only when nothing is selected. With a selection the
/// moving edge of the highlight *is* the caret, and drawing both would put a
/// reversed cell inside a reversed run, which cancels out and reads as a hole.
fn editor_lines(editor: &TextEditor, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let wrapped = wrap::wrap(editor.lines(), width);
    let selection = editor
        .selection()
        .map(|(start, end)| (wrapped.place(start), wrapped.place(end)));
    let caret = selection.is_none().then(|| wrapped.place(editor.cursor()));

    wrapped
        .rows
        .iter()
        .enumerate()
        .map(|(vr, row)| {
            // Does the selection run past this row's end? Only mark the break
            // when the next row starts a new logical line - a wrap continuation
            // has no newline to show.
            let breaks = wrapped
                .rows
                .get(vr + 1)
                .is_some_and(|next| next.line != row.line);
            row_line(
                &row.text,
                row.line == 0,
                theme,
                selected_range(selection, vr, row.text.chars().count(), breaks),
                caret.filter(|(cr, _)| *cr == vr).map(|(_, cc)| cc),
            )
        })
        .collect()
}

/// The half-open range of cells on visual row `vr` that the selection covers.
/// The range may extend one past `len` to mark a selected line break.
fn selected_range(
    selection: Option<((usize, usize), (usize, usize))>,
    vr: usize,
    len: usize,
    breaks: bool,
) -> Option<(usize, usize)> {
    let ((sr, sc), (er, ec)) = selection?;
    if vr < sr || vr > er {
        return None;
    }
    let from = if vr == sr { sc } else { 0 };
    let to = if vr == er {
        ec
    } else if breaks {
        len + 1
    } else {
        len
    };
    (from < to).then_some((from, to))
}

/// One rendered row: bold when it belongs to the title line, with the selected
/// run highlighted and an optional reversed-cell caret at `caret_col` (a block
/// caret past the last character).
///
/// Built cell by cell and then coalesced into spans. Slicing the row at the
/// selection and caret boundaries directly would mean four overlapping cases;
/// styling each cell and merging equal neighbours has one.
fn row_line(
    text: &str,
    title: bool,
    theme: &Theme,
    selected: Option<(usize, usize)>,
    caret_col: Option<usize>,
) -> Line<'static> {
    let mut base = Style::new().fg(theme.note_fg);
    if title {
        base = base.add_modifier(Modifier::BOLD);
    }
    let highlight = base.bg(theme.accent).fg(theme.mantle);
    let caret = base.add_modifier(Modifier::REVERSED);

    let mut cells: Vec<(char, Style)> = text
        .chars()
        .enumerate()
        .map(|(i, c)| {
            let style = match selected {
                Some((from, to)) if (from..to).contains(&i) => highlight,
                _ => base,
            };
            (c, style)
        })
        .collect();

    // A selected line break, and a caret past the last character, both show as
    // one extra cell on the end of the row.
    if selected.is_some_and(|(_, to)| to > cells.len()) {
        cells.push((' ', highlight));
    }
    if let Some(col) = caret_col {
        match cells.get_mut(col) {
            Some(cell) => cell.1 = caret,
            None => cells.push((' ', caret)),
        }
    }

    let mut spans: Vec<Span<'static>> = Vec::new();
    for (c, style) in cells {
        match spans.last_mut() {
            Some(span) if span.style == style => span.content.to_mut().push(c),
            _ => spans.push(Span::styled(c.to_string(), style)),
        }
    }
    Line::from(spans)
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
    // Mid-drop the footer states the outcome, naming both the pin and where it
    // is about to go - so a mis-aimed drop is caught before you let go, not
    // discovered afterwards on a board you were not looking at.
    if let (Some(target), Some(note)) = (app.drop_target(), app.dragging_note()) {
        let line = Line::from(vec![
            Span::styled(" release ", Style::new().fg(theme.overlay1)),
            Span::styled("to move ", Style::new().fg(theme.overlay1)),
            Span::styled(
                format!("\"{}\"", note.title),
                Style::new().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" to ", Style::new().fg(theme.overlay1)),
            Span::styled(
                app.boards()[target.world].name.clone(),
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            ),
        ]);
        frame.render_widget(
            Paragraph::new(line).style(Style::new().bg(theme.mantle)),
            area,
        );
        return;
    }

    // A one-off message takes the whole footer: it is news, and the hints will
    // still be there on the next keystroke. Without this a copy - which changes
    // nothing on screen - would give no sign it happened at all.
    if let Some(status) = app.status() {
        let line = Line::from(Span::styled(
            format!(" {status} "),
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(
            Paragraph::new(line).style(Style::new().bg(theme.mantle)),
            area,
        );
        return;
    }

    // A sticky warning sits below one-off news but above the hints: a stopped
    // sync stays visible for the whole session, unlike the pre-TUI stderr line
    // the alternate screen used to eat.
    if let Some(warning) = app.warning() {
        let line = Line::from(Span::styled(
            format!(" !! {warning} "),
            Style::new()
                .fg(theme.note(NoteColor::Red))
                .add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(
            Paragraph::new(line).style(Style::new().bg(theme.mantle)),
            area,
        );
        return;
    }

    let hint = match app.mode() {
        Mode::Prompt => Line::from(vec![
            Span::styled(
                "naming a world",
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ·  ", Style::new().fg(theme.overlay0)),
            key_hint("enter", "create", theme.overlay1),
            key_hint("esc", "cancel", theme.overlay1),
        ]),
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
            key_hint("shift+←→", "select", theme.overlay1),
            key_hint("ctrl+c", "copy", theme.overlay1),
            key_hint("ctrl+⌫", "del word", theme.overlay1),
            key_hint("esc", "save", theme.overlay1),
        ]),
        Mode::Nav => Line::from(vec![
            key_hint("scroll/±", "zoom", theme.overlay1),
            key_hint("drag", "move/pan", theme.overlay1),
            key_hint("n", "new", theme.overlay1),
            key_hint("e", "edit", theme.overlay1),
            key_hint("y", "copy", theme.overlay1),
            key_hint("c", "color", theme.overlay1),
            key_hint("d", "del", theme.overlay1),
            key_hint("tab", "world", theme.overlay1),
            key_hint("w", "+world", theme.overlay1),
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
    use crate::editor::{Cursor, Motion};
    use crate::theme;
    use pinz_core::{MemoryStore, Store};
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
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
        assert!(
            text.contains("new note"),
            "title should be on screen:\n{text}"
        );
        assert!(
            text.contains("hello"),
            "typed body should be on screen:\n{text}"
        );
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
    fn the_tab_strip_offers_a_plus_and_the_prompt_draws_over_the_board() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut store = MemoryStore::seeded();
        let mut app = App::new(store.load().unwrap());
        let mut terminal = Terminal::new(TestBackend::new(90, 22)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert!(
            buffer_text(&terminal.backend().buffer().clone()).contains("+"),
            "the tab strip should offer a + to add a world"
        );

        let press = |c| KeyEvent::new(c, KeyModifiers::NONE);
        app.on_key(press(KeyCode::Char('w')));
        for c in "reading".chars() {
            app.on_key(press(KeyCode::Char(c)));
        }
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal.backend().buffer().clone());
        assert!(text.contains("new world"), "prompt title missing:\n{text}");
        assert!(text.contains("reading"), "typed name missing:\n{text}");
        assert!(text.contains("esc to cancel"), "hint missing:\n{text}");

        // A refused name shows why, in place of the hint, and keeps the text.
        app.on_key(press(KeyCode::Char('/')));
        app.on_key(press(KeyCode::Enter));
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal.backend().buffer().clone());
        assert!(text.contains("no slashes"), "reason missing:\n{text}");
        assert!(
            text.contains("reading/"),
            "typed name should survive:\n{text}"
        );
        assert!(
            !text.contains("esc to cancel"),
            "the reason replaces the hint"
        );
    }

    /// Every cell of a rendered line as (char, style), so a test can ask what
    /// each character was painted with rather than guess at span boundaries.
    fn cells(line: &Line<'static>) -> Vec<(char, Style)> {
        line.spans
            .iter()
            .flat_map(|s| s.content.chars().map(|c| (c, s.style)))
            .collect()
    }

    fn highlighted(line: &Line<'static>, theme: &Theme) -> String {
        cells(line)
            .iter()
            .filter(|(_, s)| s.bg == Some(theme.accent))
            .map(|(c, _)| *c)
            .collect()
    }

    fn has_caret(line: &Line<'static>) -> bool {
        cells(line)
            .iter()
            .any(|(_, s)| s.add_modifier.contains(Modifier::REVERSED))
    }

    #[test]
    fn the_selected_run_is_highlighted() {
        let theme = &theme::THEMES[0];
        let mut e = TextEditor::new("hello");
        e.step(Motion::Left, true);
        e.step(Motion::Left, true); // "lo"
        let lines = editor_lines(&e, 20, theme);
        assert_eq!(highlighted(&lines[0], theme), "lo");
    }

    #[test]
    fn nothing_is_highlighted_without_a_selection() {
        let theme = &theme::THEMES[0];
        let e = TextEditor::new("hello");
        let lines = editor_lines(&e, 20, theme);
        assert_eq!(highlighted(&lines[0], theme), "");
    }

    #[test]
    fn the_caret_is_drawn_only_when_nothing_is_selected() {
        let theme = &theme::THEMES[0];
        let mut e = TextEditor::new("hello");
        assert!(
            has_caret(&editor_lines(&e, 20, theme)[0]),
            "caret with no selection"
        );
        e.step(Motion::Left, true);
        let lines = editor_lines(&e, 20, theme);
        assert!(!has_caret(&lines[0]), "the highlight edge is the caret");
    }

    #[test]
    fn a_selection_across_lines_marks_the_break_and_both_partial_rows() {
        let theme = &theme::THEMES[0];
        let mut e = TextEditor::new("ab\ncd");
        e.set_cursor(Cursor { row: 0, col: 1 }, false);
        e.set_cursor(Cursor { row: 1, col: 1 }, true); // "b\nc"
        let lines = editor_lines(&e, 20, theme);
        assert_eq!(
            highlighted(&lines[0], theme),
            "b ",
            "the trailing cell is the newline"
        );
        assert_eq!(highlighted(&lines[1], theme), "c");
    }

    #[test]
    fn a_selection_highlights_across_a_wrap_without_inventing_a_break() {
        let theme = &theme::THEMES[0];
        let mut e = TextEditor::new("hello world");
        e.select_all();
        let lines = editor_lines(&e, 6, theme);
        assert_eq!(lines.len(), 2, "wrapped into two rows");
        assert_eq!(
            highlighted(&lines[0], theme),
            "hello ",
            "a wrap continuation has no newline cell to add"
        );
        assert_eq!(highlighted(&lines[1], theme), "world");
    }

    #[test]
    fn the_footer_offers_the_copy_keys_in_both_modes() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let press = |c| KeyEvent::new(c, KeyModifiers::NONE);
        let mut store = MemoryStore::seeded();
        let mut app = App::new(store.load().unwrap());
        let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();

        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal.backend().buffer().clone());
        assert!(
            text.contains("y copy"),
            "nav should offer the yank:\n{text}"
        );

        app.on_key(press(KeyCode::Char('n'))); // into the editor
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal.backend().buffer().clone());
        assert!(
            text.contains("select"),
            "edit should offer selection:\n{text}"
        );
        assert!(text.contains("copy"), "edit should offer copy:\n{text}");
    }

    #[test]
    fn a_copy_takes_over_the_footer_and_the_next_key_gives_it_back() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let press = |c| KeyEvent::new(c, KeyModifiers::NONE);
        let mut store = MemoryStore::seeded();
        let mut app = App::new(store.load().unwrap());
        let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();

        app.on_key(press(KeyCode::Char('n')));
        app.on_key(press(KeyCode::Esc)); // save, still selected in nav
        app.on_key(press(KeyCode::Char('y')));
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal.backend().buffer().clone());
        assert!(text.contains("copied 8 chars"), "status missing:\n{text}");

        app.on_key(press(KeyCode::Esc));
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal.backend().buffer().clone());
        assert!(!text.contains("copied"), "status should clear:\n{text}");
        assert!(text.contains("y copy"), "hints should come back:\n{text}");
    }

    /// The whole path in one test: real key events into [`App`], a real frame
    /// out, and the selection visible on it. The per-function tests above each
    /// cover one side of the app/ui seam; this is the only one that crosses it.
    #[test]
    fn shift_arrows_produce_a_visible_highlight_on_a_real_frame() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut store = MemoryStore::seeded();
        let mut app = App::new(store.load().unwrap());
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap(); // viewport + centering

        app.on_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        for _ in 0..4 {
            app.on_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
        }
        terminal.draw(|f| draw(f, &mut app)).unwrap();

        let accent = theme::THEMES[0].accent;
        let buf = terminal.backend().buffer().clone();
        let picked: String = buf
            .content()
            .iter()
            .filter(|c| c.bg == accent)
            .map(|c| c.symbol())
            .collect();
        assert_eq!(picked, "note", "the last four characters of \"new note\"");
    }

    /// An app mid-drag with a pin held over another world's tab, drawn.
    fn dragging_over_tab(index: usize) -> (App, Terminal<TestBackend>) {
        use crate::view::View;
        let mut store = MemoryStore::seeded();
        let mut app = App::new(store.load().unwrap());
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap(); // establishes viewport + tabs

        // Grab the first pin at its centre.
        let note = app.active_board().notes[0].clone();
        let view = View::new(app.camera(), app.viewport());
        let (cx, cy) = view.cell_of(note.center());
        let cell = |x: f64, base: u16| (base as f64 + x).round() as u16;
        app.on_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            cell(cx, app.viewport().x),
            cell(cy, app.viewport().y),
        ));
        // Then hover the target world's tab.
        let tab = app
            .tabs()
            .into_iter()
            .find(|t| matches!(t.kind, TabKind::World { index: i, .. } if i == index))
            .expect("world tab");
        let (tc, tr) = (tab.x + tab.width / 2, 1);
        app.on_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), tc, tr));
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        (app, terminal)
    }

    fn mouse(kind: MouseEventKind, col: u16, row: u16) -> ratatui::crossterm::event::MouseEvent {
        ratatui::crossterm::event::MouseEvent {
            kind,
            column: col,
            row,
            modifiers: ratatui::crossterm::event::KeyModifiers::NONE,
        }
    }

    #[test]
    fn the_armed_tab_takes_the_accent_background() {
        let (app, terminal) = dragging_over_tab(1);
        let target = app.drop_target().expect("a tab should be armed");
        assert_eq!(target.world, 1);

        // Asserted as a column range rather than by matching the label text: the
        // pin glyph deliberately covers two cells of the name, so the visible
        // characters are not the name any more. The highlighted span is.
        let tab = app
            .tabs()
            .into_iter()
            .find(|t| matches!(t.kind, TabKind::World { index: 1, .. }))
            .expect("world tab");
        let accent = theme::THEMES[0].accent;
        let buf = terminal.backend().buffer().clone();
        // The cell after the pin is the continuation of a wide grapheme, and
        // `Buffer::diff` deliberately skips those - so the test backend still
        // holds the previous frame's background there. The real terminal draws
        // the pin across both cells.
        let continuation = target.col + 1;
        for x in tab.x..tab.x + tab.width {
            if x == continuation {
                continue;
            }
            assert_eq!(
                buf[(x, 1)].bg,
                accent,
                "column {x} of the armed tab is not lit"
            );
        }
        for x in 0..buf.area.width {
            if (tab.x..tab.x + tab.width).contains(&x) {
                continue;
            }
            assert_ne!(
                buf[(x, 1)].bg,
                accent,
                "column {x} outside the armed tab is lit"
            );
        }
    }

    #[test]
    fn a_pin_rides_the_cursor_over_the_tab_strip() {
        let (app, terminal) = dragging_over_tab(1);
        let col = app.drop_target().unwrap().col;
        let buf = terminal.backend().buffer().clone();
        assert_eq!(
            buf[(col, 1)].symbol(),
            "📌",
            "the pin should sit under the cursor, not somewhere else in the strip"
        );
    }

    #[test]
    fn the_footer_says_what_the_drop_will_do() {
        let (app, terminal) = dragging_over_tab(1);
        let text = buffer_text(&terminal.backend().buffer().clone());
        let title = &app.boards()[0].notes[0].title;
        assert!(text.contains("release to move"), "no drop hint:\n{text}");
        assert!(
            text.contains(title.as_str()),
            "the pin is not named:\n{text}"
        );
        assert!(
            text.contains(app.boards()[1].name.as_str()),
            "no destination:\n{text}"
        );
    }

    #[test]
    fn no_tab_is_armed_when_nothing_is_being_dragged() {
        let mut store = MemoryStore::seeded();
        let mut app = App::new(store.load().unwrap());
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let accent = theme::THEMES[0].accent;
        let buf = terminal.backend().buffer().clone();
        assert!(
            !(0..buf.area.width).any(|x| buf[(x, 1)].bg == accent),
            "nothing should be armed at rest"
        );
        assert!(!buffer_text(&buf).contains("release to move"));
    }

    #[test]
    fn tiny_terminal_does_not_panic() {
        let mut store = MemoryStore::seeded();
        let mut app = App::new(store.load().unwrap());
        let mut terminal = Terminal::new(TestBackend::new(8, 5)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
    }
}
