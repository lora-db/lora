---
title: Using LoraDB in Go
sidebar_label: Go
description: Install and use LoraDB in Go via the lora-go cgo wrapper over the shared lora-ffi C ABI — in-process execution, snapshots, and WAL persistence.
---

# Using LoraDB in Go

## Overview

`lora-go` is a thin cgo wrapper over the shared [`lora-ffi`](https://github.com/lora-db/lora/tree/main/crates/lora-ffi)
C ABI. The engine runs in-process — no separate server, no socket
hop. Values follow the same tagged model as the Node, Python, WASM,
and Ruby bindings (primitives pass through; nodes, relationships,
paths, temporals, and points come back as `map[string]any` with a
`"kind"` discriminator).

## Installation / Setup

### Requirements

- Go **1.21+**
- A C toolchain with cgo enabled (`clang` / `gcc`)
- The `liblora_ffi` static library on disk (built locally with
  `cargo build --release -p lora-ffi`, or downloaded from a tagged
  GitHub Release as `lora-ffi-vX.Y.Z-<triple>.tar.gz`)

### Install

```bash
go get github.com/lora-db/lora/crates/lora-go
```

Because the binding links against the Rust engine, `go build` needs
`liblora_ffi.a` on disk before it runs. The simplest path is to
clone the workspace and build the FFI in-tree:

```bash
git clone https://github.com/lora-db/lora
cd lora
cargo build --release -p lora-ffi    # produces target/release/liblora_ffi.a
cd crates/lora-go
go test -race ./...
```

The default `#cgo LDFLAGS` in `lora.go` resolves to
`${SRCDIR}/../../target/release/liblora_ffi.a` — the right path in
the workspace layout.

For consumer projects outside the repo, build `lora-ffi` once and
override the cgo flags in the environment:

```bash
export CGO_CFLAGS="-I$PWD/lora/crates/lora-go/include"
export CGO_LDFLAGS="-L$PWD/lora/target/release -llora_ffi -lm -ldl -lpthread"
go build ./...
```

See [`crates/lora-go/README.md`](https://github.com/lora-db/lora/tree/main/crates/lora-go)
for the full build-from-release-archive flow.

## Creating a Client / Connection

```go
import lora "github.com/lora-db/lora/crates/lora-go"

db, err := lora.New()
if err != nil { log.Fatal(err) }
defer db.Close()
```

`lora.New()` and `lora.NewDatabase()` are the same constructor —
both return a ready-to-use handle over an empty in-memory graph.

## Running Your First Query

```go
package main

import (
    "fmt"
    "log"

    lora "github.com/lora-db/lora/crates/lora-go"
)

func main() {
    db, err := lora.New()
    if err != nil { log.Fatal(err) }
    defer db.Close()

    if _, err := db.Execute(
        "CREATE (:Person {name: 'Ada', born: 1815})",
        nil,
    ); err != nil { log.Fatal(err) }

    r, err := db.Execute(
        "MATCH (p:Person) RETURN p.name AS name, p.born AS born",
        nil,
    )
    if err != nil { log.Fatal(err) }

    fmt.Println(r.Columns, r.Rows)
    // [name born] [map[name:Ada born:1815]]
}
```

## Examples

### Parameterised query

```go
r, err := db.Execute(
    "MATCH (p:Person) WHERE p.name = $name RETURN p.name AS name",
    lora.Params{"name": "Ada"},
)
```

Go values map automatically: `int`/`int64` → `Integer`,
`float64` → `Float`, `string` → `String`, `bool` → `Boolean`,
`nil` → `Null`, `[]any` → `List`, `map[string]any` → `Map`. Use the
tagged helpers for dates, durations, and points — see
[typed helpers](#typed-helpers) below.

### Structured result handling

```go
r, err := db.Execute("MATCH (n:Person) RETURN n", nil)
if err != nil { log.Fatal(err) }

for _, row := range r.Rows {
    if lora.IsNode(row["n"]) {
        n := row["n"].(map[string]any)
        fmt.Println(n["id"], n["labels"], n["properties"])
    }
}
```

Available guards: `IsNode`, `IsRelationship`, `IsPath`, `IsPoint`,
`IsTemporal`.

### Context cancellation (important caveat)

```go
ctx, cancel := context.WithTimeout(ctx, 500*time.Millisecond)
defer cancel()

r, err := db.ExecuteContext(ctx, "MATCH (n) RETURN count(n)", nil)
```

`ExecuteContext` honours `context.Context` deadlines on the Go side
— the call returns `ctx.Err()` as soon as the context fires. But
the engine does **not** yet support mid-query cancellation, so the
native call keeps running in a helper goroutine and holds the
database's internal mutex until it finishes. Any follow-up call
that needs the mutex blocks until then.

If you rely on a hard deadline, either keep queries small enough
that their worst-case latency is acceptable even if they can't be
interrupted, or guard the database with a higher-level rate-limiter.

### Typed helpers

```go
db.Execute(
    "CREATE (:Trip {when: $when, span: $span, origin: $origin})",
    lora.Params{
        "when":   lora.DateTime("2026-05-01T10:15:00Z"),
        "span":   lora.Duration("PT90M"),
        "origin": lora.WGS84(4.89, 52.37),
    },
)
```

Available helpers: `Date`, `Time`, `LocalTime`, `DateTime`,
`LocalDateTime`, `Duration`, `Cartesian`, `Cartesian3D`, `WGS84`,
`WGS84_3D`.

### Handle errors

```go
if err != nil {
    var lerr *lora.LoraError
    if errors.As(err, &lerr) {
        switch lerr.Code {
        case lora.CodeInvalidParams:
            // bad params
        case lora.CodeLoraError:
            // parse / analyze / execute failure
        }
    }
}
```

### Persisting your graph

LoraDB can save the in-memory graph to a single file and restore it
later. Go has three persistence shapes:

- `lora.New()` / `lora.NewDatabase()` => in-memory
- `lora.New("app", lora.Options{DatabaseDir: "./data"})` / `lora.NewDatabase("app", lora.Options{DatabaseDir: "./data"})` => archive-backed
- `lora.OpenWal(lora.WalOptions{WalDir: "./data/wal", SnapshotDir: "./data/snapshots"})` => explicit WAL with optional managed snapshots

```go
import lora "github.com/lora-db/lora/crates/lora-go"

db, err := lora.New() // in-memory
// db, err := lora.New("app", lora.Options{DatabaseDir: "./data"}) // archive: ./data/app.loradb
if err != nil { log.Fatal(err) }
defer db.Close()

if _, err := db.Execute("CREATE (:Person {name: 'Ada'})", nil); err != nil {
    log.Fatal(err)
}

meta, err := db.SaveSnapshot("graph.bin")
if err != nil { log.Fatal(err) }
fmt.Printf("nodes=%d rels=%d\n", meta.NodeCount, meta.RelationshipCount)

db2, err := lora.New()
if err != nil { log.Fatal(err) }
defer db2.Close()

if _, err := db2.LoadSnapshot("graph.bin"); err != nil {
    log.Fatal(err)
}

durable, err := lora.OpenWal(lora.WalOptions{
    WalDir:               "./data/wal",
    SnapshotDir:          "./data/snapshots",
    SnapshotEveryCommits: 1000,
    SnapshotKeepOld:      2,
    SnapshotOptions: &lora.SnapshotOptions{
        Compression: &lora.SnapshotCompression{Format: "gzip", Level: 1},
    },
})
if err != nil { log.Fatal(err) }
defer durable.Close()
```

`SnapshotMeta.WalLsn` is a `*uint64`; it is `nil` for a pure snapshot
and non-`nil` when you load or save a checkpoint snapshot written by a
WAL-enabled deployment. Both save and load hold
the store mutex for the duration of the call — concurrent
`Execute` calls block until the snapshot operation finishes. A crash
between saves loses every mutation since the last save.

Passing a database name and directory opens or creates an archive-backed persistent
database at `<databaseDir>/<name>.loradb`. Reopening the same path replays committed
writes before the handle is returned. `OpenWal` opens a raw WAL
directory; when `SnapshotDir` and `SnapshotEveryCommits` are set, the
database writes managed checkpoint snapshots after that many committed
transactions. Go does not expose WAL status, truncate, or sync-mode
controls; use Rust or `lora-server` for those operator knobs.

If you run `lora-server` alongside a Go client, you can also drive the
admin surface as an ordinary HTTP request — see
[`lora-server` → Snapshots, WAL, and restore](./server#snapshots-wal-and-restore)
and [`POST /admin/snapshot/save`](../api/http#admin-endpoints-opt-in).

See the canonical [Snapshots guide](../snapshot) for the full metadata
shape, atomic-rename guarantees, and boundaries, and
[WAL and checkpoints](../wal) for the recovery model.

## Common Patterns

### Bulk insert from a Go slice

```go
rows := make([]any, 0, 100)
for i := 0; i < 100; i++ {
    rows = append(rows, map[string]any{"id": i, "name": fmt.Sprintf("user-%d", i)})
}

db.Execute(
    "UNWIND $rows AS row CREATE (:User {id: row.id, name: row.name})",
    lora.Params{"rows": rows},
)
```

See [`UNWIND`](../queries/unwind-merge#bulk-load-from-parameter).

### Other methods

```go
db.Clear()                   // drop all nodes + relationships
db.NodeCount()               // int64, error
db.RelationshipCount()       // int64, error
db.Version()                 // module / engine version string
```

## Error Handling

| Code | When |
|---|---|
| `LORA_ERROR` | Parse / analyze / execute failure |
| `INVALID_PARAMS` | A parameter value couldn't be mapped |
| `PANIC` | The engine panicked; the FFI caught it and surfaced the message |
| `UNKNOWN` | Catch-all for messages without a recognised prefix |

Engine-level causes live in [Troubleshooting](../troubleshooting).

## Performance / Best Practices

- **Platform support.** Linux and macOS (x86_64, arm64). Windows is
  not yet supported — revisit once a Windows Go target ships.
- **One mutex per `Database`.** Parallel `Execute` calls on the same
  handle serialise on the engine mutex. For read parallelism, spin
  up multiple `Database` instances (each with its own graph).
- **No cancellation.** `ExecuteContext` returns the context error
  immediately but the native call keeps running. See
  [the caveat above](#context-cancellation-important-caveat).
- **Parameters, not string concatenation.** The only safe way to
  mix untrusted input into a query.

## See also

- [**Ten-Minute Tour**](./tutorial) — guided walkthrough.
- [**Queries → Parameters**](../queries/parameters) — binding typed values.
- [**Data Types**](../data-types/overview) — Go ↔ engine mapping.
- [**Binding README**](https://github.com/lora-db/lora/tree/main/crates/lora-go) — the source-of-truth install and build guide.
- [**Troubleshooting**](../troubleshooting).
