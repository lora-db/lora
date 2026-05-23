#!/usr/bin/env node
/**
 * Regression-check the `memory` bench output against the checked-in
 * baseline at `crates/lora-database/benches/memory_baseline.json`.
 *
 *   cargo bench -p lora-database --bench memory -- --quick 2>&1 \
 *     | node scripts/check-mem-bench.mjs
 *
 * Exits 0 when every scenario's `total_bytes` is within
 * `baseline.total_bytes * (1 + tolerance_pct/100)`,
 * 1 when any scenario exceeds that tolerance,
 * 2 when the input is unparseable or a scenario is missing from the
 * baseline (a new scenario landed but the baseline wasn't refreshed).
 *
 * The memory bench prints one `memreport scenario=… total=… …` line
 * per build / after-query phase, plus Criterion's usual timing lines
 * (which this script ignores). See
 * `crates/lora-database/benches/memory.rs` for the line shape and
 * `MemoryReport` for the methodology behind the numbers.
 *
 * Flags:
 *   --baseline <path>   Override baseline JSON path.
 *   --update            Rewrite the baseline from the incoming output
 *                       (preserves `_meta` + per-scenario `tolerance_pct`).
 *   --input <path>      Read bench output from a file instead of stdin.
 *   --format <text|markdown>
 *                       Output format. `text` (default) prints a
 *                       fixed-width table for CI logs; `markdown`
 *                       prints a GitHub-flavoured markdown table
 *                       suitable for `$GITHUB_STEP_SUMMARY`.
 *
 * Intentionally separate from `check-perf-smoke.mjs` because:
 *   - the inputs are different (memreport key=value lines vs Criterion
 *     bencher format),
 *   - the regression metric is bytes-retained, not ns/iter,
 *   - the failure narrative is "memory leak / shape change", not
 *     "perf canary died".
 */

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT_FILE = fileURLToPath(import.meta.url);
const SCRIPT_DIR = path.dirname(SCRIPT_FILE);
const REPO_ROOT = path.resolve(SCRIPT_DIR, "..");
const DEFAULT_BASELINE = path.join(
  REPO_ROOT,
  "crates/lora-database/benches/memory_baseline.json",
);
const DEFAULT_TOLERANCE_PCT = 10;

function parseArgs(argv) {
  const opts = {
    baseline: DEFAULT_BASELINE,
    update: false,
    input: null,
    format: "text",
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    switch (a) {
      case "--baseline":
        opts.baseline = argv[++i];
        break;
      case "--update":
        opts.update = true;
        break;
      case "--input":
        opts.input = argv[++i];
        break;
      case "--format":
        opts.format = argv[++i];
        if (opts.format !== "text" && opts.format !== "markdown") {
          fail(`--format must be 'text' or 'markdown' (got ${opts.format})`, 2);
        }
        break;
      case "-h":
      case "--help":
        printHelp();
        process.exit(0);
        break;
      default:
        fail(`unknown argument: ${a}`, 2);
    }
  }
  return opts;
}

function printHelp() {
  console.log(
    [
      "usage: node scripts/check-mem-bench.mjs [--baseline <path>]",
      "                                        [--update]",
      "                                        [--input <path>]",
      "                                        [--format text|markdown]",
      "",
      "Pipe memory bench output into stdin (default) or pass --input.",
    ].join("\n"),
  );
}

function fail(msg, code = 2) {
  console.error(`check-mem-bench: ${msg}`);
  process.exit(code);
}

function readInput(inputPath) {
  if (inputPath) return fs.readFileSync(inputPath, "utf8");
  return fs.readFileSync(0, "utf8");
}

// One bench line:
//   memreport scenario=chain_1000.build nodes=1000 rels=999 total=665387 ...
//             bytes_per_node=213.0 bytes_per_rel=100.0
//
// Order of keys after `scenario=` varies (the bench inserts them into a
// BTreeMap before printing), so we parse them all into a record and
// pick out the few we care about.
const LINE_RE = /^memreport\s+(.*)$/;
function parseLines(text) {
  const out = {};
  for (const rawLine of text.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line) continue;
    const m = LINE_RE.exec(line);
    if (!m) continue;
    const fields = {};
    for (const tok of m[1].split(/\s+/)) {
      const eq = tok.indexOf("=");
      if (eq <= 0) continue;
      const k = tok.slice(0, eq);
      const v = tok.slice(eq + 1);
      const n = Number(v);
      fields[k] = Number.isFinite(n) ? n : v;
    }
    if (typeof fields.scenario !== "string") continue;
    out[fields.scenario] = fields;
  }
  return out;
}

function loadBaseline(file) {
  let raw;
  try {
    raw = fs.readFileSync(file, "utf8");
  } catch (e) {
    fail(`couldn't read baseline ${file}: ${e.message}`);
  }
  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch (e) {
    fail(`baseline ${file} is not valid JSON: ${e.message}`);
  }
  if (!parsed || typeof parsed !== "object" || !parsed.benchmarks) {
    fail(`baseline ${file} is missing "benchmarks"`);
  }
  return parsed;
}

function writeBaseline(file, baseline) {
  const text = JSON.stringify(baseline, null, 2) + "\n";
  fs.writeFileSync(file, text);
}

function regressionLimit(baselineEntry, defaultPct) {
  const pct = Number.isFinite(baselineEntry.tolerance_pct)
    ? baselineEntry.tolerance_pct
    : defaultPct;
  return baselineEntry.total_bytes * (1 + pct / 100);
}

function formatRow(row) {
  return [
    row.scenario.padEnd(30),
    String(row.baseline).padStart(12),
    String(row.observed).padStart(12),
    row.delta.toFixed(2).padStart(8) + "%",
    row.status.padEnd(7),
  ].join(" │ ");
}

function renderText(rows) {
  const header =
    [
      "scenario".padEnd(30),
      "baseline".padStart(12),
      "observed".padStart(12),
      "Δ".padStart(9),
      "status".padEnd(7),
    ].join(" │ ") + "\n";
  const sep = "─".repeat(header.length) + "\n";
  return header + sep + rows.map(formatRow).join("\n") + "\n";
}

function renderMarkdown(rows) {
  const head = "| scenario | baseline | observed | Δ | status |";
  const sep = "|---|---:|---:|---:|---|";
  const body = rows
    .map(
      (r) =>
        `| ${r.scenario} | ${r.baseline} | ${r.observed} | ${r.delta.toFixed(2)}% | ${r.status} |`,
    )
    .join("\n");
  return [head, sep, body, ""].join("\n");
}

function main() {
  const opts = parseArgs(process.argv.slice(2));
  const text = readInput(opts.input);
  const observed = parseLines(text);

  if (Object.keys(observed).length === 0) {
    fail(
      "no `memreport` lines found in input — did the bench run? (try `cargo bench -p lora-database --bench memory -- --quick`)",
    );
  }

  if (opts.update) {
    const baseline = loadBaseline(opts.baseline);
    const tolerances = {};
    for (const [scenario, entry] of Object.entries(baseline.benchmarks)) {
      tolerances[scenario] = entry.tolerance_pct ?? DEFAULT_TOLERANCE_PCT;
    }
    const next = { _meta: baseline._meta ?? {}, benchmarks: {} };
    next._meta.last_updated = new Date().toISOString().slice(0, 10);
    for (const [scenario, row] of Object.entries(observed)) {
      const entry = {
        total_bytes: row.total ?? 0,
        tolerance_pct: tolerances[scenario] ?? DEFAULT_TOLERANCE_PCT,
      };
      if (typeof row.bytes_per_node === "number") {
        entry.bytes_per_node = row.bytes_per_node;
      }
      if (typeof row.bytes_per_rel === "number") {
        entry.bytes_per_rel = row.bytes_per_rel;
      }
      next.benchmarks[scenario] = entry;
    }
    writeBaseline(opts.baseline, next);
    console.log(`wrote ${Object.keys(next.benchmarks).length} scenarios to ${opts.baseline}`);
    return;
  }

  const baseline = loadBaseline(opts.baseline);
  const defaultPct = baseline._meta?.default_tolerance_pct ?? DEFAULT_TOLERANCE_PCT;
  const rows = [];
  let regressions = 0;
  let missing = 0;

  for (const [scenario, entry] of Object.entries(baseline.benchmarks)) {
    const obs = observed[scenario];
    if (!obs) {
      rows.push({
        scenario,
        baseline: entry.total_bytes,
        observed: "—",
        delta: 0,
        status: "MISSING",
      });
      missing += 1;
      continue;
    }
    const observedBytes = obs.total ?? 0;
    const limit = regressionLimit(entry, defaultPct);
    const delta = ((observedBytes - entry.total_bytes) / entry.total_bytes) * 100;
    const regressed = observedBytes > limit;
    if (regressed) regressions += 1;
    rows.push({
      scenario,
      baseline: entry.total_bytes,
      observed: observedBytes,
      delta,
      status: regressed ? "FAIL" : "OK",
    });
  }

  const out = opts.format === "markdown" ? renderMarkdown(rows) : renderText(rows);
  process.stdout.write(out);

  if (missing > 0) {
    process.stderr.write(
      `\ncheck-mem-bench: ${missing} scenario(s) missing from bench output\n`,
    );
    process.exit(2);
  }
  if (regressions > 0) {
    process.stderr.write(
      `\ncheck-mem-bench: ${regressions} scenario(s) regressed\n`,
    );
    process.exit(1);
  }
}

main();
