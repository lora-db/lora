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

Snapshots (see [../operations/snapshots.md](../operations/snapshots.md)) are a durable on-disk contract. Two constants in `crates/lora-store/src/snapshot.rs` govern compatibility:

| Constant | Meaning |
|---|---|
| `SNAPSHOT_FORMAT_VERSION` | The format the current writer always emits. Currently `1`. |
| `SNAPSHOT_MIN_SUPPORTED_FORMAT_VERSION` | The oldest format the current reader will accept. Older files fail with `SnapshotError::UnsupportedVersion`. |

The reader accepts any version in `[MIN..=CURRENT]` and migrates legacy payloads to the current in-memory shape on load. This is the only place the engine supports backward-compatible file reads.

#### When to bump `SNAPSHOT_FORMAT_VERSION`

**Required** for any change to:

- `SnapshotPayload` struct layout (fields added, removed, reordered, renamed).
- Any `PropertyValue` variant (added, removed, reordered, renamed).
- Any field on `NodeRecord` or `RelationshipRecord`.
- Any temporal type (`LoraDate`, `LoraTime`, `LoraLocalTime`, `LoraDateTime`, `LoraLocalDateTime`, `LoraDuration`).
- Any spatial type (`LoraPoint`, SRID handling).
- Any vector-related type (`LoraVector`, `VectorValues`, `VectorCoordinateType` discriminant order).
- The on-disk header layout beyond the reserved fields.

Every bump **must** come with a reader path that accepts the prior version — deserialize into the legacy shape, then migrate to current. See the checklist below.

#### When to raise `SNAPSHOT_MIN_SUPPORTED_FORMAT_VERSION`

Reserved for dropping support for genuinely obsolete files. Requires:

- A release-notes warning on the major version that raises the floor.
- A migration recipe (typically: use the last release that accepted the old version to export via Cypher, then re-import on the new release).
- An explicit contract: any user holding files below the new floor must migrate before upgrading.

Raise this rarely. The cost of maintaining a reader path for old versions is low; the cost of breaking operator backups is high.

#### Forward-compatible changes (safe, no version bump)

- The snapshot header has a `header_flags: u32` field. Bit 0 (`has_wal_lsn`) gates the reserved `wal_lsn` slot. Future writers may set additional bits; current readers **must** ignore unknown bits. Adding a new bit is forward-compatible provided its semantics leave the payload itself unchanged.
- The CRC32 trailer is validated on every load — a truncated or bit-flipped file must fail loudly with `SnapshotError::BadCrc` or `SnapshotError::BadMagic`. Never silently accept a partially-read file.
- Indexes (label, relationship-type, adjacency) are rebuilt on load and are **not** serialized. Changing the in-memory index layout is therefore not a wire change.

#### Checklist for bumping `SNAPSHOT_FORMAT_VERSION`

- [ ] Update `SNAPSHOT_FORMAT_VERSION` in `crates/lora-store/src/snapshot.rs`.
- [ ] Add a reader branch for the prior version that deserializes the legacy shape and converts it to current.
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
