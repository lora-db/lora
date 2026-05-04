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

See `crates/lora-database/src/error/code.rs` for the catalog.
