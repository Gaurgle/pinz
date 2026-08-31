//! What pinz has released, so a build can say where it stands.
//!
//! The running version comes from the crate manifest at compile time and is
//! always knowable. The newest released version lives on the source repo as a
//! tag, and asking for it means the network, which means it is sometimes not
//! knowable at all. Everything here is built around that asymmetry: the answer
//! is an `Option`, and not getting one is an ordinary outcome rather than an
//! error.
//!
//! Tags rather than the GitHub API, and `git ls-remote` rather than an HTTP
//! client. git is already a hard requirement of a tool that keeps its pins in a
//! git repo, so this costs no dependency, no token, and none of the API's
//! hourly request budget. It also survives the repo moving off GitHub.

use std::cmp::Ordering;
use std::fmt;
use std::process::Command;

/// A released version, `major.minor.patch`.
///
/// Field order is the comparison order, which is what `derive(Ord)` gives us
/// and the only reason the fields are declared this way round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

/// Where a running build stands against the newest release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// Running exactly what was last released.
    Current,
    /// A newer release exists.
    Behind,
    /// Running something never released - a build from a working tree, or a
    /// version bumped in the repo and not yet tagged.
    Ahead,
}

impl Version {
    /// Parse `v1.2.3` or `1.2.3`. Anything else is `None`.
    ///
    /// Deliberately strict about the three-number shape: a pre-release tag
    /// (`v1.2.3-rc.1`) is not a release, and reading one as `1.2.3` would tell
    /// someone a newer version is out when nothing has shipped. Refusing to
    /// parse it drops it out of the running instead.
    pub fn parse(text: &str) -> Option<Version> {
        let digits = text.strip_prefix('v').unwrap_or(text);
        let mut parts = digits.split('.');
        let mut next = || parts.next().filter(|p| !p.is_empty())?.parse::<u32>().ok();
        let version = Version {
            major: next()?,
            minor: next()?,
            patch: next()?,
        };
        // A fourth component means this is some other numbering scheme.
        parts.next().is_none().then_some(version)
    }

    /// Where `self`, the running build, stands against `latest`.
    pub fn standing(&self, latest: &Version) -> Standing {
        match self.cmp(latest) {
            Ordering::Equal => Standing::Current,
            Ordering::Less => Standing::Behind,
            Ordering::Greater => Standing::Ahead,
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The highest version among the tags in `git ls-remote` output.
///
/// Split out from the command that produces it so the part with the judgement
/// in it - which of these is newest - is testable without a network or a repo.
/// Lines that are not a version tag are ignored rather than fatal: a repo is
/// free to carry tags that have nothing to do with releases.
pub fn latest_in(ls_remote: &str) -> Option<Version> {
    ls_remote
        .lines()
        .filter_map(|line| line.split_whitespace().last())
        .filter_map(|reference| reference.strip_prefix("refs/tags/"))
        .filter_map(Version::parse)
        .max()
}

/// The newest release of `repository`, or `None` if the answer did not arrive.
///
/// `None` covers every way this can fail to produce a number - no network, no
/// git, a repo that is gone, a repo with no version tags - because the caller
/// does the same thing in all of them: say it does not know.
///
/// Two settings git needs, both because its defaults suit a person waiting at a
/// prompt rather than a version string:
///
/// - the low-speed cutoff, or an unreachable host hangs the command for far
///   longer than anyone will wait to be told their version. Five seconds
///   without progress is an answer.
/// - `GIT_TERMINAL_PROMPT=0`, or a repo that 404s stops to ask for
///   credentials. Nothing here is worth a password prompt.
pub fn latest_release(repository: &str) -> Option<Version> {
    let out = Command::new("git")
        .args(["-c", "http.lowSpeedLimit=1000"])
        .args(["-c", "http.lowSpeedTime=5"])
        .args(["ls-remote", "--tags", "--refs", repository])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .ok()?;
    out.status.success().then(|| latest_in(&String::from_utf8_lossy(&out.stdout)))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(major: u32, minor: u32, patch: u32) -> Version {
        Version { major, minor, patch }
    }

    #[test]
    fn a_tag_parses_with_or_without_its_v() {
        assert_eq!(Version::parse("v0.4.1"), Some(v(0, 4, 1)));
        assert_eq!(Version::parse("0.4.1"), Some(v(0, 4, 1)));
        assert_eq!(Version::parse("12.0.30"), Some(v(12, 0, 30)));
    }

    #[test]
    fn anything_that_is_not_three_numbers_is_not_a_version() {
        for text in ["v0.4", "v.0.2.0", "0.4.1.2", "v0.4.x", "nightly", "", "v"] {
            assert_eq!(Version::parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn a_pre_release_tag_does_not_count_as_a_release() {
        // Reading this as 0.5.0 would announce a release that never shipped.
        assert_eq!(Version::parse("v0.5.0-rc.1"), None);
    }

    #[test]
    fn versions_order_by_number_and_not_by_text() {
        // The text order would put 0.10.0 before 0.9.0.
        assert!(v(0, 10, 0) > v(0, 9, 0));
        assert!(v(1, 0, 0) > v(0, 99, 99));
        assert!(v(0, 4, 1) > v(0, 4, 0));
    }

    #[test]
    fn the_newest_tag_wins_whatever_order_they_arrive_in() {
        let output = "\
0d1a2b3\trefs/tags/v0.2.0
4c5d6e7\trefs/tags/v0.4.0
89abcde\trefs/tags/v0.3.0";
        assert_eq!(latest_in(output), Some(v(0, 4, 0)));
    }

    #[test]
    fn tags_that_are_not_versions_are_ignored_rather_than_fatal() {
        let output = "\
0d1a2b3\trefs/tags/v0.4.0
4c5d6e7\trefs/tags/design-freeze
89abcde\trefs/tags/v0.5.0-rc.1";
        assert_eq!(latest_in(output), Some(v(0, 4, 0)));
    }

    #[test]
    fn a_repo_with_no_release_tags_has_no_latest() {
        assert_eq!(latest_in(""), None);
        assert_eq!(latest_in("0d1a2b3\trefs/tags/design-freeze"), None);
    }

    #[test]
    fn standing_reads_from_the_running_build_towards_the_release() {
        assert_eq!(v(0, 4, 0).standing(&v(0, 4, 0)), Standing::Current);
        assert_eq!(v(0, 4, 0).standing(&v(0, 5, 0)), Standing::Behind);
        // The state this repo was in on 2026-08-27: bumped, never tagged.
        assert_eq!(v(0, 4, 1).standing(&v(0, 4, 0)), Standing::Ahead);
    }

    #[test]
    fn a_version_prints_without_its_v() {
        assert_eq!(v(0, 4, 1).to_string(), "0.4.1");
    }

    #[test]
    fn an_unreachable_remote_is_unknown_rather_than_an_error() {
        // A path that is not a repo: git fails, and failure is not a version.
        assert_eq!(latest_release("/nonexistent/pinz-not-a-repo"), None);
    }
}
