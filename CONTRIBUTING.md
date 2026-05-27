# Contributing to LoraDB

## Getting started

1. Clone the repository
2. Ensure Rust 1.87+ is available through `rustup` (the `rust-toolchain.toml` pins stable and installs `rustfmt` / `clippy`)
3. Install Node.js 20+ and enable Corepack if you will touch JS workspaces, the docs site, or the playground
4. Run `cargo build --workspace` to verify the Rust workspace compiles
5. Run `cargo test --workspace` to verify all Rust tests pass

For JavaScript workspaces, bootstrap once from the repository root:

```bash
corepack enable
yarn install --immutable
```

If you only want to try the query language before building locally, use
the browser playground at <https://play.loradb.com>.

## Development workflow

### Building

```bash
cargo build                    # debug build
cargo build --release          # release build (LTO enabled via .cargo/config.toml)
```

### Running the server

```bash
cargo run -p lora-server
```

The server starts at `http://127.0.0.1:4747`. Use `POST /query` with `{"query": "...", "params": {...}, "format": "rows"}` to execute Cypher with optional parameters and response formatting. Override with `--host`/`--port` or `LORA_SERVER_HOST`/`LORA_SERVER_PORT`.

### Testing

```bash
cargo test --workspace         # all tests
cargo test -p lora-store       # single crate
cargo test -p lora-server      # server + HTTP integration tests
```

For docs and frontend work:

```bash
yarn workspace loradb-docs validate-cypher --quiet
yarn workspace @loradb/lora-query build
yarn workspace loradb-docs build
yarn workspace @loradb/play test
```

Docs examples that use `QueryCodeBlock` are treated as runnable
Cypher. Use `CypherSnippet` for fragments, intentionally unsupported
syntax, or examples that require setup not shown nearby. The docs site
has additional authoring notes in `apps/loradb.com/CONTRIBUTING-DOCS.md`.

### Code quality

```bash
cargo clippy --workspace       # lint
cargo fmt --all --check        # format check
cargo fmt --all                # auto-format
```

## Code organization

The workspace has a **core engine pipeline** plus bindings that wrap
it for other runtimes.

Core engine crates (every Cypher query walks these in order):

1. **lora-ast** -- AST type definitions only, no logic
2. **lora-parser** -- PEG grammar (pest) + lowering to AST
3. **lora-builtins-meta** -- generated metadata for built-in functions/operators
4. **lora-store** -- `GraphStorage` / `GraphStorageMut` traits + `InMemoryGraph`
5. **lora-analyzer** -- semantic analysis (variable scoping, label validation)
6. **lora-compiler** -- logical plan, optimizer, physical plan
7. **lora-executor** -- physical plan execution, expression evaluation
8. **lora-snapshot** -- columnar snapshot codec
9. **lora-io** -- filesystem and `.loradb` container helpers
10. **lora-wal** -- write-ahead log segments, replay, and checkpoint fences
11. **lora-database** -- orchestration layer; `Database::execute` drives the pipeline
12. **lora-server** -- HTTP server (Axum), `QueryService` orchestrator

Binding / transport crates (each wraps `lora-database` for one host
runtime):

- **lora-ffi** -- C ABI (`catch_unwind` guards + release header) shared
  by `lora-go` and any third-party cgo consumer
- **lora-binding-buffer** -- shared binary buffer helpers used by native bindings
- **lora-node** -- napi-rs binding for Node.js / TypeScript
- **lora-wasm** -- wasm-pack binding for browser + Node (WASM target)
- **lora-python** -- PyO3 binding built with maturin
- **lora-go** -- cgo binding over `lora-ffi`
- **lora-ruby** -- Magnus / rb-sys native extension
- **bindings/shared-ts** -- shared TypeScript types for `lora-node` + `lora-wasm`

Changes to Cypher language support typically touch crates 1-6 in
order. See [docs/internals/cypher-development.md](docs/internals/cypher-development.md)
for a step-by-step walkthrough.

## Adding a new Cypher feature

The general flow for adding a new clause or expression:

1. Add the grammar rule in `lora-parser/src/cypher.pest`
2. Add the AST type in `lora-ast/src/ast.rs`
3. Add parser lowering in `lora-parser/src/parser.rs`
4. Add resolved types in `lora-analyzer/src/resolved.rs`
5. Add analysis in `lora-analyzer/src/analyzer.rs`
6. Add plan nodes in `lora-compiler/src/logical.rs` and `physical.rs`
7. Add planner logic in `lora-compiler/src/planner.rs`
8. Add execution in `lora-executor/src/executor.rs`
9. Add integration or HTTP test cases in the affected crate's `tests/`
   directory, usually `crates/lora-database/tests/` or
   `crates/lora-server/tests/http.rs`

Update user-facing docs when the feature changes syntax, errors, or
observable behavior. The public docs source lives in
`apps/loradb.com/docs/`; implementation notes belong under `docs/`.

## Commit conventions

All commits on `main` and every commit in a pull request **must** follow
[Conventional Commits](https://www.conventionalcommits.org/). This is
enforced locally via a Husky `commit-msg` hook (commitlint) and in CI via
`.github/workflows/commitlint.yml`.

### Format

```
<type>(<optional scope>): <short subject>

<optional body>

<optional footer(s)>
```

Allowed types:

| Type       | When to use                                                           |
| ---------- | --------------------------------------------------------------------- |
| `feat`     | A new feature visible to users (new clause, function, CLI flag, API). |
| `fix`      | A bug fix.                                                            |
| `docs`     | Documentation-only changes (README, `docs/`, `apps/loradb.com`).      |
| `refactor` | Internal restructuring with no behavior change.                       |
| `perf`     | Performance improvement with no behavior change.                      |
| `test`     | Adding or correcting tests only.                                      |
| `build`    | Build system, Cargo, npm, maturin, packaging changes.                 |
| `ci`       | CI/CD configuration and workflow changes.                             |
| `chore`    | Repo maintenance, dependency bumps, tooling, non-code housekeeping.   |
| `revert`   | Reverting a previous commit.                                          |

Scopes are free-form, but prefer crate or area names: `parser`, `analyzer`,
`compiler`, `executor`, `store`, `server`, `ffi`, `node`, `wasm`, `python`,
`go`, `ruby`, `docs-site`, `release`, `repo`.

Mark breaking changes with either:

- a `!` after the type/scope: `feat(parser)!: drop FOREACH support`, or
- a `BREAKING CHANGE:` footer.

### Examples

```
feat(executor): implement DETACH DELETE

Removes all relationships incident to a matched node before deleting the
node itself. Previously this required an explicit two-step MATCH + DELETE
pattern.
```

```
fix(parser): accept trailing comma in list literals

Closes #123
```

```
ci(commits): enforce conventional commits on pull requests
```

### Local setup (one-time)

```bash
corepack enable
yarn install --immutable    # installs commitlint + husky into the repo root
```

After that, `git commit` runs the Rust formatting and clippy gates through
`.husky/pre-commit`, then runs `commitlint` through `.husky/commit-msg`.

### Other rules

- One logical change per commit.
- Ensure `cargo fmt --all --check` passes before committing.
- Ensure `cargo clippy --workspace -- -D warnings` produces no warnings.
- Ensure `cargo test --workspace` passes before committing.
- Squash fix-up commits before requesting review (`git rebase -i`).

## Pull request process

1. Create a feature branch from `main`
2. Make your changes following the code organization above
3. Add tests for new functionality
4. Ensure all existing tests pass
5. Submit a PR with a clear description of what changed and why

## Contributor License Agreement

By submitting a contribution to this repository, you agree that:

- You have the right to submit the contribution.
- You license your contribution to LoraDB, Inc. under the repository's current
  license terms.
- You grant LoraDB, Inc. the right to relicense your contribution, including
  for future open source conversion, commercial licensing, hosted platform
  licensing, or other distribution models.

This CLA-style grant is required because the core database is licensed under
BSL 1.1 today and converts to Apache 2.0 after the Change Date. LoraDB must be
able to maintain that licensing model for all accepted contributions.
