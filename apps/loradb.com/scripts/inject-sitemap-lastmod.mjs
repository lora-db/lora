#!/usr/bin/env node
//
// Postbuild sitemap rewriter — adds <lastmod> to every <url> in
// build/sitemap.xml using the most recent git commit timestamp touching
// the source file behind the route (with a stat-mtime fallback for
// uncommitted files, and the build timestamp for synthetic routes).
//
// Docusaurus' built-in sitemap plugin does not emit <lastmod>. Crawlers
// use it as a freshness signal; without it the entire sitemap looks
// equally stale every fetch. This also makes the --since filter on
// scripts/indexnow.mjs actually do something.
//
// Why a postbuild script and not a Docusaurus plugin: the sitemap is
// emitted in the sitemap plugin's postBuild hook, and Docusaurus runs
// postBuild hooks in parallel. A second postBuild plugin would race
// the sitemap writer. Running this script after `docusaurus build`
// completes sidesteps the race.
//
// Route → source resolution:
//   /                     → src/pages/index.{jsx,tsx,md}
//   /docs                 → docs/index.md
//   /docs/<a>/<b>         → docs/<a>/<b>.md, then docs/<a>/<b>/index.md
//   /blog                 → newest mtime under blog/
//   /blog/<slug>          → blog/<dir>/index.md (matched by frontmatter slug)
//   /<name>               → src/pages/<name>.{jsx,tsx,md,mdx}

import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";
import matter from "gray-matter";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const APP_ROOT = path.resolve(HERE, "..");
const SITE_URL = "https://loradb.com";
const SITEMAP = path.join(APP_ROOT, "build", "sitemap.xml");

function main() {
  if (!existsSync(SITEMAP)) {
    console.error(`[sitemap-lastmod] ${SITEMAP} not found; run yarn build first`);
    process.exit(1);
  }

  const xml = readFileSync(SITEMAP, "utf8");
  const blogSlugMap = buildBlogSlugMap();
  const buildTimestamp = isoNow();
  let edited = 0;
  let withGit = 0;
  let fallback = 0;

  const updated = xml.replace(/<url>([\s\S]*?)<\/url>/g, (block) => {
    if (/<lastmod>/.test(block)) return block;
    const locMatch = block.match(/<loc>([^<]+)<\/loc>/);
    if (!locMatch) return block;

    const route = urlToRoute(locMatch[1]);
    const source = routeToSource(route, blogSlugMap);
    let ts = null;
    if (source) ts = gitLastModified(source);
    if (ts) withGit++;
    if (!ts && source) {
      try {
        ts = isoFromMs(statSync(source).mtimeMs);
      } catch {
        // ignore
      }
    }
    if (!ts) {
      ts = buildTimestamp;
      fallback++;
    }
    edited++;
    return block.replace(
      /<loc>[^<]+<\/loc>/,
      (loc) => `${loc}<lastmod>${ts}</lastmod>`,
    );
  });

  writeFileSync(SITEMAP, updated, "utf8");
  console.log(
    `[sitemap-lastmod] tagged ${edited} URL(s) (${withGit} from git, ${edited - withGit - fallback} from mtime, ${fallback} fallback)`,
  );
}

function urlToRoute(url) {
  const u = url.replace(SITE_URL, "");
  return u.replace(/\/+$/, "") || "/";
}

function buildBlogSlugMap() {
  const blogDir = path.join(APP_ROOT, "blog");
  const map = new Map();
  let entries;
  try {
    entries = readdirSync(blogDir, { withFileTypes: true });
  } catch {
    return map;
  }
  for (const entry of entries) {
    if (!entry.isDirectory()) continue;
    const mdPath = path.join(blogDir, entry.name, "index.md");
    if (!existsSync(mdPath)) continue;
    const raw = readFileSync(mdPath, "utf8");
    const { data } = matter(raw);
    const slug = data.slug || entry.name.replace(/^\d{4}-\d{2}-\d{2}-/, "");
    map.set(slug, mdPath);
  }
  return map;
}

function routeToSource(route, blogSlugMap) {
  if (route === "/") {
    return firstExisting([
      "src/pages/index.jsx",
      "src/pages/index.tsx",
      "src/pages/index.md",
    ]);
  }
  if (route === "/docs") {
    return path.join(APP_ROOT, "docs", "index.md");
  }
  if (route.startsWith("/docs/")) {
    const rel = route.slice("/docs/".length);
    return firstExisting([
      `docs/${rel}.md`,
      `docs/${rel}/index.md`,
      `docs/${rel}.mdx`,
    ]);
  }
  if (route === "/blog") {
    return newestBlogSource();
  }
  if (route.startsWith("/blog/")) {
    const slug = route.slice("/blog/".length).split("/")[0];
    return blogSlugMap.get(slug) || null;
  }
  const name = route.replace(/^\/+/, "");
  return firstExisting([
    `src/pages/${name}.jsx`,
    `src/pages/${name}.tsx`,
    `src/pages/${name}.md`,
    `src/pages/${name}.mdx`,
  ]);
}

function firstExisting(rels) {
  for (const rel of rels) {
    const abs = path.join(APP_ROOT, rel);
    if (existsSync(abs)) return abs;
  }
  return null;
}

function newestBlogSource() {
  const blogDir = path.join(APP_ROOT, "blog");
  let entries;
  try {
    entries = readdirSync(blogDir, { withFileTypes: true });
  } catch {
    return null;
  }
  let best = null;
  let bestTs = -Infinity;
  for (const e of entries) {
    if (!e.isDirectory()) continue;
    const p = path.join(blogDir, e.name, "index.md");
    if (!existsSync(p)) continue;
    const ts = statSync(p).mtimeMs;
    if (ts > bestTs) {
      bestTs = ts;
      best = p;
    }
  }
  return best;
}

const gitCache = new Map();

function gitLastModified(absPath) {
  if (gitCache.has(absPath)) return gitCache.get(absPath);
  let ts = null;
  try {
    const out = execFileSync(
      "git",
      ["log", "-1", "--format=%cI", "--", absPath],
      { cwd: APP_ROOT, encoding: "utf8" },
    ).trim();
    if (out) ts = normalizeTimestamp(out);
  } catch {
    // file not tracked, or git unavailable
  }
  gitCache.set(absPath, ts);
  return ts;
}

function normalizeTimestamp(iso) {
  // Convert "2026-05-21T13:04:11+02:00" → keep tz, normalize spacing.
  return iso.replace(/\s+/g, "");
}

function isoNow() {
  return new Date().toISOString().replace(/\.\d+Z$/, "+00:00");
}

function isoFromMs(ms) {
  return new Date(ms).toISOString().replace(/\.\d+Z$/, "+00:00");
}

main();
