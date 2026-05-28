#!/usr/bin/env node
// Generates the v0.15 release-post header banner.
//
//   yarn workspace loradb-docs node scripts/build-blog-banner-v0-15.mjs
//
// Output:
//   static/img/blog/loradb-v0-15-benchmarks-in-public-header.png      (1280x400)
//   static/img/blog/loradb-v0-15-benchmarks-in-public-header@2x.png   (2560x800)
//
// Visual: same layout family as v0.10 / v0.11 / v0.12 / v0.13 / v0.14
// (eyebrow + headline + tagline on the left, panel on the right). The
// right panel is a horizontal bar chart of the seven engines compared
// in this release's benchmark suite, scaled log10 so the 1x-to-57x
// span stays readable. LoraDB sits at the top with the bright filled
// bar; the other six engines fade with the magnitude of their
// geomean slowdown.
//
// Deterministic: same SVG -> same PNG bytes (sharp metadata stripped).

import { writeFile, mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import sharp from "sharp";

const __dirname = dirname(fileURLToPath(import.meta.url));
const OUT_DIR = resolve(__dirname, "..", "static", "img", "blog");
const BASE_NAME = "loradb-v0-15-benchmarks-in-public-header";
const W = 1280;
const H = 400;

// Brand tokens. Same values used in src/styles for the dark theme.
const BG_A = "#0b1020";
const BG_B = "#161c34";
const PANEL = "#0f1530";
const PANEL_LINE = "#1e2748";
const ACCENT_A = "#5b8def"; // brand-accent-a (blue)
const ACCENT_B = "#9b6bff"; // brand-accent-b (violet)
const INK = "#e7ecff";
const INK_DIM = "#9aa3c2";
const TRACK = "#2a3361"; // muted track for non-winner bars
const FASTEST = "#8fd4a1"; // mint for the LoraDB "fastest" bar

// Right panel inner rect (matches the prior banners so the layout
// family stays consistent across releases).
const PANEL_X = 640;
const PANEL_Y = 40;
const PANEL_W = 600;
const PANEL_H = 320;

// Bar chart geometry — seven rows, one per engine. The label column
// stays a fixed width so the bars all start at the same x.
// LoraDB row sits below the release stamp (which occupies y 64-122 of
// the panel), so the chart starts at PANEL_Y + 96 to clear it without
// the "fastest" value running into the stamp's right edge. Seven rows
// of 32px fill the remaining 200px of panel height.
const CHART_X = PANEL_X + 28;
const CHART_Y = PANEL_Y + 96;
const CHART_W = 540;
const ROW_H = 30;
const BAR_H = 16;
const LABEL_W = 90;
const VALUE_W = 56;
const TRACK_X = CHART_X + LABEL_W;
const TRACK_W = CHART_W - LABEL_W - VALUE_W;

// Engine rows. `slowdown` is the geomean number from
// comparisons/report.md's total row. The bar widths are log10-scaled
// against the max so a 57x bar visually reads as "much slower" without
// crushing the 2-10x bars to pixel dust.
const ENGINES = [
  { id: "lora", label: "LoraDB", slowdown: 1, isWinner: true },
  { id: "grafeo", label: "Grafeo", slowdown: 2.3 },
  { id: "kuzu", label: "Kuzu", slowdown: 3.3 },
  { id: "neo4j", label: "Neo4j", slowdown: 7.99 },
  { id: "memgraph", label: "Memgraph", slowdown: 9.75 },
  { id: "surrealdb", label: "SurrealDB", slowdown: 48.58 },
  { id: "helixdb", label: "HelixDB", slowdown: 57.2 },
];

function escape(s) {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function formatRatio(n) {
  if (n <= 1.0) return "1×";
  if (n >= 100) return `${n.toFixed(0)}×`;
  if (n >= 10) return `${n.toFixed(1)}×`;
  return `${n.toFixed(2)}×`;
}

function buildSvg() {
  // Subtle grid behind the panel.
  const grid = [];
  for (let x = PANEL_X + 30; x < PANEL_X + PANEL_W; x += 30) {
    grid.push(
      `<line x1="${x}" y1="${PANEL_Y + 14}" x2="${x}" y2="${PANEL_Y + PANEL_H - 14}" stroke="${PANEL_LINE}" stroke-opacity="0.4" stroke-width="1"/>`,
    );
  }
  for (let y = PANEL_Y + 30; y < PANEL_Y + PANEL_H; y += 30) {
    grid.push(
      `<line x1="${PANEL_X + 14}" y1="${y}" x2="${PANEL_X + PANEL_W - 14}" y2="${y}" stroke="${PANEL_LINE}" stroke-opacity="0.4" stroke-width="1"/>`,
    );
  }

  // Chart captions. The hint sits directly under the heading so it
  // doesn't collide with the release stamp in the top-right corner.
  const captionY = PANEL_Y + 38;
  const chartCaption = `<text x="${CHART_X}" y="${captionY}" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="11" font-weight="700" fill="${INK}">geomean slowdown</text>`;
  const chartHint = `<text x="${CHART_X}" y="${captionY + 14}" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="10" fill="${INK_DIM}">82 workloads · log scale</text>`;

  // Bar rows. Log10-mapped — 1x sits at 6% (a thin "winner" hint),
  // 100x sits at 100%.
  const maxRatio = Math.max(...ENGINES.map((e) => e.slowdown));
  const logMax = Math.log10(maxRatio);
  const bars = ENGINES.map((engine, idx) => {
    const rowY = CHART_Y + idx * ROW_H;
    const baselineY = rowY + BAR_H / 2;
    const safe = Math.max(1, engine.slowdown);
    const pct = engine.isWinner
      ? 0.06
      : Math.max(0.06, Math.log10(safe) / logMax);
    const barW = Math.round(TRACK_W * pct);
    const labelColor = engine.isWinner ? FASTEST : INK_DIM;
    const valueColor = engine.isWinner ? FASTEST : INK_DIM;
    const barFill = engine.isWinner ? FASTEST : "url(#barGrad)";

    return [
      `<text x="${CHART_X + LABEL_W - 12}" y="${baselineY + 4}" text-anchor="end" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="11" font-weight="${engine.isWinner ? 700 : 500}" fill="${labelColor}">${escape(engine.label)}</text>`,
      `<rect x="${TRACK_X}" y="${rowY}" width="${TRACK_W}" height="${BAR_H}" rx="3" fill="${TRACK}" opacity="0.55"/>`,
      `<rect x="${TRACK_X}" y="${rowY}" width="${barW}" height="${BAR_H}" rx="3" fill="${barFill}"/>`,
      `<text x="${TRACK_X + TRACK_W + 8}" y="${baselineY + 4}" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="11" font-weight="${engine.isWinner ? 700 : 500}" fill="${valueColor}">${escape(engine.isWinner ? "fastest" : formatRatio(engine.slowdown))}</text>`,
    ].join("\n");
  }).join("\n");

  // Footer hint along the bottom of the panel — a single muted line
  // pointing the reader at the new page.
  const footerY = PANEL_Y + PANEL_H - 14;
  const footer = `<text x="${PANEL_X + 28}" y="${footerY}" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="10" fill="${INK_DIM}">loradb.com/benchmarks · 8 of 12 groups won by LoraDB</text>`;

  // Top-right corner stamp — release tag.
  const stamp =
    `<g transform="translate(${PANEL_X + PANEL_W - 184}, ${PANEL_Y + 24})">` +
    `<rect x="0" y="0" width="170" height="58" rx="8" fill="${BG_A}" stroke="${PANEL_LINE}" stroke-width="1" opacity="0.92"/>` +
    `<text x="12" y="20" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="10" fill="${INK_DIM}">SUITE · v0.15</text>` +
    `<text x="12" y="36" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="10" fill="${INK}">/benchmarks live</text>` +
    `<text x="12" y="50" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="10" fill="${FASTEST}">comparisons/ in tree</text>` +
    `</g>`;

  return `<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${H}" viewBox="0 0 ${W} ${H}">
  <defs>
    <linearGradient id="bgGrad" x1="0%" y1="0%" x2="100%" y2="100%">
      <stop offset="0%" stop-color="${BG_A}"/>
      <stop offset="100%" stop-color="${BG_B}"/>
    </linearGradient>
    <linearGradient id="coreGrad" x1="0%" y1="0%" x2="100%" y2="100%">
      <stop offset="0%" stop-color="${ACCENT_A}"/>
      <stop offset="100%" stop-color="${ACCENT_B}"/>
    </linearGradient>
    <linearGradient id="headlineGrad" x1="0%" y1="0%" x2="100%" y2="0%">
      <stop offset="0%" stop-color="${ACCENT_A}"/>
      <stop offset="100%" stop-color="${ACCENT_B}"/>
    </linearGradient>
    <linearGradient id="barGrad" x1="0%" y1="0%" x2="100%" y2="0%">
      <stop offset="0%" stop-color="${ACCENT_A}" stop-opacity="0.85"/>
      <stop offset="100%" stop-color="${ACCENT_B}" stop-opacity="0.85"/>
    </linearGradient>
  </defs>

  <!-- background -->
  <rect width="${W}" height="${H}" fill="url(#bgGrad)"/>

  <!-- subtle horizontal stripe texture -->
  <g opacity="0.06" stroke="${INK}" stroke-width="1">
    ${Array.from({ length: 8 }, (_, i) => `<line x1="0" y1="${50 * i}" x2="${W}" y2="${50 * i}"/>`).join("")}
  </g>

  <!-- wordmark -->
  <g transform="translate(40, 36)">
    <rect x="0" y="0" width="28" height="28" rx="6" fill="url(#coreGrad)"/>
    <path d="M9 9 L14 20 L19 9" stroke="${INK}" stroke-width="1.5" fill="none" opacity="0.6"/>
    <circle cx="9" cy="9" r="3" fill="${INK}"/>
    <circle cx="19" cy="9" r="3" fill="${INK}"/>
    <circle cx="14" cy="20" r="3" fill="${INK}"/>
    <text x="40" y="20" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="18" font-weight="700" fill="${INK}">LoraDB</text>
    <text x="120" y="20" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="18" font-weight="500" fill="${ACCENT_A}">${escape("· Blog")}</text>
  </g>

  <!-- eyebrow -->
  <text x="40" y="170" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="14" font-weight="600" letter-spacing="3" fill="${INK_DIM}">${escape("RELEASE · v0.15 · OPEN BENCHMARK SUITE")}</text>

  <!-- headline -->
  <text x="40" y="234" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="52" font-weight="800" fill="${INK}">Benchmarks,</text>
  <text x="40" y="294" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="52" font-weight="800" fill="url(#headlineGrad)">in public.</text>

  <!-- tagline -->
  <text x="40" y="340" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="18" font-weight="400" fill="${INK_DIM}">Seven engines, 82 workloads, geomean slowdowns published</text>
  <text x="40" y="364" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="18" font-weight="400" fill="${INK_DIM}">in full, with every omission honestly noted.</text>

  <!-- right panel: per-engine bar chart + group chips + release stamp -->
  <g>
    <rect x="${PANEL_X}" y="${PANEL_Y}" width="${PANEL_W}" height="${PANEL_H}" rx="14" fill="${PANEL}" stroke="${PANEL_LINE}" stroke-width="1"/>
    <clipPath id="panelClip">
      <rect x="${PANEL_X}" y="${PANEL_Y}" width="${PANEL_W}" height="${PANEL_H}" rx="14"/>
    </clipPath>
    <g clip-path="url(#panelClip)">
      ${grid.join("\n")}
      ${chartCaption}
      ${chartHint}
      ${bars}
      ${footer}
    </g>
    ${stamp}
  </g>
</svg>`;
}

async function render(svg, width, height, outPath) {
  const buf = await sharp(Buffer.from(svg))
    .resize(width, height)
    .png({ compressionLevel: 9 })
    .withMetadata({})
    .toBuffer();
  await writeFile(outPath, buf);
  return buf.length;
}

async function main() {
  await mkdir(OUT_DIR, { recursive: true });
  const svg = buildSvg();
  const out1x = resolve(OUT_DIR, `${BASE_NAME}.png`);
  const out2x = resolve(OUT_DIR, `${BASE_NAME}@2x.png`);

  const [b1, b2] = await Promise.all([
    render(svg, W, H, out1x),
    render(svg, W * 2, H * 2, out2x),
  ]);

  const kb = (n) => `${(n / 1024).toFixed(1)} KB`;
  console.log(`[banner] wrote ${out1x} (${kb(b1)})`);
  console.log(`[banner] wrote ${out2x} (${kb(b2)})`);
}

main().catch((err) => {
  console.error("[banner] failed:", err);
  process.exit(1);
});
