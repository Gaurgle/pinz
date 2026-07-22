//! `pinz` - a spatial bulletin board in your terminal.
//!
//! This binary is the Ratatui renderer. It loads boards through the
//! [`Store`](pinz_core::Store) seam, then hands drawing to [`ui`] and input to
//! [`app::App`]. All the domain and projection math lives in `pinz-core`; this
//! crate is just the terminal skin over it.

mod app;
mod theme;
mod ui;
mod view;

use std::io::{self, Stdout};
use std::panic;

use app::App;
use pinz_core::{MemoryStore, Store};
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::Terminal;

type Tui = Terminal<CrosstermBackend<Stdout>>;

fn main() -> io::Result<()> {
    let mut store = MemoryStore::seeded();
    let boards = store.load().map_err(|e| io::Error::other(e.to_string()))?;
    let mut app = App::new(boards);
    if let Some(name) = requested_theme() {
        app.set_theme_by_name(&name);
    }

    let mut terminal = setup()?;
    let result = run(&mut terminal, &mut app);
    restore()?;

    // Persist on exit. For the in-memory store this is a no-op, but it exercises
    // the seam so a git-backed store later needs no changes here.
    let _ = store.save(app.boards());

    result
}

/// A starting theme from the command line, if given: `pinz nord` or
/// `pinz --theme "solarized light"`. The name is matched loosely later, so a
/// rough spelling is fine. Anything unrecognized just falls back to the default.
fn requested_theme() -> Option<String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--theme" | "-t" => return args.get(i + 1).cloned(),
            s if !s.starts_with('-') => return Some(s.to_string()),
            _ => i += 1,
        }
    }
    None
}

/// Enter raw mode + the alternate screen + mouse capture, and install a panic
/// hook that puts the terminal back before the panic message prints - otherwise
/// a crash leaves the user's shell wrecked.
fn setup() -> io::Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;

    let hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let _ = restore();
        hook(info);
    }));

    Terminal::new(CrosstermBackend::new(stdout))
}

fn restore() -> io::Result<()> {
    execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
    disable_raw_mode()
}

/// Draw, then block for the next event and apply it. No animation loop: the
/// board only changes in response to input, so a redraw per event is enough and
/// keeps the app idle at zero CPU.
fn run(terminal: &mut Tui, app: &mut App) -> io::Result<()> {
    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;
        if app.should_quit() {
            return Ok(());
        }
        match event::read()? {
            // Only act on key presses; ignore key-release/repeat where the
            // terminal reports them, so a keystroke fires once.
            Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
            Event::Mouse(mouse) => app.on_mouse(mouse),
            _ => {}
        }
    }
}
