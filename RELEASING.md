# Releasing pinz

The order below is not arbitrary. `pinz` (the binary crate) depends on a
*published* `pinz-core`, so the core has to reach crates.io first or the second
publish is rejected. Everything else follows from that.

Nothing here is reversible in the way git is: a crates.io version can be
yanked but never replaced, and a Homebrew user who has already installed will
not re-download a tarball you swapped out. Get the version number right before
you start.

## 1. Before anything

- `main` is green in CI: tests on Linux and macOS, clippy with `-D warnings`,
  and a build on the declared `rust-version`.
- The working tree is clean and you are on `main`.

## 2. Bump the version

Two adjacent lines in the workspace manifest, since both crates inherit them:

```sh
$EDITOR Cargo.toml        # [workspace.package] version, and the pinz-core
                          # version in [workspace.dependencies] - same number
cargo test --workspace    # refreshes Cargo.lock with the new version
git commit -am "chore: release vX.Y.Z"
git push
```

Miss the second and cargo says so straight away, because a path dependency's
version requirement is checked against the crate it points at:

```
error: failed to select a version for the requirement `pinz-core = "^X.Y.Z"`
```

Bump the minor for a feature, the patch for a fix. The version in the tag, the
manifest and the formula are the same number, always.

## 3. Tag it

```sh
git tag -a vX.Y.Z -m "pinz vX.Y.Z"
git push origin vX.Y.Z
```

Pushing the tag starts the release workflow, which:

1. opens a draft release for the tag if one does not exist,
2. builds `pinz` for macOS arm64, macOS x86-64 and Linux x86-64, and attaches
   a `.tar.gz` and a `.sha256` for each,
3. renders `pinz.rb` from those checksums and attaches it too.

If the tag already went out without assets, run the workflow by hand instead:
Actions -> Release -> Run workflow, and give it the tag.

## 4. Write the notes and publish

The draft's notes are a placeholder. Replace them with what actually changed,
in the shape the previous releases use: a heading per change, what it does for
someone using the board, and any gap that is still open. Then publish.

## 5. Publish to crates.io

**Order matters.** The core first:

```sh
cargo publish -p pinz-core
```

Wait for it to appear in the index (usually seconds), then the binary:

```sh
cargo publish -p pinz
```

A dry run of the second one fails until the first is live, and that is expected
rather than a problem to debug:

```
error: no matching package named `pinz-core` found
```

Note that publishing makes the source public regardless of the GitHub
repository's visibility. Do not publish before the repository is public.

## 6. Update the Homebrew tap

The workflow already rendered the formula with the right URLs and checksums.
Download `pinz.rb` from the release and commit it to the tap:

```sh
gh release download vX.Y.Z --pattern pinz.rb --dir /tmp
cp /tmp/pinz.rb ~/repos/homebrew-tap/Formula/pinz.rb
cd ~/repos/homebrew-tap && git commit -am "pinz X.Y.Z" && git push
```

Then check it end to end on a machine that does not have pinz built:

```sh
brew update && brew install Gaurgle/tap/pinz && pinz version
```

`pinz.rb` is generated. Never hand-edit it in the tap: the next release
overwrites it. Fix `packaging/homebrew/render-formula.sh` instead.

## Adding a platform

A new target is three lines in the `build` matrix of
`.github/workflows/release.yml` and one `on_*` block in
`packaging/homebrew/render-formula.sh`. The tarball name is
`pinz-<version>-<target>.tar.gz`, and the renderer looks the checksum up by
that name, so the two files have to agree on the target triple exactly.
