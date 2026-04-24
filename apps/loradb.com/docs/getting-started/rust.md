---
title: Using LoraDB in Rust
sidebar_label: Rust
description: Embed LoraDB directly in a Rust binary via the lora-database crate — the reference API with a strongly typed LoraValue enum, Result-based errors, and a cheap Send + Sync handle.
---

# Using LoraDB in Rust

## Overview

The Rust API is the reference surface — every other binding wraps
the same `lora_database::Database` type. Results map to a strongly
typed `LoraValue` enum; errors propagate through `Result`. The
handle is `Send + Sync` and cheap to clone; the underlying store is
guarded by a mutex.

## Installation / Setup

[![crates.io](https://img.shields.io/crates/v/lora-database?label=crates.io&logo=rust)](https://crates.io/crates/lora-database)

While pre-release, consume the crate as a workspace path or git
dependency rather than from crates.io:

```toml
# Cargo.toml
[dependencies]
lora-database = { path = "../../crates/lora-database" }
anyhow        = "1"
# or, once published:
# lora-database = "0.1"
```

## Creating a Client / Connection

```rust
use lora_database::Database;

fn main() -> anyhow::Result<()> {
    let db = Database::in_memory();
    Ok(())
}
```

`Database::in_memory()` returns a ready-to-use handle with an empty
graph. Clone it (via `Arc`) to share across threads — the inner
store is shared, not duplicated.

## Running Your First Query

```rust
use lora_database::Database;

fn main() -> anyhow::Result<()> {
    let db = Database::in_memory();

    db.execute("CREATE (:Person {name: 'Ada', born: 1815})", None)?;

    let result = db.execute(
        "MATCH (p:Person) RETURN p.name AS name",
        None,
    )?;

    println!("{:?}", result);
    Ok(())
}
```

The second argument is `Option<ExecuteOptions>` — pass `None` for
defaults.

## Examples

### Minimal working example

Already shown above — `in_memory` → `execute` → inspect.

### Parameterised query

```rust
use std::collections::BTreeMap;
use lora_database::{Database, LoraValue};

fn main() -> anyhow::Result<()> {
    let db = Database::in_memory();
    db.execute("CREATE (:Person {name: 'Ada', born: 1815})", None)?;

    let mut params = BTreeMap::new();
    params.insert("name".to_string(), LoraValue::String("Ada".into()));
    params.insert("min".to_string(),  LoraValue::Int(1800));

    let result = db.execute_with_params(
        "MATCH (p:Person)
         WHERE p.name = $name AND p.born >= $min
         RETURN p.name AS name, p.born AS born",
        None,
        params,
    )?;
    println!("{:?}", result);
    Ok(())
}
```

Missing parameters resolve to `null`. Always bind every `$name` used
in the query. See [Queries → Parameters](../queries/parameters).

### Structured result handling

```rust
use lora_database::{Database, LoraValue, QueryResult};

fn names(db: &Database) -> anyhow::Result<Vec<String>> {
    let result = db.execute("MATCH (p:Person) RETURN p.name AS name", None)?;
    let QueryResult::RowArrays { columns, rows } = result else {
        anyhow::bail!("unexpected result shape");
    };
    let idx = columns.iter().position(|c| c == "name").unwrap();

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        if let LoraValue::String(s) = &row[idx] {
            out.push(s.clone());
        }
    }
    Ok(out)
}
```

See [Data Types → Scalars](../data-types/scalars) for the full
`LoraValue` variants.

### Service-layer abstraction

A thin wrapper you'd realistically put in your application code:

```rust
use std::collections::BTreeMap;
use std::sync::Arc;
use lora_database::{Database, LoraValue};

#[derive(Clone)]
pub struct UserService {
    db: Arc<Database>,
}

impl UserService {
    pub fn new(db: Arc<Database>) -> Self { Self { db } }

    pub fn upsert_user(&self, id: i64, name: &str) -> anyhow::Result<()> {
        let mut params = BTreeMap::new();
        params.insert("id".into(),   LoraValue::Int(id));
        params.insert("name".into(), LoraValue::String(name.into()));
        self.db.execute_with_params(
            "MERGE (u:User {id: $id})
             ON CREATE SET u.created = timestamp()
             SET u.name = $name, u.updated = timestamp()",
            None,
            params,
        )?;
        Ok(())
    }

    pub fn count(&self) -> anyhow::Result<i64> {
        let r = self.db.execute("MATCH (u:User) RETURN count(*) AS n", None)?;
        extract_int(r, "n")
    }
}

// helper omitted for brevity — map the RowArrays result to an i64
# fn extract_int(_r: lora_database::QueryResult, _c: &str) -> anyhow::Result<i64> { Ok(0) }
```

### Handle errors

Every `execute` call returns `Result`. Distinguish query errors from
connection-layer errors (not currently surfaced in the in-memory
binding, but relevant when embedding):

```rust
use lora_database::Database;

fn main() {
    let db = Database::in_memory();

    match db.execute("BAD QUERY", None) {
        Ok(_)  => println!("ok"),
        Err(e) => {
            // engine-level parse / semantic / runtime error
            eprintln!("query failed: {e}");
        }
    }
}
```

Common causes: parse errors, unknown labels, unknown functions. See
[Troubleshooting → Parse errors](../troubleshooting#parse-errors)
and [Semantic errors](../troubleshooting#semantic-errors).

### Concurrency

```rust
use std::sync::Arc;
use lora_database::Database;

fn main() -> anyhow::Result<()> {
    let db = Arc::new(Database::in_memory());

    let h1 = {
        let db = Arc::clone(&db);
        std::thread::spawn(move || -> anyhow::Result<()> {
            db.execute("CREATE (:X)", None)?;
            Ok(())
        })
    };
    let h2 = {
        let db = Arc::clone(&db);
        std::thread::spawn(move || -> anyhow::Result<()> {
            db.execute("MATCH (x) RETURN count(*)", None)?;
            Ok(())
        })
    };
    h1.join().unwrap()?;
    h2.join().unwrap()?;
    Ok(())
}
```

Calls serialise on the inner mutex; no data races, but no parallel
execution either.

### Persisting your graph

LoraDB can save the in-memory graph to a single file and restore it
later. It's a point-in-time dump — simple, atomic on rename, no WAL.

```rust
use lora_database::{Database, SnapshotMeta};

let db = Database::in_memory();
db.execute("CREATE (:Person {name: 'Ada'})", None)?;

// Save everything to disk.
let meta: SnapshotMeta = db.save_snapshot_to("graph.bin")?;
println!(
    "{} nodes, {} relationships",
    meta.node_count, meta.relationship_count,
);

// Boot a fresh Database from the saved file.
let db2 = Database::in_memory_from_snapshot("graph.bin")?;

// Or overlay a snapshot onto an existing handle.
db.load_snapshot_from("graph.bin")?;
```

Both save and load serialise against every query on the handle — the
snapshot holds the same mutex as `execute`. A crash between saves
loses every mutation since the last save.

See the canonical [Snapshots guide](../snapshot) for the full
metadata shape, file format, atomic-rename guarantees, and boundaries.

## Common Patterns

### Bulk insert from a `Vec`

```rust
use lora_database::{Database, LoraValue};
use std::collections::BTreeMap;

let db = Database::in_memory();
let rows: Vec<LoraValue> = (0..1000u64).map(|i| {
    let mut m: BTreeMap<String, LoraValue> = BTreeMap::new();
    m.insert("id".into(),   LoraValue::Int(i as i64));
    m.insert("name".into(), LoraValue::String(format!("user-{i}")));
    LoraValue::Map(m)
}).collect();

let mut params: BTreeMap<String, LoraValue> = BTreeMap::new();
params.insert("rows".into(), LoraValue::List(rows));

db.execute_with_params(
    "UNWIND $rows AS row CREATE (:User {id: row.id, name: row.name})",
    None,
    params,
)?;
```

See [UNWIND](../queries/unwind-merge#bulk-load-from-parameter).

### Share a `Database` across threads or tasks

Wrap in `Arc` and clone freely. Calls serialise on the internal
mutex — the clones share a single graph.

### Result format selection

`execute` returns `Result<QueryResult>`. `QueryResult` has variants
for different output shapes:

```rust
pub enum QueryResult {
    RowArrays { columns: Vec<String>, rows: Vec<Vec<LoraValue>> },
    Rows       { columns: Vec<String>, rows: Vec<BTreeMap<String, LoraValue>> },
    Graph      { /* nodes, relationships */ },
    Combined   { /* rows + graph */ },
}
```

Control which shape you get via `ExecuteOptions::format`. The engine
default is `Graph`. See [Result formats](../concepts/result-formats)
for how each shape looks and when to pick which.

### LoraValue at a glance

- `Null`, `Bool`, `Int(i64)`, `Float(f64)`, `String`
- `List(Vec<LoraValue>)`, `Map(BTreeMap<String, LoraValue>)`
- `Node(u64)`, `Relationship(u64)`, `Path { nodes, rels }`
- Temporal types: `Date`, `Time`, `LocalTime`, `DateTime`,
  `LocalDateTime`, `Duration`
- `Point { x, y, z?, srid }`

Node and relationship variants hold only an ID. Use
`ExecuteOptions { format: ResultFormat::Rows, hydrate: true }` (or
re-materialise with `MATCH (n) … RETURN n {.*}`) for full maps with
labels and properties.

## Error Handling

Everything is `Result`. Errors fall into three buckets:

| Bucket | Typical cause | How to handle |
|---|---|---|
| Parse | Missing paren, bad syntax | Fix the query string |
| Semantic | Unknown label, unknown function, wrong arity | Adjust names or fix version |
| Runtime | `DeleteNodeWithRelationships`, division by zero (returns `null`, doesn't error), integer overflow (debug only) | Adjust query; see [Troubleshooting](../troubleshooting) |

Pattern:

```rust
if let Err(e) = db.execute("BAD QUERY", None) {
    tracing::error!(error = %e, "query failed");
}
```

## Performance / Best Practices

- **One mutex, one graph.** Multiple `execute()` calls on the same
  `Database` serialise.
- **Clone the handle, not the data.** `Arc<Database>` gives every
  thread / task a cheap clone; the inner `Arc<Mutex<Store>>` is
  shared.
- **No query timeout.** A pathological query will hold the lock
  indefinitely. Cap variable-length traversals, and ensure parameter
  sizes are reasonable.
- **Release build for benchmarks.** Debug builds are ~10× slower
  for most query shapes.

## See also

- [**Ten-Minute Tour**](./tutorial) — guided walkthrough (same queries in Rust).
- [**Queries**](../queries) — clause reference.
- [**Cookbook**](../cookbook) — scenario-based recipes.
- [**Functions**](../functions/overview) — every built-in.
- [**Data types**](../data-types/overview) — host ↔ engine mapping.
- [**Troubleshooting**](../troubleshooting) — common errors.
- [**Limitations**](../limitations) — what's not supported.
- [**Node guide**](./node) / [**Python guide**](./python) /
  [**WASM guide**](./wasm) / [**HTTP server**](./server) — same
  surface, different host.
