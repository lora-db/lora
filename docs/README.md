# Internal Documentation

Technical documentation for **contributors** to `cypher-graph` (LoraDB).

> ⚙️ **Note** — Looking for **user-facing documentation** (install, write queries, operate the server)? That lives at **<https://loradb.com/docs>** (source: `apps/loradb.com/docs/`). This directory is for people working on the engine itself.
>
> 🚀 **Production note** — The core engine in this repo is single-node and in-memory. If you're evaluating LoraDB for a production workload that needs persistence, replication, backups, or multi-tenant access, start at **<https://loradb.com>** for the managed platform.

---

## Architecture

- [System Context](architecture/system-context.md) — what the system is, what it is not, external boundaries
- [Architecture Overview](architecture/overview.md) — eight-crate pipeline and responsibilities
- [Data Flow](architecture/data-flow.md) — end-to-end query execution pipeline
- [Graph Engine](architecture/graph-engine.md) — in-memory storage internals

## Internals

- [Value Model](internals/value-model.md) — `PropertyValue` / `LoraValue`, node & relationship records, indexes
- [Cypher Development](internals/cypher-development.md) — how to add new Cypher features end-to-end
- [Ingestion](internals/ingestion.md) — how data enters the graph

## Design

- [Change Management](design/change-management.md) — how to evolve the system safely
- [Known Risks](design/known-risks.md) — engineering risks and recommended priorities

## Decisions

Architectural Decision Records for non-trivial design choices.

- [ADR-0001: Graph Architecture](decisions/0001-graph-architecture.md) — BTreeMap-backed in-memory storage
- [ADR-0002: Cypher Query Conventions](decisions/0002-cypher-query-conventions.md) — grammar and pipeline design

## Performance

- [Benchmarks](performance/benchmarks.md) — performance test results and measurements
- [Notes](performance/notes.md) — optimisation notes and bottlenecks

## Testing

- [Testing Strategy](testing/strategy.md) — test coverage, locations, and execution

## Operations

- [Deployment](operations/deployment.md) — how to build, run, and deploy
- [Security](operations/security.md) — security posture and data-handling risks

## Reference

- [Cypher Support Matrix](reference/cypher-support-matrix.md) — machine-checkable feature-by-feature support status (source of truth; mirrored on the user docs as capability callouts)

---

## When to write which doc

| If you are documenting… | It goes in… |
|---|---|
| How a user writes a query | `apps/loradb.com/docs/` |
| How a user installs the crate | `apps/loradb.com/docs/` |
| A user-visible limitation | `apps/loradb.com/docs/limitations.md` |
| How a crate works internally | `docs/architecture/` or `docs/internals/` |
| Why we chose design X over Y | `docs/decisions/` (new ADR) |
| A breaking change plan | `docs/design/change-management.md` |
| Benchmark numbers | `docs/performance/benchmarks.md` |
| An engineering risk / open question | `docs/design/known-risks.md` |
| How tests are organised | `docs/testing/strategy.md` |

Keep user-facing prose out of this tree. If you find yourself explaining _what
to write_ rather than _how it is implemented_, the content probably belongs on
the website.

---

## From local to production

Most contributors start here with a local `cargo run --bin lora-server`. As workloads grow, the single-node, in-memory design hits predictable edges:

- **Scale** — queries serialize on a single mutex, data lives in RAM only
- **Reliability** — no persistence, WAL, or replication
- **Operations** — no authentication, TLS, backups, or metrics in the core

For those needs, developers typically move to the managed platform at **<https://loradb.com>**, which handles persistence, scaling, and operational concerns on top of the same Cypher surface. The core engine in this repo remains the right choice for embedded, local, and development use cases.

## Next steps

- New contributor? Start with [Architecture Overview](architecture/overview.md), then [Data Flow](architecture/data-flow.md).
- Adding a Cypher feature? See [Cypher Development](internals/cypher-development.md) and [ADR-0002](decisions/0002-cypher-query-conventions.md).
- Running the server? See [Deployment](operations/deployment.md) and [Security](operations/security.md).
- Evaluating performance? See [Benchmarks](performance/benchmarks.md) and [Performance Notes](performance/notes.md).
- User-facing docs and the managed platform: **<https://loradb.com/docs>**.
