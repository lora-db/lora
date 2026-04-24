---
title: Temporal Data Types
sidebar_label: Temporal
description: LoraDB's six temporal types — Date, Time, LocalTime, DateTime, LocalDateTime, and Duration — with component access, arithmetic, truncation, and ordering semantics.
---

# Temporal Data Types

LoraDB has six first-class temporal types. They can be stored as
[node](../concepts/nodes) / [relationship](../concepts/relationships)
[properties](../concepts/properties), compared,
[ordered](../queries/ordering), and used in arithmetic with `Duration`.

| Type | Components | Timezone |
|---|---|---|
| `Date` | year, month, day | — |
| `Time` | hour, minute, second, nanosecond | UTC offset |
| `LocalTime` | hour, minute, second, nanosecond | — |
| `DateTime` | Date + Time fields | UTC offset |
| `LocalDateTime` | Date + LocalTime fields | — |
| `Duration` | months, days, seconds, nanoseconds | — |

See [Temporal Functions](../functions/temporal) for the full
constructor, [truncation](../functions/temporal#truncation), and
[arithmetic](../functions/temporal#arithmetic) reference. This page
focuses on the *types*.

## Which one do I use?

| Situation | Type |
|---|---|
| A calendar day (invoice date, birthday) | `Date` |
| An instant with offset (event timestamp, audit log) | `DateTime` |
| A wall-clock time with offset | `Time` |
| A naive local wall-clock time | `LocalTime` |
| A naive local moment (meeting at 10:00 "Amsterdam time") | `LocalDateTime` |
| A span (90 minutes, 30 days, 2 weeks) | `Duration` |

When in doubt, use `DateTime` for instants and `Date` for calendar days
— they cover most real-world needs.

## Writing temporals

### Literals via constructor

There are no bare temporal literals — always use a constructor:

```cypher
CREATE (e:Event {
  title:    'Launch',
  at:       datetime('2026-05-01T09:00:00Z'),
  day:      date('2026-05-01'),
  clock:    localtime('09:00:00'),
  runs_for: duration('PT90M')
})
```

### Component maps

```cypher
CREATE (d:Day {
  on: date({year: 2024, month: 1, day: 15})
})
```

See more in [Temporal Functions → Constructors](../functions/temporal#constructors).

### From host language

Every binding ships a helper so you don't have to write constructor
strings manually:

- Node/WASM — [typed helpers](../getting-started/node#typed-helpers)
- Python — [parameters](../getting-started/python#parameterised-query)

## Comparison and ordering

Values of the **same** temporal type are totally ordered.

```cypher
MATCH (e:Event)
WHERE e.at >= datetime() AND e.at < datetime() + duration('P7D')
RETURN e
ORDER BY e.at
```

Different temporal types are **not** cross-comparable — convert first
or compare in matching units.

## Arithmetic

- `Date + Duration` → `Date`
- `DateTime + Duration` → `DateTime`
- `DateTime - DateTime` → `Duration`

```cypher
RETURN date('2024-01-15') + duration('P30D')
-- 2024-02-14

RETURN datetime('2025-01-01T00:00:00Z') - datetime('2024-01-01T00:00:00Z')
-- P366D    (a Duration — 2024 is a leap year)
```

`Duration` is **calendar-aware**: `duration('P1M')` is "one month"
(variable length in days), not exactly 30 days. Use `duration('P30D')`
for a fixed 30-day window.

## Component access

```cypher
WITH datetime('2024-01-15T10:30:45Z') AS dt
RETURN dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second
```

For the full list see
[Temporal Functions → component access](../functions/temporal#component-access).

## Serialisation

Across host-language bindings, temporals are tagged:

| Type | Shape |
|---|---|
| `Date` | `{kind: "date", iso: "YYYY-MM-DD"}` |
| `Time` | `{kind: "time", iso: "HH:MM:SS.nnnnn+ZZ:ZZ"}` |
| `LocalTime` | `{kind: "localtime", iso: "HH:MM:SS.nnnnn"}` |
| `DateTime` | `{kind: "datetime", iso: "YYYY-MM-DDTHH:MM:SS.nnnnn+ZZ:ZZ"}` |
| `LocalDateTime` | `{kind: "localdatetime", iso: "YYYY-MM-DDTHH:MM:SS.nnnnn"}` |
| `Duration` | `{kind: "duration", iso: "P…"}` |

Use the host-language helpers (`date()`, `datetime()`, `duration()` in
each binding — see [Node](../getting-started/node#typed-helpers),
[Python](../getting-started/python#parameterised-query)) to build these values
without touching the tagged shape manually.

## Examples

### Events in the next week

```cypher
MATCH (e:Event)
WHERE e.at >= datetime() AND e.at < datetime() + duration('P7D')
RETURN e.title, e.at
ORDER BY e.at
```

### Bucketed by month

```cypher
MATCH (e:Event)
RETURN date.truncate('month', e.on) AS month, count(*) AS events
ORDER BY month
```

### Age from birthday

```cypher
MATCH (p:Person)
RETURN p.name,
       duration.inDays(p.born, date()) / 365 AS approx_age_years
```

### Duration arithmetic

```cypher
CREATE (m:Meeting {
  start: datetime('2026-05-01T09:00:00Z'),
  len:   duration('PT1H30M')
})

MATCH (m:Meeting)
RETURN m.start, m.start + m.len AS end
```

### Active in last 30 days

```cypher
MATCH (u:User)
WHERE u.last_seen >= datetime() - duration('P30D')
RETURN count(*) AS active_30d
```

### Group by year of birth

```cypher
MATCH (p:Person) WHERE p.born IS NOT NULL
RETURN p.born.year AS year, count(*) AS people
ORDER BY year
```

### Window query — past N days

```cypher
MATCH (e:Event)
WHERE e.at >= datetime() - duration({days: $days})
RETURN e
ORDER BY e.at DESC
```

Bind `$days` as an integer from the host — the `duration({days: …})`
constructor accepts a variable, unlike the ISO string form
`duration('P7D')` which must be a literal.

### Age-bracket bucketing

```cypher
MATCH (p:Person)
WITH p, duration.inDays(p.born, date()) / 365 AS age_years
RETURN CASE
         WHEN age_years < 18 THEN 'minor'
         WHEN age_years < 65 THEN 'adult'
         ELSE                     'senior'
       END AS bracket,
       count(*) AS people
ORDER BY people DESC
```

Uses [`CASE`](../queries/return-with#case-expressions) to bucket a
numeric age.

### Retention cohort

```cypher
MATCH (u:User)-[:SIGNED_UP_ON]->(d:Day)
WITH date.truncate('month', d.on) AS cohort, u
OPTIONAL MATCH (u)-[:LOGGED_IN]->(l:Login)
WHERE l.at >= datetime() - duration('P30D')
RETURN cohort,
       count(DISTINCT u)                                                 AS total,
       count(DISTINCT CASE WHEN l IS NOT NULL THEN u END)                AS active_30d
ORDER BY cohort
```

## Edge cases

### Date arithmetic on month-ends

`Duration` calendar-aware arithmetic handles month-end clamping:

```cypher
RETURN date('2024-01-31') + duration('P1M')    -- 2024-02-29
RETURN date('2024-03-31') + duration('P1M')    -- 2024-04-30
```

### Timezone-aware comparison

`DateTime` values in different offsets compare by the **same UTC
instant** — ordering is timezone-safe.

```cypher
RETURN datetime('2024-01-01T12:00:00Z') =
       datetime('2024-01-01T13:00:00+01:00')
-- true
```

### Cross-type comparison

`Date` and `DateTime` aren't directly comparable. Convert via
component reconstruction:

```cypher
MATCH (e:Event)
WHERE date({year: e.at.year, month: e.at.month, day: e.at.day}) = date('2024-01-15')
RETURN e
```

### `duration('P1M')` vs `duration('P30D')`

Calendar-aware vs fixed:

```cypher
RETURN date('2024-02-15') + duration('P1M')    -- 2024-03-15
RETURN date('2024-02-15') + duration('P30D')   -- 2024-03-16
```

### Storing as string

If you only want ISO string storage, use `String` — but then sorting
by date requires parsing on every comparison. Prefer the typed form.

## Limitations

- `date.truncate` supports only `"year"` and `"month"`.
- `datetime.truncate` supports only `"day"`, `"hour"`, `"month"`.
- Parsing is strict ISO 8601 — non-ISO shapes (`MM/DD/YYYY`,
  RFC-2822) are rejected.
- Arithmetic between different temporal types (e.g. `Date - Time`) is
  not supported.

## See also

- [**Temporal Functions**](../functions/temporal) — constructors, truncation, arithmetic.
- [**WHERE**](../queries/where) — temporal predicates and ranges.
- [**Ordering**](../queries/ordering) — chronological sort.
- [**Aggregation**](../queries/aggregation) — date-bucketed group-bys.
- [**Scalars → Integer/Float**](./scalars) — underlying component types.
