#!/usr/bin/env node
/**
 * Build-time Open Graph image generator for the LoraDB Playground.
 *
 * Renders a 1200x630 PNG that pairs the favicon mark with the brand
 * lockup ("LoraDB Playground" / "In-browser Cypher IDE") on a dark
 * canvas. The image is composed entirely as an inline SVG so the
 * script has zero runtime font-loading concerns — we lean on
 * librsvg's built-in `system-ui` fallback (which sharp invokes under
 * the hood) for text rasterization.
 *
 * If sharp can't be resolved from the workspace, we degrade
 * gracefully: emit a 1x1 transparent PNG so the build still
 * succeeds and log a clear warning describing the fallback. This
 * keeps the playground deployable from environments that strip
 * Next.js's optional native deps (e.g. lightweight Docker images).
 *
 * Idempotent: the output bytes are deterministic, so running the
 * script twice in a row produces identical files. Sharp's metadata
 * is explicitly stripped to avoid embedding timestamps.
 */

import { writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const OUT_PATH = resolve(__dirname, "..", "public", "og-image.png");

/** 1x1 transparent PNG (89 bytes) — emitted when sharp is unreachable. */
const TRANSPARENT_PNG_1X1 = Buffer.from(
  "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4" +
    "890000000d49444154789c6300010000000500010d0a2db40000000049454e44ae426082",
  "hex",
);

/**
 * Attempt to resolve sharp. We try the workspace install first
 * (Next.js bundles it as an optional dep for image optimization)
 * and fall back to a plain dynamic import which uses Node's normal
 * module resolution.
 */
async function loadSharp() {
  try {
    const mod = await import("sharp");
    return mod.default ?? mod;
  } catch (err) {
    console.warn(
      "[og] sharp unavailable — falling back to 1x1 transparent placeholder.",
    );
    if (err instanceof Error) {
      console.warn(`[og] reason: ${err.message}`);
    }
    return null;
  }
}

/**
 * Compose the OG image as a single SVG string. Mirrors the colours
 * of the live favicon (`public/favicon.svg`) and our editor theme
 * (`bg.editor = #1e1e1e`).
 */
function buildSvg() {
  const w = 1200;
  const h = 630;
  // Mark sizing — scale the 32-unit favicon viewport up to 280px.
  const markSize = 280;
  const markX = 110;
  const markY = (h - markSize) / 2;
  const markScale = markSize / 32;

  // Text origin sits to the right of the mark with a gap.
  const textX = markX + markSize + 60;
  const titleY = h / 2 - 10;
  const subtitleY = h / 2 + 50;

  return `<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">
  <rect width="${w}" height="${h}" fill="#1e1e1e"/>

  <g transform="translate(${markX}, ${markY}) scale(${markScale})">
    <rect x="2" y="2" width="28" height="28" rx="6" fill="#0e639c"/>
    <path d="M11 11 L16 22 L21 11" stroke="#ffffff" stroke-width="1.5" fill="none" opacity="0.6"/>
    <circle cx="11" cy="11" r="3" fill="#ffffff"/>
    <circle cx="21" cy="11" r="3" fill="#ffffff"/>
    <circle cx="16" cy="22" r="3" fill="#ffffff"/>
  </g>

  <text x="${textX}" y="${titleY}"
    font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif"
    font-size="60" font-weight="700" fill="#ffffff">LoraDB Playground</text>
  <text x="${textX}" y="${subtitleY}"
    font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif"
    font-size="32" font-weight="400" fill="#a0a0a0">In-browser Cypher IDE</text>

  <g font-family="system-ui, -apple-system, Segoe UI, Roboto, sans-serif"
     font-size="22" font-weight="600" fill="#ffffff">
    <rect x="${textX}" y="${h - 100}" width="110" height="40" rx="6" fill="#0e639c"/>
    <text x="${textX + 55}" y="${h - 73}" text-anchor="middle">Cypher</text>

    <rect x="${textX + 130}" y="${h - 100}" width="110" height="40" rx="6" fill="#0e639c"/>
    <text x="${textX + 185}" y="${h - 73}" text-anchor="middle">WASM</text>

    <rect x="${textX + 260}" y="${h - 100}" width="110" height="40" rx="6" fill="#0e639c"/>
    <text x="${textX + 315}" y="${h - 73}" text-anchor="middle">Graph</text>
  </g>
</svg>`;
}

/**
 * Format a byte count as "NN.N KB" for the success log line.
 */
function formatBytes(n) {
  return `${(n / 1024).toFixed(1)} KB`;
}

async function main() {
  const sharp = await loadSharp();

  if (sharp === null) {
    await writeFile(OUT_PATH, TRANSPARENT_PNG_1X1);
    console.warn(
      `[og] wrote placeholder ${OUT_PATH} (${formatBytes(TRANSPARENT_PNG_1X1.length)})`,
    );
    return;
  }

  const svg = buildSvg();
  const buffer = await sharp(Buffer.from(svg))
    .resize(1200, 630)
    .png({ compressionLevel: 9 })
    .withMetadata({})
    .toBuffer();

  await writeFile(OUT_PATH, buffer);
  console.log(`[og] wrote public/og-image.png (${formatBytes(buffer.length)})`);
}

main().catch((err) => {
  console.error("[og] failed:", err);
  // Last-ditch fallback: still emit the placeholder so a downstream
  // `next build` doesn't 404 on the reference in the metadata.
  writeFile(OUT_PATH, TRANSPARENT_PNG_1X1)
    .then(() => {
      console.warn("[og] wrote 1x1 placeholder after error");
    })
    .catch(() => {
      // Nothing else we can do — surface a non-zero exit if even the
      // placeholder write fails so CI flags the issue.
      process.exit(1);
    });
});
