# Contributing to LoraDB

## Getting started

1. Clone the repository
2. Ensure Rust stable is installed (the `rust-toolchain.toml` will handle component installation)
3. Run `cargo build` to verify the workspace compiles
4. Run `cargo test --workspace` to verify all tests pass

## Development workflow

### Building

```bash
cargo build                    # debug build
cargo build --release          # release build (LTO enabled via .cargo/config.toml)
```

### Running the server

```bash
cargo run --bin lora-server
```

The server starts at `http://127.0.0.1:4747`. Use `POST /query` with `{"query": "..."}` to execute Cypher. Override with `--host`/`--port` or `LORA_SERVER_HOST`/`LORA_SERVER_PORT`.

### Testing

```bash
cargo test --workspace         # all tests
cargo test -p lora-store     # single crate
cargo test -p lora-server      # server + HTTP integration tests
```

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
3. **lora-store** -- `GraphStorage` / `GraphStorageMut` traits + `InMemoryGraph`
4. **lora-analyzer** -- semantic analysis (variable scoping, label validation)
5. **lora-compiler** -- logical plan, optimizer, physical plan
6. **lora-executor** -- physical plan execution, expression evaluation
7. **lora-database** -- orchestration layer; `Database::execute` drives the pipeline
8. **lora-server** -- HTTP server (Axum), `QueryService` orchestrator

Binding / transport crates (each wraps `lora-database` for one host
runtime):

- **lora-ffi** -- C ABI (`catch_unwind` guards + release header) shared
  by `lora-go` and any third-party cgo consumer
- **lora-node** -- napi-rs binding for Node.js / TypeScript
- **lora-wasm** -- wasm-pack binding for browser + Node (WASM target)
- **lora-python** -- PyO3 binding built with maturin
- **lora-go** -- cgo binding over `lora-ffi`
- **lora-ruby** -- Magnus / rb-sys native extension
- **shared-ts** -- shared TypeScript types for `lora-node` + `lora-wasm`

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
9. Add HTTP test cases in `lora-server/tests/queries.http`

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
npm install    # installs commitlint + husky into the repo root
```

After that, `git commit` runs `commitlint` automatically through the
`.husky/commit-msg` hook.

### Other rules

- One logical change per commit.
- Ensure `cargo test --workspace` passes before committing.
- Ensure `cargo clippy --workspace` produces no warnings.
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
