#!/usr/bin/env node
//
// IndexNow ping — submits the URLs from build/sitemap.xml to api.indexnow.org
// so participating search engines (Bing, Yandex, Seznam, Naver) discover
// new and updated pages immediately instead of waiting for their own
// crawler to swing by.
//
// Usage:
//   node scripts/indexnow.mjs               # submit all URLs from sitemap
//   node scripts/indexnow.mjs --since=24h   # only URLs whose <lastmod> is within 24h
//   node scripts/indexnow.mjs --dry-run     # print what would be sent
//   node scripts/indexnow.mjs --sitemap=P   # read the sitemap from P
//
// Wired into the deploy job of .github/workflows/loradb-docs.yml, which
// runs it with --since after the Cloudflare Pages upload succeeds.
//
// Run after `yarn build` (and ideally after deploy, so the new pages are
// actually fetchable). The key file at static/<KEY>.txt must be deployed
// alongside the site for IndexNow to verify ownership; if it is missing,
// the API returns 403 and no URLs are accepted.
//
// IndexNow is fire-and-forget: a single POST per submission with up to
// 10,000 URLs in the body. No auth, no rate limit beyond the per-key
// "ownership verification" check.

import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const APP_ROOT = path.resolve(HERE, "..");

const HOST = "loradb.com";
const KEY = "83ba3712c7aa856e69601ce9b0470cb8";
const KEY_LOCATION = `https://${HOST}/${KEY}.txt`;
const ENDPOINT = "https://api.indexnow.org/IndexNow";
const DEFAULT_SITEMAP_PATH = path.join(APP_ROOT, "build", "sitemap.xml");

function parseArgs(argv) {
  const args = { dryRun: false, sinceMs: null, sitemap: DEFAULT_SITEMAP_PATH };
  for (const a of argv.slice(2)) {
    if (a === "--dry-run") args.dryRun = true;
    // The deploy job downloads the build artifact to a path of its own
    // choosing and never checks out apps/loradb.com/build, so it needs to
    // point the script at the sitemap explicitly.
    else if (a.startsWith("--sitemap=")) {
      args.sitemap = path.resolve(a.slice("--sitemap=".length));
    } else if (a.startsWith("--since=")) {
      const v = a.slice("--since=".length);
      const m = v.match(/^(\d+)(h|d)$/);
      if (!m) {
        console.error(`bad --since value: ${v} (expected e.g. 24h, 7d)`);
        process.exit(2);
      }
      const n = Number(m[1]);
      args.sinceMs = m[2] === "h" ? n * 3_600_000 : n * 86_400_000;
    } else {
      console.error(`unknown arg: ${a}`);
      process.exit(2);
    }
  }
  return args;
}

function extractEntries(xml) {
  const entries = [];
  const urlBlocks = xml.match(/<url>[\s\S]*?<\/url>/g) || [];
  for (const block of urlBlocks) {
    const loc = block.match(/<loc>([^<]+)<\/loc>/)?.[1];
    const lastmod = block.match(/<lastmod>([^<]+)<\/lastmod>/)?.[1];
    if (loc) entries.push({ loc, lastmod });
  }
  return entries;
}

async function main() {
  const { dryRun, sinceMs, sitemap: SITEMAP_PATH } = parseArgs(process.argv);

  let xml;
  try {
    xml = await readFile(SITEMAP_PATH, "utf8");
  } catch (err) {
    console.error(`could not read ${SITEMAP_PATH}: ${err.message}`);
    console.error("run `yarn build` first.");
    process.exit(1);
  }

  let entries = extractEntries(xml);
  if (entries.length === 0) {
    console.error("no <url> entries found in sitemap; nothing to submit.");
    process.exit(1);
  }

  if (sinceMs != null) {
    const cutoff = Date.now() - sinceMs;
    entries = entries.filter((e) => {
      if (!e.lastmod) return false;
      const t = Date.parse(e.lastmod);
      return Number.isFinite(t) && t >= cutoff;
    });
  }

  const urls = entries.map((e) => e.loc);

  if (urls.length === 0) {
    console.log("no URLs matched the filter; nothing to submit.");
    return;
  }

  if (urls.length > 10_000) {
    console.error(
      `IndexNow accepts at most 10,000 URLs per request (got ${urls.length}). Split the submission.`,
    );
    process.exit(2);
  }

  const payload = {
    host: HOST,
    key: KEY,
    keyLocation: KEY_LOCATION,
    urlList: urls,
  };

  if (dryRun) {
    console.log(`[dry-run] would POST ${urls.length} URL(s) to ${ENDPOINT}`);
    for (const u of urls) console.log(`  ${u}`);
    return;
  }

  const res = await fetch(ENDPOINT, {
    method: "POST",
    headers: { "Content-Type": "application/json; charset=utf-8" },
    body: JSON.stringify(payload),
  });

  const status = res.status;
  const body = await res.text();
  if (status === 200 || status === 202) {
    console.log(`IndexNow accepted ${urls.length} URL(s) (HTTP ${status}).`);
  } else {
    console.error(`IndexNow rejected submission: HTTP ${status}`);
    if (body) console.error(body);
    process.exit(1);
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
