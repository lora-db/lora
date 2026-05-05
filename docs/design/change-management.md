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

### Snapshot format compatibility

Snapshots (see [../operations/snapshots.md](../operations/snapshots.md)) are a durable on-disk contract. Compatibility is governed by `crates/lora-snapshot/src/format.rs`:

| Constant | Meaning |
|---|---|
| `FORMAT_VERSION` | The `LORACOL1` envelope format the current writer emits. |
| `BODY_FORMAT_VERSION` | The graph payload body format encoded inside the envelope. |

The current reader accepts the supported envelope/body versions and rejects
unknown formats with an unsupported-version error. Backward compatibility
requires adding an explicit reader/migration path before changing either format.

#### When to bump snapshot format constants

**Required** for any change to:

- `SnapshotPayload` struct layout (fields added, removed, reordered, renamed).
- Any `PropertyValue` variant (added, removed, reordered, renamed).
- Any field on `NodeRecord` or `RelationshipRecord`.
- Any temporal type (`LoraDate`, `LoraTime`, `LoraLocalTime`, `LoraDateTime`, `LoraLocalDateTime`, `LoraDuration`).
- Any spatial type (`LoraPoint`, SRID handling).
- Any vector-related type (`LoraVector`, `VectorValues`, `VectorCoordinateType` discriminant order).
- The `LORACOL1` envelope header, manifest, compression/encryption metadata, or
  body layout.

Every bump **must** come with a reader path that accepts the prior version or an
explicit migration recipe. See the checklist below.

#### When to drop support for an older snapshot format

Reserved for dropping support for genuinely obsolete files. Requires:

- A release-notes warning on the version that drops support.
- A migration recipe (typically: use the last release that accepted the old version to export via Cypher, then re-import on the new release).
- An explicit contract: any user holding files below the new floor must migrate before upgrading.

Do this rarely. The cost of maintaining a reader path for old versions is often
lower than the cost of breaking operator backups.

#### Forward-compatible changes (safe, no version bump)

- Adding Rust methods or helper APIs that do not change serialized records or
  manifest/body layout.
- Changing in-memory index layout when the snapshot body and rebuild semantics
  are unchanged.
- Adding validation that rejects values which older writers never produced.

The BLAKE3 checksum is validated on every load. A truncated or bit-flipped file
must fail loudly; never silently accept a partially-read file.

#### Checklist for bumping snapshot formats

- [ ] Update `FORMAT_VERSION` and/or `BODY_FORMAT_VERSION` in `crates/lora-snapshot/src/format.rs`.
- [ ] Add a reader branch for the prior version and convert it to current graph state, or document a migration recipe.
- [ ] Add an integration test in `crates/lora-database/tests/snapshot.rs` that loads a frozen file produced by the prior version and verifies the migrated in-memory state.
- [ ] Update the file-format table in [../operations/snapshots.md](../operations/snapshots.md#file-format).
- [ ] Update the serialization-stability rules in [../internals/value-model.md](../internals/value-model.md#serialization-stability) if a new value-level invariant applies.
- [ ] Note the change in [../decisions/0003-snapshot-format.md](../decisions/0003-snapshot-format.md) if the decision space shifted (new header flag semantics, new WAL interplay, etc.).

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
