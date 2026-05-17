---
title: Temporal Functions (Dates, Times, Durations)
sidebar_label: Temporal
description: Temporal functions in LoraDB — current-time helpers, cast-based temporal construction, component accessors, truncation, and Duration arithmetic.
---

# Temporal Functions (Dates, Times, Durations)

LoraDB supports the Cypher temporal model end-to-end — see
[Temporal Data Types](../data-types/temporal) for the type details.
Each value is first-class: store it as a
[property](../concepts/properties), compare it, do arithmetic on it.

## Overview

| Goal | Function |
|---|---|
| Current date/time | <CypherCode code="temporal.today()" />, <CypherCode code="temporal.now('date')" />, <CypherCode code="temporal.now()" /> / <CypherCode code="now()" />, <CypherCode code="temporal.now('time')" />, <CypherCode code="temporal.now('local_time')" />, <CypherCode code="temporal.now('local_datetime')" /> |
| Parse ISO string | <CypherCode code="'…'::DATE" />, <CypherCode code="'…'::DATETIME" />, etc. |
| From components | <CypherCode code="{year, month, day}::DATE" />, … |
| Construct duration | <CypherCode code="'P…'::DURATION" />, <CypherCode code="{days, hours, …}::DURATION" /> |
| Truncate | [<CypherCode code="temporal.truncate(unit, value)" />](#truncation) |
| Difference | [<CypherCode code="temporal.between(a, b)" />, <CypherCode code="temporal.in_days(a, b)" />](#temporalbetween--temporalin_days) |
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

## Construction And Current Time

Construct temporal values with casts. `value::TYPE` is compact for
handwritten Cypher, while `CAST(value AS TYPE)` is also supported by the
Cypher grammar. `TRY_CAST(value AS TYPE)` returns `null` instead of
reporting a conversion error.

The zero-argument current-value helpers also have bare aliases:
<CypherCode code="now()" /> for <CypherCode code="temporal.now()" />,
<CypherCode code="timestamp()" /> for
<CypherCode code="temporal.timestamp()" />, and
<CypherCode code="timezone()" /> for
<CypherCode code="temporal.timezone()" />.

There are two separate jobs here:

- **Current-time helpers** create a value from the database clock.
- **Casts** create or convert a value from query text, maps, parameters,
  or other expressions.

Avoid wrapping an already-cast value in an old constructor-shaped helper.
For example, write `$value::DATETIME`, not `datetime($value::DATETIME)`
or `temporal.datetime($value::DATETIME)`.

### Current-time helpers

| Helper | Returns | Use when |
|---|---|---|
| <CypherCode code="temporal.today()" /> | `DATE` | You need the current calendar day. |
| <CypherCode code="temporal.now('date')" /> | `DATE` | Equivalent current-day form when the kind is parameterized. |
| <CypherCode code="temporal.now()" /> / <CypherCode code="now()" /> | `DATETIME` | You need the current instant with timezone offset. |
| <CypherCode code="temporal.now('time')" /> | `TIME` | You need only the current time-of-day with offset. |
| <CypherCode code="temporal.now('local_time')" /> | `LOCAL_TIME` | You need a wall-clock time without timezone. |
| <CypherCode code="temporal.now('local_datetime')" /> | `LOCAL_DATETIME` | You need date and wall-clock time without timezone. |
| <CypherCode code="temporal.timestamp()" /> / <CypherCode code="timestamp()" /> | `INTEGER` | You need Unix epoch milliseconds. |
| <CypherCode code="temporal.timezone()" /> / <CypherCode code="timezone()" /> | `STRING` | You need the database timezone label, currently `UTC`. |

Use `temporal.now()` for stored instants such as `created_at` and
`updated_at`. Use `temporal.today()` for date-only concepts such as
birthdays, billing days, and cohort dates. Use local variants only when
the value is intentionally a wall-clock value rather than an absolute
instant.

### Date

| Form | Example |
|---|---|
| Current day | <CypherCode code="temporal.today()" /> |
| ISO string | <CypherCode code="'2024-01-15'::DATE" /> |
| Map | <CypherCode code="{year: 2024, month: 1, day: 15}::DATE" /> |
| CAST form | <CypherCode code="CAST('2024-01-15' AS DATE)" /> |

<QueryCodeBlock code={String.raw`RETURN temporal.today();                         // today
RETURN '2024-01-15'::DATE;                       // 2024-01-15
RETURN {year: 2024, month: 1, day: 15}::DATE;    // 2024-01-15
RETURN TRY_CAST($maybe_date AS DATE)            // null on invalid input`} />

### DateTime

| Form | Example |
|---|---|
| Current instant | <CypherCode code="temporal.now()" /> / <CypherCode code="now()" /> |
| ISO string | <CypherCode code="'2024-01-15T10:00:00Z'::DATETIME" /> |
| Map | <CypherCode code="{year, month, day, hour, minute, second, millisecond, timezone}::DATETIME" /> |
| Local current instant | <CypherCode code="temporal.now('local_datetime')" /> |

<QueryCodeBlock code={String.raw`RETURN '2024-01-15T10:00:00Z'::DATETIME;
RETURN {year: 2024, month: 1, day: 15, hour: 10, minute: 0}::DATETIME;
RETURN '2024-01-15T10:00:00+02:00'::DATETIME`} />

### Time / LocalTime / LocalDateTime

<QueryCodeBlock code={String.raw`RETURN '12:34:56'::TIME;                 // with UTC offset (default Z)
RETURN '12:34:56+02:00'::TIME;
RETURN '12:34:56'::LOCAL_TIME;           // no timezone
RETURN '2024-01-15T10:00:00'::LOCAL_DATETIME;
RETURN temporal.now('time');
RETURN temporal.now('local_time');
RETURN temporal.now('local_datetime')`} />

### duration

ISO 8601 string or a component map.

<QueryCodeBlock code={String.raw`RETURN 'P30D'::DURATION;                         // 30 days
RETURN 'P1Y2M3DT4H5M6S'::DURATION;               // full form
RETURN 'PT90M'::DURATION;                        // 90 minutes
RETURN {years: 1, months: 2, days: 3}::DURATION; // equivalent map form
RETURN CAST('PT90M' AS DURATION)                // CAST form`} />

### Query casts vs parameters

Every binding ships a helper so you can pass typed values in
host-language parameter maps without writing query casts:

```ts
// Node.js / WASM
import { datetime, duration } from "@loradb/lora-node";

await db.execute(
  "CREATE (:Event {at: $at, len: $len})",
  { at: datetime("2026-05-01T09:00:00Z"), len: duration("PT90M") }
);
```

See [Node → typed helpers](../getting-started/node#typed-helpers) and
[Python → parameters](../getting-started/python#parameterised-query).

## Component access

Temporal values expose components via property access.

<QueryCodeBlock code={String.raw`RETURN '2024-01-15'::DATE.year;                    // 2024
RETURN '2024-01-15'::DATE.month;                   // 1
RETURN '2024-01-15T10:30:00Z'::DATETIME.hour;      // 10
RETURN '2024-01-15T10:30:45Z'::DATETIME.second;    // 45
RETURN 'P30D'::DURATION.days;                      // 30
RETURN 'P1Y'::DURATION.months                     // 12`} />

Available: `.year`, `.month`, `.day`, `.hour`, `.minute`, `.second`,
`.millisecond`, `.days`, `.months`, `.years`, `.hours`, `.minutes`,
`.seconds`.

### Build a year-month key

<QueryCodeBlock code={String.raw`MATCH (e:Event)
RETURN e.at.year AS year,
       e.at.month AS month,
       count(*) AS events
ORDER BY year, month`} />

## Truncation

Reduce a temporal value to a coarser unit.

| Function | Supported units |
|---|---|
| <CypherCode code="temporal.truncate(unit, date)" /> | `"year"`, `"month"` |
| <CypherCode code="temporal.truncate(unit, datetime)" /> | `"day"`, `"hour"`, `"month"` |

<QueryCodeBlock code={String.raw`RETURN temporal.truncate('month', '2024-01-15'::DATE);       // 2024-01-01
RETURN temporal.truncate('year',  '2024-07-01'::DATE);       // 2024-01-01
RETURN temporal.truncate('hour', '2024-01-15T10:42:00Z'::DATETIME)
        // 2024-01-15T10:00:00Z`} />

### Bucketing rows

<QueryCodeBlock code={String.raw`MATCH (e:Event)
RETURN temporal.truncate('month', e.at) AS month, count(*) AS events
ORDER BY month`} />

<QueryCodeBlock code={String.raw`MATCH (r:Request)
RETURN temporal.truncate('hour', r.at) AS hour, count(*) AS hits
ORDER BY hour`} />

## Arithmetic

- <CypherCode code="Date + Duration" /> → `Date`
- <CypherCode code="DateTime + Duration" /> → `DateTime`
- <CypherCode code="DateTime - DateTime" /> → `Duration`

Duration arithmetic preserves calendar semantics: months and days are
stored separately from seconds.

<QueryCodeBlock code={String.raw`RETURN '2024-01-15'::DATE + 'P30D'::DURATION
;          // 2024-02-14

RETURN '2024-01-15T00:00:00Z'::DATETIME + 'PT36H'::DURATION
;          // 2024-01-16T12:00:00Z

RETURN '2024-12-31T00:00:00Z'::DATETIME - '2024-01-01T00:00:00Z'::DATETIME
          // P365D (a Duration)`} />

### Calendar vs fixed durations

`'P1M'::DURATION` is "one month" — a variable number of days. `'P30D'::DURATION`
is exactly 30 days.

<QueryCodeBlock code={String.raw`RETURN '2024-01-31'::DATE + 'P1M'::DURATION;     // 2024-02-29 (leap year)
RETURN '2024-01-31'::DATE + 'P30D'::DURATION    // 2024-03-01`} />

### temporal.between / temporal.in_days

<QueryCodeBlock code={String.raw`RETURN temporal.between('2024-01-01'::DATE, '2024-12-31'::DATE)
;       // P365D (Duration)

RETURN temporal.in_days('2024-01-01'::DATE, '2024-04-10'::DATE)
       // 100`} />

`temporal.in_days` is for `DATE` values. For `DATETIME` values, use
`temporal.between(a, b).days` when you need the day component.

## Comparison

Comparable within the same type using `<`, `<=`, `>`, `>=`, `=`, `<>`.
Cross-type comparisons (e.g. `Date` vs `DateTime`) return `null`.

<QueryCodeBlock code={String.raw`MATCH (e:Event)
WHERE e.at >= temporal.now() AND e.at < temporal.now() + 'P7D'::DURATION
RETURN e
ORDER BY e.at`} />

<QueryCodeBlock code={String.raw`MATCH (p:Person)
WHERE p.born < '1900-01-01'::DATE
RETURN p.name, p.born`} />

## Storing temporal values

Temporals serialise tagged: `{"kind": "date", "iso": "2024-01-15"}` etc.
(see [Temporal Data Types](../data-types/temporal#serialisation)). They
round-trip cleanly through `CREATE` and `MATCH`.

<QueryCodeBlock code={String.raw`CREATE (e:Event {
  title:    'Launch',
  at:       '2026-05-01T09:00:00Z'::DATETIME,
  runs_for: 'PT90M'::DURATION,
  day:      '2026-05-01'::DATE
});

MATCH (e:Event)
RETURN e.title,
       e.at,
       e.at + e.runs_for AS ends_at`} />

## Common patterns

### Events in the next week

<QueryCodeBlock code={String.raw`MATCH (e:Event)
WHERE e.at >= temporal.now()
  AND e.at <  temporal.now() + 'P7D'::DURATION
RETURN e
ORDER BY e.at`} />

### Events in a month

<QueryCodeBlock code={String.raw`MATCH (e:Event)
WHERE temporal.truncate('month', e.at) = '2026-05-01'::DATE
RETURN e`} />

### Age from birthday

<QueryCodeBlock code={String.raw`MATCH (p:Person)
RETURN p.name,
       temporal.in_days(p.born, temporal.today()) / 365 AS approx_age_years`} />

### Rolling 30-day active users

<QueryCodeBlock code={String.raw`MATCH (u:User)-[:VIEWED]->(:Page)
WHERE u.last_seen >= temporal.now() - 'P30D'::DURATION
RETURN count(DISTINCT u) AS active_30d`} />

### Session length

<QueryCodeBlock code={String.raw`MATCH (s:Session)
RETURN s.id, (s.ended - s.started) AS duration
ORDER BY duration DESC`} />

### First / last event per user

<QueryCodeBlock code={String.raw`MATCH (u:User)-[:DID]->(e:Event)
RETURN u.id,
       min(e.at) AS first_event,
       max(e.at) AS last_event`} />

### Cohorts by signup month

<QueryCodeBlock code={String.raw`MATCH (u:User)
RETURN temporal.truncate('month', u.created) AS cohort,
       count(*)                           AS signups
ORDER BY cohort`} />

### "Since last seen" bucket

<QueryCodeBlock code={String.raw`MATCH (u:User)
WITH u,
     temporal.between(u.last_seen, temporal.now()).days AS days_away
RETURN CASE
         WHEN days_away <= 1   THEN 'today'
         WHEN days_away <= 7   THEN 'week'
         WHEN days_away <= 30  THEN 'month'
         ELSE                       'dormant'
       END AS freshness,
       count(*) AS users
ORDER BY users DESC`} />

Uses [`CASE`](../queries/return-with#case-expressions) to bucket a
continuous duration into named tiers.

### Time-of-day histogram

<QueryCodeBlock code={String.raw`MATCH (e:Event)
RETURN e.at.hour AS hour, count(*) AS events
ORDER BY hour`} />

Component access on a `DateTime` returns integers — no string parsing
needed.

### Recurring window — "same time next week"

<QueryCodeBlock code={String.raw`MATCH (m:Meeting {id: $id})
RETURN m.start,
       m.start + 'P7D'::DURATION AS next_week,
       m.start + 'P14D'::DURATION AS two_weeks`} />

### Build ISO timestamp for serialisation

<QueryCodeBlock code={String.raw`MATCH (e:Event)
RETURN e.id, e.at::STRING AS iso`} />

`CAST(e.at AS STRING)` / `e.at::STRING` on a `DateTime` emits a
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
`'…'::DATE` / `'…'::DATETIME`.

### `temporal.today()` with no args — now vs wall clock

In WASM, `temporal.today()` resolves to `Date.now()` at millisecond precision —
nanosecond fields are zero. In native builds, it reflects the OS clock.
See [WASM → gotchas](../getting-started/wasm#performance--best-practices).

## Limitations

- **`temporal.truncate`** supports `"year"` and `"month"` for `DATE`
  values; `"day"`, `"hour"`, and `"month"` for `DATETIME` values.
- Arithmetic between values of **different** temporal types
  (e.g. `Date - Time`) is not supported. Convert first.
- Parsing is strict ISO 8601 — non-ISO shapes (`MM/DD/YYYY`,
  RFC-2822) are rejected.
- No component-access shortcuts on `Duration` beyond the listed
  fields.

## See also

- [**Temporal Data Types**](../data-types/temporal) — type reference.
- [**Scalars**](../data-types/scalars) — underlying numeric components.
- [**WHERE**](../queries/where) — temporal predicates.
- [**Ordering**](../queries/ordering) — chronological sorting.
- [**Aggregation**](./aggregation) — bucketing with `temporal.truncate`.
