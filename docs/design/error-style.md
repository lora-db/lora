# Error message style guide

This guide governs every `#[error("...")]` string in the workspace and every
manual `Display` impl on an error type. The goal is that a user reading a
LoraDB error — in a CLI, an HTTP response body, or a binding-layer
exception — sees the same shape and tone regardless of which crate raised it.

## Rules

1. **Lowercase first letter.** The message is a clause, not a sentence.
   Acronyms keep their case (`WAL`, `CRC`, `I/O`, `UNION`, `MATCH`).

2. **No trailing punctuation.** No period, no exclamation mark, no ellipsis.

3. **Identifiers in backticks.** Variable names, property names, label
   names, paths, flag names, and enum-string values are wrapped in
   `` `...` ``. Single quotes (`'foo'`) and bare values (`foo`) are not used
   for identifiers.

4. **Hint after a semicolon, not a new sentence.** If the message includes
   a remediation hint, separate it with `; ` rather than `. `:

   - Good: `` cannot delete node 5 because it still has relationships; use DETACH DELETE to remove the node and its relationships ``
   - Bad: `` Cannot delete node 5. It still has relationships. Use DETACH DELETE. ``

5. **Layer prefix only when it disambiguates.** A WAL error starts with
   `WAL `, a snapshot error starts with `snapshot `, a vector build error
   starts with `vector `. Higher layers do not re-prefix — `lora-database`
   surfacing a WAL failure does **not** prepend `database error:`.

6. **Include the offending value.** If a name, identifier, or value caused
   the failure, name it. `` unknown variable `n` `` is correct; `unknown
   variable` alone is not.

7. **No `Debug` formatting in user-facing messages.** Use `Display`, or
   format named fields explicitly. `{span:?}` is wrong; `{}..{}` against
   `span.start, span.end` is right.

## Composition

The canonical shape is one of:

- `<what failed>` — `wal is poisoned`
- `<what failed>: <details>` — `unknown variable \`n\``
- `<what failed> because <reason>` — `cannot delete node 5 because it still has relationships`
- `<what failed>; <hint>` — `wal is poisoned; restore from a snapshot to recover`
- `<what failed> because <reason>; <hint>` — combination of the above

## Codes are the contract

Programmatic consumers — the bindings, the HTTP layer, integration tests —
must match on the stable `LoraErrorCode` wire string (`LORA_PARSE`,
`LORA_TIMEOUT`, ...), never on the message text. Messages may be
rewritten between minor versions to improve clarity; codes are part of
the public API and never change.

See `crates/lora-database/src/error.rs` for the catalog.

## Boundary discipline

`anyhow::Error` is allowed in **internal** `?`-chains because it makes
multi-layer error funnels ergonomic, but it must not appear in the
return type of any `pub fn` in `lora-database` or `lora-server`. Public
methods on `Database`, `Transaction`, the `QueryRunner`/`SnapshotAdmin`/
`WalAdmin` traits, and HTTP handlers all return `Result<T, LoraError>`.
This guarantees that every external caller — bindings, transports,
tests — receives a typed code at the boundary and never has to downcast
through an `anyhow` chain.

When converting an internal `anyhow::Error` to a `LoraError` at the
boundary, the `From<anyhow::Error> for LoraError` impl downcasts to any
known concrete type (`ParseError`, `WalError`, `LoraError` itself, …)
and falls back to `LoraErrorCode::Internal` when the chain is opaque.

## HTTP status mapping

The transport in `lora-server` maps codes to status as follows:

| Code | Status | Notes |
| --- | --- | --- |
| `LORA_PARSE`, `LORA_SEMANTIC`, `LORA_READ_ONLY`, `LORA_DATABASE_NAME`, `LORA_CONFIG` | 400 | Caller-fixable mistake |
| `LORA_INVALID_PARAMS`, `LORA_INVALID_VECTOR` | 422 | Well-formed request, semantically invalid value |
| `LORA_NOT_FOUND` | 404 | Named entity does not exist |
| `LORA_CONSTRAINT` | 409 | Action conflicts with current state |
| `LORA_TIMEOUT` | 408 | Cooperative deadline expired |
| `LORA_WAL_POISONED` | 503 | Engine cannot accept further writes |
| `LORA_IO`, `LORA_WAL_CORRUPTION`, `LORA_SNAPSHOT_CODEC`, `LORA_SNAPSHOT_CRYPTO`, `LORA_INTERNAL` | 500 | Server-side failure |
