# `pinz version` reports the build against the latest release

Date: 2026-08-31. Status: approved in conversation, this file records it.

## The gap this closes

`pinz version` printed one number: the crate version of the running build.
That answers "what am I on" and nothing else, and on 2026-08-27 it started
answering it misleadingly. `v0.4.0` was the newest tag on GitHub, the
workspace version had been bumped to `0.4.1` in a feature commit, and the
header and `pinz version` both said `0.4.1` - a version nobody could
download. Nothing in the tool could tell you that.

With two machines the same question comes up from the other direction: one
of them is behind and there is no way to see it without opening a browser.

## What it prints

`pinz version` prints the running build, the newest release, and the
relationship between them:

```
pinz    0.4.1
latest  0.4.0  (Gaurgle/pinz)
ahead: this build is not released yet
```

The repository is named the way the sync report already names a remote, owner
and repo through `short_remote`, so the two blocks say a URL the same way.

Four standings, one line each:

| Standing | Line |
|---|---|
| equal | `up to date` |
| behind | `a newer release is out: <releases url>` |
| ahead | `ahead: this build is not released yet` |
| unknown | (no third line; the latest line says why) |

Unknown prints `latest  unknown  (could not reach Gaurgle/pinz)` and exits 0.
Not knowing what GitHub has is not a failure of the question that was asked:
the running build is still reported, which is the part that always works.

## Where the number comes from

`git ls-remote --tags --refs <repository>` against the URL in the crate
manifest, read at compile time from `CARGO_PKG_REPOSITORY` so it is never
written down twice. Every tag that parses as `vX.Y.Z` is a candidate and the
highest wins; anything else on the line is ignored.

Not the GitHub API. `ls-remote` needs no dependency (git is already a hard
requirement of a tool that stores pins in a git repo), no token, and has no
60-request hourly limit to fall off. It also keeps working if the repo ever
moves off GitHub.

Two flags git needs to be told, because the default for both is wrong here:

- `-c http.lowSpeedLimit=1000 -c http.lowSpeedTime=5` - an unreachable host
  otherwise hangs the command far longer than anyone will wait for a version
  string. Five seconds of no progress is an answer: unknown.
- `GIT_TERMINAL_PROMPT=0` - a repo that 404s must not stop and ask for
  credentials. There is no terminal prompt worth showing for this question.

## Where the code lives

`pinz-core::release` holds the domain half: a `Version` that parses and
orders, `latest_in` (the pure parser over `ls-remote` output, tested with no
network), `latest_release` (the one function that shells out), and
`Standing`. `pinz-tui` holds the phrasing. The split is the usual one: core
knows which version is newer, the renderer knows how to say it.

## What does not change

- **The TUI header still shows only the running build.** It is drawn on
  launch, and launch stays offline and instant. A network call for a number
  nobody asked for is not worth a delayed first paint.
- The sync report is untouched.
- `pinz --version` and `pinz -V` behave exactly like `pinz version`, as
  before.
- No new dependency.
