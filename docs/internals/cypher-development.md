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

Not every feature requires changes in every crate. A new function only needs executor changes (plus tests). A new clause needs all six. `lora-server` is a transport and rarely needs updating for language features.

## Walkthrough: Adding a new clause

This example walks through what it would take to add a hypothetical `FOREACH` clause.

### Step 1: AST definition (`lora-ast/src/ast.rs`)

Add a new struct:

```rust
#[derive(Debug, Clone)]
pub struct ForEach {
    pub variable: Variable,
    pub list: Expr,
    pub updating_clauses: Vec<UpdatingClause>,
    pub span: Span,
}
```

Add it to the `UpdatingClause` enum:

```rust
pub enum UpdatingClause {
    // ... existing variants
    ForEach(ForEach),
}
```

### Step 2: Grammar rule (`lora-parser/src/cypher.pest`)

Add the PEG rule:

```pest
foreach_clause = { FOREACH ~ lparen ~ variable ~ IN ~ expression ~ pipe ~ updating_clause+ ~ rparen }
```

Add `FOREACH` to the reserved words and keyword list.

Add `foreach_clause` to the `updating_clause` alternatives.

### Step 3: Parser lowering (`lora-parser/src/parser.rs`)

Add a `lower_foreach` function that converts a pest pair into the AST struct:

```rust
fn lower_foreach(pair: Pair<Rule>) -> Result<ForEach, ParseError> {
    // Extract children, call lower_expression, lower_updating_clause, etc.
}
```

Wire it into `lower_updating_clause` match arm.

### Step 4: Resolved types (`lora-analyzer/src/resolved.rs`)

Add the resolved representation:

```rust
#[derive(Debug, Clone)]
pub struct ResolvedForEach {
    pub variable: VarId,
    pub list: ResolvedExpr,
    pub clauses: Vec<ResolvedClause>,
}
```

Add `ForEach(ResolvedForEach)` to `ResolvedClause`.

### Step 5: Analyzer (`lora-analyzer/src/analyzer.rs`)

Add analysis logic:

```rust
fn analyze_foreach(&mut self, f: &ForEach) -> Result<ResolvedForEach, SemanticError> {
    let list = self.analyze_expr(&f.list)?;
    let variable = self.declare_fresh_variable(&f.variable.name)?;
    let clauses = f.updating_clauses.iter()
        .map(|c| self.analyze_updating_clause(c))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ResolvedForEach { variable, list, clauses })
}
```

Wire into `analyze_updating_clause`.

### Step 6: Plan nodes (`lora-compiler/src/logical.rs` + `physical.rs`)

Add logical operator:

```rust
pub struct ForEachOp {
    pub input: PlanNodeId,
    pub variable: VarId,
    pub list: ResolvedExpr,
    pub body: Vec<PlanNodeId>,
}
```

Add the physical equivalent and lowering in `optimizer.rs`.

### Step 7: Planner (`lora-compiler/src/planner.rs`)

Add `plan_foreach` method to convert the resolved clause into plan nodes.

### Step 8: Executor (`lora-executor/src/executor.rs`)

Add execution logic in `MutableExecutor`:

```rust
fn exec_foreach(&mut self, plan: &PhysicalPlan, op: &ForEachExec) -> ExecResult<Vec<Row>> {
    // Evaluate list, iterate, execute body for each element
}
```

### Step 8a: Mutation event (write-only features)

If the new feature adds or changes a `GraphStorageMut` method, it must also extend the `MutationEvent` enum. Without this the durability, CDC, and future WAL layer silently drop the mutation.

1. Add or extend a variant in `crates/lora-store/src/mutation.rs::MutationEvent`. The variant must carry exactly the information needed to replay the mutation against an empty store (node IDs, labels, properties, relationship endpoints, etc.) — no references back into the source store.
2. Update the `InMemoryGraph` implementation of the `GraphStorageMut` method to emit the event through the optional recorder **before** returning success. The null-recorder fast path is one pointer check; do not construct the event eagerly.
3. Add a test in `crates/lora-database/tests/snapshot.rs` (or a neighbouring file) that installs a recording `MutationRecorder`, runs the new clause, and asserts the expected event sequence and payload shape.

See [../operations/snapshots.md#mutation-events](../operations/snapshots.md#mutation-events) for the recorder contract and the existing variant list, and [../architecture/graph-engine.md#durability](../architecture/graph-engine.md#durability) for where the trait sits.

### Step 9: Tests

Add integration tests in `crates/lora-database/tests/` (one file per feature area — pick the best fit or create a new one) and unit tests in the relevant crates. For HTTP-layer behavior, extend `crates/lora-server/tests/http.rs`. If the feature is a write, confirm Step 8a's event shape is covered by a recorder test.

## Walkthrough: Adding a new function

Functions are simpler because they only require executor changes:

1. Add handling in `lora-executor/src/eval.rs` in the `eval_function` match:

```rust
"size" => match args.first() {
    Some(LoraValue::List(l)) => LoraValue::Int(l.len() as i64),
    Some(LoraValue::String(s)) => LoraValue::Int(s.len() as i64),
    _ => LoraValue::Null,
},
```

The parser already handles `function_name(args...)` syntax generically.

## Walkthrough: Adding a new expression operator

1. Add the AST operator variant in `lora-ast/src/ast.rs` (e.g., in `BinaryOp`)
2. Add the grammar rule in `cypher.pest`
3. Add parser lowering
4. The analyzer passes through operators without transformation
5. Add evaluation in `lora-executor/src/eval.rs` (`eval_binary` or `eval_unary`)

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

The `PatternContext` enum (`Read` / `Write`) controls validation behavior:
- In `Read` context: labels and types must exist in the graph
- In `Write` context: any label or type name is accepted
