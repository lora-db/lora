# ADR-0002: Cypher Query Conventions

## Status

Accepted (inferred from implementation)

## Context

The project implements a Cypher query engine. Key design decisions include:

1. How to parse Cypher
2. How to structure the compilation pipeline
3. How to separate read and write execution paths

## Decision

### PEG parser (pest)

Cypher syntax is defined as a PEG grammar in `cypher.pest` (currently ~370 lines) and parsed using the `pest` library. The parser produces a pest parse tree which is lowered into a typed AST.

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

### Schema-aware analysis

The analyzer validates labels, relationship types, and property keys against the live graph state during `MATCH` but accepts any names during `CREATE`/`MERGE`. This catches typos in read queries without restricting write queries.

### VarId-based variable resolution

Variables are resolved to `VarId(u32)` during analysis. All downstream stages (compiler, executor) use `VarId` instead of string names. This avoids string comparisons during execution and ensures variable scoping is handled once.

## Rationale

- **PEG via pest** provides a simple, declarative grammar with good error messages. PEG grammars are unambiguous by construction, avoiding the complexity of LR/LALR parser generators. The trade-off is that PEG grammars cannot express left-recursive rules directly.
- **Explicit pipeline stages** follow standard compiler design, making the system easier to understand, debug, and extend. Each stage can be tested and optimized independently.
- **Read/write separation** at the executor level provides type-safe enforcement that read-only contexts cannot modify the graph. This will be useful if read replicas or caching layers are added.
- **Schema-aware analysis** provides early error detection for common mistakes (misspelled labels) while remaining flexible for schema evolution.
- **VarId resolution** eliminates the cost of string-based variable lookups during execution and centralizes scoping rules in the analyzer.

## Consequences

- The pest grammar is the single source of truth for Cypher syntax; changing it requires understanding PEG semantics
- Five pipeline stages mean changes to a Cypher feature require changes across multiple crates
- The analyzer's live-graph validation means an empty graph accepts any query, but a non-empty graph may reject queries with unknown names
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
