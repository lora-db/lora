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

1. Tag:
   ```bash
   git tag -a vX.Y.Z -m "lora vX.Y.Z"
   git push origin main
   git push origin vX.Y.Z
   ```
2. Watch the `release` workflow under **Actions**. Wait for every matrix leg to succeed.
3. Review the draft release on GitHub:
   - [ ] Every archive attached (Linux x86_64, Windows x86_64, macOS Intel, macOS ARM).
   - [ ] Matching `.sha256` next to each archive.
   - [ ] `lora-server-vX.Y.Z-SHA256SUMS.txt` present.
4. Paste the release notes and **Publish**.

## Post-release

- [ ] Smoke-test one downloaded archive end-to-end (verify checksum, extract, start server, run one query).
- [ ] Close / move the milestone.
- [ ] Open the next iteration's milestone and bump `version` in workspace `Cargo.toml` to the next `-dev`/pre-release tag if desired.

## Emergency rollback

- A published release can be **unpublished** on GitHub (it becomes a draft).
- A pushed tag can be moved, but only before anyone relies on it. After that,
  cut a new patch release (`vX.Y.Z+1`) instead.
- A pushed commit cannot be taken back once anyone has fetched it — prefer a
  revert commit over force-pushing on `main`.
