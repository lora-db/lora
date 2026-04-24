---
title: String Functions
sidebar_label: String
description: String functions in LoraDB — substring, replace, split, trim, case conversion, numeric parsing — with null propagation and ASCII case semantics.
---

# String Functions

Every function on this page returns `null` when any of its arguments is
`null`. Case operations are **ASCII-only**; `normalize` is a no-op
placeholder.

## Overview

| Goal | Function |
|---|---|
| Case conversion | [`toLower`, `toUpper`](#tolower--toupper) |
| Trim whitespace | [`trim`, `lTrim`, `rTrim`](#trim--ltrim--rtrim) |
| Substring replace | [`replace`](#replace) |
| Substring slice | [`substring`](#substring) |
| Left / right slice | [`left`, `right`](#left--right) |
| Split by delimiter | [`split`](#split) |
| Reverse | [`reverse`](#reverse) |
| Length | [`size`, `length`, `charLength`](#size--length--charlength) |
| Pad | [`lpad`, `rpad`](#lpad--rpad) |
| Normalise (NFC) | [`normalize`](#normalize) (placeholder) |
| Convert type | [`toString`, `toInteger`, `toFloat`, `toBoolean`](#type-conversion) |
| Predicate in `WHERE` | [`STARTS WITH`, `ENDS WITH`, `CONTAINS`, `=~`](#string-operators-in-where) |

## toLower / toUpper

ASCII case conversion.

```cypher
RETURN toLower('Ada LoveLace')    -- 'ada lovelace'
RETURN toUpper('ada')             -- 'ADA'
```

Non-ASCII letters pass through unchanged:

```cypher
RETURN toLower('Ångström')        -- 'Ångström'   (non-ASCII untouched)
```

### Case-insensitive matching

```cypher
MATCH (u:User)
WHERE toLower(u.email) = toLower($search)
RETURN u
```

```cypher
MATCH (u:User)
WHERE toLower(u.name) STARTS WITH toLower($prefix)
RETURN u
```

## trim / lTrim / rTrim

Strip whitespace from both ends / left / right.

```cypher
RETURN trim('   hi   ')    -- 'hi'
RETURN lTrim('   hi   ')   -- 'hi   '
RETURN rTrim('   hi   ')   -- '   hi'
```

Common for cleaning up user input before storage:

```cypher
UNWIND $rows AS row
CREATE (:Contact {email: toLower(trim(row.email))})
```

## replace

`replace(str, find, replacement)` — replaces every occurrence.

```cypher
RETURN replace('banana', 'a', 'o')    -- 'bonono'
RETURN replace('hello', 'x', 'y')     -- 'hello'
RETURN replace('abc def', ' ', '_')   -- 'abc_def'
```

### Multi-step replace

```cypher
WITH 'Joe O\'Brien' AS raw
RETURN replace(replace(raw, ' ', '_'), '\'', '') AS slug
-- 'Joe_OBrien'
```

## substring

`substring(str, start[, length])` — 0-based indices.

```cypher
RETURN substring('loradb', 0, 4)   -- 'lora'
RETURN substring('loradb', 4)      -- 'db'
RETURN substring('hello', 1, 3)    -- 'ell'
```

Out-of-range indices return an empty string rather than an error.

```cypher
RETURN substring('hi', 99)         -- ''
RETURN substring('hi', 0, 99)      -- 'hi'
```

## left / right

```cypher
RETURN left('graphdb', 5)     -- 'graph'
RETURN right('graphdb', 2)    -- 'db'
```

Length exceeding the input returns the whole string:

```cypher
RETURN left('ab', 99)         -- 'ab'
```

## split

```cypher
RETURN split('a,b,c,d', ',')         -- ['a', 'b', 'c', 'd']
RETURN split('one two three', ' ')   -- ['one', 'two', 'three']
RETURN split('x', ',')               -- ['x']
```

Empty input returns `['']`.

### Split + UNWIND

Turn comma-separated values into rows:

```cypher
UNWIND split('red,green,blue', ',') AS color
CREATE (:Swatch {color: color})
```

## reverse

Works on both strings and lists.

```cypher
RETURN reverse('hello')       -- 'olleh'
RETURN reverse([1, 2, 3])     -- [3, 2, 1]
```

## size / length / charLength

| Function | Measures |
|---|---|
| `size(s)` | Length of the string (bytes for ASCII-only; code units otherwise) |
| `length(s)` | Alias for `size` on strings; also accepts paths |
| `charLength(s)` | Unicode code-point count |

```cypher
RETURN size('abc')           -- 3
RETURN length('abc')         -- 3
RETURN charLength('café')    -- 4
```

`length` also accepts paths — see
[Paths → path functions](../queries/paths#path-functions).

## lpad / rpad

`lpad(str, length, padding)` / `rpad(str, length, padding)` — pads to
the target length using the padding character repeated.

```cypher
RETURN lpad('7',   3, '0')   -- '007'
RETURN rpad('7',   3, '0')   -- '700'
RETURN lpad('abc', 5, '.')   -- '..abc'
RETURN rpad('abc', 5, '.')   -- 'abc..'
```

If the input is already longer than `length`, it's returned unchanged.

### Fixed-width formatting

```cypher
MATCH (r:Record)
RETURN lpad(toString(r.id), 6, '0') AS padded_id
```

## normalize

Placeholder for Unicode NFC normalisation. Today it returns the input
unchanged.

```cypher
RETURN normalize('café')   -- 'café'   (no NFC applied)
```

If you need real NFC normalisation, apply it host-side before writing.

## Type conversion

| Function | Accepts | Returns |
|---|---|---|
| `toString(x)` | any | `String`; `null` → `null` |
| `toInteger(x)` / `toInt(x)` | `Int`, `Float` (truncates), `String`, `Bool` | `Int` or `null` on parse failure |
| `toFloat(x)` | `Int`, `Float`, `String` | `Float` or `null` on parse failure |
| `toBoolean(x)` / `toBooleanOrNull(x)` | `Bool`, `String` (`"true"`/`"false"`), `Int` (0 / non-0) | `Bool` or `null` on parse failure |

```cypher
RETURN toString(42)              -- '42'
RETURN toString(true)            -- 'true'
RETURN toString(date('2024-01-15'))  -- '2024-01-15'

RETURN toInteger('007')          -- 7
RETURN toInteger(3.9)            -- 3       (truncates)
RETURN toInteger(true)           -- 1
RETURN toInteger('not a number') -- null    (parse fails)

RETURN toFloat('3.14')           -- 3.14
RETURN toFloat(42)               -- 42.0

RETURN toBoolean('TRUE')         -- true
RETURN toBoolean(0)              -- false
RETURN toBoolean('maybe')        -- null
```

### Safe conversion pattern

Combine with [`coalesce`](./overview#type-conversion-and-checking) for a
default on parse failure:

```cypher
MATCH (p:Product) RETURN coalesce(toInteger(p.stock), 0) AS stock
```

## String operators (in [`WHERE`](../queries/where)) {#string-operators-in-where}

Covered in the [`WHERE`](../queries/where#string-matching) page —
included here for completeness:

| Operator | Case-sensitive | Description |
|---|---|---|
| `STARTS WITH` | yes | Prefix match |
| `ENDS WITH` | yes | Suffix match |
| `CONTAINS` | yes | Substring match |
| `=~` | yes | Regex match (Rust `regex`, RE2-style — no backreferences) |

```cypher
MATCH (u:User) WHERE u.email ENDS WITH '@loradb.com' RETURN u
MATCH (u:User) WHERE toLower(u.email) =~ '.*@loradb\\.com$' RETURN u
MATCH (u:User) WHERE u.name CONTAINS 'Admin' RETURN u
```

### Regex vs CONTAINS

Regex is more expressive but slower and strict-anchored (`=~ 'foo'`
matches only the full string `foo`). Prefer `CONTAINS` for simple
substring matches.

## Common patterns

### Slugify

```cypher
WITH 'Hello, World! 2024' AS raw
RETURN toLower(replace(replace(raw, ',', ''), ' ', '-')) AS slug
-- 'hello--world!-2024'
```

Not a full slugifier — punctuation survives. For real slugs, normalise
host-side.

### Initials

```cypher
MATCH (p:Person) WHERE p.name IS NOT NULL
RETURN p.name,
       reduce(acc = '', part IN split(p.name, ' ') |
              acc + left(part, 1)) AS initials
```

### Domain from email

```cypher
MATCH (u:User) WHERE u.email CONTAINS '@'
RETURN u.email,
       substring(u.email, size(split(u.email, '@')[0]) + 1) AS domain
```

### Normalise for comparison

```cypher
MATCH (u:User)
WHERE toLower(trim(u.email)) = toLower(trim($candidate))
RETURN u
```

### Join a list into a string

There's no `join` function. Use
[`reduce`](./list#reduce):

```cypher
MATCH (u:User)
RETURN u.name,
       reduce(out = '', t IN u.tags |
              CASE WHEN out = '' THEN t ELSE out + ', ' + t END
       ) AS tags_csv
```

### Parse `key=value` pairs

```cypher
WITH 'a=1;b=2;c=3' AS s
RETURN reduce(
  m = {},
  pair IN split(s, ';') |
  m + {[split(pair, '=')[0]]: split(pair, '=')[1]}
) AS parsed
-- {a: '1', b: '2', c: '3'}
```

Values are strings — wrap each with
[`toInteger`](../functions/string#type-conversion) if you need numeric
types.

### Truncate for preview

```cypher
MATCH (p:Post)
RETURN p.id,
       CASE WHEN size(p.body) > 100
            THEN left(p.body, 97) + '...'
            ELSE p.body
       END AS preview
```

The conditional here is a [`CASE`](../queries/return-with#case-expressions)
expression — LoraDB's ternary. See that page for the full reference.

## Limitations

- `toLower` / `toUpper` are **ASCII-only**. For Unicode case folding,
  normalise inputs host-side before passing them into LoraDB.
- `normalize` is a placeholder — it does not apply Unicode NFC today.
- String indexing is byte-based inside `size` and `length`. Use
  `charLength` when you need Unicode code-point counts.

## See also

- [**Scalars → String**](../data-types/scalars#string) — literal syntax and comparison.
- [**WHERE → String matching**](../queries/where#string-matching).
- [**Lists**](../data-types/lists-and-maps#lists) — `split` returns a list.
- [**Functions → Overview**](./overview) — `toString`, `toInteger`, etc.
