#!/usr/bin/env node
// Compute the set of workspace crates affected by the currently staged
// changes, expanded to include the reverse-dep closure (so a change in
// crate A also re-tests crates that depend on A — including via dev or
// build deps).
//
// Output:
//   "--workspace"                  → broad change, fall back to whole workspace
//   "-p crateA -p crateB ..."      → scoped set (lora-python/lora_ruby filtered out)
//   ""                             → no Rust crates touched, nothing to run
//
// Used by .husky/pre-commit. Designed to be cheap: one `cargo metadata`
// call (cached by cargo) + one `git diff --cached` call.

import { execSync } from 'node:child_process';
import { dirname, resolve } from 'node:path';

const root = execSync('git rev-parse --show-toplevel', { encoding: 'utf8' }).trim();

const staged = execSync('git diff --cached --name-only --diff-filter=ACMR', {
  encoding: 'utf8',
  cwd: root,
}).split('\n').filter(Boolean);

// Broad triggers → re-test everything. The workspace root manifest, the
// lockfile, or the toolchain pin can affect every crate.
const BROAD = new Set(['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'clippy.toml']);
if (staged.some(f => BROAD.has(f))) {
  process.stdout.write('--workspace');
  process.exit(0);
}

const meta = JSON.parse(execSync('cargo metadata --format-version=1', {
  encoding: 'utf8',
  cwd: root,
  maxBuffer: 64 * 1024 * 1024,
}));

const wsMembers = new Set(meta.workspace_members);
const memberPkgs = meta.packages.filter(p => wsMembers.has(p.id));

// crate manifest dir → crate name
const dirToCrate = new Map();
for (const p of memberPkgs) {
  dirToCrate.set(dirname(p.manifest_path), p.name);
}

// reverse-dep map: crateName → Set of workspace crates that depend on it
// (covers normal, dev, and build deps, since any of them affect tests).
const memberNames = new Set(memberPkgs.map(p => p.name));
const reverseDeps = new Map();
for (const name of memberNames) reverseDeps.set(name, new Set());
for (const p of memberPkgs) {
  for (const dep of p.dependencies) {
    if (memberNames.has(dep.name)) {
      reverseDeps.get(dep.name).add(p.name);
    }
  }
}

// Map each staged file to a workspace crate by walking up to the nearest
// known manifest dir.
const affected = new Set();
for (const f of staged) {
  if (!f.endsWith('.rs') && !f.endsWith('Cargo.toml')) continue;
  let dir = dirname(resolve(root, f));
  while (dir.length >= root.length) {
    if (dirToCrate.has(dir)) {
      affected.add(dirToCrate.get(dir));
      break;
    }
    const parent = dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
}

if (affected.size === 0) {
  process.stdout.write('');
  process.exit(0);
}

// Reverse-dep closure.
const closure = new Set(affected);
const stack = [...affected];
while (stack.length) {
  const c = stack.pop();
  for (const d of reverseDeps.get(c) ?? []) {
    if (!closure.has(d)) {
      closure.add(d);
      stack.push(d);
    }
  }
}

// These bindings need a Python/Ruby toolchain to test locally — CI handles
// them. Match the existing pre-commit exclusions.
closure.delete('lora-python');
closure.delete('lora_ruby');

if (closure.size === 0) {
  process.stdout.write('');
  process.exit(0);
}

process.stdout.write([...closure].map(c => `-p ${c}`).join(' '));
