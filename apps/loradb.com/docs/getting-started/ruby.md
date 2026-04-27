---
title: Using LoraDB in Ruby
sidebar_label: Ruby
description: Install and use LoraDB in Ruby via the lora-ruby native extension built with Magnus and rb-sys — in-process execution with the same tagged value model as the other bindings.
---

# Using LoraDB in Ruby

## Overview

`lora-ruby` is a native extension built with
[Magnus](https://github.com/matsadler/magnus) on top of
[`rb-sys`](https://github.com/oxidize-rb/rb-sys). The engine runs
in-process — no separate server, no socket hop. Values follow the
same tagged model as the Node, Python, WASM, and Go bindings
(primitives pass through; nodes, relationships, paths, temporals,
and points come back as `Hash`es with a `"kind"` discriminator).

## Installation / Setup

### Requirements

- Ruby **3.1+**
- Rust toolchain (`rustup`) — only needed when no precompiled
  platform gem is available for your platform

### Install

```bash
gem install lora-ruby
# or in a Gemfile
gem "lora-ruby"
```

If a precompiled platform gem exists for your `{os, arch, ruby ABI}`
the install is a direct download; otherwise `gem install` falls
through to a source build via `cargo` + `rb-sys`.

## Creating a Client / Connection

```ruby
require "lora_ruby"

db = LoraRuby::Database.create
```

`LoraRuby::Database.create` and `LoraRuby::Database.new` are the same
constructor — both return a ready-to-use handle over an empty
in-memory graph.

## Running Your First Query

```ruby
require "lora_ruby"

db = LoraRuby::Database.create

db.execute("CREATE (:Person {name: 'Ada', born: 1815})")

result = db.execute("MATCH (p:Person) RETURN p.name AS name, p.born AS born")

puts result["rows"]
# [{"name"=>"Ada", "born"=>1815}]
```

## Examples

### Parameterised query

```ruby
result = db.execute(
  "MATCH (p:Person) WHERE p.name = $name RETURN p.name AS name",
  { name: "Ada" },
)
```

Params accept String or Symbol keys. Ruby values map automatically:
`nil` → `Null`, `true`/`false` → `Boolean`, `Integer` → `Integer`,
`Float` → `Float`, `String`/`Symbol` → `String`, `Array` → `List`,
`Hash` → `Map`. Use the tagged helpers for dates, durations, and
points — see [typed helpers](#typed-helpers) below.

### Structured result handling

```ruby
result = db.execute("MATCH (n:Person) RETURN n")

result["rows"].each do |row|
  n = row["n"]
  puts n["properties"]["name"] if LoraRuby.node?(n)
end
```

Available guards: `node?`, `relationship?`, `path?`, `point?`,
`temporal?` — re-exported on both `LoraRuby` and `LoraRuby::Types`.

### Typed helpers

```ruby
db.execute(
  "CREATE (:Trip {when: $when, span: $span, origin: $origin})",
  {
    when:   LoraRuby.datetime("2026-05-01T10:15:00Z"),
    span:   LoraRuby.duration("PT90M"),
    origin: LoraRuby.wgs84(4.89, 52.37),
  },
)
```

Available helpers: `date`, `time`, `localtime`, `datetime`,
`localdatetime`, `duration`, `cartesian`, `cartesian_3d`, `wgs84`,
`wgs84_3d`.

### Handle errors

```ruby
begin
  db.execute("BAD QUERY")
rescue LoraRuby::QueryError => e
  puts "query failed: #{e.message}"
rescue LoraRuby::InvalidParamsError => e
  puts "bad params: #{e.message}"
end
```

`LoraRuby::Error` is the common base class — rescue it if you don't
need to distinguish.

### Rack / Rails integration

```ruby
# config/initializers/lora.rb
require "lora_ruby"

LORA_DB = LoraRuby::Database.create

# app/controllers/users_controller.rb
def show
  res = LORA_DB.execute(
    "MATCH (u:User {id: $id}) RETURN u {.id, .handle, .tier} AS user",
    { id: params[:id].to_i },
  )
  return head :not_found if res["rows"].empty?
  render json: res["rows"].first["user"]
rescue LoraRuby::QueryError => e
  render json: { error: e.message }, status: :bad_request
end
```

### Persisting your graph

LoraDB can save the in-memory graph to a single file and restore it
later. Ruby now supports the same simple initialization rule as the
other filesystem-backed bindings:

- `LoraRuby::Database.create` / `LoraRuby::Database.new` => in-memory
- `LoraRuby::Database.create("app", {"database_dir": "./data"})` / `LoraRuby::Database.new("app", { database_dir: "./data" })` => persistent

```ruby
require 'lora_ruby'

db = LoraRuby::Database.new # in-memory
# db = LoraRuby::Database.new("app", { database_dir: "./data" }) # persistent: ./data/app.loradb
db.execute("CREATE (:Person {name: 'Ada'})")

# Save everything to disk.
meta = db.save_snapshot("graph.bin")
puts "#{meta['nodeCount']} nodes, #{meta['relationshipCount']} relationships"

# Restore into a fresh handle (in a new process, for example).
db = LoraRuby::Database.new
db.load_snapshot("graph.bin")
```

Both save and load serialise against every query on the handle. A
crash between saves loses every mutation since the last save. See
the
[Snapshots operator doc (internal)](https://github.com/lora-db/lora/blob/main/docs/operations/snapshots.md)
for the wire format and atomic-rename guarantees.

Passing a database name and directory opens or creates an archive-backed persistent
database at `<database_dir>/<name>.loradb`. Reopening the same path replays committed
writes before the handle is returned. This first Ruby persistence slice
intentionally stays small: the binding exposes archive-backed
initialization plus snapshots, but not checkpoint, truncate, status, or
sync-mode controls. Call `db.close` before reopening the same archive
directory inside one process.

## Common Patterns

### Bulk insert from a Ruby array

```ruby
rows = (1..100).map { |i| { id: i, name: "user-#{i}" } }

db.execute(
  "UNWIND $rows AS row CREATE (:User {id: row.id, name: row.name})",
  { rows: rows },
)
```

See [`UNWIND`](../queries/unwind-merge#bulk-load-from-parameter).

### Other methods

```ruby
db.clear                   # drop all nodes + relationships → nil
db.close                   # release the native handle
db.node_count              # Integer
db.relationship_count      # Integer
LoraRuby::VERSION          # gem version
```

## Error Handling

| Class | When |
|---|---|
| `LoraRuby::Error` | Base — rescue if you don't need to distinguish |
| `LoraRuby::QueryError` | Parse / analyze / execute failure |
| `LoraRuby::InvalidParamsError` | A parameter couldn't be mapped to a `LoraValue` |

Engine-level causes live in [Troubleshooting](../troubleshooting).

## Performance / Best Practices

- **GVL release.** `Database#execute` calls
  `rb_thread_call_without_gvl`, so other Ruby threads run while a
  query is in flight. Concurrent queries against the same
  `Database` serialise on an internal `Mutex`; parallel queries
  against **different** `Database` instances have no shared state.
- **Interrupts after current query.** The engine has no
  cancellation hook, so a thread interrupted mid-query
  (`Thread#kill`) will observe the interrupt **after** the current
  query finishes. Keep queries short if you rely on cooperative
  cancellation.
- **String keys on output.** Result Hashes always use string keys,
  matching the Node, Python, WASM, and Go bindings. Input Hashes
  accept either symbol or string keys.
- **Parameters, not string concatenation.** The only safe way to
  mix untrusted input into a query.

## See also

- [**Ten-Minute Tour**](./tutorial) — guided walkthrough.
- [**Queries → Parameters**](../queries/parameters) — binding typed values.
- [**Data Types**](../data-types/overview) — Ruby ↔ engine mapping.
- [**Binding README**](https://github.com/lora-db/lora/tree/main/crates/lora-ruby) — the source-of-truth install and build guide.
- [**Troubleshooting**](../troubleshooting).
