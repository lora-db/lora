#!/usr/bin/env node
// Generates the v0.11 release-post header banner.
//
//   yarn workspace loradb-docs node scripts/build-blog-banner-v0-11.mjs
//
// Output:
//   static/img/blog/loradb-v0-11-playground-header.png      (1280x400)
//   static/img/blog/loradb-v0-11-playground-header@2x.png   (2560x800)
//
// Visual: same layout family as the v0.10 banner — eyebrow, headline,
// tagline on the left; a mini IDE mock (window chrome, syntax-coloured
// Cypher query, graph with brand-gradient edges) on the right. Colours
// pulled from the loradb.com brand tokens so the rendered banner reads
// consistently with the marketing pages.
//
// Deterministic: same SVG → same PNG bytes (sharp metadata stripped).

import { writeFile, mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import sharp from "sharp";

const __dirname = dirname(fileURLToPath(import.meta.url));
const OUT_DIR = resolve(__dirname, "..", "static", "img", "blog");
const BASE_NAME = "loradb-v0-11-playground-header";
const W = 1280;
const H = 400;

// Brand tokens — same values used in src/styles for the dark theme.
const BG_A = "#0b1020";
const BG_B = "#161c34";
const PANEL = "#0f1530";
const PANEL_LINE = "#1e2748";
const ACCENT_A = "#5b8def"; // brand-accent-a (blue)
const ACCENT_B = "#9b6bff"; // brand-accent-b (violet)
const INK = "#e7ecff";
const INK_DIM = "#9aa3c2";
const KEYWORD = "#9b6bff";
const STRING = "#8fd4a1";
const FN = "#5b8def";
const VAR = "#e7ecff";
const PUNCT = "#7c87aa";

// Cypher query lines for the editor pane. Kept short so they don't
// overflow the 460px-wide editor at any density.
const LINES = [
  [
    ["kw", "MATCH"],
    ["p", " (a:"],
    ["lbl", "Agent"],
    ["p", ")-->("],
    ["lbl", "Ctx"],
    ["p", ")"],
  ],
  [
    ["p", "  -->("],
    ["lbl", "Entity"],
    ["p", ")"],
  ],
  [
    ["kw", "WHERE"],
    ["p", " c.fresh"],
  ],
  [
    ["kw", "RETURN"],
    ["p", " "],
    ["fn", "collect"],
    ["p", "(c)"],
  ],
];

// Graph layout for the right panel — hub-and-spoke that mirrors the
// query: Agent at the centre, Context+Entity satellites.
const NODES = [
  { id: "agent", x: 1090, y: 200, r: 22, label: "Agent", kind: "core" },
  { id: "c1", x: 985, y: 120, r: 14, label: "Context", kind: "primary" },
  { id: "c2", x: 960, y: 215, r: 14, label: "Context", kind: "primary" },
  { id: "c3", x: 980, y: 305, r: 14, label: "Context", kind: "primary" },
  { id: "e1", x: 875, y: 95, r: 11, label: "Entity", kind: "primary" },
  { id: "e2", x: 855, y: 215, r: 11, label: "Entity", kind: "primary" },
  { id: "e3", x: 875, y: 330, r: 11, label: "Entity", kind: "primary" },
  { id: "tool", x: 1175, y: 115, r: 8, label: "Tool", kind: "satellite" },
  { id: "session", x: 1185, y: 290, r: 8, label: "Session", kind: "satellite" },
];

const EDGES = [
  ["agent", "c1", "flow"],
  ["agent", "c2", "flow"],
  ["agent", "c3", "flow"],
  ["c1", "e1", "flow"],
  ["c2", "e2", "flow"],
  ["c3", "e3", "flow"],
  ["agent", "tool", "soft"],
  ["agent", "session", "soft"],
];

function escape(s) {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

function token(kind, text) {
  const fill = {
    kw: KEYWORD,
    lbl: STRING,
    rel: FN,
    fn: FN,
    p: VAR,
    s: STRING,
  }[kind] || VAR;
  return `<tspan fill="${fill}">${escape(text)}</tspan>`;
}

function edgePath(a, b, bend) {
  const mx = (a.x + b.x) / 2;
  const my = (a.y + b.y) / 2;
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const cx = mx + -dy * bend;
  const cy = my + dx * bend;
  return `M ${a.x} ${a.y} Q ${cx} ${cy} ${b.x} ${b.y}`;
}

function buildSvg() {
  const byId = Object.fromEntries(NODES.map((n) => [n.id, n]));

  const edgeSvg = EDGES.map(([from, to, variant], i) => {
    const a = byId[from];
    const b = byId[to];
    const bend = ((i % 2) === 0 ? 1 : -1) * 0.1;
    const d = edgePath(a, b, bend);
    if (variant === "soft") {
      return `<path d="${d}" fill="none" stroke="${PANEL_LINE}" stroke-width="1.5" stroke-dasharray="4 4" stroke-linecap="round" opacity="0.85"/>`;
    }
    return `<path d="${d}" fill="none" stroke="url(#edgeGrad)" stroke-width="2" stroke-linecap="round" opacity="0.95"/>`;
  }).join("\n");

  const nodeSvg = NODES.map((n) => {
    const halo = n.kind === "core"
      ? `<circle cx="${n.x}" cy="${n.y}" r="${n.r + 18}" fill="url(#coreGlow)" />`
      : "";
    const ring = n.kind !== "satellite"
      ? `<circle cx="${n.x}" cy="${n.y}" r="${n.r + 5}" fill="none" stroke="${ACCENT_A}" stroke-opacity="0.35" stroke-width="1"/>`
      : "";
    const fill = n.kind === "core" ? "url(#coreGrad)" : ACCENT_A;
    const dot = `<circle cx="${n.x}" cy="${n.y}" r="${n.r}" fill="${fill}" stroke="${INK}" stroke-opacity="0.18"/>`;
    const inner = `<circle cx="${n.x}" cy="${n.y}" r="${(n.r * 0.35).toFixed(2)}" fill="${INK}" opacity="0.85"/>`;
    return `${halo}${ring}${dot}${inner}`;
  }).join("\n");

  // Editor pane sits inside the right panel: x ∈ [650, 940].
  // Gutter at x=672, code at x=692.
  const editorY0 = 130;
  const lineH = 38;
  const editorSvg = LINES.map((line, i) => {
    const y = editorY0 + i * lineH;
    const parts = line.map(([k, t]) => token(k, t)).join("");
    return `<text xml:space="preserve" x="692" y="${y}" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="13" fill="${INK}">${parts}</text>
            <text x="672" y="${y}" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="11" fill="${INK_DIM}" text-anchor="end" opacity="0.55">${i + 1}</text>`;
  }).join("\n");

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

  <!-- subtle grid texture -->
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
    <text x="120" y="20" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="18" font-weight="500" fill="${ACCENT_A}">· Blog</text>
  </g>

  <!-- eyebrow -->
  <text x="40" y="170" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="14" font-weight="600" letter-spacing="3" fill="${INK_DIM}">RELEASE · v0.11 · PLAYGROUND</text>

  <!-- headline -->
  <text x="40" y="234" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="52" font-weight="800" fill="${INK}">Cypher in your</text>
  <text x="40" y="294" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="52" font-weight="800" fill="url(#headlineGrad)">browser.</text>

  <!-- tagline -->
  <text x="40" y="340" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="18" font-weight="400" fill="${INK_DIM}">An in-browser IDE for LoraDB — WASM engine, graph canvas,</text>
  <text x="40" y="364" font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif" font-size="18" font-weight="400" fill="${INK_DIM}">shareable queries. play.loradb.com.</text>

  <!-- right panel: mini IDE mock -->

  <g>
    <!-- panel background -->
    <rect x="640" y="40" width="600" height="320" rx="14" fill="${PANEL}" stroke="${PANEL_LINE}" stroke-width="1"/>

    <!-- title bar -->
    <rect x="640" y="40" width="600" height="34" rx="14" fill="${PANEL_LINE}" opacity="0.55"/>
    <rect x="640" y="60" width="600" height="14" fill="${PANEL_LINE}" opacity="0.55"/>
    <g transform="translate(660, 50)">
      <circle cx="0" cy="7" r="5" fill="#ff6058"/>
      <circle cx="16" cy="7" r="5" fill="#ffbd2e"/>
      <circle cx="32" cy="7" r="5" fill="#27c93f"/>
    </g>
    <text x="850" y="62" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="12" fill="${INK_DIM}" text-anchor="middle">play.loradb.com</text>
    <g transform="translate(1175, 50)">
      <rect x="0" y="-1" width="48" height="18" rx="9" fill="url(#coreGrad)"/>
      <text x="24" y="12" font-family="system-ui, sans-serif" font-size="10" font-weight="700" fill="${INK}" text-anchor="middle">RUN</text>
    </g>

    <!-- divider between editor and graph -->
    <line x1="940" y1="74" x2="940" y2="360" stroke="${PANEL_LINE}" stroke-width="1"/>

    <!-- editor pane -->
    ${editorSvg}

    <!-- graph pane -->
    ${edgeSvg}
    ${nodeSvg}
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
