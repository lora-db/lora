---
title: License Strategy
---

# LoraDB License Strategy

LoraDB is built for developer adoption first. The core database engine is
source-available under the Business Source License 1.1 (BSL), while the
documentation website in `apps/loradb.com` is MIT licensed.

This gives developers broad access to the database source code while protecting
the hosted platform business that funds long-term development.

## Why BSL

Database infrastructure is expensive to build and maintain. Developers need to
inspect the code, run it locally, debug behavior, and trust the system before
they adopt it. At the same time, the main commercial risk is a third party
turning the core engine into a competing hosted database service without
contributing to the company that builds it.

The BSL is designed for that balance:

- Developers can read, run, test, and modify the source.
- Companies can use LoraDB internally.
- The community can evaluate and contribute to the core.
- LoraDB can still build a hosted platform around the engine.
- Versions eventually convert to an open source license.

## Business Model

The intended model is:

1. Developers adopt LoraDB because the core engine is easy to inspect, run, and
   understand.
2. Teams use LoraDB internally and build confidence in the product.
3. Production teams that want hosted operations, managed scaling, backups,
   support, and platform features can choose the official hosted LoraDB service.

The restriction is narrow and deliberate: do not use this repository to provide
database-as-a-service, hosted APIs, managed database platforms, or similar
hosted resale offerings to third parties.

## Why The Website Is MIT

The `apps/loradb.com` folder contains the public documentation website. It is
licensed separately under MIT so documentation-site contributions, examples,
styling, and site tooling are easy to reuse.

That MIT exception does not apply to the database engine, crates, bindings,
server, tests, or root documentation unless a file explicitly says otherwise.

## Conversion To Apache 2.0

The BSL includes a Change Date. On that date, the covered version of LoraDB
converts to the Apache License 2.0.

For this release policy:

- Change Date: April 19, 2029
- Change License: Apache License 2.0

The conversion applies per released version. A version released later may have a
later Change Date.

## Practical Guidance

Use LoraDB freely for development, evaluation, internal tools, and internal
production systems. Build with it, learn from it, and contribute back.

Talk to LoraDB before building anything where customers or other third parties
access hosted LoraDB functionality through your service. That is the boundary
the license is designed to protect.
