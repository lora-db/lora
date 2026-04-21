# Release Checklist

Operational checklist for cutting a LoraDB release. The technical workflow
(build matrix, artifact layout, checksums, recovery) lives in
[`RELEASING.md`](RELEASING.md) — this document is the human checklist.

## Pre-release

- [ ] `main` is green: `cargo test --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo fmt --all --check`.
- [ ] `CHANGELOG.md` (or the draft release notes) describe every user-visible change since the previous tag.
- [ ] No `TODO (release)` or `XXX` markers left in code paths touched this cycle (`rg 'TODO ?\(release\)|XXX' crates/`).
- [ ] Every public commit on `main` since the last tag follows Conventional Commits (`npx commitlint --from=<lastTag> --to=HEAD`).
- [ ] `README.md` install / usage snippets still work against a fresh clone.
- [ ] Licensing is correct:
  - [ ] Root `LICENSE` is BSL 1.1.
  - [ ] BSL Change Date is three years from the intended public release date.
  - [ ] BSL Change License is Apache License 2.0.
  - [ ] Root package metadata uses `BUSL-1.1`.
  - [ ] `apps/loradb.com/LICENSE` is MIT.
  - [ ] No project-authored docs claim the database core is MIT or Apache before
        the Change Date.
- [ ] Version bumped consistently. Run the sync helper — it updates every manifest in one go:
  ```bash
  node scripts/sync-versions.mjs X.Y.Z
  cargo check --workspace                                # refresh Cargo.lock
  (cd crates/lora-node && npm install --package-lock-only --ignore-scripts)
  (cd crates/lora-wasm && npm install --package-lock-only --ignore-scripts)
  (cd apps/loradb.com  && npm install --package-lock-only --ignore-scripts)
  node scripts/sync-versions.mjs X.Y.Z --check           # sanity check
  ```
  Touches: workspace `Cargo.toml`, `crates/lora-node/package.json`,
  `crates/lora-wasm/package.json`, `apps/loradb.com/package.json`,
  `crates/lora-python/pyproject.toml`, and the corresponding lockfiles.
- [ ] The commit that bumps versions is `chore(release): vX.Y.Z`.
- [ ] The release workflow's `verify-versions` job will re-run this check on
      the pushed tag; if any manifest is out of sync with the tag, the release
      fails before any build runs.

## Secrets and sensitive data audit

Run before every tag that will end up on a public remote:

- [ ] `git ls-files | rg -iE '\\.(env|pem|key|p12|jks|keystore|db|sqlite|dump|sql)$'` returns no unexpected hits.
- [ ] `git log --all --source -p -S 'PRIVATE KEY' -S 'BEGIN RSA' -S 'api_key' -S 'aws_secret'` is empty.
- [ ] No embedded production database dumps, customer data, or proprietary third-party data.
- [ ] No credentials in CI workflows (they should all be `${{ secrets.* }}`).
- [ ] No large generated artifacts tracked (`dist/`, `build/`, `pkg-*`, `*.node`, `.venv/`, `node_modules/`).

## Cutting the release

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
3. Three workflows trigger in parallel:
   - `release` — builds the `lora-server` binaries and creates a draft
     GitHub Release.
   - `packages-release` — builds and publishes `@loradb/lora-wasm`,
     `@loradb/lora-node` (+ platform subpackages), and `lora-python` to
     npm / PyPI.
   - `cargo-release` — publishes every public workspace crate to
     crates.io in dependency order.
4. Review the draft GitHub Release:
   - [ ] Every archive attached (Linux x86_64, Windows x86_64, macOS Intel, macOS ARM).
   - [ ] Matching `.sha256` next to each archive.
   - [ ] `lora-server-vX.Y.Z-SHA256SUMS.txt` present.
5. Confirm the published client packages:
   - [ ] <https://www.npmjs.com/package/@loradb/lora-wasm> shows `X.Y.Z`.
   - [ ] <https://www.npmjs.com/package/@loradb/lora-node> shows `X.Y.Z`
         with matching `optionalDependencies` for every platform
         subpackage.
   - [ ] <https://pypi.org/project/lora-python/X.Y.Z/> shows the sdist
         and every platform wheel.
   - [ ] <https://crates.io/crates/lora-database/X.Y.Z> exists, plus the
         other seven public crates (`lora-ast`, `lora-store`,
         `lora-parser`, `lora-analyzer`, `lora-compiler`,
         `lora-executor`, `lora-server`).
6. Paste the release notes and **Publish** the server draft.

## Post-release

- [ ] Smoke-test one downloaded archive end-to-end (verify checksum, extract, start server, run one query).
- [ ] `npm install @loradb/lora-wasm@X.Y.Z` in a throwaway dir and import once.
- [ ] `npm install @loradb/lora-node@X.Y.Z` and run the `require()` smoke test.
- [ ] `pip install lora-python==X.Y.Z` in a fresh venv and run `python examples/basic.py`.
- [ ] `cargo add lora-database@X.Y.Z` in a throwaway crate and run the README snippet.
- [ ] `cargo install lora-server --version X.Y.Z` and start the binary once.
- [ ] Close / move the milestone.
- [ ] Open the next iteration's milestone and bump `version` in workspace `Cargo.toml` to the next `-dev`/pre-release tag if desired.

## Emergency rollback

- A published **server release** can be unpublished on GitHub (it becomes a draft).
- **Published npm / PyPI versions cannot be overwritten.** npm's
  `unpublish` window is 72 hours for packages with no dependents; after
  that, ship a patch release. PyPI never allows re-uploading the same
  version. For any publish mistake, cut `vX.Y.(Z+1)` with the fix.
- **Published crates.io versions cannot be overwritten ever.** You can
  `cargo yank` a broken version (it stays resolvable for existing
  `Cargo.lock` files but is hidden from new dependency solves). Yank is
  not a rollback — cut a patch release.
- A pushed tag can be moved, but only before anyone relies on it. After that,
  cut a new patch release (`vX.Y.Z+1`) instead.
- A pushed commit cannot be taken back once anyone has fetched it — prefer a
  revert commit over force-pushing on `main`.
- See the "Recovery from a failed publish" section in `RELEASING.md` for
  partial-publish recovery (some subpackages out, others not).
