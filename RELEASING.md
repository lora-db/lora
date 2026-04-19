# Releasing `lora-server`

This document describes how release artifacts for the `lora-server` binary are
produced and published.

## Overview

Releases are driven by **annotated semver tags** of the form `vX.Y.Z`. Pushing
such a tag triggers the [`release`](.github/workflows/release.yml) workflow,
which runs in four stages:

1. **`build`** — one matrix job per target (Linux x86_64, Windows x86_64,
   macOS Intel, macOS Apple Silicon):
   1. Checks out the tagged commit.
   2. Builds `lora-server` in release mode for the target.
   3. Packages the binary + `README.md` + `RELEASING.md` into a per-target
      archive (`.tar.gz` on Unix, `.zip` on Windows).
   4. Writes a SHA-256 checksum next to the archive.
   5. Uploads archive + checksum as a workflow artifact.
2. **`changelog`** — runs in parallel with `build`:
   1. Checks out the tagged commit with full git history + tags.
   2. Runs [`git-cliff`](https://git-cliff.org) (config: `cliff.toml`) over
      the Conventional-Commits history to render two files:
      - `release-notes.md` — the changes introduced by this tag only, used
        as the body of the GitHub Release draft;
      - `CHANGELOG.md` — the full, cumulative changelog across every tag,
        attached to the release as a downloadable asset.
   3. Uploads both as a single workflow artifact.
3. **`bench`** — runs in parallel with `build`:
   1. Checks out the tagged commit.
   2. Executes the criterion benchmark suite (`cargo bench --locked -p
      lora-database`) in release mode on `ubuntu-latest`.
   3. Captures raw stdout plus the HTML reports under `target/criterion/`,
      adds a short `README.txt` describing the snapshot, tarballs everything
      as `lora-server-vX.Y.Z-benchmarks.tar.gz`, writes a `.sha256`, and
      uploads archive + checksum as a workflow artifact.
4. **`publish`** — runs once, after every `build` leg, `changelog`, and
   `bench` have succeeded:
   1. Downloads every workflow artifact produced upstream into `dist/`.
   2. Concatenates the individual `.sha256` files (binary archives +
      benchmarks) into an aggregated `lora-server-vX.Y.Z-SHA256SUMS.txt`.
   3. Composes the release body from `release-notes.md` and appends an asset
      table.
   4. Creates (or updates) a **draft** GitHub Release for the tag and
      attaches every binary archive, every per-archive `.sha256`, the
      aggregated `SHA256SUMS.txt`, the full `CHANGELOG.md`, and the
      benchmark tarball as release assets — in a single atomic step.

The release is left as a **draft** on purpose. A maintainer reviews the
assets, fills in release notes, and publishes manually.

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
lora-server-vX.Y.Z-benchmarks.tar.gz
lora-server-vX.Y.Z-benchmarks.tar.gz.sha256
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
lora-server-v0.1.0-benchmarks.tar.gz
lora-server-v0.1.0-benchmarks.tar.gz.sha256
CHANGELOG.md
```

`SHA256SUMS.txt` is the concatenation of all per-archive `.sha256` files and
can be used to verify every archive in one go.

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

1. **Bump the version.** Update `version` in the workspace `Cargo.toml` to the
   new `X.Y.Z`. Run `cargo check --workspace` so `Cargo.lock` picks up the
   change, then commit:

   ```bash
   git commit -am "Release vX.Y.Z"
   ```

2. **Tag the commit.** Use an annotated tag matching `vX.Y.Z`
   (or `vX.Y.Z-<pre>` for pre-releases such as `v0.2.0-rc.1`):

   ```bash
   git tag -a vX.Y.Z -m "lora-server vX.Y.Z"
   ```

3. **Push the commit and the tag.**

   ```bash
   git push origin main
   git push origin vX.Y.Z
   ```

   Pushing the tag is what starts the release workflow. Watch it in the
   **Actions** tab of the repository.

4. **Publish the draft release.** Once the `publish` job finishes, open the
   draft release under **Releases** on GitHub:
   - Confirm all expected assets are attached: one archive + one `.sha256`
     per row in the matrix above, plus the aggregated `SHA256SUMS.txt`.
   - Add release notes (highlights, breaking changes, upgrade steps) — the
     workflow pre-fills a short platform/asset overview you can keep or
     replace.
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
- **Version drift.** The artifact filename uses the **tag**, not the
  `Cargo.toml` version. If the two disagree, reconcile `Cargo.toml` first and
  re-tag against a new commit. Keep them in sync.
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

The `bench` job runs the criterion suite defined in
`crates/lora-database/benches/` once per release tag and pins the output to
the release:

```
lora-server-vX.Y.Z-benchmarks.tar.gz
  ├── benchmarks.log        # raw `cargo bench` stdout (bencher format)
  ├── criterion/            # criterion HTML reports + estimates.json
  └── README.txt            # describes the snapshot
```

Treat the numbers as a **relative trend signal**, not authoritative
microbenchmark results. GitHub-hosted runners have noisy neighbors, variable
CPU topology, and thermal throttling. The value is that every released
version has the same benchmark harness executed against the same code, so
regressions and improvements show up even if absolute numbers drift.

For more rigorous comparisons, run the suite on dedicated hardware:

```bash
cargo bench -p lora-database
open target/criterion/report/index.html
```

If the benchmark job flakes (transient runner failure, toolchain hiccup), use
**Actions → release → Run workflow** with the existing tag to rebuild. All
jobs are idempotent against an existing tag — cached Rust builds make
recovery cheap.

## What is intentionally **not** done yet

These would be reasonable next steps but are out of scope for now:

- Code signing (Authenticode on Windows, codesign/notarization on macOS).
- Reproducible-build flags (`--remap-path-prefix`, `SOURCE_DATE_EPOCH`).
- Publishing to `crates.io` or a package manager (winget, Homebrew, apt, etc.).
- ARM Linux / musl / 32-bit targets.
- Bench regression gating (e.g. fail the release if a benchmark regresses
  beyond a threshold vs. the previous tag). Requires a low-noise runner.
- Auto-committing `CHANGELOG.md` back to `main`. The release ships it as an
  asset; syncing the repo copy is a separate PR today.
