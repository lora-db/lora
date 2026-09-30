# Releasing LoraDB

One `vX.Y.Z` tag releases everything: the `lora-server` binaries, the
client packages (npm, PyPI, RubyGems, the Go module) and the Rust crates
on crates.io. This document starts with the human checklist for cutting
a release, then describes each release workflow in turn:

- [Release checklist](#release-checklist) — what a maintainer does, in order.
- [Releasing `lora-server`](#releasing-lora-server) — `release.yml`.
- [Releasing the client packages](#releasing-the-client-packages) —
  `packages-release.yml`.
- [Releasing the Rust crates](#releasing-the-rust-crates-cratesio) —
  `cargo-release.yml`.
- [Releasing the Go binding](#releasing-the-go-binding-githubcomlora-dblora-cratesbindingslora-go) —
  the verify-only Go path in `packages-release.yml`.

## Release checklist

### Pre-release

- [ ] `main` is green: `cargo test --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo fmt --all --check`.
- [ ] Every public commit on `main` since the last tag follows Conventional Commits
      (`corepack yarn exec commitlint --from=<lastTag> --to=HEAD`). There is no
      hand-written `CHANGELOG.md` in the repo: the `changelog` job in
      `release.yml` renders the release notes from these commits with
      `git-cliff` (see [Changelog generation](#changelog-generation)). To
      preview them locally: `git cliff --unreleased`.
- [ ] No `TODO (release)` or `XXX` markers left in code paths touched this cycle (`rg 'TODO ?\(release\)|XXX' crates/`).
- [ ] `README.md` install / usage snippets still work against a fresh clone.
- [ ] Licensing is correct:
  - [ ] Root `LICENSE` is BSL 1.1.
  - [ ] BSL Change Date is three years from the intended public release date.
  - [ ] BSL Change License is Apache License 2.0.
  - [ ] Root package metadata uses `BUSL-1.1`.
  - [ ] `apps/loradb.com/LICENSE` is MIT.
  - [ ] No project-authored docs claim the database core is MIT or Apache before
        the Change Date.
- [ ] Version bumped consistently. Run the sync helper — it updates every manifest in one go —
      then refresh the two committed lockfiles and commit:
  ```bash
  node scripts/sync-versions.mjs X.Y.Z
  cargo check --workspace                         # refreshes Cargo.lock
  corepack yarn install --mode=update-lockfile    # refreshes yarn.lock
  node scripts/sync-versions.mjs X.Y.Z --check    # sanity check
  git commit -am "chore(release): vX.Y.Z"
  ```
  Touches: workspace `Cargo.toml` (`[workspace.package].version` and the
  `=X.Y.Z` internal-dep pins in `[workspace.dependencies]`),
  `crates/bindings/lora-node/package.json`,
  `crates/bindings/lora-wasm/package.json`,
  `packages/lora-query/package.json`, `packages/lora-graph-canvas/package.json`,
  `packages/lora-graphql/package.json` (version **and** its
  `@loradb/lora-node` peer range `^X.Y.Z`), `apps/loradb.com/package.json`,
  `crates/bindings/lora-python/pyproject.toml`, and
  `crates/bindings/lora-ruby/lib/lora_ruby/version.rb`.
  The repo is Yarn 4 with a single root `yarn.lock` (`package-lock.json`
  is gitignored). `yarn.lock` records the `@loradb/lora-graphql` peer
  range, so it changes on every bump, and CI installs with
  `yarn install --immutable`: a stale `yarn.lock` fails every JS job.
  `crates/bindings/lora-ruby/Gemfile.lock` is gitignored; a local
  `bundle install` refreshes your copy, but there is nothing to commit.
- [ ] The commit that bumps versions is `chore(release): vX.Y.Z` (commitlint
      rejects a bare `Release vX.Y.Z`; git-cliff skips `chore(release):`
      commits so the notes don't reference themselves).
- [ ] Every release workflow (`release`, `packages-release`, `cargo-release`,
      and the manual `benchmarks`) starts with a `verify-versions` job that
      re-runs `sync-versions.mjs --check` against the tag; if any manifest is
      out of sync with the tag, that workflow fails before any build runs.

### Secrets and sensitive data audit

Run before every tag that will end up on a public remote:

- [ ] `git ls-files | rg -i '\.(env|pem|key|p12|jks|keystore|db|sqlite|dump|sql)$'` returns no unexpected hits.
- [ ] `for s in 'PRIVATE KEY' 'BEGIN RSA' 'api_key' 'aws_secret'; do git log --all --oneline -S "$s"; done` returns no unexpected commits.
- [ ] No embedded production database dumps, customer data, or proprietary third-party data.
- [ ] No credentials in CI workflows (they should all be `${{ secrets.* }}`).
- [ ] No large generated artifacts tracked (`dist/`, `build/`, `pkg-*`, `*.node`, `.venv/`, `node_modules/`).

### Cutting the release

1. (Optional but recommended on workflow changes) Dry-run both client
   pipelines:
   - **Actions → packages-release → Run workflow**, `tag: vX.Y.Z`, `dry_run: true`.
   - **Actions → cargo-release → Run workflow**, `tag: vX.Y.Z`, `dry_run: true`.
   - Also run `node scripts/publish-crates.mjs --dry-run` locally — it
     exercises `cargo publish --workspace --dry-run` end-to-end.
2. Tag:
   ```bash
   git tag -a vX.Y.Z -m "lora vX.Y.Z"
   git push origin main
   git push origin vX.Y.Z
   ```
   Use `vX.Y.Z-<pre>` (for example `v0.2.0-rc.1`) for a pre-release.
3. Three workflows trigger in parallel:
   - `release` — builds the `lora-server` binaries and creates a draft
     GitHub Release.
   - `packages-release` — builds and publishes `@loradb/lora-wasm`,
     `@loradb/lora-node` (+ platform subpackages), `@loradb/lora-query`,
     `@loradb/lora-graph-canvas`, `@loradb/lora-graphql` (after
     `@loradb/lora-node`), `lora-python`, and `lora-ruby` (+ precompiled
     platform gems) to npm / PyPI / RubyGems, and verifies the Go module.
   - `cargo-release` — publishes every public workspace crate to
     crates.io in dependency order.
4. Review the draft GitHub Release:
   - [ ] Every archive attached (Linux x86_64, Windows x86_64, macOS Intel, macOS ARM).
   - [ ] Matching `.sha256` next to each archive.
   - [ ] `lora-server-vX.Y.Z-SHA256SUMS.txt` and `CHANGELOG.md` present.
5. Confirm the published client packages:
   - [ ] <https://www.npmjs.com/package/@loradb/lora-wasm> shows `X.Y.Z`.
   - [ ] <https://www.npmjs.com/package/@loradb/lora-node> shows `X.Y.Z`
         with matching `optionalDependencies` for every platform
         subpackage.
   - [ ] <https://www.npmjs.com/package/@loradb/lora-query>,
         <https://www.npmjs.com/package/@loradb/lora-graph-canvas>, and
         <https://www.npmjs.com/package/@loradb/lora-graphql> show `X.Y.Z`.
   - [ ] <https://pypi.org/project/lora-python/X.Y.Z/> shows the sdist
         and every platform wheel.
   - [ ] <https://rubygems.org/gems/lora-ruby/versions/X.Y.Z> lists the
         source gem and every precompiled platform gem
         (`x86_64-linux`, `aarch64-linux`, `x86_64-darwin`,
         `arm64-darwin`, `x64-mingw-ucrt`).
   - [ ] <https://crates.io/crates/lora-database/X.Y.Z> exists, plus the
         other eleven public crates (`lora-ast`, `lora-builtins-meta`,
         `lora-store`, `lora-snapshot`, `lora-parser`, `lora-analyzer`,
         `lora-compiler`, `lora-executor`, `lora-io`, `lora-wal`,
         `lora-server`).
   - [ ] The Go binding resolves via the proxy:
         `GOPROXY=https://proxy.golang.org go list -m github.com/lora-db/lora/crates/bindings/lora-go@vX.Y.Z`
         returns the expected version. The `verify-go-module-resolvable`
         CI job also covers this, but re-running locally surfaces any
         network flakiness that only shows up in your environment.
6. Review the git-cliff release notes that pre-fill the server draft, add
   highlights / breaking changes / upgrade steps if needed, and **Publish**
   the draft.
7. (Optional) Benchmark snapshot: **Actions → benchmarks → Run workflow**
   with `tag: vX.Y.Z`. It runs outside the release path and, with
   `attach_to_release: true`, attaches the snapshot to the release (see
   [Benchmark snapshots](#benchmark-snapshots)).

### Post-release

- [ ] Smoke-test one downloaded archive end-to-end (verify checksum, extract, start server, run one query).
- [ ] `npm install @loradb/lora-wasm@X.Y.Z` in a throwaway dir and import once.
- [ ] `npm install @loradb/lora-node@X.Y.Z` and run the `require()` smoke test.
- [ ] `npm install @loradb/lora-graphql@X.Y.Z @loradb/lora-node@X.Y.Z graphql` in a
      throwaway dir and import once (the peer range must resolve to the
      `lora-node` just published).
- [ ] `pip install lora-python==X.Y.Z` in a fresh venv and run `python examples/basic.py`.
- [ ] `gem install lora-ruby -v X.Y.Z` in a throwaway dir and run
      `ruby -r lora_ruby -e 'puts LoraRuby::VERSION'`, then
      `ruby examples/basic.rb` from the checked-out crate.
- [ ] `cargo add lora-database@X.Y.Z` in a throwaway crate and run the README snippet.
- [ ] `cargo install lora-server --version X.Y.Z` and start the binary once.
- [ ] `go get github.com/lora-db/lora/crates/bindings/lora-go@vX.Y.Z` in a
      throwaway module, then `go run ./examples/basic` (or a tiny
      `main.go` that opens a DB, runs `MATCH (n) RETURN count(n)`,
      and prints the result) to confirm the module builds against a
      freshly compiled `lora-ffi`.
- [ ] Close / move the milestone.
- [ ] Open the next iteration's milestone and bump to the next
      pre-release version with `scripts/sync-versions.mjs` if desired (same
      lockfile + commit steps as above).

### Emergency rollback

- A published **server release** can be unpublished on GitHub (it becomes a draft).
- **Published npm / PyPI / RubyGems versions cannot be overwritten.**
  npm's `unpublish` window is 72 hours for packages with no dependents;
  after that, ship a patch release. PyPI never allows re-uploading the
  same version. RubyGems allows `gem yank X.Y.Z` (hides it from new
  resolves while leaving existing `Gemfile.lock` files working) but not
  a re-push of the same version. For any publish mistake, cut
  `vX.Y.(Z+1)` with the fix.
- **Published crates.io versions cannot be overwritten ever.** You can
  `cargo yank` a broken version (it stays resolvable for existing
  `Cargo.lock` files but is hidden from new dependency solves). Yank is
  not a rollback — cut a patch release.
- A pushed tag can be moved, but only before anyone relies on it. After that,
  cut a new patch release (`vX.Y.Z+1`) instead.
- A pushed commit cannot be taken back once anyone has fetched it — prefer a
  revert commit over force-pushing on `main`.
- See the "Recovery from a failed publish" sections below for
  partial-publish recovery (some subpackages out, others not).

---

# Releasing `lora-server`

This part describes how release artifacts for the `lora-server` binary are
produced and published.

## Overview

Releases are driven by **annotated semver tags** of the form `vX.Y.Z`. Pushing
such a tag triggers the [`release`](.github/workflows/release.yml) workflow,
which runs in four stages:

1. **`verify-versions`** — re-runs `node scripts/sync-versions.mjs <version>
   --check` against the tag. Every other job needs it, so a manifest that
   disagrees with the tag stops the release before anything is built.
2. **`build`** — one matrix job per target (Linux x86_64, Windows x86_64,
   macOS Intel, macOS Apple Silicon):
   1. Checks out the tagged commit.
   2. Builds `lora-server` in release mode for the target.
   3. Packages the binary + `README.md` + `RELEASING.md` into a per-target
      archive (`.tar.gz` on Unix, `.zip` on Windows).
   4. Writes a SHA-256 checksum next to the archive.
   5. Uploads archive + checksum as a workflow artifact.
3. **`changelog`** — runs in parallel with `build`:
   1. Checks out the tagged commit with full git history + tags.
   2. Runs [`git-cliff`](https://git-cliff.org) (config: `cliff.toml`) over
      the Conventional-Commits history to render two files:
      - `release-notes.md` — the changes introduced by this tag only, used
        as the body of the GitHub Release draft;
      - `CHANGELOG.md` — the full, cumulative changelog across every tag,
        attached to the release as a downloadable asset.
   3. Uploads both as a single workflow artifact.
4. **`publish`** — runs once, after `verify-versions`, every `build` leg,
   and `changelog` have succeeded:
   1. Downloads every workflow artifact produced upstream into `dist/`.
   2. Concatenates the per-archive `.sha256` files into an aggregated
      `lora-server-vX.Y.Z-SHA256SUMS.txt`.
   3. Composes the release body from `release-notes.md` and appends an asset
      table.
   4. Creates (or updates) a **draft** GitHub Release for the tag and
      attaches every binary archive, every per-archive `.sha256`, the
      aggregated `SHA256SUMS.txt`, and the full `CHANGELOG.md` as release
      assets — in a single atomic step.

Benchmarks are deliberately **not** part of `release.yml`, so benchmark
noise or runtime never blocks a release. They run on demand in the separate
`benchmarks.yml` workflow (see [Benchmark snapshots](#benchmark-snapshots)).

The release is left as a **draft** on purpose. A maintainer reviews the
assets and the pre-filled release notes, and publishes manually.

The workflow artifacts are kept for 30 days as a secondary copy — the main
downloadable distribution path is the GitHub Release assets.

## Release triggers

| Trigger                                  | Behavior                               |
| ---------------------------------------- | -------------------------------------- |
| `git push origin vX.Y.Z`                 | Full release build, draft created.     |
| `git push origin vX.Y.Z-<pre>`           | Same, marked as pre-release on GitHub. |
| Actions → **release** → *Run workflow*   | Rebuild an existing tag (recovery).    |

The tag glob accepted by the workflow is `v[0-9]+.[0-9]+.[0-9]+` with an
optional `-<suffix>` for pre-releases.

## Built targets

| Platform              | Runner          | Target triple                 | Archive   |
| --------------------- | --------------- | ----------------------------- | --------- |
| Linux (x86_64)        | `ubuntu-latest` | `x86_64-unknown-linux-gnu`    | `.tar.gz` |
| Windows (x86_64)      | `windows-latest`| `x86_64-pc-windows-msvc`      | `.zip`    |
| macOS (Intel)         | `macos-latest`  | `x86_64-apple-darwin`         | `.tar.gz` |
| macOS (Apple Silicon) | `macos-latest`  | `aarch64-apple-darwin`        | `.tar.gz` |

The Intel macOS binary is **cross-compiled** from the Apple Silicon runner
(`macos-latest`) rather than scheduled on `macos-13`. The legacy Intel
runner pool is queue-constrained and being phased out; cross-compiling
avoids getting stuck waiting for a runner to pick up the Intel job.

Adding more targets (ARM Linux, musl, etc.) is a matter of adding a row to
`matrix.include` in `.github/workflows/release.yml` — the rest of the workflow
is target-agnostic.

## Artifact naming

```
lora-server-vX.Y.Z-<target-triple>.<ext>
lora-server-vX.Y.Z-<target-triple>.<ext>.sha256
lora-server-vX.Y.Z-SHA256SUMS.txt
CHANGELOG.md
```

For `v0.1.0` this produces the following assets on the GitHub Release:

```
lora-server-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
lora-server-v0.1.0-x86_64-unknown-linux-gnu.tar.gz.sha256
lora-server-v0.1.0-x86_64-pc-windows-msvc.zip
lora-server-v0.1.0-x86_64-pc-windows-msvc.zip.sha256
lora-server-v0.1.0-x86_64-apple-darwin.tar.gz
lora-server-v0.1.0-x86_64-apple-darwin.tar.gz.sha256
lora-server-v0.1.0-aarch64-apple-darwin.tar.gz
lora-server-v0.1.0-aarch64-apple-darwin.tar.gz.sha256
lora-server-v0.1.0-SHA256SUMS.txt
CHANGELOG.md
```

`SHA256SUMS.txt` is the concatenation of all per-archive `.sha256` files and
can be used to verify every archive in one go.

If `benchmarks.yml` is later run for the tag with `attach_to_release: true`,
it adds `lora-server-vX.Y.Z-benchmarks.tar.gz`,
`lora-server-vX.Y.Z-benchmarks.summary.json`, and a `.sha256` for each.
These are not part of `SHA256SUMS.txt`.

Each archive contains a single top-level directory named after the archive
(without the extension). Inside:

- `lora-server` (or `lora-server.exe` on Windows)
- `README.md`
- `RELEASING.md`

## Verifying a download

Each archive ships with a matching `.sha256` file. You can verify a single
archive, or download the aggregated `SHA256SUMS.txt` and verify everything
you downloaded in one command.

```bash
# Linux — single archive
sha256sum -c lora-server-v0.1.0-x86_64-unknown-linux-gnu.tar.gz.sha256

# Linux — verify every archive present in the current directory
sha256sum --ignore-missing -c lora-server-v0.1.0-SHA256SUMS.txt

# macOS — single archive
shasum -a 256 -c lora-server-v0.1.0-x86_64-apple-darwin.tar.gz.sha256

# macOS — verify every archive present in the current directory
shasum -a 256 --ignore-missing -c lora-server-v0.1.0-SHA256SUMS.txt
```

```powershell
# Windows (PowerShell): compare to the first token of the .sha256 file
$expected = (Get-Content .\lora-server-v0.1.0-x86_64-pc-windows-msvc.zip.sha256).Split()[0]
$actual   = (Get-FileHash .\lora-server-v0.1.0-x86_64-pc-windows-msvc.zip -Algorithm SHA256).Hash.ToLower()
if ($expected -eq $actual) { "ok" } else { "MISMATCH" }
```

## Starting the downloaded binary

```bash
# Extract (Linux / macOS)
tar -xzf lora-server-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
cd lora-server-v0.1.0-x86_64-unknown-linux-gnu

# Defaults: 127.0.0.1:4747
./lora-server

# Custom host/port via flags
./lora-server --host 0.0.0.0 --port 8080

# Or via environment
LORA_SERVER_HOST=0.0.0.0 LORA_SERVER_PORT=8080 ./lora-server
```

```powershell
# Windows
Expand-Archive .\lora-server-v0.1.0-x86_64-pc-windows-msvc.zip .
cd .\lora-server-v0.1.0-x86_64-pc-windows-msvc
.\lora-server.exe --host 0.0.0.0 --port 8080
```

See the [Running `lora-server`](README.md#running-lora-server) section in the
README for the full option list.

## Cutting a release

The full sequence (version bump, audits, tag, post-release checks) is the
[Release checklist](#release-checklist) at the top of this file. The
server-specific part:

1. **Bump the version** with `scripts/sync-versions.mjs`, refresh the
   lockfiles, and commit as `chore(release): vX.Y.Z` — see the checklist's
   version-bump step. Do **not** edit only `Cargo.toml`: `verify-versions`
   checks every manifest against the tag and fails the release on any
   drift.

2. **Tag the commit.** Use an annotated tag matching `vX.Y.Z`
   (or `vX.Y.Z-<pre>` for pre-releases such as `v0.2.0-rc.1`):

   ```bash
   git tag -a vX.Y.Z -m "lora vX.Y.Z"
   ```

3. **Push the commit and the tag.**

   ```bash
   git push origin main
   git push origin vX.Y.Z
   ```

   Pushing the tag is what starts the release workflow (and
   `packages-release` + `cargo-release` alongside it). Watch it in the
   **Actions** tab of the repository.

4. **Publish the draft release.** Once the `publish` job finishes, open the
   draft release under **Releases** on GitHub:
   - Confirm all expected assets are attached: one archive + one `.sha256`
     per row in the matrix above, plus the aggregated `SHA256SUMS.txt` and
     `CHANGELOG.md`.
   - Review the release notes. The body is pre-filled with the git-cliff
     notes for this tag plus an asset table; add highlights, breaking
     changes, or upgrade steps as needed.
   - Click **Publish release**.

## Re-running a release (recovery)

If the workflow fails midway — a runner flake, a flaky cache, a transient
upload error — you do **not** need to re-tag. The workflow has a
`workflow_dispatch` trigger that accepts the existing tag:

1. Go to **Actions → release → Run workflow**.
2. Enter the tag (for example `v0.1.0`).
3. Run. The matrix rebuilds every target and the `publish` job replaces the
   asset set on the existing draft release in one atomic step.

The `publish` job requires every matrix leg to succeed before it runs. This
is intentional: a release never ends up with a partial asset set attached.
The workflow artifacts from a partial run are still available under the
failed workflow run for 30 days if you need to inspect them directly.

## Pre-releases

Tags that contain a hyphen (for example `v0.2.0-rc.1`, `v0.2.0-beta.2`) are
automatically marked as **pre-release** on GitHub. Everything else follows the
same flow as a regular release.

## Troubleshooting

- **Tag didn't trigger the workflow.** The tag must match the glob
  `v[0-9]+.[0-9]+.[0-9]+` (optionally followed by `-<suffix>`), must have been
  pushed (`git push origin vX.Y.Z`), and workflow runs must be enabled for the
  repository.
- **`fail_on_unmatched_files` error during publish.** One of the asset
  globs in the `publish` job matched zero files in `dist/`. Usually this
  means a `build` leg produced no archive (inspect its logs) or a matrix
  `archive-ext` was changed without updating the glob list in `publish`.
- **One platform failed, others succeeded.** The `publish` job is skipped
  because `needs: build` requires every leg to pass. Re-run the workflow via
  **workflow_dispatch** with the same tag — previously successful legs are
  fast because `Swatinem/rust-cache` hits, and once every leg passes, the
  draft release is populated in one atomic `publish` step.
- **`verify-versions` failed (version drift).** A manifest disagrees with
  the tag, so nothing was built. The artifact filenames use the **tag**, so
  never work around it: run `node scripts/sync-versions.mjs X.Y.Z`, commit
  the fix, and tag the new commit (use a new patch version if the tag was
  already public).
- **macOS runners are slow.** First run per target on a new cache may take up
  to ~15 minutes. Subsequent runs hit `Swatinem/rust-cache` and are much
  faster.

## Changelog generation

The `changelog` job in the release workflow uses
[`git-cliff`](https://git-cliff.org) (configured in [`cliff.toml`](cliff.toml))
to parse the Conventional-Commits history that commitlint + husky already
enforce on this repository.

- **`release-notes.md`** — rendered with `git cliff --latest --strip header`
  and used verbatim as the GitHub Release body. The body is prefixed to an
  auto-generated asset table in the `publish` job.
- **`CHANGELOG.md`** — rendered with `git cliff` (full history) and attached
  as a release asset so anyone can download the complete cumulative log for
  a given version.

The `cliff.toml` `tag_pattern` matches the same `vX.Y.Z` / `vX.Y.Z-<pre>`
glob used by the release trigger, so every tag that produces binaries also
produces a changelog section.

Commit types are grouped under human-readable headings: **Features**,
**Bug fixes**, **Performance**, **Refactoring**, **Documentation**,
**Tests**, **Build system**, **CI/CD**, **Maintenance**, **Reverts**, and a
dedicated **Breaking changes** group sourced from `BREAKING CHANGE:`
footers regardless of type. `chore(release):` commits are skipped to avoid
the changelog referencing itself.

If the tag contains no conventional-commits changes (e.g. a re-tag without
new work), the release body falls back to an explicit
`_(no conventional-commits changes detected for this tag)_` line so the
release is still readable.

## Benchmark snapshots

Benchmarks are **not** run by `release.yml`. The separate
[`benchmarks`](.github/workflows/benchmarks.yml) workflow runs on manual
dispatch only (**Actions → benchmarks → Run workflow**) with two inputs:
`tag` (an existing release tag) and `attach_to_release` (default `true`).

It re-runs `verify-versions` for the tag, then runs the criterion suite
defined in `crates/lora-database/benches/` (`cargo bench --locked -p
lora-database --benches`) on `ubuntu-latest` and packages a snapshot:

```
lora-server-vX.Y.Z-benchmarks.tar.gz
  ├── benchmarks.log          # raw `cargo bench` stdout (bencher format)
  ├── benchmark-summary.json  # machine-readable summary (scripts/summarize-benchmarks.mjs)
  ├── criterion/              # criterion HTML reports + estimates.json
  └── README.txt              # describes the snapshot
lora-server-vX.Y.Z-benchmarks.summary.json   # the same summary, standalone
```

Each file gets a `.sha256`. Both are uploaded as a workflow artifact and,
with `attach_to_release: true`, added to the tag's GitHub Release.

Treat the numbers as a **relative trend signal**, not authoritative
microbenchmark results. GitHub-hosted runners have noisy neighbors, variable
CPU topology, and thermal throttling. The value is that every version run
through the workflow has the same benchmark harness executed against the
same code, so large shifts show up even if absolute numbers drift. The
snapshot is not compared against a previous one; per-PR regression gating
is the job of `perf-smoke.yml` (and `memory-bench.yml` for memory).

For more rigorous comparisons, run the suite on dedicated hardware:

```bash
cargo bench -p lora-database
open target/criterion/report/index.html
```

If the benchmark job flakes (transient runner failure, toolchain hiccup),
dispatch it again with the same tag — it is idempotent against an existing
tag, and it never affects the release itself.

## What is intentionally **not** done yet

These would be reasonable next steps but are out of scope for now:

- Code signing (Authenticode on Windows, codesign/notarization on macOS).
- Reproducible-build flags (`--remap-path-prefix`, `SOURCE_DATE_EPOCH`).
- Publishing to additional package managers (winget, Homebrew, apt, etc.).
- musl / 32-bit / ARM Windows targets for `lora-server`, and 32-bit / ARM
  Windows targets for `lora-node` (which does ship musl builds).
- Bench regression gating (e.g. fail the release if a benchmark regresses
  beyond a threshold vs. the previous tag). Requires a low-noise runner.
- A `CHANGELOG.md` in the repository. The release renders it with git-cliff
  and ships it only as a release asset; nothing commits it back to `main`.

---

# Releasing the client packages

The client packages — the `lora-node`, `lora-wasm`, `lora-python`, and
`lora-ruby` bindings plus the JS/TS packages under `packages/`
(`lora-query`, `lora-graph-canvas`, `lora-graphql`) — use the **same
semver tag** as the server. One `git push origin vX.Y.Z` triggers
`packages-release.yml` alongside the other release workflows:

- `release.yml` — builds `lora-server` binaries and creates a draft GitHub
  Release (see above).
- `packages-release.yml` — builds the seven client packages and publishes
  them to npm, PyPI, and RubyGems (plus the verify-only Go path, see
  [Releasing the Go binding](#releasing-the-go-binding-githubcomlora-dblora-cratesbindingslora-go)).

They run in parallel. Nothing the package workflow does depends on the
server release draft, and vice versa.

## Package matrix

| Package              | Registry | Distribution model                                          |
| -------------------- | -------- | ----------------------------------------------------------- |
| `@loradb/lora-wasm`  | npm      | Single tarball with `dist/` + `pkg-node/` + `pkg-bundler/` + `pkg-web/`. |
| `@loradb/lora-node`  | npm      | Root package + one optional platform subpackage per napi triple. |
| `@loradb/lora-query` | npm      | React + CodeMirror Cypher editor: `dist/` plus the `wasm/` build of the embedded `lora-query-wasm` parser crate. |
| `@loradb/lora-graph-canvas` | npm | Pure-TS React graph canvas (`dist/`). No native or WASM code. |
| `@loradb/lora-graphql` | npm    | Pure-TS tarball. Released in lockstep with `@loradb/lora-node`: its peer range is `^<version>` (kept by `scripts/sync-versions.mjs`), and `publish-graphql` waits for `publish-node`. |
| `lora-python`        | PyPI     | abi3-py38 wheels (manylinux x64 + arm64, macOS x64 + arm64, Windows x64) plus an sdist. |
| `lora-ruby`          | RubyGems | Source gem + precompiled platform gems (linux x64 + arm64, macOS x64 + arm64, Windows ucrt). |

The `@loradb` npm scope is the **organization scope** on npmjs.com. The
GitHub organization is a separate thing (`lora-db`) — the two names do
**not** have to match.

### `@loradb/lora-node` platform subpackages

napi-rs' recommended "optional platform-package" layout is used so that
installing `@loradb/lora-node@X.Y.Z` does **not** pull in every native
`.node` binary. npm resolves `optionalDependencies` per-platform, so a
Linux x64 install pulls only `@loradb/lora-node-linux-x64-gnu`.

Shipped triples (bumped in lockstep with the root version):

| Triple                     | npm subpackage                          | Runner             |
| -------------------------- | --------------------------------------- | ------------------ |
| `linux-x64-gnu`            | `@loradb/lora-node-linux-x64-gnu`       | `ubuntu-latest`    |
| `linux-arm64-gnu`          | `@loradb/lora-node-linux-arm64-gnu`     | `ubuntu-latest` + zig cross |
| `linux-x64-musl`           | `@loradb/lora-node-linux-x64-musl`      | `ubuntu-latest` + zig cross |
| `linux-arm64-musl`         | `@loradb/lora-node-linux-arm64-musl`    | `ubuntu-latest` + zig cross |
| `darwin-x64`               | `@loradb/lora-node-darwin-x64`          | `macos-latest` (cross from arm64 host) |
| `darwin-arm64`             | `@loradb/lora-node-darwin-arm64`        | `macos-latest`     |
| `win32-x64-msvc`           | `@loradb/lora-node-win32-x64-msvc`      | `windows-latest`   |

freebsd, arm32, and Windows-arm64 are intentionally not built. The musl
builds cover Alpine-based images; the loader picks them by checking
`ldd --version`. `ts/native.js` will throw a clear "no native binary for this
platform" error on unsupported hosts instead of crashing silently.

To add a triple: extend `napi.triples.additional` in
`crates/bindings/lora-node/package.json` AND the `build-node` matrix in
`.github/workflows/packages-release.yml`. Nothing else needs to change.

### `lora-python` wheel layout

`pyo3` is configured with `abi3-py38`, so one compiled wheel covers every
Python 3.8+ interpreter. That means:

- The release workflow **does not** put Python versions into its matrix.
  Putting `[3.8, 3.9, 3.10, …]` there would produce identical abi3 wheel
  filenames, and PyPI would reject the duplicates.
- The `lora-python` CI workflow (`lora-python.yml`) **does** cross Python
  versions — that's where interpreter compatibility is verified. Release
  and CI intentionally serve different purposes.

The sdist is built once and uploaded alongside the wheels.

### `lora-ruby` platform gems

rb-sys' cross-gem action builds one **fat** gem per platform that
contains every supported Ruby ABI (3.1, 3.2, 3.3). This mirrors how
popular Rust-backed gems (`wasmtime-rb`, `bootsnap`, `rustler`) ship.

Shipped platforms (bumped in lockstep with the workspace version):

| Platform          | Gem filename                              | Runner          |
| ----------------- | ----------------------------------------- | --------------- |
| `x86_64-linux`    | `lora-ruby-<v>-x86_64-linux.gem`          | `ubuntu-latest` |
| `aarch64-linux`   | `lora-ruby-<v>-aarch64-linux.gem`         | `ubuntu-latest` + rb-sys cross image |
| `x86_64-darwin`   | `lora-ruby-<v>-x86_64-darwin.gem`         | `ubuntu-latest` + rb-sys cross image |
| `arm64-darwin`    | `lora-ruby-<v>-arm64-darwin.gem`          | `ubuntu-latest` + rb-sys cross image |
| `x64-mingw-ucrt`  | `lora-ruby-<v>-x64-mingw-ucrt.gem`        | `ubuntu-latest` + rb-sys cross image |

Musl Linux, freebsd, and Windows-arm64 are not built. On a platform
without a precompiled gem, `gem install lora-ruby` falls back to the
source gem and rebuilds locally — this requires a Rust toolchain
(1.87+). The source gem is always published.

To add a platform: extend `ext.cross_platform` in
`crates/bindings/lora-ruby/Rakefile` AND the `build-ruby-platform` matrix in
`.github/workflows/packages-release.yml`. Nothing else needs to change.

## One-time registry setup

### GitHub environments

Every publish flow binds to a GitHub environment, so branch protection
can gate who is allowed to trigger a release and secrets stay scoped
to the job that uses them. See
[`.github/workflows/README.md` → Environments & secrets](.github/workflows/README.md#environments--secrets)
for the canonical table. The bootstrap details for each environment:

- **`npm-publish`** — used by `publish-wasm`, `publish-node`,
  `publish-query`, `publish-graph-canvas`, and `publish-graphql` in
  `packages-release.yml`.
  - Secret: `NPM_TOKEN` (automation token, `publish` permission). Only
    required until trusted publishing is live for every npm package
    below. After that, delete the secret.
- **`pypi-publish`** — used by `publish-python` in `packages-release.yml`.
  - Secret: `PYPI_API_TOKEN` — only required until a PyPI trusted
    publisher is configured. After that, delete the secret.
- **`rubygems-publish`** — used by `publish-ruby` in `packages-release.yml`.
  - Secret: `RUBYGEMS_API_KEY` — only required until a RubyGems
    trusted publisher is configured. After that, delete the secret.
- **`crates-io-publish`** — used by `publish` in `cargo-release.yml`.
  - Secret: `CARGO_REGISTRY_TOKEN` (required; see "Trusted publishing
    (OIDC) — current status" further down for why OIDC is not yet an
    option on crates.io).
- **`production`** — used by `deploy` in `loradb-docs.yml` (loradb.com)
  and `deploy` in `play-loradb.yml` (play.loradb.com). Both deploy to
  Cloudflare Pages via `cloudflare/wrangler-action@v3`.
  - Secrets (repository- or environment-level): `CLOUDFLARE_API_TOKEN`
    (scoped to "Pages — Edit") and `CLOUDFLARE_ACCOUNT_ID`. Add a required reviewer here if you want a
    manual approval gate on site deploys.

Create all five environments under **Settings → Environments**
in this repository. Add at least one required reviewer on each if
you want a manual approval gate before any publish runs.

### npm: publish the `@loradb` scope

1. Create the npm organization `loradb` on
   <https://www.npmjs.com/org/create>. Add your npm user as an owner.
2. First-time publish of each package name:
   - `@loradb/lora-wasm`
   - `@loradb/lora-node`
   - every `@loradb/lora-node-<triple>` subpackage
   - `@loradb/lora-query`
   - `@loradb/lora-graph-canvas`
   - `@loradb/lora-graphql`
3. npm refuses to register a new package name via OIDC trusted publishing
   alone — it needs an initial publish to exist. Two options:

   **Option A — bootstrap each name with a token (recommended).**
   1. Create a scoped automation token at
      <https://www.npmjs.com/settings/YOUR_USER/tokens> with the
      `@loradb` scope and `publish` permission.
   2. Add it as environment secret `NPM_TOKEN` on the `npm-publish`
      environment.
   3. Cut a single release (e.g. `v0.1.0-rc.1`). The tokenised publish
      registers every package name on npm.
   4. After that first publish succeeds, go to each package's **Settings
      → Publishing access** on npmjs.com and configure **Trusted
      publishing** with:
      - Repository: `lora-db/lora`
      - Workflow filename: `packages-release.yml`
      - Environment name: `npm-publish`
   5. Remove the `NPM_TOKEN` secret.

   **Option B — set up trusted publishing before the first release.**
   If npm accepts trusted-publisher-only registration for your scope by
   the time you read this, skip Option A: configure trusted publishing
   for every expected package name first, then cut `v0.1.0-rc.1`
   directly with `NPM_TOKEN` empty.

4. `npm publish --provenance` requires npm ≥ 9.5. The workflow
   explicitly installs the latest npm (`npm install -g npm@latest`)
   before every publish step.

### PyPI: `lora-python`

1. Register the package name `lora-python` on PyPI. Either:
   - Visit <https://pypi.org/manage/account/publishing/> and add a
     **pending trusted publisher** for the not-yet-existing project.
     Fill in: owner `lora-db`, repository `lora`, workflow file
     `packages-release.yml`, environment `pypi-publish`.
   - Or, bootstrap with a token: create a scoped API token at
     <https://pypi.org/manage/account/token/> (scoped to project
     `lora-python` once it exists, or to your user for the first
     publish), store it as `PYPI_API_TOKEN` on the `pypi-publish`
     environment, cut one release, then configure a trusted publisher
     and drop the secret.

2. Once the trusted publisher is active, leave `PYPI_API_TOKEN` unset.
   `pypa/gh-action-pypi-publish` ignores `password` when OIDC is
   available.

3. TestPyPI (optional staging target):
   - Add a second trusted publisher at <https://test.pypi.org/> pointing
     at a separate environment (`testpypi-publish`) if you want a proper
     staging flow. This workflow does not currently wire up TestPyPI; to
     enable it, copy the `publish-python` job, change
     `environment:` to `testpypi-publish`, and pass
     `repository-url: https://test.pypi.org/legacy/` to the action.
   - Cheap alternative for a single rehearsal: run the workflow via
     `workflow_dispatch` with `dry_run: true` — everything builds, the
     `.whl` / `.tar.gz` / `.tgz` files are uploaded as workflow
     artifacts, but nothing is pushed to PyPI.

### RubyGems: `lora-ruby`

1. Register the gem name `lora-ruby` on RubyGems. Either:
   - Visit <https://rubygems.org/profile/oidc/api_key_roles/new> and
     configure a **trusted publisher** for a not-yet-existing gem.
     Fill in: repository `lora-db/lora`, workflow file
     `packages-release.yml`, environment `rubygems-publish`. RubyGems
     released OIDC trusted publishing in 2024 and accepts
     pending-trusted-publisher registrations (same model as PyPI).
   - Or, bootstrap with an API key: create a scoped API key at
     <https://rubygems.org/profile/api_keys> with `push_rubygem` scope
     (scoped to `lora-ruby` once it exists, or global for the first
     publish), store it as `RUBYGEMS_API_KEY` on the `rubygems-publish`
     environment, cut one release, then configure a trusted publisher
     and drop the secret.

2. Once trusted publishing is active, leave `RUBYGEMS_API_KEY` unset.
   `rubygems/configure-rubygems-credentials` will negotiate OIDC via
   the `id-token: write` permission the `publish-ruby` job holds. With
   the secret set, the action prefers it (fallback path — useful for
   the first few releases before trusted publishing is live).

3. There is no TestGems equivalent of TestPyPI. For a rehearsal:
   - Run the workflow via `workflow_dispatch` with `dry_run: true`.
     Every gem — source + each platform — is uploaded as a workflow
     artifact. Nothing is pushed to RubyGems.
   - Alternatively, push a pre-release tag (`vX.Y.Z-rc.1`). RubyGems
     will accept it as a normal version; consumers have to opt in with
     `gem install lora-ruby --pre`.

4. RubyGems enforces 2FA on newly created gems by default. When
   bootstrapping with an API key, generate a **scoped** key with 2FA
   enabled on the account; otherwise the first `gem push` is rejected
   with `You must enable MFA`.

## Release flow

1. Bump every manifest — the version-bump step of the
   [Release checklist](#release-checklist), shared by every release
   workflow:

   ```bash
   node scripts/sync-versions.mjs X.Y.Z
   cargo check --workspace
   corepack yarn install --mode=update-lockfile
   node scripts/sync-versions.mjs X.Y.Z --check
   git commit -am "chore(release): vX.Y.Z"
   ```

2. **Dry-run the package pipeline once** (recommended for any release
   where the workflow itself has changed):

   - **Actions → packages-release → Run workflow**
   - Tag: the tag you're about to push (e.g. `v0.2.0`)
   - dry_run: `true`
   - Verify every `build-*` job succeeded, inspect the uploaded
     artifacts if you want, and confirm no `publish-*` jobs ran.

3. Push commit and tag:

   ```bash
   git push origin main
   git push origin vX.Y.Z
   ```

   All three release workflows trigger. Watch them side by side under
   **Actions**.

4. When `packages-release` is green:
   - <https://www.npmjs.com/package/@loradb/lora-wasm> lists the new version.
   - <https://www.npmjs.com/package/@loradb/lora-node> lists the new version
     and its `optionalDependencies` references every platform subpackage
     at the same version.
   - <https://www.npmjs.com/package/@loradb/lora-query>,
     <https://www.npmjs.com/package/@loradb/lora-graph-canvas>, and
     <https://www.npmjs.com/package/@loradb/lora-graphql> list the new
     version. `publish-graphql` runs only after `publish-node`, so the
     `@loradb/lora-node` release its peer range names is already live.
   - <https://pypi.org/project/lora-python/> lists the new version with
     one sdist + every platform wheel.
   - <https://rubygems.org/gems/lora-ruby> lists the new version with a
     source gem and one precompiled gem per supported platform
     (`x86_64-linux`, `aarch64-linux`, `x86_64-darwin`, `arm64-darwin`,
     `x64-mingw-ucrt`).

5. When `release.yml` is green: finish the server draft release as
   usual (see [Cutting a release](#cutting-a-release)).

## Recovery from a failed publish

Publishes are not always atomic — a matrix leg can fail after some
subpackages are already public. Recovery rules:

- **Never re-tag.** npm and PyPI both reject re-uploading an existing
  version; you cannot overwrite a published file. The
  `skip-existing: true` setting on the PyPI action means "don't fail if
  this wheel is already up," which is what you want on a retry. npm does
  not have an equivalent flag, so the publish-node job will fail fast on
  subpackages that are already live.

- **Workflow dispatch the exact same tag.** All jobs are idempotent
  against the tag: they re-check out the tagged commit, rebuild, and
  publish whatever is still missing. Previously succeeded legs make the
  cache hot so recovery is fast.

- **If a platform subpackage failed to publish while the root did
  succeed:** run `packages-release` again. npm will reject the root
  because the version exists. Work around by publishing only the
  missing subpackage manually (one-off, with your user token):

  ```bash
  cd crates/bindings/lora-node
  npm run build:native -- --target <target>
  npx napi create-npm-dir -t .
  mkdir -p artifacts && cp lora-node.<triple>.node artifacts/
  npx napi artifacts --dir artifacts --dist npm
  (cd npm/<triple> && npm publish --access public)
  ```

  Then cut a patch release `vX.Y.(Z+1)` with only a `chore(release):`
  commit so installs eventually converge on a version with complete
  platform coverage.

- **If the sdist published but a wheel did not:** re-run the workflow
  against the tag. The PyPI action's `skip-existing: true` handles the
  already-uploaded sdist, and only the missing wheel is pushed.

- **If a platform gem failed to publish while the source gem is
  live:** re-run the workflow against the tag. The `publish-ruby`
  job's push loop treats "has already been pushed" as success, so
  previously-published gems are skipped and only the missing ones are
  pushed. If the failure is structural (bad triple, stale Rakefile
  target list), fix + cut `vX.Y.(Z+1)`.

- **If a platform gem is subtly wrong** (wrong Ruby ABI compiled in,
  missing native library, etc.) — `gem yank lora-ruby -v X.Y.Z
  --platform <platform>` removes just that platform gem from the index
  without touching the source / other platforms. Then cut a patch
  release with the fix. Yanks never free the version number for
  reuse; a patch bump is the only forward path.

- **Workflow artifacts are kept for 30 days.** If a build-matrix leg
  succeeded but the publish job crashed before its step ran, you can
  download the `.node` / `.whl` / `.tgz` directly from the failed run
  and decide manually.

## Troubleshooting

- **`npm publish` error `E402 You must sign up for private packages` or
  `EEXIST`.** Means the scope is not a public org, or the version already
  exists. Create the `@loradb` org on npmjs.com; never re-use a version.
- **`npm error code E401 Unauthorized`.** Either the automation token is
  missing/revoked, or trusted publishing is misconfigured (wrong repo,
  wrong workflow filename, wrong environment name). Double-check the
  trusted publisher entry on npmjs.com matches **exactly**
  `lora-db/lora` + `packages-release.yml` + `npm-publish`.
- **`pypi: the workflow is not authorized`.** The trusted publisher on
  PyPI expects the same three strings: owner `lora-db`, repository
  `lora`, environment `pypi-publish`. Mismatched environment names are
  the most common cause.
- **`napi artifacts` reports "No dist dir found".** The `.node` file in
  the `artifacts/` dir didn't match any triple in
  `napi.triples.additional`. Either a new triple was added to the
  workflow matrix without updating `package.json`, or a binary was
  uploaded with the wrong filename. Check
  `crates/bindings/lora-node/package.json` → `napi.triples.additional`.
- **Version drift.** The package pipeline starts with
  `verify-versions`, which re-runs `scripts/sync-versions.mjs --check`.
  If any of workspace `Cargo.toml` (including the internal-dep pins in
  `[workspace.dependencies]`), `crates/bindings/lora-node/package.json`,
  `crates/bindings/lora-wasm/package.json`, the three
  `packages/*/package.json` (plus the `@loradb/lora-graphql` peer range on
  `@loradb/lora-node`), `apps/loradb.com/package.json`,
  `crates/bindings/lora-python/pyproject.toml`, or
  `crates/bindings/lora-ruby/lib/lora_ruby/version.rb` disagrees with the tag,
  the build never starts.

- **`Error fetching gem: You are rate limited.`** RubyGems throttles
  pushes per-account (around 100/hour). Unlikely to hit with the
  handful of gems in one release, but a flaky re-run of
  `workflow_dispatch` can accumulate attempts. Wait ten minutes and
  re-run.

- **`gem push` rejects OIDC (`Trusted publishers are not configured`).**
  The `rubygems-publish` environment name on the action must match
  **exactly** what's configured on rubygems.org. Also: the environment
  only has `id-token: write` on the publish job, not the builds — that
  is intentional. If you split the builds to require OIDC, the trusted
  publisher evaluation will still run in the publish job where the
  token is minted.

---

# Releasing the Rust crates (`crates.io`)

A semver tag push also triggers `cargo-release.yml`, which publishes the
workspace's library + server crates to [crates.io](https://crates.io).
It runs in parallel with `release.yml` (server binaries) and
`packages-release.yml` (npm + PyPI + RubyGems) — same tag, three independent
workflows.

## Which crates go public

Published on every release:

| Crate           | Role                                                       |
| --------------- | ---------------------------------------------------------- |
| `lora-ast`      | AST types for the Cypher query language.                   |
| `lora-builtins-meta` | Static metadata table for the namespaced builtin functions (arity, aliases, aggregates), shared by the analyzer, executor, and editor WASM. |
| `lora-store`    | In-memory graph store with property indexes.               |
| `lora-snapshot` | Column-oriented snapshot encoding, compression, and encryption. |
| `lora-parser`   | Cypher grammar + parser (pest-based).                      |
| `lora-analyzer` | Semantic analysis over parsed Cypher queries.              |
| `lora-compiler` | Query-plan compiler.                                       |
| `lora-executor` | Query-plan executor.                                       |
| `lora-io`       | Row-level bulk import/export codecs (JSONL, JSON, CSV).    |
| `lora-wal`      | Write-ahead log and replay engine.                         |
| `lora-database` | Embeddable in-memory graph database — the main public API. |
| `lora-server`   | HTTP server binary (`lora-server`) wrapping `lora-database`. |

Intentionally **not** published to crates.io:

| Crate         | Why                                                         |
| ------------- | ----------------------------------------------------------- |
| `lora-node`   | napi-rs cdylib shipped as an npm package; its Rust surface is a JS-facing FFI layer. Rust users depend on `lora-database` directly. |
| `lora-wasm`   | wasm-bindgen cdylib shipped as an npm package; same reasoning. |
| `lora-python` | pyo3 cdylib shipped as a PyPI wheel; same reasoning.        |
| `lora-ffi`    | C ABI helper crate used by out-of-tree language bindings.   |
| `lora-ruby`   | Ruby native extension shipped as a RubyGem.                 |
| `lora-binding-buffer` | Shared binary buffer helpers for the native bindings; internal only. |
| `lora-query-wasm` | The parser bridge embedded in `packages/lora-query`, shipped inside the `@loradb/lora-query` npm package. |

All seven keep `publish = false` in their `Cargo.toml`.

## Publish order

Computed from the workspace dependency DAG and hard-coded in
`scripts/publish-crates.mjs`:

```
lora-ast
  -> lora-builtins-meta
  -> lora-store
  -> lora-snapshot
  -> lora-parser
  -> lora-analyzer
  -> lora-compiler
  -> lora-executor
  -> lora-io
  -> lora-wal
  -> lora-database
  -> lora-server
```

If you add a new crate with `publish = true`, add it to the DAG in the
script **and** to `[workspace.dependencies]` (`path` + pinned version) if
anything else in the workspace depends on it.

## One-time registry setup

1. **Create a crates.io account** at <https://crates.io/me>.
   - Verify an email address — crates.io refuses to publish from
     unverified accounts.
2. **Reserve the crate names.** crates.io is first-come-first-served. All
   publishable workspace crate names must be available and owned by you
   before the first release. Otherwise one publish step will fail because
   crates.io reports that the crate name is already taken. Quick check:

   ```bash
   for name in lora-ast lora-builtins-meta lora-store lora-snapshot \
               lora-parser lora-analyzer lora-compiler lora-executor \
               lora-io lora-wal lora-database lora-server; do
     status=$(curl -sSo /dev/null -w "%{http_code}" \
       "https://crates.io/api/v1/crates/${name}")
     echo "${status} ${name}"
   done
   ```

   A `404` means the name is free. A `200` means someone else holds it —
   see "If a name is already taken" below.

3. **Create a scoped API token** at
   <https://crates.io/settings/tokens/new>:
   - Scope: `publish-new` + `publish-update` (both).
   - Lifetime: short (90 days) is fine — rotate when it expires.
   - Crates: leave unrestricted for the first release (all publishable
     workspace crate names need to be registered); scope the next token
     to those names once
     they exist.

4. **Add the GitHub secret.** Settings → Environments → new environment
   named `crates-io-publish`. Add secret `CARGO_REGISTRY_TOKEN` with the
   token value. The workflow hard-fails if that secret is missing.

   Optional but recommended: add a required reviewer on the
   `crates-io-publish` environment so a crates.io publish requires
   manual approval even after a tag is pushed.

### If a name is already taken

crates.io does not have namespacing, so a stolen name means you need a
new one. Two options:

- **Rename the whole crate** via its `[package] name = "..."` field,
  then update every `use` and every `[dependencies]` entry. This is
  invasive — touches every downstream crate's Cargo.toml and every
  Rust source that imports it (the module name also changes unless you
  use `[lib] name = "..."` to pin it).
- **Prefix with the project name** — e.g. `loradb-ast`, `loradb-parser`.
  Same refactor cost.

Rust crates don't support scoped/namespaced names the way npm does, so
you cannot simply move to `@loradb/lora-ast` on crates.io. If you go
the rename route, do it in a single commit before the first publish —
once a crate is on crates.io under a name, moving it is painful.

### Trusted publishing (OIDC) — current status

As of this writing crates.io does **not** support OIDC-based trusted
publishing from GitHub Actions. The published path remains a scoped API
token stored in `CARGO_REGISTRY_TOKEN`. If/when crates.io adds trusted
publishing (tracked upstream), the only change needed here is:
- Add `permissions: id-token: write` to the `publish` job.
- Delete the `CARGO_REGISTRY_TOKEN` secret.
- Configure the trusted publisher on crates.io bound to environment
  `crates-io-publish` + workflow `cargo-release.yml`.

The workflow itself doesn't have to change.

## Release flow

1. Bump every manifest in one shot — the version-bump step of the
   [Release checklist](#release-checklist). `scripts/sync-versions.mjs`
   also rewrites the pinned internal-dep versions in
   `[workspace.dependencies]`, so a single call covers the crates.io side
   too:

   ```bash
   node scripts/sync-versions.mjs X.Y.Z
   cargo check --workspace                         # refresh Cargo.lock (not --locked: the lockfile must change)
   corepack yarn install --mode=update-lockfile    # refresh yarn.lock
   node scripts/sync-versions.mjs X.Y.Z --check
   git commit -am "chore(release): vX.Y.Z"
   ```

2. **Dry-run locally before you tag** (optional but cheap):

   ```bash
   node scripts/publish-crates.mjs --dry-run
   ```

   This runs `cargo publish --workspace --dry-run --locked`, which
   packages every publishable crate and compiles each one against its
   packaged siblings via a temp registry. A clean run here means the
   same step will pass in CI.

   If you still have uncommitted changes you're iterating on, pass
   `--allow-dirty` — CI will **not** accept `--allow-dirty`, but local
   rehearsals should.

3. **Dry-run the CI pipeline** for a real-world rehearsal:

   - Actions → `cargo-release` → Run workflow.
   - Tag: the tag you're about to push (e.g. `v0.2.0`).
   - dry_run: `true`.
   - Verify `verify-versions` and `dry-run` pass. The `publish` job is
     skipped in dry-run mode.

4. Push the commit and tag:

   ```bash
   git push origin main
   git push origin vX.Y.Z
   ```

   Three workflows trigger:
   - `release.yml` — server binaries → draft GitHub Release.
   - `packages-release.yml` — npm + PyPI + RubyGems (+ the Go module check).
   - `cargo-release.yml` — crates.io.

5. When `cargo-release` is green, confirm
   <https://crates.io/crates/lora-database> and
   <https://crates.io/crates/lora-server> show the new version, plus the
   other publishable workspace crates.

## Recovery from a failed publish

crates.io publishes are **not** transactional. If publish fails halfway
through, some crates are live and some aren't.

- **Never re-tag.** crates.io refuses to re-upload an existing version
  for all time. Mismatched source for the same version cannot be fixed;
  a new version must be cut.

- **Re-run the workflow against the same tag.** Actions →
  `cargo-release` → Run workflow, `tag: vX.Y.Z`, `dry_run: false`.
  `scripts/publish-crates.mjs` is called with `--skip-published`, which
  queries the crates.io sparse index for every crate and skips any that
  are already at version `X.Y.Z`. For crates not yet live it runs the
  publish normally. If cargo itself rejects a duplicate (rare — index
  propagation lag), the script recognises "already uploaded" / "already
  exists on crates.io" in stderr and treats it as success.

- **If a crate's source must change before it can publish** — e.g. a
  manifest bug only `cargo publish` catches — cut `vX.Y.(Z+1)` with the
  fix. You can't amend a version that's already out.

- **If a yanked version exists at `X.Y.Z`**, `--skip-published` still
  skips it because yanked counts as "published". Unyanking, publishing a
  new `X.Y.Z+1`, or pushing a new version are all options; cut the
  patch.

- **Manual single-crate recovery** (only if the workflow itself is
  broken and you can't wait):

  ```bash
  node scripts/sync-versions.mjs X.Y.Z --check
  cargo publish --locked -p <crate>          # with CARGO_REGISTRY_TOKEN in env
  ```

  Do not `--allow-dirty` for a real publish.

## Troubleshooting

- **`error: manifest has no description field`** — every publishable
  crate must set `description`. `scripts/publish-crates.mjs --dry-run`
  catches this before tagging.
- **`error: failed to prepare local package … no matching package named
  <lora-foo>`** during local dry-run for a downstream crate — running
  the per-crate dry-run in isolation without `--workspace` will fail
  because the upstream crate isn't on crates.io yet. Use
  `cargo publish --workspace --dry-run` (what the script does) instead.
- **`CARGO_REGISTRY_TOKEN secret is missing`** — add the secret to the
  `crates-io-publish` GitHub environment (see setup above).
- **`crate X is already taken`** — the name is owned by someone else on
  crates.io. Choose a new name (see "If a name is already taken").
- **Version drift on the pinned internal deps.** `[workspace.dependencies]`
  uses `version = "=X.Y.Z"` for every internal crate.
  `scripts/sync-versions.mjs` rewrites them in lockstep with
  `[workspace.package].version`; both are checked by `--check`. If you
  edit `Cargo.toml` by hand, re-run `node scripts/sync-versions.mjs X.Y.Z`.

---

# Releasing the Go binding (`github.com/lora-db/lora/crates/bindings/lora-go`)

The Go binding ships via Go's standard module/tag resolution: there is
no registry upload. When a consumer runs
`go get github.com/lora-db/lora/crates/bindings/lora-go@vX.Y.Z`, the Go module
proxy (`proxy.golang.org`) walks the repo, finds the tag, and serves
the source at that commit. The release pipeline therefore does not
"push" anywhere; it verifies that the tagged tree builds, that the Go
toolchain is happy with it, and that the module proxy has picked up
the new tag.

The Go binding is **not** listed in
[`scripts/sync-versions.mjs`](scripts/sync-versions.mjs) on purpose:
the module version is derived from the git tag at consume time
(`runtime/debug.ReadBuildInfo`) and there is no `go.mod` version field
to keep in lockstep. One fewer synced manifest.

## Architecture

Two crates in one release:

- `crates/bindings/lora-ffi` — `publish = false` Rust crate with
  `crate-type = ["staticlib", "cdylib", "rlib"]`. Exposes a stable C
  ABI over `lora-database` (see `crates/bindings/lora-ffi/src/lib.rs` and
  `crates/bindings/lora-go/include/lora_ffi.h`). Uses `catch_unwind` at every
  entry point so a Rust panic never unwinds into the caller.
- `crates/bindings/lora-go` — a Go module (`go.mod` with module path
  `github.com/lora-db/lora/crates/bindings/lora-go`) that cgo-links against
  `liblora_ffi.a`. Value model is the same tagged JSON used by the
  other bindings (`lora-node`, `lora-wasm`, `lora-python`,
  `lora-ruby`).

The `#cgo` directives in `crates/bindings/lora-go/lora.go` pin the linker to
`${SRCDIR}/../../target/release/liblora_ffi.a`, so building the FFI is
a prerequisite for `go test` / `go build` on any consumer's machine.

## Release flow

`packages-release.yml` contains three Go-specific jobs that run on
every tag push or dispatch:

1. **`verify-go`** (matrix: ubuntu-latest, macos-latest). Checks out
   the tag, builds `lora-ffi` (release), runs `go mod tidy` and fails
   if the tree is now dirty, runs `gofmt -l` / `go vet ./...` /
   `go test -race ./...`. If this job is green, the tag is a valid
   Go module at that commit.
2. **`build-go-archives`** (matrix: linux-x64, darwin-x64,
   darwin-arm64). Builds `liblora_ffi.a` for each triple, stages it
   alongside `lora_ffi.h` + `LICENSE` + `README.md`, and uploads
   `lora-ffi-<tag>-<triple>.tar.gz` + `.sha256` as a workflow artifact
   (30-day retention). These are **convenience** artifacts — the Go
   toolchain itself resolves source from the tag, not from these
   archives — but they are useful for downstream consumers who vendor
   a prebuilt static lib.
3. **`verify-go-module-resolvable`** (only on push, only in non-dry-run
   mode). First tags the release commit `crates/bindings/lora-go/<tag>`:
   Go resolves a module in a subdirectory from tags that carry its
   path, so `vX.Y.Z` alone is not a version of the Go binding. Then polls `GOPROXY=https://proxy.golang.org go list -m
   github.com/lora-db/lora/crates/bindings/lora-go@<tag>` every 30 seconds for
   up to 5 minutes. Fails if the proxy never returns the tag. This is
   the Go equivalent of checking an npm / PyPI / crates.io package
   page.

All three are in the `summary` job's `needs:` list, so the release
checklist's "single green job" gate still reflects Go's state.

## Recovery from a failed Go "publish"

"Publish" is the git tag plus proxy resolution. There is nothing to
re-upload.

- **Re-run the workflow against the same tag.** Actions →
  `packages-release` → Run workflow → `tag: vX.Y.Z`. `verify-go` and
  `build-go-archives` are idempotent against the tag: they re-check
  out, re-build, and re-upload artifacts. No registry has seen
  anything, so there is no "already exists" state to clean up.

- **If `verify-go-module-resolvable` fails** because the proxy has
  not indexed the tag yet (very rare; usually sub-minute), wait a few
  more minutes and re-run the workflow, or force a direct fetch once
  from any machine:

  ```bash
  GOPROXY=direct go get github.com/lora-db/lora/crates/bindings/lora-go@vX.Y.Z
  ```

  which causes `proxy.golang.org` to index the tag on the next
  subsequent `GOPROXY=https://proxy.golang.org` request. Never re-tag
  just because the proxy is slow.
- If `go get` reports `unknown revision crates/bindings/lora-go/vX.Y.Z`,
  the module tag is missing (releases before v0.17.0 never had one).
  Create it on the release commit:

  ```bash
  git tag -a crates/bindings/lora-go/vX.Y.Z 'vX.Y.Z^{}' -m "lora-go vX.Y.Z"
  git push origin crates/bindings/lora-go/vX.Y.Z
  ```

- **If `verify-go` fails against a freshly pushed tag** (e.g. `gofmt`
  drift was not caught before tagging), the Go module is still
  resolvable at that tag, but consumers who run `go test ./...`
  against it will see the same failure. Cut `vX.Y.(Z+1)` with the
  fix; the bad tag stays out there and becomes "don't use this
  version" — mirror the crates.io yank semantics (yank a tag by
  convention, not by deletion).

## Platform support

Built and tested in CI: Linux x86_64, macOS x86_64, macOS ARM64.
Windows and FreeBSD are **not** supported for v0.1. Adding a Windows
target requires cgo + MinGW or MSVC tooling and a round-trip against
the Windows cdylib's import-library conventions; it lives in the same
bucket as "Windows wheels for `lora-python`" — easy enough to add,
intentionally deferred.

## Troubleshooting

- **`could not import C` at build time.** The Rust FFI hasn't been
  built yet, or was built in debug mode. Run
  `cargo build --release -p lora-ffi` from the workspace root before
  `go build` / `go test`. `make test` from `crates/bindings/lora-go/` does this
  automatically.
- **`ld: library not found for -llora_ffi`** when building the Go
  module. The cgo linker is looking under
  `crates/bindings/lora-go/../../target/release/liblora_ffi.a`. Either the
  FFI was built for a different target directory (e.g. `CARGO_TARGET_DIR`
  override) or the release build step was skipped. Rebuild with the
  default target dir, or adjust the `#cgo LDFLAGS` line to point at
  the override.
- **`unknown directive: go 1.22` during `go mod tidy` in CI.** The
  toolchain version is older than `go.mod`'s `go` directive. CI pins
  to `go-version: "1.22"` via `actions/setup-go`; bump both in
  lockstep.
- **`ld: warning: ... has malformed LC_DYSYMTAB`** on macOS. Benign
  Xcode 15 ld64 + cgo interaction; does not affect correctness.
