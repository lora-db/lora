# Change Management

## How to evolve the system safely

### Schema evolution

The graph is schema-free -- labels, types, and properties are implicit. This means:

- **Adding a new label**: just use it in a `CREATE` or `SET` statement
- **Adding a new relationship type**: just use it in a `CREATE` statement
- **Adding a property**: just `SET n.newProp = value`
- **Removing a property**: `REMOVE n.prop` on relevant nodes/relationships
- **Renaming a label**: there is no rename operation; create with the new label and remove the old one

There are no migration scripts or schema definition files. All changes are immediate and in-memory.

### Backward compatibility considerations

Since the analyzer validates labels and types against the live graph for `MATCH` queries:

1. If a label is removed from all nodes, `MATCH (n:OldLabel)` will fail with `UnknownLabel`
2. If a relationship type is removed from all relationships, `MATCH ()-[:OLD_TYPE]->()` will fail with `UnknownRelationshipType`
3. If a property key is removed from all entities, `WHERE n.oldProp = x` will fail with `UnknownPropertyAt`

This means removing the last instance of a label/type/property is a breaking change for existing queries. On an empty graph, all names are accepted.

### Adding new Cypher features

Follow the pipeline described in [../internals/cypher-development.md](../internals/cypher-development.md):

1. **Grammar first** — ensure the syntax parses correctly
2. **AST + analyzer** — add type definitions and semantic validation
3. **Compiler + executor** — add planning and execution
4. **Tests** — add integration tests under `crates/lora-database/tests/` and unit tests where applicable
5. **Documentation** — update the [cypher support matrix](../reference/cypher-support-matrix.md), the relevant user-facing pages under `apps/loradb.com/docs/`, and any affected ADR or internal doc

### Changing existing behavior

When modifying how an existing clause works:

1. Read the existing implementation across all pipeline stages
2. Check `crates/lora-database/tests/*.rs` for existing coverage of that behavior
3. Make the change
4. Run `cargo test --workspace`
5. Spot-check with `cargo test -p lora-database --test <area>` for the affected area

### Dependency updates

```bash
cargo update                    # update to latest compatible versions
cargo audit                     # check for known vulnerabilities
cargo test --workspace          # verify nothing broke
```

The `smallvec = "2.0.0-alpha.12"` dependency is a pre-release version. Monitor for a stable 2.0 release.

## Risk areas for changes

| Area | Risk | Reason |
|------|------|--------|
| `cypher.pest` grammar | High | Changes can break parsing of all queries; PEG grammars are sensitive to rule ordering |
| `GraphStorage` / `GraphStorageMut` traits | High | Any signature change affects all downstream crates |
| `InMemoryGraph` indexes | High | Index maintenance bugs cause silent data inconsistency |
| `eval.rs` expression / function dispatch | Medium | Expression semantics affect all queries; test carefully |
| `executor.rs` | Medium | Complex file with interleaved read and write paths |
| `parser.rs` | Medium | Large file; easy to break one rule when adding another |
| Result projection | Low | Only affects output format, not correctness |
| HTTP routing | Low | Simple Axum routes, well-isolated |

## Rollback strategy

Since the graph is ephemeral and there is no deployment pipeline:

- **Code changes**: revert the git commit
- **Data changes**: restart the server (all data is lost)
- **There is no way to undo a query** -- `DELETE` and `SET` are permanent within a session
