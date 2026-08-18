//! The board's write lock: one writer per board, on one machine.
//!
//! Two pinz instances open on the same machine share one directory, so git
//! never sees them clash and cannot help. The second instance holds a pin as it
//! was when it started, and rewrites the file when it saves - silently undoing
//! the first instance's edit. This module is what prevents that: the first
//! instance takes `.pinz-lock` in the pin root and may write; later ones learn
//! they are not the owner and open read-only.
//!
//! Deliberately machine-local. Two *machines* are git's problem and are handled
//! by the pin merge; `.pinz-lock` is gitignored and must never sync.
//!
//! The lock is advisory and fails open: if the file cannot be created at all
//! (a read-only or odd filesystem), the session still owns its board. Locking
//! someone out of their own pins over a filesystem quirk is the worse failure.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// The lock file's name inside the pin root.
pub const LOCK_FILE: &str = ".pinz-lock";

/// The result of trying to take the board's write lock.
#[derive(Debug)]
pub enum Ownership {
    /// This process owns the board and may write. Released on drop.
    Owner(BoardLock),
    /// Another live pinz owns it, so this session must stay read-only.
    Busy { pid: u32 },
}

impl Ownership {
    pub fn is_owner(&self) -> bool {
        matches!(self, Ownership::Owner(_))
    }
}

/// A held write lock. Removes its file when dropped, including while a panic
/// unwinds, so a crash does not strand the board.
#[derive(Debug)]
pub struct BoardLock {
    /// `None` when the filesystem would not take a lock file: the session owns
    /// the board anyway, and has nothing to clean up.
    path: Option<PathBuf>,
}

impl BoardLock {
    /// Try to become the writer for the board rooted at `root`.
    ///
    /// A lock naming a process that is no longer running is stale and gets
    /// taken over, so a hard kill never locks you out permanently.
    pub fn acquire(root: impl AsRef<Path>) -> Ownership {
        let root = root.as_ref();
        ensure_ignored(root);
        let path = root.join(LOCK_FILE);
        match write_new(&path) {
            Written::Ok => return Ownership::Owner(BoardLock { path: Some(path) }),
            Written::Failed => return Ownership::Owner(BoardLock { path: None }),
            Written::Exists => {}
        }

        // Someone got there first - unless they are gone, in which case the
        // file is debris from a crash and we take it over. A lock naming a live
        // process is honoured even when that pid is our own: an owner we cannot
        // tell apart from ourselves is still an owner, and the alternative
        // would let a second instance steal a lock that is genuinely held.
        match read_pid(&path) {
            Some(pid) if is_running(pid) => Ownership::Busy { pid },
            _ => {
                let _ = fs::remove_file(&path);
                match write_new(&path) {
                    Written::Ok => Ownership::Owner(BoardLock { path: Some(path) }),
                    // Lost the race to a third instance, or cannot write.
                    Written::Exists => match read_pid(&path) {
                        Some(pid) => Ownership::Busy { pid },
                        None => Ownership::Owner(BoardLock { path: None }),
                    },
                    Written::Failed => Ownership::Owner(BoardLock { path: None }),
                }
            }
        }
    }
}

impl Drop for BoardLock {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = fs::remove_file(path);
        }
    }
}

enum Written {
    Ok,
    /// Someone else holds it.
    Exists,
    /// The filesystem would not take the file at all.
    Failed,
}

/// Create the lock file, failing rather than truncating if it already exists.
fn write_new(path: &Path) -> Written {
    use std::io::Write;
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => {
            let body = format!("pid: {}\nsince: {}\n", std::process::id(), now_secs());
            match file.write_all(body.as_bytes()) {
                Ok(()) => Written::Ok,
                Err(_) => Written::Failed,
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Written::Exists,
        Err(_) => Written::Failed,
    }
}

/// Keep the lock file out of git.
///
/// `pinz sync` stages everything, so an un-ignored lock would be committed and
/// carried to the other machine, where its pid means nothing and would lock a
/// board nobody is using. Appends rather than writes, because the pin repo is
/// the user's and may already ignore things of their own.
fn ensure_ignored(root: &Path) {
    use std::io::Write;
    let path = root.join(".gitignore");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    if existing.lines().any(|line| line.trim() == LOCK_FILE) {
        return;
    }
    let prefix = if existing.is_empty() || existing.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{prefix}{LOCK_FILE}");
    }
}

/// The pid a lock file names, if it names a readable one. An unreadable or
/// malformed file has no identifiable owner, which counts as stale.
fn read_pid(path: &Path) -> Option<u32> {
    let text = fs::read_to_string(path).ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("pid:"))
        .and_then(|v| v.trim().parse().ok())
}

/// Is a process still running? `kill -0` signals nothing and only reports
/// whether the process is there, which is exactly the question.
fn is_running(pid: u32) -> bool {
    Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);

    impl Temp {
        fn new(tag: &str) -> Self {
            let n = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let dir = std::env::temp_dir().join(format!("pinz-lock-{tag}-{n}"));
            fs::create_dir_all(&dir).unwrap();
            Temp(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A pid that is certainly not running: spawn a process and reap it.
    fn dead_pid() -> u32 {
        let mut child = Command::new("/usr/bin/true")
            .spawn()
            .or_else(|_| Command::new("/bin/echo").spawn())
            .expect("spawn a short-lived process");
        let pid = child.id();
        child.wait().unwrap();
        pid
    }

    #[test]
    fn the_first_instance_owns_the_board() {
        let t = Temp::new("first");
        let held = BoardLock::acquire(t.path());
        assert!(held.is_owner(), "got {held:?}");
        assert!(
            t.path().join(LOCK_FILE).exists(),
            "the lock file is written"
        );
    }

    #[test]
    fn a_second_instance_is_told_who_owns_it() {
        let t = Temp::new("second");
        let _first = BoardLock::acquire(t.path());
        match BoardLock::acquire(t.path()) {
            Ownership::Busy { pid } => assert_eq!(pid, std::process::id(), "names the holder"),
            other => panic!("a second instance must not own the board: {other:?}"),
        }
    }

    #[test]
    fn releasing_lets_the_next_instance_in() {
        let t = Temp::new("release");
        let first = BoardLock::acquire(t.path());
        assert!(first.is_owner());
        drop(first);
        assert!(
            !t.path().join(LOCK_FILE).exists(),
            "dropping removes the lock file"
        );
        assert!(
            BoardLock::acquire(t.path()).is_owner(),
            "the next one gets in"
        );
    }

    #[test]
    fn a_lock_left_by_a_dead_process_is_taken_over() {
        let t = Temp::new("stale");
        fs::write(
            t.path().join(LOCK_FILE),
            format!("pid: {}\nsince: 0\n", dead_pid()),
        )
        .unwrap();
        assert!(
            BoardLock::acquire(t.path()).is_owner(),
            "a hard kill must not lock you out forever"
        );
    }

    #[test]
    fn taking_the_lock_keeps_it_out_of_git() {
        // `pinz sync` stages everything, so an un-ignored lock file would be
        // committed and shipped to the other machine, where its pid is
        // meaningless and would lock the board for no reason.
        let t = Temp::new("ignore");
        let _held = BoardLock::acquire(t.path());
        let ignore = fs::read_to_string(t.path().join(".gitignore")).unwrap();
        assert!(ignore.contains(LOCK_FILE), "got {ignore:?}");
    }

    #[test]
    fn an_existing_gitignore_is_added_to_once_not_clobbered() {
        let t = Temp::new("ignore-existing");
        fs::write(t.path().join(".gitignore"), "something-else\n").unwrap();
        drop(BoardLock::acquire(t.path()));
        drop(BoardLock::acquire(t.path()));
        let ignore = fs::read_to_string(t.path().join(".gitignore")).unwrap();
        assert!(ignore.contains("something-else"), "kept: {ignore:?}");
        assert_eq!(
            ignore.matches(LOCK_FILE).count(),
            1,
            "added exactly once: {ignore:?}"
        );
    }

    #[test]
    fn a_lock_file_with_no_readable_owner_is_taken_over() {
        let t = Temp::new("garbage");
        fs::write(t.path().join(LOCK_FILE), "not a lock file\n").unwrap();
        assert!(BoardLock::acquire(t.path()).is_owner());
    }
}
