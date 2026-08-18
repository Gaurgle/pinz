//! The git-friendly file store: pinz's own corpus on disk.
//!
//! Layout is one directory per board, one markdown file per pin:
//!
//! ```text
//! ~/pinz-board/
//!   ideas/
//!     2026-08-01-143022-buy-a-new-lamp.md
//!   sketches/
//! ```
//!
//! A pin carries its spatial metadata in a small frontmatter header and its
//! text as ordinary markdown:
//!
//! ```text
//! ---
//! x: 720
//! y: 380
//! z: 4
//! color: green
//! ---
//! # buy a new lamp
//!
//! The one by the desk flickers.
//! ```
//!
//! **One file per pin is a sync decision, not a style one.** These files live in
//! a git repo synced between machines; a single board file would conflict
//! whenever both ends touched anything on that board, while per-pin files
//! conflict only when the same pin was edited twice. It also keeps saves
//! incremental - only files whose bytes actually changed are rewritten, so a
//! drag doesn't churn the whole board's history.
//!
//! These are pinz's own files, not notez2 notes. The markdown body is what makes
//! promoting a pin into a real note easy later; nothing here reads or writes a
//! notez2 workspace.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::model::{Board, Color, Note};
use crate::store::{Result, Store, StoreError};

/// Longest slug taken from a title when naming a file.
const SLUG_MAX: usize = 40;

/// A board directory or pin file starting with `.` is ours to ignore (`.git`,
/// `.gitkeep`, editor droppings).
fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

fn backend<E: std::fmt::Display>(what: &str, e: E) -> StoreError {
    StoreError::Backend(format!("{what}: {e}"))
}

/// Boards and pins as files under a root directory.
pub struct FileStore {
    root: PathBuf,
    /// Note id -> the file it was loaded from, so a save can rewrite, move, or
    /// delete exactly the files this store is responsible for and no others.
    paths: HashMap<u64, PathBuf>,
    next_id: u64,
}

impl FileStore {
    /// A store rooted at `root`, creating the directory if it is missing.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(|e| backend("creating the pinz directory", e))?;
        Ok(Self {
            root,
            paths: HashMap::new(),
            next_id: 1,
        })
    }

    /// The default root: `$PINZ_HOME`, else `~/pinz-board`.
    ///
    /// Named to match the git repo it usually is, so the directory on disk and
    /// the repo on the remote are not two names for one thing.
    pub fn default_root() -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os("PINZ_HOME") {
            return Some(PathBuf::from(dir));
        }
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("pinz-board"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Board directories, sorted by name so the tab strip is stable across runs
    /// and machines.
    fn board_dirs(&self) -> Result<Vec<PathBuf>> {
        let mut dirs = Vec::new();
        let entries =
            fs::read_dir(&self.root).map_err(|e| backend("reading the pinz directory", e))?;
        for entry in entries {
            let entry = entry.map_err(|e| backend("reading the pinz directory", e))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_hidden(&name) {
                continue;
            }
            if entry.path().is_dir() {
                dirs.push(entry.path());
            }
        }
        dirs.sort();
        Ok(dirs)
    }

    /// The `.md` files of one board, sorted by name - which, because names lead
    /// with a timestamp, means oldest pin first.
    fn pin_files(dir: &Path) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        let entries = fs::read_dir(dir).map_err(|e| backend("reading a board directory", e))?;
        for entry in entries {
            let entry = entry.map_err(|e| backend("reading a board directory", e))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_hidden(&name) || !name.ends_with(".md") {
                continue;
            }
            files.push(entry.path());
        }
        files.sort();
        Ok(files)
    }

    /// A fresh, unused path for `note` on `board`.
    fn new_path(&self, board: &str, note: &Note) -> PathBuf {
        let dir = self.root.join(board);
        let stem = format!("{}-{}", timestamp_prefix(now_secs()), slug(&note.title));
        let mut candidate = dir.join(format!("{stem}.md"));
        let mut n = 2;
        while candidate.exists() {
            candidate = dir.join(format!("{stem}-{n}.md"));
            n += 1;
        }
        candidate
    }

    /// Write `contents` only if it differs from what is already there, so a save
    /// that changed nothing leaves the working tree (and git) untouched.
    fn write_if_changed(path: &Path, contents: &str) -> Result<()> {
        if let Ok(existing) = fs::read_to_string(path) {
            if existing == contents {
                return Ok(());
            }
        }
        fs::write(path, contents).map_err(|e| backend("writing a pin", e))
    }
}

impl Store for FileStore {
    fn load(&mut self) -> Result<Vec<Board>> {
        self.paths.clear();
        self.next_id = 1;

        let mut boards = Vec::new();
        for dir in self.board_dirs()? {
            let name = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let mut board = Board::new(name);
            for path in Self::pin_files(&dir)? {
                let text = fs::read_to_string(&path).map_err(|e| backend("reading a pin", e))?;
                let id = self.next_id;
                self.next_id += 1;
                board.notes.push(parse_pin(&text, id));
                self.paths.insert(id, path);
            }
            boards.push(board);
        }
        Ok(boards)
    }

    fn save(&mut self, boards: &[Board]) -> Result<()> {
        let mut written: HashMap<u64, PathBuf> = HashMap::new();

        for board in boards {
            let dir = self.root.join(&board.name);
            fs::create_dir_all(&dir).map_err(|e| backend("creating a board directory", e))?;

            for note in &board.notes {
                // A pin we loaded keeps its filename, unless it has been dragged
                // to another board - then it moves directory, keeping its name.
                let path = match self.paths.get(&note.id) {
                    Some(old) if old.parent() == Some(dir.as_path()) => old.clone(),
                    Some(old) => {
                        let moved = dir.join(old.file_name().unwrap_or_default());
                        let _ = fs::remove_file(old);
                        moved
                    }
                    None => self.new_path(&board.name, note),
                };
                Self::write_if_changed(&path, &render_pin(note))?;
                written.insert(note.id, path);
            }

            // Git cannot track an empty directory, so an emptied board would
            // vanish on the next machine. A marker keeps the tab alive.
            if board.notes.is_empty() {
                let keep = dir.join(".gitkeep");
                if !keep.exists() {
                    fs::write(&keep, "").map_err(|e| backend("marking an empty board", e))?;
                }
            } else {
                let _ = fs::remove_file(dir.join(".gitkeep"));
            }
        }

        // Delete only files this store loaded and that are no longer on a board.
        // Anything we never loaded is not ours to remove.
        for (id, path) in &self.paths {
            if !written.contains_key(id) {
                let _ = fs::remove_file(path);
            }
        }

        self.paths = written;
        Ok(())
    }
}

// ---- the pin file format ----

/// A pin as file contents: frontmatter, the title as an H1, then the body.
pub fn render_pin(note: &Note) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!("x: {}\n", note.x.round() as i64));
    out.push_str(&format!("y: {}\n", note.y.round() as i64));
    out.push_str(&format!("z: {}\n", note.z));
    out.push_str(&format!("color: {}\n", note.color.as_str()));
    out.push_str("---\n");
    out.push_str(&format!("# {}\n", note.title));
    if !note.body.is_empty() {
        out.push('\n');
        out.push_str(note.body.trim_end());
        out.push('\n');
    }
    out
}

/// Parse file contents into a note with the given id.
///
/// Deliberately forgiving: a hand-written file with no frontmatter, or a missing
/// field, still loads as a pin at the origin rather than failing the whole
/// board. Losing a note's text to a strict parser would be the worse outcome.
pub fn parse_pin(text: &str, id: u64) -> Note {
    let (fields, rest) = split_frontmatter(text);

    let (title, body) = split_title_and_body(rest);
    Note {
        id,
        title,
        body,
        x: fields.get("x").and_then(|v| v.parse().ok()).unwrap_or(0.0),
        y: fields.get("y").and_then(|v| v.parse().ok()).unwrap_or(0.0),
        z: fields.get("z").and_then(|v| v.parse().ok()).unwrap_or(0),
        color: fields
            .get("color")
            .and_then(|v| v.parse().ok())
            .unwrap_or(Color::Yellow),
    }
}

/// Split a leading `---` block into `key: value` pairs, returning the pairs and
/// the remaining text. No frontmatter means no pairs and the text unchanged.
fn split_frontmatter(text: &str) -> (HashMap<String, String>, &str) {
    let mut fields = HashMap::new();
    let Some(after_open) = text.strip_prefix("---\n") else {
        return (fields, text);
    };
    let Some(end) = after_open.find("\n---") else {
        return (fields, text); // unterminated: treat the whole thing as body
    };
    let (block, tail) = after_open.split_at(end);
    for line in block.lines() {
        if let Some((key, value)) = line.split_once(':') {
            fields.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    let rest = tail
        .strip_prefix("\n---")
        .unwrap_or(tail)
        .strip_prefix('\n')
        .unwrap_or("");
    (fields, rest)
}

/// First line is the title (with its `# ` stripped if present), the rest is the
/// body, minus one blank separator line.
fn split_title_and_body(text: &str) -> (String, String) {
    let text = text.trim_start_matches('\n');
    let (first, rest) = match text.split_once('\n') {
        Some((first, rest)) => (first, rest),
        None => (text, ""),
    };
    let title = first.strip_prefix("# ").unwrap_or(first).to_string();
    let body = rest
        .strip_prefix('\n')
        .unwrap_or(rest)
        .trim_end()
        .to_string();
    (title, body)
}

// ---- naming ----

/// A filename-safe slug of a title: lowercase, runs of anything else collapsed
/// to single hyphens.
fn slug(title: &str) -> String {
    let mut out = String::new();
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
        if out.len() >= SLUG_MAX {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "untitled".to_string()
    } else {
        out
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `YYYY-MM-DD-HHMMSS` in UTC. UTC rather than local time because `std` has no
/// timezone database, and this only ever names a file - the pin's own text is
/// what anyone reads.
fn timestamp_prefix(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}-{:02}{:02}{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Days since the Unix epoch -> (year, month, day). Howard Hinnant's
/// `civil_from_days`, which is exact and needs no date library.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: u64, title: &str, body: &str) -> Note {
        Note {
            id,
            title: title.to_string(),
            body: body.to_string(),
            x: 120.0,
            y: 110.0,
            z: 3,
            color: Color::Green,
        }
    }

    /// A scratch root that cleans itself up.
    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("pinz-test-{tag}-{}", now_secs()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            TempRoot(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_pin_round_trips_through_its_file() {
        let n = note(
            1,
            "buy a new lamp",
            "The one by the desk flickers.\n\nSecond para.",
        );
        let parsed = parse_pin(&render_pin(&n), 1);
        assert_eq!(parsed, n);
    }

    #[test]
    fn rendered_pin_has_frontmatter_then_an_h1() {
        let text = render_pin(&note(1, "Hello", "body"));
        assert_eq!(
            text,
            "---\nx: 120\ny: 110\nz: 3\ncolor: green\n---\n# Hello\n\nbody\n"
        );
    }

    #[test]
    fn a_bodyless_pin_round_trips() {
        let n = note(7, "Just a title", "");
        let parsed = parse_pin(&render_pin(&n), 7);
        assert_eq!(parsed.title, "Just a title");
        assert_eq!(parsed.body, "");
    }

    #[test]
    fn a_hand_written_file_without_frontmatter_still_loads() {
        // Someone drops a plain markdown file into a board directory. It should
        // become a pin at the origin, not break the board.
        let parsed = parse_pin("# Scribbled\n\nsome text\n", 4);
        assert_eq!(parsed.title, "Scribbled");
        assert_eq!(parsed.body, "some text");
        assert_eq!((parsed.x, parsed.y, parsed.z), (0.0, 0.0, 0));
        assert_eq!(parsed.color, Color::Yellow);
    }

    #[test]
    fn an_unknown_color_falls_back_rather_than_failing() {
        let parsed = parse_pin("---\ncolor: chartreuse\n---\n# t\n", 1);
        assert_eq!(parsed.color, Color::Yellow);
    }

    #[test]
    fn save_then_load_round_trips_through_disk() {
        let root = TempRoot::new("roundtrip");
        let mut store = FileStore::open(root.path()).unwrap();
        let boards = vec![
            Board {
                name: "ideas".to_string(),
                notes: vec![note(1, "First", "one"), note(2, "Second", "two")],
            },
            Board {
                name: "sketches".to_string(),
                notes: vec![note(3, "Third", "")],
            },
        ];
        store.save(&boards).unwrap();

        let mut fresh = FileStore::open(root.path()).unwrap();
        let loaded = fresh.load().unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].name, "ideas");
        assert_eq!(loaded[1].name, "sketches");
        let titles: Vec<&str> = loaded[0].notes.iter().map(|n| n.title.as_str()).collect();
        assert_eq!(titles, ["First", "Second"]);
        assert_eq!(loaded[0].notes[0].x, 120.0);
        assert_eq!(loaded[0].notes[0].color, Color::Green);
    }

    #[test]
    fn an_unchanged_save_does_not_touch_the_file() {
        let root = TempRoot::new("untouched");
        let mut store = FileStore::open(root.path()).unwrap();
        let boards = vec![Board {
            name: "ideas".to_string(),
            notes: vec![note(1, "Same", "body")],
        }];
        store.save(&boards).unwrap();

        let file = &store.paths[&1].clone();
        let before = fs::metadata(file).unwrap().modified().unwrap();
        store.save(&boards).unwrap();
        let after = fs::metadata(file).unwrap().modified().unwrap();
        assert_eq!(
            before, after,
            "an identical save should not rewrite the file"
        );
    }

    #[test]
    fn deleting_a_pin_deletes_its_file() {
        let root = TempRoot::new("delete");
        let mut store = FileStore::open(root.path()).unwrap();
        let mut boards = vec![Board {
            name: "ideas".to_string(),
            notes: vec![note(1, "Keep", ""), note(2, "Drop", "")],
        }];
        store.save(&boards).unwrap();
        let dropped = store.paths[&2].clone();
        assert!(dropped.exists());

        boards[0].notes.retain(|n| n.id != 2);
        store.save(&boards).unwrap();
        assert!(!dropped.exists(), "the removed pin's file should be gone");
        assert!(store.paths[&1].exists(), "the kept pin should survive");
    }

    #[test]
    fn a_file_the_store_never_loaded_is_left_alone() {
        let root = TempRoot::new("foreign");
        fs::create_dir_all(root.path().join("ideas")).unwrap();
        let foreign = root.path().join("ideas/somebody-elses.md");
        fs::write(&foreign, "# not ours\n").unwrap();

        let mut store = FileStore::open(root.path()).unwrap();
        // Save without ever loading, so the file is unknown to this store.
        store
            .save(&[Board {
                name: "ideas".to_string(),
                notes: vec![note(1, "Ours", "")],
            }])
            .unwrap();
        assert!(foreign.exists(), "never delete what we did not load");
    }

    #[test]
    fn moving_a_pin_to_another_board_moves_its_file() {
        let root = TempRoot::new("move");
        let mut store = FileStore::open(root.path()).unwrap();
        store
            .save(&[
                Board {
                    name: "ideas".to_string(),
                    notes: vec![note(1, "Travels", "")],
                },
                Board {
                    name: "sketches".to_string(),
                    notes: vec![],
                },
            ])
            .unwrap();
        let before = store.paths[&1].clone();

        store
            .save(&[
                Board {
                    name: "ideas".to_string(),
                    notes: vec![],
                },
                Board {
                    name: "sketches".to_string(),
                    notes: vec![note(1, "Travels", "")],
                },
            ])
            .unwrap();

        assert!(!before.exists(), "the old file should be gone");
        let after = store.paths[&1].clone();
        assert!(after.exists());
        assert_eq!(after.parent().unwrap().file_name().unwrap(), "sketches");
    }

    #[test]
    fn an_emptied_board_keeps_a_marker_so_it_survives_a_sync() {
        let root = TempRoot::new("empty");
        let mut store = FileStore::open(root.path()).unwrap();
        store
            .save(&[Board {
                name: "ideas".to_string(),
                notes: vec![],
            }])
            .unwrap();
        assert!(root.path().join("ideas/.gitkeep").exists());

        let mut fresh = FileStore::open(root.path()).unwrap();
        let loaded = fresh.load().unwrap();
        assert_eq!(loaded.len(), 1, "the empty board still shows up");
        assert!(loaded[0].notes.is_empty());
    }

    #[test]
    fn hidden_directories_are_not_boards() {
        let root = TempRoot::new("hidden");
        fs::create_dir_all(root.path().join(".git")).unwrap();
        fs::create_dir_all(root.path().join("ideas")).unwrap();
        let mut store = FileStore::open(root.path()).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "ideas");
    }

    #[test]
    fn slug_is_filename_safe_and_bounded() {
        assert_eq!(slug("Buy a new lamp"), "buy-a-new-lamp");
        assert_eq!(slug("  weird///chars!! "), "weird-chars");
        assert_eq!(slug("åäö"), "untitled", "non-ascii collapses away");
        assert_eq!(slug(""), "untitled");
        assert!(slug(&"x".repeat(200)).len() <= SLUG_MAX);
    }

    #[test]
    fn timestamp_prefix_matches_a_known_epoch_second() {
        // Values cross-checked against `date -u -r <secs>`.
        assert_eq!(timestamp_prefix(1_785_594_622), "2026-08-01-143022");
        assert_eq!(timestamp_prefix(1_785_508_222), "2026-07-31-143022");
        // The epoch itself, and a leap day.
        assert_eq!(timestamp_prefix(0), "1970-01-01-000000");
        assert_eq!(timestamp_prefix(1_709_164_800), "2024-02-29-000000");
    }
}
