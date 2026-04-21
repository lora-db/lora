---
title: Temporal Functions (Dates, Times, Durations)
sidebar_label: Temporal
---

# Temporal Functions (Dates, Times, Durations)

LoraDB supports the full Cypher temporal model — see
[Temporal Data Types](../data-types/temporal) for the type details.
Every value is first-class: store it on a
[property](../concepts/properties), compare it, do arithmetic on it.

## Overview

| Goal | Function |
|---|---|
| Current date/time | <CypherCode code="date()" />, <CypherCode code="datetime()" />, <CypherCode code="time()" />, <CypherCode code="localtime()" />, <CypherCode code="localdatetime()" /> |
| Parse ISO string | <CypherCode code="date('…')" />, <CypherCode code="datetime('…')" />, etc. |
| From components | <CypherCode code="date({year, month, day})" />, … |
| Construct duration | <CypherCode code="duration('P…')" />, <CypherCode code="duration({days, hours, …})" /> |
| Truncate | [<CypherCode code="date.truncate" />, <CypherCode code="datetime.truncate" />](#truncation) |
| Difference | [<CypherCode code="duration.between" />, <CypherCode code="duration.inDays" />](#durationbetween--durationindays) |
| Component access | <CypherCode code="dt.year" />, <CypherCode code="dt.month" />, <CypherCode code="dt.hour" />, <CypherCode code="dur.days" /> … |
| Add/subtract | <CypherCode code="date + duration" />, <CypherCode code="datetime - datetime" /> |

## Temporal types at a glance

| Type | Components | Timezone |
|---|---|---|
| `Date` | year, month, day | — |
| `Time` | hour, minute, second, nanosecond | UTC offset |
| `LocalTime` | hour, minute, second, nanosecond | — |
| `DateTime` | Date + Time fields | UTC offset |
| `LocalDateTime` | Date + LocalTime fields | — |
| `Duration` | months, days, seconds, nanoseconds | — |

## Constructors

### date

| Form | Example |
|---|---|
| No args | <CypherCode code="date()" /> — today |
| ISO string | <CypherCode code="date('2024-01-15')" /> |
| Map | <CypherCode code="date({year: 2024, month: 1, day: 15})" /> |

```cypher
RETURN date()                                    -- today
RETURN date('2024-01-15')                        -- 2024-01-15
RETURN date({year: 2024, month: 1, day: 15})     -- 2024-01-15
```

### datetime

| Form | Example |
|---|---|
| No args | <CypherCode code="datetime()" /> |
| ISO string | <CypherCode code="datetime('2024-01-15T10:00:00Z')" /> |
| Map | <CypherCode code="datetime({year, month, day, hour, minute, second, millisecond, timezone})" /> |

```cypher
RETURN datetime('2024-01-15T10:00:00Z')
RETURN datetime({year: 2024, month: 1, day: 15, hour: 10, minute: 0})
RETURN datetime('2024-01-15T10:00:00+02:00')
```

### time / localtime / localdatetime

```cypher
RETURN time('12:34:56')                 -- with UTC offset (default Z)
RETURN time('12:34:56+02:00')
RETURN localtime('12:34:56')            -- no timezone
RETURN localdatetime('2024-01-15T10:00:00')
```

### duration

ISO 8601 string or a component map.

```cypher
RETURN duration('P30D')                         -- 30 days
RETURN duration('P1Y2M3DT4H5M6S')               -- full form
RETURN duration('PT90M')                        -- 90 minutes
RETURN duration({years: 1, months: 2, days: 3}) -- equivalent map form
```

### Constructing vs parameters

Every binding ships a helper so you don't have to write constructor
strings by hand:

```ts
// Node.js / WASM
await db.execute(
  "CREATE (:Event {at: $at, len: $len})",
  { at: datetime('2026-05-01T09:00:00Z'), len: duration('PT90M') }
);
```

See [Node → typed helpers](../getting-started/node#typed-helpers) and
[Python → parameters](../getting-started/python#parameterised-query).

## Component access

Temporal values expose components via property access.

```cypher
RETURN date('2024-01-15').year                    -- 2024
RETURN date('2024-01-15').month                   -- 1
RETURN datetime('2024-01-15T10:30:00Z').hour      -- 10
RETURN datetime('2024-01-15T10:30:45Z').second    -- 45
RETURN duration('P30D').days                      -- 30
RETURN duration('P1Y').months                     -- 12
```

Available: `.year`, `.month`, `.day`, `.hour`, `.minute`, `.second`,
`.millisecond`, `.days`, `.months`, `.years`, `.hours`, `.minutes`,
`.seconds`.

### Build a year-month key

```cypher
MATCH (e:Event)
RETURN e.at.year AS year,
       e.at.month AS month,
       count(*) AS events
ORDER BY year, month
```

## Truncation

Reduce a temporal value to a coarser unit.

| Function | Supported units |
|---|---|
| <CypherCode code="date.truncate(unit, date)" /> | `"year"`, `"month"` |
| <CypherCode code="datetime.truncate(unit, datetime)" /> | `"day"`, `"hour"`, `"month"` |

```cypher
RETURN date.truncate('month', date('2024-01-15'))       -- 2024-01-01
RETURN date.truncate('year',  date('2024-07-01'))       -- 2024-01-01
RETURN datetime.truncate('hour', datetime('2024-01-15T10:42:00Z'))
        -- 2024-01-15T10:00:00Z
```

### Bucketing rows

```cypher
MATCH (e:Event)
RETURN date.truncate('month', e.at) AS month, count(*) AS events
ORDER BY month
```

```cypher
MATCH (r:Request)
RETURN datetime.truncate('hour', r.at) AS hour, count(*) AS hits
ORDER BY hour
```

## Arithmetic

- <CypherCode code="Date + Duration" /> → `Date`
- <CypherCode code="DateTime + Duration" /> → `DateTime`
- <CypherCode code="DateTime - DateTime" /> → `Duration`

Duration arithmetic preserves calendar semantics: months and days are
stored separately from seconds.

```cypher
RETURN date('2024-01-15') + duration('P30D')
          -- 2024-02-14

RETURN datetime('2024-01-15T00:00:00Z') + duration('PT36H')
          -- 2024-01-16T12:00:00Z

RETURN datetime('2024-12-31T00:00:00Z') - datetime('2024-01-01T00:00:00Z')
          -- P365D (a Duration)
```

### Calendar vs fixed durations

`duration('P1M')` is "one month" — a variable number of days. `duration('P30D')`
is exactly 30 days.

```cypher
RETURN date('2024-01-31') + duration('P1M')     -- 2024-02-29 (leap year)
RETURN date('2024-01-31') + duration('P30D')    -- 2024-03-01
```

### duration.between / duration.inDays

```cypher
RETURN duration.between(date('2024-01-01'), date('2024-12-31'))
       -- P365D (Duration)

RETURN duration.inDays(date('2024-01-01'), date('2024-04-10'))
       -- 100
```

## Comparison

Comparable within the same type using `<`, `<=`, `>`, `>=`, `=`, `<>`.
Cross-type comparisons (e.g. `Date` vs `DateTime`) return `null`.

```cypher
MATCH (e:Event)
WHERE e.at >= datetime() AND e.at < datetime() + duration('P7D')
RETURN e
ORDER BY e.at
```

```cypher
MATCH (p:Person)
WHERE p.born < date('1900-01-01')
RETURN p.name, p.born
```

## Storing temporal values

Temporals serialise tagged: `{"kind": "date", "iso": "2024-01-15"}` etc.
(see [Temporal Data Types](../data-types/temporal#serialisation)). They
round-trip cleanly through `CREATE` and `MATCH`.

```cypher
CREATE (e:Event {
  title:    'Launch',
  at:       datetime('2026-05-01T09:00:00Z'),
  runs_for: duration('PT90M'),
  day:      date('2026-05-01')
})

MATCH (e:Event)
RETURN e.title,
       e.at,
       e.at + e.runs_for AS ends_at
```

## Common patterns

### Events in the next week

```cypher
MATCH (e:Event)
WHERE e.at >= datetime()
  AND e.at <  datetime() + duration('P7D')
RETURN e
ORDER BY e.at
```

### Events in a month

```cypher
MATCH (e:Event)
WHERE date.truncate('month', e.at) = date('2026-05-01')
RETURN e
```

### Age from birthday

```cypher
MATCH (p:Person)
RETURN p.name,
       duration.inDays(p.born, date()) / 365 AS approx_age_years
```

### Rolling 30-day active users

```cypher
MATCH (u:User)-[:VIEWED]->(:Page)
WHERE u.last_seen >= datetime() - duration('P30D')
RETURN count(DISTINCT u) AS active_30d
```

### Session length

```cypher
MATCH (s:Session)
RETURN s.id, (s.ended - s.started) AS duration
ORDER BY duration DESC
```

### First / last event per user

```cypher
MATCH (u:User)-[:DID]->(e:Event)
RETURN u.id,
       min(e.at) AS first_event,
       max(e.at) AS last_event
```

### Cohorts by signup month

```cypher
MATCH (u:User)
RETURN date.truncate('month', u.created) AS cohort,
       count(*)                           AS signups
ORDER BY cohort
```

### "Since last seen" bucket

```cypher
MATCH (u:User)
WITH u,
     duration.inDays(u.last_seen, datetime()) AS days_away
RETURN CASE
         WHEN days_away <= 1   THEN 'today'
         WHEN days_away <= 7   THEN 'week'
         WHEN days_away <= 30  THEN 'month'
         ELSE                       'dormant'
       END AS freshness,
       count(*) AS users
ORDER BY users DESC
```

Uses [`CASE`](../queries/return-with#case-expressions) to bucket a
continuous duration into named tiers.

### Time-of-day histogram

```cypher
MATCH (e:Event)
RETURN e.at.hour AS hour, count(*) AS events
ORDER BY hour
```

Component access on a `DateTime` returns integers — no string parsing
needed.

### Recurring window — "same time next week"

```cypher
MATCH (m:Meeting {id: $id})
RETURN m.start,
       m.start + duration('P7D') AS next_week,
       m.start + duration('P14D') AS two_weeks
```

### Build ISO timestamp for serialisation

```cypher
MATCH (e:Event)
RETURN e.id, toString(e.at) AS iso
```

[`toString`](./string#type-conversion) on a `DateTime` emits a
round-trippable ISO 8601 string.

## Edge cases

### Mixing types

`Date - DateTime`, `Time + Duration` — not supported. Convert first to
matching types.

### Timezone handling

`DateTime` carries a UTC offset. Compare `DateTime` values across zones
freely — they're normalised to UTC internally. `LocalDateTime` has no
zone; two `LocalDateTime` values compare by naive wall-clock order.

### Strict ISO parsing

Non-ISO shapes (`MM/DD/YYYY`, RFC-2822, ISO week-dates) are rejected at
parse time. Normalise on the host side before passing to
`date('…')` / `datetime('…')`.

### `date()` with no args — now vs wall clock

In WASM, `date()` resolves to `Date.now()` at millisecond precision —
nanosecond fields are zero. In native builds, it reflects the OS clock.
See [WASM → gotchas](../getting-started/wasm#performance--best-practices).

## Limitations

- **`date.truncate`** supports only `"year"` and `"month"` today — no
  `"quarter"`, `"week"`, or `"day"`.
- **`datetime.truncate`** supports `"day"`, `"hour"`, and `"month"` —
  no sub-hour units.
- Temporal arithmetic between values of **different** temporal types
  (e.g. `Date - Time`) is not supported. Convert first.
- Parsing is strict ISO 8601 — non-ISO shapes (`MM/DD/YYYY`,
  RFC-2822) are rejected.
- No `hour-minute-second-offset` component-access shortcuts on
  `Duration` beyond the listed fields.

## See also

- [**Temporal Data Types**](../data-types/temporal) — type reference.
- [**Scalars**](../data-types/scalars) — underlying numeric components.
- [**WHERE**](../queries/where) — temporal predicates.
- [**Ordering**](../queries/ordering) — chronological sorting.
- [**Aggregation**](./aggregation) — bucketing with `date.truncate`.
