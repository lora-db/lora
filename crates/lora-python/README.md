# lora-python

Python bindings for the [Lora](../../README.md) graph
engine. Ships both a synchronous PyO3 `Database` class and an
asyncio-compatible `AsyncDatabase` wrapper that never blocks the event loop.

> **Status:** prototype / feasibility check. Not published to PyPI.

## Install (local dev)

```bash
cd crates/lora-python
python3 -m venv .venv && source .venv/bin/activate
pip install -U pip maturin pytest pytest-asyncio
maturin develop         # builds the Rust extension into the venv
pytest                  # runs the sync + async smoke tests
```

`maturin develop` produces a `lora_python/_native.<platform>.so` inside the
package and makes `import lora_python` work immediately.

## Sync usage

```python
from lora_python import Database, is_node

db = Database.create()
db.execute("CREATE (:Person {name: $n, age: $a})", {"n": "Alice", "a": 30})

res = db.execute("MATCH (n:Person) RETURN n")
for row in res["rows"]:
    n = row["n"]
    if is_node(n):
        print(n["properties"]["name"])
```

Initialization rule:

```python
from lora_python import Database

scratch = Database.create()            # in-memory
persistent = Database.create("app", {"database_dir": "./data"})  # persistent: ./data/app.loradb
```

If you want persistence, pass a database name and `database_dir` to
`Database.create(...)` or `Database(...)`.

## Async usage (non-blocking)

```python
import asyncio
from lora_python import AsyncDatabase

async def main():
    db = await AsyncDatabase.create()
    await db.execute("CREATE (:Person {name: 'Alice'})")
    r = await db.execute("MATCH (n:Person) RETURN n.name AS name")
    print(r["rows"])

asyncio.run(main())
```

Async initialization follows the same rule:

```python
db = await AsyncDatabase.create()            # in-memory
db = await AsyncDatabase.create("app", {"database_dir": "./data"})  # persistent: ./data/app.loradb
```

`AsyncDatabase.execute` dispatches the query onto the default asyncio
thread pool via `asyncio.to_thread`. The PyO3 `Database.execute` releases
the Python GIL for the duration of engine work, so other coroutines on the
event loop can progress while a query runs. A dedicated test proves the
event loop continues ticking during a 2 000-node `MATCH`.

## Typed value model

Same conceptual contract as `lora-node` / `lora-wasm`:

| Python shape                                               | Lora value         |
|------------------------------------------------------------|----------------------|
| `None`, `bool`, `int`, `float`, `str`                      | scalars              |
| `list`, `dict`                                             | collections          |
| `{"kind": "node", "id", "labels", "properties"}`           | node                 |
| `{"kind": "relationship", "id", …}`                        | relationship         |
| `{"kind": "path", "nodes": [...], "rels": [...]}`          | path                 |
| `{"kind": "date", "iso": "YYYY-MM-DD"}` (and `time`, …)    | temporal             |
| point dicts — see below                                    | point                |

Points are returned as dicts keyed on their CRS:

| SRID | Dict                                                                                               |
|------|----------------------------------------------------------------------------------------------------|
| 7203 | `{"kind": "point", "srid": 7203, "crs": "cartesian", "x", "y"}`                                    |
| 9157 | `{"kind": "point", "srid": 9157, "crs": "cartesian-3D", "x", "y", "z"}`                            |
| 4326 | `{"kind": "point", "srid": 4326, "crs": "WGS-84-2D", "x", "y", "longitude", "latitude"}`           |
| 4979 | `{"kind": "point", "srid": 4979, "crs": "WGS-84-3D", "x", "y", "z", "longitude", "latitude", "height"}` |

Constructors and guards are exported from `lora_python.types`:
`date`, `time`, `localtime`, `datetime`, `localdatetime`, `duration`,
`cartesian`, `cartesian_3d`, `wgs84`, `wgs84_3d`, `is_node`,
`is_relationship`, `is_path`, `is_point`, `is_temporal`.

> `distance()` on WGS-84-3D points ignores `height` — see
> [functions reference](../../apps/loradb.com/docs/functions/overview.md) for the full spatial
> reference and known limitations.

## Errors

- `LoraError` — base class
- `LoraQueryError` — parse / analyze / execute failure
- `InvalidParamsError` — a parameter value couldn't be mapped

All three are available as `lora_python.LoraError`, etc.

## Persistence

`Database.create("app", {"database_dir": "./data"})`, `Database("app", {"database_dir": "./data"})`, and
`await AsyncDatabase.create("app", {"database_dir": "./data"})` open or create
an archive-backed persistent database at `./data/app.loradb`. Reopening the same path
replays committed writes before returning the handle.

Call `db.close()` / `await db.close()` before reopening the same archive
inside one process.

This first Python persistence slice intentionally stays small: the
binding exposes archive-backed initialization plus the existing
`save_snapshot` / `load_snapshot` APIs, but not checkpoint, truncate,
status, or sync-mode controls.

Snapshots accept the same broad shapes in sync and async APIs:

```python
import io
from lora_python import Database

db = Database.create()
db.execute("CREATE (:Person {name: 'Alice'})")

meta = db.save_snapshot("./graph.lorasnap")   # path / PathLike
raw = db.save_snapshot("binary")              # bytes
text = db.save_snapshot("base64")             # base64 str
buf = io.BytesIO()
meta = db.save_snapshot(buf)                  # binary writer

db.load_snapshot("./graph.lorasnap")
db.load_snapshot(raw)
db.load_snapshot(io.BytesIO(buf.getvalue()))
db.load_snapshot(text, format="base64")
```

## Architecture

```
lora-database (Rust)
   └── lora-python (crate, cdylib)             <- PyO3 bindings
          ├── Database (sync, releases the GIL)
          └── python/lora_python/
                 ├── _async.py  AsyncDatabase via asyncio.to_thread
                 └── types.py   typed dicts + constructors + guards
```
