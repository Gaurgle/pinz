//! Git sync for the pin repo: pull on start, commit and push on change.
//!
//! This is pinz's own sync, not a hook into anything else. It only ever runs
//! against the pin root (`~/pinz` by default), which contains nothing but pins -
//! that isolation is the whole reason the pins live in their own repo instead of
//! riding along in a notes repo, where an auto-push would sweep up unrelated
//! work in progress and an auto-pull would be blocked by it.
//!
//! It lives in the core rather than a renderer so the standalone TUI and the
//! Epoz tab sync identically. Shelling out to `git` keeps the crate dependency
//! free and means the repo stays an ordinary repo you can fix by hand.
//!
//! The governing rule is **stop rather than guess**. A fetch that fails (offline,
//! no remote, no upstream) is not an error: pinz says so and carries on with
//! local files. But anything that would need a judgement call about whose
//! version of a pin wins leaves the repo exactly as it was and hands it back to
//! you.

use std::path::{Path, PathBuf};
use std::process::Command;

/// What a sync step did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncOutcome {
    /// Nothing needed doing, or nothing could be done without a network. Not an
    /// error: the app carries on with local files.
    Idle(String),
    /// Something happened, and it worked.
    Done(String),
    /// Deliberately stopped, leaving the repo untouched. Needs a human.
    Stopped(String),
}

impl SyncOutcome {
    /// A one-line summary, suitable for printing before the TUI takes the
    /// screen.
    pub fn message(&self) -> &str {
        match self {
            SyncOutcome::Idle(m) | SyncOutcome::Done(m) | SyncOutcome::Stopped(m) => m,
        }
    }

    pub fn is_stopped(&self) -> bool {
        matches!(self, SyncOutcome::Stopped(_))
    }
}

/// Git operations scoped to one directory.
pub struct Sync {
    root: PathBuf,
}

/// A finished `git` invocation.
struct Run {
    ok: bool,
    stdout: String,
    stderr: String,
}

impl Sync {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn git(&self, args: &[&str]) -> Option<Run> {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .output()
            .ok()?;
        Some(Run {
            ok: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).trim().to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        })
    }

    pub fn is_repo(&self) -> bool {
        self.git(&["rev-parse", "--git-dir"])
            .map(|r| r.ok)
            .unwrap_or(false)
    }

    fn has_remote(&self) -> bool {
        self.git(&["remote"])
            .map(|r| r.ok && !r.stdout.is_empty())
            .unwrap_or(false)
    }

    fn has_upstream(&self) -> bool {
        self.git(&["rev-parse", "--abbrev-ref", "@{u}"])
            .map(|r| r.ok)
            .unwrap_or(false)
    }

    /// Commits behind and ahead of the upstream, or `None` without one.
    fn behind_ahead(&self) -> Option<(u32, u32)> {
        let out = self.git(&["rev-list", "--left-right", "--count", "@{u}...HEAD"])?;
        if !out.ok {
            return None;
        }
        let mut parts = out.stdout.split_whitespace();
        let behind = parts.next()?.parse().ok()?;
        let ahead = parts.next()?.parse().ok()?;
        Some((behind, ahead))
    }

    /// Turn the pin directory into a git repo if it isn't one. Local only: the
    /// remote is yours to create and name.
    pub fn init(&self) -> SyncOutcome {
        if self.is_repo() {
            return SyncOutcome::Idle("already a git repo".into());
        }
        match self.git(&["init"]) {
            Some(r) if r.ok => SyncOutcome::Done(format!("initialized {}", self.root.display())),
            Some(r) => SyncOutcome::Stopped(format!("git init failed: {}", first_line(&r.stderr))),
            None => SyncOutcome::Stopped("git is not on PATH".into()),
        }
    }

    /// Bring in whatever the other machine pushed.
    ///
    /// Fast-forwards when only the remote moved. When both ends have commits,
    /// replays the local ones on top; if that can't be done cleanly the rebase
    /// is aborted, so the working tree is exactly as it was before the call.
    pub fn pull(&self) -> SyncOutcome {
        if !self.is_repo() {
            return SyncOutcome::Idle("not a git repo yet".into());
        }
        if !self.has_remote() {
            return SyncOutcome::Idle("no remote configured".into());
        }
        match self.git(&["fetch", "--quiet"]) {
            Some(r) if r.ok => {}
            // Offline, or the remote is unreachable. Local files are still fine.
            Some(r) => return SyncOutcome::Idle(format!("could not fetch: {}", first_line(&r.stderr))),
            None => return SyncOutcome::Idle("git is not on PATH".into()),
        }
        if !self.has_upstream() {
            return SyncOutcome::Idle("no upstream branch set".into());
        }
        let Some((behind, ahead)) = self.behind_ahead() else {
            return SyncOutcome::Idle("could not compare with the remote".into());
        };
        if behind == 0 {
            return SyncOutcome::Idle("already up to date".into());
        }
        if ahead == 0 {
            return match self.git(&["merge", "--ff-only", "@{u}"]) {
                Some(r) if r.ok => SyncOutcome::Done(format!("pulled {behind} commit(s)")),
                Some(r) => SyncOutcome::Stopped(format!(
                    "could not fast-forward: {}",
                    first_line(&r.stderr)
                )),
                None => SyncOutcome::Stopped("git is not on PATH".into()),
            };
        }
        match self.git(&["rebase", "@{u}"]) {
            Some(r) if r.ok => SyncOutcome::Done(format!("replayed {ahead} local commit(s)")),
            Some(_) => {
                // Leave no half-finished rebase behind.
                let _ = self.git(&["rebase", "--abort"]);
                SyncOutcome::Stopped(format!(
                    "the same pin changed on both machines - {} local and {behind} remote commit(s) conflict; resolve in {}",
                    ahead,
                    self.root.display()
                ))
            }
            None => SyncOutcome::Stopped("git is not on PATH".into()),
        }
    }

    /// Commit whatever changed and push it, if there is anywhere to push to.
    pub fn push(&self, message: &str) -> SyncOutcome {
        if !self.is_repo() {
            return SyncOutcome::Idle("not a git repo yet".into());
        }
        match self.git(&["add", "-A"]) {
            Some(r) if r.ok => {}
            Some(r) => return SyncOutcome::Stopped(format!("git add failed: {}", first_line(&r.stderr))),
            None => return SyncOutcome::Idle("git is not on PATH".into()),
        }

        // `diff --cached --quiet` exits non-zero when something is staged.
        let staged = self
            .git(&["diff", "--cached", "--quiet"])
            .map(|r| !r.ok)
            .unwrap_or(false);
        if staged {
            match self.git(&["commit", "-m", message]) {
                Some(r) if r.ok => {}
                Some(r) => {
                    return SyncOutcome::Stopped(format!(
                        "git commit failed: {}",
                        first_line(&if r.stderr.is_empty() { r.stdout } else { r.stderr })
                    ))
                }
                None => return SyncOutcome::Idle("git is not on PATH".into()),
            }
        }

        if !self.has_remote() {
            let what = if staged { "committed" } else { "nothing to commit" };
            return SyncOutcome::Idle(format!("{what}; no remote to push to"));
        }
        // Nothing new here and nothing waiting: say so rather than claiming a
        // push that moved no commits.
        if !staged && self.behind_ahead().map(|(_, ahead)| ahead == 0).unwrap_or(false) {
            return SyncOutcome::Idle("nothing to sync".into());
        }
        // A repo that has never been pushed has no upstream to push against.
        let args: Vec<&str> = if self.has_upstream() {
            vec!["push", "--quiet"]
        } else {
            vec!["push", "--quiet", "-u", "origin", "HEAD"]
        };
        match self.git(&args) {
            Some(r) if r.ok => SyncOutcome::Done(if staged {
                "committed and pushed".into()
            } else {
                "pushed".into()
            }),
            Some(r) => SyncOutcome::Stopped(format!("push failed: {}", first_line(&r.stderr))),
            None => SyncOutcome::Idle("git is not on PATH".into()),
        }
    }
}

fn first_line(s: &str) -> &str {
    s.lines().find(|l| !l.trim().is_empty()).unwrap_or(s).trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A scratch directory that cleans itself up.
    struct Temp(PathBuf);

    impl Temp {
        fn new(tag: &str) -> Self {
            let n = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let dir = std::env::temp_dir().join(format!("pinz-sync-{tag}-{n}"));
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

    fn git_in(dir: &Path, args: &[&str]) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// A repo with an identity set, so committing works on a bare CI box too.
    fn init_repo(dir: &Path) {
        assert!(git_in(dir, &["init", "--quiet"]));
        git_in(dir, &["config", "user.name", "pinz test"]);
        git_in(dir, &["config", "user.email", "pinz@test.local"]);
    }

    fn write_pin(dir: &Path, board: &str, name: &str, body: &str) {
        let d = dir.join(board);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join(name), body).unwrap();
    }

    #[test]
    fn a_directory_that_is_not_a_repo_is_idle_never_an_error() {
        let t = Temp::new("norepo");
        let sync = Sync::new(t.path());
        assert!(!sync.is_repo());
        assert!(matches!(sync.pull(), SyncOutcome::Idle(_)));
        assert!(matches!(sync.push("x"), SyncOutcome::Idle(_)));
    }

    #[test]
    fn init_makes_a_repo_and_is_idempotent() {
        let t = Temp::new("init");
        let sync = Sync::new(t.path());
        assert!(matches!(sync.init(), SyncOutcome::Done(_)));
        assert!(sync.is_repo());
        assert!(matches!(sync.init(), SyncOutcome::Idle(_)));
    }

    #[test]
    fn push_commits_locally_when_there_is_no_remote() {
        let t = Temp::new("nolocal");
        init_repo(t.path());
        write_pin(t.path(), "ideas", "a.md", "# a\n");

        let sync = Sync::new(t.path());
        let out = sync.push("pinz: update pins");
        assert!(matches!(out, SyncOutcome::Idle(_)), "got {out:?}");
        assert!(out.message().contains("no remote"));

        // The commit still happened, so nothing is lost while offline.
        let log = Command::new("git")
            .arg("-C")
            .arg(t.path())
            .args(["log", "--oneline"])
            .output()
            .unwrap();
        let log = String::from_utf8_lossy(&log.stdout);
        assert!(log.contains("pinz: update pins"), "log was:\n{log}");
    }

    #[test]
    fn a_second_push_with_nothing_changed_does_not_commit_again() {
        let t = Temp::new("noop");
        init_repo(t.path());
        write_pin(t.path(), "ideas", "a.md", "# a\n");
        let sync = Sync::new(t.path());
        sync.push("first");
        sync.push("second");

        let log = Command::new("git")
            .arg("-C")
            .arg(t.path())
            .args(["log", "--oneline"])
            .output()
            .unwrap();
        let count = String::from_utf8_lossy(&log.stdout).lines().count();
        assert_eq!(count, 1, "an unchanged push should add no commit");
    }

    #[test]
    fn a_pin_pushed_from_one_machine_arrives_on_the_other() {
        let remote = Temp::new("remote");
        assert!(git_in(remote.path(), &["init", "--bare", "--quiet"]));

        let a = Temp::new("machine-a");
        init_repo(a.path());
        git_in(a.path(), &["remote", "add", "origin", &remote.path().to_string_lossy()]);
        write_pin(a.path(), "ideas", "first.md", "# from a\n");
        let out = Sync::new(a.path()).push("pinz: from a");
        assert!(matches!(out, SyncOutcome::Done(_)), "got {out:?}");

        // Machine B clones, then A adds another pin and pushes.
        let b = Temp::new("machine-b");
        assert!(Command::new("git")
            .args(["clone", "--quiet"])
            .arg(remote.path())
            .arg(b.path())
            .output()
            .unwrap()
            .status
            .success());
        git_in(b.path(), &["config", "user.name", "pinz test"]);
        git_in(b.path(), &["config", "user.email", "pinz@test.local"]);

        write_pin(a.path(), "ideas", "second.md", "# also from a\n");
        Sync::new(a.path()).push("pinz: second from a");

        let out = Sync::new(b.path()).pull();
        assert!(matches!(out, SyncOutcome::Done(_)), "got {out:?}");
        assert!(b.path().join("ideas/second.md").exists(), "the pin should have arrived");
    }

    #[test]
    fn a_push_with_nothing_to_send_says_so_rather_than_claiming_a_push() {
        let remote = Temp::new("remote3");
        assert!(git_in(remote.path(), &["init", "--bare", "--quiet"]));
        let a = Temp::new("uptodate");
        init_repo(a.path());
        git_in(a.path(), &["remote", "add", "origin", &remote.path().to_string_lossy()]);
        write_pin(a.path(), "ideas", "a.md", "# a\n");
        assert!(matches!(Sync::new(a.path()).push("first"), SyncOutcome::Done(_)));

        let out = Sync::new(a.path()).push("second");
        assert!(matches!(out, SyncOutcome::Idle(_)), "got {out:?}");
        assert_eq!(out.message(), "nothing to sync");
    }

    #[test]
    fn the_same_pin_edited_on_both_machines_stops_and_leaves_the_repo_alone() {
        let remote = Temp::new("remote2");
        assert!(git_in(remote.path(), &["init", "--bare", "--quiet"]));

        let a = Temp::new("conflict-a");
        init_repo(a.path());
        git_in(a.path(), &["remote", "add", "origin", &remote.path().to_string_lossy()]);
        write_pin(a.path(), "ideas", "shared.md", "# original\n");
        Sync::new(a.path()).push("pinz: original");

        let b = Temp::new("conflict-b");
        assert!(Command::new("git")
            .args(["clone", "--quiet"])
            .arg(remote.path())
            .arg(b.path())
            .output()
            .unwrap()
            .status
            .success());
        git_in(b.path(), &["config", "user.name", "pinz test"]);
        git_in(b.path(), &["config", "user.email", "pinz@test.local"]);

        // Both machines edit the same pin; A gets there first.
        write_pin(a.path(), "ideas", "shared.md", "# edited on a\n");
        Sync::new(a.path()).push("pinz: from a");
        write_pin(b.path(), "ideas", "shared.md", "# edited on b\n");
        Sync::new(b.path()).push("pinz: from b"); // commits, push is rejected

        let out = Sync::new(b.path()).pull();
        assert!(out.is_stopped(), "a real conflict must stop, got {out:?}");

        // The abort must leave no rebase in progress and B's own edit intact.
        let status = Command::new("git")
            .arg("-C")
            .arg(b.path())
            .args(["status", "--porcelain=v2", "--branch"])
            .output()
            .unwrap();
        let status = String::from_utf8_lossy(&status.stdout);
        assert!(!status.contains("rebase"), "left mid-rebase:\n{status}");
        let kept = fs::read_to_string(b.path().join("ideas/shared.md")).unwrap();
        assert_eq!(kept, "# edited on b\n", "local work must survive the stop");
    }
}
