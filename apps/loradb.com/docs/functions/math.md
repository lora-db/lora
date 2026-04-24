---
title: Math Functions
sidebar_label: Math
description: Math functions in LoraDB — abs, sign, sqrt, exp, log, trigonometry, rounding, and constants — with domain-safe fallbacks to null rather than errors.
---

# Math Functions

All math functions return `null` on `null` input. Functions with a
restricted domain (e.g. `sqrt` of a negative number) return `null`
rather than raising.

## Overview

| Goal | Function |
|---|---|
| Absolute value | [`abs(n)`](#rounding-and-absolute-value) |
| Round | [`ceil`, `floor`, `round`, `sign`](#rounding-and-absolute-value) |
| Root, power, log | [`sqrt`, `exp`, `log`, `ln`, `log10`](#roots-powers-logs) |
| Trig | [`sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`](#trigonometry) |
| Unit conversion | [`degrees`, `radians`](#trigonometry) |
| Constants | [`pi()`, `e()`](#constants) |
| Random | [`rand()`](#random) |
| Arithmetic | [`+ - * / % ^`](#arithmetic-operators) |

## Rounding and absolute value

| Function | Behaviour |
|---|---|
| `abs(n)` | Absolute value — `Int` stays `Int`, `Float` stays `Float` |
| `ceil(n)` | Round up to the nearest integer |
| `floor(n)` | Round down |
| `round(n)` | Round to nearest, half-to-even |
| `sign(n)` | `-1`, `0`, or `1` |

```cypher
RETURN abs(-3.5)    -- 3.5
RETURN ceil(3.1)    -- 4
RETURN floor(3.9)   -- 3
RETURN round(3.5)   -- 4
RETURN round(2.5)   -- 2       (banker's rounding: half-to-even)
RETURN sign(-9)     -- -1
RETURN sign(0)      --  0
RETURN sign(9)      --  1
```

### Banker's rounding

`round()` breaks ties toward the nearest even integer, which reduces
bias on long series:

```cypher
RETURN round(0.5), round(1.5), round(2.5), round(3.5)
-- 0, 2, 2, 4
```

### Fractional rounding

No built-in `round(n, digits)`. Compose with multiply / divide:

```cypher
WITH 3.14159 AS pi
RETURN round(pi * 100) / 100   -- 3.14
```

## Roots, powers, logs

| Function | Domain | Returns |
|---|---|---|
| `sqrt(n)` | `n ≥ 0` | `Float`; `null` on negative input |
| `log(n)` / `ln(n)` | `n > 0` | Natural log; `null` out of domain |
| `log10(n)` | `n > 0` | Base-10 log; `null` out of domain |
| `exp(n)` | any | `e^n` |

```cypher
RETURN sqrt(16)       -- 4.0
RETURN sqrt(-1)       -- null
RETURN log(e())       -- 1.0
RETURN log10(1000)    -- 3.0
RETURN exp(1)         -- 2.7182818…
```

### Hypotenuse

```cypher
WITH 3 AS a, 4 AS b
RETURN sqrt(a ^ 2 + b ^ 2)   -- 5.0
```

For 2D Cartesian points, [`distance`](./spatial#distance) does this.

## Trigonometry

All angles are in **radians**. Domain violations (e.g. `asin(2)`)
return `null` rather than raising.

| Function | Notes |
|---|---|
| `sin(x)`, `cos(x)`, `tan(x)` | Standard trig |
| `asin(x)`, `acos(x)`, `atan(x)` | Inverse trig; domain `[-1, 1]` for `asin`/`acos` |
| `atan2(y, x)` | Two-argument arctangent — `y` first |
| `degrees(r)` / `radians(d)` | Unit conversion |

```cypher
RETURN sin(pi() / 2)          -- 1.0
RETURN cos(0)                 -- 1.0
RETURN tan(pi() / 4)          -- 0.9999…    (float rounding)
RETURN asin(1)                -- 1.5707… (π/2)
RETURN asin(2)                -- null       (out of domain)
RETURN atan2(1, 1)            -- 0.7853… (π/4)
RETURN degrees(pi())          -- 180.0
RETURN radians(180)           -- 3.1415…
```

### Work in degrees

Most geometry inputs are degrees — wrap every trig call:

```cypher
WITH 30 AS deg
RETURN sin(radians(deg)),
       cos(radians(deg))
```

### Bearing between two points

```cypher
WITH 4.89 AS lon1, 52.37 AS lat1,
     4.40 AS lon2, 51.00 AS lat2
WITH radians(lat1) AS φ1, radians(lat2) AS φ2,
     radians(lon2 - lon1) AS dλ
RETURN (degrees(atan2(
  sin(dλ) * cos(φ2),
  cos(φ1) * sin(φ2) - sin(φ1) * cos(φ2) * cos(dλ)
)) + 360) % 360 AS bearing
```

Approximation — use [`distance`](./spatial#distance) for real geodesic
distance between WGS-84 points.

## Constants

```cypher
RETURN pi()     -- 3.14159…
RETURN e()      -- 2.71828…
```

## Random

`rand()` returns a `Float` in `[0, 1)`.

```cypher
RETURN rand()                    -- e.g. 0.42389…
RETURN rand() * 100              -- a Float in [0, 100)
RETURN toInteger(rand() * 100)   -- an Int in [0, 99]
```

Not cryptographically secure — seeded from system-time nanoseconds. Do
**not** use for key generation or sampling that must be unpredictable.

### Random sample

Each row gets its own `rand()` value:

```cypher
MATCH (u:User)
RETURN u
ORDER BY rand()
LIMIT 100
```

### Weighted pick

```cypher
MATCH (p:Prize)
WITH p, rand() * p.weight AS score
ORDER BY score DESC
LIMIT 1
RETURN p
```

## Arithmetic operators

| Op | Example | Notes |
|---|---|---|
| `+` | `a + b` | Also concatenates strings and lists |
| `-` | `a - b` | Unary `-x` allowed |
| `*` | `a * b` | |
| `/` | `a / b` | Integer / integer is integer division; divide by zero → `null` |
| `%` | `a % b` | Modulo; mod by zero → `null` |
| `^` | `a ^ b` | Exponent |

```cypher
RETURN 10 / 3           -- 3         (integer division)
RETURN 10 / 3.0         -- 3.333…    (float)
RETURN 10 % 3           -- 1
RETURN 2 ^ 10           -- 1024
RETURN 1 / 0            -- null
```

### Mixed-type arithmetic

```cypher
RETURN 1 + 2.5          -- 3.5 (Float)
RETURN 10 / 3.0         -- 3.333…
```

Any `Float` operand promotes the result to `Float`. To do integer
division on a float input, floor first:

```cypher
RETURN toInteger(10 / 3.0)    -- 3
```

## Numeric precedence

`^` binds tightest, then unary `-`/`+`, then `*` / `/` / `%`, then
binary `+` / `-`, then comparisons. Use parentheses freely.

```cypher
RETURN 1 + 2 * 3        -- 7
RETURN (1 + 2) * 3      -- 9
RETURN -2 ^ 2           -- -4        (binds as -(2^2))
RETURN (-2) ^ 2         -- 4
```

## Common patterns

### Clamp a value

```cypher
WITH $raw AS raw, 0 AS lo, 100 AS hi
RETURN CASE
  WHEN raw < lo THEN lo
  WHEN raw > hi THEN hi
  ELSE raw
END AS clamped
```

Uses [`CASE`](../queries/return-with#case-expressions) — LoraDB's
conditional expression.

### Normalise to 0..1

```cypher
MATCH (m:Metric)
WITH min(m.value) AS lo, max(m.value) AS hi
MATCH (m:Metric)
RETURN m.id,
       (m.value - lo) / (hi - lo) AS normalised
```

### Bucket a number

```cypher
MATCH (p:Product)
RETURN (p.price / 10) * 10 AS bucket, count(*) AS n
ORDER BY bucket
```

### Log-scale bucket

```cypher
MATCH (p:Post)
WITH p, CASE WHEN p.views > 0 THEN toInteger(log10(p.views)) ELSE 0 END AS decade
RETURN decade, count(*) AS posts
ORDER BY decade
```

### Exponential decay (recency score)

```cypher
MATCH (p:Post)
WITH p, duration.inDays(p.published_at, datetime()) AS age
RETURN p.id, p.title,
       p.views * exp(-age * 0.05) AS score
ORDER BY score DESC
LIMIT 20
```

Half-life of roughly 14 days at `0.05` — tune the constant to taste.

### Percent change

```cypher
MATCH (m:Metric)
RETURN m.id,
       m.current,
       m.previous,
       CASE
         WHEN m.previous = 0 OR m.previous IS NULL THEN null
         ELSE (m.current - m.previous) * 100.0 / m.previous
       END AS pct_change
```

Guard against zero/null — see
[`CASE`](../queries/return-with#case-expressions).

### Weighted average

```cypher
MATCH (r:Review)
RETURN sum(r.stars * r.weight) / sum(r.weight) AS weighted_mean
```

## Edge cases

### Integer overflow

No guard — Rust panics in debug, wraps in release. For potentially
huge inputs, coerce to `Float` with
[`toFloat`](./string#type-conversion) first.

### NaN / Infinity

IEEE 754 applies. `NaN` is neither less than nor greater than any
value; `NaN == NaN` is `false`.

```cypher
RETURN 1.0 / 0.0          -- Infinity
RETURN 0.0 / 0.0          -- NaN
RETURN sqrt(-1)           -- null   (Cypher-level domain guard, not NaN)
```

### Integer division vs float division

```cypher
RETURN 7 / 2       -- 3
RETURN 7 / 2.0     -- 3.5
RETURN 7.0 / 2     -- 3.5
```

A single `.0` on either side flips to float division.

## Limitations

- Integer overflow is not explicitly guarded. Rust panics in debug
  builds, wraps in release. Coerce to `Float` with `toFloat` on
  potentially huge inputs.
- `NaN` and `Infinity` follow IEEE 754 — `NaN == NaN` evaluates to
  `false`, and `NaN` is neither less nor greater than any other value.
- `rand()` is not cryptographically secure.

## See also

- [**Scalars → Integer / Float**](../data-types/scalars#integer) — numeric types.
- [**WHERE**](../queries/where#arithmetic-in-where) — arithmetic in filters.
- [**Spatial Functions**](./spatial) — `distance` as the geodesic alternative.
- [**Aggregation Functions**](./aggregation) — `sum`, `avg`, percentiles.
