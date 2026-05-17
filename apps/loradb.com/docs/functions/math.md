---
title: Math Functions
sidebar_label: Math
description: Math functions in LoraDB — math.abs, math.sign, math.sqrt, math.exp, math.log, trigonometry, rounding, and constants — with domain-safe fallbacks to null rather than errors.
---

# Math Functions

Every math function returns `null` on `null` input. Functions with
a restricted domain (e.g. `math.sqrt` of a negative number) return
`null` rather than raising.

## Overview

| Goal | Function |
|---|---|
| Absolute value | [`math.abs(n)`](#rounding-and-absolute-value) |
| Round | [`math.ceil`, `math.floor`, `math.trunc`, `math.round`, `math.sign`](#rounding-and-absolute-value) |
| Root, power, log | [`math.sqrt`, `math.hypot`, `math.pow`, `math.exp`, `math.log`, `math.ln`, `math.log10`, `math.log_base`](#roots-powers-logs) |
| Bounds and interpolation | [`math.min`, `math.max`, `math.clamp`, `math.lerp`](#bounds-and-interpolation) |
| Number formatting and predicates | [`number.format`, `number.to_base`, `number.from_base`, `number.to_roman`, `number.from_roman`, `number.is_integer`, `number.is_even`, `number.is_odd`, `number.is_positive`](#number-formatting-and-predicates) |
| Trig | [`math.sin`, `math.cos`, `math.tan`, `math.asin`, `math.acos`, `math.atan`, `math.atan2`](#trigonometry) |
| Unit conversion | [`math.degrees`, `math.radians`](#trigonometry) |
| Constants | [`math.pi()`, `math.e()`](#constants) |
| Random | [`math.random()` / `random()`](#random) |
| Arithmetic | [`+ - * / % ^`](#arithmetic-operators) |

## Rounding and absolute value

| Function | Behaviour |
|---|---|
| `math.abs(n)` | Absolute value — `Int` stays `Int`, `Float` stays `Float` |
| `math.ceil(n)` | Round up to the nearest integer |
| `math.floor(n)` | Round down |
| `math.trunc(n)` | Drop the fractional part, toward zero |
| `math.round(n[, digits[, mode]])` | Round to nearest; default mode is `half_up` |
| `math.sign(n)` | `-1`, `0`, or `1` |

<QueryCodeBlock code={String.raw`RETURN math.abs(-3.5);    // 3.5
RETURN math.ceil(3.1);    // 4
RETURN math.floor(3.9);   // 3
RETURN math.trunc(-3.9);  // -3
RETURN math.round(3.5);   // 4
RETURN math.round(2.5);   // 3       (default half-up rounding)
RETURN math.sign(-9);     // -1
RETURN math.sign(0);      //  0
RETURN math.sign(9)      //  1`} />

### Rounding modes

`math.round(n)` defaults to half-up rounding and returns an integer.
Pass `digits` to round to a decimal place. Pass a mode string to choose
another strategy: `'half_up'`, `'half_even'`, `'ceil'`, `'floor'`, or
`'trunc'`.

<QueryCodeBlock code={String.raw`RETURN math.round(3.14159, 2);                  // 3.14
RETURN math.round(2.5);                         // 3
RETURN math.round(2.5, 0, 'half_even')         // 2.0`} />

## Roots, powers, logs

| Function | Domain | Returns |
|---|---|---|
| `math.sqrt(n)` | `n ≥ 0` | `Float`; `null` on negative input |
| `math.hypot(a, b)` | numeric inputs | `sqrt(a² + b²)` without intermediate overflow |
| `math.pow(a, b)` | numeric inputs | `a` raised to `b` |
| `math.log(n)` / `math.ln(n)` | `n > 0` | Natural log; `null` out of domain |
| `math.log10(n)` | `n > 0` | Base-10 log; `null` out of domain |
| `math.log_base(n, base)` | `n > 0`, `base > 0`, `base != 1` | Logarithm in an arbitrary base |
| `math.exp(n)` | any | `e^n` |

<QueryCodeBlock code={String.raw`RETURN math.sqrt(16);       // 4.0
RETURN math.hypot(3, 4);    // 5.0
RETURN math.pow(2, 10);     // 1024.0
RETURN math.sqrt(-1);       // null
RETURN math.log(math.e());       // 1.0
RETURN math.log10(1000);    // 3.0
RETURN math.log_base(8, 2); // 3.0
RETURN math.exp(1)         // 2.7182818…`} />

### Hypotenuse

<QueryCodeBlock code={String.raw`WITH 3 AS a, 4 AS b
RETURN math.hypot(a, b)   // 5.0`} />

For 2D Cartesian points, [`geo.distance`](./spatial#geodistance) does this.

## Bounds and Interpolation

`math.min(a, b, ...)` and `math.max(a, b, ...)` compare numeric
arguments inside a single row. They are scalar helpers, not aggregate
functions. Use [`min(expr)` and `max(expr)`](./aggregation#min--max)
when you need the smallest or largest value across rows in a group.

All arguments must be finite numbers. A `null`, string, map, list, or
non-finite float returns `null`.

<QueryCodeBlock code={String.raw`RETURN math.min(3, 1, 2);      // 1
RETURN math.max(3, 1, 2);      // 3
RETURN math.min(3, 1.5, 2);    // 1.5
RETURN math.max(3, null);      // null
RETURN math.clamp(125, 0, 100); // 100
RETURN math.lerp(10, 20, 0.25) // 12.5`} />

`math.clamp(x, lo, hi)` constrains a value to an inclusive range.
`math.lerp(a, b, t)` returns `a + (b - a) * t`, which is useful for
normalising scores and building weighted formulas.

## Number Formatting And Predicates

The `number.*` namespace contains numeric presentation helpers and
integer predicates. Base conversion supports radices from 2 through 36.
Invalid digits, unsupported bases, and overflow return `null`.

<QueryCodeBlock code={String.raw`RETURN number.format(12345.678, 2, ',');  // '12,345.68'
RETURN number.to_base(255, 16);           // 'ff'
RETURN number.from_base('ff', 16);        // 255
RETURN number.to_roman(1994);             // 'MCMXCIV'
RETURN number.from_roman('MCMXCIV')      // 1994`} />

Predicates are intentionally type-aware:

<QueryCodeBlock code={String.raw`RETURN number.is_integer(42);     // true
RETURN number.is_integer(42.5);   // false
RETURN number.is_even(42);        // true
RETURN number.is_odd(42);         // false
RETURN number.is_even(42.0);      // null
RETURN number.is_positive(0.1);   // true
RETURN number.is_negative(-1);    // true
RETURN number.is_zero(0.0);       // true
RETURN number.is_nan(0.0 / 0.0)  // true`} />

`number.is_even` and `number.is_odd` accept integer values. Use
`number.is_integer` first when data may arrive as floats. Sign
predicates accept both integers and floats.

## Trigonometry

All angles are in **radians**. Domain violations (e.g. `math.asin(2)`)
return `null` rather than raising.

| Function | Notes |
|---|---|
| `math.sin(x)`, `math.cos(x)`, `math.tan(x)` | Standard trig |
| `math.asin(x)`, `math.acos(x)`, `math.atan(x)` | Inverse trig; domain `[-1, 1]` for `asin`/`acos` |
| `math.atan2(y, x)` | Two-argument arctangent — `y` first |
| `math.degrees(r)` / `math.radians(d)` | Unit conversion |

<QueryCodeBlock code={String.raw`RETURN math.sin(math.pi() / 2);          // 1.0
RETURN math.cos(0);                 // 1.0
RETURN math.tan(math.pi() / 4);          // 0.9999…    (float rounding)
RETURN math.asin(1);                // 1.5707… (π/2)
RETURN math.asin(2);                // null       (out of domain)
RETURN math.atan2(1, 1);            // 0.7853… (π/4)
RETURN math.degrees(math.pi());          // 180.0
RETURN math.radians(180)           // 3.1415…`} />

### Work in degrees

Most geometry inputs are degrees — wrap every trig call:

<QueryCodeBlock code={String.raw`WITH 30 AS deg
RETURN math.sin(math.radians(deg)),
       math.cos(math.radians(deg))`} />

### Bearing between two points

<QueryCodeBlock code={String.raw`WITH 4.89 AS lon1, 52.37 AS lat1,
     4.40 AS lon2, 51.00 AS lat2
WITH math.radians(lat1) AS φ1, math.radians(lat2) AS φ2,
     math.radians(lon2 - lon1) AS dλ
RETURN (math.degrees(math.atan2(
  math.sin(dλ) * math.cos(φ2),
  math.cos(φ1) * math.sin(φ2) - math.sin(φ1) * math.cos(φ2) * math.cos(dλ)
)) + 360) % 360 AS bearing`} />

Approximation — use [`geo.distance`](./spatial#geodistance) for real geodesic
distance between WGS-84 points.

## Constants

<QueryCodeBlock code={String.raw`RETURN math.pi();     // 3.14159…
RETURN math.e()      // 2.71828…`} />

## Random

`math.random()` returns a `Float` in `[0, 1)`. The bare
`random()` form is an alias for the same function.

<QueryCodeBlock code={String.raw`RETURN math.random();                    // e.g. 0.42389…
RETURN math.random() * 100;              // a Float in [0, 100)
RETURN toInteger(math.random() * 100)   // an Int in [0, 99]`} />

Not cryptographically secure — seeded from system-time nanoseconds. Do
**not** use for key generation or sampling that must be unpredictable.

### Random sample

Each row gets its own `math.random()` value:

<QueryCodeBlock code={String.raw`MATCH (u:User)
RETURN u
ORDER BY math.random()
LIMIT 100`} />

### Weighted pick

<QueryCodeBlock code={String.raw`MATCH (p:Prize)
WITH p, math.random() * p.weight AS score
ORDER BY score DESC
LIMIT 1
RETURN p`} />

## Arithmetic operators

| Op | Example | Notes |
|---|---|---|
| `+` | `a + b` | Also concatenates strings and lists |
| `-` | `a - b` | Unary `-x` allowed |
| `*` | `a * b` | |
| `/` | `a / b` | Integer / integer is integer division; divide by zero → `null` |
| `%` | `a % b` | Modulo; mod by zero → `null` |
| `^` | `a ^ b` | Exponent |

<QueryCodeBlock code={String.raw`RETURN 10 / 3;           // 3         (integer division)
RETURN 10 / 3.0;         // 3.333…    (float)
RETURN 10 % 3;           // 1
RETURN 2 ^ 10;           // 1024
RETURN 1 / 0            // null`} />

### Mixed-type arithmetic

<QueryCodeBlock code={String.raw`RETURN 1 + 2.5;          // 3.5 (Float)
RETURN 10 / 3.0         // 3.333…`} />

Any `Float` operand promotes the result to `Float`. To do integer
division on a float input, floor first:

<QueryCodeBlock code={String.raw`RETURN toInteger(10 / 3.0)    // 3`} />

## Numeric precedence

`^` binds tightest, then unary `-`/`+`, then `*` / `/` / `%`, then
binary `+` / `-`, then comparisons. Use parentheses freely.

<QueryCodeBlock code={String.raw`RETURN 1 + 2 * 3;        // 7
RETURN (1 + 2) * 3;      // 9
RETURN -2 ^ 2;           // -4        (binds as -(2^2))
RETURN (-2) ^ 2         // 4`} />

## Common patterns

### Clamp a value

<QueryCodeBlock code={String.raw`WITH $raw AS raw, 0 AS lo, 100 AS hi
RETURN math.clamp(raw, lo, hi) AS clamped`} />

### Normalise to 0..1

<QueryCodeBlock code={String.raw`MATCH (m:Metric)
WITH min(m.value) AS lo, max(m.value) AS hi
MATCH (m:Metric)
RETURN m.id,
       (m.value - lo) / (hi - lo) AS normalised`} />

### Bucket a number

<QueryCodeBlock code={String.raw`MATCH (p:Product)
RETURN (p.price / 10) * 10 AS bucket, count(*) AS n
ORDER BY bucket`} />

### Log-scale bucket

<QueryCodeBlock code={String.raw`MATCH (p:Post)
WITH p, CASE WHEN p.views > 0 THEN toInteger(math.log10(p.views)) ELSE 0 END AS decade
RETURN decade, count(*) AS posts
ORDER BY decade`} />

### Exponential decay (recency score)

<QueryCodeBlock code={String.raw`MATCH (p:Post)
WITH p, temporal.between(p.published_at, temporal.now()).days AS age
RETURN p.id, p.title,
       p.views * math.exp(-age * 0.05) AS score
ORDER BY score DESC
LIMIT 20`} />

Half-life of roughly 14 days at `0.05` — tune the constant to taste.

### Percent change

<QueryCodeBlock code={String.raw`MATCH (m:Metric)
RETURN m.id,
       m.current,
       m.previous,
       CASE
         WHEN m.previous = 0 OR m.previous IS NULL THEN null
         ELSE (m.current - m.previous) * 100.0 / m.previous
       END AS pct_change`} />

Guard against zero/null — see
[`CASE`](../queries/return-with#case-expressions).

### Weighted average

<QueryCodeBlock code={String.raw`MATCH (r:Review)
RETURN sum(r.stars * r.weight) / sum(r.weight) AS weighted_mean`} />

## Edge cases

### Integer overflow

No guard — Rust panics in debug, wraps in release. For potentially
huge inputs, coerce to `Float` with
[`toFloat`](./string#type-conversion) first.

### NaN / Infinity

IEEE 754 applies. `NaN` is neither less than nor greater than any
value; `NaN == NaN` is `false`.

<QueryCodeBlock code={String.raw`RETURN 1.0 / 0.0;          // Infinity
RETURN 0.0 / 0.0;          // NaN
RETURN math.sqrt(-1)           // null   (Cypher-level domain guard, not NaN)`} />

### Integer division vs float division

<QueryCodeBlock code={String.raw`RETURN 7 / 2;       // 3
RETURN 7 / 2.0;     // 3.5
RETURN 7.0 / 2     // 3.5`} />

A single `.0` on either side flips to float division.

## Limitations

- Integer overflow is not explicitly guarded. Rust panics in debug
  builds, wraps in release. Coerce to `Float` with `toFloat` on
  potentially huge inputs.
- `NaN` and `Infinity` follow IEEE 754 — `NaN == NaN` evaluates to
  `false`, and `NaN` is neither less nor greater than any other value.
- `math.random()` / `random()` is not cryptographically secure.

## See also

- [**Scalars → Integer / Float**](../data-types/scalars#integer) — numeric types.
- [**WHERE**](../queries/where#arithmetic-in-where) — arithmetic in filters.
- [**Spatial Functions**](./spatial) — `geo.distance` as the geodesic alternative.
- [**Aggregation Functions**](./aggregation) — `sum`, `avg`, percentiles.
