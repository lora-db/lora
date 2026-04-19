---
title: Spatial Functions (Points, Distance)
sidebar_label: Spatial
---

# Spatial Functions (Points, Distance)

LoraDB has a `Point` [type](../data-types/spatial) with 2D and 3D
variants in both Cartesian and WGS-84 (geographic) coordinate reference
systems. Points round-trip through [property](../concepts/properties)
storage, [parameters](../queries/#parameters), and results.

## Overview

| Goal | Function |
|---|---|
| Construct a point | [<CypherCode code="point({…})" />](#constructors) |
| Distance between two points | [<CypherCode code="distance(a, b)" /> / <CypherCode code="point.distance(a, b)" />](#distance) |
| Access components | <CypherCode code="p.x" />, <CypherCode code="p.y" />, <CypherCode code="p.z" />, <CypherCode code="p.latitude" />, <CypherCode code="p.longitude" />, <CypherCode code="p.height" /> |
| SRID / CRS metadata | <CypherCode code="p.srid" />, <CypherCode code="p.crs" /> |
| Filter by radius | [<CypherCode code="distance(p, centre) < r" />](#storing-points) |

## SRIDs

| SRID | System | Components |
|---|---|---|
| `7203` | Cartesian 2D | `x`, `y` |
| `9157` | Cartesian 3D | `x`, `y`, `z` |
| `4326` | WGS-84 geographic 2D | `longitude`, `latitude` |
| `4979` | WGS-84 geographic 3D | `longitude`, `latitude`, `height` |

CRS names accepted on the `crs` key (case-insensitive): `cartesian`,
`cartesian-3D`, `WGS-84-2D`, `WGS-84-3D`. `WGS-84` is an alias for
`WGS-84-2D`. `z` is accepted as an alias for `height` in geographic
points.

## Constructors

### Cartesian

```cypher
RETURN point({x: 1, y: 2})                    -- SRID 7203
RETURN point({x: 1, y: 2, z: 3})              -- SRID 9157
RETURN point({x: 1, y: 2}, 7203)              -- explicit SRID
RETURN point({x: 1, y: 2, z: 3, crs: 'cartesian-3D'})
```

### Geographic (WGS-84)

```cypher
RETURN point({latitude: 52.37, longitude: 4.89})               -- SRID 4326
RETURN point({longitude: 4.89, latitude: 52.37, height: 20})   -- SRID 4979
RETURN point({longitude: 4.89, latitude: 52.37, z: 20})        -- also 4979 (z = height)
RETURN point({x: 4.89, y: 52.37, crs: 'WGS-84-2D'})            -- CRS promotes x/y to lon/lat
```

### Rules

- `x`/`y` may not be mixed with `longitude`/`latitude` in the same map.
- `z` and `height` may not both be present — they're aliases.
- If both `crs` and `srid` are given, they must agree.
- Any `null` coordinate → `point(…)` returns `null`.
- Unknown keys (`lon`, `elevation`, …) are rejected — no silent typos.

## distance

`distance(a, b)` — or the alias `point.distance(a, b)`.

| Same-SRID pair | Formula |
|---|---|
| Cartesian 2D | `sqrt(dx² + dy²)` |
| Cartesian 3D | `sqrt(dx² + dy² + dz²)` |
| WGS-84 2D | Haversine great-circle, Earth radius 6 371 km |
| WGS-84 3D | **Haversine surface only — height is ignored** |

```cypher
-- Cartesian 2D
RETURN distance(point({x: 0, y: 0}), point({x: 3, y: 4}))
       -- 5.0

-- Cartesian 3D
RETURN distance(point({x: 0, y: 0, z: 0}), point({x: 2, y: 3, z: 6}))
       -- 7.0

-- WGS-84 2D (metres)
RETURN distance(
  point({latitude: 52.37, longitude: 4.89}),   -- Amsterdam
  point({latitude: 51.00, longitude: 4.40})    -- Antwerp
)
       -- ≈ 155_000.0
```

`distance` on points with different SRIDs returns `null`. That covers
Cartesian-vs-geographic, 2D-vs-3D mismatches, and any custom SRID.

## Component access

| Accessor | 2D Cart | 3D Cart | WGS-84 2D | WGS-84 3D |
|---|---|---|---|---|
| `p.x` / `p.y` | ✓ | ✓ | ✓ (= lon/lat) | ✓ |
| `p.z` | `null` | ✓ | `null` | ✓ (= height) |
| `p.longitude` / `p.latitude` | `null` | `null` | ✓ | ✓ |
| `p.height` | `null` | `null` | `null` | ✓ |
| `p.srid` | ✓ | ✓ | ✓ | ✓ |
| `p.crs` | ✓ | ✓ | ✓ | ✓ |

Geographic accessors return `null` on Cartesian points by design —
they have no meaningful projection onto latitude / longitude.

```cypher
WITH point({latitude: 52.37, longitude: 4.89, height: 12}) AS p
RETURN p.latitude,  -- 52.37
       p.longitude, -- 4.89
       p.height,    -- 12
       p.srid,      -- 4979
       p.crs        -- 'WGS-84-3D'
```

## Storing points

```cypher
CREATE (c:City {
  name:     'Amsterdam',
  location: point({latitude: 52.37, longitude: 4.89})
})
```

### Nearest N cities

```cypher
MATCH (c:City {name: 'Amsterdam'})
MATCH (other:City)
WHERE other.name <> 'Amsterdam'
RETURN other.name,
       distance(c.location, other.location) AS metres
ORDER BY metres ASC
LIMIT 5
```

### Radius filter

```cypher
MATCH (v:Venue)
WHERE distance(v.location, $centre) < 1000
RETURN v
```

### Bounding-box filter

There's no `withinBBox`; compose with component access. LoraDB doesn't
support the `BETWEEN` keyword — use explicit `>=` / `<=`:

```cypher
MATCH (c:City)
WHERE c.location.latitude  >= 50 AND c.location.latitude  <= 55
  AND c.location.longitude >=  3 AND c.location.longitude <=  7
RETURN c
```

## Parameters

Pass a literal `point(…)` call, or bind the tagged map shape from your
host language (see [Node → typed helpers](../getting-started/node#typed-helpers),
[Python → parameters](../getting-started/python#b-parameterised-query)).

```cypher
MATCH (c:City)
WHERE distance(c.location, $here) < 10000
RETURN c
```

From Node/WASM:

```ts
import { wgs84 } from 'lora-node';
await db.execute(query, { here: wgs84(4.89, 52.37) });
```

## Common patterns

### Closest-first list

```cypher
MATCH (shop:Shop)
RETURN shop,
       distance(shop.location, $me) AS metres
ORDER BY metres
LIMIT 20
```

### Group by distance bucket

```cypher
MATCH (s:Station)
WITH s, toInteger(distance(s.location, $me) / 1000) AS km
RETURN km AS distance_km, count(*) AS stations
ORDER BY distance_km
```

### Count things within radius

```cypher
MATCH (b:Business)
WHERE distance(b.location, $origin) < $radius
RETURN b.category, count(*) AS n
ORDER BY n DESC
```

### Is any member of a set within range

```cypher
MATCH (u:User {id: $id})
RETURN any(
  s IN [(u)-[:OWNS]->(:Car) | s] | true   -- example placeholder
) AS owns_car
```

For pattern-based `any`, prefer
[`EXISTS { … }`](../queries/where#pattern-existence).

### Nearest-per-category

```cypher
MATCH (v:Venue)
WITH v.category AS category, v, distance(v.location, $me) AS metres
ORDER BY metres ASC
WITH category, collect({v: v, metres: metres})[0] AS nearest
RETURN category, nearest.v.name AS name, nearest.metres AS metres
ORDER BY metres
```

One nearest venue per category. The `collect(…)[0]` after `ORDER BY`
picks the first (smallest distance) within each group.

### Haversine sanity check

LoraDB's `distance` on WGS-84 uses Haversine with Earth radius 6 371
km. For two closely spaced points, Cartesian distance on
latitude/longitude is a useful local approximation:

```cypher
WITH point({latitude: 52.37, longitude: 4.89}) AS a,
     point({latitude: 52.40, longitude: 4.92}) AS b
RETURN distance(a, b)                                        AS haversine_metres,
       sqrt(power((b.latitude - a.latitude) * 111000, 2) +
            power((b.longitude - a.longitude) * 111000 *
                  cos(radians(a.latitude)), 2))              AS approx_metres
```

Expect tiny differences within a city. The Cartesian approximation
diverges quickly over continental scales — stick with `distance` for
real geodetic work.

### Project onto Cartesian for game coordinates

Cartesian 2D is perfect for canvas/game logic. Points can share an
entity with other properties:

```cypher
UNWIND range(1, 100) AS i
CREATE (:Spawn {
  id:       i,
  position: point({x: rand() * 1000, y: rand() * 1000})
})

MATCH (a:Spawn {id: 1}), (b:Spawn)
WHERE b.id <> 1
RETURN b.id, distance(a.position, b.position) AS dist
ORDER BY dist ASC
LIMIT 5
```

## Edge cases

### Cross-SRID distance

Returns `null` rather than raising:

```cypher
RETURN distance(point({x: 0, y: 0}), point({latitude: 0, longitude: 0}))
-- null
```

Detect at analysis time:

```cypher
MATCH (a:Spot), (b:Spot)
WHERE a.location.srid = b.location.srid
RETURN a, b, distance(a.location, b.location)
```

### Null coordinate

`point({latitude: null, longitude: 4.89})` returns `null`, not a point
— any downstream use propagates null.

### 3D geographic height

`distance` on WGS-84 3D points is still surface-only. For true
ellipsoidal/altitude distance, compute host-side.

### Integer vs float coordinates

Both are accepted. Integer coordinates are promoted to `Float` when
used with `distance`.

## Limitations

- **WGS-84 3D `distance` ignores `height`** — it computes surface
  great-circle distance only. A true 3D geodesic (ellipsoid + altitude)
  distance is not implemented.
- **Cross-SRID distance** returns `null`. There is no built-in CRS
  transformation.
- **`point.withinBBox()`** is not implemented.
- **`point.fromWKT()` / WKT output** is not implemented.
- No custom SRIDs — only the four listed above.

## See also

- [**Spatial Data Types**](../data-types/spatial) — type reference.
- [**Math**](./math) — underlying arithmetic for Cartesian distance.
- [**WHERE**](../queries/where) — radius filters.
- [**Ordering**](../queries/ordering) — nearest-first via `ORDER BY distance(…)`.
