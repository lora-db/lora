# lora-ruby

Ruby bindings for the [Lora](../../README.md) graph engine.
Ships a native extension built with [Magnus](https://github.com/matsadler/magnus)
on top of [`rb-sys`](https://github.com/oxidize-rb/rb-sys) so the Rust
engine runs in-process — no separate server, no socket hop.

> **Status:** prototype / feasibility check. Published source gem on
> RubyGems; precompiled platform gems are built for the supported
> targets (see "Release" below).

## Install

```bash
gem install lora-ruby
# or in a Gemfile
gem "lora-ruby"
```

`require "lora_ruby"` loads the native extension from
`lib/lora_ruby/lora_ruby.{so,bundle,dll}`. If a precompiled gem for
your platform exists on RubyGems, the install is a direct download; if
not, the source gem is built locally with `cargo` and a stable Rust
toolchain (1.87+).

## Usage

```ruby
require "lora_ruby"

db = LoraRuby::Database.create

db.execute("CREATE (:Person {name: $n, age: $a})", { n: "Alice", a: 30 })

result = db.execute("MATCH (n:Person) RETURN n")
result["rows"].each do |row|
  n = row["n"]
  puts n["properties"]["name"] if LoraRuby.node?(n)
end
```

Initialization rule:

```ruby
scratch = LoraRuby::Database.create         # in-memory
persistent = LoraRuby::Database.create("app", {"database_dir": "./data"}) # persistent: ./data/app.loradb
```

If you want persistence, pass a database name and `database_dir` to
`LoraRuby::Database.create(...)` or `LoraRuby::Database.new(...)`.

### Params

`execute` accepts a second argument — either `nil` or a `Hash` keyed by
parameter name (`String` or `Symbol`). Values can be any of:

- `nil`, `true`, `false`, `Integer`, `Float`, `String`, `Symbol` (stringified)
- `Array` of the above (recursive)
- `Hash` keyed by `String`/`Symbol` with the above values (recursive)
- Tagged temporal/spatial Hashes produced by the constructors below

## Module shape — why `LoraRuby::Database`?

Three reasonable options were considered:

- `LoraRuby::Database` — matches the gem filename; scope is obvious.
- `LoraDB::Database` — matches the brand (loradb.com).
- `Lora::Database` — shortest, but collides with arbitrary "lora" apps.

We went with **`LoraRuby::Database`** for symmetry with the Python
binding's `lora_python.Database` and because it mirrors the
`require "lora_ruby"` path exactly. The gem name on RubyGems is
`lora-ruby` (hyphen); the Ruby constant follows Rubocop convention
(CamelCase, no hyphen).

## Public API

```ruby
LoraRuby::Database.create(database_name = nil, options = nil)  # -> Database
LoraRuby::Database.new(database_name = nil, options = nil)     # -> Database
LoraRuby::Database.open_wal(wal_dir, options = nil)            # -> Database

db.execute(query, params = nil)       # -> { "columns" => [...], "rows" => [...] }
db.clear                              # -> nil
db.node_count                         # -> Integer
db.relationship_count                 # -> Integer

LoraRuby::VERSION                    # gem version
```

Result shape:

```ruby
{
  "columns" => ["name"],
  "rows"    => [{ "name" => "Alice" }],
}
```

Hash keys on the output are always **strings**, matching the `lora-node`,
`lora-wasm`, and `lora-python` bindings. Input Hashes accept either
symbol or string keys — both work for param names and for tagged
constructor Hashes like `point`/`date`/...

## Typed value model

Identical contract to the other bindings:

| Ruby shape                                                              | Lora value      |
|-------------------------------------------------------------------------|-------------------|
| `nil`, `true`/`false`, `Integer`, `Float`, `String`                      | scalars           |
| `Array`, `Hash`                                                          | collections       |
| `{"kind" => "node", "id", "labels", "properties"}`                       | node              |
| `{"kind" => "relationship", "id", …}`                                    | relationship      |
| `{"kind" => "path", "nodes" => [...], "rels" => [...]}`                  | path              |
| `{"kind" => "date", "iso" => "YYYY-MM-DD"}` (and `time`, …)              | temporal          |
| point Hashes (below)                                                    | point             |

Points are returned as Hashes keyed on their CRS:

| SRID | Hash                                                                                                       |
|------|------------------------------------------------------------------------------------------------------------|
| 7203 | `{"kind"=>"point","srid"=>7203,"crs"=>"cartesian","x","y"}`                                                |
| 9157 | `{"kind"=>"point","srid"=>9157,"crs"=>"cartesian-3D","x","y","z"}`                                         |
| 4326 | `{"kind"=>"point","srid"=>4326,"crs"=>"WGS-84-2D","x","y","longitude","latitude"}`                         |
| 4979 | `{"kind"=>"point","srid"=>4979,"crs"=>"WGS-84-3D","x","y","z","longitude","latitude","height"}`            |

### Constructors and guards

Re-exported on both `LoraRuby` and `LoraRuby::Types`:

- Constructors: `date`, `time`, `localtime`, `datetime`,
  `localdatetime`, `duration`, `cartesian`, `cartesian_3d`, `wgs84`,
  `wgs84_3d`.
- Guards: `node?`, `relationship?`, `path?`, `point?`, `temporal?`.

```ruby
db.execute(
  "CREATE (:Event {on: $d, at: $c})",
  { d: LoraRuby.date("2025-03-14"), c: LoraRuby.cartesian(1.5, 2.5) },
)
```

## Errors

- `LoraRuby::Error` — base class (extends `StandardError`).
- `LoraRuby::QueryError` — parse / analyze / execute failure.
- `LoraRuby::InvalidParamsError` — a parameter value couldn't be mapped.

## Persistence

`LoraRuby::Database.create("app", {"database_dir": "./data"})` and
`LoraRuby::Database.new("app", { database_dir: "./data" })` open or create
an archive-backed persistent database at `./data/app.loradb`. Reopening the same path
replays committed writes before returning the handle.

Call `db.close` before reopening the same archive inside one
process.

For explicit WAL directories with managed snapshots, use `open_wal`:

```ruby
db = LoraRuby::Database.open_wal(
  "./data/wal",
  snapshot_dir: "./data/snapshots",
  snapshot_every_commits: 1000,
  snapshot_keep_old: 2,
)
```

`snapshot_options` accepts the same compression/encryption options as
`save_snapshot`.

## Concurrency (GVL release)

`Database#execute` calls `rb_thread_call_without_gvl`, so other Ruby
threads run while the engine is busy. Concurrent queries against the
same `Database` serialise on an internal `Mutex`; parallel queries
against **different** `Database` instances have no shared state.

The engine has no cancellation hook, so we pass a `NULL` unblock
function. A thread interrupted mid-query (`Thread#kill`) will observe
the interrupt **after** the current query finishes. Keep queries short
if you rely on cooperative cancellation.

## Local development

```bash
cd crates/lora-ruby
bundle install
bundle exec rake compile        # cargo build → lib/lora_ruby/lora_ruby.<ext>
bundle exec rake test           # minitest
bundle exec rake build          # pkg/lora-ruby-<version>.gem
```

`rake compile` drives `cargo` through `rb_sys/extensiontask`; it is
what `gem install` runs on end-user machines that don't have a
precompiled platform gem.

## Architecture

```
lora-database (Rust, embedded)
   └── crates/lora-ruby/                (gem root + cargo crate)
          ├── Cargo.toml                 Rust workspace member
          ├── extconf.rb                 rb-sys / mkmf entry point
          ├── src/lib.rs                 <- Magnus / rb-sys bindings
          └── lib/lora_ruby/
                 ├── lora_ruby.<ext>      (native, built by rake compile)
                 ├── types.rb             tagged-dict constructors + guards
                 └── version.rb           gem version
```

rb-sys' convention keeps `Cargo.toml` and `extconf.rb` side by side so
the cargo manifest directory IS the gem root. That makes the crate
a first-class Cargo workspace member (shared `Cargo.lock`,
`target/`, `cargo check --workspace` coverage) without a nested
`ext/<name>/` directory.

## Release

Source gem is always built. Precompiled platform gems are emitted via
`rb_sys/cross` for `{x86_64,aarch64}-linux`, `{x86_64,arm64}-darwin`, and
`x64-mingw-ucrt`. See `.github/workflows/packages-release.yml` (Ruby
section) and `RELEASING.md`.
