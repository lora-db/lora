#!/usr/bin/env node
// Generates the v0.14 release-post header banner.
//
//   yarn workspace loradb-docs node scripts/build-blog-banner-v0-14.mjs
//
// Output:
//   static/img/blog/loradb-v0-14-hot-paths-and-honest-errors-header.png      (1280x400)
//   static/img/blog/loradb-v0-14-hot-paths-and-honest-errors-header@2x.png   (2560x800)
//
// Visual: same layout family as v0.10 / v0.11 / v0.12 / v0.13 (eyebrow +
// headline + tagline on the left, panel on the right). The right panel
// pairs a small retained-heap bar chart (the new MemoryReport surface
// that the Stats side panel renders) with a graph cluster and a stamp
// listing the new error codes that v0.14 ships. The two halves match
// the release's two anchor stories: hot paths (memory work and the OCC
// staged write) and honest errors (the expanded LoraErrorCode).
//
// Deterministic: same SVG -> same PNG bytes (sharp metadata stripped).

import { writeFile, mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import sharp from "sharp";

const __dirname = dirname(fileURLToPath(import.meta.url));
const OUT_DIR = resolve(__dirname, "..", "static", "img", "blog");
const BASE_NAME = "loradb-v0-14-hot-paths-and-honest-errors-header";
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
const NODE_FILL = "#8fd4a1"; // graph-node mint
const BAR_BEFORE = "#3b4774"; // dim track (pre-interning baseline)
const BAR_AFTER = "#5b8def"; // bright fill (post-interning footprint)
const ERR_FILL = "#ff7e8b"; // soft red for error-code chips

// Right panel inner rect (matches the prior banners so the layout
// family stays consistent across releases).
const PANEL_X = 640;
const PANEL_Y = 40;
const PANEL_W = 600;
const PANEL_H = 320;

// Left half of the panel: the retained-heap bar chart. Four rows,
// each showing a before/after footprint for one MemoryReport bucket.
// The "after" widths are intentionally smaller than "before" — the
// metaphor is "interning shrinks retained heap", not literal numbers.
const CHART_X = PANEL_X + 28;
const CHART_Y = PANEL_Y + 44;
const CHART_W = 248;
const ROW_H = 30;
const BAR_H = 14;

const BUCKETS = [
  { label: "nodes", before: 0.96, after: 0.42 },
  { label: "rels", before: 0.82, after: 0.36 },
  { label: "indexes", before: 0.6, after: 0.5 },
  { label: "adj", before: 0.48, after: 0.46 },
];

// Right half of the panel: a small graph cluster. The cluster is
// purely brand continuity — same shape family as the v0.13 banner so
// the release-card stack stays coherent.
const GRAPH_CENTRE_X = PANEL_X + PANEL_W - 102;
const GRAPH_CENTRE_Y = PANEL_Y + PANEL_H / 2 - 30;
const NODES = [
  { x: GRAPH_CENTRE_X - 38, y: GRAPH_CENTRE_Y - 56 },
  { x: GRAPH_CENTRE_X + 40, y: GRAPH_CENTRE_Y - 30 },
  { x: GRAPH_CENTRE_X + 46, y: GRAPH_CENTRE_Y + 30 },
  { x: GRAPH_CENTRE_X - 22, y: GRAPH_CENTRE_Y + 56 },
  { x: GRAPH_CENTRE_X - 54, y: GRAPH_CENTRE_Y + 4 },
];
const EDGES = [
  [0, 1],
  [1, 2],
  [2, 3],
  [3, 4],
  [4, 0],
  [0, 2],
];

// Error-code chips along the bottom of the right panel. Four codes
// shown — the release introduces six but four fit visually.
const ERR_CODES = [
  "LORA_VALIDATION",
  "LORA_UNIQUE_CONSTRAINT",
  "LORA_FOREIGN_KEY",
  "LORA_CONNECTION",
];

function escape(s) {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
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

  // Chart caption.
  const chartCaption = `<text x="${CHART_X}" y="${PANEL_Y + 30}" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="11" font-weight="700" fill="${INK}">retained heap</text>`;
  const chartHint = `<text x="${CHART_X + CHART_W}" y="${PANEL_Y + 30}" text-anchor="end" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="10" fill="${INK_DIM}">before / after</text>`;

  // Bar rows.
  const bars = BUCKETS.map((bucket, idx) => {
    const rowY = CHART_Y + idx * ROW_H;
    const labelY = rowY + BAR_H - 3;
    const beforeW = Math.round(CHART_W * 0.62 * bucket.before);
    const afterW = Math.round(CHART_W * 0.62 * bucket.after);
    const trackX = CHART_X + 60;
    return [
      `<text x="${CHART_X}" y="${labelY}" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="11" fill="${INK_DIM}">${escape(bucket.label)}</text>`,
      `<rect x="${trackX}" y="${rowY}" width="${beforeW}" height="${BAR_H}" rx="3" fill="${BAR_BEFORE}" opacity="0.85"/>`,
      `<rect x="${trackX}" y="${rowY}" width="${afterW}" height="${BAR_H}" rx="3" fill="${BAR_AFTER}"/>`,
    ].join("\n");
  }).join("\n");

  // Graph cluster: edges first so the nodes sit on top.
  const edgeLines = EDGES.map(([a, b]) => {
    const na = NODES[a];
    const nb = NODES[b];
    return `<line x1="${na.x}" y1="${na.y}" x2="${nb.x}" y2="${nb.y}" stroke="${PANEL_LINE}" stroke-width="1.5" opacity="0.8"/>`;
  }).join("\n");
  const nodeDots = NODES.map(
    (n) =>
      `<circle cx="${n.x}" cy="${n.y}" r="14" fill="${NODE_FILL}" stroke="${INK}" stroke-opacity="0.32" stroke-width="1"/>`,
  ).join("\n");
  const graphCaption = `<text x="${GRAPH_CENTRE_X}" y="${PANEL_Y + PANEL_H - 96}" text-anchor="middle" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="11" fill="${INK_DIM}">staged · published</text>`;

  // Error-code chips along the panel bottom.
  const chipY = PANEL_Y + PANEL_H - 56;
  const chipH = 22;
  const chipGap = 6;
  let chipX = PANEL_X + 18;
  const chips = ERR_CODES.map((code) => {
    const textW = code.length * 6 + 18;
    const chip = `<g transform="translate(${chipX}, ${chipY})">
  <rect x="0" y="0" width="${textW}" height="${chipH}" rx="6" fill="${BG_A}" stroke="${ERR_FILL}" stroke-opacity="0.55" stroke-width="1"/>
  <text x="${textW / 2}" y="${chipH - 7}" text-anchor="middle" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="10" fill="${ERR_FILL}">${escape(code)}</text>
</g>`;
    chipX += textW + chipGap;
    return chip;
  }).join("\n");
  const chipsCaption = `<text x="${PANEL_X + 18}" y="${chipY - 10}" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="10" font-weight="700" fill="${INK}">new error codes</text>`;

  // Top-right corner stamp — release tag.
  const stamp =
    `<g transform="translate(${PANEL_X + PANEL_W - 184}, ${PANEL_Y + 24})">` +
    `<rect x="0" y="0" width="170" height="58" rx="8" fill="${BG_A}" stroke="${PANEL_LINE}" stroke-width="1" opacity="0.92"/>` +
    `<text x="12" y="20" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="10" fill="${INK_DIM}">ENGINE · v0.14</text>` +
    `<text x="12" y="36" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="10" fill="${INK}">staged auto-commit</text>` +
    `<text x="12" y="50" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="10" fill="${NODE_FILL}">interned property keys</text>` +
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
  <text x="40" y="170" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="14" font-weight="600" letter-spacing="3" fill="${INK_DIM}">${escape("RELEASE · v0.14 · STAGED WRITES + TYPED ERRORS")}</text>

  <!-- headline -->
  <text x="40" y="234" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="52" font-weight="800" fill="${INK}">Hot paths,</text>
  <text x="40" y="294" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="52" font-weight="800" fill="url(#headlineGrad)">honest errors.</text>

  <!-- tagline -->
  <text x="40" y="340" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="18" font-weight="400" fill="${INK_DIM}">A staged auto-commit path, interned property keys,</text>
  <text x="40" y="364" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="18" font-weight="400" fill="${INK_DIM}">and an error model bindings can actually branch on.</text>

  <!-- right panel: memory breakdown + graph cluster + error chips -->
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
      ${edgeLines}
      ${nodeDots}
      ${graphCaption}
      ${chipsCaption}
      ${chips}
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
