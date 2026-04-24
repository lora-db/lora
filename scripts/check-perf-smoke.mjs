#!/usr/bin/env node
/**
 * Regression-check the perf_smoke_benchmarks suite against a checked-in
 * baseline.
 *
 *   cargo bench -p lora-database --bench perf_smoke_benchmarks \
 *       -- --output-format bencher \
 *     | node scripts/check-perf-smoke.mjs
 *
 * Exits 0 when every bench is within `baseline.ns * threshold`,
 * 1 when any bench regresses past that multiplier,
 * 2 when the input is unparseable or a bench is missing from the baseline.
 *
 * Flags:
 *   --baseline <path>   Override baseline JSON path.
 *   --threshold <n>     Override default multiplier (default: 3.0).
 *   --update            Rewrite the baseline from the incoming bencher output
 *                       (keeps existing `_meta` + per-bench `threshold` fields).
 *   --input <path>      Read bencher output from a file instead of stdin.
 *
 * This is a "catch obvious big regressions" tool, not a measurement lab.
 * See docs/performance/perf-smoke.md.
 */

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const REPO_ROOT = path.resolve(__dirname, "..");
const DEFAULT_BASELINE = path.join(
  REPO_ROOT,
  "crates/lora-database/benches/perf_smoke_baseline.json",
);
const DEFAULT_THRESHOLD = 3.0;

// ---------- argument parsing ------------------------------------------------

function parseArgs(argv) {
  const opts = {
    baseline: DEFAULT_BASELINE,
    threshold: null,
    update: false,
    input: null,
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    switch (a) {
      case "--baseline":
        opts.baseline = argv[++i];
        break;
      case "--threshold":
        opts.threshold = Number(argv[++i]);
        if (!Number.isFinite(opts.threshold) || opts.threshold <= 1) {
          fail(`--threshold must be a number > 1 (got ${argv[i]})`, 2);
        }
        break;
      case "--update":
        opts.update = true;
        break;
      case "--input":
        opts.input = argv[++i];
        break;
      case "-h":
      case "--help":
        printHelp();
        process.exit(0);
      default:
        fail(`unknown argument: ${a}`, 2);
    }
  }
  return opts;
}

function printHelp() {
  console.log(
    [
      "usage: node scripts/check-perf-smoke.mjs [--baseline <path>]",
      "                                         [--threshold <n>]",
      "                                         [--update]",
      "                                         [--input <path>]",
      "",
      "Pipe Criterion bencher output into stdin (default) or pass --input.",
    ].join("\n"),
  );
}

// ---------- I/O -------------------------------------------------------------

function fail(msg, code = 2) {
  console.error(`check-perf-smoke: ${msg}`);
  process.exit(code);
}

function readInput(inputPath) {
  if (inputPath) {
    return fs.readFileSync(inputPath, "utf8");
  }
  // Read all of stdin.
  return fs.readFileSync(0, "utf8");
}

// ---------- bencher parsing -------------------------------------------------

// Criterion's --output-format bencher emits one line per benchmark:
//   test <id> ... bench:        123456 ns/iter (+/- 4321)
// The id preserves `/` from the benchmark group, e.g. `perf_smoke/scan_1k`.
// Criterion prints comma-grouped thousands (276,653). Nightly libtest uses
// underscores. Accept either, plus bare digits.
const BENCH_RE =
  /^test\s+(\S+)\s+\.\.\.\s+bench:\s*([0-9][0-9_,]*)\s+ns\/iter\s*\(\+\/-\s*([0-9][0-9_,]*)\)/;

function parseBencher(text) {
  const results = {};
  for (const rawLine of text.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line) continue;
    const m = BENCH_RE.exec(line);
    if (!m) continue;
    const name = m[1];
    const ns = Number(m[2].replaceAll(/[_,]/g, ""));
    const err = Number(m[3].replaceAll(/[_,]/g, ""));
    if (!Number.isFinite(ns)) continue;
    results[name] = { ns, err };
  }
  return results;
}

// ---------- baseline I/O ----------------------------------------------------

function loadBaseline(file) {
  let raw;
  try {
    raw = fs.readFileSync(file, "utf8");
  } catch (e) {
    fail(`cannot read baseline ${file}: ${e.message}`, 2);
  }
  let data;
  try {
    data = JSON.parse(raw);
  } catch (e) {
    fail(`baseline ${file} is not valid JSON: ${e.message}`, 2);
  }
  if (!data || typeof data !== "object" || !data.benchmarks) {
    fail(`baseline ${file} is missing a "benchmarks" object`, 2);
  }
  return data;
}

function writeBaseline(file, data) {
  const json = JSON.stringify(data, null, 2) + "\n";
  fs.writeFileSync(file, json);
}

// ---------- formatting ------------------------------------------------------

function fmtNs(n) {
  if (n >= 1e9) return `${(n / 1e9).toFixed(2)} s`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(2)} ms`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(2)} µs`;
  return `${n.toFixed(0)} ns`;
}

function fmtRatio(r) {
  return `${r.toFixed(2)}x`;
}

// ---------- main ------------------------------------------------------------

function main() {
  const opts = parseArgs(process.argv.slice(2));
  const raw = readInput(opts.input);
  const current = parseBencher(raw);
  const names = Object.keys(current);

  if (names.length === 0) {
    fail(
      "no bencher-format lines found on stdin. Did you pass '-- --output-format bencher' to cargo bench?",
      2,
    );
  }

  const baseline = loadBaseline(opts.baseline);
  const defaultThreshold =
    opts.threshold ??
    (baseline._meta && Number(baseline._meta.default_threshold)) ??
    DEFAULT_THRESHOLD;

  if (opts.update) {
    const updated = {
      ...baseline,
      _meta: {
        ...(baseline._meta || {}),
        default_threshold:
          (baseline._meta && baseline._meta.default_threshold) ??
          DEFAULT_THRESHOLD,
        last_updated: new Date().toISOString().slice(0, 10),
      },
      benchmarks: {},
    };
    for (const name of names) {
      const existing = baseline.benchmarks[name] || {};
      updated.benchmarks[name] = {
        ns: current[name].ns,
        ...(existing.threshold !== undefined
          ? { threshold: existing.threshold }
          : {}),
      };
    }
    writeBaseline(opts.baseline, updated);
    console.log(
      `check-perf-smoke: wrote ${names.length} benchmark(s) to ${path.relative(
        REPO_ROOT,
        opts.baseline,
      )}`,
    );
    return;
  }

  // Compare.
  const rows = [];
  let failed = 0;
  let missing = 0;

  for (const name of names) {
    const baseEntry = baseline.benchmarks[name];
    if (!baseEntry) {
      rows.push({
        name,
        baseline: "—",
        current: fmtNs(current[name].ns),
        ratio: "—",
        threshold: "—",
        status: "NEW",
      });
      missing++;
      continue;
    }
    const threshold = Number(baseEntry.threshold ?? defaultThreshold);
    const ratio = current[name].ns / baseEntry.ns;
    const regressed = ratio > threshold;
    if (regressed) failed++;
    rows.push({
      name,
      baseline: fmtNs(baseEntry.ns),
      current: fmtNs(current[name].ns),
      ratio: fmtRatio(ratio),
      threshold: fmtRatio(threshold),
      status: regressed ? "REGRESSED" : "ok",
    });
  }

  // Also check for benches in the baseline that didn't appear in the run —
  // usually means the bench was renamed or removed.
  const orphan = [];
  for (const name of Object.keys(baseline.benchmarks)) {
    if (!current[name]) orphan.push(name);
  }

  // Render.
  const col = (s, w) => String(s).padEnd(w);
  const wName = Math.max(4, ...rows.map((r) => r.name.length));
  const wBase = Math.max(10, ...rows.map((r) => r.baseline.length));
  const wCur = Math.max(10, ...rows.map((r) => r.current.length));
  const wRatio = Math.max(8, ...rows.map((r) => r.ratio.length));
  const wThr = Math.max(10, ...rows.map((r) => r.threshold.length));

  console.log(
    `${col("BENCH", wName)}  ${col("BASELINE", wBase)}  ${col(
      "CURRENT",
      wCur,
    )}  ${col("RATIO", wRatio)}  ${col("THRESHOLD", wThr)}  STATUS`,
  );
  for (const r of rows) {
    console.log(
      `${col(r.name, wName)}  ${col(r.baseline, wBase)}  ${col(
        r.current,
        wCur,
      )}  ${col(r.ratio, wRatio)}  ${col(r.threshold, wThr)}  ${r.status}`,
    );
  }

  if (orphan.length) {
    console.log("");
    console.log(
      `check-perf-smoke: ${orphan.length} baseline bench(es) not produced by this run (renamed or removed?):`,
    );
    for (const n of orphan) console.log(`  - ${n}`);
  }

  if (failed > 0) {
    console.log("");
    console.log(
      `check-perf-smoke: ${failed} benchmark(s) regressed past the configured threshold.`,
    );
    console.log(
      "If this regression is intentional, refresh the baseline with --update.",
    );
    process.exit(1);
  }
  if (missing > 0) {
    console.log("");
    console.log(
      `check-perf-smoke: ${missing} benchmark(s) are not in the baseline yet.`,
    );
    console.log("Add them by re-running with --update.");
    // Missing-from-baseline is a soft signal. Exit 1 so PRs notice and fix it.
    process.exit(1);
  }
  console.log("");
  console.log(
    `check-perf-smoke: ${rows.length} benchmark(s) within threshold.`,
  );
}

main();
