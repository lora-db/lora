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
| From epoch | <CypherCode code="datetime.fromepoch(seconds, nanos)" />, <CypherCode code="datetime.fromepochmillis(ms)" /> |
| Truncate | [<CypherCode code="temporal.truncate(unit, value)" />, <CypherCode code="date.truncate(unit, value, map)" />](#truncation) |
| Difference | [<CypherCode code="duration.between(a, b)" />, <CypherCode code="duration.inDays(a, b)" />](#durationbetween), [<CypherCode code="temporal.between(a, b)" />, <CypherCode code="temporal.in_days(a, b)" />](#temporalbetween--temporalin_days) |
| Component access | <CypherCode code="dt.year" />, <CypherCode code="dt.month" />, <CypherCode code="dt.hour" />, <CypherCode code="dur.days" /> … |
| Add/subtract | <CypherCode code="date + duration" />, <CypherCode code="datetime - datetime" /> |

## Temporal types at a glance

| Type | Components | Timezone |
|---|---|---|
| `Date` | year, month, day | — |
| `Time` | hour, minute, second, nanosecond | UTC offset |
| `LocalTime` | hour, minute, second, nanosecond | — |
| `DateTime` | Date + Time fields | UTC offset, and optionally a named zone |
| `LocalDateTime` | Date + LocalTime fields | — |
| `Duration` | months, days, seconds, nanoseconds | — |

## Construction And Current Time

Construct temporal values with casts. `value::TYPE` is compact for
handwritten Cypher, while `CAST(value AS TYPE)` is also supported by the
Cypher grammar. `TRY_CAST(value AS TYPE)` returns `null` instead of
reporting a conversion error.

The Cypher constructors work as well, and build the type they are named
for. With no argument they return the current value: `date()` is today's
`DATE`, `datetime()` the current `DATETIME`, and `localdatetime()`,
`time()` and `localtime()` likewise. With one argument they are the cast:
`date(x)` is `x::DATE`, `duration(x)` is `x::DURATION`. Another temporal
keeps the components the target has, so `date(datetime())` is today's
date and `localtime(dt)` drops the offset. An argument that does not
convert is an error.

<QueryCodeBlock code={String.raw`RETURN date() AS today,
       date(datetime('2026-10-01T23:30:00+02:00')) AS day,  // 2026-10-01
       datetime('2026-10-01') AS midnight,                   // 2026-10-01T00:00:00Z
       time({hour: 9, minute: 30, timezone: '+01:00'}) AS t`} />

`date.transaction()`, `date.statement()` and `date.realtime()` (and the
same on `time`, `localtime`, `datetime` and `localdatetime`) return the
current value too. LoraDB has one clock, so the three agree.

A map gives the components. Besides `year, month, day`, a date can be an
ISO week date (`{year, week, dayOfWeek}`, where `year` is the week-based
year), an ordinal date (`{year, ordinalDay}`) or a quarter date
(`{year, quarter, dayOfQuarter}`). A clock takes `hour, minute, second,
millisecond, microsecond, nanosecond`, and the zoned types a `timezone`.
`date`, `time` and `datetime` keys start from another temporal and the
other keys override its components; a zoned value given a new `timezone`
keeps its instant. A smaller component needs the larger ones (`day`
needs `month`, `second` needs `minute`), and a key the type has no use
for is an error that names it. `datetime({epochSeconds, nanosecond})` and
`datetime({epochMillis})` build an instant from the Unix epoch.

<QueryCodeBlock code={String.raw`RETURN date({year: 1984, week: 10, dayOfWeek: 3}) AS week_date,      // 1984-03-07
       date({year: 1984, ordinalDay: 202}) AS ordinal,                  // 1984-07-20
       date({year: 1984, quarter: 3, dayOfQuarter: 45}) AS quarter,     // 1984-08-14
       date({date: date('1984-10-11'), day: 28}) AS moved,              // 1984-10-28
       datetime.fromepochmillis(1724198400000) AS instant               // 2024-08-21T00:00:00Z`} />

Strings take every ISO 8601 form, extended or basic: `2015-07-21` /
`20150721`, `2015-07`, `2015`, week dates `2015-W30-2` / `2015W302`,
ordinal dates `2015-202` / `2015202`, quarter dates `2015-Q3-21`, and
times such as `21:40:32.142+01:00`, `214032.142+0100`, `21:40` or `21`
with an offset of `Z`, `±HH:MM`, `±HHMM` or `±HH`. Fields are fixed width
(`2015-7-21` is refused), and the local types refuse an offset rather
than drop it: `localtime('12:00+01:00')` is an error.

### Time zones

A `timezone` can be an offset (`'+02:00'`, `'Z'`) or an IANA zone name
(`'Europe/Amsterdam'`, matched case-insensitively). A `DATETIME` in a
named zone keeps the zone, prints it after the offset
(`2026-07-01T12:00:00+02:00[Europe/Amsterdam]`) and parses back from
that form; a string with a zone and no offset takes the zone's offset,
and an offset the zone does not have at that moment is an error. The
zone database is built into LoraDB, so every platform, including the
browser, resolves zones the same way.

Daylight saving follows the zone's rules, as in Neo4j. A local time
that does not exist (the spring gap) moves forward by the length of the
gap, and one that happens twice (the autumn overlap) takes the earlier
offset. Adding a duration moves the calendar part (months, days) on the
wall clock and the rest (hours and smaller) on the instant, so `P1D`
keeps the time of day across a change and `PT24H` is 24 hours. A
`TIME` has no date to resolve daylight saving against, so a named zone
gives it the zone's current offset.

<QueryCodeBlock code={String.raw`RETURN datetime({year: 2026, month: 7, day: 1, hour: 12, timezone: 'Europe/Amsterdam'});
        // 2026-07-01T12:00:00+02:00[Europe/Amsterdam]
RETURN datetime('2026-03-29T02:30[Europe/Amsterdam]');      // 2026-03-29T03:30:00+02:00[Europe/Amsterdam]
RETURN datetime('2026-03-28T12:00[Europe/Amsterdam]') + duration('P1D')
        // 2026-03-29T12:00:00+02:00[Europe/Amsterdam]`} />

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

A duration map takes `years, months, weeks, days, hours, minutes,
seconds, milliseconds, microseconds, nanoseconds`, each an integer or a
float. A fraction cascades into the smaller units as in Neo4j: a month
is 30.436875 days on average.

<QueryCodeBlock code={String.raw`RETURN duration({hours: 1.5});                    // PT1H30M
RETURN duration({weeks: 2.5});                    // P17DT12H
RETURN duration({months: 0.75});                  // P22DT19H51M49.5S
RETURN duration({seconds: 1, milliseconds: 500})  // PT1.5S`} />

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

Reduce a temporal value to a coarser unit. The units are
`"millennium"`, `"century"`, `"decade"`, `"year"`, `"weekYear"` (the
Monday starting week 1 of the ISO week-based year), `"quarter"`,
`"month"`, `"week"` (the Monday of the week), `"day"`, `"hour"`,
`"minute"`, `"second"`, `"millisecond"` and `"microsecond"`. A `DATE`
takes the units down to `"day"`; a `TIME` or `LOCAL_TIME` the units from
`"day"` down.

`temporal.truncate(unit, value)` keeps the value's type and returns
`null` for a unit it cannot apply. The Cypher forms
`date.truncate(unit, value[, map])`, and likewise on `datetime`,
`localdatetime`, `time` and `localtime`, build the named type, then apply
the map's components over the result; an unknown unit or a unit the type
does not have is an error. A `timezone` in the map replaces the zone and
keeps the local time.

<QueryCodeBlock code={String.raw`RETURN date.truncate('week', date('2017-11-11'));                  // 2017-11-06
RETURN date.truncate('week', date('2017-11-11'), {dayOfWeek: 2});  // 2017-11-07
RETURN datetime.truncate('minute', datetime('2017-11-11T12:31:14Z'))
        // 2017-11-11T12:31:00Z`} />

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

### duration.between

`duration.between(a, b)` is the duration from `a` to `b` in months, then
days, then time, as in Neo4j. `duration.inMonths`, `duration.inDays` and
`duration.inSeconds` measure in one unit only, counting complete units.
The two values can be of different temporal types: a value without a
time is at midnight, with only one side zoned the other is read in the
same zone, and when either lacks a date only the times of day count.

<QueryCodeBlock code={String.raw`RETURN duration.between(date('1984-10-11'), date('1985-11-25'));   // P1Y1M14D
RETURN duration.inDays(date('1984-10-11'), date('1985-11-25'));    // P410D
RETURN duration.inSeconds(date('1984-10-11'),
                          datetime('1984-10-12T01:00:32.142+01:00'))
        // PT25H32.142S`} />

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

`DateTime` carries a UTC offset, and a named zone when it was built with
one (see [Time zones](#time-zones)). Compare `DateTime` values across
zones freely: they order by instant. Equality also compares the zone, so
the same instant at `+02:00` and in `Europe/Amsterdam` are not equal. `LocalDateTime` has no
zone; two `LocalDateTime` values compare by naive wall-clock order.

### Strict ISO parsing

Non-ISO shapes (`MM/DD/YYYY`, RFC-2822) are rejected at parse time. Normalise on the host side before passing to
`'…'::DATE` / `'…'::DATETIME`.

### `temporal.today()` with no args — now vs wall clock

In WASM, `temporal.today()` resolves to `Date.now()` at millisecond precision —
nanosecond fields are zero. In native builds, it reflects the OS clock.
See [WASM → gotchas](../getting-started/wasm#performance--best-practices).

## Limitations

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
