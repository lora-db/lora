# Cypher Development Guide

This guide walks through how to extend the Cypher implementation in Lora. It covers adding new clauses, expressions, operators, and functions.

## Pipeline overview

Every Cypher feature touches up to six crates in a fixed order:

```
1. lora-ast         Add AST type definitions
2. lora-parser      Add grammar rule + AST lowering
3. lora-analyzer    Add semantic analysis + resolved types
4. lora-compiler    Add logical/physical plan nodes + planner logic
5. lora-executor    Add execution logic
6. lora-database    Add integration tests under tests/
```

Not every feature requires changes in every crate. A new function needs a
`lora-builtins-meta` entry plus executor changes (and tests). A new row-producing clause usually needs all
six. Schema commands are the main exception: `CREATE INDEX`, `DROP INDEX`, and
`SHOW INDEXES` parse into `Statement::Schema` and are routed by
`lora-database/src/database/schema.rs` directly to the store catalog instead of
going through analyzer/compiler/executor operators. `lora-server` is a transport
and rarely needs updating for language features.

## Walkthrough: Adding a new clause

This walks through how `FOREACH` was added, using the real code, as a template
for the next clause. (For a feature that is not implemented yet, see the
"Not implemented" rows of the
[Cypher support matrix](../reference/cypher-support-matrix.md), for example
`COLLECT { }` subqueries.)

### Step 1: AST definition (`lora-ast/src/ast.rs`)

Add a new struct:

```rust
#[derive(Debug, Clone)]
pub struct Foreach {
    pub variable: Variable,
    pub list: Expr,
    pub body: Vec<UpdatingClause>,
    pub span: Span,
}
```

Add it to the `UpdatingClause` enum:

```rust
pub enum UpdatingClause {
    // ... existing variants
    Foreach(Foreach),
}
```

### Step 2: Grammar rule (`lora-parser/src/cypher.pest`)

Add the PEG rule:

```pest
foreach_clause = {
    FOREACH ~ lparen ~ variable ~ IN ~ expression ~ pipe
    ~ updating_clause+
    ~ rparen
}
```

Add a `FOREACH = @{ ^"FOREACH" ~ !ident_part }` keyword rule and add `FOREACH`
to the reserved-word list.

Add `foreach_clause` to the `updating_clause` alternatives.

### Step 3: Parser lowering (`lora-parser/src/parser/`)

Lowering is split by area under `src/parser/`. `lower_foreach` lives in
`clauses.rs` and converts a pest pair into the AST struct:

```rust
pub(super) fn lower_foreach(pair: Pair<Rule>) -> Result<Foreach, ParseError> {
    // Walk pair.into_inner(): Rule::variable -> lower_variable,
    // Rule::expression -> lower_expression,
    // Rule::updating_clause -> super::query::lower_updating_clause
}
```

It is wired into the `Rule::foreach_clause` arm of `lower_updating_clause` in
`query.rs`.

### Step 4: Resolved types (`lora-analyzer/src/resolved.rs`)

Add the resolved representation:

```rust
#[derive(Debug, Clone)]
pub struct ResolvedForeach {
    pub variable: VarId,
    pub list: ResolvedExpr,
    pub body: Vec<ResolvedClause>,
}
```

Add `Foreach(ResolvedForeach)` to `ResolvedClause`.

### Step 5: Analyzer (`lora-analyzer/src/analyzer/clauses.rs`)

Add analysis logic. The loop variable must not leak into later clauses, so the
outer scope is snapshotted and restored:

```rust
pub(super) fn analyze_foreach(&mut self, f: &Foreach) -> Result<ResolvedForeach, SemanticError> {
    let list = self.analyze_expr(&f.list)?;          // outer scope
    let outer = self.visible_bindings();
    let variable = self.declare_fresh_variable(&f.variable.name)?;
    let body = f.body.iter()
        .map(|c| self.analyze_foreach_body_clause(c)) // updating clauses only
        .collect::<Result<Vec<_>, _>>()?;
    self.replace_scope(outer);
    Ok(ResolvedForeach { variable, list, body })
}
```

Wire it into the updating-clause match.

### Step 6: Plan nodes (`lora-compiler/src/logical.rs` + `physical.rs`)

Add the logical operator:

```rust
pub struct Foreach {
    pub input: PlanNodeId,
    pub variable: VarId,
    pub list: ResolvedExpr,
    pub body: Vec<ResolvedClause>,
}
```

Add the physical equivalent (`ForeachExec`) and its arm in
`lower_logical_op` in `optimizer.rs`. If the operator writes, add it to
`plan_is_mutating` in `lora-executor/src/pull/shape.rs`, or the plan is
classified read-only and runs on the read-only executor.

### Step 7: Planner (`lora-compiler/src/planner.rs`)

Add a `plan_foreach` method to convert the resolved clause into a plan node.

### Step 8: Executor (`lora-executor/src/executor/` and `src/pull/`)

Add execution logic in `MutableExecutor` (`executor/mutable.rs`):

```rust
fn exec_foreach(&mut self, plan: &PhysicalPlan, op: &ForeachExec) -> ExecResult<Vec<Row>> {
    // Evaluate list per input row, bind the variable, apply each body clause,
    // pass the input row through unchanged
}
```

Row-producing read operators should also get a streaming `RowSource` in
`src/pull/` so `stream()` and early-`LIMIT` reads stay lazy. An operator with
no pull source (as `Foreach` has none) runs through the buffered executor as a
fallback.

### Step 8a: Mutation event (write-only features)

If the new feature adds or changes a `GraphStorageMut` method, it must also extend the `MutationEvent` enum. Without this the durability, CDC, and future WAL layer silently drop the mutation.

1. Add or extend a variant in `crates/lora-store/src/mutation.rs::MutationEvent`. The variant must carry exactly the information needed to replay the mutation against an empty store (node IDs, labels, properties, relationship endpoints, etc.) — no references back into the source store.
2. Update the `InMemoryGraph` implementation of the `GraphStorageMut` method to emit the event through the optional recorder **before** returning success. The null-recorder fast path is one pointer check; do not construct the event eagerly.
3. Add a test in `crates/lora-database/tests/snapshot.rs` (or a neighbouring file) that installs a recording `MutationRecorder`, runs the new clause, and asserts the expected event sequence and payload shape.

See [../operations/snapshots.md#mutation-events](../operations/snapshots.md#mutation-events) for the recorder contract and the existing variant list, and [../architecture/graph-engine.md#durability](../architecture/graph-engine.md#durability) for where the trait sits.

### Step 9: Tests

Add integration tests in `crates/lora-database/tests/` (one file per feature area — pick the best fit or create a new one) and unit tests in the relevant crates. For HTTP-layer behavior, extend `crates/lora-server/tests/http.rs`. If the feature is a write, confirm Step 8a's event shape is covered by a recorder test.

## Walkthrough: Adding a new function

Functions are simpler. Builtins are namespaced (`<namespace>.<operation>`,
e.g. `value.size`); bare historical names such as `size()` are aliases.

1. Declare it in `crates/lora-builtins-meta/src/lib.rs`: add a `spec(...)`
   entry (name, min/max arity) to `BUILTIN_SPECS`, and an `alias(...)` to
   `BUILTIN_ALIASES` if a bare name should resolve to it. The analyzer checks
   names and arities against this table, so without the entry the function is
   rejected as unknown.
2. Add the dispatch arm in the namespace module under
   `lora-executor/src/eval/builtins/` (for example `value.rs`):

```rust
"size" => size(args),
```

The `drift_tests` in `eval/builtins/mod.rs` fail if a `BUILTIN_SPECS` entry has
no dispatch arm. The parser already handles `function_name(args...)` syntax
generically.

## Walkthrough: Adding a new expression operator

1. Add the AST operator variant in `lora-ast/src/ast.rs` (e.g., in `BinaryOp`)
2. Add the grammar rule in `cypher.pest`
3. Add parser lowering
4. The analyzer passes through operators without transformation
5. Add evaluation in `lora-executor/src/eval/` (`binops.rs` for binary operators, `expr.rs` for the rest)

## Naming conventions (observed)

| Concept | Convention | Example |
|---------|-----------|---------|
| AST types | PascalCase, matches Cypher syntax | `Match`, `Create`, `PatternElement` |
| Resolved types | Prefix with `Resolved` | `ResolvedMatch`, `ResolvedExpr` |
| Logical operators | Short PascalCase | `NodeScan`, `Expand`, `Filter` |
| Physical operators | Suffix with `Exec` | `NodeScanExec`, `FilterExec` |
| Variables | `VarId(u32)` | Monotonically assigned |
| Node/Rel IDs | `NodeId` / `RelationshipId` (both `u64`) | Monotonically assigned |
| Parser functions | `lower_` prefix | `lower_match`, `lower_expression` |
| Executor functions | `exec_` prefix | `exec_filter`, `exec_expand` |

## Common patterns

### Error handling

- Parser errors: `ParseError::new(message, start, end)`
- Analyzer errors: `SemanticError` enum variants
- Executor errors: `ExecutorError` enum variants
- All use `thiserror` for derive

### Span tracking

Every AST node carries a `Span { start, end }` representing byte offsets in the source text. When creating new AST nodes, always extract the span from the pest pair using `pair_span(&pair)`.

### Working with pest pairs

The parser uses several helper patterns:
- `single_inner(pair)` -- extract the single child of a pair
- `pair_span(pair)` -- convert pest span to AST span
- `unexpected_rule(context, pair)` -- create an error for unexpected grammar matches

### Read vs write context in analyzer

The `PatternContext` enum (`Read` / `OptionalRead` / `Write`) records where a
pattern appears. In read contexts it still drives the check that one node
variable is not given conflicting label sets (`analyzer/patterns.rs`). It no
longer gates names: `validate_label_name` and
`validate_relationship_type_name` are no-ops. Since 2026-09-29 (`d004ba4e`)
the analyzer does not read the graph, so any label, relationship type, or
property key is accepted in every context. An unknown label or type matches nothing and
an unknown property reads `null`. Do not add checks against the stored catalog:
a query's validity must not depend on the data.
