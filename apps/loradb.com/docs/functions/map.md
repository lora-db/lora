---
title: Map Functions
sidebar_label: Map
description: Map functions in LoraDB — lookup, projection, patching, shallow and deep merge, nested paths, entries, flattening, grouping, and index-building helpers for MAP values.
---

# Map Functions

[Maps](../data-types/lists-and-maps#maps) are ordered by key when they
are returned as values, which makes function output deterministic in
tests and API responses. Map functions return `null` when the input is
not a map or a required key argument is not a string.

## Overview

| Goal | Function |
|---|---|
| Lookup | `map.get`, `map.has_key` |
| Shape projection | `map.pick`, `map.keys`, `map.values`, `map.entries` |
| Patching | `map.set`, `map.remove`, `map.rename`, `map.merge`, `map.deep_merge`, `map.compact` |
| Construct maps | `map.from` |
| Nested map paths | `map.get_path`, `map.set_path`, `map.remove_path`, `map.flatten`, `map.unflatten` |
| Row-shaped list helpers | `map.group_by`, `map.index_by` |
| Reverse lookup | `map.invert` |
| Size | `map.size`, `value.size` |

## Lookup

Use `map.get(map, key[, default])` when a missing key should not be
confused with a stored `null` value. Use `map.has_key` when presence is
the question.

```cypher
RETURN map.get({name: 'Ada'}, 'name')              -- 'Ada'
RETURN map.get({name: 'Ada'}, 'missing')           -- null
RETURN map.get({name: 'Ada'}, 'missing', 'n/a')    -- 'n/a'

RETURN map.has_key({a: 1, b: null}, 'b')           -- true
RETURN map.has_key({a: 1}, 'b')                    -- false
```

Dot lookup remains the shortest form when a missing key can simply
become `null`:

```cypher
WITH {name: 'Ada'} AS person
RETURN person.name
```

## Projection

`map.pick(map, keys)` keeps only the requested keys that exist. The
returned map is still sorted by key in output.

```cypher
RETURN map.pick({id: 1, name: 'Ada', email: 'ada@example.test'}, ['id', 'name'])
-- {id: 1, name: 'Ada'}
```

Introspection helpers expose a map as lists:

```cypher
RETURN map.keys({c: 1, a: 2, b: 3})       -- ['a', 'b', 'c']
RETURN map.values({a: 1, b: 2})           -- [1, 2]
RETURN map.values({a: 1, b: 2}, ['b'])    -- [2]
RETURN map.entries({a: 1, b: 2})          -- [['a', 1], ['b', 2]]
RETURN map.size({a: 1, b: 2})             -- 2
```

## Patching

All patching helpers return a new map value; they do not mutate stored
properties by themselves. To persist a patched map, assign it in `SET`.

```cypher
RETURN map.set({a: 1}, 'b', 2)             -- {a: 1, b: 2}
RETURN map.remove({a: 1, b: 2}, 'a')       -- {b: 2}
RETURN map.rename({first: 'Ada'}, 'first', 'name')
-- {name: 'Ada'}
RETURN map.compact({a: 1, b: null, c: 3})  -- {a: 1, c: 3}
```

`map.remove` also accepts a list of keys:

```cypher
RETURN map.remove({a: 1, b: 2, c: 3}, ['a', 'c'])
-- {b: 2}
```

## Merge

`map.merge(left, right[, strategy])` combines two maps.

| Strategy | Behaviour |
|---|---|
| `'right'` | Default. Values from `right` replace values from `left`. |
| `'left'` | Existing values from `left` win. |
| `'error'` | Returns `null` if both maps contain the same key. |

```cypher
RETURN map.merge({a: 1}, {a: 9, b: 2})           -- {a: 9, b: 2}
RETURN map.merge({a: 1}, {a: 9, b: 2}, 'left')   -- {a: 1, b: 2}
RETURN map.merge({a: 1}, {a: 9}, 'error')        -- null
```

`map.deep_merge(left, right[, strategy])` uses the same conflict
strategies, but when both sides contain a map at the same key it merges
those nested maps recursively.

```cypher
RETURN map.deep_merge(
  {user: {name: 'Ada', flags: {admin: false}}},
  {user: {email: 'ada@example.test', flags: {admin: true}}}
)
-- {user: {email: 'ada@example.test', flags: {admin: true}, name: 'Ada'}}
```

Use `map.merge` for flat patches where replacement is exactly what you
want. Use `map.deep_merge` for JSON-like configuration and payload
maps where nested keys should survive partial updates.

## Constructing Maps

`map.from` builds a map from alternating key/value lists, pair lists, or
parallel key and value lists. Keys must be strings.

```cypher
RETURN map.from(['a', 1, 'b', 2])                 -- {a: 1, b: 2}
RETURN map.from([['a', 1], ['b', 2]])             -- {a: 1, b: 2}
RETURN map.from(['a', 'b'], [1, 2])               -- {a: 1, b: 2}
```

## Flattening

For direct nested lookup and patching, use path helpers. A path can be a
dotted string or a list of string keys. Missing reads return `null`, or
the optional default value when one is supplied.

```cypher
RETURN map.get_path({user: {name: 'Ada'}}, 'user.name')
-- 'Ada'

RETURN map.get_path({user: {name: 'Ada'}}, ['user', 'email'], 'n/a')
-- 'n/a'

RETURN map.set_path({user: {name: 'Ada'}}, 'user.email', 'ada@example.test')
-- {user: {email: 'ada@example.test', name: 'Ada'}}

RETURN map.remove_path({user: {name: 'Ada', email: 'a@x'}}, 'user.email')
-- {user: {name: 'Ada'}}
```

`map.set_path` creates missing intermediate maps. If an intermediate
path segment exists but is not a map, it is replaced with a map so the
requested nested value can be written.

For shape conversion, `map.flatten` turns nested maps into dotted keys.
`map.unflatten` reverses the operation. Pass a separator string when `.`
is not a good fit for your data.

```cypher
RETURN map.flatten({user: {name: 'Ada', age: 36}, active: true})
-- {active: true, user.age: 36, user.name: 'Ada'}

RETURN map.unflatten(map.flatten({user: {name: 'Ada'}}))
-- {user: {name: 'Ada'}}
```

## Grouping And Indexing

These helpers operate on a list of maps, which is a common shape after
collecting rows or decoding JSON.

```cypher
WITH [
  {id: 'u1', team: 'eng', name: 'Ada'},
  {id: 'u2', team: 'eng', name: 'Grace'},
  {id: 'u3', team: 'ops', name: 'Lin'}
] AS rows
RETURN map.group_by(rows, 'team') AS by_team,
       map.index_by(rows, 'id') AS by_id
```

`map.group_by` returns a map from key value to list of matching maps.
`map.index_by` returns a map from key value to the last matching map.
Both stringify the grouping value, so integers, booleans, and strings
can all be used as group keys.

## Inverting

`map.invert(map)` creates a reverse lookup from each value to its key.
Values are stringified, and duplicate stringified values keep the last
key in sorted map order.

```cypher
RETURN map.invert({draft: 0, published: 1})
-- {'0': 'draft', '1': 'published'}
```

## See Also

- [**Lists and Maps**](../data-types/lists-and-maps) — literals,
  property access, and dynamic keys.
- [**List Functions**](./list) — helpers for lists of maps.
- [**JSON Functions**](./overview#categories) — `json.decode` commonly
  returns nested maps.
