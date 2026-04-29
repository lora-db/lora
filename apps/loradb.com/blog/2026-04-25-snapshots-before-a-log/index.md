---
slug: snapshots-before-a-log
title: "Snapshots before a log"
description: "Why LoraDB v0.3 ships manual point-in-time snapshots before any write-ahead log, what that primitive teaches, and how durability gets layered on top of it."
authors: [joost]
tags: [founder-notes, persistence, design, operations]
---

Most databases I have worked with had a write-ahead log before they had a
snapshot story. LoraDB went the other way.

:::info Current API note

This founder note is historical. The WAL has since landed on every
filesystem-backed surface, snapshots now write the columnar `LORACOL1`
format with compression/encryption support, and WASM uses
`saveSnapshot` / `loadSnapshot` instead of the older byte-specific
method names.

:::

At the time, v0.3 shipped manual point-in-time snapshots and nothing else on the
persistence side. No append-only log. No background checkpoint loop. No
continuous durability. One file on disk, taken on demand, atomic on
rename.

The order is intentional, and it is the kind of decision that is easier
to defend before the release than after.

<!-- truncate -->

## The Default Narrative For "Adding Persistence"

The default narrative for adding persistence to an in-memory database is
some version of:

1. ship a write-ahead log,
2. background checkpoints flush state to disk,
3. on boot, replay the log on top of the latest checkpoint,
4. announce "durable."

That is what mature databases do, eventually. It is also the wrong
*first* step for a project where the data model and the storage tier are
both still settling.

A WAL is a long-term commitment to a concrete write path. Every mutation
has to know how to serialize itself. Every recovery routine has to
dispatch on event type. Every release after the first one inherits the
log format, the recovery state machine, and the assumptions baked into
both. Get any of that wrong on day one and the project carries the
mistake forward — or pays for an expensive migration to fix it.

For a database that is two minor versions old and still figuring out
what its read and write paths look like, that is too much surface area
to commit to.

## What A Snapshot Teaches That A WAL Would Not

A snapshot is the lowest-risk way to learn the shape of "the graph as a
serialized artifact." It forces the project to answer a small set of
concrete questions:

- What does the file format look like? v0.3 answered with the original
  `LORASNAP` envelope; current releases write `LORACOL1`, a columnar
  snapshot payload with a BLAKE3 checksum plus compression and
  encryption metadata.
- How is the write atomic? `<path>.tmp`, `fsync`, rename over the
  target.
- How is the read atomic? Hold the store mutex, validate, swap the
  graph in one shot.
- What does the API look like across every binding? `save_snapshot`,
  `load_snapshot`, `in_memory_from_snapshot`, plus an opt-in HTTP admin
  surface.
- What does every binding return? A single `SnapshotMeta` shape with
  `formatVersion`, `nodeCount`, `relationshipCount`, and a reserved
  `walLsn`.
- What does the operator contract look like? `--snapshot-path`,
  `--restore-from`, off-by-default admin endpoints, no auth, behind
  ingress only.

None of those answers went away when the WAL arrived. A checkpoint, by
definition, is a snapshot with a WAL LSN attached. In current releases,
pure snapshots report `walLsn: null`; checkpoint snapshots stamp it
with the durable WAL fence, and recovery replays committed records
newer than that fence.

In other words, the snapshot work was not throwaway scaffolding. It is
the foundation the current log now sits on.

## What A Snapshot Is Honest About

A snapshot is not durability. It is point-in-time persistence. The
difference matters.

- A crash between two saves loses every mutation in the window.
- The store mutex is held for the duration of both save and load.
  Concurrent queries block.
- There is no incremental save. The whole graph serializes each time.
- There is no auto-cadence. Saves happen because someone called
  `save_snapshot` or hit the admin endpoint.

That set of caveats is also exactly what makes the primitive useful
right now without overcommitting. A single-node service, a notebook, a
seeded process, a service with a controlled shutdown window, a backup
cron — all of those need the property "the graph as of now, written to
one file." None of them need a continuous log to be useful.

The shapes that genuinely need a WAL — multi-node clusters, zero-data-loss
writes, mid-second crash recovery — are not the shapes LoraDB is good at
today. Building a half-finished log inside a single-process engine ends
up with a journal that is less reliable than just snapshotting more
often, and worse, with a contract that is harder to read.

## Why Honest Boundaries Matter More Than Marketing

The thing I see most often in databases that overpromised on durability
is silent data loss between two undocumented seams. That happens when
"persistent" is sold as a complete story before the moving parts have
settled — when there is durability marketing language without a clean
operator contract underneath.

The contract I want for snapshots is small enough to fit on a card:

- The save renames `<path>.tmp` over `<path>`. A crash mid-save can leave
  the `.tmp` file behind. It cannot leave a half-written `<path>`.
- The load swaps the live graph in one shot. Concurrent queries see the
  old graph or the new one. Never both. Never a partial.
- A crash between two saves loses every mutation in the window. Pick a
  cadence accordingly.
- The HTTP admin endpoints are off by default. They have no
  authentication. They are intended to sit behind your ingress.

That is what v0.3 ships. It is the smallest set of guarantees that
actually mean what they say. None of them are advertised more
aggressively than they are documented.

## Where The Persistence Story Goes Next

Three steps line up against the boundary above, in order.

**A write-ahead log.** This has since landed. Rust and `lora-server`
expose the full WAL/checkpoint/status/truncate surface; Node, Python,
Go, and Ruby expose archive-backed opens plus explicit raw-WAL helpers.
WAL checkpoints are snapshots with a meaningful `walLsn`, exactly the
shape the snapshot contract prepared for.

**Checkpoint automation.** Current raw-WAL helpers can write managed
snapshots after N committed transactions. The remaining boundary is
wall-clock scheduling: if you want time-based checkpoints, run them
from the host process, cron, systemd, or an operator loop.

**Auth on the admin surface.** Token-based auth in front of `/admin/*`
so the endpoints can be enabled on hosts that face a real network
without an external reverse proxy. Hosted operations come after that,
not before — the moment to charge people to run LoraDB for them is the
moment its operator contract is durable enough to charge for.

There is a fourth thread that is less visible but matters more: the
contract should stay easy to read while it grows. Each step adds
capability without adding ambiguity. A WAL should not turn "durable"
into a fuzzy word; it should turn the existing snapshot contract into
a strictly stronger one.

## How This Fits The Customer Journey

The persistence staircase mirrors the adoption staircase.

1. **Discovery.** A developer runs `cargo run --bin lora-server` and
   types a query. There is no persistence to think about yet.
2. **Local prototype.** They want to keep the graph between sessions.
   `--snapshot-path` and `--restore-from` are enough.
3. **Internal service.** They want graceful-shutdown saves and
   scheduled backups. `POST /admin/snapshot/save` from a systemd unit
   or a cron is enough.
4. **Production with tighter SLAs.** They need continuous durability —
   point-in-time recovery to the last committed WAL transaction, not
   the last manual save. That is where WAL-backed opens and
   checkpoints now fit.
5. **Managed operations.** They do not want to operate the engine at
   all. That is when the hosted platform takes over the snapshot
   cadence, the WAL config, and the auth boundary.

Each step adds capability the previous step's users do not regress on.
That is the point of building from a snapshot up rather than a WAL
down.

## The Feedback That Will Shape v0.4

The clearest signal for whether v0.3 lands is concrete:

- how big does your graph get, and how long does `save_snapshot` take
  at that size;
- what cadence did you settle on — seconds, minutes, on-shutdown only,
  every-N-mutations;
- did atomic rename land cleanly on your filesystem (we test on Linux
  ext4/xfs and macOS APFS);
- which binding did you use, and did the WASM pathless
  `saveSnapshot` / `loadSnapshot` surface fit your storage layer
  (IndexedDB, OPFS, a backend POST) without extra glue;
- what does your ingress look like for the admin endpoints — reverse
  proxy, Unix socket, or "not exposed at all";
- where did manual snapshots stop being acceptable for your workload,
  and what WAL sync/checkpoint cadence did you need?

The answers decide what cadence the WAL has to support, which crash
windows we need to harden first, and which surfaces need the auth
contract before the rest.

## Closing

LoraDB's persistence story is not "we shipped a quarter of a WAL." It
is "we shipped the smallest persistence primitive that means what it
says, and the next steps build on top of it without changing what we
already promised."

Snapshots before a log is the order I would pick if I had to do this
again. The right first step toward durability is not a journal. It is
a contract.

---

Canonical references:

- [Snapshots](/docs/snapshot) — file format, atomic-rename
  protocol, binding examples, and the security warning on the admin
  surface.
- [HTTP server quickstart → Snapshots, WAL, and restore](/docs/getting-started/server#snapshots-wal-and-restore)
  — `--snapshot-path` and `--restore-from` in context.
- [v0.3 release notes](/blog/loradb-v0-3-snapshots) — the team-side
  announcement, the full binding table, and the explicit list of what
  is still out of scope.
