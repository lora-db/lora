# GitHub Actions — CI/CD map

This directory holds every CI/CD workflow for the Lora workspace. GitHub
Actions requires workflow YAML to live directly under `.github/workflows/`,
so this folder is intentionally flat. Structure comes from naming
conventions, composite actions under [`.github/actions/`](../actions),
and this index.

## Reading this page

1. [Classification](#classification) — what each workflow is *for*.
2. [Workflow index](#workflow-index) — every workflow at a glance.
3. [Shared composite actions](#shared-composite-actions) — when to reuse.
4. [Conventions](#conventions) — shape every workflow follows.
5. [Environments & secrets](#environments--secrets) — what each release
   path needs configured on GitHub.
6. [Adding a new language binding](#adding-a-new-language-binding) —
   checklist for wiring up the next `lora-<lang>` workflow.

## Classification

Each workflow fits into exactly one bucket. If you are adding a workflow,
pick the bucket first — that answers trigger, permissions, and whether
it needs a tag resolver.

| Bucket                       | Workflows                                                        | Gate semantics                                                                                   |
| ---------------------------- | ---------------------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| **Quality gates**            | `commitlint`, `workspace-quality`                                | Run on every PR + every push to `main`. Must be green to merge.                                  |
| **Per-binding CI**           | `lora-server`, `lora-node`, `lora-wasm`, `lora-python`, `lora-ruby`, `lora-go` | Path-filtered per binding. Run on PR + push + manual dispatch. Must be green to merge.          |
| **Docs / site**              | `loradb-docs`                                                    | Builds `apps/loradb.com`; deploys to GitHub Pages on tag push, `release.released`, and dispatch. |
| **Server binary release**    | `release`                                                        | Tag-driven. Builds `lora-server` archives and creates a **draft** GitHub Release.                |
| **Client package release**   | `packages-release`                                               | Tag-driven. Publishes `@loradb/lora-wasm`, `@loradb/lora-node`, `lora-python`, `lora-ruby`; verifies + archives the Go binding. |
| **Crates.io release**        | `cargo-release`                                                  | Tag-driven. Publishes every public Rust crate to crates.io in dependency order.                  |
| **Benchmark / maintenance**  | `benchmarks`, `perf-smoke`                                       | `benchmarks` runs the full criterion suite on manual dispatch only. `perf-smoke` runs a 4-bench canary on every PR + push to `main` and fails only on ≥3× regressions. |

The three tag-driven release workflows (`release`, `packages-release`,
`cargo-release`) fire off the same `vX.Y.Z[-pre]` tag push and run in
parallel. They do not depend on each other.

## Workflow index

### Quality gates

| Workflow            | Trigger                                                                          | What it does                                                                                                              |
| ------------------- | -------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| `commitlint.yml`    | PR (opened/edited/reopened/synchronize), push to `main`                          | Validates PR title + every commit in the range against Conventional-Commits (`@commitlint/config-conventional`).          |
| `workspace-quality.yml` | PR + push `main` (path-filtered to `crates/**`, Cargo files, `setup-rust` action, own yml), dispatch | Workspace-wide `cargo fmt --all --check`, `cargo clippy --workspace --exclude lora-node -D warnings`, `cargo test --workspace --exclude lora-node`. `lora-node` is excluded because napi-rs codegen needs the package-level build to compile cleanly — end-to-end coverage comes from `lora-node.yml`. |

### Per-binding CI

Each per-binding workflow is path-filtered. The filter covers the
binding's own crate, every shared core crate it depends on, the Cargo
manifests, the toolchain pin, the `setup-rust` composite action, and
the workflow file itself. Bindings that share TS types
(`lora-node`, `lora-wasm`) additionally include `crates/shared-ts/**`.

| Workflow            | Matrix                                                                  | Per-binding-specific steps                                                                                                                                                                 |
| ------------------- | ----------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `lora-server.yml`   | `{ubuntu-latest, windows-latest}`                                       | `cargo check / test / build --release -p lora-server`. Uploads a CI release-profile binary as a 7-day artifact (not a release asset).                                                      |
| `lora-node.yml`     | `{ubuntu-latest, macos-latest}`                                         | `cargo check -p lora-node` + `npm run verify:types` + `npm run build:native` (napi) + `build:ts` + `typecheck` + `vitest` + `npm pack`. Uploads a tarball artifact.                         |
| `lora-wasm.yml`     | `ubuntu-latest`                                                         | `cargo check` (host + `wasm32-unknown-unknown`) + wasm-pack build + `build:ts` + `typecheck` + `vitest` + Playwright browser smoke + `npm pack`. Uploads a tarball artifact.                |
| `lora-python.yml`   | `{ubuntu-latest, macos-latest} × {3.8, 3.11, 3.12}`                     | venv + `maturin develop --release` (no separate `cargo check` — maturin drives cargo internally) + `pytest` + `maturin build --release` + sanity import + example run. Uploads wheels.      |
| `lora-ruby.yml`     | `{ubuntu-latest, macos-latest} × {3.1, 3.2, 3.3}`                       | `cargo check -p lora_ruby` + `bundle exec rake compile` (rb-sys) + `rake test` (minitest) + sanity require + example run + `rake build` (source gem). Uploads the built `.gem` artifact.   |
| `lora-go.yml`       | `{ubuntu-latest, macos-latest}`                                         | `cargo build --release -p lora-ffi` + `cargo check -p lora-ffi` + `gofmt -l` + `go vet` + `go test -race` + `go run ./examples/basic`. Uploads the built `liblora_ffi.{a,so,dylib}` artifact. |

### Docs / site

| Workflow            | Trigger                                                                        | What it does                                                                                                                                                                                                                                                 |
| ------------------- | ------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `loradb-docs.yml`   | Tag push `v*.*.*[-*]`, PR (path-filtered to `apps/loradb.com/**`), `release.released`, dispatch | Builds `apps/loradb.com` (Docusaurus). On PR, uploads the build output as an inspection artifact only. On every other trigger, uploads the Pages artifact and deploys to `github-pages` at https://loradb.com. Branch pushes to `main` are intentionally not deployed. |

### Server binary release

| Workflow        | Trigger                                                                        | What it does                                                                                                                                                                                                                                                                              |
| --------------- | ------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `release.yml`   | Tag push `v*.*.*[-*]`, dispatch (explicit tag input for recovery)              | `verify-versions` (`sync-versions.mjs --check`) → `build` × 4 targets (Linux x86_64, Windows x86_64, macOS Intel, macOS ARM) + `changelog` (git-cliff) in parallel → `publish` creates a **draft** GitHub Release with archives + `.sha256` + aggregated `SHA256SUMS.txt` + `CHANGELOG.md`. |

### Client package release

| Workflow                | Trigger                                                                         | What it does                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| ----------------------- | ------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `packages-release.yml`  | Tag push `v*.*.*[-*]`, dispatch (`tag` + `dry_run` inputs)                      | `verify-versions` → parallel build legs for every ecosystem → per-ecosystem publish jobs guarded by `dry_run != 'true'` (wasm + node → npm, python → PyPI, ruby → RubyGems). The Go binding is a verify-only path: it runs `verify-go` (matrix) + `build-go-archives` (convenience tarballs) + `verify-go-module-resolvable` (polls `proxy.golang.org` on real tag pushes). One final `summary` job is the branch-protection gate — fails if any upstream job failed or (outside dry-run) any publish job didn't succeed. Skipped jobs are treated as neutral. |

### Crates.io release

| Workflow              | Trigger                                                                         | What it does                                                                                                                                                                                                                                                                                                                                              |
| --------------------- | ------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `cargo-release.yml`   | Tag push `v*.*.*[-*]`, dispatch (`tag` + `dry_run` inputs)                      | `verify-versions` → `dry-run` (`cargo publish --workspace --dry-run --locked` via `scripts/publish-crates.mjs`) → `publish` (runs only when `dry_run != 'true'`, uses environment `crates-io-publish`, calls `scripts/publish-crates.mjs --skip-published` so recovery runs converge). Final `summary` job is the branch-protection gate. |

### Benchmark / maintenance

| Workflow             | Trigger                                    | What it does                                                                                                                                                                                                    |
| -------------------- | ------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `benchmarks.yml`     | Dispatch only (`tag` + `attach_to_release`) | `verify-versions` → `bench` runs `cargo bench -p lora-database` for the tagged commit and packages a criterion snapshot. Optional `attach-to-release` step uploads the archive as a GitHub Release asset.       |
| `perf-smoke.yml`     | PR + push `main` (path-filtered to engine crates, bench sources, baseline, script, own yml), dispatch | Runs the 4-bench `perf_smoke_benchmarks` suite and pipes the bencher output into `scripts/check-perf-smoke.mjs`. Fails only when a bench is ≥3× slower than its checked-in baseline. See [`docs/performance/perf-smoke.md`](../../docs/performance/perf-smoke.md). |

## Shared composite actions

Composite actions live under [`.github/actions/`](../actions/) and are
called with `uses: ./.github/actions/<name>`. Each call is short,
documented, and either eliminates repetition that already bit us or
reduces the chance of a Windows/macOS/Linux branching footgun.

| Action                                                            | Used by                                                                                                       | When to use                                                                                                                                        |
| ----------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| [`setup-rust`](../actions/setup-rust/action.yml)                  | every workflow that compiles Rust                                                                             | Installs the stable toolchain (optional `targets` / `components`) and, when `cache-key` is passed, primes a Swatinem/rust-cache partition. The `cache-key` per call keeps workflows from ever sharing `target/` — a stale cross-workflow cache previously masked pyo3 SIGILLs. |
| [`resolve-release-tag`](../actions/resolve-release-tag/action.yml) | `release`, `cargo-release`, `packages-release`, `benchmarks`                                                  | Resolves a release tag from either an explicit `tag` input or `GITHUB_REF_NAME`, validates it as `vX.Y.Z[-pre]`, emits `tag` / `version` / `prerelease` outputs. Keeps tag parsing in one place. |
| [`copy-license`](../actions/copy-license/action.yml)              | `packages-release` (wasm, node, python, ruby, go archive jobs)                                                | Copies the repo-root `LICENSE` into a destination directory in a cross-platform way. Replaces ad-hoc `cp ../../LICENSE LICENSE` / pwsh `Copy-Item` branches. |

Why we intentionally **don't** have `setup-node-package`,
`setup-python-maturin`, `setup-go-binding`, or `setup-ruby-gem`
composites: each would save 4–6 lines in at most two workflows, and
each binding's setup differs enough (matrix shape, registry config,
sccache, extension tasks) that a composite would either be parametric
enough to read worse than the inline YAML or would only cover the
boring prefix. Keep setup legible per-binding until the repeated
surface outgrows that trade-off.

## Conventions

Everything below is load-bearing — copying from an existing workflow is
the safest way to stay consistent.

- **Top comment.** Every workflow except the trivial quality gates opens
  with a short comment explaining what it exists for and when it fires.
  Keep it current — a stale header is worse than none.
- **`permissions:` block.** Always set at the top level. Default is
  `contents: read`. Raise only where needed: `release` / `benchmarks`
  use `contents: write`; `loradb-docs` adds `pages: write` +
  `id-token: write`; the npm/PyPI/RubyGems publish jobs in
  `packages-release` set `id-token: write` at the job level for OIDC
  trusted publishing.
- **`concurrency:` block.** Always set. PR-facing workflows cancel
  in-progress runs of the same ref:
  `cancel-in-progress: ${{ github.event_name == 'pull_request' }}`.
  Tag-driven release workflows key on the tag and never cancel mid-run
  (`cancel-in-progress: false`). `loradb-docs` uses a pair of groups to
  serialise deploys across refs but still cancel stale PR builds.
- **`workflow_dispatch`.** Every workflow exposes manual dispatch.
  Release-style workflows accept a `tag` input so an existing tag can
  be re-run without re-tagging. `packages-release` and `cargo-release`
  also accept `dry_run` (default `true` on dispatch).
- **`timeout-minutes`.** Always set per job. Calibrate to the slowest
  historical run plus headroom, not to "infinity". Typical values: 5
  for version checks, 5–15 for quality/unit jobs, 20–30 for
  package/native builds, 45–60 for release builds and benchmarks.
- **Path filters.** Per-binding CI filters always include:
  - the binding's own crate (`crates/lora-<lang>/**`),
  - every shared core crate it depends on (`crates/lora-ast`,
    `lora-parser`, `lora-analyzer`, `lora-compiler`, `lora-executor`,
    `lora-store`, `lora-database`, plus `crates/shared-ts` for the TS
    bindings and `crates/lora-ffi` for `lora-go`),
  - `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`,
  - `.github/actions/setup-rust/**`,
  - the workflow file itself.
- **Third-party actions** are pinned to a major version tag
  (`@v5`, `@v2`, `@v4`, `@v0.4.0`). Upgrading a third-party action is
  an intentional PR, not a drive-by bump.
- **Action pin exceptions** — pinned at exact minor/patch because a
  known-good tag is needed: `jetli/wasm-pack-action@v0.4.0` (pinned
  wasm-pack binary), `goto-bus-stop/setup-zig@v2` (zig 0.13.0 input),
  `rubygems/configure-rubygems-credentials@v1.0.0` (first stable
  release). If/when upstream ships a new major, update deliberately.
- **Artifact naming.** Release artifacts include the tag:
  `lora-server-<tag>-<target>.<ext>`, `lora-ffi-<tag>-<triple>.tar.gz`,
  `lora-server-<tag>-benchmarks.tar.gz`. CI artifacts don't encode a
  tag but encode the OS/runtime they were built on so PR runs do not
  collide: `lora-node-<os>-tarball`, `lora-python-<os>-py<ver>-wheel`,
  `lora-ruby-<os>-ruby<ver>-gem`, etc.

## Environments & secrets

GitHub environments scope secrets and add an optional manual-approval
gate for anything that publishes. All five environments below must
exist under **Settings → Environments** before the matching release
path will succeed.

| Environment          | Used by                                    | Primary auth                                                                             | Fallback secret                                                       |
| -------------------- | ------------------------------------------ | ---------------------------------------------------------------------------------------- | --------------------------------------------------------------------- |
| `npm-publish`        | `packages-release` → `publish-wasm`, `publish-node` | OIDC trusted publisher configured on npmjs.com per package, bound to this environment.   | `NPM_TOKEN` (scoped automation token with `publish` on `@loradb`).    |
| `pypi-publish`       | `packages-release` → `publish-python`      | OIDC trusted publisher configured on pypi.org for project `lora-python`, bound here.     | `PYPI_API_TOKEN` (project-scoped API token).                          |
| `rubygems-publish`   | `packages-release` → `publish-ruby`        | OIDC trusted publisher configured on rubygems.org for gem `lora-ruby`, bound here.       | `RUBYGEMS_API_KEY` (gem-scoped API key with `push_rubygem`).          |
| `crates-io-publish`  | `cargo-release` → `publish`                | **Required**: `CARGO_REGISTRY_TOKEN`. crates.io has no OIDC trusted publishing yet; if/when it ships, flip to `id-token: write` and delete the secret. | — |
| `github-pages`       | `loradb-docs` → `deploy`                   | OIDC via `actions/deploy-pages@v4` (`id-token: write` on the job).                       | —                                                                     |

Bootstrap details (first publish of each name, trusted-publisher
registration, yank / recovery semantics) live in
[`../../RELEASING.md`](../../RELEASING.md).

## Adding a new language binding

When a new `crates/lora-<lang>` binding joins the workspace, do exactly
the following in a single PR:

1. **Crate**. Add `crates/lora-<lang>` to `[workspace].members` in
   `Cargo.toml`. Set `publish = false` in the crate's own
   `Cargo.toml` (client bindings don't publish to crates.io).
2. **Version propagation**. If the binding has its own versioned
   manifest (e.g. `package.json`, `pyproject.toml`, `version.rb`), add
   it to the `targets` list in
   [`scripts/sync-versions.mjs`](../../scripts/sync-versions.mjs). If
   the binding's version is derived from the git tag at consume time
   (like `lora-go`), skip this step and document that choice in
   `RELEASING.md`.
3. **CI workflow**. Add `.github/workflows/lora-<lang>.yml`. Copy the
   closest existing binding workflow (Node/Wasm if TS-shaped, Python
   if maturin-shaped, Ruby if rb-sys-shaped, Go if cgo-shaped) and
   adjust:
   - Path filter: binding's crate, every shared core crate it depends
     on, `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`,
     `.github/actions/setup-rust/**`, and the new workflow file
     itself.
   - Matrix: follow the binding's ecosystem norms. Two OSes is the
     floor; add interpreter/ABI axes only if that ecosystem's
     consumers actually cross them.
   - Artifact upload: name includes OS + version axes so PR runs
     don't collide.
4. **Release workflow**. Add a build leg + (if applicable) a publish
   job to `.github/workflows/packages-release.yml`. Use the existing
   ecosystem sections as templates — every build job uploads an
   artifact under a namespaced pattern (`lora-<lang>-*`), every
   publish job sets `environment:` + `permissions: id-token: write`
   for OIDC. Add the new jobs to the final `summary` job's `needs:`
   list so the single-job gate still reflects reality.
5. **GitHub environment**. Create a new `<registry>-publish`
   environment (or reuse an existing one if the registry is already
   covered) and register a trusted publisher at the registry bound to
   that environment + `packages-release.yml`. Add a fallback
   `*_TOKEN` secret only if trusted publishing is not available for
   the bootstrap.
6. **Docs**.
   - Add a row to [the classification table](#classification) and
     [the workflow index](#workflow-index) in this file.
   - Add the environment to [Environments & secrets](#environments--secrets).
   - In [`../../RELEASE.md`](../../RELEASE.md): add the binding to the
     pre-release version-bump block, the published-package sanity
     list, and the post-release smoke-test list.
   - In [`../../RELEASING.md`](../../RELEASING.md): add a "Releasing
     the X binding" section following the Go section as a template.
7. **Shared actions**. If the binding introduces a pattern that will
   appear in a second workflow in the near term, extract a composite
   action under `.github/actions/` — otherwise leave it inline.
   `copy-license` is an example of a composite worth extracting once
   a copy pattern is repeated across ≥ 3 jobs.

## Related files

- [`RELEASE.md`](../../RELEASE.md) — human release checklist.
- [`RELEASING.md`](../../RELEASING.md) — technical release flow
  (what each release workflow does, how to recover).
- [`cliff.toml`](../../cliff.toml) — changelog template used by
  `release.yml`.
- [`scripts/sync-versions.mjs`](../../scripts/sync-versions.mjs) —
  single source of truth for propagating a version to every
  manifest; invoked by `verify-versions` in every release workflow.
- [`scripts/publish-crates.mjs`](../../scripts/publish-crates.mjs) —
  crates.io publish driver used by `cargo-release.yml`.
