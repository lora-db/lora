# `github.com/lora-db/lora/crates/lora-go`

Go bindings for [LoraDB](https://github.com/lora-db/lora) — an
embeddable graph database with Cypher. The binding is a
thin cgo layer over `crates/lora-ffi` (a C ABI around
`lora-database`) and ships the same typed value model as the
`lora-node`, `lora-wasm`, and `lora-python` bindings.

## Install

```bash
go get github.com/lora-db/lora/crates/lora-go
```

Because the binding links against the Rust engine, **building requires
the `liblora_ffi.a` static library to exist on disk before `go build`
runs.** There are two supported deployment models:

**1. Repo checkout (the default path the tests and `make` assume).**
Clone the LoraDB repo, build the FFI, then `go build` / `go test`
against the checked-out module:

```bash
git clone https://github.com/lora-db/lora
cd lora
cargo build --release -p lora-ffi    # produces target/release/liblora_ffi.a
cd crates/lora-go
go test -race ./...
```

The default `#cgo LDFLAGS` in `lora.go` resolves to
`${SRCDIR}/../../target/release/liblora_ffi.a`, which is the right
path in this layout.

**2. Consumer project (`go get` from outside the repo).** The Go
module cache lives under `$GOPATH/pkg/mod/`, which does **not**
contain `target/release/`, so the default `-L` path is wrong. Build
the FFI once somewhere you control, then override the cgo flags in
the environment when building your consumer project:

```bash
# one-off build of the FFI
git clone --depth=1 --branch vX.Y.Z https://github.com/lora-db/lora
(cd lora && cargo build --release -p lora-ffi)

# in your project
export CGO_CFLAGS="-I$PWD/lora/crates/lora-go/include"
export CGO_LDFLAGS="-L$PWD/lora/target/release -llora_ffi -lm -ldl -lpthread"
go build ./...
```

Or download a prebuilt archive from the GitHub Release (`lora-ffi-vX.Y.Z-<triple>.tar.gz`,
produced by `packages-release.yml`) and point `CGO_LDFLAGS` at its
extracted directory instead.

## Quick start

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
        "CREATE (:Person {name: $n, born: $d})",
        lora.Params{"n": "Alice", "d": lora.Date("1990-01-15")},
    ); err != nil { log.Fatal(err) }

    r, err := db.Execute(
        "MATCH (p:Person) RETURN p.name AS name, p.born AS born",
        nil,
    )
    if err != nil { log.Fatal(err) }

    fmt.Println(r.Columns, r.Rows)
}
```

Run the canned example:

```bash
make example   # builds lora-ffi, then runs `go run ./examples/basic`
```

Initialization rule:

```go
db, err := lora.New()        // in-memory
db, err := lora.New("./app") // persistent: directory string
```

If you want persistence, pass one directory string to `New(...)` or
`NewDatabase(...)`.

## Value model

Every value follows the **shared tagged model** used by the other
LoraDB bindings. Primitives come back as Go natives; structured,
temporal, and spatial values come back as `map[string]any` with a
`"kind"` discriminator.

| Cypher / engine value | Go representation                                    |
| --------------------- | ---------------------------------------------------- |
| `null`                | `nil`                                                |
| boolean               | `bool`                                               |
| integer               | `int64`                                              |
| float                 | `float64`                                            |
| string                | `string`                                             |
| list                  | `[]any`                                              |
| map                   | `map[string]any`                                     |
| node                  | `{"kind":"node","id":…,"labels":[…],"properties":…}` |
| relationship          | `{"kind":"relationship","id":…}`                     |
| path                  | `{"kind":"path","nodes":[…],"rels":[…]}`             |
| date                  | `{"kind":"date","iso":"YYYY-MM-DD"}`                 |
| time / localtime      | `{"kind":"time","iso":…}` / `{"kind":"localtime",…}` |
| datetime / local      | `{"kind":"datetime",…}` / `{"kind":"localdatetime",…}` |
| duration              | `{"kind":"duration","iso":"P1Y2M3DT4H5M6S"}`         |
| point (all SRIDs)     | `{"kind":"point","srid":…,"crs":…,"x":…,"y":…,…}`    |

Build typed params with the constructors in `helpers.go`:

```go
lora.Params{
    "when":  lora.DateTime("2025-04-22T10:15:00Z"),
    "for":   lora.Duration("PT90M"),
    "where": lora.WGS84(4.9, 52.37),
}
```

Narrow returned values with the guards:

```go
if lora.IsNode(row["n"]) {
    node := row["n"].(map[string]any)
    // use node["id"], node["labels"], node["properties"] …
}
```

## Context cancellation (important caveat)

`ExecuteContext` honours `context.Context` deadlines in the Go sense
— the function returns `ctx.Err()` as soon as the context fires. But
this binding does not pass a deadline into Rust, so the native call
continues running in a helper goroutine and will release its Rust-side
store lock only once it completes. Any follow-up call that needs that
lock blocks until then.

If you rely on a hard deadline, either (a) ensure the query is small
enough that the worst-case latency is acceptable even if it can't be
interrupted, or (b) guard the database with a per-call timeout at a
higher layer (e.g. a rate-limited queue).

## Errors

Every method returns a `*LoraError` on failure. The `Code` field is
one of:

- `LORA_ERROR` — parse / analyze / execute failure
- `INVALID_PARAMS` — a parameter value could not be mapped
- `PANIC` — the engine panicked; the FFI caught it and surfaced the
  message
- `UNKNOWN` — catch-all for messages without a recognised prefix

```go
if err != nil {
    var lerr *lora.LoraError
    if errors.As(err, &lerr) && lerr.Code == lora.CodeInvalidParams {
        // …
    }
}
```

## Persistence

`lora.New("./app")` and `lora.NewDatabase("./app")` open or create a
WAL-backed persistent database rooted at that directory. Reopening the
same path replays committed writes before returning the handle.

This first Go persistence slice intentionally stays small: the binding
exposes WAL-backed initialization plus the existing snapshot APIs, but
not checkpoint, truncate, status, or sync-mode controls.

## Platform support

- Linux (x86_64, arm64) — supported
- macOS (x86_64, arm64) — supported
- Windows — not supported in v0.1; revisit once the other bindings
  ship a Windows Go target.
- FreeBSD / other — not tested.

## Building locally

```bash
# From the workspace root — produces ../../target/release/liblora_ffi.a
cargo build --release -p lora-ffi

# From this directory — runs the Go test suite against the engine
cd crates/lora-go
go test -race ./...
```

Or just `make test` from this directory, which chains the `cargo`
build into a `go vet` + `go test -race`.

## Versioning

Go modules derive their version from the git tag; there is no
hand-edited `version` field. `lora.Version()` returns the module
version when the binding is consumed via `go get`, and falls back to
the `lora-ffi` crate version (from `lora_version()`) for source
builds.

Releases are driven by the `vX.Y.Z` tags that already drive the other
LoraDB bindings — see `RELEASING.md` at the repo root for the full
release flow.

## Contributing

This package follows the same contribution rules as the rest of the
repository — see `CONTRIBUTING.md` at the repo root. In particular:
run `gofmt`, `go vet ./...`, and `go test -race ./...` before
submitting a PR; the `lora-go` CI workflow enforces the same.

## License

BUSL-1.1 — see `LICENSE` at the repo root.
