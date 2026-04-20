# GitHub Actions — workflows index

This directory holds every CI/CD workflow for the Lora workspace. It's kept
flat because GitHub Actions requires workflows to live directly under
`.github/workflows/`. Shared logic is extracted into composite actions under
[`.github/actions/`](../actions), not into more YAML in this folder.

## Naming convention

| Prefix / name        | Scope                                             |
| -------------------- | ------------------------------------------------- |
| `lora-<crate>.yml`   | CI for a single Rust crate / language binding    |
| `workspace-*.yml`    | Cross-workspace checks (fmt, clippy, test)        |
| `loradb-*.yml`       | The `loradb.com` Docusaurus site (under `apps/`)  |
| `release.yml`        | Tag-driven release pipeline for `lora-server`     |
| `benchmarks.yml`     | On-demand criterion benchmarks for a release tag  |
| `commitlint.yml`     | Conventional-Commits gate on PRs + pushes         |

## Workflows at a glance

| Workflow                  | Trigger                                                        | Purpose                                                                  |
| ------------------------- | -------------------------------------------------------------- | ------------------------------------------------------------------------ |
| `commitlint.yml`          | PR (title + commits), push `main`                              | Enforce Conventional-Commits on every PR and merged commit.              |
| `workspace-quality.yml`   | PR, push `main`, dispatch (path-filtered to `crates/**`)       | Workspace-wide `cargo fmt` / `cargo clippy -D warnings` / `cargo test`.  |
| `lora-server.yml`         | PR, push `main`, dispatch (path-filtered)                      | Per-crate CI for the `lora-server` binary; uploads a CI-build artifact.  |
| `lora-node.yml`           | PR, push `main`, dispatch (path-filtered)                      | napi-rs build + vitest + `npm pack` across Ubuntu and macOS.             |
| `lora-wasm.yml`           | PR, push `main`, dispatch (path-filtered)                      | wasm-pack build + vitest + Playwright smoke test + `npm pack`.           |
| `lora-python.yml`         | PR, push `main`, dispatch (path-filtered)                      | maturin build + pytest across `{ubuntu, macos} × {3.8, 3.11, 3.12}`.     |
| `loradb-docs.yml`         | PR, push `main`, `release.released`, dispatch                  | Docusaurus build; deploys to GitHub Pages on non-PR events.              |
| `release.yml`             | Push semver tag `vX.Y.Z` (optionally `-pre`), dispatch         | Cross-platform `lora-server` builds + changelog + draft GitHub Release.  |
| `benchmarks.yml`          | Dispatch only                                                  | Criterion benchmarks for an existing tag; can attach to the Release.     |

## Shared composite actions

Instead of copy-pasting setup steps, per-crate workflows call these:

- [`.github/actions/setup-rust`](../actions/setup-rust/action.yml) — installs
  the stable Rust toolchain (with optional `targets` / `components`) and
  optionally primes a Swatinem/rust-cache partition via `cache-key`. Every
  call supplies its own cache key so workflows never share build artefacts.
- [`.github/actions/resolve-release-tag`](../actions/resolve-release-tag/action.yml)
  — resolves a release tag from either an explicit `tag` input or
  `GITHUB_REF_NAME`, validates it as `vX.Y.Z[-pre]`, and exposes `tag` /
  `version` / `prerelease` outputs. Used by `release.yml` and `benchmarks.yml`.

## Conventions

- **Permissions**: every workflow sets a top-level `permissions:` block. Default
  is `contents: read`; release + benchmarks raise to `contents: write`;
  `loradb-docs` adds `pages: write` + `id-token: write`.
- **Concurrency**: PRs cancel in-progress runs of themselves; release-style
  workflows (`release`, `benchmarks`, Pages deploys) never cancel mid-run.
- **Path filters**: per-crate workflows include their own workflow file and the
  shared `setup-rust` action in their `paths:` so changes to either retrigger
  verification.
- **Third-party actions** are pinned to major-version tags (e.g. `@v5`,
  `@v2`, `@v4`). Upgrading a third-party action is an intentional PR, not a
  drive-by bump.

## Related

- [`RELEASE.md`](../../RELEASE.md) — human release checklist.
- [`RELEASING.md`](../../RELEASING.md) — technical release flow (what
  `release.yml` does, how to recover).
- [`cliff.toml`](../../cliff.toml) — changelog template used by `release.yml`.
- [`scripts/sync-versions.mjs`](../../scripts/sync-versions.mjs) — manifest
  version sync, invoked by `verify-versions` in both `release.yml` and
  `benchmarks.yml`.
