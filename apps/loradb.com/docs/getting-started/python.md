---
title: Using LoraDB in Python
sidebar_label: Python
description: Install and use LoraDB in Python via the PyO3 lora-python binding — synchronous Database and asyncio-friendly AsyncDatabase with identical shapes, installed through pip.
---

# Using LoraDB in Python

## Overview

`lora-python` is a PyO3 binding built with `maturin`. It ships two
classes with identical shapes: a synchronous `Database` and an
asyncio-friendly `AsyncDatabase`. Switching between them is a
one-line import change.

## Installation / Setup

[![PyPI](https://img.shields.io/pypi/v/lora-python?label=pypi&logo=pypi&logoColor=white)](https://pypi.org/project/lora-python/)

### Requirements

- Python **3.8+**
- For building from source: Rust toolchain (`rustup`) + `maturin`

### Install (from source, pre-release)

```bash
cd crates/lora-python
python3 -m venv .venv
source .venv/bin/activate
pip install -U maturin
maturin develop            # editable install into the active venv
```

### Install (release wheel)

```bash
maturin build --release
pip install target/wheels/lora_python-*.whl
```

### Install (after publish)

```bash
pip install lora-python
```

## Creating a Client / Connection

```python
from lora_python import Database

db = Database.create()
```

`Database.create()` and `Database()` do the same thing — the factory
exists for API symmetry with `AsyncDatabase`.

Asyncio equivalent:

```python
from lora_python import AsyncDatabase

db = await AsyncDatabase.create()
```

## Running Your First Query

```python
from lora_python import Database

db = Database.create()

db.execute("CREATE (:Person {name: 'Ada', born: 1815})")

result = db.execute("MATCH (p:Person) RETURN p.name AS name, p.born AS born")

print(result["rows"])
# [{'name': 'Ada', 'born': 1815}]
```

## Examples

### Minimal working example

Already shown above.

### Parameterised query

```python
result = db.execute(
    "MATCH (p:Person) WHERE p.name = $name RETURN p.name AS name",
    {"name": "Ada"},
)
```

Python values map to engine values automatically:
`int`/`float`/`bool`/`str`/`None` and `list`/`dict` pass through. For
temporal and spatial values, use the tagged helpers below.

### Structured result handling

```python
from lora_python import Database, is_node

db = Database.create()
db.execute("CREATE (:Person {name: 'Ada'})")

result = db.execute("MATCH (n:Person) RETURN n")

for row in result["rows"]:
    n = row["n"]
    if is_node(n):
        print(n["id"], n["labels"], n["properties"])
```

Available guards: `is_node`, `is_relationship`, `is_path`,
`is_point`, `is_temporal`.

### FastAPI route handler

```python
from fastapi import FastAPI, HTTPException
from lora_python import AsyncDatabase, LoraQueryError

app = FastAPI()
db: AsyncDatabase  # initialised at startup

@app.on_event("startup")
async def _bootstrap():
    global db
    db = await AsyncDatabase.create()

@app.get("/users/{user_id}")
async def get_user(user_id: int):
    try:
        res = await db.execute(
            "MATCH (u:User {id: $id}) RETURN u {.id, .handle, .tier} AS user",
            {"id": user_id},
        )
    except LoraQueryError as exc:
        raise HTTPException(status_code=400, detail=str(exc))
    rows = res["rows"]
    if not rows:
        raise HTTPException(status_code=404)
    return rows[0]["user"]
```

Works unchanged under Flask/Django/Litestar — swap the framework
but keep the `AsyncDatabase` instance at module scope.

### Handle errors

```python
from lora_python import Database, LoraQueryError, InvalidParamsError

db = Database.create()

try:
    db.execute("BAD QUERY")
except LoraQueryError as exc:
    print("query failed:", exc)
except InvalidParamsError as exc:
    print("bad params:", exc)
```

`LoraError` is the common base class — catch it if you don't need
to distinguish.

### Sync vs async

Sync:

```python
from lora_python import Database

db = Database.create()
db.execute("CREATE (:Node)")
```

Async:

```python
import asyncio
from lora_python import AsyncDatabase

async def main():
    db = await AsyncDatabase.create()
    await db.execute("CREATE (:Person {name: 'Ada'})")
    result = await db.execute("MATCH (n:Person) RETURN n.name AS name")
    return result["rows"]

asyncio.run(main())
```

`AsyncDatabase` delegates to `asyncio.to_thread` so long queries
don't block the event loop. The surface is identical — switching is
a one-line import change.

## Common Patterns

### Bulk insert from a list

```python
rows = [{"id": i, "name": f"user-{i}"} for i in range(100)]

db.execute(
    "UNWIND $rows AS row CREATE (:User {id: row.id, name: row.name})",
    {"rows": rows},
)
```

See [`UNWIND`](../queries/unwind-merge#bulk-load-from-parameter).

### Typed helpers

```python
from lora_python import Database, date, duration, wgs84

db = Database.create()

db.execute(
    "CREATE (:Trip {when: $when, span: $span, origin: $origin})",
    {
        "when":   date("2026-05-01"),
        "span":   duration("PT90M"),
        "origin": wgs84(4.89, 52.37),
    },
)
```

Available helpers: `date`, `time`, `localtime`, `datetime`,
`localdatetime`, `duration`, `cartesian`, `cartesian_3d`, `wgs84`,
`wgs84_3d`.

### Repository pattern

```python
from lora_python import Database, LoraQueryError

class UserRepo:
    def __init__(self, db: Database):
        self._db = db

    def upsert(self, user_id: int, handle: str):
        self._db.execute(
            """
            MERGE (u:User {id: $id})
              ON CREATE SET u.created = timestamp()
              SET u.handle = $handle, u.updated = timestamp()
            """,
            {"id": user_id, "handle": handle},
        )

    def find_by_handle(self, handle: str):
        res = self._db.execute(
            "MATCH (u:User {handle: $handle}) RETURN u {.*} AS user",
            {"handle": handle},
        )
        rows = res["rows"]
        return rows[0]["user"] if rows else None
```

### Other methods

```python
db.clear()                        # drop all nodes + relationships
db.node_count                     # int — property, not a method
db.relationship_count             # int — property
```

`node_count` and `relationship_count` are read-only properties.
`AsyncDatabase` exposes `node_count()` and `relationship_count()` as
awaitables.

## Error Handling

| Class | When |
|---|---|
| `LoraError` | Base — catch if you don't need to distinguish |
| `LoraQueryError` | Parse / semantic / runtime query error |
| `InvalidParamsError` | A parameter couldn't be mapped to a `LoraValue` |

Engine-level causes live in [Troubleshooting](../troubleshooting).

## Performance / Best Practices

- **Thread-safety.** `Database` is safe to share across threads —
  the underlying mutex serialises access. No Python-level locking
  needed.
- **GIL.** `Database.execute` releases the GIL while Rust code
  runs, so other Python threads / asyncio tasks can progress. This
  is the real non-blocking mechanism — `AsyncDatabase` is a thin
  wrapper that uses it.
- **Integer precision.** Python integers are arbitrary precision,
  so `i64` values round-trip cleanly (unlike the JS bindings).
- **No cancellation.** Once a query is dispatched it runs to
  completion — bound traversals and `UNWIND` sizes.
- **Parameters, not f-strings.** Never interpolate user input into
  a query string.

## See also

- [**Ten-Minute Tour**](./tutorial) — guided walkthrough.
- [**Queries → Parameters**](../queries/#parameters) — binding typed values.
- [**Cookbook**](../cookbook) — scenario-based recipes.
- [**Data Types**](../data-types/overview) — Python ↔ engine mapping.
- [**Temporal Functions**](../functions/temporal) /
  [**Spatial Functions**](../functions/spatial) — helpers used above.
- [**Troubleshooting**](../troubleshooting).
