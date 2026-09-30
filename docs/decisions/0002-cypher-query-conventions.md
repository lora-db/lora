# ADR-0002: Cypher Query Conventions

## Status

Accepted (inferred from implementation)

Superseded in part (2026-09-29, `d004ba4e`): the "Schema-aware analysis"
decision below was reversed. See the note in that section.

## Context

The project implements a Cypher query engine. Key design decisions include:

1. How to parse Cypher
2. How to structure the compilation pipeline
3. How to separate read and write execution paths

## Decision

### PEG parser (pest)

Cypher syntax is defined as a PEG grammar in `crates/lora-parser/src/cypher.pest` (~370 lines when this ADR was written; ~663 lines as of 2026-09-30) and parsed using the `pest` library. The parser produces a pest parse tree which is lowered into a typed AST.

### Compiler-style pipeline

Queries flow through five explicit stages:

```
Text -> AST -> ResolvedQuery -> LogicalPlan -> PhysicalPlan -> Rows
```

Each stage has its own data types and can be tested independently.

### Read/write executor separation

Two executor structs exist:

- `Executor<S: GraphStorage>` -- read-only; returns errors for write operators
- `MutableExecutor<S: GraphStorageMut>` -- handles all operators

The server always uses `MutableExecutor` since it cannot know at parse time whether a query is read-only.

> **Update (2026-09-30):** no longer accurate. `Database` compiles the query
> first and classifies the plan (`classify_stream`). Read-only plans run
> through the read-only executors (`PullExecutor` or `Executor`, see
> `crates/lora-database/src/database/stream.rs` and `database/execute.rs`);
> only mutating plans use `MutableExecutor` / `MutablePullExecutor`.

### Schema-aware analysis

The analyzer validates labels, relationship types, and property keys against the live graph state during `MATCH` but accepts any names during `CREATE`/`MERGE`. This catches typos in read queries without restricting write queries.

> **Superseded (2026-09-29, `d004ba4e`):** the analyzer no longer reads the
> graph. Unknown labels and relationship types match nothing and unknown
> property keys read `null`, as in standard Cypher. Checking the stored
> catalog made a query's validity depend on the data (the same query failed
> or passed depending on what had been written, including earlier in the same
> statement). `Analyzer` keeps its `GraphCatalog` type parameter only as
> `PhantomData` (`crates/lora-analyzer/src/analyzer/state.rs`), and
> `property_access_allowed` always returns `true`.

### VarId-based variable resolution

Variables are resolved to `VarId(u32)` during analysis. All downstream stages (compiler, executor) use `VarId` instead of string names. This avoids string comparisons during execution and ensures variable scoping is handled once.

## Rationale

- **PEG via pest** provides a simple, declarative grammar with good error messages. PEG grammars are unambiguous by construction, avoiding the complexity of LR/LALR parser generators. The trade-off is that PEG grammars cannot express left-recursive rules directly.
- **Explicit pipeline stages** follow standard compiler design, making the system easier to understand, debug, and extend. Each stage can be tested and optimized independently.
- **Read/write separation** at the executor level provides type-safe enforcement that read-only contexts cannot modify the graph. This will be useful if read replicas or caching layers are added.
- **Schema-aware analysis** provides early error detection for common mistakes (misspelled labels) while remaining flexible for schema evolution. *(Superseded 2026-09-29; see above.)*
- **VarId resolution** eliminates the cost of string-based variable lookups during execution and centralizes scoping rules in the analyzer.

## Consequences

- The pest grammar is the single source of truth for Cypher syntax; changing it requires understanding PEG semantics
- Five pipeline stages mean changes to a Cypher feature require changes across multiple crates
- The analyzer's live-graph validation means an empty graph accepts any query, but a non-empty graph may reject queries with unknown names *(no longer true since 2026-09-29, `d004ba4e`: unknown names are never errors)*
- `VarId` resolution means variable names are not available at execution time (only in the Row's `RowEntry.name` field, set during projection)

## Conventions

### Naming

| Pipeline stage | Type prefix | Example |
|---------------|-------------|---------|
| AST | (none) | `Match`, `Create`, `Expr` |
| Resolved IR | `Resolved` | `ResolvedMatch`, `ResolvedExpr` |
| Logical plan | (descriptive) | `NodeScan`, `Expand`, `Filter` |
| Physical plan | suffix `Exec` | `NodeScanExec`, `FilterExec` |
| Executor functions | `exec_` prefix | `exec_filter`, `exec_expand` |
| Parser functions | `lower_` prefix | `lower_match`, `lower_expression` |

### Error handling

Each stage has its own error type:
- `ParseError` -- syntax errors with span information
- `SemanticError` -- analysis errors (unknown variable, duplicate alias, etc.)
- `ExecutorError` -- runtime errors (type mismatches, constraint violations)

All use `thiserror` for ergonomic `Display` implementations.

### Result format convention

The server defaults to `Graph` format which extracts node/relationship projections from result rows. The `"format"` field in the request allows clients to choose:

- `"rows"` -- named variable maps
- `"rowArrays"` -- columnar format with a columns header
- `"graph"` -- extracted node/relationship objects (default)
- `"combined"` -- columns + rows + graph in a single payload
