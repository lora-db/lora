---
slug: loradb-release-journey
title: "The LoraDB release journey so far"
description: "A narrative map of LoraDB from public v0.1 through vectors, snapshots, WAL recovery, streaming execution, columnar checkpoints, and the current performance/concurrency work."
authors: [loradb]
tags: [founder-notes, release-notes, architecture, performance]
image: /img/blog/loradb-release-journey-header.png
---

![The LoraDB release journey — v0.1 through v0.6 milestones.](/img/blog/loradb-release-journey-header.png)

LoraDB has moved quickly since the public release, so it is worth
stepping back from the version numbers and looking at the journey.

The individual posts tell the detail of each release. This one is the
map: why the releases landed in this order, what each one proved, and
how the current work fits the long arc from "fast local graph engine" to
"database people can trust in the product loop."

<!-- truncate -->

## v0.1: Make The Core Public

The first public release made the bet visible.

LoraDB shipped as a Rust in-memory graph database with a Cypher-shaped
query engine, an HTTP server, and early bindings. The important part was
not that every database feature existed. It was that the core pipeline was
there and readable:

- parse;
- analyze;
- plan;
- execute;
- store graph values;
- return results through Rust, HTTP, and bindings.

That release set the product tone. LoraDB would earn trust locally before
asking anyone to trust a hosted platform.

## v0.2: Put Vectors Inside The Graph

The second release added first-class `VECTOR` values.

That was not a pivot into being a vector database. It was the opposite:
an argument that embeddings are more useful when they live next to labels,
properties, and relationships.

Similarity can find candidates. The graph can explain, filter, and rank
them with context. v0.2 made that possible in one value model and one
Cypher surface, across the bindings.

## v0.3: Save The Graph

v0.3 added manual snapshots.

That sounds small until you look at the trust boundary it creates. Before
snapshots, LoraDB was a fast in-memory graph that disappeared with the
process. After snapshots, a developer could carry graph state across
sessions, ship a seed file, back up a notebook, or restore a service from
one artifact.

Snapshots were not marketed as full durability. They were deliberately
point-in-time persistence. That honesty mattered because the snapshot
contract became the base for checkpoints later.

## v0.4: Recover Committed Writes

v0.4 added the WAL.

This is where persistence became continuous on filesystem-backed
surfaces. Committed writes could be replayed after restart, torn log tails
could be handled, and the server gained admin routes for checkpointing and
WAL management.

The story changed from "save when you decide" to "recover what committed."
Snapshots stayed important. They became the thing a WAL can checkpoint
against.

## v0.5: Stream The Query Path

v0.5 shifted attention from durability to flow.

The pull-based executor, owned query streams, client stream APIs, property
indexes, and memory indexing work all point at the same product feeling:
the database should not force large graph work through one oversized
materialized response.

That is a necessary step for a local-first graph engine. If LoraDB is
going to sit close to applications, it has to hand results to those
applications in a shape they can process naturally.

The v0.5 patch releases also tightened WAL persistence edges. That is how
trust accumulates: feature, use, fix, repeat.

## v0.6: Turn Persistence Into A System

v0.6 upgraded snapshots into the current columnar `LORACOL1` format and
connected them more tightly to WAL checkpoints.

This is the release where the persistence staircase became easy to
explain:

- in-memory for the fastest local loop;
- snapshots for point-in-time graph artifacts;
- WAL for committed write recovery;
- checkpoint snapshots to bound replay.

The docs were refreshed around that model because old release notes can
become dangerous if they teach old API names. The posts now preserve the
history while pointing readers at the current shape.

## Current Work: Make The Hot Path Concurrent

The latest commits after v0.6 continue the same thread.

Plan caching reduces repeated parse/analyze/compile work. Lock-free reads
via an `ArcSwap` snapshot store make reads cheaper under concurrency.
Arc-wrapped records make snapshot clones less expensive. Optimistic writes,
per-record validation, a lock table, mutation write sets, and merge replay
move the write path toward finer-grained concurrency.

The refactors that followed are not cosmetic. Splitting large modules in
the store, database, WAL, snapshot, parser, analyzer, executor, server, and
bindings makes the next round of performance work less risky. A system gets
faster when its parts are easier to reason about.

## The Pattern

The release pattern is now visible:

1. **Make the developer loop good.** In-memory, readable, local,
   Cypher-shaped.
2. **Add the value model people need.** Vectors belong in the graph when
   context matters.
3. **Add persistence one honest contract at a time.** Snapshot first, WAL
   second, checkpoints after both are clear.
4. **Improve execution flow.** Stream rows, index common lookups, avoid
   unnecessary materialization.
5. **Harden concurrency and structure.** Make reads cheap, writes safer,
   and internals easier to change without breaking the product surface.

That pattern matters more than any single feature. It is how LoraDB keeps
the product journey coherent while the engine grows quickly.

## What Readers Should Take Away

LoraDB is not trying to jump straight from a public repo to a hosted graph
cloud by announcement.

It is building the trust layers in order:

- can I understand it;
- can I query it;
- can I store the values my workload needs;
- can I save and recover it;
- can I stream results;
- can it stay fast under concurrent use;
- can someone else operate it for me later.

That is the journey of LoraDB so far. The next releases should keep making
that journey easier to follow, not just longer.

