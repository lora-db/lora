---
title: Built-in Function Development
sidebar_label: Functions
description: Developer guide for adding, testing, and documenting built-in functions in LoraDB.
---

# Built-in Function Development

LoraDB's function surface has four moving parts: the analyzer owns
public signatures, the executor owns behavior, integration tests
exercise the query pipeline, and the docs describe the user-facing
contract. When adding a function, update all four together.

## Naming Rules

New scalar functions should use a canonical namespaced name:

```text
namespace.operation
```

Use lowercase snake_case for both segments. The namespace should name
the value family or concern:

| Namespace | Use it for |
|---|---|
| `string.*` | Text cleanup, search, slicing, tokenizing, encoding |
| `text.*` | Fuzzy text distance, similarity, and phonetic helpers |
| `number.*` | Formatting, radix conversion, numeric predicates |
| `bits.*` | Integer bit operations |
| `math.*` | Numeric formulas, rounding, trig, constants, random |
| `list.*` | List indexing, reshaping, set-like transforms |
| `map.*` | Map lookup, patching, nested paths, entries |
| `temporal.*` | Current time, truncation, fields, date differences |
| `geo.*` | Point predicates and distances |
| `vector.*` | Vector distance, similarity, shape, coordinates |
| `node.*`, `edge.*`, `path.*` | Graph entity introspection |
| `type.*` | Runtime type inspection and checks |
| `cast.*` | Explicit conversion helpers |
| `value.*` | Polymorphic helpers that genuinely apply to many types |

Prefer one operation name per concept. Let behavior vary by optional
arguments only when that keeps the API clearer than adding a second
function name.

## Implementation Checklist

1. Add the signature in `crates/lora-analyzer/src/analyzer/builtin_signatures.rs`.
2. Implement or extend the matching executor module in `crates/lora-executor/src/eval/builtins/`.
3. Add integration tests in `crates/lora-database/tests/builtin_namespaces.rs`.
4. Update the relevant reference page in `apps/loradb.com/docs/functions/`.
5. If the function belongs to a new docs category, add it to `apps/loradb.com/sidebars.js` and link it from `apps/loradb.com/docs/functions/overview.md`.
6. Run the focused tests listed below.

Keep functions pure. Builtins should compute values only. Mutating graph
work belongs in clauses or procedures, not scalar functions.

## Analyzer Signatures

The analyzer signature table is the public source of truth. It validates
unknown names, arity, type-literal arguments, and enum-like arguments
before the executor sees a row.

```rust
spec("number.to_base", 2, Some(2)),
spec("number.from_base", 2, Some(2)),
spec("math.min", 1, None),
spec_enum("vector.distance", 3, Some(3), &[2]),
spec_type("cast.try", 2, Some(2), &[1]),
```

Use:

| Helper | Meaning |
|---|---|
| `spec(name, min, max)` | Ordinary value arguments |
| `spec_enum(name, min, max, slots)` | Arguments that accept enum-like literals, such as vector metrics |
| `spec_type(name, min, max, slots)` | Arguments that accept type literals, such as `DATE` or `INTEGER` |
| `max: None` | Variadic function |

Only add a compatibility alias when there is a real user-facing reason,
such as a Cypher spelling that developers already expect.

## Executor Dispatch

Each namespace has a module under `crates/lora-executor/src/eval/builtins/`.
Dispatch returns `None` only when the operation name is unknown for that
namespace. Known functions return a `LoraValue`, usually
`LoraValue::Null` for invalid runtime inputs.

```rust
pub(super) fn dispatch(op: &str, args: &[LoraValue]) -> Option<LoraValue> {
    Some(match op {
        "to_base" => to_base(args),
        "from_base" => from_base(args),
        _ => return None,
    })
}
```

Prefer small helper functions. Validate argument count in the analyzer;
validate runtime types, domains, overflow, and option values in the
executor.

## Null And Error Semantics

Most scalar functions should return `null` when:

- an input is `null`
- an input has the wrong runtime type
- a numeric operation is out of domain
- a conversion overflows
- an option string is unknown

Strict casts are the main exception: `CAST(value AS TYPE)` may report a
conversion error. `TRY_CAST(value AS TYPE)` should return `null`.

Aggregates have their own row-group semantics and should be documented
on the aggregation page instead of copied into scalar function docs.

## Tests

Add tests that call the function through Cypher, not just Rust helpers.
That catches parser, analyzer, compiler, and executor drift together.

```rust
#[test]
fn number_radix_conversions() {
    assert_eq!(db().scalar("RETURN number.to_base(255, 16)"), json!("ff"));
    assert_eq!(db().scalar("RETURN number.from_base('ff', 16)"), json!(255));
}
```

Run the drift tests and the focused integration tests:

```bash
cargo test -p lora-executor drift_tests
cargo test -p lora-database --test builtin_namespaces number_radix_conversions
```

For larger function work, run the whole namespace integration file:

```bash
cargo test -p lora-database --test builtin_namespaces
```

## Documentation

Every added function needs user-facing docs in `apps/loradb.com`.
Document the signature, null behavior, invalid input behavior, and at
least one realistic query pattern.

For a new or substantially expanded category:

- add or update `apps/loradb.com/docs/functions/<category>.md`
- link it from `apps/loradb.com/docs/functions/overview.md`
- add it to the Functions section in `apps/loradb.com/sidebars.js`

Docs should prefer canonical namespaced functions. Mention aliases only
when they are common Cypher compatibility forms or migration aids.

## Review Checklist

Before shipping a function addition, verify:

| Question | Why it matters |
|---|---|
| Does the analyzer signature match the executor behavior? | Wrong arity or missing dispatch breaks queries before runtime. |
| Does invalid input return the documented value? | Users build ingestion flows around null behavior. |
| Are examples copy-pastable Cypher? | Docs double as tests people run manually. |
| Is the function in the right namespace? | The function library stays learnable only if names remain predictable. |
| Did the docs page and sidebar change together? | Docusaurus builds fail on missing sidebar ids. |
