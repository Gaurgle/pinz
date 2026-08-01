//! `pinz` - a spatial bulletin board in your terminal.
//!
//! This binary is the Ratatui renderer. It loads boards through the
//! [`Store`](pinz_core::Store) seam, then hands drawing to [`ui`] and input to
//! [`app::App`]. All the domain and projection math lives in `pinz-core`; this
//! crate is just the terminal skin over it.
//!
//! Pins live in their own git repo (`~/pinz` by default, `$PINZ_HOME` to move
//! it). The board is written to disk as you change it rather than only on exit,
//! because a corkboard is meant to stay open; git sync happens at the edges
//! (pull on start, commit and push on quit) so a drag doesn't mint a commit.

mod app;
mod editor;
mod theme;
mod ui;
mod view;

use std::io::{self, Stdout};
use std::panic;
use std::path::PathBuf;

use app::App;
use pinz_core::{Board, FileStore, MemoryStore, Store, Sync, SyncOutcome};
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::Terminal;

type Tui = Terminal<CrosstermBackend<Stdout>>;

/// The board a brand new pin repo starts with, so there is always a tab.
const FIRST_BOARD: &str = "ideas";

fn main() -> io::Result<()> {
    let opts = Options::parse(std::env::args().skip(1));
    match opts.command {
        Command::Help => {
            print_help();
            Ok(())
        }
        Command::Sync => git_command(Command::Sync),
        Command::Status => git_command(Command::Status),
        Command::Pull => git_command(Command::Pull),
        Command::Push => git_command(Command::Push),
        Command::Run => run_app(opts),
    }
}

// ---- command line ----

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Run,
    /// Do whatever the repo needs: pull what's waiting, commit what changed,
    /// push what's ahead.
    Sync,
    /// Report the repo's state and do nothing.
    Status,
    /// Only bring the other machine's pins in.
    Pull,
    /// Only commit and send this machine's pins.
    Push,
    Help,
}

impl Command {
    /// Subcommand names. `sync` is the one to reach for; `pull` and `push` are
    /// for when you want exactly one half of it.
    ///
    /// Only `st` is abbreviated, and only because guessing wrong about it is
    /// free: the worst a misread `st` can do is print a report. Everything that
    /// moves commits must be typed in full. An earlier pass had `s`, `up` and
    /// `down`, and every one of them could be read as a different command than
    /// it ran - `s` looks like *status* to anyone whose git is configured that
    /// way but committed and pushed, and `up` reads as *update*, meaning pull,
    /// while it pushed. A saved keystroke is not worth a command that does the
    /// opposite of what you meant.
    fn from_word(word: &str) -> Option<Command> {
        Some(match word {
            "sync" => Command::Sync,
            "status" | "st" => Command::Status,
            "pull" => Command::Pull,
            "push" => Command::Push,
            "help" | "--help" | "-h" => Command::Help,
            _ => return None,
        })
    }
}

#[derive(Debug)]
struct Options {
    command: Command,
    /// A starting theme, matched loosely later.
    theme: Option<String>,
    /// Whether to touch git at all this run.
    sync: bool,
    /// Run against seeded in-memory boards, touching no files. Handy for
    /// screenshots and for trying the app without a pin repo.
    demo: bool,
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Self {
        let mut opts = Options {
            command: Command::Run,
            theme: None,
            sync: true,
            demo: false,
        };
        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--no-sync" => opts.sync = false,
                "--demo" => opts.demo = true,
                "--theme" | "-t" => opts.theme = args.next(),
                // A subcommand, if it names one; otherwise a bare word is the
                // theme: `pinz nord`.
                s => match Command::from_word(s) {
                    Some(command) => opts.command = command,
                    None if !s.starts_with('-') && opts.theme.is_none() => {
                        opts.theme = Some(s.to_string())
                    }
                    None => {}
                },
            }
        }
        opts
    }
}

fn print_help() {
    println!(
        "pinz - a spatial bulletin board in your terminal

USAGE:
    pinz [THEME]        open the board (optionally in a named theme)
    pinz sync           do whatever the repo needs: pull, commit, push
    pinz status  (st)   report what is waiting, and change nothing
    pinz pull           only bring the other machine's pins in
    pinz push           only commit and send this machine's pins
    pinz help           show this

OPTIONS:
    -t, --theme <NAME>  start in a theme (mocha, tokyo, gruvbox, nord, light)
        --no-sync       do not touch git this run
        --demo          seeded in-memory boards; writes nothing to disk

PINS:
    Pins live in $PINZ_HOME, or ~/pinz. One directory per board, one markdown
    file per pin. It is an ordinary git repo: add a remote and `pinz sync`
    keeps your machines level."
    );
}

fn pin_root() -> io::Result<PathBuf> {
    FileStore::default_root()
        .ok_or_else(|| io::Error::other("cannot find a home directory; set PINZ_HOME"))
}

// ---- the sync subcommand ----

/// The git-facing subcommands. Syncing pins is pinz's own job, not something
/// bolted onto another tool's sync, so they all live here.
///
/// Each one looks at the repo's state first and only does the steps that state
/// calls for, which is what makes `sync` safe to run reflexively: with nothing
/// waiting it says so and touches nothing.
fn git_command(command: Command) -> io::Result<()> {
    let root = pin_root()?;
    // Opening the store creates the directory, and gives a brand new repo its
    // first board - otherwise there would be nothing to commit and no way to
    // push, which is exactly the state a fresh machine starts in.
    let mut store = FileStore::open(&root).map_err(|e| io::Error::other(e.to_string()))?;
    let boards = store.load().map_err(|e| io::Error::other(e.to_string()))?;
    if boards.is_empty() {
        store
            .save(&[Board::new(FIRST_BOARD)])
            .map_err(|e| io::Error::other(e.to_string()))?;
    }

    let sync = Sync::new(&root);
    if !sync.is_repo() {
        report("init", sync.init());
    }

    // A status is only current once we have asked the remote.
    let fetched = sync.fetch();
    let status = sync.status();
    println!("   {} - {}", root.display(), status.summary());
    if !status.has_remote {
        println!("   no remote yet: create one, then `git -C {} remote add origin <url>`", root.display());
    } else if let SyncOutcome::Idle(why) = &fetched {
        println!("   (remote not reachable: {why})");
    }

    if command == Command::Status {
        return Ok(());
    }

    let wants_pull = matches!(command, Command::Sync | Command::Pull);
    let wants_push = matches!(command, Command::Sync | Command::Push);

    if wants_pull {
        let pulled = sync.pull();
        report("pull", &pulled);
        if pulled.is_stopped() {
            eprintln!("\nstopping here: resolve the conflict above before pushing.");
            return Ok(());
        }
    }
    if wants_push {
        report("push", sync.push("pinz: update pins"));
    }
    Ok(())
}

fn report(step: &str, outcome: impl AsOutcome) {
    let outcome = outcome.as_outcome();
    let mark = match outcome {
        SyncOutcome::Done(_) => "ok",
        SyncOutcome::Idle(_) => "--",
        SyncOutcome::Stopped(_) => "!!",
    };
    println!("{mark} {step}: {}", outcome.message());
}

/// Lets `report` take an outcome by value or by reference without cloning.
trait AsOutcome {
    fn as_outcome(&self) -> &SyncOutcome;
}

impl AsOutcome for SyncOutcome {
    fn as_outcome(&self) -> &SyncOutcome {
        self
    }
}

impl AsOutcome for &SyncOutcome {
    fn as_outcome(&self) -> &SyncOutcome {
        self
    }
}

// ---- the app ----

fn run_app(opts: Options) -> io::Result<()> {
    let mut store: Box<dyn Store> = if opts.demo {
        Box::new(MemoryStore::seeded())
    } else {
        let root = pin_root()?;
        Box::new(FileStore::open(&root).map_err(|e| io::Error::other(e.to_string()))?)
    };

    // Pull before loading, so the board you see is the merged one. A pull that
    // stops leaves local files untouched and only costs us the push on exit.
    let sync = (!opts.demo && opts.sync)
        .then(pin_root)
        .transpose()?
        .map(Sync::new);
    let mut may_push = true;
    if let Some(sync) = &sync {
        let pulled = sync.pull();
        if pulled.is_stopped() {
            may_push = false;
            eprintln!("!! {}", pulled.message());
            eprintln!("   your pins are safe and the board still opens; pinz will not push this run.");
        }
    }

    let mut boards = store.load().map_err(|e| io::Error::other(e.to_string()))?;
    if boards.is_empty() {
        boards.push(Board::new(FIRST_BOARD));
    }
    let mut app = App::new(boards);
    if let Some(name) = &opts.theme {
        app.set_theme_by_name(name);
    }

    let mut terminal = setup()?;
    let result = run(&mut terminal, &mut app, store.as_mut());
    restore()?;

    let save_error = result?;
    if let Some(message) = &save_error {
        eprintln!("!! could not write pins: {message}");
    }

    // A last save catches anything the loop deferred (a quit mid-drag).
    if let Err(e) = store.save(app.boards()) {
        eprintln!("!! could not write pins: {e}");
        return Ok(());
    }
    if let (Some(sync), true) = (&sync, may_push && save_error.is_none()) {
        let pushed = sync.push("pinz: update pins");
        if pushed.is_stopped() {
            eprintln!("!! {}", pushed.message());
        }
    }
    Ok(())
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
///
/// Returns a save error, if one happened, for the caller to print once the
/// terminal is back - nothing may be written to the screen while the TUI owns it.
fn run(terminal: &mut Tui, app: &mut App, store: &mut dyn Store) -> io::Result<Option<String>> {
    let mut saved = app.revision();
    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;
        if app.should_quit() {
            return Ok(None);
        }
        match event::read()? {
            // Only act on key presses; ignore key-release/repeat where the
            // terminal reports them, so a keystroke fires once.
            Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
            Event::Mouse(mouse) => app.on_mouse(mouse),
            _ => {}
        }
        // Persist as you work, but never mid-gesture: a drag would otherwise
        // rewrite the pin's file on every mouse-move.
        if app.revision() != saved && !app.is_dragging() {
            if let Err(e) = store.save(app.boards()) {
                return Ok(Some(e.to_string()));
            }
            saved = app.revision();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Options {
        Options::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn a_bare_word_is_a_theme() {
        let o = parse(&["nord"]);
        assert_eq!(o.command, Command::Run);
        assert_eq!(o.theme.as_deref(), Some("nord"));
        assert!(o.sync, "sync is on by default");
    }

    #[test]
    fn sync_is_a_command_not_a_theme() {
        let o = parse(&["sync"]);
        assert_eq!(o.command, Command::Sync);
        assert_eq!(o.theme, None, "\"sync\" must not be read as a theme name");
    }

    #[test]
    fn every_git_subcommand_resolves() {
        for (word, expected) in [
            ("sync", Command::Sync),
            ("status", Command::Status),
            ("st", Command::Status),
            ("pull", Command::Pull),
            ("push", Command::Push),
        ] {
            let o = parse(&[word]);
            assert_eq!(o.command, expected, "{word:?} should be {expected:?}");
            assert_eq!(o.theme, None, "{word:?} must not be read as a theme");
        }
    }

    #[test]
    fn only_the_read_only_command_may_be_abbreviated() {
        // A short alias is allowed only where misreading it costs nothing. "s"
        // and "up" are the dangerous ones: both could be taken for a command
        // other than the one they would run, and both move commits.
        for word in ["s", "up", "down", "sy", "pu"] {
            let o = parse(&[word]);
            assert_eq!(
                o.command,
                Command::Run,
                "{word:?} must not be a shortcut for anything that moves commits"
            );
        }
        assert_eq!(parse(&["st"]).command, Command::Status, "st is read-only, so it may be short");
    }

    #[test]
    fn a_word_that_is_not_a_subcommand_is_still_a_theme() {
        // The aliases must not swallow theme names.
        for theme in ["nord", "gruvbox", "light", "mocha", "tokyo"] {
            let o = parse(&[theme]);
            assert_eq!(o.command, Command::Run, "{theme:?} should just open the board");
            assert_eq!(o.theme.as_deref(), Some(theme));
        }
    }

    #[test]
    fn flags_parse_in_any_order() {
        let o = parse(&["--no-sync", "--theme", "gruvbox"]);
        assert!(!o.sync);
        assert_eq!(o.theme.as_deref(), Some("gruvbox"));

        let o = parse(&["--demo", "light"]);
        assert!(o.demo);
        assert_eq!(o.theme.as_deref(), Some("light"));
    }

    #[test]
    fn help_wins_over_a_theme() {
        assert_eq!(parse(&["--help"]).command, Command::Help);
        assert_eq!(parse(&["help"]).command, Command::Help);
    }

    #[test]
    fn no_arguments_just_runs() {
        let o = parse(&[]);
        assert_eq!(o.command, Command::Run);
        assert_eq!(o.theme, None);
        assert!(!o.demo);
    }

    #[test]
    fn a_pin_written_in_the_app_survives_a_round_trip_through_disk() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use ratatui::layout::Rect;

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("pinz-e2e-{nanos}"));
        let mut store = FileStore::open(&dir).unwrap();

        let mut app = App::new(vec![Board::new("ideas")]);
        app.set_viewport(Rect {
            x: 0,
            y: 2,
            width: 100,
            height: 30,
        });
        let press = |c| KeyEvent::new(c, KeyModifiers::NONE);

        let before = app.revision();
        app.on_key(press(KeyCode::Char('n'))); // new pin, opens the editor
        // ctrl+u clears the placeholder title, as it would in the app
        app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        for c in "buy milk".chars() {
            app.on_key(press(KeyCode::Char(c)));
        }
        app.on_key(press(KeyCode::Enter));
        for c in "oat, not soy".chars() {
            app.on_key(press(KeyCode::Char(c)));
        }
        app.on_key(press(KeyCode::Esc)); // save the edit
        assert!(app.revision() > before, "the runner would now write to disk");

        store.save(app.boards()).unwrap();

        // A fresh store, as a different machine (or the next launch) would see it.
        let mut fresh = FileStore::open(&dir).unwrap();
        let boards = fresh.load().unwrap();
        assert_eq!(boards.len(), 1);
        assert_eq!(boards[0].name, "ideas");
        assert_eq!(boards[0].notes.len(), 1);
        let pin = &boards[0].notes[0];
        assert_eq!(pin.title, "buy milk");
        assert_eq!(pin.body, "oat, not soy");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
