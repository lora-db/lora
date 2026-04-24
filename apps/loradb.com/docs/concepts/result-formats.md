---
title: Query Result Formats
sidebar_label: Result Formats
description: The four result shapes LoraDB returns — rows, rowArrays, graph, and combined — with the trade-offs between payload size, client ergonomics, and graph versus tabular access.
---

# Query Result Formats

Every query returns the **same data**, but the engine can shape it into
one of four wire formats. The choice is a trade-off between payload
size, ease of access in your host code, and whether you want a row view
or a graph view.

## The four formats

| Format | Shape | Use when… |
|---|---|---|
| [`rows`](#rows) | One object per row, keyed by column. | Most app code — each row reads like a record. |
| [`rowArrays`](#rowarrays) | Columns plus tuple rows. | Wide result sets where keys are repetitive. |
| [`graph`](#graph) | De-duplicated nodes + relationships. | Rendering a graph — visualisers, diffs. |
| [`combined`](#combined) | Rows **and** a side graph. | You want both per-row data and the underlying entities. |

## Choosing a format

| If your consumer… | Pick |
|---|---|
| Reads values by column name | `rows` |
| Processes many rows with the same columns | `rowArrays` (smaller payload) |
| Draws a graph of matched entities | `graph` |
| Needs rows **and** the graph they touched | `combined` |

If you don't care: `rows` is the most ergonomic default in host code.

## Setting the format

| Transport | How |
|---|---|
| Rust / Node / Python / WASM / Go / Ruby | `ExecuteOptions { format }` or per-call option |
| HTTP | `"format": "rows"` in the `POST /query` body |

If omitted, the engine-default is [`graph`](#graph). Bindings may
override that default for ergonomics — for example, `lora-node` and
`lora-python` return `rows` unless you ask for another shape.

## `rows`

The most natural shape for application code: one JSON object per row,
keyed by the `RETURN` column name.

### Example

For `MATCH (p:Person) RETURN p.name AS name, p.born AS born`:

```json
{
  "rows": [
    { "name": "Ada",   "born": 1815 },
    { "name": "Grace", "born": 1906 }
  ]
}
```

### When to prefer it

- You want to iterate with `row.name`, `row["name"]`, etc.
- Result sets are small-to-medium; the per-row key repetition isn't a
  concern.

## `rowArrays`

Columns are listed once; rows are arrays indexed by column position.

### Example

```json
{
  "columns": ["name", "born"],
  "rows": [
    ["Ada",   1815],
    ["Grace", 1906]
  ]
}
```

### When to prefer it

- Large result sets — avoids repeating the column keys per row.
- You're streaming into a tabular UI that already knows column order.

### Gotcha

You index rows by position. A schema change (e.g. reordering `RETURN`
expressions) breaks downstream code. Prefer `rows` for code that's
read outside the query author's head.

## `graph`

Every matched [node](./nodes) and [relationship](./relationships),
de-duplicated, with labels, types, and properties hydrated.

### Example

For `MATCH (a:Person)-[r:KNOWS]->(b:Person) RETURN a, r, b`:

```json
{
  "graph": {
    "nodes": [
      { "id": 1, "labels": ["Person"], "properties": { "name": "Ada" } },
      { "id": 2, "labels": ["Person"], "properties": { "name": "Grace" } }
    ],
    "relationships": [
      { "id": 10, "startId": 1, "endId": 2, "type": "KNOWS", "properties": {} }
    ]
  }
}
```

### When to prefer it

- You're rendering the graph in a visualiser.
- You need every entity the query touched, exactly once, regardless of
  how many rows referenced it.

### Gotcha

Scalar projections (`RETURN p.name`) don't contribute new entities to
the graph — you'll get an empty `nodes` / `relationships` list unless
the query returns nodes or relationships directly.

## `combined`

Rows and a side graph in one payload. Useful when you want both
per-row results and the backing entities (for example: rows for a
table, and nodes+edges for a graph preview beside it).

### Example

```json
{
  "columns": ["a", "r", "b"],
  "data": [
    { "a": { "$node": 1 }, "r": { "$rel": 10 }, "b": { "$node": 2 } }
  ],
  "graph": {
    "nodes":         [ /* nodes 1 and 2, as in the `graph` format */ ],
    "relationships": [ /* relationship 10 */ ]
  }
}
```

Entities in `data` appear as references (e.g. `{"$node": 1}`); their
full hydrated form lives in `graph`. This keeps the payload compact
when the same node appears in many rows.

### When to prefer it

- Dual views (table + graph) of the same query.
- You've got a UI that links a table row to a node on a canvas.

## Typed values in every format

Regardless of format, scalar and structured values round-trip through
the same [shared contract](../data-types/overview):

- Primitives — `Null`, `Boolean`, `Integer`, `Float`, `String`.
- [Lists & maps](../data-types/lists-and-maps).
- [Temporal values](../data-types/temporal) — tagged with
  `{kind: "date" | "datetime" | …, iso: "…"}`.
- [Spatial points](../data-types/spatial) — tagged with
  `{kind: "point", srid, crs, x, y[, z]}`.
- Graph values — tagged with `{kind: "node" | "relationship" | "path"}`.

Each language binding provides narrow type guards to tell them apart
(`isNode`, `is_point`, …).

## See also

- [Data types overview](../data-types/overview) — the value model.
- [Queries — Overview](../queries/) — clauses and the pipeline.
- [HTTP API reference](../api/http) — how `format` is set over HTTP.
- [Troubleshooting → JSON/result format confusion](../troubleshooting#queries-return-empty-results).
