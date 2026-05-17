---
title: Number Functions
sidebar_label: Number
description: Number functions in LoraDB - formatting, radix conversion, Roman numerals, integer and sign predicates, finite-value predicates, and bit helpers.
---

# Number Functions

Number functions cover value-level numeric utilities that are not really
math formulas: display formatting, base conversion, Roman numeral
interop, and predicates for validating imported data. For numeric
formulas, trigonometry, rounding, constants, and random values, use
[Math Functions](./math).

Most functions return `null` when an argument is `null`, has the wrong
type, overflows, or uses an unsupported option. That keeps ingestion
queries simple: invalid values can be filtered with `IS NOT NULL` or
replaced with `coalesce`.

## Overview

| Goal | Function |
|---|---|
| Format for display | `number.format(n[, precision[, thousands]])` |
| Convert integer to base 2..36 | `number.to_base(n, radix)` |
| Parse base 2..36 text | `number.from_base(text, radix)` |
| Roman numeral interop | `number.to_roman(n)`, `number.from_roman(text)` |
| Integer predicates | `number.is_integer(n)`, `number.is_even(n)`, `number.is_odd(n)` |
| Sign predicates | `number.is_positive(n)`, `number.is_negative(n)`, `number.is_zero(n)` |
| Float predicates | `number.is_nan(n)`, `number.is_finite(n)`, `number.is_infinite(n)` |
| Bit operations | `bits.and`, `bits.or`, `bits.xor`, `bits.not`, `bits.shift_left`, `bits.shift_right` |

## Formatting

`number.format(n[, precision[, thousands]])` returns a string. When
`precision` is provided, the number is rounded to that many decimal
places. When `thousands` is provided, it is inserted between groups of
three digits in the integer part.

<QueryCodeBlock code={String.raw`RETURN number.format(1234);             // '1234'
RETURN number.format(1234.567, 2);      // '1234.57'
RETURN number.format(1234567, 0, ',');  // '1,234,567'
RETURN number.format(-12345.6, 1, '_') // '-12_345.6'`} />

Use `math.round` when the result should remain numeric:

<QueryCodeBlock code={String.raw`RETURN math.round(1234.567, 2);         // 1234.57
RETURN number.format(1234.567, 2)      // '1234.57'`} />

## Radix Conversion

Use `number.to_base(n, radix)` and `number.from_base(text, radix)` for
integer identifiers, compact codes, imported binary or hexadecimal
fields, and similar data-shaping jobs. `radix` must be between `2` and
`36`. Output uses lowercase digits `0-9` and `a-z`.

<QueryCodeBlock code={String.raw`RETURN number.to_base(255, 16);       // 'ff'
RETURN number.to_base(42, 2);         // '101010'
RETURN number.to_base(-10, 2);        // '-1010'

RETURN number.from_base('ff', 16);    // 255
RETURN number.from_base('101010', 2); // 42
RETURN number.from_base('-1010', 2)  // -10`} />

Invalid digits, unsupported radix values, and integer overflow return
`null`:

<QueryCodeBlock code={String.raw`RETURN number.from_base('ff', 10);    // null
RETURN number.to_base(10, 1)         // null`} />

### Import Hex IDs

<QueryCodeBlock code={String.raw`UNWIND $rows AS row
WITH row, number.from_base(row.hex_id, 16) AS id
WHERE id IS NOT NULL
MERGE (:ExternalThing {id: id})`} />

## Roman Numerals

`number.to_roman(n)` supports integers from `1` through `3999`.
`number.from_roman(text)` accepts uppercase or lowercase Roman numerals
and returns an integer.

<QueryCodeBlock code={String.raw`RETURN number.to_roman(1994);        // 'MCMXCIV'
RETURN number.from_roman('MCMXCIV'); // 1994
RETURN number.to_roman(0)           // null`} />

Roman parsing is intended for interop and display cleanup, not strict
historical validation. Prefer storing ordinary integers as properties.

## Predicates

`number.is_integer(n)` accepts both `INTEGER` values and finite `FLOAT`
values with no fractional part. `number.is_even` and `number.is_odd`
require an `INTEGER`; they return `null` for floats, even when the float
looks integral. The sign predicates accept both integers and floats.

<QueryCodeBlock code={String.raw`RETURN number.is_integer(42);     // true
RETURN number.is_integer(42.0);   // true
RETURN number.is_integer(42.5);   // false

RETURN number.is_even(42);        // true
RETURN number.is_odd(41);         // true
RETURN number.is_even(42.0);      // null

RETURN number.is_positive(0.1);   // true
RETURN number.is_negative(-1);    // true
RETURN number.is_zero(0.0)       // true`} />

The finite-value predicates are mostly useful when a host binding or
JSON import can produce special floating-point values:

<QueryCodeBlock code={String.raw`RETURN number.is_finite(1.5);     // true
RETURN number.is_nan($maybe_nan) // true, when a host binding supplies NaN`} />

## Bit Operations

Bit operations live in the `bits.*` namespace because they are integer
operations over the two's-complement representation, not ordinary
numeric formulas.

<QueryCodeBlock code={String.raw`RETURN bits.and(12, 10);          // 8
RETURN bits.or(12, 10);           // 14
RETURN bits.xor(12, 10);          // 6
RETURN bits.not(0);               // -1
RETURN bits.shift_left(3, 2);     // 12
RETURN bits.shift_right(12, 2)   // 3`} />

For new queries, prefer the named `bits.*` helpers over
`number.bitop(a, op, b)`. The older `number.bitop` form remains
available for generated queries that need to pass the operation name as
a string.

<QueryCodeBlock code={String.raw`RETURN number.bitop(12, 'and', 10) // 8`} />

## Common Patterns

### Validate Imported Integers

<QueryCodeBlock code={String.raw`UNWIND $rows AS row
WITH row, cast.try(row.rank, INTEGER) AS rank
WHERE number.is_integer(rank) AND number.is_even(rank)
CREATE (:ImportedRank {rank: rank, source: row.source})`} />

### Build Stable Short Codes

<QueryCodeBlock code={String.raw`MATCH (n:Thing)
RETURN n.id, string.upper(number.to_base(n.id, 36)) AS code
ORDER BY n.id`} />

### Keep Numeric And Display Values Separate

<QueryCodeBlock code={String.raw`MATCH (invoice:Invoice)
WITH invoice, invoice.total_cents / 100.0 AS total
RETURN invoice.id,
       total,
       number.format(total, 2, ',') AS display_total`} />
