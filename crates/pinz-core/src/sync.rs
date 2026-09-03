//! Git sync for the pin repo: pull on start, commit and push on change.
//!
//! This is pinz's own sync, not a hook into anything else. It only ever runs
//! against the pin root (`~/pinz-board` by default), which contains nothing but
//! pins - that isolation is the whole reason they live in their own repo instead of
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

use crate::merge::merge_pin;
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

/// What git currently thinks of the pin repo. Cheap to take, so a command can
/// decide what actually needs doing instead of firing every step blindly.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyncStatus {
    pub is_repo: bool,
    pub has_remote: bool,
    pub has_upstream: bool,
    /// Files changed since the last commit.
    pub dirty: usize,
    /// Commits waiting on the remote, and waiting to be sent.
    pub behind: u32,
    pub ahead: u32,
}

impl SyncStatus {
    /// Is there anything at all to do?
    pub fn is_settled(&self) -> bool {
        self.dirty == 0 && self.behind == 0 && self.ahead == 0
    }

    /// One line for a human: what state the repo is in.
    pub fn summary(&self) -> String {
        if !self.is_repo {
            return "not a git repo yet".into();
        }
        let mut parts = Vec::new();
        if self.dirty > 0 {
            parts.push(format!("{} uncommitted change(s)", self.dirty));
        }
        if self.behind > 0 {
            parts.push(format!("{} to pull", self.behind));
        }
        if self.ahead > 0 {
            parts.push(format!("{} to push", self.ahead));
        }
        if parts.is_empty() {
            return if self.has_remote {
                "in sync".into()
            } else {
                "up to date locally (no remote)".into()
            };
        }
        parts.join(", ")
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

    /// Where a push goes, as git has it written down, or `None` with no remote.
    ///
    /// A fact rather than a phrasing: the raw URL, for a caller to shorten or
    /// print however suits it.
    pub fn remote_url(&self) -> Option<String> {
        let out = self.git(&["remote", "get-url", "origin"])?;
        (out.ok && !out.stdout.is_empty()).then_some(out.stdout)
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

    /// How many commits the current branch carries. Zero for a directory that
    /// is not a repo, and zero for a repo whose first commit has not happened
    /// yet - `init` makes the repo, the first `sync` makes the commit.
    pub fn commit_count(&self) -> u32 {
        self.git(&["rev-list", "--count", "HEAD"])
            .filter(|r| r.ok)
            .and_then(|r| r.stdout.parse().ok())
            .unwrap_or(0)
    }

    /// Ask the remote what it has, without changing anything locally. Failure
    /// here means offline or no remote, which is never fatal.
    pub fn fetch(&self) -> SyncOutcome {
        if !self.is_repo() {
            return SyncOutcome::Idle("not a git repo yet".into());
        }
        if !self.has_remote() {
            return SyncOutcome::Idle("no remote configured".into());
        }
        match self.git(&["fetch", "--quiet"]) {
            Some(r) if r.ok => SyncOutcome::Done("fetched".into()),
            Some(r) => SyncOutcome::Idle(format!("could not fetch: {}", first_line(&r.stderr))),
            None => SyncOutcome::Idle("git is not on PATH".into()),
        }
    }

    /// A snapshot of the repo's state. Reads only what git already knows - call
    /// [`Sync::fetch`] first if the remote counts need to be current.
    pub fn status(&self) -> SyncStatus {
        if !self.is_repo() {
            return SyncStatus::default();
        }
        let dirty = self
            .git(&["status", "--porcelain"])
            .filter(|r| r.ok)
            .map(|r| r.stdout.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0);
        let (behind, ahead) = self.behind_ahead().unwrap_or((0, 0));
        SyncStatus {
            is_repo: true,
            has_remote: self.has_remote(),
            has_upstream: self.has_upstream(),
            dirty,
            behind,
            ahead,
        }
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
        // Not a repo, no remote, or offline: all reported as idle, because local
        // files are still perfectly fine to work with.
        if let SyncOutcome::Idle(why) = self.fetch() {
            return SyncOutcome::Idle(why);
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
            Some(_) => self.resolve_conflicted_rebase(behind, ahead),
            None => SyncOutcome::Stopped("git is not on PATH".into()),
        }
    }

    /// A rebase has stopped on conflicts. Try to settle each conflicted pin
    /// with [`merge_pin`]; if every one resolves, the rebase continues,
    /// otherwise it is aborted so the repo is exactly as it was before the
    /// pull. Cosmetics (position, color) are never worth stopping a sync over;
    /// a real content conflict still is.
    fn resolve_conflicted_rebase(&self, behind: u32, ahead: u32) -> SyncOutcome {
        let stop = || {
            // Leave no half-finished rebase behind.
            let _ = self.git(&["rebase", "--abort"]);
            SyncOutcome::Stopped(format!(
                "the same pin changed on both machines - {ahead} local and {behind} remote commit(s) conflict; resolve in {}",
                self.root.display()
            ))
        };

        // Each pass settles one replayed commit's conflicts; `--continue` then
        // either finishes or stops on the next commit. Bounded by `ahead`
        // because that is every commit the rebase can possibly stop on.
        let mut merged_pins = 0usize;
        for _ in 0..ahead {
            let Some(conflicts) = self.conflicted_pin_files() else {
                return stop();
            };
            for file in &conflicts {
                if !self.resolve_pin_conflict(file) {
                    return stop();
                }
            }
            merged_pins += conflicts.len();
            // `core.editor=true` keeps the reworded-commit editor from ever
            // opening; the message is kept as it was.
            match self.git(&["-c", "core.editor=true", "rebase", "--continue"]) {
                Some(r) if r.ok => {
                    return SyncOutcome::Done(format!(
                        "replayed {ahead} local commit(s), auto-merged {merged_pins} pin(s)"
                    ));
                }
                Some(_) => {} // stopped on the next commit; go around again
                None => return stop(),
            }
        }
        stop()
    }

    /// The files the stopped rebase is conflicted on - but only if every one
    /// of them is a both-modified `.md` file. Any other conflict shape (a
    /// delete against an edit, both machines adding different files, a
    /// non-pin file) means this is not ours to settle: `None`.
    fn conflicted_pin_files(&self) -> Option<Vec<String>> {
        // `-z` gives NUL-separated entries with unquoted paths, so board names
        // with spaces survive.
        let out = self.git(&["status", "--porcelain", "-z"])?;
        if !out.ok {
            return None;
        }
        let mut conflicts = Vec::new();
        for entry in out.stdout.split('\0').filter(|e| e.len() > 3) {
            let (code, path) = entry.split_at(2);
            let path = path.trim_start();
            if !code.contains('U') && code != "AA" && code != "DD" {
                continue; // not a conflict entry (a cleanly-applied file)
            }
            if code != "UU" || !path.ends_with(".md") {
                return None;
            }
            conflicts.push(path.to_string());
        }
        if conflicts.is_empty() {
            return None; // stopped for a reason we do not understand
        }
        Some(conflicts)
    }

    /// Settle one both-modified pin file and stage the result. During a rebase
    /// stage 2 is the upstream (remote) side and stage 3 is the local commit
    /// being replayed; stage 1, the common ancestor, can be absent.
    fn resolve_pin_conflict(&self, file: &str) -> bool {
        let show = |stage: char| {
            self.git(&["show", &format!(":{stage}:{file}")])
                .filter(|r| r.ok)
                .map(|r| r.stdout)
        };
        let base = show('1');
        let (Some(remote), Some(local)) = (show('2'), show('3')) else {
            return false;
        };
        let Some(merged) = merge_pin(base.as_deref(), &remote, &local) else {
            return false;
        };
        if std::fs::write(self.root.join(file), merged).is_err() {
            return false;
        }
        self.git(&["add", "--", file]).is_some_and(|r| r.ok)
    }

    /// Checkpoint whatever changed, without sending it anywhere.
    ///
    /// Worth its own step because git refuses to pull over uncommitted changes
    /// to a file the other machine also touched - so a board with unsaved edits
    /// could not receive the other machine's pins at all. Committing first
    /// turns that refusal into an ordinary rebase, which [`Sync::pull`] can
    /// usually settle by itself. The commit is free: the pins are already on
    /// disk, and quitting would have committed them anyway.
    pub fn commit(&self, message: &str) -> SyncOutcome {
        match self.stage_and_commit(message) {
            Err(outcome) => outcome,
            Ok(false) => SyncOutcome::Idle("nothing to commit".into()),
            Ok(true) => SyncOutcome::Done("committed local pins".into()),
        }
    }

    /// Stage everything and commit it if anything was staged, reporting whether
    /// a commit happened. `Err` carries the outcome the caller should return.
    fn stage_and_commit(&self, message: &str) -> std::result::Result<bool, SyncOutcome> {
        if !self.is_repo() {
            return Err(SyncOutcome::Idle("not a git repo yet".into()));
        }
        match self.git(&["add", "-A"]) {
            Some(r) if r.ok => {}
            Some(r) => {
                return Err(SyncOutcome::Stopped(format!(
                    "git add failed: {}",
                    first_line(&r.stderr)
                )))
            }
            None => return Err(SyncOutcome::Idle("git is not on PATH".into())),
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
                    return Err(SyncOutcome::Stopped(format!(
                        "git commit failed: {}",
                        first_line(&if r.stderr.is_empty() {
                            r.stdout
                        } else {
                            r.stderr
                        })
                    )))
                }
                None => return Err(SyncOutcome::Idle("git is not on PATH".into())),
            }
        }
        Ok(staged)
    }

    /// Commit whatever changed and push it, if there is anywhere to push to.
    pub fn push(&self, message: &str) -> SyncOutcome {
        let staged = match self.stage_and_commit(message) {
            Ok(staged) => staged,
            Err(outcome) => return outcome,
        };

        if !self.has_remote() {
            let what = if staged {
                "committed"
            } else {
                "nothing to commit"
            };
            return SyncOutcome::Idle(format!("{what}; no remote to push to"));
        }
        // Nothing new here and nothing waiting: say so rather than claiming a
        // push that moved no commits.
        if !staged
            && self
                .behind_ahead()
                .map(|(_, ahead)| ahead == 0)
                .unwrap_or(false)
        {
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

/// Clone a pin repo from `url` into `dest`, which must not exist yet.
///
/// A free function rather than a method: every other git operation here acts
/// on a board that is already on disk, and this one is what puts it there.
pub fn clone_into(url: &str, dest: &Path) -> SyncOutcome {
    if dest.exists() {
        return SyncOutcome::Stopped(format!("{} already exists", dest.display()));
    }
    let out = Command::new("git").arg("clone").arg(url).arg(dest).output();
    match out {
        Ok(o) if o.status.success() => SyncOutcome::Done(format!("cloned into {}", dest.display())),
        // git already says why in a sentence a person can act on; passing it
        // through beats paraphrasing a message we did not write.
        Ok(o) => {
            SyncOutcome::Stopped(first_line(&String::from_utf8_lossy(&o.stderr)).to_string())
        }
        Err(e) => SyncOutcome::Stopped(format!("could not run git: {e}")),
    }
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
    fn commit_count_counts_what_is_on_the_branch() {
        let t = Temp::new("count");
        init_repo(t.path());
        let sync = Sync::new(t.path());
        assert_eq!(sync.commit_count(), 0, "nothing committed yet");
        write_pin(t.path(), "ideas", "one.md", "# one");
        assert!(matches!(sync.commit("first"), SyncOutcome::Done(_)));
        assert_eq!(sync.commit_count(), 1);
        write_pin(t.path(), "ideas", "two.md", "# two");
        assert!(matches!(sync.commit("second"), SyncOutcome::Done(_)));
        assert_eq!(sync.commit_count(), 2);
    }

    #[test]
    fn commit_count_is_zero_for_a_directory_that_is_not_a_repo() {
        let t = Temp::new("count-norepo");
        assert_eq!(Sync::new(t.path()).commit_count(), 0);
    }

    #[test]
    fn clone_into_brings_a_board_down_from_a_url() {
        let src = Temp::new("clone-src");
        init_repo(src.path());
        write_pin(src.path(), "ideas", "one.md", "# one");
        assert!(matches!(
            Sync::new(src.path()).commit("seed"),
            SyncOutcome::Done(_)
        ));

        let parent = Temp::new("clone-dest");
        let dest = parent.path().join("board");
        let url = src.path().to_string_lossy().to_string();
        assert!(matches!(clone_into(&url, &dest), SyncOutcome::Done(_)));
        assert!(dest.join("ideas/one.md").is_file(), "the pin came with it");
        assert!(Sync::new(&dest).is_repo());
        assert_eq!(Sync::new(&dest).commit_count(), 1);
    }

    #[test]
    fn clone_into_stops_rather_than_panicking_on_a_bad_url() {
        let parent = Temp::new("clone-bad");
        let dest = parent.path().join("board");
        let outcome = clone_into("/definitely/not/a/repo/anywhere", &dest);
        assert!(outcome.is_stopped(), "got {outcome:?}");
        assert!(!dest.exists(), "a failed clone leaves nothing behind");
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
        git_in(
            a.path(),
            &["remote", "add", "origin", &remote.path().to_string_lossy()],
        );
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
        assert!(
            b.path().join("ideas/second.md").exists(),
            "the pin should have arrived"
        );
    }

    #[test]
    fn status_on_a_plain_directory_reports_no_repo() {
        let t = Temp::new("status-norepo");
        let st = Sync::new(t.path()).status();
        assert!(!st.is_repo);
        assert_eq!(st.summary(), "not a git repo yet");
        assert!(st.is_settled(), "nothing to do without a repo");
    }

    #[test]
    fn status_counts_uncommitted_work_then_clears() {
        let t = Temp::new("status-dirty");
        init_repo(t.path());
        write_pin(t.path(), "ideas", "a.md", "# a\n");
        write_pin(t.path(), "ideas", "b.md", "# b\n");

        let sync = Sync::new(t.path());
        let st = sync.status();
        assert!(st.is_repo);
        assert!(!st.has_remote);
        assert!(st.dirty > 0, "untracked pins count as work to commit");
        assert!(!st.is_settled());
        assert!(st.summary().contains("uncommitted"), "got {}", st.summary());

        sync.push("pinz: update pins");
        let st = sync.status();
        assert_eq!(st.dirty, 0);
        assert!(st.is_settled());
        assert_eq!(st.summary(), "up to date locally (no remote)");
    }

    #[test]
    fn status_sees_what_is_waiting_in_both_directions() {
        let remote = Temp::new("status-remote");
        assert!(git_in(remote.path(), &["init", "--bare", "--quiet"]));
        let a = Temp::new("status-a");
        init_repo(a.path());
        git_in(
            a.path(),
            &["remote", "add", "origin", &remote.path().to_string_lossy()],
        );
        write_pin(a.path(), "ideas", "a.md", "# a\n");
        Sync::new(a.path()).push("pinz: first");

        let b = Temp::new("status-b");
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
        assert_eq!(Sync::new(b.path()).status().summary(), "in sync");

        // A pushes something new; B has its own uncommitted pin.
        write_pin(a.path(), "ideas", "second.md", "# second\n");
        Sync::new(a.path()).push("pinz: second");
        write_pin(b.path(), "ideas", "mine.md", "# mine\n");

        let sync_b = Sync::new(b.path());
        sync_b.fetch();
        let st = sync_b.status();
        assert_eq!(st.behind, 1, "one commit waiting to be pulled");
        assert_eq!(st.ahead, 0);
        assert_eq!(st.dirty, 1, "one pin waiting to be committed");
        let summary = st.summary();
        assert!(summary.contains("1 uncommitted"), "got {summary}");
        assert!(summary.contains("1 to pull"), "got {summary}");
    }

    #[test]
    fn a_push_with_nothing_to_send_says_so_rather_than_claiming_a_push() {
        let remote = Temp::new("remote3");
        assert!(git_in(remote.path(), &["init", "--bare", "--quiet"]));
        let a = Temp::new("uptodate");
        init_repo(a.path());
        git_in(
            a.path(),
            &["remote", "add", "origin", &remote.path().to_string_lossy()],
        );
        write_pin(a.path(), "ideas", "a.md", "# a\n");
        assert!(matches!(
            Sync::new(a.path()).push("first"),
            SyncOutcome::Done(_)
        ));

        let out = Sync::new(a.path()).push("second");
        assert!(matches!(out, SyncOutcome::Idle(_)), "got {out:?}");
        assert_eq!(out.message(), "nothing to sync");
    }

    /// The clone-and-configure half of a two-machine setup, shared by the
    /// conflict tests.
    fn clone_repo(remote: &Path, into: &Path) {
        assert!(Command::new("git")
            .args(["clone", "--quiet"])
            .arg(remote)
            .arg(into)
            .output()
            .unwrap()
            .status
            .success());
        git_in(into, &["config", "user.name", "pinz test"]);
        git_in(into, &["config", "user.email", "pinz@test.local"]);
    }

    #[test]
    fn commit_records_local_pins_without_sending_them() {
        let remote = Temp::new("commit-remote");
        assert!(git_in(remote.path(), &["init", "--bare", "--quiet"]));
        let a = Temp::new("commit-a");
        init_repo(a.path());
        git_in(
            a.path(),
            &["remote", "add", "origin", &remote.path().to_string_lossy()],
        );
        write_pin(a.path(), "ideas", "a.md", "# a\n");
        Sync::new(a.path()).push("pinz: first");

        write_pin(a.path(), "ideas", "a.md", "# edited\n");
        let sync = Sync::new(a.path());
        let out = sync.commit("pinz: checkpoint");
        assert!(matches!(out, SyncOutcome::Done(_)), "got {out:?}");

        let status = sync.status();
        assert_eq!(status.dirty, 0, "the edit is committed");
        assert_eq!(status.ahead, 1, "and is waiting to be pushed, not sent");
    }

    #[test]
    fn commit_with_nothing_changed_is_idle() {
        let t = Temp::new("commit-clean");
        init_repo(t.path());
        write_pin(t.path(), "ideas", "a.md", "# a\n");
        let sync = Sync::new(t.path());
        sync.commit("pinz: first");
        let out = sync.commit("pinz: again");
        assert!(matches!(out, SyncOutcome::Idle(_)), "got {out:?}");
    }

    /// The first half of the 2026-08-07 incident: the other machine moved, and
    /// this one has *uncommitted* edits to the same pin. Git refuses to pull
    /// over those, so checkpointing them first is what lets the pull happen at
    /// all - and the pin merge then settles the rest.
    #[test]
    fn committing_first_rescues_a_pull_that_uncommitted_edits_would_refuse() {
        let remote = Temp::new("dirty-remote");
        assert!(git_in(remote.path(), &["init", "--bare", "--quiet"]));
        let a = Temp::new("dirty-a");
        init_repo(a.path());
        git_in(
            a.path(),
            &["remote", "add", "origin", &remote.path().to_string_lossy()],
        );
        let original = "---\nx: 10\ny: 20\nz: 5\ncolor: green\n---\n# Keeper\n\nold todo\n";
        write_pin(a.path(), "ideas", "keeper.md", original);
        Sync::new(a.path()).push("pinz: original");

        let b = Temp::new("dirty-b");
        clone_repo(remote.path(), b.path());

        // A restacks the pin and pushes; B has an uncommitted body edit.
        write_pin(
            a.path(),
            "ideas",
            "keeper.md",
            &original.replace("z: 5", "z: 6"),
        );
        Sync::new(a.path()).push("pinz: restacked");
        write_pin(
            b.path(),
            "ideas",
            "keeper.md",
            &original.replace("old todo", "go with Keeper Commander"),
        );

        // Prove the dirty tree is what blocks a plain pull, so this test cannot
        // quietly pass for some other reason.
        let sync_b = Sync::new(b.path());
        sync_b.fetch();
        let refused = sync_b.git(&["merge", "--ff-only", "@{u}"]).unwrap();
        assert!(
            !refused.ok,
            "a dirty tree must be what blocks the plain pull"
        );

        sync_b.commit("pinz: update pins");
        let out = sync_b.pull();
        assert!(matches!(out, SyncOutcome::Done(_)), "got {out:?}");

        let merged = fs::read_to_string(b.path().join("ideas/keeper.md")).unwrap();
        assert!(
            merged.contains("go with Keeper Commander"),
            "the local edit survives:\n{merged}"
        );
        assert!(
            merged.contains("z: 6"),
            "and so does the remote restack:\n{merged}"
        );
    }

    /// The 2026-08-07 incident, in miniature: both machines restacked the same
    /// pin (a same-line git conflict on `z:`), machine A also recolored it and
    /// machine B rewrote its body. None of that is a judgement call, so the
    /// pull merges all of it instead of stopping.
    #[test]
    fn a_move_on_one_machine_and_an_edit_on_the_other_sync_cleanly() {
        let remote = Temp::new("automerge-remote");
        assert!(git_in(remote.path(), &["init", "--bare", "--quiet"]));
        let a = Temp::new("automerge-a");
        init_repo(a.path());
        git_in(
            a.path(),
            &["remote", "add", "origin", &remote.path().to_string_lossy()],
        );
        let original = "---\nx: 10\ny: 20\nz: 5\ncolor: green\n---\n# Keeper\n\nold todo\n";
        write_pin(a.path(), "ideas", "keeper.md", original);
        Sync::new(a.path()).push("pinz: original");

        let b = Temp::new("automerge-b");
        clone_repo(remote.path(), b.path());

        let restacked = original
            .replace("z: 5", "z: 6")
            .replace("color: green", "color: blue");
        write_pin(a.path(), "ideas", "keeper.md", &restacked);
        Sync::new(a.path()).push("pinz: restacked and recolored");
        let edited = original
            .replace("z: 5", "z: 7")
            .replace("old todo", "go with Keeper Commander");
        write_pin(b.path(), "ideas", "keeper.md", &edited);
        Sync::new(b.path()).push("pinz: edited"); // commits, push is rejected

        let out = Sync::new(b.path()).pull();
        assert!(
            matches!(out, SyncOutcome::Done(_)),
            "should auto-merge, got {out:?}"
        );

        let merged = fs::read_to_string(b.path().join("ideas/keeper.md")).unwrap();
        assert!(
            merged.contains("go with Keeper Commander"),
            "local body kept:\n{merged}"
        );
        assert!(
            merged.contains("z: 7"),
            "the restack tie goes to local:\n{merged}"
        );
        assert!(
            merged.contains("color: blue"),
            "remote recolor kept:\n{merged}"
        );

        // The rebase finished for real: nothing is in progress and the merged
        // result pushes.
        let status = Command::new("git")
            .arg("-C")
            .arg(b.path())
            .args(["status", "--porcelain=v2", "--branch"])
            .output()
            .unwrap();
        let status = String::from_utf8_lossy(&status.stdout);
        assert!(!status.contains("rebase"), "left mid-rebase:\n{status}");
        let pushed = Sync::new(b.path()).push("pinz: after");
        assert!(matches!(pushed, SyncOutcome::Done(_)), "got {pushed:?}");
    }

    /// A delete against an edit is a judgement call, so the guardrail holds:
    /// stop, leave the repo exactly as it was.
    #[test]
    fn a_delete_against_an_edit_still_stops() {
        let remote = Temp::new("delconflict-remote");
        assert!(git_in(remote.path(), &["init", "--bare", "--quiet"]));
        let a = Temp::new("delconflict-a");
        init_repo(a.path());
        git_in(
            a.path(),
            &["remote", "add", "origin", &remote.path().to_string_lossy()],
        );
        write_pin(
            a.path(),
            "ideas",
            "doomed.md",
            "---\nx: 0\ny: 0\nz: 1\ncolor: yellow\n---\n# t\n\nbody\n",
        );
        Sync::new(a.path()).push("pinz: original");

        let b = Temp::new("delconflict-b");
        clone_repo(remote.path(), b.path());

        fs::remove_file(a.path().join("ideas/doomed.md")).unwrap();
        Sync::new(a.path()).push("pinz: deleted");
        write_pin(
            b.path(),
            "ideas",
            "doomed.md",
            "---\nx: 0\ny: 0\nz: 1\ncolor: yellow\n---\n# t\n\nedited\n",
        );
        Sync::new(b.path()).push("pinz: edited");

        let out = Sync::new(b.path()).pull();
        assert!(out.is_stopped(), "delete vs edit must stop, got {out:?}");
        let kept = fs::read_to_string(b.path().join("ideas/doomed.md")).unwrap();
        assert!(kept.contains("edited"), "local work must survive the stop");
    }

    #[test]
    fn the_same_pin_edited_on_both_machines_stops_and_leaves_the_repo_alone() {
        let remote = Temp::new("remote2");
        assert!(git_in(remote.path(), &["init", "--bare", "--quiet"]));

        let a = Temp::new("conflict-a");
        init_repo(a.path());
        git_in(
            a.path(),
            &["remote", "add", "origin", &remote.path().to_string_lossy()],
        );
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

    #[test]
    fn a_repo_with_no_remote_has_nowhere_to_say_it_pushed_to() {
        let t = Temp::new("nourl");
        init_repo(t.path());
        assert_eq!(Sync::new(t.path()).remote_url(), None);
    }

    #[test]
    fn the_push_destination_is_whatever_origin_points_at() {
        let t = Temp::new("url");
        init_repo(t.path());
        assert!(git_in(
            t.path(),
            &["remote", "add", "origin", "git@github.com:someone/pinz-board.git"]
        ));
        assert_eq!(
            Sync::new(t.path()).remote_url().as_deref(),
            Some("git@github.com:someone/pinz-board.git")
        );
    }
}
