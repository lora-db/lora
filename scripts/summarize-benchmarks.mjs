#!/usr/bin/env node
/**
 * Convert Criterion/libtest "bencher" output into a compact JSON summary.
 *
 * Intended for CI artifacts: the raw log remains available for humans, while
 * this file gives dashboards, release tooling, and operators a stable view of
 * the current database benchmark state.
 */

import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT_FILE = fileURLToPath(import.meta.url);
const SCRIPT_DIR = path.dirname(SCRIPT_FILE);
const REPO_ROOT = path.resolve(SCRIPT_DIR, "..");
const DEFAULT_BASELINE = path.join(
  REPO_ROOT,
  "crates/lora-database/benches/perf_smoke_baseline.json",
);

const BENCH_RE =
  /^test\s+(\S+)\s+\.\.\.\s+bench:\s*([0-9][0-9_,]*)\s+ns\/iter\s*\(\+\/-\s*([0-9][0-9_,]*)\)/;

function parseArgs(argv) {
  const opts = {
    input: null,
    output: null,
    baseline: null,
    suite: null,
    failOnEmpty: true,
  };

  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    switch (arg) {
      case "--input":
        opts.input = argv[++i];
        break;
      case "--output":
        opts.output = argv[++i];
        break;
      case "--baseline":
        opts.baseline = argv[++i] || DEFAULT_BASELINE;
        break;
      case "--suite":
        opts.suite = argv[++i];
        break;
      case "--allow-empty":
        opts.failOnEmpty = false;
        break;
      case "-h":
      case "--help":
        printHelp();
        process.exit(0);
        break;
      default:
        fail(`unknown argument: ${arg}`);
    }
  }

  if (!opts.input) fail("--input is required");
  if (!opts.output) fail("--output is required");
  return opts;
}

function printHelp() {
  console.log(
    [
      "usage: node scripts/summarize-benchmarks.mjs --input <bencher.log>",
      "                                              --output <summary.json>",
      "                                             [--baseline <baseline.json>]",
      "                                             [--suite <name>]",
      "                                             [--allow-empty]",
      "",
      "Parses Criterion output produced with '-- --output-format bencher'.",
    ].join("\n"),
  );
}

function fail(message) {
  console.error(`summarize-benchmarks: ${message}`);
  process.exit(2);
}

function parseNumber(value) {
  return Number(value.replaceAll(/[_,]/g, ""));
}

function parseBencher(text) {
  const benchmarks = [];
  for (const rawLine of text.split(/\r?\n/)) {
    const line = rawLine.trim();
    const match = BENCH_RE.exec(line);
    if (!match) continue;

    const name = match[1];
    const ns = parseNumber(match[2]);
    const err = parseNumber(match[3]);
    if (!Number.isFinite(ns) || !Number.isFinite(err)) continue;

    const parts = name.split("/");
    benchmarks.push({
      name,
      group: parts[0] || "unknown",
      case: parts.slice(1).join("/") || parts[0] || name,
      ns_per_iter: ns,
      error_ns: err,
      relative_error_pct: ns > 0 ? round((err / ns) * 100, 3) : null,
      iterations_per_second: ns > 0 ? round(1_000_000_000 / ns, 3) : null,
    });
  }
  benchmarks.sort((a, b) => a.name.localeCompare(b.name));
  return benchmarks;
}

function readBaseline(file) {
  const raw = fs.readFileSync(file, "utf8");
  const data = JSON.parse(raw);
  if (!data || typeof data !== "object" || !data.benchmarks) {
    fail(`baseline ${file} is missing a benchmarks object`);
  }
  return data;
}

function round(value, digits = 2) {
  return Number(value.toFixed(digits));
}

function percentile(sorted, p) {
  if (sorted.length === 0) return null;
  const idx = Math.min(
    sorted.length - 1,
    Math.max(0, Math.ceil((p / 100) * sorted.length) - 1),
  );
  return sorted[idx];
}

function summarizeGroup(name, items) {
  const sorted = [...items].sort((a, b) => a.ns_per_iter - b.ns_per_iter);
  const totalNs = sorted.reduce((sum, b) => sum + b.ns_per_iter, 0);
  return {
    name,
    benchmark_count: sorted.length,
    fastest: sorted[0]?.name ?? null,
    fastest_ns_per_iter: sorted[0]?.ns_per_iter ?? null,
    median_ns_per_iter: percentile(
      sorted.map((b) => b.ns_per_iter),
      50,
    ),
    p95_ns_per_iter: percentile(
      sorted.map((b) => b.ns_per_iter),
      95,
    ),
    slowest: sorted[sorted.length - 1]?.name ?? null,
    slowest_ns_per_iter: sorted[sorted.length - 1]?.ns_per_iter ?? null,
    average_ns_per_iter:
      sorted.length > 0 ? Math.round(totalNs / sorted.length) : null,
  };
}

function attachBaseline(benchmarks, baseline) {
  if (!baseline) {
    return {
      default_threshold: null,
      compared_count: 0,
      ok_count: 0,
      regressed_count: 0,
      new_count: 0,
      missing_count: 0,
      regressions: [],
      new_benchmarks: [],
      missing_benchmarks: [],
    };
  }

  const defaultThreshold = Number(baseline._meta?.default_threshold ?? 3);
  const byName = new Map(benchmarks.map((bench) => [bench.name, bench]));
  const regressions = [];
  const newBenchmarks = [];
  let compared = 0;
  let ok = 0;

  for (const bench of benchmarks) {
    const baseEntry = baseline.benchmarks[bench.name];
    if (!baseEntry) {
      bench.baseline = { status: "new" };
      newBenchmarks.push(bench.name);
      continue;
    }

    const threshold = Number(baseEntry.threshold ?? defaultThreshold);
    const ratio = bench.ns_per_iter / Number(baseEntry.ns);
    const status = ratio > threshold ? "regressed" : "ok";
    bench.baseline = {
      status,
      ns_per_iter: Number(baseEntry.ns),
      ratio: round(ratio, 3),
      threshold,
    };
    compared++;
    if (status === "regressed") {
      regressions.push(bench.name);
    } else {
      ok++;
    }
  }

  const missing = Object.keys(baseline.benchmarks)
    .filter((name) => !byName.has(name))
    .sort();

  return {
    default_threshold: defaultThreshold,
    compared_count: compared,
    ok_count: ok,
    regressed_count: regressions.length,
    new_count: newBenchmarks.length,
    missing_count: missing.length,
    regressions,
    new_benchmarks: newBenchmarks.sort(),
    missing_benchmarks: missing,
  };
}

function groupBy(items, keyFn) {
  const groups = new Map();
  for (const item of items) {
    const key = keyFn(item);
    const existing = groups.get(key);
    if (existing) {
      existing.push(item);
    } else {
      groups.set(key, [item]);
    }
  }
  return groups;
}

function gitHubContext() {
  return {
    repository: process.env.GITHUB_REPOSITORY || null,
    run_id: process.env.GITHUB_RUN_ID || null,
    run_attempt: process.env.GITHUB_RUN_ATTEMPT || null,
    workflow: process.env.GITHUB_WORKFLOW || null,
    ref: process.env.GITHUB_REF || null,
    ref_name: process.env.GITHUB_REF_NAME || null,
    sha: process.env.GITHUB_SHA || null,
    event_name: process.env.GITHUB_EVENT_NAME || null,
    actor: process.env.GITHUB_ACTOR || null,
  };
}

function main() {
  const opts = parseArgs(process.argv.slice(2));
  const raw = fs.readFileSync(opts.input, "utf8");
  const benchmarks = parseBencher(raw);

  if (benchmarks.length === 0 && opts.failOnEmpty) {
    fail(
      "no bencher-format lines found. Did the bench command include '-- --output-format bencher'?",
    );
  }

  const baseline = opts.baseline ? readBaseline(opts.baseline) : null;
  const baselineSummary = attachBaseline(benchmarks, baseline);
  const groups = groupBy(benchmarks, (bench) => bench.group);
  const groupSummaries = [...groups.entries()]
    .map(([name, items]) => summarizeGroup(name, items))
    .sort((a, b) => a.name.localeCompare(b.name));
  const bySpeed = [...benchmarks].sort((a, b) => a.ns_per_iter - b.ns_per_iter);

  const summary = {
    schema_version: 1,
    generated_at: new Date().toISOString(),
    suite: opts.suite || process.env.GITHUB_WORKFLOW || "criterion",
    source: {
      input: path.relative(REPO_ROOT, path.resolve(opts.input)),
      baseline: opts.baseline
        ? path.relative(REPO_ROOT, path.resolve(opts.baseline))
        : null,
      github: gitHubContext(),
      runner: {
        os: os.platform(),
        arch: os.arch(),
        cpus: os.cpus().length,
      },
    },
    summary: {
      benchmark_count: benchmarks.length,
      group_count: groupSummaries.length,
      fastest: bySpeed[0]?.name ?? null,
      fastest_ns_per_iter: bySpeed[0]?.ns_per_iter ?? null,
      slowest: bySpeed[bySpeed.length - 1]?.name ?? null,
      slowest_ns_per_iter: bySpeed[bySpeed.length - 1]?.ns_per_iter ?? null,
      top_fastest: bySpeed.slice(0, 10).map((bench) => bench.name),
      top_slowest: bySpeed
        .slice(-10)
        .reverse()
        .map((bench) => bench.name),
    },
    baseline: baselineSummary,
    groups: groupSummaries,
    benchmarks,
  };

  fs.mkdirSync(path.dirname(opts.output), { recursive: true });
  fs.writeFileSync(opts.output, JSON.stringify(summary, null, 2) + "\n");
  console.log(
    `summarize-benchmarks: wrote ${benchmarks.length} benchmark(s) to ${path.relative(
      REPO_ROOT,
      path.resolve(opts.output),
    )}`,
  );
}

main();
