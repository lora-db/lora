---
slug: from-developer-trust-to-hosted-platform
title: "From developer trust to hosted platform"
description: "The customer journey behind LoraDB: local adoption first, managed operations later."
authors: [joost]
tags: [founder-notes, design, architecture]
---

The easiest way to misunderstand LoraDB is to see the open core and the hosted
platform as separate ideas.

They are the same journey.

The core database has to be developer-first because graph databases ask for a
lot of trust. You are not just storing records. You are putting relationships,
paths, and product logic into a system that needs to be correct and fast. If a
developer cannot run it locally, inspect it, and build confidence in the query
engine, the hosted product has no foundation.

<!-- truncate -->

## Trust Starts Before Production

The first customer is usually not a procurement team. It is one developer with
a problem that keeps resisting simpler models.

They have a set of entities. The relationships matter. SQL joins are becoming
awkward. A document model hides too much structure. A vector database retrieves
similar things, but it does not explain how they connect.

That developer does not want a sales call first. They want to try the thing.

For LoraDB, the first trust moment should look like this:

```bash
cargo run --bin lora-server
```

Then a query:

```cypher
MATCH (a:Account)-[:OWNS]->(p:Project)-[:DEPENDS_ON]->(s:Service)
WHERE a.id = $account
RETURN p.name, collect(s.name) AS services
```

Then a reaction: "This is the shape of my problem."

Everything before that moment is friction.

## Why Source-Available Core Matters

For infrastructure, source availability is not only about ideology. It is a
trust tool.

Developers can inspect the parser. They can read the planner. They can see
where values are represented. They can understand limitations without waiting
for marketing language to become documentation.

That is especially important for a young database. LoraDB should not ask
people to believe that every edge case is solved. It should show the work:

- here is the AST;
- here is semantic analysis;
- here is logical planning;
- here is physical execution;
- here is the in-memory store;
- here are the tests.

That transparency is part of the product.

## Why Not Fully Open For Hosted Resale

The other side of the strategy is the BSL license.

The goal is not to stop developers from using LoraDB. The goal is to stop the
core engine from being repackaged immediately as a competing hosted database
service by someone who did not build or maintain it.

That boundary is important because database companies need long time horizons.
The hard work is not only writing a parser or a store. It is maintaining the
engine, supporting users, improving performance, documenting behavior, and
building the operational platform around it.

The BSL lets LoraDB be open enough for adoption while protecting the hosted
business model:

- internal business use is allowed;
- development and non-production use are allowed;
- source reading and modification are allowed;
- hosted database-as-a-service for third parties is restricted;
- each version converts to Apache 2.0 after the Change Date.

That is the intended balance.

## The Journey In Four Stages

The customer journey I want is deliberately simple.

### 1. Discovery

A developer finds LoraDB because they need a graph, not because they want a
platform. They read the docs, run examples, and try the query model.

The goal here is clarity. The website should explain what LoraDB is and what it
is not. The repo should be readable. The license should be explicit.

### 2. Local Adoption

The developer builds a prototype with local data. They care about:

- quick setup;
- familiar Cypher-shaped queries;
- predictable performance;
- useful errors;
- enough documentation to keep moving.

This is where an in-memory engine shines. No cluster. No hosted account. No
waiting for infrastructure.

### 3. Internal Production

The team starts using LoraDB in an internal tool, agent memory system,
workflow engine, or product feature where the graph is close to the
application.

They care about reliability, performance, and whether limitations are honest.
This is why docs, tests, and release notes matter as much as features.

### 4. Managed Operations

Eventually, some teams do not want to operate the database themselves. They
need persistence, backups, monitoring, auth, scaling, and support.

That is where the hosted platform belongs. It should not replace developer
adoption. It should follow it.

## Why This Matters For Product Quality

A hosted-first database can accidentally optimize for the buyer before the
builder. LoraDB should optimize for the builder first.

That affects product decisions:

- keep the core small enough to understand;
- make docs practical, not ornamental;
- publish limitations clearly;
- make release notes explain why changes matter;
- keep local development fast;
- protect the hosted business without making the core feel closed.

The hosted platform becomes credible only if the core earns trust.

## What I Want LoraDB To Feel Like

I want LoraDB to feel like a database you can pick up in an afternoon and still
respect after a month.

Small enough to understand. Fast enough to stay close to the application.
Efficient enough that the business model does not depend on waste. Clear enough
that a developer can explain it to a teammate.

That is the bridge from developer trust to hosted platform.

The final post in this series is the public release announcement: what LoraDB
is today, what is included, what is intentionally not included yet, and where
the project goes next.
