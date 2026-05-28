# Private Comparison Benches

This standalone crate is intentionally outside the main LoraDB workspace. It is
used for local head-to-head experiments against other embedded graph databases.

Run the full suite:

```bash
cargo bench --manifest-path comparisons/Cargo.toml --bench comparison
```

Run only the SurrealDB graph-query benches:

```bash
cargo bench --manifest-path comparisons/Cargo.toml --bench comparison surrealdb_graph
```

The SurrealDB cases use the embedded in-memory engine and SurrealQL graph
relations:

- `RELATE source->edge->target` to seed edge tables.
- `record->edge->table` and `SELECT ->edge->table ...` for traversal.
- `->(SELECT ... FROM edge ...)` for graph clauses on edge records.
- `@.{n}->edge->table` for fixed-depth recursive graph paths.

## Optional engines

### Memgraph

Memgraph is included as a Cypher-compatible reference but is **server-only**:
unlike the other engines it has no embedded mode, so the adapter talks to a
running Memgraph instance over the Bolt protocol via the `rsmgclient` crate.

Before running the bench, start Memgraph and make sure it is reachable at
`bolt://127.0.0.1:7687` (the standard default). The simplest way is via
Docker:

```bash
docker run --rm -p 7687:7687 memgraph/memgraph
```

Then enable the `memgraph` Cargo feature:

```bash
cargo bench --manifest-path comparisons/Cargo.toml --features memgraph
```

The bench does not start, stop, or configure Memgraph itself. Between
iterations it issues `MATCH (n) DETACH DELETE n` to wipe the live database
and re-opens the Bolt connection so transaction state is clean.

### Neo4j

Neo4j is another Cypher-compatible, server-only engine. The adapter speaks
Bolt via the `neo4rs` crate and inherits Grafeo's Cypher query strings
through the same alias mechanism Memgraph uses, so no separate `neo4j:`
keys are required in `workloads.yml`.

Because Memgraph already occupies the standard Bolt port `7687`, the Neo4j
adapter defaults to `bolt://127.0.0.1:7688` so both servers can run
side-by-side. Override the URL with `NEO4J_URL` and the credentials with
`NEO4J_USER` / `NEO4J_PASSWORD` (defaults: `neo4j` / `bench`).

```bash
docker run -d --name neo4j-bench -p 7688:7687 \
  -e NEO4J_AUTH=neo4j/bench \
  -e NEO4J_dbms_security_auth__minimum__password__length=4 \
  neo4j:5
```

The `NEO4J_dbms_security_auth__minimum__password__length=4` override is
required: Neo4j 5 rejects passwords shorter than 8 characters, and the
adapter's default password `bench` is shorter than that, so without the
override the container refuses to start. Alternatively, set an 8+
character password via `NEO4J_PASSWORD` and skip the override.

Then enable the `neo4j` Cargo feature:

```bash
cargo bench --manifest-path comparisons/Cargo.toml --features neo4j
```

The bench does not start, stop, or configure Neo4j itself. Between
iterations it issues `MATCH (n) DETACH DELETE n` to wipe the live database
and re-opens the Bolt connection so transaction state is clean.
