---
title: String Functions
sidebar_label: String
description: String functions in LoraDB — string.slice, string.replace, string.split, string.join, string.normalize, string.slugify, regex helpers, case conversion, and numeric parsing.
---

# String Functions

Every function on this page returns `null` when any argument is
`null`, unless the function explicitly documents a different fallback.
String slicing and length helpers count Unicode code points, not UTF-8
bytes. Regex helpers and `string.find` report Rust regex/string byte
offsets.

## Overview

| Goal | Function |
|---|---|
| Case conversion | [`string.lower`, `string.upper`](#tolower--toupper) |
| Case style conversion | [`string.capitalize`, `string.case`, `string.swap_case`](#case-style) |
| Trim whitespace | [`string.trim`, `string.trim_left`, `string.trim_right`](#trim--ltrim--rtrim) |
| Substring replace | [`string.replace`](#replace) |
| Count / find text | [`string.find`, `string.count`, `string.before`, `string.after`](#find-count-before-after) |
| Substring slice | [`string.slice`](#substring) |
| Left / right slice | [`string.prefix`, `string.suffix`](#left--right) |
| Split / join | [`string.split`, `string.join`, `string.words`](#split) |
| Blank check | [`string.is_blank`](#stringis_blank) |
| Reverse | [`string.reverse`](#reverse) |
| Length | [`string.length`, `value.size`](#stringlength--valuesize) |
| Pad | [`string.pad_left`, `string.pad_right`](#lpad--rpad) |
| Slug / escape / URL | [`string.slugify`, `string.escape`, `string.url_encode`, `string.url_decode`](#encoding-and-escaping) |
| Normalise Unicode | [`string.normalize`](#normalize) |
| Convert type | [`toString`, `toInteger`, `toFloat`, `toBoolean`](#type-conversion) |
| Predicate in `WHERE` | [`STARTS WITH`, `ENDS WITH`, `CONTAINS`, `=~`](#string-operators-in-where) |

## string.lower / string.upper {#tolower--toupper}

Unicode case conversion.

<QueryCodeBlock code={String.raw`RETURN string.lower('Ada LoveLace');    // 'ada lovelace'
RETURN string.upper('ada')             // 'ADA'`} />

Non-ASCII letters follow Rust's Unicode case mapping:

<QueryCodeBlock code={String.raw`RETURN string.lower('Ångström')        // 'ångström'`} />

## Case Style

Use `string.capitalize(s)` for the first character, or
`string.capitalize(s, true)` to capitalize each whitespace-delimited word.
Use `string.case(s, style)` when generating identifiers or display text.

<QueryCodeBlock code={String.raw`RETURN string.capitalize('ada lovelace');          // 'Ada lovelace'
RETURN string.capitalize('ada lovelace', true);    // 'Ada Lovelace'
RETURN string.case('hello world', 'camel');        // 'helloWorld'
RETURN string.case('helloWorld', 'snake');         // 'hello_world'
RETURN string.swap_case('LoraDB')                 // 'lORAdb'`} />

Supported `string.case` styles are `'camel'`, `'pascal'`, `'snake'`,
`'kebab'`, `'screaming_snake'` / `'constant'`, and `'title'`.

### Case-insensitive matching

<QueryCodeBlock code={String.raw`MATCH (u:User)
WHERE string.lower(u.email) = string.lower($search)
RETURN u`} />

<QueryCodeBlock code={String.raw`MATCH (u:User)
WHERE string.lower(u.name) STARTS WITH string.lower($prefix)
RETURN u`} />

## string.trim / string.trim_left / string.trim_right {#trim--ltrim--rtrim}

Strip whitespace from both ends / left / right.

<QueryCodeBlock code={String.raw`RETURN string.trim('   hi   ');    // 'hi'
RETURN string.trim_left('   hi   ');   // 'hi   '
RETURN string.trim_right('   hi   ')   // '   hi'`} />

Common for cleaning up user input before storage:

<QueryCodeBlock code={String.raw`UNWIND $rows AS row
CREATE (:Contact {email: string.lower(string.trim(row.email))})`} />

## string.replace {#replace}

`string.replace(str, find, replacement)` — replaces every occurrence.

<QueryCodeBlock code={String.raw`RETURN string.replace('banana', 'a', 'o');    // 'bonono'
RETURN string.replace('hello', 'x', 'y');     // 'hello'
RETURN string.replace('abc def', ' ', '_')   // 'abc_def'`} />

### Multi-step replace

<QueryCodeBlock code={String.raw`WITH 'Joe O\'Brien' AS raw
RETURN string.replace(string.replace(raw, ' ', '_'), '\'', '') AS slug
// 'Joe_OBrien'`} />

## string.find / string.count / string.before / string.after {#find-count-before-after}

Use these helpers when you need positions or delimiter-based extraction
inside one row.

| Function | Behaviour |
|---|---|
| `string.find(s, needle)` | First byte offset, or `-1` when missing |
| `string.find(s, needle, true)` | All byte offsets as a list |
| `string.count(s, needle)` | Non-overlapping occurrence count |
| `string.before(s, needle)` | Text before the first occurrence, or `null` |
| `string.after(s, needle)` | Text after the first occurrence, or `null` |

<QueryCodeBlock code={String.raw`RETURN string.find('banana', 'na');       // 2
RETURN string.count('banana', 'na');      // 2
RETURN string.before('a=b=c', '=');       // 'a'
RETURN string.after('a=b=c', '=');        // 'b=c'
RETURN string.after('abc', '=')          // null`} />

Regex patterns can be used with `string.count` by wrapping the pattern
in slashes:

<QueryCodeBlock code={String.raw`RETURN string.count('a1 b22 c333', '/\d+/')  // 3`} />

An empty `needle` returns `null`; pass a concrete delimiter or regex so
the count has a clear meaning.

## string.slice {#substring}

`string.slice(str, start[, length])` — 0-based indices.

<QueryCodeBlock code={String.raw`RETURN string.slice('loradb', 0, 4);   // 'lora'
RETURN string.slice('loradb', 4);      // 'db'
RETURN string.slice('hello', 1, 3)    // 'ell'`} />

Out-of-range indices return an empty string rather than an error.

<QueryCodeBlock code={String.raw`RETURN string.slice('hi', 99);         // ''
RETURN string.slice('hi', 0, 99)      // 'hi'`} />

## string.prefix / string.suffix {#left--right}

<QueryCodeBlock code={String.raw`RETURN string.prefix('graphdb', 5);     // 'graph'
RETURN string.suffix('graphdb', 2)    // 'db'`} />

Length exceeding the input returns the whole string:

<QueryCodeBlock code={String.raw`RETURN string.prefix('ab', 99)         // 'ab'`} />

## string.split / string.join / string.words {#split}

<QueryCodeBlock code={String.raw`RETURN string.split('a,b,c,d', ',');         // ['a', 'b', 'c', 'd']
RETURN string.split('one two three', ' ');   // ['one', 'two', 'three']
RETURN string.split('x', ',');               // ['x']
RETURN string.join(['red', 'green'], ', ');  // 'red, green'
RETURN string.words('  red green\tblue ')   // ['red', 'green', 'blue']`} />

Empty input returns `['']`.
`string.words` uses Unicode whitespace and drops empty fields, which is
usually what you want for tokenizing human-entered text.

### Split + UNWIND

Turn comma-separated values into rows:

<QueryCodeBlock code={String.raw`UNWIND string.split('red,green,blue', ',') AS color
CREATE (:Swatch {color: color})`} />

### Join values for display

`string.join(list, separator)` accepts strings, numbers, booleans, and
`null` values. `null` becomes an empty field.

<QueryCodeBlock code={String.raw`MATCH (u:User)
RETURN u.name, string.join(u.tags, ', ') AS tags_csv`} />

## string.reverse / value.reverse {#reverse}

Works on both strings and lists.

<QueryCodeBlock code={String.raw`RETURN string.reverse('hello');       // 'olleh'
RETURN value.reverse([1, 2, 3])      // [3, 2, 1]`} />

`reverse(x)` is a compatibility alias for `value.reverse(x)`, which
works for both strings and lists. Use `string.reverse(s)` when you want
to be explicit that the input should be text.

## string.length / value.size {#stringlength--valuesize}

| Function | Measures |
|---|---|
| `string.length(s)` | Unicode code-point count |
| `value.size(s)` / `size(s)` | Polymorphic size helper; strings return their length |
| `path.length(p)` / `length(p)` | Hop count for paths, not a string helper |

<QueryCodeBlock code={String.raw`RETURN string.length('abc');   // 3
RETURN string.length('café');  // 4
RETURN value.size('abc')      // 3`} />

For paths, use [`path.length(p)`](../queries/paths#path-functions).

## string.is_blank {#stringis_blank}

`string.is_blank(s)` returns `true` when a string is empty or contains
only whitespace. It is useful for imports where a missing field arrives
as spaces instead of `null`.

<QueryCodeBlock code={String.raw`RETURN string.is_blank('   ');       // true
RETURN string.is_blank('\t\n');      // true
RETURN string.is_blank(' data ')    // false`} />

## string.pad_left / string.pad_right {#lpad--rpad}

`string.pad_left(str, length, padding)` / `string.pad_right(str, length, padding)` — pads to
the target length using the padding character repeated.

<QueryCodeBlock code={String.raw`RETURN string.pad_left('7',   3, '0');   // '007'
RETURN string.pad_right('7',   3, '0');   // '700'
RETURN string.pad_left('abc', 5, '.');   // '..abc'
RETURN string.pad_right('abc', 5, '.')   // 'abc..'`} />

If the input is already longer than `length`, it's returned unchanged.

### Fixed-width formatting

<QueryCodeBlock code={String.raw`MATCH (r:Record)
RETURN string.pad_left(toString(r.id), 6, '0') AS padded_id`} />

## Encoding and Escaping

Use `string.slugify` for simple URL slugs, `string.escape` when embedding
text into another format, and URL helpers for query-string safe values.

<QueryCodeBlock code={String.raw`RETURN string.slugify('Hello, World! 2026');      // 'hello-world-2026'
RETURN string.escape('Tom & "Ada"', 'html');      // 'Tom &amp; &quot;Ada&quot;'
RETURN string.escape('Tom & "Ada"', 'json');      // '"Tom & \"Ada\""'
RETURN string.url_encode('a b/c');                // 'a%20b%2Fc'
RETURN string.url_decode('a%20b%2Fc')            // 'a b/c'`} />

`string.escape` supports `'json'`, `'html'`, and `'cypher'` / `'lora'`.
`string.slugify` is intentionally conservative: it lowercases ASCII
letters and collapses non-alphanumeric runs to `-`.

## string.normalize {#normalize}

`string.normalize(s[, form])` applies Unicode normalization. The default
form is `'nfc'`. Supported forms are `'nfc'`, `'nfd'`, `'nfkc'`, and
`'nfkd'`.

<QueryCodeBlock code={String.raw`RETURN string.normalize('Café');                   // 'Café'   (NFC)
RETURN string.length(string.normalize('é', 'nfd')) // 2`} />

Normalize text before storing it when users may submit visually identical
strings in different Unicode forms. Pair with `string.lower` and
`string.trim` for stable email or slug comparisons.

## Type conversion

These helpers are kept for Cypher compatibility and quick scalar
parsing. For explicit typed construction, prefer casts:
`'2024-01-15'::DATE`, `{x: 1, y: 2}::POINT`,
`[1, 2, 3]::VECTOR<INTEGER>(3)`, or `CAST(value AS TYPE)`.
Use `TRY_CAST(value AS TYPE)` when invalid input should become `null`.

| Function | Accepts | Returns |
|---|---|---|
| `toString(x)` | any | `String`; `null` → `null` |
| `toInteger(x)` / `toIntegerOrNull(x)` | `Int`, `Float` (truncates), `String`, `Bool` | `Int`; `OrNull` form returns `null` on parse failure |
| `toFloat(x)` / `toFloatOrNull(x)` | `Int`, `Float`, `String` | `Float`; `OrNull` form returns `null` on parse failure |
| `toBoolean(x)` / `toBooleanOrNull(x)` | `Bool`, `String` (`"true"`/`"false"`), `Int` (0 / non-0) | `Bool`; `OrNull` form returns `null` on parse failure |

<QueryCodeBlock code={String.raw`RETURN toString(42);              // '42'
RETURN toString(true);            // 'true'
RETURN toString('2024-01-15'::DATE);  // '2024-01-15'

RETURN toInteger('007');          // 7
RETURN toInteger(3.9);            // 3       (truncates)
RETURN toInteger(true);           // 1
RETURN toIntegerOrNull('not a number'); // null    (parse fails)

RETURN toFloat('3.14');           // 3.14
RETURN toFloat(42);               // 42.0

RETURN toBoolean('TRUE');         // true
RETURN toBoolean(0);              // false
RETURN toBooleanOrNull('maybe')  // null`} />

### Safe conversion pattern

Combine with [`coalesce`](./overview#type-conversion-and-checking) for a
default on parse failure:

<QueryCodeBlock code={String.raw`MATCH (p:Product) RETURN coalesce(toInteger(p.stock), 0) AS stock`} />

For imports where the target type is not one of the simple scalar helper
names, keep the conversion explicit:

<QueryCodeBlock code={String.raw`UNWIND $rows AS row
WITH row, TRY_CAST(row.shipped_on AS DATE) AS shipped_on
WHERE shipped_on IS NOT NULL
CREATE (:Shipment {id: row.id, shipped_on: shipped_on})`} />

## String operators (in [`WHERE`](../queries/where)) {#string-operators-in-where}

Covered in the [`WHERE`](../queries/where#string-matching) page —
included here for completeness:

| Operator | Case-sensitive | Description |
|---|---|---|
| `STARTS WITH` | yes | Prefix match |
| `ENDS WITH` | yes | Suffix match |
| `CONTAINS` | yes | Substring match |
| `=~` | yes | Regex match (Rust `regex`, RE2-style — no backreferences) |

<QueryCodeBlock code={String.raw`MATCH (u:User) WHERE u.email ENDS WITH '@loradb.com' RETURN u;
MATCH (u:User) WHERE string.lower(u.email) =~ '.*@loradb\\.com$' RETURN u;
MATCH (u:User) WHERE u.name CONTAINS 'Admin' RETURN u`} />

### Regex vs CONTAINS

Regex is more expressive but slower and strict-anchored (`=~ 'foo'`
matches only the full string `foo`). Prefer `CONTAINS` for simple
substring matches.

## Common patterns

### Slugify

<QueryCodeBlock code={String.raw`WITH 'Hello, World! 2024' AS raw
RETURN string.slugify(raw) AS slug
// 'hello-world-2024'`} />

For international slugs, normalize first and decide host-side whether to
transliterate non-ASCII characters.

### Initials

<QueryCodeBlock code={String.raw`MATCH (p:Person) WHERE p.name IS NOT NULL
RETURN p.name,
       reduce(acc = '', part IN string.split(p.name, ' ') |
              acc + string.prefix(part, 1)) AS initials`} />

### Domain from email

<QueryCodeBlock code={String.raw`MATCH (u:User) WHERE u.email CONTAINS '@'
RETURN u.email,
       string.slice(u.email, value.size(string.split(u.email, '@')[0]) + 1) AS domain`} />

### Normalise for comparison

<QueryCodeBlock code={String.raw`MATCH (u:User)
WHERE string.lower(string.trim(u.email)) = string.lower(string.trim($candidate))
RETURN u`} />

### Join a list into a string

<QueryCodeBlock code={String.raw`MATCH (u:User)
RETURN u.name, string.join(u.tags, ', ') AS tags_csv`} />

For conditional joins or per-element formatting, use
[`reduce`](./list#reduce) so each element can be transformed before it
is appended.

### Parse `key=value` pairs

<QueryCodeBlock code={String.raw`WITH 'a=1;b=2;c=3' AS s
RETURN reduce(
  m = {},
  pair IN string.split(s, ';') |
  m + {[string.split(pair, '=')[0]]: string.split(pair, '=')[1]}
) AS parsed
// {a: '1', b: '2', c: '3'}`} />

Values are strings — wrap each with
[`toInteger`](../functions/string#type-conversion) if you need numeric
types.

### Truncate for preview

<QueryCodeBlock code={String.raw`MATCH (p:Post)
RETURN p.id,
       CASE WHEN string.length(p.body) > 100
            THEN string.prefix(p.body, 97) + '...'
            ELSE p.body
       END AS preview`} />

The conditional here is a [`CASE`](../queries/return-with#case-expressions)
expression — LoraDB's ternary. See that page for the full reference.

## Limitations

- `string.lower` / `string.upper` use Unicode case mapping, not
  locale-specific case folding. Turkish dotted/dotless `i` and similar
  locale-sensitive cases should still be handled host-side when exact
  locale behavior matters.
- `string.find` returns byte offsets. Use `string.slice` for code-point
  slicing once you already know the desired character positions.
- `string.normalize` handles Unicode normalization forms, but it is not
  a locale-aware collation or fuzzy matching function.
- Use `string.length` when you need Unicode code-point counts.

## See also

- [**Scalars → String**](../data-types/scalars#string) — literal syntax and comparison.
- [**WHERE → String matching**](../queries/where#string-matching).
- [**Lists**](../data-types/lists-and-maps#lists) — `string.split` returns a list.
- [**Functions → Overview**](./overview) — `toString`, `toInteger`, etc.
