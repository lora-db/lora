#!/usr/bin/env node
// Generates the v0.17 release-post header banner.
//
//   yarn workspace loradb-docs node scripts/build-blog-banner-v0-17.mjs
//
// Output:
//   static/img/blog/loradb-v0-17-inside-lora-graphql-header.png      (1280x400)
//   static/img/blog/loradb-v0-17-inside-lora-graphql-header@2x.png   (2560x800)
//
// Visual: same layout family as v0.10 to v0.15 (eyebrow + headline +
// tagline on the left, panel on the right). The right panel shows the
// lora-graphql compile pipeline as four chips (SDL, model, schema,
// Cypher) above an excerpt of the statement one root field compiles to,
// taken from the post's own example.
//
// Deterministic: same SVG -> same PNG bytes (sharp metadata stripped).

import { writeFile, mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import sharp from "sharp";

const __dirname = dirname(fileURLToPath(import.meta.url));
const OUT_DIR = resolve(__dirname, "..", "static", "img", "blog");
const BASE_NAME = "loradb-v0-17-inside-lora-graphql-header";
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
const MINT = "#8fd4a1";
const CODE_BG = "#0b1020";

const PANEL_X = 640;
const PANEL_Y = 40;
const PANEL_W = 600;
const PANEL_H = 320;

const MONO = "ui-monospace, SFMono-Regular, Menlo, monospace";
const SANS = "system-ui, -apple-system, Segoe UI, Roboto, sans-serif";

// Pipeline chips, left to right. The last one is the output.
const STAGES = [
  { label: "SDL", hint: "annotated" },
  { label: "model", hint: "validated" },
  { label: "schema", hint: "graphql-js" },
  { label: "Cypher", hint: "1 per field", out: true },
];

// Excerpt of the compiled statement (see the post). Each line is a list
// of [text, color] runs.
const KW = ACCENT_B;
const PARAM = MINT;
const CODE = [
  [["MATCH ", KW], ["(this:Festival)", INK]],
  [["WHERE ", KW], ["this.capacity > ", INK], ["$p0", PARAM]],
  [["WITH ", KW], ["this ", INK], ["ORDER BY ", KW], ["this.name ", INK], ["LIMIT ", KW], ["$p1", PARAM]],
  [["CALL ", KW], ["{ ", INK], ["MATCH ", KW], ["(this)<-[:FOLLOWS]-(f:User) ", INK], ["… ", INK_DIM], ["}", INK]],
  [["RETURN ", KW], ["this { .key, .name, followers } ", INK], ["AS ", KW], ["this", INK]],
];

function escape(s) {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function buildSvg() {
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

  const left = PANEL_X + 28;
  const captionY = PANEL_Y + 38;
  const caption =
    `<text x="${left}" y="${captionY}" font-family="${MONO}" font-size="11" font-weight="700" fill="${INK}">compile pipeline</text>` +
    `<text x="${left}" y="${captionY + 14}" font-family="${MONO}" font-size="10" fill="${INK_DIM}">no resolvers walking the graph</text>`;

  // Chips sit below the release stamp.
  const CHIP_W = 112;
  const CHIP_H = 44;
  const GAP = 24;
  const chipsY = PANEL_Y + 100;
  const chips = STAGES.map((stage, i) => {
    const x = left + i * (CHIP_W + GAP);
    const fill = stage.out ? "url(#coreGrad)" : BG_A;
    const stroke = stage.out ? "none" : PANEL_LINE;
    const labelColor = INK;
    const hintColor = stage.out ? INK : INK_DIM;
    const arrow =
      i < STAGES.length - 1
        ? `<path d="M${x + CHIP_W + 5} ${chipsY + CHIP_H / 2} L${x + CHIP_W + GAP - 7} ${chipsY + CHIP_H / 2}" stroke="${ACCENT_A}" stroke-width="1.5"/>` +
          `<path d="M${x + CHIP_W + GAP - 11} ${chipsY + CHIP_H / 2 - 4} L${x + CHIP_W + GAP - 6} ${chipsY + CHIP_H / 2} L${x + CHIP_W + GAP - 11} ${chipsY + CHIP_H / 2 + 4}" stroke="${ACCENT_A}" stroke-width="1.5" fill="none"/>`
        : "";
    return (
      `<rect x="${x}" y="${chipsY}" width="${CHIP_W}" height="${CHIP_H}" rx="8" fill="${fill}" stroke="${stroke}" stroke-width="1"/>` +
      `<text x="${x + 12}" y="${chipsY + 19}" font-family="${MONO}" font-size="13" font-weight="700" fill="${labelColor}">${escape(stage.label)}</text>` +
      `<text x="${x + 12}" y="${chipsY + 35}" font-family="${MONO}" font-size="10" fill="${hintColor}">${escape(stage.hint)}</text>` +
      arrow
    );
  }).join("\n");

  // Code block under the chips.
  const codeY = chipsY + CHIP_H + 16;
  const codeH = 118;
  const codeW = PANEL_W - 56;
  const LINE_H = 20;
  const lines = CODE.map((runs, i) => {
    const tspans = runs
      .map(([text, color]) => `<tspan fill="${color}">${escape(text)}</tspan>`)
      .join("");
    return `<text x="${left + 14}" y="${codeY + 26 + i * LINE_H}" font-family="${MONO}" font-size="11.5" xml:space="preserve">${tspans}</text>`;
  }).join("\n");
  const code =
    `<rect x="${left}" y="${codeY}" width="${codeW}" height="${codeH}" rx="8" fill="${CODE_BG}" stroke="${PANEL_LINE}" stroke-width="1"/>` +
    lines;

  const footerY = PANEL_Y + PANEL_H - 14;
  const footer = `<text x="${left}" y="${footerY}" font-family="${MONO}" font-size="10" fill="${INK_DIM}">explain()-checked · rules compiled in · keyed write-sets</text>`;

  const stamp =
    `<g transform="translate(${PANEL_X + PANEL_W - 184}, ${PANEL_Y + 24})">` +
    `<rect x="0" y="0" width="170" height="58" rx="8" fill="${BG_A}" stroke="${PANEL_LINE}" stroke-width="1" opacity="0.92"/>` +
    `<text x="12" y="20" font-family="${MONO}" font-size="10" fill="${INK_DIM}">GRAPHQL · v0.17</text>` +
    `<text x="12" y="36" font-family="${MONO}" font-size="10" fill="${INK}">@loradb/lora-graphql</text>` +
    `<text x="12" y="50" font-family="${MONO}" font-size="10" fill="${MINT}">graphql 16 and 17</text>` +
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
    <text x="40" y="20" font-family="${SANS}" font-size="18" font-weight="700" fill="${INK}">LoraDB</text>
    <text x="120" y="20" font-family="${SANS}" font-size="18" font-weight="500" fill="${ACCENT_A}">${escape("· Blog")}</text>
  </g>

  <!-- eyebrow -->
  <text x="40" y="170" font-family="${SANS}" font-size="14" font-weight="600" letter-spacing="3" fill="${INK_DIM}">${escape("RELEASE · v0.17 · HOW IT WORKS")}</text>

  <!-- headline -->
  <text x="40" y="234" font-family="${SANS}" font-size="52" font-weight="800" fill="${INK}">Inside</text>
  <text x="40" y="294" font-family="${SANS}" font-size="52" font-weight="800" fill="url(#headlineGrad)">lora-graphql.</text>

  <!-- tagline -->
  <text x="40" y="340" font-family="${SANS}" font-size="18" font-weight="400" fill="${INK_DIM}">One schema in, one Cypher statement per root field out,</text>
  <text x="40" y="364" font-family="${SANS}" font-size="18" font-weight="400" fill="${INK_DIM}">with its plan checked before it ever runs.</text>

  <!-- right panel -->
  <g>
    <rect x="${PANEL_X}" y="${PANEL_Y}" width="${PANEL_W}" height="${PANEL_H}" rx="14" fill="${PANEL}" stroke="${PANEL_LINE}" stroke-width="1"/>
    <clipPath id="panelClip">
      <rect x="${PANEL_X}" y="${PANEL_Y}" width="${PANEL_W}" height="${PANEL_H}" rx="14"/>
    </clipPath>
    <g clip-path="url(#panelClip)">
      ${grid.join("\n")}
      ${caption}
      ${chips}
      ${code}
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
