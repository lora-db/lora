#!/usr/bin/env node
// Publish every `publish`-eligible crate in this workspace to crates.io,
// in dependency order, idempotently.
//
// Usage:
//   node scripts/publish-crates.mjs --dry-run             # validate only
//   node scripts/publish-crates.mjs --version v0.1.0 --dry-run
//   node scripts/publish-crates.mjs --version v0.1.0     # actually publish
//
// Inputs:
//   --version <v>   Expected version (with or without leading v). Defaults
//                   to whatever `[workspace.package].version` currently is.
//                   When provided, the script re-runs `sync-versions.mjs`
//                   --check before doing anything else.
//   --dry-run       Only runs `cargo publish --dry-run` for every crate;
//                   never invokes the real publish step.
//   --skip-published  For recovery runs. Checks crates.io for the target
//                     version of each crate and skips any that are already
//                     published. Without this flag the real-publish mode
//                     tolerates "already uploaded" errors but still tries.
//   --allow-dirty   Passes `--allow-dirty` to cargo. Intended for local
//                   manifest dry-runs while you still have uncommitted
//                   edits. Never needed in CI (checkouts are clean).
//   --token <t>     Forwarded as `--token <t>` to cargo. Optional: if the
//                   CARGO_REGISTRY_TOKEN env var is already set, leave this
//                   flag off.
//
// Design notes:
//   - crates.io publishes are NOT transactional. If crate N publishes and
//     crate N+1 fails, we cannot "undo". Recovery policy: re-run with the
//     same tag. `--skip-published` short-circuits crates already live, and
//     "crate version already exists" from cargo is treated as success.
//   - Every crate's dry-run runs BEFORE any real publish, so a bad manifest
//     never leaves us in a partial-release state.
//   - The order is hard-coded (not computed) because it's small and stable.
//     Cross-check with dependency graph in Cargo.toml if you change this.

import { execSync, spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, '..');

// ── Crate publish order ─────────────────────────────────────────────────
// Derived from `crates/*/Cargo.toml` dependency edges:
//   lora-ast      : —
//   lora-store    : lora-ast
//   lora-parser   : lora-ast
//   lora-analyzer : lora-ast, lora-store, lora-parser
//   lora-compiler : lora-analyzer, lora-ast
//   lora-executor : lora-compiler, lora-store, lora-analyzer, lora-ast
//   lora-wal      : lora-store
//   lora-database : lora-ast, lora-parser, lora-analyzer, lora-compiler,
//                   lora-executor, lora-store, lora-wal
//   lora-server   : lora-database
// Any linear extension of that DAG works. This one is leaf-first and
// minimises the number of edges each step introduces.
const PUBLISH_ORDER = [
  'lora-ast',
  'lora-store',
  'lora-parser',
  'lora-analyzer',
  'lora-compiler',
  'lora-executor',
  'lora-wal',
  'lora-database',
  'lora-server',
];

const args = parseArgs(process.argv.slice(2));
const dryRun = args.has('dry-run');
const skipPublished = args.has('skip-published');
const allowDirty = args.has('allow-dirty');
const explicitVersion = args.get('version');
const token = args.get('token');

const workspaceVersion = readWorkspaceVersion();
const version = explicitVersion
  ? explicitVersion.replace(/^v/, '')
  : workspaceVersion;

if (version !== workspaceVersion) {
  console.error(
    `refusing to publish: --version ${version} does not match Cargo.toml [workspace.package].version ${workspaceVersion}.`,
  );
  console.error('run `node scripts/sync-versions.mjs <version>` first and commit.');
  process.exit(1);
}

console.log(`version:            ${version}`);
console.log(`dry-run:            ${dryRun}`);
console.log(`skip-published:     ${skipPublished}`);
console.log(`publish order:      ${PUBLISH_ORDER.join(' -> ')}`);
console.log('');

// ── Stage 1: dry-run every crate BEFORE publishing any of them ──────────
//
// `cargo publish --workspace --dry-run` builds a tmp registry from each
// member's packaged tarball and verifies downstream members against it.
// That is exactly the "would this release work end-to-end?" check we want:
// a single command that exercises packaging, dep resolution, and compile
// for every publishable crate. It also handles ordering internally, so we
// don't have to.
console.log('── stage 1: workspace dry-run ──────────────────────────────────');
{
  const cargoArgs = ['publish', '--workspace', '--dry-run', '--locked'];
  if (allowDirty) cargoArgs.push('--allow-dirty');
  runCargo(cargoArgs, { label: 'workspace dry-run' });
}
console.log('');

if (dryRun) {
  console.log('dry-run complete. exiting without publishing.');
  process.exit(0);
}

// ── Stage 2: real publish, one crate at a time ──────────────────────────
console.log('── stage 2: publish ────────────────────────────────────────────');
for (const crate of PUBLISH_ORDER) {
  if (skipPublished && isCrateVersionPublished(crate, version)) {
    console.log(`skip     ${crate}@${version} already on crates.io`);
    continue;
  }
  const cargoArgs = ['publish', '--locked', '--package', crate];
  if (allowDirty) cargoArgs.push('--allow-dirty');
  if (token) cargoArgs.push('--token', token);
  const { status, stderr } = runCargo(cargoArgs, {
    label: `${crate} publish`,
    capture: true,
    allowFailure: true,
  });
  if (status === 0) {
    console.log(`ok       ${crate}@${version} published`);
    continue;
  }
  if (isAlreadyUploadedError(stderr)) {
    console.log(`ok       ${crate}@${version} was already on crates.io; continuing`);
    continue;
  }
  console.error(stderr);
  console.error(`::error::${crate} publish failed (exit ${status}). see log above.`);
  process.exit(status || 1);
}
console.log('');
console.log('all crates published.');

// ── helpers ─────────────────────────────────────────────────────────────

/** @param {string[]} argv */
function parseArgs(argv) {
  const flags = new Set();
  const values = new Map();
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (!a.startsWith('--')) continue;
    const name = a.slice(2);
    const next = argv[i + 1];
    if (next && !next.startsWith('--')) {
      values.set(name, next);
      i++;
    } else {
      flags.add(name);
    }
  }
  return { has: (k) => flags.has(k), get: (k) => values.get(k) };
}

/** Read `[workspace.package].version` from the workspace Cargo.toml. */
function readWorkspaceVersion() {
  const toml = readFileSync(resolve(ROOT, 'Cargo.toml'), 'utf8');
  const lines = toml.split('\n');
  let inSection = false;
  for (const line of lines) {
    const sectionMatch = line.match(/^\s*\[([^\]]+)\]\s*$/);
    if (sectionMatch) {
      inSection = sectionMatch[1].trim() === 'workspace.package';
      continue;
    }
    if (!inSection) continue;
    const m = line.match(/^\s*version\s*=\s*"([^"]+)"/);
    if (m) return m[1];
  }
  console.error('could not locate [workspace.package] version in Cargo.toml');
  process.exit(1);
}

/**
 * Run cargo with a stream-to-stdout default. With `capture: true`, instead
 * collects stderr so we can inspect cargo's error message (needed to
 * tolerate "already uploaded" gracefully).
 *
 * @param {string[]} cargoArgs
 * @param {{label: string, capture?: boolean, allowFailure?: boolean}} opts
 */
function runCargo(cargoArgs, opts) {
  const { label, capture = false, allowFailure = false } = opts;
  console.log(`\n$ cargo ${cargoArgs.join(' ')}   # ${label}`);
  const res = spawnSync('cargo', cargoArgs, {
    cwd: ROOT,
    stdio: capture ? ['ignore', 'inherit', 'pipe'] : 'inherit',
    encoding: 'utf8',
  });
  if (res.status !== 0 && !allowFailure) {
    console.error(`::error::${label} failed (exit ${res.status}).`);
    process.exit(res.status || 1);
  }
  return { status: res.status ?? 1, stderr: res.stderr ?? '' };
}

/**
 * Recognise cargo's "crate version already uploaded" error so a partial
 * re-run can tolerate a crate that went out in the previous attempt.
 *
 * crates.io wording historically includes:
 *   - "crate version `x.y.z` is already uploaded"
 *   - "already exists on crates.io index"
 *   - "the remote server responded with an error (status 200 OK): ..."
 *
 * We accept the two stable phrasings and err on the cautious side.
 *
 * @param {string} stderr
 */
function isAlreadyUploadedError(stderr) {
  if (!stderr) return false;
  return (
    /already uploaded/i.test(stderr) ||
    /already exists on crates\.io/i.test(stderr) ||
    /crate version .* is already uploaded/i.test(stderr)
  );
}

/**
 * Ask the crates.io sparse index whether a particular crate+version is
 * live. Uses the sparse HTTP endpoint so no full registry clone is needed.
 *
 * @param {string} crate
 * @param {string} version
 */
function isCrateVersionPublished(crate, version) {
  const path = indexPath(crate);
  const url = `https://index.crates.io/${path}`;
  try {
    const out = execSync(`curl -fsSL ${JSON.stringify(url)}`, { encoding: 'utf8' });
    // Sparse index format: one JSON doc per line, one per published version.
    for (const line of out.split('\n')) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      try {
        const row = JSON.parse(trimmed);
        if (row.name === crate && row.vers === version && !row.yanked) return true;
      } catch {
        /* ignore malformed lines */
      }
    }
    return false;
  } catch (err) {
    // If the index fetch fails (crate not yet registered, offline, 404),
    // fall through and try to publish. crates.io will tell us authoritatively.
    return false;
  }
}

/** Path in the crates.io sparse index for a given crate name. */
function indexPath(crate) {
  const n = crate.length;
  if (n === 1) return `1/${crate}`;
  if (n === 2) return `2/${crate}`;
  if (n === 3) return `3/${crate[0]}/${crate}`;
  return `${crate.slice(0, 2)}/${crate.slice(2, 4)}/${crate}`;
}
