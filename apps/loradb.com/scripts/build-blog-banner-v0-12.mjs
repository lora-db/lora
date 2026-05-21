#!/usr/bin/env node
// Generates the v0.12 release-post header banner.
//
//   yarn workspace loradb-docs node scripts/build-blog-banner-v0-12.mjs
//
// Output:
//   static/img/blog/loradb-v0-12-vector-indexing-header.png      (1280x400)
//   static/img/blog/loradb-v0-12-vector-indexing-header@2x.png   (2560x800)
//
// Visual: same layout family as v0.10 / v0.11 (eyebrow + headline +
// tagline on the left, panel on the right) but the right panel
// renders a 2D embedding space instead of a Cypher IDE mock. A query
// point in the centre, scattered embeddings around it, the top-k
// nearest neighbours highlighted with the brand-gradient edge. The
// metaphor matches what v0.12 ships: similarity search over vectors.
//
// Deterministic: same SVG -> same PNG bytes (sharp metadata stripped).

import { writeFile, mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import sharp from "sharp";

const __dirname = dirname(fileURLToPath(import.meta.url));
const OUT_DIR = resolve(__dirname, "..", "static", "img", "blog");
const BASE_NAME = "loradb-v0-12-vector-indexing-header";
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
const HIT = "#8fd4a1"; // top-k neighbour highlight (mint)
const FAR = "#3a4470"; // far-away embeddings (dim)

// Right panel inner rect (matches v0.11 banner so the layout family
// stays consistent across releases).
const PANEL_X = 640;
const PANEL_Y = 40;
const PANEL_W = 600;
const PANEL_H = 320;

// Embedding space inside the panel. The query lands near the visual
// centre; neighbours are placed deterministically so the generated
// PNG is reproducible.
const QUERY = { x: 940, y: 210, r: 14 };

// 5 nearest neighbours, ordered by distance to QUERY. These are
// emphasised with a brand-gradient connector and a green halo.
const NEAREST = [
  { x: 902, y: 178, r: 8, rank: 1 },
  { x: 980, y: 178, r: 8, rank: 2 },
  { x: 905, y: 244, r: 8, rank: 3 },
  { x: 978, y: 246, r: 8, rank: 4 },
  { x: 940, y: 156, r: 8, rank: 5 },
];

// Far-field embeddings drawn dim. Hand-placed to feel scattered
// without overlapping the nearest set.
const FARS = [
  { x: 730, y: 110, r: 5 },
  { x: 805, y: 95, r: 5 },
  { x: 870, y: 305, r: 5 },
  { x: 1040, y: 100, r: 5 },
  { x: 1120, y: 175, r: 5 },
  { x: 1160, y: 270, r: 5 },
  { x: 1075, y: 315, r: 5 },
  { x: 760, y: 230, r: 5 },
  { x: 700, y: 305, r: 5 },
  { x: 1150, y: 105, r: 5 },
  { x: 820, y: 320, r: 5 },
  { x: 1185, y: 200, r: 5 },
  { x: 870, y: 130, r: 5 },
  { x: 1010, y: 305, r: 5 },
  { x: 1180, y: 130, r: 5 },
  { x: 745, y: 175, r: 5 },
];

function escape(s) {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function buildSvg() {
  // Grid lines suggest the embedding space.
  const grid = [];
  for (let x = PANEL_X + 30; x < PANEL_X + PANEL_W; x += 30) {
    grid.push(
      `<line x1="${x}" y1="${PANEL_Y + 14}" x2="${x}" y2="${PANEL_Y + PANEL_H - 14}" stroke="${PANEL_LINE}" stroke-opacity="0.5" stroke-width="1"/>`,
    );
  }
  for (let y = PANEL_Y + 30; y < PANEL_Y + PANEL_H; y += 30) {
    grid.push(
      `<line x1="${PANEL_X + 14}" y1="${y}" x2="${PANEL_X + PANEL_W - 14}" y2="${y}" stroke="${PANEL_LINE}" stroke-opacity="0.5" stroke-width="1"/>`,
    );
  }

  // Connectors from query to top-k. Drawn under the points so the
  // dot caps cover the line endings cleanly.
  const connectors = NEAREST.map(
    (n) =>
      `<line x1="${QUERY.x}" y1="${QUERY.y}" x2="${n.x}" y2="${n.y}" stroke="url(#edgeGrad)" stroke-width="1.5" stroke-linecap="round" opacity="0.95"/>`,
  ).join("\n");

  // Far-field points: dim, no labels.
  const farDots = FARS.map(
    (n) =>
      `<circle cx="${n.x}" cy="${n.y}" r="${n.r}" fill="${FAR}" opacity="0.85"/>`,
  ).join("\n");

  // Top-k points: green halo + mint fill, with the rank glyph
  // overlaid on the largest hit (so the visual reads as "ranked
  // nearest neighbours").
  const hitDots = NEAREST.map(
    (n) =>
      `<circle cx="${n.x}" cy="${n.y}" r="${n.r + 6}" fill="${HIT}" opacity="0.12"/>` +
      `<circle cx="${n.x}" cy="${n.y}" r="${n.r}" fill="${HIT}" stroke="${INK}" stroke-opacity="0.4" stroke-width="1"/>` +
      `<text x="${n.x}" y="${n.y + 3}" text-anchor="middle" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="9" font-weight="700" fill="${BG_A}">${n.rank}</text>`,
  ).join("\n");

  // Query point. Large, brand-gradient filled, with a soft halo so
  // it reads as the centre of attention.
  const queryDot =
    `<circle cx="${QUERY.x}" cy="${QUERY.y}" r="${QUERY.r + 22}" fill="url(#coreGlow)"/>` +
    `<circle cx="${QUERY.x}" cy="${QUERY.y}" r="${QUERY.r + 5}" fill="none" stroke="${ACCENT_A}" stroke-opacity="0.35" stroke-width="1"/>` +
    `<circle cx="${QUERY.x}" cy="${QUERY.y}" r="${QUERY.r}" fill="url(#coreGrad)" stroke="${INK}" stroke-opacity="0.18"/>` +
    `<circle cx="${QUERY.x}" cy="${QUERY.y}" r="${(QUERY.r * 0.4).toFixed(2)}" fill="${INK}" opacity="0.85"/>`;

  // Floating caption above the query so the metaphor is legible
  // even at thumbnail size. Kept short.
  const caption = `<text x="${QUERY.x}" y="${PANEL_Y + PANEL_H - 14}" text-anchor="middle" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="11" fill="${INK_DIM}">k-NN over embeddings</text>`;

  // Top-right corner stamp with the active configuration so the
  // banner feels like a screenshot of a real tool rather than pure
  // illustration.
  const stamp =
    `<g transform="translate(${PANEL_X + PANEL_W - 184}, ${PANEL_Y + 24})">` +
    `<rect x="0" y="0" width="170" height="58" rx="8" fill="${BG_A}" stroke="${PANEL_LINE}" stroke-width="1" opacity="0.92"/>` +
    `<text x="12" y="20" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="10" fill="${INK_DIM}">VECTOR INDEX</text>` +
    `<text x="12" y="36" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="10" fill="${INK}">hnsw · cosine</text>` +
    `<text x="12" y="50" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="10" fill="${HIT}">k=5 · recall ≥ 0.95</text>` +
    `</g>`;

  return `<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${H}" viewBox="0 0 ${W} ${H}">
  <defs>
    <linearGradient id="bgGrad" x1="0%" y1="0%" x2="100%" y2="100%">
      <stop offset="0%" stop-color="${BG_A}"/>
      <stop offset="100%" stop-color="${BG_B}"/>
    </linearGradient>
    <linearGradient id="edgeGrad" x1="0%" y1="0%" x2="100%" y2="100%">
      <stop offset="0%" stop-color="${ACCENT_A}"/>
      <stop offset="100%" stop-color="${ACCENT_B}"/>
    </linearGradient>
    <linearGradient id="coreGrad" x1="0%" y1="0%" x2="100%" y2="100%">
      <stop offset="0%" stop-color="${ACCENT_A}"/>
      <stop offset="100%" stop-color="${ACCENT_B}"/>
    </linearGradient>
    <radialGradient id="coreGlow" cx="50%" cy="50%" r="50%">
      <stop offset="0%" stop-color="${ACCENT_B}" stop-opacity="0.55"/>
      <stop offset="100%" stop-color="${ACCENT_B}" stop-opacity="0"/>
    </radialGradient>
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
  <text x="40" y="170" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="14" font-weight="600" letter-spacing="3" fill="${INK_DIM}">${escape("RELEASE · v0.12 · VECTORS")}</text>

  <!-- headline -->
  <text x="40" y="234" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="52" font-weight="800" fill="${INK}">Vectors,</text>
  <text x="40" y="294" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="52" font-weight="800" fill="url(#headlineGrad)">end to end.</text>

  <!-- tagline -->
  <text x="40" y="340" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="18" font-weight="400" fill="${INK_DIM}">HNSW k-NN, hybrid filters, int8 quantization,</text>
  <text x="40" y="364" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="18" font-weight="400" fill="${INK_DIM}">persisted graphs. All in the same Cypher engine.</text>

  <!-- right panel: embedding space -->
  <g>
    <rect x="${PANEL_X}" y="${PANEL_Y}" width="${PANEL_W}" height="${PANEL_H}" rx="14" fill="${PANEL}" stroke="${PANEL_LINE}" stroke-width="1"/>
    <clipPath id="panelClip">
      <rect x="${PANEL_X}" y="${PANEL_Y}" width="${PANEL_W}" height="${PANEL_H}" rx="14"/>
    </clipPath>
    <g clip-path="url(#panelClip)">
      ${grid.join("\n")}
      ${farDots}
      ${connectors}
      ${hitDots}
      ${queryDot}
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
