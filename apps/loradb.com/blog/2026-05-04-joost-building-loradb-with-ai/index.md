---
slug: building-loradb-with-ai
title: "Building LoraDB with Claude and Codex"
description: "Joost van Berkel on honestly using Claude and Codex across LoraDB while steering the project toward a production-grade persistent and concurrent database."
authors: [joost]
tags: [founder-notes, ai, architecture, design]
---

I use Claude and Codex to build LoraDB.

That sentence still feels a little exposed to write, which is probably why it
is worth writing plainly.

I do not mean that AI owns the project. I do not mean that a model decides the
architecture while I watch. I mean something more practical and more intimate:
Claude and Codex are part of the loop I use to think, audit, refactor, explain,
and steer LoraDB.

<!-- truncate -->

## The Honest Version

The honest version is not glamorous.

Codex helps inside the repository. It reads files, follows code paths, runs
commands, patches docs, checks links, finds stale claims, and keeps track of
the kind of mechanical release work that is easy to get almost right.

Claude helps outside and around the repository. It is useful for shaping the
argument, challenging the product direction, and asking whether a phrase like
"production-grade" is being earned or merely borrowed.

I use both.

I also reject both, often.

That is the arrangement.

## Why This Matters For LoraDB

LoraDB is at an awkward and interesting stage.

It is no longer just a small in-memory graph experiment. It has a query
pipeline, a graph store, vectors, snapshots, WAL, checkpointing, streaming,
bindings, an HTTP server, and a growing documentation site.

But it is also not the database I want it to become yet.

The direction is a production-grade persistent and concurrent graph database.

That requires structure.

It requires knowing what the query engine promises. It requires knowing where
state lives. It requires knowing how recovery works. It requires knowing which
surfaces are stable and which are experiments. It requires knowing where
concurrency is real, where it is conservative, and where it is still future
work.

AI is useful because it helps me hold that structure in view.

## The Risk

The risk is that AI is fluent.

It can explain a feature that does not exist. It can make a limitation sound
like an implementation detail. It can blur "we want this" into "we have this."
It can turn a database roadmap into a brochure if you let it.

That is exactly what I do not want.

So the rule has to be boring:

the code wins.

If Codex writes a claim, the implementation has to back it.

If Claude shapes a story, the story has to respect the current product.

If I call something work in progress, I should not hide that phrase because it
feels less impressive.

## What The AI Loop Is Good For

The loop is good for breadth.

LoraDB spans several kinds of work:

- Rust engine design;
- query semantics;
- persistence and recovery;
- concurrency boundaries;
- HTTP behavior;
- language bindings;
- documentation;
- release process;
- product narrative.

A human can absolutely reason about all of that. But context switching has a
cost, and AI helps reduce that cost.

Codex can keep asking, "where is this in the code?"

Claude can keep asking, "what does this mean for the user?"

Between those two questions, a lot of vague thinking gets squeezed out.

## What I Still Own

I own the direction.

I own the taste.

I own the decision to merge or not merge, to publish or not publish, to call
something stable or experimental, to make the next release about a feature or
about structure.

That part cannot be outsourced.

AI can give me leverage, but leverage is not judgment. It just makes judgment
matter more.

## Toward The Harder Database

The harder database is the one that can be trusted when things go wrong.

Not just when a demo query runs. When a process dies. When a WAL has to replay.
When a checkpoint is stale. When two clients write. When a stream stays open.
When a binding exposes a smaller surface than Rust. When docs are all someone
has at midnight.

That is the database I want LoraDB to move toward.

Claude and Codex help me steer in that direction by making the project easier
to interrogate.

They do not make it correct.

They make it harder for me to avoid the places where correctness still needs
work.

## Why I Am Saying This Publicly

Because hiding AI usage would be less honest than using it.

Because the way LoraDB is built is becoming part of what LoraDB is.

Because I think there is a serious version of AI-assisted engineering that is
neither hype nor shame: use the tools, name the tools, verify the tools, and
keep responsibility with the human building the system.

That is the version I want here.

LoraDB is being built with Claude and Codex in the room.

The promise is not that they make the database trustworthy.

The promise is that I will use them to keep finding the places where the
database still needs to earn that trust.
