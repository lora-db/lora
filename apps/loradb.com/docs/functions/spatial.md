---
title: Spatial Functions (Points, Distance)
sidebar_label: Spatial
description: Spatial functions in LoraDB — cast-based POINT construction, 2D/3D Cartesian and WGS-84 distance, bounding boxes, property round-tripping, and parameter binding.
---

# Spatial Functions (Points, Distance)

LoraDB has a `Point` [type](../data-types/spatial) with 2D and 3D
variants in both Cartesian and WGS-84 (geographic) coordinate reference
systems. Points round-trip through [property](../concepts/properties)
storage, [parameters](../queries/parameters), and results.

## Overview

| Goal | Function |
|---|---|
| Construct a point | [<CypherCode code="{…}::POINT" />](#construction) |
| Distance between two points | [<CypherCode code="geo.distance(a, b)" />](#geodistance) |
| Bounding-box test | [<CypherCode code="geo.within_bbox(p, ll, ur)" />](#geowithin_bbox) |
| Access components | <CypherCode code="p.x" />, <CypherCode code="p.y" />, <CypherCode code="p.z" />, <CypherCode code="p.latitude" />, <CypherCode code="p.longitude" />, <CypherCode code="p.height" /> |
| SRID / CRS metadata | <CypherCode code="p.srid" />, <CypherCode code="p.crs" /> |
| Filter by radius | [<CypherCode code="geo.distance(p, centre) < r" />](#storing-points) |

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

## Construction

Construct points by casting a map to `POINT`. `value::POINT` is compact
for handwritten Cypher, and `CAST(value AS POINT)` is also part of the
Cypher grammar.

### Cartesian

```cypher
RETURN {x: 1, y: 2}::POINT                    -- SRID 7203
RETURN {x: 1, y: 2, z: 3}::POINT              -- SRID 9157
RETURN {x: 1, y: 2, srid: 7203}::POINT        -- explicit SRID
RETURN {x: 1, y: 2, z: 3, crs: 'cartesian-3D'}::POINT
RETURN CAST({x: 1, y: 2} AS POINT)            -- CAST form
```

### Geographic (WGS-84)

```cypher
RETURN {latitude: 52.37, longitude: 4.89}::POINT               -- SRID 4326
RETURN {longitude: 4.89, latitude: 52.37, height: 20}::POINT   -- SRID 4979
RETURN {longitude: 4.89, latitude: 52.37, z: 20}::POINT        -- also 4979 (z = height)
RETURN {x: 4.89, y: 52.37, crs: 'WGS-84-2D'}::POINT            -- CRS promotes x/y to lon/lat
```

### Rules

- `x`/`y` may not be mixed with `longitude`/`latitude` in the same map.
- `z` and `height` may not both be present — they're aliases.
- If both `crs` and `srid` are given, they must agree.
- Any `null` coordinate → `{…}::POINT` returns `null`.
- Unknown keys (`lon`, `elevation`, …) are rejected — no silent typos.

## geo.distance

`geo.distance(a, b)`.

| Same-SRID pair | Formula |
|---|---|
| Cartesian 2D | `math.sqrt(dx² + dy²)` |
| Cartesian 3D | `math.sqrt(dx² + dy² + dz²)` |
| WGS-84 2D | Haversine great-circle, Earth radius 6 371 km |
| WGS-84 3D | **Haversine surface only — height is ignored** |

```cypher
-- Cartesian 2D
RETURN geo.distance({x: 0, y: 0}::POINT, {x: 3, y: 4}::POINT)
       -- 5.0

-- Cartesian 3D
RETURN geo.distance({x: 0, y: 0, z: 0}::POINT, {x: 2, y: 3, z: 6}::POINT)
       -- 7.0

-- WGS-84 2D (metres)
RETURN geo.distance(
  {latitude: 52.37, longitude: 4.89}::POINT,   -- Amsterdam
  {latitude: 51.00, longitude: 4.40}::POINT    -- Antwerp
)
       -- ≈ 155_000.0
```

`geo.distance` on points with different SRIDs returns `null`. That covers
Cartesian-vs-geographic, 2D-vs-3D mismatches, and any custom SRID.

## geo.within_bbox

`geo.within_bbox(p, lowerLeft, upperRight)` returns `true` when `p`
falls inside the closed bounding box formed by the two corner points.
All three points must share an SRID. For 3D points, all three must carry
the third coordinate; mixed 2D/3D inputs return `null`.

```cypher
MATCH (v:Venue)
WHERE geo.within_bbox(
  v.location,
  {longitude: 4.7, latitude: 52.2}::POINT,
  {longitude: 5.1, latitude: 52.5}::POINT
)
RETURN v
```

POINT indexes can accelerate bounding-box and radius predicates when
the query is scoped to a matching label or relationship type:

```cypher
CREATE POINT INDEX venue_location FOR (v:Venue) ON (v.location)
MATCH (v:Venue)
WHERE geo.within_bbox(v.location, $southwest, $northeast)
RETURN v
```

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
WITH {latitude: 52.37, longitude: 4.89, height: 12}::POINT AS p
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
  location: {latitude: 52.37, longitude: 4.89}::POINT
})
```

### Nearest N cities

```cypher
MATCH (c:City {name: 'Amsterdam'})
MATCH (other:City)
WHERE other.name <> 'Amsterdam'
RETURN other.name,
       geo.distance(c.location, other.location) AS metres
ORDER BY metres ASC
LIMIT 5
```

### Radius filter

```cypher
MATCH (v:Venue)
WHERE geo.distance(v.location, $centre) < 1000
RETURN v
```

### Bounding-box filter

```cypher
MATCH (c:City)
WHERE geo.within_bbox(
  c.location,
  {longitude: 3, latitude: 50}::POINT,
  {longitude: 7, latitude: 55}::POINT
)
RETURN c
```

## Parameters

Pass a literal `{…}::POINT` cast, or bind the tagged point value from your
host language (see [Node → typed helpers](../getting-started/node#typed-helpers),
[Python → parameters](../getting-started/python#parameterised-query)).

```cypher
MATCH (c:City)
WHERE geo.distance(c.location, $here) < 10000
RETURN c
```

From Node/WASM:

```ts
import { wgs84 } from '@loradb/lora-node';
await db.execute(query, { here: wgs84(4.89, 52.37) });
```

## Common patterns

### Closest-first list

```cypher
MATCH (shop:Shop)
RETURN shop,
       geo.distance(shop.location, $me) AS metres
ORDER BY metres
LIMIT 20
```

### Group by distance bucket

```cypher
MATCH (s:Station)
WITH s, toInteger(geo.distance(s.location, $me) / 1000) AS km
RETURN km AS distance_km, count(*) AS stations
ORDER BY distance_km
```

### Count things within radius

```cypher
MATCH (b:Business)
WHERE geo.distance(b.location, $origin) < $radius
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
WITH v.category AS category, v, geo.distance(v.location, $me) AS metres
ORDER BY metres ASC
WITH category, collect({v: v, metres: metres})[0] AS nearest
RETURN category, nearest.v.name AS name, nearest.metres AS metres
ORDER BY metres
```

One nearest venue per category. The `collect(…)[0]` after `ORDER BY`
picks the first (smallest distance) within each group.

### Haversine sanity check

LoraDB's `geo.distance` on WGS-84 uses Haversine with Earth radius 6 371
km. For two closely spaced points, Cartesian distance on
latitude/longitude is a useful local approximation:

```cypher
WITH {latitude: 52.37, longitude: 4.89}::POINT AS a,
     {latitude: 52.40, longitude: 4.92}::POINT AS b
RETURN geo.distance(a, b)                                        AS haversine_metres,
       math.sqrt(math.pow((b.latitude - a.latitude) * 111000, 2) +
            math.pow((b.longitude - a.longitude) * 111000 *
                  math.cos(math.radians(a.latitude)), 2))              AS approx_metres
```

Expect tiny differences within a city. The Cartesian approximation
diverges quickly over continental scales — stick with `geo.distance` for
real geodetic work.

### Project onto Cartesian for game coordinates

Cartesian 2D is perfect for canvas/game logic. Points can share an
entity with other properties:

```cypher
UNWIND list.range(1, 100) AS i
CREATE (:Spawn {
  id:       i,
  position: {x: math.random() * 1000, y: math.random() * 1000}::POINT
})

MATCH (a:Spawn {id: 1}), (b:Spawn)
WHERE b.id <> 1
RETURN b.id, geo.distance(a.position, b.position) AS dist
ORDER BY dist ASC
LIMIT 5
```

## Edge cases

### Cross-SRID distance

Returns `null` rather than raising:

```cypher
RETURN geo.distance({x: 0, y: 0}::POINT, {latitude: 0, longitude: 0}::POINT)
-- null
```

Detect at analysis time:

```cypher
MATCH (a:Spot), (b:Spot)
WHERE a.location.srid = b.location.srid
RETURN a, b, geo.distance(a.location, b.location)
```

### Null coordinate

`{latitude: null, longitude: 4.89}::POINT` returns `null`, not a point
— any downstream use propagates null.

### 3D geographic height

`geo.distance` on WGS-84 3D points is still surface-only. For true
ellipsoidal/altitude distance, compute host-side.

### Integer vs float coordinates

Both are accepted. Integer coordinates are promoted to `Float` when
used with `geo.distance`.

## Limitations

- **WGS-84 3D `geo.distance` ignores `height`** — it computes surface
  great-circle distance only. A true 3D geodesic (ellipsoid + altitude)
  distance is not implemented.
- **Cross-SRID distance** returns `null`. There is no built-in CRS
  transformation.
- **No WKT I/O or CRS transforms.** Convert WKT host-side and keep all
  compared points in the same SRID.
- **`point.fromWKT()` / WKT output** is not implemented.
- No custom SRIDs — only the four listed above.

## See also

- [**Spatial Data Types**](../data-types/spatial) — type reference.
- [**Math**](./math) — underlying arithmetic for Cartesian distance.
- [**WHERE**](../queries/where) — radius filters.
- [**Ordering**](../queries/ordering) — nearest-first via `ORDER BY geo.distance(…)`.
