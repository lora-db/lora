---
title: Scalar Types — Null, Boolean, Integer, Float, String
sidebar_label: Scalars
---

# Scalar Types

The five scalar types — `Null`, `Boolean`, `Integer`, `Float`, `String`
— are the atoms of every value in LoraDB. Lists, maps, temporals,
spatials, and graph types are all compositions over these.

## Null

Represents the absence of a value. LoraDB uses **three-valued logic**
for comparisons involving `null`.

```cypher
RETURN null, null = null, null <> null, 1 = null
-- null, null, null, null
```

Boolean operators propagate `null` carefully:

| Expression | Result |
|---|---|
| `null AND true` | `null` |
| `null AND false` | `false` |
| `null OR true` | `true` |
| `null OR false` | `null` |
| `NOT null` | `null` |

### Null tests

Use [`IS NULL` / `IS NOT NULL`](../queries/where#null-checks) — **not**
`= null`:

```cypher
MATCH (n) WHERE n.optional IS NULL     RETURN n
MATCH (n) WHERE n.optional IS NOT NULL RETURN n
```

### Null in aggregates

| Aggregate | Behaviour |
|---|---|
| `count(*)` | Counts rows; `null` bindings still count |
| `count(expr)` | Skips `null` |
| `sum`, `avg`, `min`, `max` | Skip `null` |
| `collect(expr)` | Keeps `null` |

See [Aggregation Functions](../functions/aggregation) for the full
table.

### Null in ordering

`null` sorts **last** in ascending order and **first** in descending
order. Guard with [`coalesce`](../functions/overview#type-conversion-and-checking)
to change placement — see [Ordering](../queries/ordering#nulls-in-ordering).

## Boolean

`true` or `false`. Bools are **not** integers in LoraDB — `true = 1`
evaluates to `false`. Use `toInteger(b)` to convert.

```cypher
RETURN true AND false            -- false
RETURN true OR false             -- true
RETURN NOT true                  -- false
RETURN true XOR false            -- true
RETURN toInteger(true),          -- 1
       toInteger(false)          -- 0
```

### Parameters

Bools bind transparently:

```cypher
MATCH (u:User) WHERE u.active = $active RETURN u
```

### Use as a flag property

```cypher
MATCH (p:Product) WHERE p.in_stock RETURN p
MATCH (p:Product) WHERE NOT p.in_stock RETURN p
```

Note the short form: `WHERE p.in_stock` is equivalent to
`WHERE p.in_stock = true`, but will also match only `true` — it drops
rows where `p.in_stock` is `false` or `null`.

## Integer

64-bit signed (`i64`). Literals can be decimal, hex, or octal.

```cypher
RETURN 42, -1, 0, 0xFF, 0o17
-- 42, -1, 0, 255, 15
```

### Arithmetic

| Op | Example | Notes |
|---|---|---|
| `+`, `-`, `*` | `1 + 2` | Integer if both operands are integers |
| `/` | `10 / 3` → `3` | Integer division when both sides are `Int` |
| `%` | `10 % 3` → `1` | Modulo |
| `^` | `2 ^ 10` → `1024` | Exponent |
| unary `-`, `+` | `-x` | |

Divide or modulo by zero → `null` rather than an error.

```cypher
RETURN 1 / 0    -- null
RETURN 10 % 0   -- null
```

See [Math Functions → Arithmetic operators](../functions/math#arithmetic-operators)
for the full details, including mixed-type arithmetic.

### Conversion

```cypher
RETURN toInteger('42'),     -- 42
       toInteger(3.9),      -- 3       (truncates)
       toInteger('abc'),    -- null
       toFloat(42)          -- 42.0
```

### Use as an id

Integers are the most common id type:

```cypher
MATCH (u:User {id: $id}) RETURN u
```

For very large ids, note [integer precision in JS](../getting-started/node#performance--best-practices)
— values above `2^53` lose precision when crossing the JS boundary.

### Limitations

Integer overflow is not explicitly guarded. Rust panics in debug, wraps
in release. For extreme inputs, convert to `Float` first.

## Float

64-bit floating point (`f64`, IEEE 754).

```cypher
RETURN 3.14, 1.0e10, -0.5
```

Mixed-type arithmetic promotes to `Float`:

```cypher
RETURN 1 + 2.5       -- 3.5 (Float)
RETURN 10 / 3.0      -- 3.333…
```

### IEEE 754 quirks

- `NaN == NaN` → `false`
- `NaN` comparisons → `false`
- `1.0 / 0.0` → `Infinity` (not `null` — float division is defined)

```cypher
RETURN 1.0 / 0.0          -- Infinity
RETURN 0.0 / 0.0          -- NaN
```

### Rounding

See [Math → Rounding](../functions/math#rounding-and-absolute-value):

```cypher
RETURN round(3.5)        -- 4
RETURN round(2.5)        -- 2        (banker's rounding)
RETURN ceil(0.1)         -- 1
RETURN floor(-0.1)       -- -1
```

### Use as a ratio / rate

```cypher
MATCH (r:Review)
RETURN r.stars / 5.0 AS normalised   -- 0.0 .. 1.0
```

## String

UTF-8 text. Either quote style works.

```cypher
RETURN 'hello', "world"
RETURN 'it''s fine'        -- 'it's fine'  (double the quote to escape)
RETURN "with \n newline"   -- string with a literal newline
```

### Concatenation

```cypher
RETURN 'Hello, ' + 'Ada'         -- 'Hello, Ada'
RETURN 'id=' + toString(42)      -- 'id=42'
```

Other types must be converted to `String` via
[`toString`](../functions/string#type-conversion) — `+` does not
implicitly stringify numeric operands.

### Useful functions

See [String Functions](../functions/string) for the full reference.
Highlights:

```cypher
RETURN toLower('LoroDB'),           -- 'lorodb'
       split('a,b,c', ','),         -- ['a', 'b', 'c']
       substring('LoroDB', 0, 4),   -- 'Loro'
       replace('aba', 'a', 'x')     -- 'xbx'
```

### Comparison

Strings sort byte-lexicographically. Case-sensitive comparisons are
the default; normalise with `toLower` / `toUpper` for case-insensitive
matching — see [WHERE → string matching](../queries/where#string-matching).

```cypher
MATCH (u:User)
WHERE toLower(u.name) = toLower($search)
RETURN u
```

### Lengths: bytes vs code points

```cypher
RETURN size('café'),        -- may be 4 or 5 depending on encoding nuances
       charLength('café')   -- 4  (code points)
```

For display-length, prefer `charLength`. For serialisation sizes,
prefer `size`.

## Parameters

Scalar [parameters](../queries/#parameters) bind transparently from
host-language values — Rust primitives, JS numbers / strings /
booleans, Python `int` / `float` / `str` / `None`. See the per-platform
[Getting Started](../getting-started/installation) guide for your
language.

```cypher
MATCH (u:User)
WHERE u.id = $id AND u.active = $active
RETURN u
```

## Comparison matrix

| | Scalar equality | Ordering |
|---|---|---|
| `Boolean` | `true = true` → `true` | `false < true` |
| `Integer` | numeric | numeric |
| `Float` | numeric (IEEE 754) | numeric; `NaN` incomparable |
| `String` | case-sensitive | byte-lex |
| `Null` | null-propagating | null-propagating (last asc, first desc) |

Cross-type comparisons return `null` (type mismatch detection is not
yet implemented — see [Limitations](../limitations)).

## Common patterns

### Default a missing scalar

```cypher
MATCH (p:Person)
RETURN p.name, coalesce(p.nickname, p.name) AS display
```

### Safe equality

```cypher
MATCH (a), (b)
WHERE coalesce(a.key, '') = coalesce(b.key, '')
RETURN a, b
```

### Case-insensitive search

```cypher
MATCH (u:User)
WHERE toLower(u.email) CONTAINS toLower($q)
RETURN u
```

### Boolean flag pattern

```cypher
MATCH (p:Product) WHERE p.in_stock RETURN p

-- Equivalent, explicit:
MATCH (p:Product) WHERE p.in_stock = true RETURN p
```

The bare form *drops* rows where `p.in_stock` is `null` — common
source of surprise when a property is missing rather than `false`.
Guard with `coalesce`:

```cypher
MATCH (p:Product)
WHERE coalesce(p.in_stock, false)
RETURN p
```

### Branch on a scalar with CASE

```cypher
MATCH (u:User)
RETURN u.handle,
       CASE u.tier
         WHEN 'pro'  THEN 'paying'
         WHEN 'free' THEN 'trial'
         ELSE             'unknown'
       END AS segment
```

See [`CASE`](../queries/return-with#case-expressions) — LoraDB's
conditional expression, supporting both "match a value" and "generic
boolean per branch" forms.

## Edge cases

### Nulls in arithmetic

`1 + null` is `null`. Any arithmetic involving `null` propagates.

```cypher
MATCH (p:Person)
RETURN p.name, p.age + 1 AS next_age
-- `null` if p.age is null
```

### Booleans and truthiness

There's no truthy coercion. `WHERE x` requires `x` to be a boolean;
`WHERE 0` or `WHERE ''` are analysis errors. Use `WHERE x IS NOT NULL`
for existence checks.

### Very small numbers

Sub-nanosecond precision is lost on `Float`. For exact arithmetic on
small quantities (money, percentages), use scaled integers (cents,
basis points).

## See also

- [**Lists & Maps**](./lists-and-maps) — collections.
- [**Temporal**](./temporal) / [**Spatial**](./spatial) — typed domains.
- [**Math Functions**](../functions/math),
  [**String Functions**](../functions/string) — operators and helpers.
- [**WHERE**](../queries/where) — null-safe filtering and comparison.
- [**Ordering → nulls**](../queries/ordering#nulls-in-ordering).
- [**Limitations**](../limitations) — overflow, type-mismatch behavior.
