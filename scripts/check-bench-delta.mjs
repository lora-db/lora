#!/usr/bin/env node
/**
 * Compare two Criterion `--output-format bencher` logs and fail when the
 * current run is slower than the baseline by more than a threshold.
 *
 * Example:
 *   cargo bench -p lora-database --bench concurrency_guard_benchmarks \
 *       -- --output-format bencher > /tmp/lora-before.bencher
 *   cargo bench -p lora-database --bench concurrency_guard_benchmarks \
 *       -- --output-format bencher > /tmp/lora-after.bencher
 *   node scripts/check-bench-delta.mjs \
 *       --baseline /tmp/lora-before.bencher \
 *       --current /tmp/lora-after.bencher \
 *       --threshold 1.15
 */

import fs from "node:fs";

const DEFAULT_THRESHOLD = 1.15;

function fail(message, code = 2) {
  console.error(`check-bench-delta: ${message}`);
  process.exit(code);
}

function parseArgs(argv) {
  const opts = {
    baseline: null,
    current: null,
    threshold: DEFAULT_THRESHOLD,
  };

  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    switch (arg) {
      case "--baseline":
        opts.baseline = argv[++i];
        break;
      case "--current":
        opts.current = argv[++i];
        break;
      case "--threshold":
        opts.threshold = Number(argv[++i]);
        if (!Number.isFinite(opts.threshold) || opts.threshold <= 1) {
          fail(`--threshold must be a number > 1 (got ${argv[i]})`);
        }
        break;
      case "-h":
      case "--help":
        printHelp();
        process.exit(0);
      default:
        fail(`unknown argument: ${arg}`);
    }
  }

  if (!opts.baseline) {
    fail("--baseline <path> is required");
  }
  return opts;
}

function printHelp() {
  console.log(
    [
      "usage: node scripts/check-bench-delta.mjs --baseline <before.bencher>",
      "                                          [--current <after.bencher>]",
      "                                          [--threshold <n>]",
      "",
      "When --current is omitted, the current bencher output is read from stdin.",
    ].join("\n"),
  );
}

function readText(file, label) {
  try {
    return file ? fs.readFileSync(file, "utf8") : fs.readFileSync(0, "utf8");
  } catch (error) {
    fail(`cannot read ${label}: ${error.message}`);
  }
}

const BENCH_RE =
  /^test\s+(\S+)\s+\.\.\.\s+bench:\s*([0-9][0-9_,]*)\s+ns\/iter\s*\(\+\/-\s*([0-9][0-9_,]*)\)/;

function parseBencher(text, label) {
  const results = {};
  for (const rawLine of text.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line) continue;
    const match = BENCH_RE.exec(line);
    if (!match) continue;
    results[match[1]] = {
      ns: Number(match[2].replaceAll(/[_,]/g, "")),
      err: Number(match[3].replaceAll(/[_,]/g, "")),
    };
  }
  if (Object.keys(results).length === 0) {
    fail(
      `no bencher-format lines found in ${label}; pass '-- --output-format bencher' to cargo bench`,
    );
  }
  return results;
}

function fmtNs(ns) {
  if (ns >= 1e9) return `${(ns / 1e9).toFixed(2)} s`;
  if (ns >= 1e6) return `${(ns / 1e6).toFixed(2)} ms`;
  if (ns >= 1e3) return `${(ns / 1e3).toFixed(2)} us`;
  return `${ns.toFixed(0)} ns`;
}

function fmtRatio(ratio) {
  return `${ratio.toFixed(3)}x`;
}

function col(value, width) {
  return String(value).padEnd(width);
}

function main() {
  const opts = parseArgs(process.argv.slice(2));
  const baseline = parseBencher(readText(opts.baseline, opts.baseline), opts.baseline);
  const current = parseBencher(
    readText(opts.current, opts.current ?? "stdin"),
    opts.current ?? "stdin",
  );

  const rows = [];
  let failed = 0;
  let missing = 0;
  let newlyAdded = 0;

  for (const name of Object.keys(current).sort()) {
    const before = baseline[name];
    const after = current[name];
    if (!before) {
      newlyAdded++;
      rows.push({
        name,
        before: "-",
        after: fmtNs(after.ns),
        delta: "-",
        status: "NEW",
      });
      continue;
    }

    const ratio = after.ns / before.ns;
    const status = ratio > opts.threshold ? "REGRESSED" : "ok";
    if (status === "REGRESSED") failed++;
    rows.push({
      name,
      before: fmtNs(before.ns),
      after: fmtNs(after.ns),
      delta: fmtRatio(ratio),
      status,
    });
  }

  for (const name of Object.keys(baseline)) {
    if (!current[name]) {
      missing++;
      rows.push({
        name,
        before: fmtNs(baseline[name].ns),
        after: "-",
        delta: "-",
        status: "MISSING",
      });
    }
  }

  const nameWidth = Math.max(5, ...rows.map((row) => row.name.length));
  const beforeWidth = Math.max(8, ...rows.map((row) => row.before.length));
  const afterWidth = Math.max(7, ...rows.map((row) => row.after.length));
  const deltaWidth = Math.max(7, ...rows.map((row) => row.delta.length));

  console.log(
    `${col("BENCH", nameWidth)}  ${col("BEFORE", beforeWidth)}  ${col(
      "AFTER",
      afterWidth,
    )}  ${col("RATIO", deltaWidth)}  STATUS`,
  );
  for (const row of rows) {
    console.log(
      `${col(row.name, nameWidth)}  ${col(row.before, beforeWidth)}  ${col(
        row.after,
        afterWidth,
      )}  ${col(row.delta, deltaWidth)}  ${row.status}`,
    );
  }

  console.log("");
  console.log(`check-bench-delta: threshold ${fmtRatio(opts.threshold)}`);

  if (failed || missing || newlyAdded) {
    if (failed) {
      console.log(
        `check-bench-delta: ${failed} benchmark(s) exceeded the allowed slowdown.`,
      );
    }
    if (missing) {
      console.log(`check-bench-delta: ${missing} baseline benchmark(s) are missing.`);
    }
    if (newlyAdded) {
      console.log(
        `check-bench-delta: ${newlyAdded} current benchmark(s) were not in the baseline.`,
      );
    }
    process.exit(1);
  }

  console.log(`check-bench-delta: ${rows.length} benchmark(s) within threshold.`);
}

main();
