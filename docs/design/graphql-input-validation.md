# lora-graphql: input validation

Date: 2026-10-07, updated 2026-10-08. Status: built, except what
[Phases](#phases) lists as open. Written against `main` at
`f943345` (v0.24.2 prepared).

The question: how should a lora-graphql schema say that a value is
acceptable (a string no longer than 80 characters, a positive number, an
email address), where is that enforced, and how do clients learn the
rules. This document records what was investigated, what was decided and
why, and what is still open.

Claims are marked as in `known-risks.md`: **Observed** was run or read in
the code, **Inferred** was deduced, **Cited** comes from an external
source listed at the end.

## Summary of the design

1. **Rules are directives on the node's fields**, not on input types. The
   library generates every input type from the node type, so a rule
   written once applies to create, update, upsert, nested creates and
   relationship properties, for variables and inline literals alike.
2. **Small typed directives** (`@length`, `@pattern`, `@format`, `@range`,
   `@multipleOf`, `@size`) whose arguments map one to one onto JSON Schema
   2020-12 keywords, so the rules can be exported faithfully.
3. **Public types never change.** A `String` stays a `String`; validation
   runs in the generated resolver.
4. **Two enforcement points.** A new pure-JavaScript pass over the input
   runs after authentication and before the transaction opens. A second
   pass runs inside the transaction for the values that cannot be known
   earlier: the results of `adjust` and of `@populatedBy` callbacks.
5. **Every violation is returned at once**, each with an input path, a
   stable rule name, the rule's parameters and a default message. The
   offending value is never echoed.
6. **Sanitising is separate and opt in** (`@normalize`: trim, Unicode
   NFC, case), and runs before validation.
7. **An escape hatch accepts any Standard Schema validator** (zod,
   valibot, arktype, yup), for cross-field rules and whatever directives
   will never cover.
8. **Clients get the rules as JSON Schema**, since introspection cannot
   expose applied directives and will not soon.
9. **This is API-level validation.** Raw Cypher and `@cypher` mutations
   bypass it, as they bypass `@uniqueTogether`. The engine has no `CHECK`
   constraint; adding one is a separate, medium-sized engine change.

## What exists today

- **Custom scalars validate** (Observed). A `scalar Email @storedAs(type:
  STRING)` with an implementation in the `scalars` option rejects
  `"nope"` and stores `Ada@Example.com` lowercased.
- **Their errors are inconsistent** (Observed). A bad variable is
  `BAD_USER_INPUT`, but only through `lora.execute()`. A bad inline
  literal is a GraphQL validation error with no code, reported before
  authentication. Through `getSchema()` with another server, neither path
  is coded.
- **Nothing validates ordinary fields** (Observed). `age: -5` on an `Int`
  is stored. `@range` and `@size` exist only on arguments of `@cypher`
  fields; on a field they are a startup error.
- **`@default` values are barely checked at startup** (Observed,
  `scalar-field.ts:383-407`): a JavaScript type check only, with no enum
  membership and no custom-scalar check.
- **`@populatedBy` results are stored with no type check at all**
  (Observed, `plan.ts:163-169`, `runner.ts:806-810`).
- **The engine cannot back this up** (Observed). Its constraint kinds are
  uniqueness, existence, node key, relationship key and property type.

## Decisions

### D1. Directives on fields, generated into every input

A constraint is a new optional property on `ScalarField`, set in
`buildScalarField` (`scalar-field.ts:283-345`). That function is called
for node fields, relationship properties and interface fields, so the
position table decides where each directive is allowed:

- **Allowed:** node fields and relationship properties.
- **Refused on interface fields**, with the existing hint to put it on
  each implementation. Otherwise an interface and its implementations
  could disagree silently.
- **Refused on `@cypher` fields**, which are not stored values.
- **Allowed on `@cypher` arguments**, where `@size` and `@range` already
  live, so custom mutations can be bounded with the same vocabulary.
- **Allowed on custom scalars**: `scalar Email @storedAs(type: STRING)
  @format(as: "email") @length(max: 254)`. This gives reusable
  constrained types without changing the wire type, the one idea worth
  taking from Gel's schema constraints (Cited).

The largest regret in prior art is changing public types to enforce rules:
graphql-constraint-directive replaced `String` with generated scalars and
spent years on broken introspection, codegen and federation (Cited). This
design never does that.

### D2. The vocabulary

| Directive | On | JSON Schema keyword |
| --- | --- | --- |
| `@length(min: Int, max: Int)` | strings, ids | `minLength`, `maxLength` |
| `@pattern(regex: String!)` | strings, ids | `pattern` |
| `@format(as: String!)` | strings | `format` |
| `@range(min:, max:, exclusiveMin:, exclusiveMax:)` | `Int`, `Float`, `BigInt` | `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum` |
| `@multipleOf(value: Float!)` | `Int`, `Float` | `multipleOf` |
| `@size(min: Int, max: Int, unique: Boolean)` | lists | `minItems`, `maxItems`, `uniqueItems` |

Every directive also takes `message: String`, a replacement for the
default message of that rule.

- **Separate directives, not one `@constraint`.** Each can be type-checked
  against the field at startup, which is this package's style, and
  `@range` and `@size` already exist. The surveyed single-directive
  designs all grew type-confusion bugs: `minLength` on a list, `format`
  on a list of strings, `min: 0` skipped by a truthiness check (Cited).
- **List rules and item rules are syntactically distinct.** `@size` is
  about the list. Every other directive on a list field applies to each
  item, as `@range` already does for list arguments. Conflating the two
  cost Lighthouse and graphql-constraint-directive a bug fix each (Cited).
- **Bounds are spelled out.** `min`/`max` are inclusive, `exclusiveMin`/
  `exclusiveMax` are strict. Users of the prior art found a bare `min`
  ambiguous (Cited).
- **`@range` today has `Float` bounds**, which are inexact for `BigInt`
  and useless for dates. Version 1 covers `Int` and `Float` exactly and
  `BigInt` within the safe-integer range. Temporal bounds (`after`,
  `before`, relative to now) are deferred: "now" is UTC in the engine and
  not even stable within one statement (Observed), so they need their own
  design.
- **Presence is not a constraint.** Nullability belongs to `!`. "Not
  blank" is `@normalize(trim: true)` with `@length(min: 1)`.

### D3. Semantics, each chosen deliberately

These are the points where existing implementations disagree and users
get surprised (Cited). Each is stated in the docs and tested.

- **`null` and absent values skip every rule.** The check is `!==
  undefined` and `!== null`, never truthiness, so `0` and `""` are
  validated.
- **String length counts Unicode code points.** That matches JSON Schema,
  zod 4, and the engine's own `size()` (Observed: an emoji is 1 in the
  engine and 2 in JavaScript's `.length`). The fast path is zod's: if the
  UTF-16 length is within the bound, the code point count is too.
  Graphemes are not counted by anyone and not here.
- **`@pattern` must match the whole value.** JSON Schema patterns are
  unanchored; Cypher's `=~` and Bean Validation match the whole string.
  Whole-string is chosen because forgetting anchors is the commonest
  authoring mistake, and because an unanchored pattern with a leading
  quantifier is the classic quadratic case (Observed: `/a+$/` takes 14 s
  on 100,000 characters). The JSON Schema export wraps the pattern as
  `^(?:...)$`, so exported rules mean the same thing.
- **Patterns are compiled once, at startup, with the `u` flag.** An
  invalid pattern is a model error.
- **`@multipleOf` does not use `%`.** `0.3 % 0.1` is not zero in IEEE
  arithmetic, in JavaScript and in the engine alike (Observed). It uses a
  rounded quotient with a stated tolerance, and the docs recommend
  integers in minor units for money.
- **Non-finite floats are refused.** GraphQL's `Float` already refuses
  them on input; the after-write pass also refuses a `NaN` or infinity an
  `adjust` produced.
- **Defaults are validated at startup.** A `@default` that violates the
  field's own rules is a model error, modelled on the existing "default
  is outside @range" check for arguments.
- **Rules apply to writes only.** Filter operands are not validated: a
  filter for a value that could never be stored simply matches nothing.
  The existing `maxStringFilter` and `maxListFilter` keep bounding them.
- **Keys are validated on create and never on lookup.** A key is stored
  once and used as an address everywhere else; validating lookups would
  make a node with an older key unaddressable. A generated UUID is not
  validated, and a rule on a `@key(generate: true)` field is a lint.
- **An update validates only the fields it sets.** Stored values that
  predate a rule are left alone and reported by tooling (see D9).
- **An upsert validates its whole input**, for both the create and the
  update path. Which path a key takes is only known after a database
  read, and a value that is invalid should not be accepted on the chance
  that it is dropped.

### D4. Where it runs

There is no hook to use (Observed). Today every input check runs after
`begin()`, and for updates after a read statement and only for visible
rows. So "before the transaction" means new code.

**Pass A, before the transaction.** A model-driven walker over the
arguments (`input`, `update`, nested `connect`/`create`/`update`, `edge`,
`node`, the member-keyed parts of interface and union inputs), placed in
`executeMutation` between `settleClaims` and `begin()`
(`mutate.ts:143-145`). It applies `@normalize`, then every rule, and
collects all violations. A bad request then never takes the writer lock,
which matters because LoraDB has one writer.

**Pass B, inside the transaction.** For the values that cannot be known
earlier:

- **`adjust` results.** Adjust is compiled into the `SET` statement
  (`adjust.ts:20-92`); the value never exists in JavaScript. The
  statement gains a `RETURN` of the adjusted properties, and the same
  JavaScript validators run on what comes back. Reading the value back,
  instead of expressing the rule a second time in Cypher, keeps one
  implementation of every rule and avoids the differences between the two
  regex engines and the two modulo operators.
- **List growth.** `push` can take a list past its `@size`. Same
  mechanism.
- **`@populatedBy` results.** They are JavaScript values at the point the
  callback returns (`runner.ts:806-810`) and are validated there. This
  also closes today's gap where they are stored unchecked.

A failure in pass B throws inside `executeMutation`'s `try`, and the
existing `catch` rolls back, exactly as `checkCardinality` and
`@uniqueTogether` do.

**Order relative to access control.** Pass A runs after the type's
`@authentication` and after rules the claims alone settle, so an
anonymous caller on a guarded type still gets `UNAUTHENTICATED`. Rules
that depend on the row cannot be settled before the transaction, so a
caller they would refuse sees the validation error first. That reveals
schema rules and never data: the answer does not depend on whether the
node exists. Lighthouse has the stronger version of this problem open
since 2019 (Cited); here it is bounded and documented.

**Caller-owned transactions.** With a `transaction` in the context, or
`mutationTransaction: "operation"`, the transaction is already open when
the resolver runs, and any failure rolls it back whole. That is existing
behaviour and stays.

### D5. Errors

No request-time convention for several problems in one error exists in
the package today (Observed), so this is a new, deliberate shape. One
error per root field:

```json
{
  "message": "2 values are not valid: input[0].email must be an email address; input[0].name must be at most 80 characters",
  "path": ["createPersons"],
  "extensions": {
    "code": "BAD_USER_INPUT",
    "violations": [
      {
        "path": ["input", 0, "email"],
        "coordinate": "Person.email",
        "rule": "format",
        "params": { "format": "email" },
        "message": "must be an email address"
      },
      {
        "path": ["input", 0, "name"],
        "coordinate": "Person.name",
        "rule": "maxLength",
        "params": { "limit": 80 },
        "message": "must be at most 80 characters"
      }
    ]
  }
}
```

- **`path` is the client's input path**, as an array of keys and indices,
  not a dotted string and not a storage name. Lighthouse's dotted keys
  and storage-name confusion are two of its long-running issues (Cited).
- **`rule` is the JSON Schema keyword and `params` follows AJV's shapes**
  (`{limit}`, `{limit, comparison}`, `{format}`, `{pattern}`,
  `{multipleOf}`). A rule name with its parameters is what a client needs
  to localise a message; messages alone (Lighthouse) or codes without
  parameters (Shopify) are the weaker forms (Cited).
- **`coordinate`** is the schema coordinate of the model field, now a
  spec concept (September 2025 edition).
- **The rejected value is never included.** Three surveyed libraries echo
  it and so leak passwords and personal data into logs (Cited). A
  write-only field (`@selectable(onRead: false)`) makes this mandatory.
- **The number of violations reported is capped** (100), so a batch of
  1,000 bad inputs cannot produce a megabyte of errors.
- **Pass A is `BAD_USER_INPUT`.** A pass B failure depends on stored
  state or on server code, so it is `CONSTRAINT_VIOLATION` with the same
  `violations` array, consistent with how state-dependent write failures
  are coded today.
- **HTTP status stays 200.** These are execution errors with a `data`
  entry. The GraphQL-over-HTTP draft reserves 4xx for requests that fail
  before execution, and a 400 from graphql-constraint-directive broke
  federation routers (Cited).
- **Top-level errors, not errors in the payload.** Shopify and
  graphql-ruby recommend typed `userErrors` in the payload. This package
  reports every other refusal as a coded top-level error with all-or-
  nothing writes, and a second error channel for one class of problem
  would be the larger inconsistency.

As part of this work the existing gap is closed: an inline literal a
scalar rejects gets `BAD_USER_INPUT` like its variable twin, on every
serving path (wrap each scalar's `parseValue`/`parseLiteral`, and recode
value-coercion validation errors in `Documents`).

### D6. Regular expressions and ReDoS

The schema author writes the pattern; the attacker supplies the input.
JavaScript's regex engine backtracks: `/^(a+)+$/` takes 7 seconds on a
30-character input (Observed), so a length cap alone does not rescue an
exponential pattern.

The engine's regex is the Rust `regex` crate, linear-time (Observed:
`(a+)+$` on 5,000 characters in 0.1 ms). Evaluating patterns there would
remove the risk by construction. It is rejected as the default for three
reasons: the dialects differ (`\d` and `\w` are Unicode-aware in the
engine and ASCII-only in JavaScript; no lookaround or backreferences), so
server and exported client rules would disagree; an invalid or oversized
pattern evaluates to `null` instead of raising (Observed), which a
validation layer must never treat as a pass; and it adds a round trip per
request.

The policy, in layers:

1. **Whole-string matching** (D3) removes the unanchored class.
2. **A pattern needs a length bound.** `@pattern` without `@length(max:)`
   on the same field is a model error unless the bound comes from a
   format. Length is checked first, so the regex never sees a long input.
3. **A conservative startup check** rejects the well-known shapes: nested
   quantifiers, a quantified group with overlapping alternatives,
   adjacent overlapping quantifiers. It is a heuristic and the docs say
   so; the surveyed star-height checkers are wrong in both directions
   (Observed for safe-regex2).
4. **Two hooks**, mirroring AJV's `code.regExp`: `regexEngine` to plug in
   a linear engine (`re2`, or `re2js`, pure JavaScript and about 14 times
   slower on benign input), and `checkPattern` to plug in a real analyser
   (`redos-detector` was correct on all eight probes run; `recheck` is
   the most accurate and suits CI).
5. **Built-in format regexes are themselves linear** and carry an implied
   maximum length.

V8's experimental linear engine is not usable: it is off by default, a
library cannot enable a process flag, and it rejects the `u` and `i`
flags (Observed on Node 24).

### D7. Formats

`@format(as:)` takes a name checked at startup against the built-in set
plus a `formats` option (`{ name: RegExp | (value) => boolean }`). An
unknown name is a model error. Formats are always enforced: JSON Schema's
default of treating `format` as an annotation is exactly why exported
rules also carry a `pattern` (D10).

Built in, all with no dependencies on Node 20 or later:

- `email`: the WHATWG HTML rule with a 254-character cap, and
  `email-strict`, which also requires a top-level domain. Neither checks
  deliverability.
- `url`: `URL.canParse` plus a scheme allow-list defaulting to `http` and
  `https`. Named `url`, not `uri`, because the WHATWG parser is not RFC
  3986, and restricted because a stored `javascript:` or `file:` URL
  becomes an injection or request-forgery vector when rendered or fetched
  (Cited).
- `uuid`: shape only, case-insensitive. A strict version check would
  reject the next UUID version.
- `hostname`, `ipv4`, `ipv6`, `slug`, `hex-color`, `ulid`.
- `e164`: the shape of an international phone number, named for what it
  is. Whether a number is assignable needs numbering-plan data the
  library will not ship.

Dates and times are not formats here: `Date`, `Time`, `DateTime` and
`Duration` are scalar types and the engine parses them.

### D8. Sanitising: `@normalize`

Validation rejects; sanitising changes the value. They are separate, and
the order is fixed and documented: **sanitise, then validate, then
store**. Lighthouse had to make this ordering explicit in a breaking
release; zod orders by declaration and yup always transforms first, which
is how the same chain accepts different input in each (Observed, Cited).

```graphql
email: String! @normalize(trim: true, case: LOWER, unicode: NFC) @format(as: "email") @unique
```

- **Opt in, never silent.** W3C's guidance is that software must not
  change the normalisation form of content unless a specification says
  so (Cited).
- **Fixed order inside the directive:** trim, Unicode normalisation, case
  mapping. The sanitised value is what is validated and stored.
- **Why it belongs in this feature:** the engine normalises nothing
  (Observed: a uniqueness constraint accepts both precomposed and
  combining "é", and `'A'`, `'a'` and `'a '` are three values). A
  `@unique` email without normalisation is a duplicate-account bug.
- **`emptyToNull: true`** turns `""` into `null` after trimming, for
  optional fields fed by HTML forms.
- **Limits stated honestly:** `trim` does not remove zero-width
  characters, case mapping can change a string's length (`İ`), and
  confusable or mixed-script detection needs Unicode data tables and is
  left to the application.

Open question: whether a normalised field's filter operands and lookup
keys are normalised too, so that `email: { eq: " Ada@X.com" }` finds the
row. It is the consistent behaviour and it is what makes a normalised
`@unique` field usable, but it changes read semantics. See "Open
questions".

### D9. Tooling

Each tool that reasons about the schema must learn the new directives
(Observed gaps in brackets):

- **Startup checks:** rule against field type, default against rule,
  pattern compiles, pattern has a bound, `min` not above `max`, format
  exists.
- **`lint`:** a rule on a field no client can set; a rule on a generated
  key; a `@unique` string without `@normalize`.
- **`diff`** [a tightened rule is invisible to both halves today]: a
  model-level comparison. Tightened is breaking, loosened is a note, and
  either way a note says stored data is not revalidated.
- **`check --database`** [nothing compares stored data with the model
  today]: scan existing nodes against the rules and report violations,
  reading values in pages and validating in JavaScript so the answer
  matches the write path.
- **`migrate neo4j`:** map a third-party `@constraint(...)` onto these
  directives, since projects coming from `@neo4j/graphql` often carry
  graphql-constraint-directive.
- **`expectAccess`** [treats `BAD_USER_INPUT` as a mismatch]: say
  clearly that the probe's sample input broke a rule.
- **Tests that enforce coverage:** `directive-positions.test.ts` (a `USE`
  sample per directive), `error-codes.test.ts`, and `docs.test.ts`, which
  builds every example on the directive reference page.

### D10. Telling clients the rules

Applied directives are not visible through introspection, and the spec
proposal to expose them was closed in 2024 in favour of a metadata
proposal that has no champion (Cited). The library has to publish the
rules itself.

- **JSON Schema 2020-12 per input type**, from `lora.inputSchemas()` and
  `lora-graphql compile`. Because `format` is annotation-only by default,
  each format also emits its `pattern`, as zod's own exporter does
  (Observed). From JSON Schema, a client can build zod, yup or valibot
  validators with existing tools.
- **Constraint sentences in the generated input descriptions**, so the
  rules show in GraphiQL and in the printed SDL. This changes the schema
  hash whenever a rule changes, which forces persisted-operation
  manifests to be rebuilt. That is the correct behaviour for a contract
  change, but it couples a rule edit to a client rebuild, so it is an
  option (`describeConstraints`, default on).
- **Not in version 1:** an annotated SDL carrying `@constraint` for
  graphql-codegen-typescript-validation-schema. JSON Schema reaches the
  same libraries without a custom SDL printer.

A caveat to document: valibot and yup count string length in UTF-16
units, so an exported `maxLength` can disagree with the server on astral
characters (Observed).

### D11. The escape hatch

```ts
new LoraGraphQL({
  typeDefs,
  driver: loraDriver(db),
  validators: {
    Booking: z.object({ startsOn: z.string(), endsOn: z.string() })
      .refine((b) => b.endsOn >= b.startsOn, {
        path: ["endsOn"],
        message: "must not be before startsOn",
      }),
  },
});
```

- **Any Standard Schema v1 object is accepted**: zod 3.24+, valibot 1+,
  arktype 2+, yup 1.7+, joi 18+ and others (Cited). The interface is
  types only and designed to be copied, so the package gains no
  dependency.
- **It runs after the built-in rules**, on the node's sanitised input, in
  pass A. The result is always awaited. The returned `value` replaces the
  input, so a validator's transforms apply.
- **Its issues join the same `violations` array**, with `rule: "custom"`,
  the validator's message, and the path prefixed with the GraphQL input
  path. Path segments are normalised (a segment may be a key or an object
  with `key`; yup reports list indices as strings) (Observed).
- **Limit stated:** a Standard Schema issue has only a message and a
  path, so custom violations carry no parameters to localise with.
- **It sees input, not state.** A rule over stored values (an update that
  sets only `endsOn`) needs the row. That case is what a future
  `@check(where:)` would cover, reusing the rule compiler that already
  compares a node's own paths; it is out of scope here.

## What this does not do

- **It does not protect the database.** Raw Cypher, `tx.execute()` and
  `@cypher` mutations write unvalidated values. The engine's five
  constraint kinds are the only rules that hold against every writer.
- **It does not revalidate stored data** when a rule is added or
  tightened. `check --database` reports it.
- **It does not validate filter operands.**
- **It does not do cross-field or state-dependent rules** beyond the
  `validators` hook.
- **It does not do temporal bounds** in version 1.

## A `CHECK` constraint in the engine

Investigated as the alternative that would protect the database, and
deferred. Constraint enforcement lives in the store, which depends only on
the AST crate and cannot evaluate expressions (Observed,
`lora-store/Cargo.toml:15`). A `REQUIRE <predicate>` constraint touches
the parser, the analyser, the store's hand-written codec (a new kind tag
that old binaries cannot read), the WAL and snapshot formats, and the
executor's statement-end check. Two engine behaviours would have to be
settled first, since both would turn a failed check into a pass: an
invalid regex evaluated to `null`, and a failing expression inside a
write was dropped silently (see below; both fixed 2026-10-11).

## Engine and binding bugs found on the way

None is part of this feature. All were reproduced on `main`, and all four
were fixed on 2026-10-11: a JavaScript integer up to 2^53 arrives as an
INTEGER, a failing expression in a write fails the statement, the
auto-commit fast path takes only plans that cannot fail, and an invalid
or oversized regex is an error whose failed compile is cached. What
follows is how they behaved before.

- **Integers outside the 32-bit range arrive in the engine as floats.** A
  JavaScript `number` parameter is an INTEGER only in `[-2^31, 2^32)`.
  Through lora-graphql, a `BigInt` field given `"5000000000"` is stored
  as FLOAT; `"3"` and `"9007199254740993"` are stored as INTEGER. It
  reads back correctly, which is why it went unnoticed.
- **A failing expression inside a write is dropped.** `SET n.x = 1/0` and
  `CREATE (:T {d: date('2020-13-01')})` succeed and leave the property
  absent. The same expressions in `RETURN` raise `LORA_VALIDATION`.
- **Auto-commit without a timeout can keep writes from a statement that
  errors.** A statement that created two nodes and then failed left both;
  with `timeoutMs` it left none. Interactive transactions roll back
  correctly, so lora-graphql mutations are not exposed.
- **An invalid or oversized regex evaluates to `null`**, and a failing
  compile is not cached: `WHERE n.s =~ '\\pL{1,300}'` over 100 rows took
  4.26 seconds.

## Phases

| Phase | Contents | Ships as |
| --- | --- | --- |
| 0 | Store `BigInt` as INTEGER; code inline-literal scalar errors | Built |
| 1 | `@length`, `@pattern`, `@format`, `@range` and `@size` on fields, `@multipleOf`; pass A and pass B; the error shape; startup checks; docs and tests | Built |
| 2 | `@normalize`; constraints on custom scalars; the `validators` hook | Built |
| 2b | The same directives on `@cypher` arguments | Built |
| 3 | `diff` awareness; the stored-value scan in `check`; JSON Schema export; rules in input descriptions; two lints | Built |
| Open | A `regexEngine` hook; normalising filter operands and lookup keys; `migrate` mapping of a third-party `@constraint`; a `describeConstraints` option | |

What phase 1 did differently from this design, and why:

- **The pattern check is a probe, not a static analysis** (D6, point 3).
  At startup each compiled pattern is run on inputs built to make it
  backtrack, at growing lengths up to the field's `@length(max:)`, and
  refused when one match overruns 25 ms. A structural check for nested
  quantifiers also flags safe, ordinary patterns such as the slug
  expression; measuring does not.
- **No `regexEngine` or `checkPattern` hooks yet**, and no custom formats:
  both need an option that the CLI, which builds a model with no options,
  cannot see. They move to phase 2 with the `validators` hook.
- **No lints yet.** They move to phase 3 with the other tooling.
- **`@range` and `@size` on `@cypher` arguments keep `min`/`max` and
  `max`.** The new arguments are refused there by name until phase 2.

What phase 3 settled:

- **Rules are in the input descriptions, with no option to turn them
  off** (D10). They are part of the contract, so the schema hash moving
  with them is the correct signal to rebuild a persisted-operation
  manifest. An option can follow if that coupling hurts in practice.
- **The exported JSON Schema was checked against AJV in strict mode**: it
  compiles as 2020-12, and AJV and the server gave the same verdict on 17
  inputs, emoji length and anchoring included.
- **The stored-value scan is part of `check()` and does not fail it.**
  It reads values in key order and applies the write path's own code, so
  the two cannot disagree. It covers node properties; relationship
  properties are open.
- **On an argument, a plain `@size(max:)` or `@range(min:, max:)` keeps
  its older single-error message.** Any other rule, or argument of those
  two, reports through `extensions.violations`.

What phase 2 settled:

- **Custom formats are constrained scalars** (D1), not a `formats`
  option. `scalar Sku @storedAs(type: STRING) @length(max: 12)
  @pattern(regex: "...")` is reusable, visible to the CLI, and shows in
  the schema as a type. An option the CLI cannot see would have made
  `lora-graphql check` fail on a schema the server accepts.
- **A field's rule replaces the scalar's rule of the same directive**;
  the scalar's other rules still apply.
- **A validator runs only when the field rules hold** (D11), so it never
  sees a value a directive refuses, and is shown dates as ISO strings.
- **`@normalize` changes writes only.** Normalising filter operands and
  lookup keys (open question 2) is not done: it changes read semantics
  and belongs with a decision on case-insensitive lookups in general.
  The directive reference says so.
- **Verified against the real libraries:** the same cross-field rule
  written in zod 4, valibot 1 and yup 1.7 produced identical violations,
  and each library's `trim` transform reached the database.

Phase 1 is the bulk of the value and is roughly the size of
`@uniqueTogether`. Phase 2 should follow closely: without `@normalize` a
validated `@unique` email still admits duplicates, and without the hook
people will ask for ever more directive arguments.

The test file for phase 1 follows `unique-together.test.ts`: one block per
directive and one per entry point (create, update, adjust, both upsert
paths, bulk update, nested create, edge on connect, re-connect and
update, member inputs, default at startup, callback result, alias naming,
write-only value not echoed), a "no statement ran" assertion for pass A, a
row-count assertion for pass B, and an ordering test that an anonymous
caller gets `UNAUTHENTICATED`. The recurring bugs of the prior art are
test cases from the start: `min: 0`, later fields after `__typename`,
lists of input objects, emoji length.

## Open questions

1. **Whole-string `@pattern`** (D3) departs from JSON Schema's unanchored
   default. Safer and matches Cypher; the cost is surprising someone who
   expects JSON Schema semantics.
2. **Normalising filter operands and lookup keys** of a `@normalize`d
   field (D8).
3. **Constraint text in descriptions by default** (D10), given the
   schema-hash coupling.
4. **`CONSTRAINT_VIOLATION` for pass B** (D5), or `BAD_USER_INPUT` for
   both so clients branch on one code.
5. **Whether phase 0 goes out on its own** before the feature.

## Sources

Prior art:
[graphql-constraint-directive](https://github.com/confuser/graphql-constraint-directive)
and its issues
[#2](https://github.com/confuser/graphql-constraint-directive/issues/2),
[#20](https://github.com/confuser/graphql-constraint-directive/issues/20),
[#92](https://github.com/confuser/graphql-constraint-directive/issues/92),
[#254](https://github.com/confuser/graphql-constraint-directive/issues/254),
[#299](https://github.com/confuser/graphql-constraint-directive/issues/299);
[Lighthouse validation](https://lighthouse-php.com/master/security/validation.html)
and issues [#683](https://github.com/nuwave/lighthouse/issues/683),
[#1780](https://github.com/nuwave/lighthouse/issues/1780);
[mercurius-validation](https://github.com/mercurius-js/validation/blob/main/docs/directive-validation.md);
[graphql-java-extended-validation](https://github.com/graphql-java/graphql-java-extended-validation);
[SmallRye GraphQL directives](https://smallrye.io/smallrye-graphql/latest/directives/);
[Gel constraints](https://docs.geldata.com/reference/datamodel/constraints);
[SurrealDB DEFINE FIELD](https://surrealdb.com/docs/surrealql/statements/define/field);
[Shopify UserError](https://shopify.dev/docs/api/admin-graphql/latest/objects/UserError);
[Hasura input validations](https://hasura.io/docs/2.0/schema/postgres/input-validations/);
[Pothos validation plugin](https://pothos-graphql.dev/docs/plugins/validation);
[graphql-codegen-typescript-validation-schema](https://the-guild.dev/graphql/codegen/plugins/typescript/typescript-validation-schema).

Standards:
[JSON Schema 2020-12 validation](https://json-schema.org/draft/2020-12/json-schema-validation);
[Standard Schema](https://standardschema.dev/schema) and
[Standard JSON Schema](https://standardschema.dev/json-schema);
[GraphQL over HTTP draft](https://github.com/graphql/graphql-over-http/blob/main/spec/GraphQLOverHTTP.md);
[graphql-spec #300](https://github.com/graphql/graphql-spec/issues/300);
[WHATWG valid e-mail address](https://html.spec.whatwg.org/multipage/input.html#valid-e-mail-address);
[RFC 8265](https://www.rfc-editor.org/rfc/rfc8265.html);
[W3C character model: normalisation](https://www.w3.org/TR/charmod-norm/);
[RFC 9562](https://www.rfc-editor.org/rfc/rfc9562.html).

Regex safety:
[V8 non-backtracking engine](https://v8.dev/blog/non-backtracking-regexp);
[AJV security](https://ajv.js.org/security.html) and
[options](https://ajv.js.org/options.html);
[re2js](https://github.com/le0pard/re2js);
[node-re2](https://github.com/uhop/node-re2);
[recheck](https://makenowjust-labs.github.io/recheck/).
