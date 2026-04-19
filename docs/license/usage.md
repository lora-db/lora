---
title: License Usage
---

# LoraDB License Usage

LoraDB's core database code is licensed under the Business Source License 1.1
(BSL). The BSL is source-available: you can read the code, modify it, run it
internally, and evaluate it freely, but you cannot use it to offer a hosted
database service to third parties.

This page explains the project policy in plain English. It is not legal advice;
read the root `LICENSE` file for the binding terms.

## What You Can Do

You may use LoraDB for:

- Local development, testing, benchmarking, and evaluation.
- Internal tools, internal applications, and employee-facing systems.
- Production workloads that support your own business, as long as you are not
  selling or providing LoraDB itself as a hosted service to third parties.
- Forking, modifying, and distributing the LoraDB core under the same BSL terms.
- Building applications that use LoraDB as an embedded or self-hosted database
  for your own product, provided customers are not buying access to LoraDB as a
  managed database service.

The website under `apps/loradb.com` is different: that folder is licensed under
the MIT License and can be used under the terms in `apps/loradb.com/LICENSE`.

## What You Cannot Do

You may not use the BSL-licensed LoraDB core to provide LoraDB functionality as
a service to third parties.

Restricted uses include:

- Running LoraDB as database-as-a-service for customers.
- Selling hosted LoraDB clusters, hosted LoraDB projects, or managed LoraDB
  workspaces.
- Exposing LoraDB through a hosted API where third parties store, query, or
  manage their own graph data.
- Offering a competing managed graph database platform powered by LoraDB.
- Reselling hosted access to LoraDB, even if LoraDB is wrapped in another API,
  SDK, dashboard, backend-as-a-service, or developer platform.

If your business model depends on hosting LoraDB for external users, you need a
commercial agreement from LoraDB.

## Allowed Examples

| Scenario | Allowed? | Why |
| --- | --- | --- |
| A developer runs LoraDB locally to learn Cypher. | Yes | Local development is allowed. |
| A company uses LoraDB for an internal fraud graph visible only to employees. | Yes | Internal business use is allowed. |
| A startup embeds LoraDB in a self-hosted product that customers deploy in their own environment. | Usually yes | The customer is not buying hosted LoraDB access from you. |
| A team forks LoraDB and contributes parser improvements back upstream. | Yes | Modification and distribution under the BSL are allowed. |
| A documentation contributor reuses code from `apps/loradb.com`. | Yes | That folder is MIT licensed. |

## Not Allowed Examples

| Scenario | Allowed? | Why |
| --- | --- | --- |
| A cloud provider offers "LoraDB Cloud" with hosted graph databases for customers. | No | That is database-as-a-service. |
| A SaaS company exposes hosted graph storage and Cypher queries for customer data through an API. | No | Third parties are accessing LoraDB functionality as a hosted service. |
| A backend platform uses LoraDB behind the scenes to provide customer-created databases. | No | It is a managed platform or resale offering. |
| A consulting firm runs long-lived LoraDB instances for multiple clients and charges for access. | No | Hosted access for third parties is restricted. |

## Change Date

Each released version of the LoraDB core converts to the Apache License 2.0 on
the Change Date stated in the root `LICENSE` file. For this release policy, the
Change Date is April 19, 2029.

After that date, that version may be used under Apache 2.0. Newer versions may
have their own Change Date.

## When To Ask

Contact LoraDB if you want to:

- Offer LoraDB as a managed service.
- Include LoraDB in a hosted developer platform where third parties create or
  query their own databases.
- Build a commercial hosted product where the line between your application and
  hosted database access is unclear.

We want developers to adopt LoraDB freely while keeping the hosted platform
business sustainable.
