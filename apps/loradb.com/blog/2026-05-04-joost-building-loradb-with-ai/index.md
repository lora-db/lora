---
slug: building-loradb-with-ai
title: "Building LoraDB with Claude and Codex"
description: "How Claude and Codex fit into LoraDB's engineering workflow: repository work, documentation, releases, and product direction, with the code remaining the source of truth."
authors: [joost]
tags: [founder-notes, ai, architecture, design]
---

I use Claude and Codex to build LoraDB.

That is not a positioning statement or a shortcut around the work. It is simply part of how the project gets built.

Claude and Codex help with different parts of the process: reading code, checking assumptions, shaping docs, reviewing release work, and testing whether the project is saying things the implementation can actually support. They are useful tools, but they are not the source of truth.

<!-- truncate -->

## The Practical Version

The practical version is fairly ordinary.

Codex is useful inside the repository. It can read files, follow code paths, run commands, patch docs, check links, find stale claims, and keep track of release work that is easy to get almost right.

Claude is useful around the repository. It helps with structure, narrative, product direction, and the question of whether a phrase like "production-grade" is being earned or just repeated.

I use both, and I reject both often. Their value is not that they are always right. Their value is that they make it easier to inspect the project from several angles before deciding what should change.

## Why This Matters For LoraDB

LoraDB is at an awkward and interesting stage.

It is no longer just a small in-memory graph experiment. It has a query pipeline, a graph store, vectors, snapshots, WAL, checkpointing, streaming, bindings, an HTTP server, and a growing documentation site.

But it is also not the database I want it to become yet.

The direction is a production-grade persistent and concurrent graph database.

That requires precision. It means knowing what the query engine promises, where state lives, how recovery works, which surfaces are stable, and which are still experimental. It also means being clear about where concurrency is real today, where it is conservative, and where it is still future work.

AI helps because it can keep more of that context visible while I work through the next change.

## The Risk

The risk is fluency.

It can explain a feature that does not exist. It can make a limitation sound like an implementation detail. It can blur "we want this" into "we have this." It can turn a database roadmap into a brochure if you let it.

So the rule is simple: the code wins.

If Codex writes a claim, the implementation has to back it. If Claude helps shape a post, the post still has to respect the current product. If something is work in progress, it should be called work in progress.

## Where It Helps

AI is useful for breadth.

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

A human can reason about all of that, but context switching has a cost. AI helps reduce that cost by making it faster to ask the obvious follow-up questions.

Codex is good at the repository question: where is this in the code?

Claude is good at the product question: what does this mean for the user?

Those two questions catch a lot of weak spots.

## What Still Has To Be Decided By Me

I own the direction.

I own the decision to merge or not merge, to publish or not publish, to call something stable or experimental, to make the next release about a feature or about structure.

That part cannot be outsourced.

AI can speed up parts of the work, but it does not decide what LoraDB should become.

## Where LoraDB Is Heading

The database I want is one that can be trusted when things go wrong.

Not just when a demo query runs, but when a process dies, a WAL has to replay, a checkpoint is stale, two clients write, a stream stays open, or a binding exposes a smaller surface than Rust.

Claude and Codex help by making the project easier to inspect.

They do not make it correct.

They make it harder for me to avoid the places where correctness still needs work.

## Why Say This

Because AI is part of the process, and it is better to be direct about that.

Because the way LoraDB is built affects the kind of project it becomes.

Because there is a serious version of AI-assisted engineering: use the tools, name the tools, verify the tools, and keep responsibility with the person building the system.

LoraDB is being built with Claude and Codex as part of the workflow.

That does not make the database trustworthy. It helps me find the places where the code, docs, and expectations do not line up. LoraDB still has to earn trust through implementation, tests, and boring operational behavior.
