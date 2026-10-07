---
title: lora-graphql CLI
sidebar_label: CLI
description: Every lora-graphql command (print, access, directives, requirements, check, compile, analyze, diff and migrate neo4j), with its flags, output and exit codes, and how to run them in CI.
---

# The lora-graphql CLI

The package installs a `lora-graphql` binary for the build-time half of
the library: reviewing the public API, printing DDL, gating CI on plans,
compiling persisted operations and diffing schemas.

```bash
npx lora-graphql <command> [arguments]
```

| Command | Does | Needs a database |
| --- | --- | --- |
| [`print`](#print) | The public SDL clients see | No |
| [`access`](#access) | Who may do what, per type, field and kind of caller | No |
| [`directives`](#directives) | The directive definitions, for editors | No |
| [`requirements`](#requirements) | Constraints and indexes the API needs | No |
| [`check`](#check) | CI gate: model, lint, `@cypher` statements and plans | In-memory, or `--database` |
| [`compile`](#compile) | Persisted-operation manifest and TypeScript types | No |
| [`analyze`](#analyze) | Statistics JSON for `useStatistics()` | `--database` |
| [`diff`](#diff) | Database statements and API changes between two SDLs | No |
| [`migrate neo4j`](#migrate-neo4j) | Rewrite an `@neo4j/graphql` SDL | No |

`check` and `analyze` open a LoraDB, so they need `@loradb/lora-node`
installed (a dev dependency is enough). The other commands work
offline.

**Exit codes.** `0` on success. `1` when a check fails, a diff has
breaking changes or a destructive database statement, the SDL is invalid,
or a file cannot be read. `2` for a usage error or an unknown command,
with the usage text on standard error. There is no `--help` flag: run
`lora-graphql` with no arguments for the usage. Flags a command does not
know are ignored, so check the spelling when one seems to have no effect.

An invalid SDL prints the same `ModelError` the constructor throws, with
every problem located:

```text
Invalid LoraGraphQL type definitions (2 problems):
  - Festival.capacity: Int does not support CONTAINS (allowed: EQ, IN, LT, LTE, GT, GTE, IS_NULL)
  - Festival.top: the statement uses $nope, which is neither an argument nor $jwt
```

## print

```bash
lora-graphql print schema.graphql
```

Prints the generated API as SDL, sorted, without model directives:
exactly what clients can query. Review it before shipping, and commit it
if you want API changes to show up in code review.

## access

```bash
lora-graphql access schema.graphql [--json]
```

Prints who may do what: for every type and guarded field, each operation
as each kind of caller, with the verdict and the rules that decide it.

```text
Post           READ    anonymous      filtered         filter[0], filter[1], filter[2], filter[3]
Post           READ    roles:admin    allowed          filter[0], filter[1], filter[2], filter[3]
Post           CREATE  anonymous      unauthenticated  @authentication, validate[0]
Post.royalties READ    authenticated  validated        validate[0]
```

Principals are `anonymous`, `authenticated` (a token with no roles), and
one per claim value the rules test with `includes`, `eq` or `in`. Verdicts
are `allowed`, `filtered` (only rows a filter admits), `validated`
(checked per row, `FORBIDDEN` on failure), `masked`, `denied` and
`unauthenticated`. The output is stable: commit it, or the `--json` form,
and review access changes as diffs. The same list comes from
[`lora.accessMatrix()`](./api-reference#accessmatrix).

## directives

```bash
lora-graphql directives > directives.graphql
```

Prints the directive definitions. Point your editor's GraphQL extension
or codegen at the file so the annotated SDL validates while you write
it. The same text is exported as `directiveTypeDefs`.

## requirements

```bash
lora-graphql requirements schema.graphql [--ddl]
```

Lists every constraint and index the API needs, with the reason:

```text
NODE_KEY constraint on :Festival(key), because Festival.key is the @key
NOT_NULL constraint on :Festival(name), because Festival.name is non-null and @sortable
RANGE index on :Festival(name), because Festival.name is @sortable
TEXT index on :Festival(name), because Festival.name is @filterable by CONTAINS
RANGE index on :Festival(capacity), because Festival.capacity is @filterable by GTE, LT
```

With `--ddl` it prints the statements instead, ready for a migration
file:

```text
CREATE CONSTRAINT `festival_key_key` IF NOT EXISTS FOR (n:`Festival`) REQUIRE n.`key` IS NODE KEY;
CREATE CONSTRAINT `festival_name_exists` IF NOT EXISTS FOR (n:`Festival`) REQUIRE n.`name` IS NOT NULL;
CREATE INDEX `festival_name_range` IF NOT EXISTS FOR (n:`Festival`) ON (n.`name`);
CREATE TEXT INDEX `festival_name_text` IF NOT EXISTS FOR (n:`Festival`) ON (n.`name`);
CREATE INDEX `festival_capacity_range` IF NOT EXISTS FOR (n:`Festival`) ON (n.`capacity`);
```

Every statement is `IF NOT EXISTS`, so the file is safe to apply
repeatedly. It is the offline equivalent of
`lora.assertSchema({ create: true })`.

## check

```bash
lora-graphql check schema.graphql \
  [--operations <file|dir>]... \
  [--variables vars.json] \
  [--context ctx.json] \
  [--baseline plans.json [--update-baseline]] \
  [--row-budget <n>] \
  [--database <dir> [--name <db>]] \
  [--json]
```

The CI gate. By default it opens an in-memory LoraDB, creates everything
the API needs, and then:

1. validates the model and prints its warnings;
2. prints lint notes, and fails on `@mutation` types whose generated writes
   no rule guards;
3. plans every `@cypher` statement with the engine;
4. compiles every **query** in the operation files and plans each
   statement, reporting any that does not seek the way it was compiled
   to.

It exits `1` on any error, and `0` otherwise:

```text
lint     Doc: filter[0] holds for every authenticated caller: it only keeps out anonymous ones (use @authentication for that)
lint     Festival.name: CASE_INSENSITIVE compares lowercased values and cannot use an index: every such filter scans the label
ok       2 root field(s) in 2 operation(s) seek as expected
```

| Line prefix | Meaning | Fails the run |
| --- | --- | --- |
| `warning` | A model warning, for example an unused `@cypher` argument | No |
| `lint` | A valid but costly or risky choice | No |
| `unused` | An index in the database that the API does not need | No |
| `error` | An unguarded `@mutation` type, a missing requirement, a rejected `@cypher` statement, an operation that does not compile, a plan finding, or a plan that differs from the baseline | Yes |

An unguarded `@mutation` type is one with a generated write (`CREATE`,
`UPDATE`, `DELETE`) that no `@authentication`, `@authorization` rule or
`@authorizationDefaults(mutations:)` default covers: any caller that
reaches the API could make it. Declare an intended one with
`@authorization(public: [CREATE, ...])`.

The authorization lints (they do not fail the run) flag:

- a filter rule every signed-in caller passes, which only keeps out
  anonymous callers;
- a rule that needs no claims, as a whole or in one `OR` branch, on a type
  or a field, in a schema that leaves
  `@authorizationDefaults(requireAuthentication:)` unset: anonymous
  callers are refused it by default, so say which was meant;
- once that setting is `false`, each write rule (CREATE, UPDATE, DELETE,
  CONNECT, DISCONNECT, UPDATE_EDGE) a signed-out caller can pass and no
  `@authentication` covers; READ rules and operations a type lists in
  `public` are left alone;
- field rules the schema's bypass skips for the callers passing it;
- every type the schema's bypass reaches without saying `bypass: true` or
  `bypass: false`, in one line.

### Operations

`--operations` takes a `.graphql` or `.gql` file or a directory (searched
recursively) and can be repeated. Each query in a file is checked on its
own, with the fragments it spreads. Operations are named
`<path>#<OperationName>` in the output, or `<path>#<n>` when unnamed.
Mutations and subscriptions are skipped: mutation statements need the
data they write against.

Required variables get sample values (`"x"`, `1`, `true`,
`"2026-01-01"`, the first enum value, and so on). When a sample would not
exercise the right plan, give real values with `--variables`: a JSON
object keyed by operation name, either the full `<path>#<Name>` or just
`<Name>`:

```json title="vars.json"
{
  "TopFestivals": { "min": 1000 },
  "src/operations/search.graphql#ByName": { "q": "land" }
}
```

### Contexts

Rules are compiled into statements, so the plan an operation gets depends
on who runs it. `--context ctx.json` gives the GraphQL context per
operation name, with `*` for every other operation:

```json title="ctx.json"
{
  "*": { "jwt": { "sub": "u1" } },
  "AdminReport": { "jwt": { "sub": "a1", "roles": ["admin"] } }
}
```

Without it, operations are planned as an anonymous caller.
`check({ operations })` takes the same as `context` on each operation.

### Plan baselines

```bash
lora-graphql check schema.graphql --operations src/operations --baseline plans.json
```

`--baseline` records the plan operators of every statement. When the
file exists, any statement whose plan differs fails the run, so a plan
change (from a schema change, an engine upgrade or a new filter) shows up
in review. `--update-baseline` accepts the new plans; a missing file is
written on the first run. Commit the file.

### Row budget

`--row-budget <n>` fails statements whose largest engine row estimate
exceeds `n`. It is off by default because estimates ignore a `LIMIT` that
stops an index-ordered scan early, so a well-planned
`festivals(sort: [{ name: ASC }], limit: 10)` can estimate the whole
label.

### Checking an existing database

```bash
lora-graphql check schema.graphql --database ./data --name app --operations src/operations
```

With `--database`, `check` opens that database (`--name` defaults to
`app`) and checks it **as it is**: it creates nothing, reports missing
requirements as errors, and lists indexes the API does not use. Plans
then reflect the real data. The database must not be open in another
process.

### JSON output

`--json` prints the full [check report](/docs/graphql/api-reference#checkoptions)
plus `ok` and `planChanges` as JSON, for tools that post results to a
pull request.

## compile

```bash
lora-graphql compile schema.graphql --operations src/operations [--out generated]
```

Validates persisted operations at build time and writes two files to
`--out` (default `lora-graphql/`):

- `manifest.json`: every operation as a validated AST, with the public
  schema's hash. Load it with `lora.loadManifest(manifest)`.
- `operations.d.ts`: `<Operation>Variables` and `<Operation>Result`
  interfaces for each operation.

```text
wrote 2 operation(s) to generated/manifest.json and generated/operations.d.ts
```

Inputs are `.graphql` files, directories, or JSON files mapping id to
source. **Operation ids** are:

- for `.graphql` files, `<path relative to the working directory>#<OperationName>`
  (or `#<n>` when unnamed). Running from the repository root with
  `--operations src/operations` gives ids like
  `src/operations/festivals.graphql#TopFestivals`;
- for JSON files, the keys of the map.

Clients send the id; the server runs
`lora.execute({ id, variables, context })`. If you want short ids, write
a JSON map instead of `.graphql` files.

Any invalid operation fails the command with every problem listed. A
manifest built for a different schema is refused by `loadManifest()`, so
a stale build fails at startup rather than at the first request.

The generated types for a query look like this:

```ts
export interface TopFestivalsVariables {
  min: number;
}

export interface TopFestivalsResult {
  festivals: Array<{
    key: string;
    name: string;
    genre: {
      name: string;
    } | null;
  }>;
}
```

## analyze

```bash
lora-graphql analyze schema.graphql --database ./data [--name app] [--sample 1000] > stats.json
```

Counts the nodes of every `@node` type and measures the degree of every
list relationship over the first `--sample` nodes of each type. Load the
output at startup:

```ts
lora.useStatistics(JSON.parse(await readFile("stats.json", "utf8")));
```

Cost estimates then use each relationship's measured maximum degree
instead of the page size. Rerun it when the shape of the data changes. See
[statistics and cost](/docs/graphql/smart-layer#s6-statistics-and-cost).

## diff

```bash
lora-graphql diff old.graphql new.graphql [--allow-breaking] [--json]
lora-graphql diff --base <git-ref> <file|dir>... [--allow-breaking] [--json]
```

Compares two SDLs and prints what the database and the API need:

```text
# Database
// destructive: no longer needed: Festival.name is @filterable by CONTAINS
DROP INDEX `festival_name_text` IF EXISTS;
// Festival.seats is @filterable by GTE
CREATE INDEX `festival_seats_range` IF NOT EXISTS FOR (n:`Festival`) ON (n.`seats`);

# API
breaking   Festival.capacity was removed.
breaking   FestivalNameFilter.contains was removed.
dangerous  An optional field seats on input type FestivalWhere was added.
```

Database statements are in the order to run them, and destructive ones
(a dropped constraint or index, a relabel, a moved property) are flagged.
It exits `1` when the API has breaking changes or the database needs a
destructive statement, so it can gate a pull request; `--allow-breaking`
accepts both, for example when the pull request is labelled for it.
`--json` prints the full `SchemaDiff`. Things the diff cannot act on, such
as a removed type whose nodes stay in the database, are printed as `note`
lines, and identical schemas print `no changes`.

`--base <git-ref>` compares the schema at a git ref with the working tree,
so a schema split over many files needs no script:

```bash
lora-graphql diff --base origin/main src/schema
```

It takes files or directories (searched recursively for `.graphql` and
`.gql`), concatenates each side in path order, and compares the two. A ref
that has none of the paths has nothing to compare and exits `0`; a ref
that is not a commit is an error.

With `graphql` 17 installed the API comparison is unavailable (that
version removed the function it relies on) and is reported as a single
breaking `UNSUPPORTED` change. Run `diff` with `graphql` 16 in CI.

Renaming a field is a remove plus an add. To rename the API field while
keeping the stored property, use `@alias(property:)`: the diff then
reports an API break and no data migration.

## migrate neo4j

```bash
lora-graphql migrate neo4j schema.graphql [--operations <file|dir>]... > schema.lora.graphql
```

Rewrites an `@neo4j/graphql` SDL into a draft lora-graphql SDL, preceded
by one `# TODO(migrate): ...` line per decision left to you. With
`--operations`, `@mutation`, `@filterable` and `@sortable` follow what
your client operations use. See
[migrating from @neo4j/graphql](/docs/graphql/migrating-from-neo4j).

## In CI

A typical job:

```bash
npx lora-graphql print schema.graphql > schema.public.graphql
git diff --exit-code schema.public.graphql   # API changes are reviewed

npx lora-graphql diff origin-schema.graphql schema.graphql  # fail on breaking changes

npx lora-graphql check schema.graphql \
  --operations src/operations \
  --variables ci/vars.json \
  --baseline ci/plans.json

npx lora-graphql compile schema.graphql --operations src/operations --out src/generated
```

Keep `@loradb/lora-node` and `@loradb/lora-graphql` on the same version
in CI and production: plans come from the engine, and the two packages
are released in lockstep.
