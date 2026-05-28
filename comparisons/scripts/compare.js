#!/usr/bin/env node
// Run the comparison bench and serialize the results into a table.
//
// Usage:
//   node scripts/compare.js                        # in-memory engines only
//   node scripts/compare.js --mode=persistent      # persistent variants only
//   node scripts/compare.js --mode=both            # in-memory + persistent
//   node scripts/compare.js --persistent           # alias for --mode=both
//   node scripts/compare.js --format=csv           # CSV instead of table
//   node scripts/compare.js --format=json          # raw JSON array
//   node scripts/compare.js --filter "scans/"      # criterion regex filter
//   node scripts/compare.js --no-run               # parse target/criterion only
//   LORA_VS_GRAFEO_SCALE=200 node scripts/compare.js
//
// Outputs the table on stdout. Cargo's progress (criterion's "Benchmarking
// ..." chatter) is forwarded to stderr so a redirect like `> table.md`
// captures only the table.

'use strict';

const { spawn } = require('child_process');
const fs = require('fs');
const path = require('path');
const readline = require('readline');

// ---------------------------------------------------------------------------
// args
// ---------------------------------------------------------------------------

const ROOT = path.resolve(__dirname, '..');
const FORMATS = new Set(['console', 'markdown', 'md', 'csv', 'tsv', 'json']);
const MODES = new Set(['mem', 'persistent', 'both']);

// Populated in main() before any formatter runs. Lookup key: `${group}/${id}`.
let enrichmentMap = new Map();

function defaultFormat() {
  // Pretty box-drawn output in a real terminal; markdown when redirected
  // to a file (so `npm run report > report.md` still produces a clean
  // markdown document).
  return process.stdout.isTTY ? 'console' : 'markdown';
}

function parseArgs(argv) {
  const out = {
    format: defaultFormat(),
    mode: 'mem',
    noRun: false,
    filter: null,
    passthrough: [],
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '-h' || a === '--help') return { help: true };
    if (a === '--no-run') { out.noRun = true; continue; }
    if (a === '--filter' || a === '-F') { out.filter = argv[++i]; continue; }
    if (a.startsWith('--filter=')) { out.filter = a.slice('--filter='.length); continue; }
    if (a === '--format' || a === '-f') { out.format = argv[++i]; continue; }
    if (a.startsWith('--format=')) { out.format = a.slice('--format='.length); continue; }
    if (a === '--mode' || a === '-m') { out.mode = argv[++i]; continue; }
    if (a.startsWith('--mode=')) { out.mode = a.slice('--mode='.length); continue; }
    // Convenience flags equivalent to --mode=...
    if (a === '--persistent') { out.mode = 'both'; continue; }
    if (a === '--persistent-only') { out.mode = 'persistent'; continue; }
    if (a === '--mem' || a === '--mem-only') { out.mode = 'mem'; continue; }
    if (a === '--') { out.passthrough.push(...argv.slice(i + 1)); break; }
    out.passthrough.push(a);
  }
  if (out.format === 'md') out.format = 'markdown';
  if (!FORMATS.has(out.format)) {
    fail(`unknown --format ${out.format}; expected one of ${[...FORMATS].join(', ')}`);
  }
  if (!MODES.has(out.mode)) {
    fail(`unknown --mode ${out.mode}; expected one of ${[...MODES].join(', ')}`);
  }
  return out;
}

function fail(msg) {
  process.stderr.write(`compare.js: ${msg}\n`);
  process.exit(2);
}

function help() {
  process.stdout.write(`Usage: node scripts/compare.js [options]

  -f, --format <console|md|csv|tsv|json>
                                   Output format. Default: console when
                                   printing to a terminal, markdown when
                                   stdout is redirected.
  -m, --mode <mem|persistent|both>
                                   Which storage variants to bench.
                                     mem        — in-memory engines only (default)
                                     persistent — on-disk engines only
                                     both       — in-memory + on-disk side-by-side
      --persistent                 Alias for --mode=both.
      --persistent-only            Alias for --mode=persistent.
      --mem-only                   Alias for --mode=mem (the default).
      --filter <regex>             Criterion --filter pattern (e.g. "scans/").
      --no-run                     Skip cargo bench; parse target/criterion only.
  -h, --help                       Show this help.

Anything after a literal "--" is forwarded to "cargo bench" verbatim.
`);
}

function envForMode(mode) {
  switch (mode) {
    case 'mem':        return {};
    case 'both':       return { LORA_VS_PERSISTENT: '1' };
    case 'persistent': return { LORA_VS_PERSISTENT: '1', LORA_VS_PERSISTENT_ONLY: '1' };
    default:           return {};
  }
}

// ---------------------------------------------------------------------------
// parsing criterion stdout
// ---------------------------------------------------------------------------

const UNIT_NS = { ns: 1, 'µs': 1e3, us: 1e3, ms: 1e6, s: 1e9 };

// Bench id line looks like: "scans/scan_label/lora/1000"
// or "setup/construct_empty/lora" (no size).
const BENCH_RE = /^([a-z][a-z_0-9]*)\/([a-z][a-z_0-9]*)\/([a-z][a-z_0-9]*)(?:\/(\d+))?$/;
// "                        time:   [848.69 ns 854.52 ns 859.25 ns]"
const TIME_RE = /time:\s+\[([\d.]+)\s+(ns|µs|us|ms|s)\s+([\d.]+)\s+(ns|µs|us|ms|s)\s+([\d.]+)\s+(ns|µs|us|ms|s)\]/;

function parseStream(rl) {
  return new Promise((resolve) => {
    const results = [];
    let pending = null;
    rl.on('line', (raw) => {
      // Forward criterion progress to stderr so the user can see it.
      process.stderr.write(raw + '\n');
      const line = raw.trim();
      const benchMatch = line.match(BENCH_RE);
      if (benchMatch) {
        pending = {
          group: benchMatch[1],
          id: benchMatch[2],
          engine: benchMatch[3],
          size: benchMatch[4] ? Number(benchMatch[4]) : null,
        };
        return;
      }
      if (pending) {
        const t = line.match(TIME_RE);
        if (t) {
          results.push({
            ...pending,
            low_ns: Number(t[1]) * UNIT_NS[t[2]],
            median_ns: Number(t[3]) * UNIT_NS[t[4]],
            high_ns: Number(t[5]) * UNIT_NS[t[6]],
          });
          pending = null;
        }
      }
    });
    rl.on('close', () => resolve(results));
  });
}

// ---------------------------------------------------------------------------
// running cargo bench
// ---------------------------------------------------------------------------

function runBench({ filter, passthrough, mode }) {
  return new Promise((resolve, reject) => {
    const args = [
      'bench',
      '--manifest-path', path.join(ROOT, 'Cargo.toml'),
      '--bench', 'comparison',
      '--',
    ];
    if (filter) args.push(filter);
    args.push(...passthrough);

    const modeEnv = envForMode(mode);
    const envBadge = Object.entries(modeEnv)
      .map(([k, v]) => `${k}=${v} `)
      .join('');
    process.stderr.write(`> ${envBadge}cargo ${args.join(' ')}\n`);
    const child = spawn('cargo', args, {
      cwd: ROOT,
      stdio: ['ignore', 'pipe', 'inherit'],
      env: { ...process.env, ...modeEnv },
    });
    const rl = readline.createInterface({ input: child.stdout });
    const resultsP = parseStream(rl);
    child.on('error', reject);
    child.on('exit', async (code) => {
      const results = await resultsP;
      if (code === 0) resolve(results);
      else reject(new Error(`cargo bench exited with code ${code}`));
    });
  });
}

// ---------------------------------------------------------------------------
// reading target/criterion (for --no-run)
// ---------------------------------------------------------------------------

// Criterion writes one directory per benchmark group, with the group
// name flattened from "{group}/{id}" to "{group}_{id}". To translate
// back we lift workload metadata (group, category, per-engine notes)
// out of workloads.yml using a tiny regex-only YAML reader — that
// avoids pulling in a YAML dependency.
function readWorkloadMeta() {
  const file = path.join(ROOT, 'benches', 'workloads.yml');
  if (!fs.existsSync(file)) return { workloads: [], groupCategories: {} };
  const yml = fs.readFileSync(file, 'utf8');

  const workloads = [];
  const groupCategories = {};
  let cur = null;
  let inNotes = false;
  let inDefaultsCategories = false;

  const flush = () => { if (cur) workloads.push(cur); cur = null; inNotes = false; };

  for (const raw of yml.split('\n')) {
    // Top-level `defaults: / categories:` block: 4-space group → category map.
    if (/^ {2}categories:\s*$/.test(raw)) { inDefaultsCategories = true; continue; }
    if (inDefaultsCategories) {
      const mc = raw.match(/^ {4}(\w+):\s*(\S+)/);
      if (mc) { groupCategories[mc[1]] = mc[2]; continue; }
      // Leaving categories block at any non-matching shallower line.
      if (/^\S/.test(raw) || /^ {2}\S/.test(raw)) inDefaultsCategories = false;
    }

    const idM = raw.match(/^ {2}- id:\s*(\S+)/);
    if (idM) {
      flush();
      cur = { id: idM[1], group: null, category: null, notes: {}, dir: null };
      continue;
    }
    if (!cur) continue;

    const gM = raw.match(/^ {4}group:\s*(\S+)/);
    if (gM) { cur.group = gM[1]; cur.dir = `${gM[1]}_${cur.id}`; inNotes = false; continue; }
    const cM = raw.match(/^ {4}category:\s*(\S+)/);
    if (cM) { cur.category = cM[1]; inNotes = false; continue; }
    if (/^ {4}notes:\s*$/.test(raw)) { inNotes = true; continue; }
    // Any other top-level workload key closes the notes sub-block.
    if (/^ {4}\w/.test(raw)) { inNotes = false; }
    if (inNotes) {
      const nm = raw.match(/^ {6}(\w+):\s*"(.*)"\s*$/) || raw.match(/^ {6}(\w+):\s*(.+?)\s*$/);
      if (nm) cur.notes[nm[1]] = nm[2];
    }
  }
  flush();

  for (const w of workloads) {
    if (!w.category) w.category = groupCategories[w.group] || w.group;
  }
  return { workloads, groupCategories };
}

function readWorkloadIndex() {
  return readWorkloadMeta().workloads.map((w) => ({
    group: w.group, id: w.id, dir: w.dir,
  }));
}

// Build a (group, id) → { category, notes } lookup for enriching results.
function workloadEnrichment() {
  const meta = readWorkloadMeta();
  const map = new Map();
  for (const w of meta.workloads) {
    map.set(`${w.group}/${w.id}`, { category: w.category, notes: w.notes });
  }
  return map;
}

function enrichResults(results) {
  const map = workloadEnrichment();
  for (const r of results) {
    const entry = map.get(`${r.group}/${r.id}`);
    if (entry) {
      r.category = entry.category;
      // Per-engine note for this specific row (if any).
      if (entry.notes && entry.notes[r.engine]) r.note = entry.notes[r.engine];
    }
  }
  return results;
}

function readEstimates(file) {
  let j;
  try { j = JSON.parse(fs.readFileSync(file, 'utf8')); }
  catch { return null; }
  return {
    low_ns: j.mean?.confidence_interval?.lower_bound ?? null,
    median_ns: j.median?.point_estimate ?? null,
    high_ns: j.mean?.confidence_interval?.upper_bound ?? null,
  };
}

function readExistingResults() {
  const root = path.join(ROOT, 'target', 'criterion');
  if (!fs.existsSync(root)) return [];
  const workloads = readWorkloadIndex();
  const out = [];

  for (const w of workloads) {
    const wdir = path.join(root, w.dir);
    let engineEntries;
    try { engineEntries = fs.readdirSync(wdir, { withFileTypes: true }); }
    catch { continue; }
    for (const ed of engineEntries) {
      if (!ed.isDirectory()) continue;
      const engine = ed.name;
      const ePath = path.join(wdir, engine);

      // Without a size: <wdir>/<engine>/new/estimates.json
      const directFile = path.join(ePath, 'new', 'estimates.json');
      if (fs.existsSync(directFile)) {
        const e = readEstimates(directFile);
        if (e) out.push({ group: w.group, id: w.id, engine, size: null, ...e });
        continue;
      }
      // With a size: <wdir>/<engine>/<size>/new/estimates.json
      let sizeEntries;
      try { sizeEntries = fs.readdirSync(ePath, { withFileTypes: true }); }
      catch { continue; }
      for (const sd of sizeEntries) {
        if (!sd.isDirectory() || !/^\d+$/.test(sd.name)) continue;
        const file = path.join(ePath, sd.name, 'new', 'estimates.json');
        if (!fs.existsSync(file)) continue;
        const e = readEstimates(file);
        if (e) out.push({ group: w.group, id: w.id, engine, size: Number(sd.name), ...e });
      }
    }
  }
  return out;
}

// ---------------------------------------------------------------------------
// formatting
// ---------------------------------------------------------------------------

function formatNs(ns) {
  if (ns == null || !Number.isFinite(ns)) return '—';
  if (ns < 1e3) return `${ns.toFixed(2)} ns`;
  if (ns < 1e6) return `${(ns / 1e3).toFixed(2)} µs`;
  if (ns < 1e9) return `${(ns / 1e6).toFixed(2)} ms`;
  return `${(ns / 1e9).toFixed(2)} s`;
}

function pivot(results) {
  // Map: group -> id -> { size, engines: { engine: result } }
  const byGroup = new Map();
  for (const r of results) {
    let g = byGroup.get(r.group);
    if (!g) { g = new Map(); byGroup.set(r.group, g); }
    let w = g.get(r.id);
    if (!w) { w = { size: r.size, engines: new Map() }; g.set(r.id, w); }
    if (w.size == null) w.size = r.size;
    w.engines.set(r.engine, r);
  }
  return byGroup;
}

function discoverEngineColumns(results) {
  // Preserve a canonical order if present, then append unknowns alphabetically.
  // In-memory variants come first so the mem column appears next to its
  // persistent counterpart in --mode=both runs.
  const preferred = [
    'lora', 'lora_wal',
    'kuzu', 'kuzu_file',
    'grafeo', 'grafeo_file',
    'surrealdb', 'surrealdb_kv',
    'memgraph',
    'neo4j',
    'helixdb',
  ];
  const seen = new Set(results.map((r) => r.engine));
  const cols = preferred.filter((e) => seen.has(e));
  for (const e of [...seen].sort()) if (!cols.includes(e)) cols.push(e);
  return cols;
}

// ---------------------------------------------------------------------------
// markdown table rendering with padded source alignment
// ---------------------------------------------------------------------------

const ALIGN_LEFT = 'left';
const ALIGN_RIGHT = 'right';

function padCell(text, width, align) {
  const s = text == null ? '' : String(text);
  // Cells whose visual width differs from JS string length (the ⭐ emoji
  // counts as one character but renders as roughly two cells) get an
  // extra column of slack so right-aligned columns still line up in
  // rendered output.
  const visual = s.length + (s.includes('⭐') ? 1 : 0);
  const slack = width - visual;
  if (slack <= 0) return s;
  return align === ALIGN_RIGHT ? ' '.repeat(slack) + s : s + ' '.repeat(slack);
}

function alignmentSeparator(width, align) {
  // Markdown alignment row: at least 3 dashes, with `:` markers for align.
  const inner = Math.max(3, width - (align === ALIGN_RIGHT ? 1 : 0));
  return align === ALIGN_RIGHT ? '-'.repeat(inner) + ':' : '-'.repeat(Math.max(3, width));
}

function renderTable(headers, aligns, rows) {
  const widths = headers.map((h, i) => {
    let max = h.length;
    for (const r of rows) {
      const v = r[i] == null ? '' : String(r[i]);
      const visual = v.length + (v.includes('⭐') ? 1 : 0);
      if (visual > max) max = visual;
    }
    return Math.max(max, 3);
  });
  const lines = [];
  lines.push('| ' + headers.map((h, i) => padCell(h, widths[i], aligns[i])).join(' | ') + ' |');
  lines.push('| ' + widths.map((w, i) => alignmentSeparator(w, aligns[i])).join(' | ') + ' |');
  for (const r of rows) {
    lines.push('| ' + headers.map((_, i) => padCell(r[i], widths[i], aligns[i])).join(' | ') + ' |');
  }
  return lines.join('\n');
}

// ---------------------------------------------------------------------------
// shared report scaffolding
// ---------------------------------------------------------------------------

function geomean(xs) {
  if (!xs.length) return null;
  const logSum = xs.reduce((s, x) => s + Math.log(x), 0);
  return Math.exp(logSum / xs.length);
}

// For each group: pick a single winner (engine with most workload wins,
// tie-broken by lowest geomean time), then for every other engine compute
// the geomean of `engine_median / winner_median` across the workloads
// where both have data. The result tells you, on average, how much slower
// each engine is than the group's winner.
function computeGroupStats(byGroup, allEngines) {
  const stats = new Map();
  for (const [groupName, workloads] of byGroup) {
    const wins = Object.fromEntries(allEngines.map((e) => [e, 0]));
    for (const w of workloads.values()) {
      const sorted = [...w.engines.values()].sort((a, b) => a.median_ns - b.median_ns);
      const fastest = sorted[0];
      if (!fastest) continue;
      for (const [eng, r] of w.engines) if (r.median_ns === fastest.median_ns) wins[eng]++;
    }

    // Tiebreak rank: per-engine geomean of *its own* times across the
    // group; whichever is smaller is faster overall.
    const ownTimes = Object.fromEntries(allEngines.map((e) => [e, []]));
    for (const w of workloads.values()) {
      for (const [eng, r] of w.engines) {
        if (r.median_ns > 0) ownTimes[eng].push(r.median_ns);
      }
    }
    const ownGeomean = Object.fromEntries(
      allEngines.map((e) => [e, geomean(ownTimes[e])]),
    );

    // Winner = most wins, fall back on geomean of own times.
    let winner = null;
    for (const e of allEngines) {
      if (wins[e] === 0 && ownTimes[e].length === 0) continue;
      if (
        winner == null ||
        wins[e] > wins[winner] ||
        (wins[e] === wins[winner] &&
          ownGeomean[e] != null &&
          (ownGeomean[winner] == null || ownGeomean[e] < ownGeomean[winner]))
      ) {
        winner = e;
      }
    }

    // Per-engine slowdown vs the group winner.
    const slowdownVsWinner = {};
    if (winner) {
      for (const e of allEngines) {
        const ratios = [];
        for (const w of workloads.values()) {
          const wr = w.engines.get(winner);
          const er = w.engines.get(e);
          if (wr && er && wr.median_ns > 0) {
            ratios.push(er.median_ns / wr.median_ns);
          }
        }
        slowdownVsWinner[e] = geomean(ratios);
      }
    }

    stats.set(groupName, {
      total: workloads.size,
      wins,
      winner,
      slowdownVsWinner,
    });
  }
  return stats;
}

function fmtSlowdown(x, isWinner) {
  if (x == null) return '—';
  if (isWinner) return 'fastest';
  // Render <1× (rare — happens when the picked winner doesn't dominate
  // every workload) honestly so the table doesn't lie about direction.
  return `${x.toFixed(2)}× slower`;
}

function workloadRow(workload, engineCols, { starWinner }) {
  const sorted = [...workload.engines.values()].sort((a, b) => a.median_ns - b.median_ns);
  const fastest = sorted[0];
  const cells = [];
  for (const e of engineCols) {
    const r = workload.engines.get(e);
    if (!r) {
      cells.push('—');
      continue;
    }
    if (r === fastest && sorted.length > 1) {
      // The fastest engine on this row.
      cells.push(starWinner ? `${formatNs(r.median_ns)} ⭐` : formatNs(r.median_ns));
    } else if (fastest && r.median_ns > fastest.median_ns && fastest.median_ns > 0) {
      // Slower engine — append its slowdown ratio versus the fastest.
      const ratio = r.median_ns / fastest.median_ns;
      cells.push(`${formatNs(r.median_ns)} (${ratio.toFixed(2)}×)`);
    } else {
      cells.push(formatNs(r.median_ns));
    }
  }
  return { cells, winner: fastest ? fastest.engine : '—' };
}

function formatMarkdown(results) {
  if (!results.length) return '_no benchmark results_\n';
  const byGroup = pivot(results);
  const allEngines = discoverEngineColumns(results);
  const groupStats = computeGroupStats(byGroup, allEngines);
  const grandTotal = [...byGroup.values()].reduce((s, m) => s + m.size, 0);
  const grandWins = Object.fromEntries(allEngines.map((e) => [e, 0]));
  for (const stat of groupStats.values()) for (const e of allEngines) grandWins[e] += stat.wins[e];

  const out = [];
  out.push('# Graph DB Comparison Report');
  out.push('');
  out.push(
    `Engines: ${allEngines.map((e) => '`' + e + '`').join(', ')}` +
      ` · ${grandTotal} workloads across ${byGroup.size} groups.`,
  );
  out.push('');

  // ---- summary table ----
  out.push('## Summary');
  out.push('');
  out.push(
    '_Each engine column shows the geometric-mean slowdown of that engine ' +
      'vs the group winner across every workload they share._',
  );
  out.push('');
  const summaryHeaders = ['Group', 'Workloads', 'Winner', ...allEngines];
  const summaryAligns = [
    ALIGN_LEFT,
    ALIGN_RIGHT,
    ALIGN_LEFT,
    ...allEngines.map(() => ALIGN_RIGHT),
  ];
  const summaryRows = [];
  for (const [groupName, stat] of groupStats) {
    summaryRows.push([
      groupName,
      String(stat.total),
      stat.winner ?? '—',
      ...allEngines.map((e) =>
        fmtSlowdown(stat.slowdownVsWinner[e], e === stat.winner),
      ),
    ]);
  }
  // Grand-total row: pick a single overall winner and recompute slowdowns
  // across every workload in every group.
  const overallWins = Object.fromEntries(allEngines.map((e) => [e, grandWins[e]]));
  let overallWinner = null;
  for (const e of allEngines) {
    if (grandWins[e] === 0) continue;
    if (overallWinner == null || overallWins[e] > overallWins[overallWinner]) {
      overallWinner = e;
    }
  }
  const overallSlowdown = Object.fromEntries(allEngines.map((e) => [e, null]));
  if (overallWinner) {
    for (const e of allEngines) {
      const ratios = [];
      for (const workloads of byGroup.values()) {
        for (const w of workloads.values()) {
          const wr = w.engines.get(overallWinner);
          const er = w.engines.get(e);
          if (wr && er && wr.median_ns > 0) ratios.push(er.median_ns / wr.median_ns);
        }
      }
      overallSlowdown[e] = geomean(ratios);
    }
  }
  summaryRows.push([
    '**total**',
    `**${grandTotal}**`,
    overallWinner ? `**${overallWinner}**` : '—',
    ...allEngines.map((e) =>
      `**${fmtSlowdown(overallSlowdown[e], e === overallWinner)}**`,
    ),
  ]);
  out.push(renderTable(summaryHeaders, summaryAligns, summaryRows));
  out.push('');

  // ---- per-group detail tables ----
  for (const [groupName, workloads] of byGroup) {
    const stat = groupStats.get(groupName);
    out.push(`## ${groupName} _(${stat.total})_`);
    out.push('');

    const engineCols = allEngines.filter((e) =>
      [...workloads.values()].some((w) => w.engines.has(e)),
    );
    const headers = ['Workload', 'Size', ...engineCols, 'Winner'];
    const aligns = [
      ALIGN_LEFT,
      ALIGN_RIGHT,
      ...engineCols.map(() => ALIGN_RIGHT),
      ALIGN_LEFT,
    ];

    const rows = [];
    const ids = [...workloads.keys()].sort();
    for (const id of ids) {
      const w = workloads.get(id);
      const { cells, winner } = workloadRow(w, engineCols, { starWinner: true });
      rows.push([id, w.size == null ? '–' : String(w.size), ...cells, winner]);
    }
    out.push(renderTable(headers, aligns, rows));
    out.push('');

    const noteLines = collectNoteLines(groupName, workloads);
    if (noteLines.length) {
      out.push('**Notes**');
      out.push('');
      for (const line of noteLines) out.push(`- ${line}`);
      out.push('');
    }
  }
  return out.join('\n');
}

// Collect "engine X on workload Y was skipped / has caveat Z" lines for a
// group's notes block. Pulls from both ran-rows (row.note) and the global
// enrichment map (workloads with notes for engines that have no row).
function collectNoteLines(groupName, workloads) {
  const lines = [];
  for (const [id, w] of workloads) {
    const entry = enrichmentMap.get(`${groupName}/${id}`);
    if (!entry || !entry.notes) continue;
    for (const [engine, text] of Object.entries(entry.notes)) {
      lines.push(`\`${id}\` · **${engine}** — ${text}`);
    }
  }
  // Also surface workloads that had notes but no row at all (e.g. the
  // entire workload was skipped because no engine ran).
  for (const [key, entry] of enrichmentMap) {
    if (!key.startsWith(`${groupName}/`)) continue;
    const id = key.slice(groupName.length + 1);
    if (workloads.has(id)) continue;
    for (const [engine, text] of Object.entries(entry.notes || {})) {
      lines.push(`\`${id}\` · **${engine}** — ${text}`);
    }
  }
  return lines;
}

// ---------------------------------------------------------------------------
// console (Unicode box-drawn) renderer
// ---------------------------------------------------------------------------

const BOX = {
  topLeft: '┌', topRight: '┐', botLeft: '└', botRight: '┘',
  midLeft: '├', midRight: '┤', topT: '┬', botT: '┴', cross: '┼',
  h: '─', v: '│',
};

function visualWidth(s) {
  // JS .length counts UTF-16 code units; emoji fall outside that. We treat
  // ⭐ as visually 2 cells in most terminals and 1 in JS; pad accordingly.
  if (s == null) return 0;
  const str = String(s);
  return str.length + (str.includes('⭐') ? 1 : 0);
}

function padBox(text, width, align) {
  const s = text == null ? '' : String(text);
  const slack = width - visualWidth(s);
  if (slack <= 0) return s;
  return align === ALIGN_RIGHT ? ' '.repeat(slack) + s : s + ' '.repeat(slack);
}

// rowGroups: an array of arrays-of-rows. Rows within a group render
// without a divider; an internal divider is drawn between groups.
function renderBoxTable(headers, aligns, rowGroups) {
  // Allow callers to pass a flat array of rows for simple tables.
  const groups = Array.isArray(rowGroups[0]?.[0])
    ? rowGroups
    : [rowGroups];

  const allRows = groups.flat();
  const widths = headers.map((h, i) => {
    let max = visualWidth(h);
    for (const r of allRows) max = Math.max(max, visualWidth(r[i]));
    return Math.max(max, 1);
  });
  const top = BOX.topLeft + widths.map((w) => BOX.h.repeat(w + 2)).join(BOX.topT) + BOX.topRight;
  const sep = BOX.midLeft + widths.map((w) => BOX.h.repeat(w + 2)).join(BOX.cross) + BOX.midRight;
  const bot = BOX.botLeft + widths.map((w) => BOX.h.repeat(w + 2)).join(BOX.botT) + BOX.botRight;
  const renderRow = (cells) =>
    BOX.v +
    cells
      .map((c, i) => ' ' + padBox(c, widths[i], aligns[i]) + ' ')
      .join(BOX.v) +
    BOX.v;

  const lines = [top, renderRow(headers), sep];
  groups.forEach((group, idx) => {
    if (idx > 0) lines.push(sep);
    for (const r of group) lines.push(renderRow(r));
  });
  lines.push(bot);
  return lines.join('\n');
}

function underline(text, ch = '─') {
  return text + '\n' + ch.repeat(visualWidth(text));
}

// Build the two rows used by the console table for one workload:
// row 1 carries the timings, row 2 carries the slowdown vs the fastest.
function consoleRowGroup(id, workload, engineCols) {
  const sorted = [...workload.engines.values()].sort((a, b) => a.median_ns - b.median_ns);
  const fastest = sorted[0];
  const sizeStr = workload.size == null ? '–' : String(workload.size);

  const timeRow = [id, sizeStr];
  const ratioRow = ['', ''];
  for (const e of engineCols) {
    const r = workload.engines.get(e);
    if (!r) {
      timeRow.push('—');
      ratioRow.push('');
      continue;
    }
    timeRow.push(formatNs(r.median_ns));
    if (!fastest || fastest.median_ns <= 0) {
      ratioRow.push('');
    } else if (r === fastest) {
      // The baseline. Mark the winner once so a quick scan finds it; the
      // Winner column at the end of the timings row spells out the name.
      ratioRow.push(sorted.length > 1 ? 'fastest' : 'solo');
    } else {
      ratioRow.push(`${(r.median_ns / fastest.median_ns).toFixed(2)}× slower`);
    }
  }
  timeRow.push(fastest ? fastest.engine : '—');
  ratioRow.push('');
  return [timeRow, ratioRow];
}

function formatConsole(results) {
  if (!results.length) return 'no benchmark results\n';
  const byGroup = pivot(results);
  const allEngines = discoverEngineColumns(results);
  const groupStats = computeGroupStats(byGroup, allEngines);
  const grandTotal = [...byGroup.values()].reduce((s, m) => s + m.size, 0);
  const grandWins = Object.fromEntries(allEngines.map((e) => [e, 0]));
  for (const stat of groupStats.values()) for (const e of allEngines) grandWins[e] += stat.wins[e];

  const out = [];
  out.push(underline('Graph DB Comparison Report', '═'));
  out.push(`Engines: ${allEngines.join(', ')}  ·  ${grandTotal} workloads / ${byGroup.size} groups`);
  out.push('');

  // ---- summary ----
  // Each engine column shows the geomean slowdown vs the group winner.
  out.push(underline('Summary'));
  const summaryHeaders = ['Group', 'Workloads', 'Winner', ...allEngines];
  const summaryAligns = [
    ALIGN_LEFT,
    ALIGN_RIGHT,
    ALIGN_LEFT,
    ...allEngines.map(() => ALIGN_RIGHT),
  ];
  const summaryRows = [];
  for (const [groupName, stat] of groupStats) {
    summaryRows.push([
      groupName,
      String(stat.total),
      stat.winner ?? '—',
      ...allEngines.map((e) =>
        fmtSlowdown(stat.slowdownVsWinner[e], e === stat.winner),
      ),
    ]);
  }
  // Grand total — overall winner + slowdowns pooled across all groups.
  let overallWinner = null;
  for (const e of allEngines) {
    if (grandWins[e] === 0) continue;
    if (overallWinner == null || grandWins[e] > grandWins[overallWinner]) overallWinner = e;
  }
  const overallSlowdown = Object.fromEntries(allEngines.map((e) => [e, null]));
  if (overallWinner) {
    for (const e of allEngines) {
      const ratios = [];
      for (const workloads of byGroup.values()) {
        for (const w of workloads.values()) {
          const wr = w.engines.get(overallWinner);
          const er = w.engines.get(e);
          if (wr && er && wr.median_ns > 0) ratios.push(er.median_ns / wr.median_ns);
        }
      }
      overallSlowdown[e] = geomean(ratios);
    }
  }
  summaryRows.push([
    'total',
    String(grandTotal),
    overallWinner ?? '—',
    ...allEngines.map((e) => fmtSlowdown(overallSlowdown[e], e === overallWinner)),
  ]);
  out.push(renderBoxTable(summaryHeaders, summaryAligns, summaryRows));
  out.push('');

  // ---- per-group, two rows per workload ----
  for (const [groupName, workloads] of byGroup) {
    const stat = groupStats.get(groupName);
    out.push(underline(`${groupName} (${stat.total})`));
    const engineCols = allEngines.filter((e) =>
      [...workloads.values()].some((w) => w.engines.has(e)),
    );
    const headers = ['Workload', 'Size', ...engineCols, 'Winner'];
    const aligns = [
      ALIGN_LEFT,
      ALIGN_RIGHT,
      ...engineCols.map(() => ALIGN_RIGHT),
      ALIGN_LEFT,
    ];
    const rowGroups = [];
    for (const id of [...workloads.keys()].sort()) {
      rowGroups.push(consoleRowGroup(id, workloads.get(id), engineCols));
    }
    out.push(renderBoxTable(headers, aligns, rowGroups));

    const noteLines = collectNoteLines(groupName, workloads);
    if (noteLines.length) {
      out.push('Notes:');
      for (const line of noteLines) {
        // Strip markdown emphasis for terminal output.
        out.push(`  - ${line.replace(/[`*]/g, '')}`);
      }
    }
    out.push('');
  }
  return out.join('\n');
}

function formatDelimited(results, sep) {
  const cols = ['group', 'id', 'engine', 'size', 'low_ns', 'median_ns', 'high_ns'];
  const lines = [cols.join(sep)];
  const sorted = [...results].sort((a, b) =>
    a.group.localeCompare(b.group) ||
    a.id.localeCompare(b.id) ||
    a.engine.localeCompare(b.engine)
  );
  for (const r of sorted) {
    const row = cols.map((c) => {
      const v = r[c];
      if (v == null) return '';
      const s = String(v);
      // CSV quoting: double the quotes, wrap if value contains comma/quote/newline.
      if (sep === ',' && /[",\n]/.test(s)) return `"${s.replace(/"/g, '""')}"`;
      return s;
    });
    lines.push(row.join(sep));
  }
  return lines.join('\n');
}

function formatJson(results) {
  return JSON.stringify(results, null, 2);
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

(async () => {
  const opts = parseArgs(process.argv.slice(2));
  if (opts.help) { help(); return; }

  const results = opts.noRun ? readExistingResults() : await runBench(opts);
  if (!results.length && !opts.noRun) {
    fail('no benchmark output parsed — check that cargo bench produced timing lines.');
  }
  enrichResults(results);
  // Cache enrichment map for formatters that want to surface notes for
  // engines that were intentionally skipped (and so have no row).
  enrichmentMap = workloadEnrichment();

  let serialized;
  switch (opts.format) {
    case 'console':  serialized = formatConsole(results); break;
    case 'markdown': serialized = formatMarkdown(results); break;
    case 'csv':      serialized = formatDelimited(results, ','); break;
    case 'tsv':      serialized = formatDelimited(results, '\t'); break;
    case 'json':     serialized = formatJson(results); break;
    default:         fail(`unhandled format ${opts.format}`);
  }

  process.stdout.write(serialized);
  if (!serialized.endsWith('\n')) process.stdout.write('\n');
})().catch((e) => {
  fail(e.stack || String(e));
});
