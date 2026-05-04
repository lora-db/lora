---
slug: loradb-v0-7-ai-assisted-engineering
title: "LoraDB v0.7: AI-assisted engineering, honestly"
description: "LoraDB v0.7 is a process release about using Claude and Codex across the project: code review, refactoring, documentation, release work, and product direction without pretending AI owns the engineering."
authors: [loradb]
tags: [release-notes, announcement, architecture, ai]
---

LoraDB v0.7 is about how the project is being built.

Claude and Codex are now part of the LoraDB engineering loop. Not as a mascot,
not as a replacement team, and not as a way to avoid responsibility. They are
used across the project: code review, refactoring, documentation, release work,
architecture pressure, and product-direction thinking.

This release says that out loud.

<!-- truncate -->

## What AI Is Doing In LoraDB

AI is being used across the full scope of the project:

- reading Rust code and tracing behavior across crates;
- auditing documentation against implementation;
- helping split large ideas into smaller changes;
- reviewing error boundaries and API surfaces;
- drafting and revising release notes;
- checking whether examples still match the code;
- running local validation commands;
- comparing architecture claims against source files;
- helping reason about where persistence and concurrency should go next.

Codex is most useful inside the repository. It can inspect the codebase, edit
files, run tests, build the docs site, and catch the small mismatches that make
a database project feel less trustworthy.

Claude is most useful as a reasoning partner. It helps pressure-test the
product narrative: what is actually implemented, what is only a direction, what
is still experimental, and what a serious user would misunderstand.

Those are different jobs. Both are useful.

## What AI Is Not Doing

AI is not the source of truth.

The source of truth is still the code, the tests, the build, and the judgment
of the maintainer.

AI does not decide that a storage model is correct. It can help inspect it.

AI does not make a persistence feature production-grade. It can help reveal
where the docs overstate the guarantee.

AI does not own a release. It can help keep the checklist from being sloppy.

AI does not remove the need to understand the database. It raises the cost of
not understanding it, because it makes it easier to ask the codebase the same
question from several angles.

## Why This Matters For A Database

Databases are promise machines.

They make promises about state, recovery, isolation, durability, query
semantics, and failure. If the words around those promises drift away from the
implementation, the project becomes dangerous in a quiet way.

That is why using AI here has to be unusually disciplined.

Claude and Codex are both capable of producing confident text about behavior
that does not exist. That is the risk. The useful pattern is the opposite:
make them search, compare, challenge, and verify.

In LoraDB, AI is valuable when it increases friction around overclaiming.

## The Whole Project Scope

The AI-assisted loop is not limited to documentation.

It touches the whole project:

- the query pipeline, where parser, analyzer, compiler, and executor behavior
  must line up;
- the graph store, where internal data structures and public semantics should
  not be confused;
- the database facade, where read/write behavior, transactions, and streams
  need precise language;
- the WAL and snapshot layers, where recovery claims must be backed by actual
  code paths;
- the HTTP server and bindings, where each surface exposes a slightly different
  operational contract;
- the docs site and blog, where user expectations are shaped;
- the release process, where versions, lockfiles, builds, and posts all have to
  move together.

That breadth is exactly why AI is useful. It can keep a large amount of context
warm while the human maintainer decides what should actually change.

## Steering Toward Production-Grade Persistence And Concurrency

LoraDB is not finished.

The direction is clear, though: production-grade persistence and production-grade
concurrency.

Persistence means more than "there is a file." It means recovery paths that are
boring, WAL behavior that can be inspected, snapshot compatibility that is
documented, checkpoint semantics that are clear, and operational surfaces that
do not surprise people.

Concurrency means more than "some reads overlap." It means read behavior, write
serialization, transaction boundaries, stream lifetimes, and future
fine-grained coordination all have to be understandable.

Claude and Codex help with that by forcing more explicit structure. They help
turn "this feels right" into "where does the code prove it?"

## The Honest Boundary

This release does not claim that AI makes LoraDB production-grade.

It claims something smaller and more useful: AI is now part of the way LoraDB is
being steered toward that goal.

Used badly, AI would make the project sound more complete than it is.

Used well, it makes the project more honest about what is implemented, what is
experimental, and what still needs real engineering.

v0.7 is a marker for that working style.

## Read Next

- [Why LoraDB](/docs/why)
- [Limitations](/docs/limitations)
- [Snapshots](/docs/snapshot)
- [WAL and checkpoints](/docs/wal)
- [Graph model](/docs/concepts/graph-model)

The goal is not to hide AI usage. The goal is to use it plainly, carefully,
and in service of a better database.
