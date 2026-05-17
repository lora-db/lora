---
title: Utility Functions
sidebar_label: Utility
description: Utility function namespaces in LoraDB, including type and cast helpers, entity introspection, path helpers, text similarity, JSON, bytes, crypto, UUID, number predicates, and bit operations.
---

# Utility Functions

This page covers the built-in namespaces that do not need a full
category page of their own. They follow the same rules as the rest of
the function library: canonical names are namespaced, arguments are
validated before execution, and most invalid or missing inputs return
`null`.

## Type, Cast, and Value Helpers

| Namespace | Functions |
|---|---|
| `type.*` | `type.of(x)`, `type.is(x, TYPE)` |
| `cast.*` | `cast.to(x, TYPE)`, `cast.try(x, TYPE)`, `cast.can(x, TYPE)` |
| `value.*` | `value.size(x)`, `value.keys(x)`, `value.properties(x)`, `value.reverse(x)`, `value.coalesce(a, b, ...)`, `value.is_null(x)`, `value.is_not_null(x)`, `value.id(x)` |

<QueryCodeBlock code={String.raw`RETURN type.of([1, 2, 3]);              // 'LIST<INTEGER>'
RETURN type.is('2026-05-01'::DATE, DATE); // true
RETURN cast.try($maybe_int, INTEGER);   // null if conversion fails
RETURN value.coalesce(null, 'fallback') // 'fallback'`} />

Prefer `value::TYPE`, `CAST(value AS TYPE)`, and
`TRY_CAST(value AS TYPE)` in handwritten query text. The function forms
are useful in generated queries where the target type is already being
assembled as an expression.

## Entity and Path Introspection

| Namespace | Functions |
|---|---|
| `node.*` | `node.id(n)`, `node.labels(n)`, `node.has_label(n, label)`, `node.keys(n)`, `node.properties(n)` |
| `edge.*` | `edge.id(r)`, `edge.type(r)`, `edge.keys(r)`, `edge.properties(r)`, `edge.start(r)`, `edge.end(r)` |
| `path.*` | `path.nodes(p)`, `path.edges(p)`, `path.length(p)`, `path.first(p)`, `path.last(p)` |
| `value.*` | `value.id(x)`, `value.keys(x)`, `value.properties(x)` for node, relationship, and map inputs |

<QueryCodeBlock code={String.raw`MATCH p = (a:Person)-[r:KNOWS]->(b:Person)
RETURN node.labels(a),
       edge.type(r),
       path.length(p),
       path.nodes(p)`} />

The familiar Cypher aliases `id`, `labels`, `type`, `keys`,
`properties`, and `length` resolve to these canonical helpers.

## Text Similarity and Phonetics

| Function | Use |
|---|---|
| `text.distance(a, b, metric)` | Edit distance as an integer |
| `text.similarity(a, b, metric)` | Similarity score as a float |
| `text.phonetic(s, algorithm)` | Phonetic key |
| `text.phonetic_match(a, b, algorithm)` | Phonetic equality predicate |

<QueryCodeBlock code={String.raw`RETURN text.distance('kitten', 'sitting', 'levenshtein'); // 3
RETURN text.similarity('lora', 'loradb', 'jaro_winkler');
RETURN text.phonetic_match('Smith', 'Smyth', 'soundex')  // true`} />

Supported distance metrics are `levenshtein`, `damerau`, and `hamming`.
Supported similarity metrics are `levenshtein`, `jaro`, `jaro_winkler`,
and `sorensen_dice`. The current phonetic algorithm is `soundex`.

## Number and Bit Utilities

| Namespace | Functions |
|---|---|
| `number.*` | `number.format(n[, precision[, thousands]])`, `number.to_base(n, radix)`, `number.from_base(s, radix)`, `number.to_roman(n)`, `number.from_roman(s)`, `number.is_integer(n)`, `number.is_even(n)`, `number.is_odd(n)`, `number.is_positive(n)`, `number.is_negative(n)`, `number.is_zero(n)`, `number.is_nan(n)`, `number.is_finite(n)`, `number.is_infinite(n)`, `number.bitop(a, op, b)` |
| `bits.*` | `bits.and(a, b)`, `bits.or(a, b)`, `bits.xor(a, b)`, `bits.shift_left(a, b)`, `bits.shift_right(a, b)`, `bits.not(a)` |

<QueryCodeBlock code={String.raw`RETURN number.format(12345.678, 2, ','); // '12,345.68'
RETURN number.to_roman(1994);            // 'MCMXCIV'
RETURN bits.and(12, 10);                 // 8
RETURN bits.shift_left(3, 2)            // 12`} />

`bits.*` operates on integers. `number.bitop` accepts operation strings
such as `'and'`, `'or'`, `'xor'`, `'shl'`, `'shr'`, and `'not'`; prefer
the named `bits.*` forms in new queries.

## Bytes, Crypto, UUID, and JSON

| Namespace | Functions |
|---|---|
| `bytes.*` | `bytes.size(x)`, `bytes.from_string(s[, encoding])`, `bytes.to_string(bytes[, encoding])`, `bytes.base64_encode(x)`, `bytes.base64_decode(s)`, `bytes.hex_encode(x)`, `bytes.hex_decode(s)`, `bytes.compress(x[, algorithm])`, `bytes.decompress(x[, algorithm])` |
| `crypto.*` | `crypto.blake3(x)`, `crypto.crc32(x)` |
| `uuid.*` | `uuid.new()`, `uuid.from_string(s)`, `uuid.is_valid(s)` |
| `json.*` | `json.encode(x[, pretty])`, `json.decode(s)`, `json.path(x, path)` |

<QueryCodeBlock code={String.raw`RETURN bytes.base64_encode('lora');       // 'bG9yYQ=='
RETURN crypto.blake3('lora');             // hex digest string
RETURN uuid.is_valid(uuid.new());         // true
RETURN json.path(json.decode('{"a":[10]}'), '$.a[0]') // 10`} />

`bytes.compress` and `bytes.decompress` support `gzip` and `deflate`.
`json.encode` supports scalar, list, map, and temporal values that can
be represented in JSON. Graph entities are intentionally not encoded as
JSON unless you first project the fields you want into a map.
