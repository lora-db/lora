---
title: Error reference
sidebar_label: Error reference
description: Stable error codes emitted by LoraDB across every binding and the HTTP server. Match on these codes — not on message text — to route failures in your application.
---

# Error reference

Every LoraDB error carries a stable `LORA_*` code drawn from a single
catalog. The code is part of the public API and never changes between
releases; the human-readable message is allowed to evolve. Bindings,
the HTTP server, and integration tests should match on the code, never
on the message.

## Codes

### Client errors (caller's fault)

| Code | When you'll see it |
|---|---|
| `LORA_PARSE` | Cypher syntax could not be parsed. |
| `LORA_SEMANTIC` | Unknown variable, label, function, or type mismatch. |
| `LORA_INVALID_PARAMS` | A parameter value couldn't be coerced into a Lora value. |
| `LORA_READ_ONLY` | A mutating statement was issued in a read-only context. |
| `LORA_NOT_FOUND` | A named entity (database, label, key) does not exist. |
| `LORA_CONSTRAINT` | A precondition failed (e.g. `DELETE` on a node with relationships — use `DETACH DELETE`). |
| `LORA_INVALID_VECTOR` | A vector value failed dimension or coordinate-type validation. |
| `LORA_TIMEOUT` | The query exceeded its cooperative deadline. |
| `LORA_DATABASE_NAME` | A logical database name violates the portable-path rules. |
| `LORA_CONFIG` | A CLI flag or configuration value is invalid. |

### Server errors (engine's fault)

| Code | When you'll see it |
|---|---|
| `LORA_IO` | An I/O failure outside the WAL / snapshot subsystems. |
| `LORA_WAL_CORRUPTION` | A WAL record was truncated, mis-CRC'd, or otherwise unreadable. |
| `LORA_WAL_POISONED` | The WAL is poisoned and no longer accepts durable writes. |
| `LORA_SNAPSHOT_CODEC` | Snapshot bad magic, version, or checksum. |
| `LORA_SNAPSHOT_CRYPTO` | Snapshot encryption / decryption / KDF failure. |
| `LORA_INTERNAL` | The engine could not classify the failure. |

### Binding-side codes

| Code | When you'll see it |
|---|---|
| `LORA_PANIC` | A Rust panic was caught at a binding boundary. The process keeps running; the call returns this code instead. |

## HTTP transport

The Axum server returns errors as JSON:

```json
{
  "error": {
    "code": "LORA_PARSE",
    "message": "parse error at 0..10: expected statement",
    "category": "client"
  }
}
```

`category` is `"client"` (4xx) or `"server"` (5xx). The HTTP status
code follows the category, with two refinements: `LORA_TIMEOUT` ⇒
`408 Request Timeout`, `LORA_NOT_FOUND` ⇒ `404 Not Found`. Everything
else in the client category is `400 Bad Request`; everything in the
server category is `500 Internal Server Error`.

## Examples per language

### Rust

```rust
use lora_database::{Database, LoraError, LoraErrorCode};

let db = Database::in_memory();
match db.execute("NOT CYPHER", None) {
    Ok(_) => {}
    Err(e) => {
        let lora = LoraError::from_anyhow(e);
        if lora.code() == LoraErrorCode::Parse {
            eprintln!("syntax: {}", lora.message());
        }
    }
}
```

### Node.js / TypeScript

```ts
import { LoraError } from "@loradb/loradb";

try {
  await db.execute("NOT CYPHER");
} catch (err) {
  if (err instanceof LoraError) {
    if (err.engineCode === "LORA_PARSE") console.error("syntax:", err.message);
    if (!err.isClient()) reportToMonitoring(err);
  }
}
```

`err.code` continues to be the legacy umbrella code (`"LORA_ERROR"` or
`"INVALID_PARAMS"`) for backwards compatibility; `err.engineCode` is
the precise wire string from the catalog above.

### Python

```python
from lora_python import LoraQueryError

try:
    db.execute("NOT CYPHER")
except LoraQueryError as e:
    code, _, message = str(e).partition(": ")
    if code == "LORA_PARSE":
        print(f"syntax: {message}")
```

### Go

```go
import "github.com/lora-db/lora/crates/bindings/lora-go"

if _, err := db.Execute("NOT CYPHER", nil); err != nil {
    var le *lora.LoraError
    if errors.As(err, &le) && le.Code == lora.CodeParse {
        fmt.Println("syntax:", le.Message)
    }
}
```

### Ruby

```ruby
begin
  db.execute("NOT CYPHER")
rescue LoraRuby::QueryError => e
  code, _, message = e.message.partition(": ")
  warn "syntax: #{message}" if code == "LORA_PARSE"
end
```

### WASM / browsers

```js
try {
  db.execute("NOT CYPHER", null);
} catch (err) {
  // Message format: "<CODE>: <human text>"
  const [code] = err.message.split(": ", 1);
  if (code === "LORA_PARSE") console.warn("syntax", err.message);
}
```

## Why match on codes, not messages

Codes are the contract. Messages are advisory: between minor releases
we rewrite individual `#[error("...")]` strings to make them clearer or
include more context. Anything matching message text — `if
err.message.contains("not found")` — will silently break. Match on
`LORA_NOT_FOUND` instead.
