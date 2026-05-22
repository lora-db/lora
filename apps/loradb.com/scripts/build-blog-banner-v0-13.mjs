#!/usr/bin/env node
// Generates the v0.13 release-post header banner.
//
//   yarn workspace loradb-docs node scripts/build-blog-banner-v0-13.mjs
//
// Output:
//   static/img/blog/loradb-v0-13-import-export-header.png      (1280x400)
//   static/img/blog/loradb-v0-13-import-export-header@2x.png   (2560x800)
//
// Visual: same layout family as v0.10 / v0.11 / v0.12 (eyebrow +
// headline + tagline on the left, panel on the right) but the right
// panel renders a table of rows flowing through a pipeline into a
// small graph cluster on the far side. The metaphor matches what
// v0.13 ships: row-level import / export across CSV, JSONL, and JSON.
//
// Deterministic: same SVG -> same PNG bytes (sharp metadata stripped).

import { writeFile, mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import sharp from "sharp";

const __dirname = dirname(fileURLToPath(import.meta.url));
const OUT_DIR = resolve(__dirname, "..", "static", "img", "blog");
const BASE_NAME = "loradb-v0-13-import-export-header";
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
const NODE_FILL = "#8fd4a1"; // graph-node mint (same as v0.12 hits)
const ROW_FILL = "#1a2447"; // row-cell background
const HEADER_FILL = "#22304f"; // column-header background

// Right panel inner rect (matches the v0.11/v0.12 banner so the layout
// family stays consistent across releases).
const PANEL_X = 640;
const PANEL_Y = 40;
const PANEL_W = 600;
const PANEL_H = 320;

// Left side of the panel: a small CSV table. Three columns, four
// data rows. Numbers are chosen so the table looks like real data,
// not lorem-ipsum-shaped placeholders.
const TABLE_X = PANEL_X + 28;
const TABLE_Y = PANEL_Y + 36;
const COL_W = [70, 92, 60];
const ROW_H = 28;
const HEADER_H = 26;

const HEADERS = ["id", "name", "age"];
const ROWS = [
  ["u1", "Alice", "32"],
  ["u2", "Bob", "27"],
  ["u3", "Carol", "41"],
  ["u4", "Dan", "29"],
];

// Right side of the panel: a small graph cluster (5 nodes, a few
// edges between them). Each node represents one row materialised
// into the graph after import.
const GRAPH_CENTRE_X = PANEL_X + PANEL_W - 110;
const GRAPH_CENTRE_Y = PANEL_Y + PANEL_H / 2;
const NODES = [
  { x: GRAPH_CENTRE_X - 38, y: GRAPH_CENTRE_Y - 60 },
  { x: GRAPH_CENTRE_X + 38, y: GRAPH_CENTRE_Y - 38 },
  { x: GRAPH_CENTRE_X + 46, y: GRAPH_CENTRE_Y + 22 },
  { x: GRAPH_CENTRE_X - 22, y: GRAPH_CENTRE_Y + 56 },
  { x: GRAPH_CENTRE_X - 56, y: GRAPH_CENTRE_Y + 4 },
];
const EDGES = [
  [0, 1],
  [1, 2],
  [2, 3],
  [3, 4],
  [4, 0],
  [0, 2],
];

// Pipeline arrows between the table and the graph cluster. Three
// rows fire off arrows to suggest streamed flow; the rest hint at
// continuation.
const ARROW_FROM_X = TABLE_X + COL_W.reduce((a, b) => a + b, 0) + 6;
const ARROW_TO_X = GRAPH_CENTRE_X - 90;
const ARROW_ROWS = [0, 1, 2];

function escape(s) {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function buildSvg() {
  // Subtle grid behind the panel suggests the import surface.
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

  // Column header strip.
  const headerCells = HEADERS.map((label, idx) => {
    const offsetX =
      TABLE_X + COL_W.slice(0, idx).reduce((a, b) => a + b, 0);
    return (
      `<rect x="${offsetX}" y="${TABLE_Y}" width="${COL_W[idx]}" height="${HEADER_H}" fill="${HEADER_FILL}" stroke="${PANEL_LINE}" stroke-width="1"/>` +
      `<text x="${offsetX + 8}" y="${TABLE_Y + 17}" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="11" font-weight="700" fill="${INK}">${escape(label)}</text>`
    );
  }).join("\n");

  // Data rows.
  const dataRows = ROWS.flatMap((cells, rowIdx) => {
    const rowY = TABLE_Y + HEADER_H + rowIdx * ROW_H;
    return cells.map((cell, colIdx) => {
      const offsetX =
        TABLE_X + COL_W.slice(0, colIdx).reduce((a, b) => a + b, 0);
      return (
        `<rect x="${offsetX}" y="${rowY}" width="${COL_W[colIdx]}" height="${ROW_H}" fill="${ROW_FILL}" stroke="${PANEL_LINE}" stroke-width="1"/>` +
        `<text x="${offsetX + 8}" y="${rowY + 18}" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="11" fill="${INK_DIM}">${escape(cell)}</text>`
      );
    });
  }).join("\n");

  // Streaming arrows from the table to the graph cluster.
  const arrows = ARROW_ROWS.map((rowIdx) => {
    const y = TABLE_Y + HEADER_H + rowIdx * ROW_H + ROW_H / 2;
    return (
      `<line x1="${ARROW_FROM_X}" y1="${y}" x2="${ARROW_TO_X - 8}" y2="${y}" stroke="url(#edgeGrad)" stroke-width="1.6" stroke-linecap="round" opacity="0.9"/>` +
      `<polygon points="${ARROW_TO_X - 8},${y - 4} ${ARROW_TO_X},${y} ${ARROW_TO_X - 8},${y + 4}" fill="${ACCENT_B}" opacity="0.95"/>`
    );
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

  // Caption above the graph cluster so the metaphor is legible at
  // thumbnail size.
  const caption = `<text x="${GRAPH_CENTRE_X}" y="${PANEL_Y + PANEL_H - 18}" text-anchor="middle" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="11" fill="${INK_DIM}">rows → graph</text>`;

  // Top-right corner stamp listing the formats v0.13 ships.
  const stamp =
    `<g transform="translate(${PANEL_X + PANEL_W - 184}, ${PANEL_Y + 24})">` +
    `<rect x="0" y="0" width="170" height="58" rx="8" fill="${BG_A}" stroke="${PANEL_LINE}" stroke-width="1" opacity="0.92"/>` +
    `<text x="12" y="20" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="10" fill="${INK_DIM}">ROW IO · v0.13</text>` +
    `<text x="12" y="36" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="10" fill="${INK}">csv · jsonl · json</text>` +
    `<text x="12" y="50" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="10" fill="${NODE_FILL}">streaming both ways</text>` +
    `</g>`;

  return `<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${H}" viewBox="0 0 ${W} ${H}">
  <defs>
    <linearGradient id="bgGrad" x1="0%" y1="0%" x2="100%" y2="100%">
      <stop offset="0%" stop-color="${BG_A}"/>
      <stop offset="100%" stop-color="${BG_B}"/>
    </linearGradient>
    <linearGradient id="edgeGrad" x1="0%" y1="0%" x2="100%" y2="0%">
      <stop offset="0%" stop-color="${ACCENT_A}"/>
      <stop offset="100%" stop-color="${ACCENT_B}"/>
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
  <text x="40" y="170" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="14" font-weight="600" letter-spacing="3" fill="${INK_DIM}">${escape("RELEASE · v0.13 · IMPORT / EXPORT")}</text>

  <!-- headline -->
  <text x="40" y="234" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="52" font-weight="800" fill="${INK}">Rows in,</text>
  <text x="40" y="294" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="52" font-weight="800" fill="url(#headlineGrad)">graph out.</text>

  <!-- tagline -->
  <text x="40" y="340" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="18" font-weight="400" fill="${INK_DIM}">CSV, JSONL, and JSON. Streaming both ways,</text>
  <text x="40" y="364" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="18" font-weight="400" fill="${INK_DIM}">through one wizard and one set of codecs.</text>

  <!-- right panel: rows -> graph -->
  <g>
    <rect x="${PANEL_X}" y="${PANEL_Y}" width="${PANEL_W}" height="${PANEL_H}" rx="14" fill="${PANEL}" stroke="${PANEL_LINE}" stroke-width="1"/>
    <clipPath id="panelClip">
      <rect x="${PANEL_X}" y="${PANEL_Y}" width="${PANEL_W}" height="${PANEL_H}" rx="14"/>
    </clipPath>
    <g clip-path="url(#panelClip)">
      ${grid.join("\n")}
      ${headerCells}
      ${dataRows}
      ${arrows}
      ${edgeLines}
      ${nodeDots}
      ${caption}
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
