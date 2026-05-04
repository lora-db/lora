# GQL Extended Schema Language

Created: 2026-05-03

This document proposes a **Lora GQL Extended Schema** layer, using
[`social-platform.gqlx`](./social-platform.gqlx) as the full worked example.
The goal is to keep ISO GQL as the executable query language while adding a
rich application schema language above standard graph type definitions.

The extension is intentionally not presented as ISO/IEC 39075:2024 syntax.
`gql.yml` gives LoraDB a standard catalog and graph type foundation, but it
does not cover many application-schema concepts that real products usually
need: custom scalars, enums, interfaces, abstract nodes, derived relationships,
authorization policies, hooks, fulltext indexes, uniqueness, deprecations, and
schema-version metadata.

## Why An Extension Is Needed

ISO-style GQL graph types are good at structural graph typing:

- node types
- edge types
- labels
- endpoint constraints
- property value types
- graph/catalog namespaces

They are much thinner as an application schema language. The SocialPlatform
example adds the missing application layer:

| Feature | In standard GQL graph type? | Extension role |
| --- | --- | --- |
| Schema `version`, `namespace`, `strict` | No | Catalog metadata and compatibility checks |
| Custom scalar aliases | No | Runtime validators over primitive GQL value types |
| Regex/length/range directives | No | Property validation |
| Enums | Partial via value typing patterns, not rich SDL | Finite domain validation |
| Value types with defaults | Partly via records | Reusable embedded records |
| Interfaces | No | Shared fields and checks |
| Abstract nodes | No | Reusable node inheritance |
| Derived fields | No | Query expansion / relationship-backed virtual fields |
| Explicit indexes | Not in this `gql.yml` dump | Physical planner hints / catalog index policy |
| Fulltext indexes | No | Search subsystem metadata |
| Policies | No | Authorization rewriting / enforcement |
| Hooks | No | Integration lifecycle events |
| Deprecations | No | Client/tooling metadata |

## File Extension

Use `.gqlx` for the extension grammar:

```text
social-platform.gqlx
```

This keeps the distinction crisp:

- `.gql` / parser dialect `Gql`: ISO-style query syntax.
- `.gqlx`: Lora schema extension syntax that compiles into catalog metadata,
  graph type definitions, constraints, index policies, and authorization rules.

## Top-Level Syntax

```gql
schema SocialPlatform
version "1.0.0"
namespace "com.example.social"
strict true
{
  // declarations...
}
```

Semantics:

- `schema` names an application schema package.
- `version` is semantic-version metadata for migrations and compatibility.
- `namespace` is a globally stable name for generated code and catalog ids.
- `strict true` means undeclared labels/properties are rejected in typed graphs.

## Declaration Kinds

The extension supports these top-level declarations:

```text
scalar <Name> [extends <BaseType>] [directives...]
enum <Name> { <Variant>* }
value <Name> { <field>* }
interface <Name> [extends <Interface>*] { <field | check>* }
abstract node <Name> [extends <NodeOrInterface>*] [implements <Interface>*] { ... }
node <Name> [extends <NodeOrInterface>*] [implements <Interface>*] { ... }
union <Name> = <Node> | <Node> ...
edge <Name> from <Node> to <Node> { <field>* } [directives...]
index <Name> on <Node>(<field-list>)
edge index <Name> on <Edge>(<field-list>)
fulltext index <Name> on <Node>(<field-list>)
unique <Name> on <Node>(<field-list>)
policy <Node> { read: <expr> create: <expr> update: <expr> delete: <expr> }
hook <phase> <operation> <Target> <action> "<handler>"
deprecated <TypeOrField> since "<version>" [note "..."] [replace_with "..."]
```

## Type System

Field type syntax:

```text
name: Type
name: Type?
name: List<Type>
name: Set<Type>
name: Type = default
```

Rules:

- `?` marks a nullable field.
- Non-null is the default for stored fields.
- `List<T>` preserves order and duplicates.
- `Set<T>` is a derived or logical collection with unique values.
- Defaults are applied on create when the field is absent.

## Directives

Directives are metadata attached to scalars, fields, nodes, edges, and schema
members.

Validation directives:

```gql
@pattern("^[a-zA-Z0-9_]{3,32}$")
@length(min: 1, max: 120)
@range(min: 0, max: 100)
@precision(scale: 2, precision: 18)
@check("acceptedAt = null or acceptedAt <= expiresAt")
```

Identity and mutability directives:

```gql
@id
@unique
@readonly
```

Relationship directives:

```gql
@derived(from: AUTHORED.out)
@derived(from: OWNED_BY_TEAM.out, exactly: 1)
@derived(from: POSTED_IN.out, at_most: 1)
@derived(from: REACTED.in where edge.kind == LIKE)
```

Edge directives:

```gql
@cardinality(one_to_many)
@cardinality(many_to_one)
@cardinality(many_to_many)
@required(from)
@required(to)
@unique_pair
@acyclic
@cascade(onDelete: from)
@restrict(onDelete: to)
@reverse("HAS_MEMBER")
```

## Complete SocialPlatform Example

The full example lives in:

[social-platform.gqlx](./social-platform.gqlx)

It includes:

- 9 base scalars and 7 constrained scalar aliases.
- 6 enums.
- 8 reusable value types.
- 6 interfaces.
- 5 abstract node types.
- 9 concrete node types.
- 3 unions.
- 14 edge types.
- 9 node indexes, 4 edge indexes, and 3 fulltext indexes.
- 4 composite uniqueness constraints.
- 6 authorization policies.
- 6 lifecycle hooks.
- 2 deprecation declarations.

## Example Excerpts

Custom scalar:

```gql
scalar Username extends String
  @pattern("^[a-zA-Z0-9_]{3,32}$")
```

Enum:

```gql
enum ContentStatus {
  DRAFT
  PUBLISHED
  ARCHIVED
  DELETED
}
```

Reusable value type:

```gql
value Profile {
  displayName: NonEmptyString @length(max: 120)
  bio: String? @length(max: 280)
  avatarUrl: URL?
  website: URL?
  location: String? @length(max: 120)
}
```

Interface with check:

```gql
interface SoftDeletable {
  isDeleted: Bool = false
  deletedAt: DateTime?

  @check("not (isDeleted = true and deletedAt = null)")
}
```

Abstract node:

```gql
abstract node Content extends TenantNode, SoftDeletable, Publishable {
  tenantId: UUID
  title: NonEmptyString @length(max: 300)
  body: String
  status: ContentStatus = DRAFT
  publishedAt: DateTime?
}
```

Concrete node with derived fields:

```gql
node Post extends Content {
  tenantId: UUID

  excerpt: String? @length(max: 500)
  author: User @derived(from: AUTHORED.in, exactly: 1)
  team: Team? @derived(from: POSTED_IN.out, at_most: 1)
  project: Project? @derived(from: BELONGS_TO_PROJECT.out, at_most: 1)
  likedBy: Set<User> @derived(from: REACTED.in where edge.kind == LIKE)
  comments: Set<Comment> @derived(from: COMMENT_ON.in)
}
```

Edge with cardinality and delete behavior:

```gql
edge AUTHORED from User to Post {
  at: DateTime
}
@cardinality(one_to_many)
@required(to)
@cascade(onDelete: from)
@reverse("AUTHOR")
```

Indexes:

```gql
index idx_post_status_published on Post(status, publishedAt desc)
edge index idx_authored_at on AUTHORED(at)
fulltext index ftx_post_text on Post(title, body, excerpt)
unique uq_team_name_per_tenant on Team(tenantId, name)
```

Policy:

```gql
policy Post {
  read: auth != null and auth.tenantId == node.tenantId and node.isDeleted == false
  create: auth != null
  update: exists(auth -> AUTHORED -> node) or auth.role == ADMIN
  delete: exists(auth -> AUTHORED -> node) or auth.role == ADMIN
}
```

Hooks and deprecations:

```gql
hook after create Post emit "events.post.created"
deprecated User.phoneNumber since "1.2.0" replace_with "notificationPreferences"
```

## Lowering Into Standard GQL Concepts

The extension should compile into a standard-ish GQL catalog plus Lora-specific
metadata.

Node declarations lower to graph type node patterns:

```gql
node User extends Principal {
  email: Email @unique
  username: Username @unique
}
```

Conceptually lowers to:

```gql
CREATE GRAPH TYPE /com/example/social/SocialPlatform AS {
  (:User {
    email STRING NOT NULL,
    username STRING NOT NULL
  })
};
```

The extension metadata retains:

- `Email` and `Username` scalar validators.
- `@unique` constraints.
- inherited fields from `Principal`.
- policies, hooks, and deprecations.
- generated indexes.

Edge declarations lower to graph type edge patterns:

```gql
edge MEMBER_OF from User to Team {
  since: DateTime
  isOwner: Bool = false
}
@cardinality(many_to_many)
@unique_pair
```

Conceptually lowers to:

```gql
(:User)-[:MEMBER_OF {since ZONED DATETIME, isOwner BOOL}]->(:Team)
```

The extension metadata retains cardinality, uniqueness, reverse name, and
delete behavior.

## Proposed AST Additions

```rust
pub enum SchemaDeclaration {
    Scalar(ScalarDecl),
    Enum(EnumDecl),
    Value(ValueDecl),
    Interface(InterfaceDecl),
    Node(NodeDecl),
    Edge(EdgeDecl),
    Union(UnionDecl),
    Index(IndexDecl),
    Unique(UniqueDecl),
    Policy(PolicyDecl),
    Hook(HookDecl),
    Deprecated(DeprecatedDecl),
}

pub struct ExtendedSchema {
    pub name: String,
    pub version: Option<String>,
    pub namespace: Option<String>,
    pub strict: bool,
    pub declarations: Vec<SchemaDeclaration>,
}
```

Keep this AST separate from query AST. The compiler can later produce:

- catalog graph type definitions
- validator metadata
- policy metadata
- index policies
- generated type bindings

## Implementation Order

1. Parse schema header and top-level declarations.
2. Parse scalars, enums, and value types.
3. Parse nodes and edges without inheritance.
4. Add inheritance/interfaces and field flattening.
5. Add directives and validation expression parsing.
6. Add indexes and uniqueness.
7. Add policies as stored metadata.
8. Add hooks as stored metadata.
9. Add deprecations for tooling.
10. Lower to GQL graph type + Lora extension catalog metadata.

## Design Decision

This should be treated as a **schema authoring language**, not as core GQL
query syntax. That lets LoraDB stay close to ISO GQL for query execution while
still offering a modern, expressive application schema layer for product
development.

