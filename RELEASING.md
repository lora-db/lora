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
- Publishing to additional package managers (winget, Homebrew, apt, etc.).
- musl / 32-bit / ARM Windows targets for `lora-server` or `lora-node`.
- Bench regression gating (e.g. fail the release if a benchmark regresses
  beyond a threshold vs. the previous tag). Requires a low-noise runner.
- Auto-committing `CHANGELOG.md` back to `main`. The release ships it as an
  asset; syncing the repo copy is a separate PR today.

---

# Releasing the client packages (`lora-node`, `lora-wasm`, `lora-python`, `lora-ruby`)

The client packages use the **same semver tag** as the server. One
`git push origin vX.Y.Z` triggers two independent workflows:

- `release.yml` — builds `lora-server` binaries and creates a draft GitHub
  Release (see above).
- `packages-release.yml` — builds the four client packages and publishes
  them to npm, PyPI, and RubyGems.

They run in parallel. Nothing the package workflow does depends on the
server release draft, and vice versa.

## Package matrix

| Package              | Registry | Distribution model                                          |
| -------------------- | -------- | ----------------------------------------------------------- |
| `@loradb/lora-wasm`  | npm      | Single tarball with `dist/` + `pkg-node/` + `pkg-bundler/` + `pkg-web/`. |
| `@loradb/lora-node`  | npm      | Root package + one optional platform subpackage per napi triple. |
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
| `darwin-x64`               | `@loradb/lora-node-darwin-x64`          | `macos-latest` (cross from arm64 host) |
| `darwin-arm64`             | `@loradb/lora-node-darwin-arm64`        | `macos-latest`     |
| `win32-x64-msvc`           | `@loradb/lora-node-win32-x64-msvc`      | `windows-latest`   |

musl Linux, freebsd, arm32, and Windows-arm64 are intentionally not
built. `ts/native.js` will throw a clear "no native binary for this
platform" error on unsupported hosts instead of crashing silently.

To add a triple: extend `napi.triples.additional` in
`crates/lora-node/package.json` AND the `build-node` matrix in
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
`crates/lora-ruby/Rakefile` AND the `build-ruby-platform` matrix in
`.github/workflows/packages-release.yml`. Nothing else needs to change.

## One-time registry setup

### GitHub environments

Every publish flow binds to a GitHub environment, so branch protection
can gate who is allowed to trigger a release and secrets stay scoped
to the job that uses them. See
[`.github/workflows/README.md` → Environments & secrets](.github/workflows/README.md#environments--secrets)
for the canonical table. The bootstrap details for each environment:

- **`npm-publish`** — used by `publish-wasm` and `publish-node` in
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
- **`github-pages`** — used by `deploy` in `loradb-docs.yml`. Created
  automatically by GitHub the first time Pages is enabled; no secret
  is needed (authentication is OIDC via `actions/deploy-pages@v4`).
  Add a required reviewer here if you want a manual approval gate on
  docs deploys.

Create the first four environments under **Settings → Environments**
in this repository. Add at least one required reviewer on each if
you want a manual approval gate before any publish runs.

### npm: publish the `@loradb` scope

1. Create the npm organization `loradb` on
   <https://www.npmjs.com/org/create>. Add your npm user as an owner.
2. First-time publish of each package name:
   - `@loradb/lora-wasm`
   - `@loradb/lora-node`
   - every `@loradb/lora-node-<triple>` subpackage
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

1. Bump every manifest (same checklist as the server release):

   ```bash
   node scripts/sync-versions.mjs X.Y.Z
   cargo check --workspace
   (cd crates/lora-node && npm install --package-lock-only --ignore-scripts)
   (cd crates/lora-wasm && npm install --package-lock-only --ignore-scripts)
   (cd apps/loradb.com  && npm install --package-lock-only --ignore-scripts)
   (cd crates/lora-ruby && bundle install)
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

   Both workflows trigger. Watch them side by side under **Actions**.

4. When `packages-release` is green:
   - <https://www.npmjs.com/package/@loradb/lora-wasm> lists the new version.
   - <https://www.npmjs.com/package/@loradb/lora-node> lists the new version
     and its `optionalDependencies` references every platform subpackage
     at the same version.
   - <https://pypi.org/project/lora-python/> lists the new version with
     one sdist + every platform wheel.
   - <https://rubygems.org/gems/lora-ruby> lists the new version with a
     source gem and one precompiled gem per supported platform
     (`x86_64-linux`, `aarch64-linux`, `x86_64-darwin`, `arm64-darwin`,
     `x64-mingw-ucrt`).

5. When `release.yml` is green: finish the server draft release as
   usual (see the top of this file).

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
  cd crates/lora-node
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
  `crates/lora-node/package.json` → `napi.triples.additional`.
- **Version drift.** The package pipeline starts with
  `verify-versions`, which re-runs `scripts/sync-versions.mjs --check`.
  If any of workspace `Cargo.toml` (including the internal-dep pins in
  `[workspace.dependencies]`), `crates/lora-node/package.json`,
  `crates/lora-wasm/package.json`, `apps/loradb.com/package.json`,
  `crates/lora-python/pyproject.toml`, or
  `crates/lora-ruby/lib/lora_ruby/version.rb` disagrees with the tag,
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
`packages-release.yml` (npm + PyPI) — same tag, three independent
workflows.

## Which crates go public

Published on every release:

| Crate           | Role                                                       |
| --------------- | ---------------------------------------------------------- |
| `lora-ast`      | AST types for the Cypher query language.                   |
| `lora-store`    | In-memory graph store with property indexes.               |
| `lora-snapshot` | Column-oriented snapshot encoding, compression, and encryption. |
| `lora-parser`   | Cypher grammar + parser (pest-based).                      |
| `lora-analyzer` | Semantic analysis over parsed Cypher queries.              |
| `lora-compiler` | Query-plan compiler.                                       |
| `lora-executor` | Query-plan executor.                                       |
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

All five keep `publish = false` in their `Cargo.toml`.

## Publish order

Computed from the workspace dependency DAG and hard-coded in
`scripts/publish-crates.mjs`:

```
lora-ast
  -> lora-store
  -> lora-snapshot
  -> lora-parser
  -> lora-analyzer
  -> lora-compiler
  -> lora-executor
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
   for name in lora-ast lora-store lora-snapshot lora-parser \
               lora-analyzer lora-compiler lora-executor lora-wal \
               lora-database lora-server; do
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

1. Bump every manifest in one shot. `scripts/sync-versions.mjs` now also
   rewrites the pinned internal-dep versions in `[workspace.dependencies]`,
   so a single call covers the crates.io side too:

   ```bash
   node scripts/sync-versions.mjs X.Y.Z
   cargo check --workspace --locked            # refresh Cargo.lock
   (cd crates/lora-node && npm install --package-lock-only --ignore-scripts)
   (cd crates/lora-wasm && npm install --package-lock-only --ignore-scripts)
   (cd apps/loradb.com  && npm install --package-lock-only --ignore-scripts)
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
   - `packages-release.yml` — npm + PyPI.
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

# Releasing the Go binding (`github.com/lora-db/lora/crates/lora-go`)

The Go binding ships via Go's standard module/tag resolution: there is
no registry upload. When a consumer runs
`go get github.com/lora-db/lora/crates/lora-go@vX.Y.Z`, the Go module
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

- `crates/lora-ffi` — `publish = false` Rust crate with
  `crate-type = ["staticlib", "cdylib", "rlib"]`. Exposes a stable C
  ABI over `lora-database` (see `crates/lora-ffi/src/lib.rs` and
  `crates/lora-go/include/lora_ffi.h`). Uses `catch_unwind` at every
  entry point so a Rust panic never unwinds into the caller.
- `crates/lora-go` — a Go module (`go.mod` with module path
  `github.com/lora-db/lora/crates/lora-go`) that cgo-links against
  `liblora_ffi.a`. Value model is the same tagged JSON used by the
  other bindings (`lora-node`, `lora-wasm`, `lora-python`,
  `lora-ruby`).

The `#cgo` directives in `crates/lora-go/lora.go` pin the linker to
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
   mode). Polls `GOPROXY=https://proxy.golang.org go list -m
   github.com/lora-db/lora/crates/lora-go@<tag>` every 30 seconds for
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
  GOPROXY=direct go get github.com/lora-db/lora/crates/lora-go@vX.Y.Z
  ```

  which causes `proxy.golang.org` to index the tag on the next
  subsequent `GOPROXY=https://proxy.golang.org` request. Never re-tag
  just because the proxy is slow.

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
  `go build` / `go test`. `make test` from `crates/lora-go/` does this
  automatically.
- **`ld: library not found for -llora_ffi`** when building the Go
  module. The cgo linker is looking under
  `crates/lora-go/../../target/release/liblora_ffi.a`. Either the
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
