---
title: Spatial Data Types (Points)
sidebar_label: Spatial
description: The Point type in LoraDB — 2D and 3D, Cartesian and WGS-84 — its SRIDs, equality rules, property storage behaviour, and links to the spatial function library.
---

# Spatial Data Types

LoraDB has a `Point` type with 2D and 3D variants in Cartesian and
WGS-84 (geographic) coordinate systems. For the
[constructors](../functions/spatial#constructors) and the
[`distance`](../functions/spatial#distance) function, see
[Spatial Functions](../functions/spatial); this page covers the *type*.

## SRIDs

| SRID | Name | Components |
|---|---|---|
| `7203` | Cartesian 2D | `x`, `y` |
| `9157` | Cartesian 3D | `x`, `y`, `z` |
| `4326` | WGS-84 geographic 2D | `longitude`, `latitude` |
| `4979` | WGS-84 geographic 3D | `longitude`, `latitude`, `height` |

## Which one do I use?

| Domain | Type |
|---|---|
| Latitude/longitude (places on Earth) | WGS-84 2D (`4326`) |
| + elevation (flights, altitudes) | WGS-84 3D (`4979`) |
| Abstract 2D plane (games, canvas) | Cartesian 2D (`7203`) |
| 3D position (physics, CAD) | Cartesian 3D (`9157`) |

## Writing points

```cypher
CREATE (c:City {
  name: 'Amsterdam',
  location: point({latitude: 52.37, longitude: 4.89})
})

CREATE (w:Waypoint {
  mark: point({x: 1.0, y: 2.0, z: 3.0})
})
```

## Reading points

Component access is well-defined across all four SRIDs — see the table
in [Spatial Functions](../functions/spatial#component-access). In brief:

```cypher
WITH point({latitude: 52.37, longitude: 4.89}) AS p
RETURN p.x,          -- 4.89   (same as longitude on geographic)
       p.y,          -- 52.37  (same as latitude  on geographic)
       p.latitude,   -- 52.37
       p.longitude,  -- 4.89
       p.srid,       -- 4326
       p.crs         -- 'WGS-84-2D'
```

Geographic accessors return `null` on Cartesian points and vice-versa —
they have no meaningful projection.

## Comparison

Points are **not ordered** — they have no total order. They compare for
equality only, by all components including SRID.

```cypher
RETURN point({x: 1, y: 2}) = point({x: 1, y: 2})             -- true
RETURN point({x: 1, y: 2}) = point({x: 1, y: 2, z: 0})       -- false (different SRID)
RETURN point({latitude: 0, longitude: 0}) = point({x: 0, y: 0})
                                                              -- false (different CRS)
```

For "within distance" filtering, use `distance`:

```cypher
MATCH (c:City)
WHERE distance(c.location, $here) < 10000
RETURN c
```

## Serialisation

Points serialise as tagged maps. In JS / TS / Python bindings, prefer
the built-in helpers (`cartesian`, `wgs84`, `cartesian3d`, `wgs84_3d`)
over writing the tagged shape by hand.

| Variant | Tagged shape |
|---|---|
| Cartesian 2D | `{kind: "point", srid: 7203, crs: "cartesian", x, y}` |
| Cartesian 3D | `{kind: "point", srid: 9157, crs: "cartesian-3D", x, y, z}` |
| WGS-84 2D | `{kind: "point", srid: 4326, crs: "WGS-84-2D", x, y, longitude, latitude}` |
| WGS-84 3D | `{kind: "point", srid: 4979, crs: "WGS-84-3D", x, y, z, longitude, latitude, height}` |

HTTP-server responses use a compact property-style form
(`{srid, x, y[, z]}`); the JS / Python bindings apply the tagged
`kind:"point"` wrapper on the way out.

## Examples

### Five nearest cities

```cypher
MATCH (c:City {name: 'Amsterdam'})
MATCH (other:City) WHERE other.name <> 'Amsterdam'
RETURN other.name,
       distance(c.location, other.location) AS metres
ORDER BY metres ASC
LIMIT 5
```

### Filter by bounding box

There's no `withinBBox`; compose with component access. LoraDB doesn't
support the `BETWEEN` keyword — use explicit `>=` / `<=`:

```cypher
MATCH (c:City)
WHERE c.location.latitude  >= 50 AND c.location.latitude  <= 55
  AND c.location.longitude >=  3 AND c.location.longitude <=  7
RETURN c
```

### 3D Cartesian distance

```cypher
CREATE (p:Anchor {pos: point({x: 0, y: 0, z: 0})})
CREATE (q:Anchor {pos: point({x: 3, y: 4, z: 12})})

MATCH (a:Anchor), (b:Anchor) WHERE id(a) < id(b)
RETURN distance(a.pos, b.pos)
-- 13.0   (sqrt(9 + 16 + 144))
```

### Group venues by kilometre ring

```cypher
MATCH (v:Venue)
WITH v, toInteger(distance(v.location, $centre) / 1000) AS km
RETURN km, count(*) AS venues
ORDER BY km
```

### Join on proximity

```cypher
MATCH (a:Spot), (b:Spot)
WHERE id(a) < id(b)
  AND distance(a.location, b.location) < 500
RETURN a.name, b.name
```

### Store a home location and match nearby

```cypher
-- Home stored as WGS-84 2D
MATCH (me:User {id: $id})
MATCH (p:Place)
WHERE distance(p.location, me.home) < 5000
RETURN p
ORDER BY distance(p.location, me.home)
LIMIT 10
```

### Bounding-box over a set of points

No `envelope` helper — aggregate the components directly:

```cypher
MATCH (c:City)
RETURN min(c.location.longitude) AS w,
       max(c.location.longitude) AS e,
       min(c.location.latitude)  AS s,
       max(c.location.latitude)  AS n
```

### Cluster by rounded coordinates

A poor man's grid clustering, useful for quick heatmaps:

```cypher
MATCH (s:Sensor)
WITH round(s.location.latitude  * 10) / 10 AS lat,
     round(s.location.longitude * 10) / 10 AS lon,
     count(*) AS n
WHERE n > 5
RETURN lat, lon, n
ORDER BY n DESC
```

0.1° grid ≈ 11 km at the equator. Adjust the multiplier for the grid
you want.

### Two-step filter — cheap first, distance second

Property filters with a label scope run fast (see
[Limitations](../limitations#storage)). Narrow with predicates first,
compute `distance` after:

```cypher
MATCH (c:City)
WHERE c.country = $country                -- cheap label + prop filter
  AND c.location.latitude  >= $s AND c.location.latitude  <= $n
  AND c.location.longitude >= $w AND c.location.longitude <= $e
WITH c, distance(c.location, $centre) AS metres
WHERE metres < $radius
RETURN c
ORDER BY metres
```

LoraDB has no `BETWEEN` keyword — use explicit `>=` / `<=`. See
[Limitations → Operators](../limitations#operators-and-expressions).

## Edge cases

### Null coordinate

Any `null` coordinate makes the constructor return `null`:

```cypher
RETURN point({latitude: null, longitude: 4.89})   -- null
```

### Cross-SRID operations

`distance` with mismatched SRIDs returns `null`, not an error:

```cypher
RETURN distance(point({x: 0, y: 0}), point({latitude: 0, longitude: 0}))
-- null
```

Detect at filter time:

```cypher
MATCH (a:Loc), (b:Loc)
WHERE a.loc.srid = b.loc.srid
RETURN distance(a.loc, b.loc)
```

### Component access on wrong SRID

Returns `null` — never raises.

```cypher
WITH point({x: 1, y: 2}) AS p
RETURN p.latitude         -- null (Cartesian has no latitude)
```

### Float precision

WGS-84 coordinates are `Float` (IEEE 754). Don't rely on exact equality
between computed points — use `distance(a, b) < epsilon` instead.

## Limitations

- **WGS-84 3D `distance` ignores `height`** — surface great-circle only.
- **Cross-SRID `distance`** returns `null` (no CRS transforms).
- **No `withinBBox` / WKT I/O** — build those with component access or
  in the host language.
- **No custom SRIDs** — only the four listed above.
- **`BETWEEN`** keyword is not supported — use `>=` / `<=`.

## See also

- [**Spatial Functions**](../functions/spatial) — constructors and `distance`.
- [**Math**](../functions/math) — Cartesian trig/arithmetic building blocks.
- [**WHERE**](../queries/where) — radius / bbox filters.
- [**Ordering**](../queries/ordering) — nearest-first sort.
