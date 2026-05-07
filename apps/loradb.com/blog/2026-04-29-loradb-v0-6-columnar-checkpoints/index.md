---
slug: loradb-v0-6-columnar-checkpoints
title: "LoraDB v0.6: columnar snapshots and managed WAL checkpoints"
description: "LoraDB v0.6 upgrades persistence with the columnar LORACOL1 snapshot format, managed commit-count WAL checkpoints, and aligned binding APIs across Rust, Node, WASM, Python, Go, and Ruby."
authors: [loradb]
tags: [release-notes, announcement, persistence, operations]
image: /img/blog/loradb-v0-6-columnar-checkpoints-header.png
---

![LoraDB v0.6 — columnar LORACOL1 snapshots and managed WAL checkpoints.](/img/blog/loradb-v0-6-columnar-checkpoints-header.png)

LoraDB v0.6 is a persistence hardening release.

v0.3 introduced snapshots. v0.4 introduced WAL recovery. v0.5 made the
engine stream results and tightened container-backed persistence. v0.6
brings those persistence pieces into a cleaner shape: columnar snapshots,
managed WAL checkpoints, and binding APIs that describe the same
operational model from every runtime.

The headline is simple: the persistence story is no longer "one file"
and "one log" as separate ideas. Snapshots are now the checkpoint
artifact the WAL can lean on.

<!-- truncate -->

## What Changed

The short list:

- the current database snapshot format is now columnar `LORACOL1`;
- snapshot envelopes carry a BLAKE3 checksum;
- snapshot metadata supports compression and encryption options;
- WAL checkpoints can stamp snapshots with a durable `walLsn` fence;
- raw-WAL helpers can write managed snapshots after N committed
  transactions;
- binding APIs were aligned for WAL and snapshot operations;
- historical persistence docs were refreshed so v0.3 and v0.4 readers
  see the current API names and current guarantees;
- snapshot decoding and checkpoint failure paths were hardened after
  release.

That last group matters almost as much as the feature list. A database
release is only useful if the error paths get clearer as the system grows.

## From LORASNAP To LORACOL1

The original snapshot work proved the operator contract:

- write a full graph to a temporary file;
- `fsync`;
- rename atomically;
- load by swapping the live graph in one shot;
- report metadata back to the caller.

That contract still stands. v0.6 changes the payload under the envelope.
`LORACOL1` is the format the current engine writes: a columnar database
snapshot with explicit metadata and checksum validation.

The format upgrade is part of the same philosophy as the rest of LoraDB:
keep the public boundary small, but make the internals more honest about
the work they need to do. A snapshot is not just "some bytes." It is a
database artifact that should be validated, described, restored, and used
as a recovery point.

## Checkpoints Make The Staircase Real

The persistence staircase now reads cleanly:

1. **In-memory.** Start fresh. No durability. Fastest loop.
2. **Snapshot-only.** Save and restore a point-in-time graph.
3. **WAL-backed.** Replay committed writes after a process restart.
4. **Snapshot plus WAL checkpointing.** Bound replay by writing
   snapshots stamped with the WAL fence they represent.

v0.6 strengthens the fourth step.

A checkpoint snapshot is a normal snapshot with a meaningful `walLsn`.
Recovery can load that graph state, then replay committed WAL records
newer than the checkpoint. That keeps the log useful without letting
replay grow forever.

## Managed Commit-Count Snapshots

Raw-WAL helpers can now write managed snapshots after N committed
transactions.

That is intentionally not a full scheduler. There is still no wall-clock
checkpoint daemon hidden inside the engine. If a production process wants
time-based checkpoints, it should run that policy from the host process,
systemd, cron, an operator, or eventually the hosted platform.

The commit-count option covers a different need: "after this many writes,
bound recovery again." It is a small operational primitive with a clear
contract, and it gives filesystem-backed bindings a practical default path
without pretending LoraDB is already a full managed service.

## Binding Alignment

v0.6 also cleaned up the API story across the surfaces:

- Rust exposes the reference checkpoint, snapshot, WAL, and sync-mode
  controls.
- `lora-server` keeps the operator-facing admin routes.
- Node, Python, Go, and Ruby can open container-backed named databases or
  explicit WAL directories.
- WASM remains pathless and snapshot-oriented, using byte/source APIs
  because the browser has no ordinary filesystem path.

The binding names differ where the host language expects them to differ.
The mental model should not.

That is why the v0.3 and v0.4 posts were refreshed instead of left as
stale artifacts. They are historical release notes, but readers should not
copy an API that the current packages no longer expose.

## What Is Still Honest

v0.6 does not turn LoraDB into a distributed database.

The boundaries remain:

- one process owns the graph;
- the HTTP admin surface still needs your ingress/auth boundary;
- checkpoints are local filesystem artifacts;
- raw-WAL helpers can checkpoint by commit count, not wall-clock time;
- there is no hosted control plane yet;
- WASM snapshots are caller-managed bytes.

Those limits are not a weakness in the release story. They are the reason
the story is readable.

## How v0.6 Fits The Journey

v0.1 asked whether a small Rust graph database could feel good to use.
v0.2 made AI context a native value, not a bolt-on store. v0.3 made the
graph portable. v0.4 made committed writes recoverable. v0.5 made larger
query results and binding streams practical.

v0.6 makes the persistence layer feel less like a set of features and more
like a system.

That is the journey LoraDB is on: not "ship every database feature at
once," but make each boundary stronger before building the next one on top.

## Read Next

- [Snapshots](/docs/snapshot)
- [WAL and checkpoints](/docs/wal)
- [HTTP server quickstart](/docs/getting-started/server)
- [HTTP API reference](/docs/api/http)

v0.6 is the release where snapshots stop being only a manual save file and
become the checkpoint language of the database.

