// Maps docs source files to their site URLs and resolves the links inside
// a page to either another docs file or an absolute URL. Shared by the
// llms-txt plugin and scripts/bundle-package-docs.mjs.

const fs = require("node:fs");
const path = require("node:path");
const matter = require("gray-matter");

const SITE_DIR = path.resolve(__dirname, "../..");
const DOCS_DIR = path.join(SITE_DIR, "docs");
const DOCS_ROUTE = "/docs";

function walk(dir, base = "") {
  const out = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const rel = base ? `${base}/${entry.name}` : entry.name;
    if (entry.isDirectory()) out.push(...walk(path.join(dir, entry.name), rel));
    else if (/\.mdx?$/.test(entry.name)) out.push(rel);
  }
  return out;
}

// URL for a docs file, honouring `slug:` frontmatter the way Docusaurus
// does for the shapes this site uses.
function urlForDoc(rel, slug) {
  if (typeof slug === "string") {
    if (slug.startsWith("/")) {
      return (DOCS_ROUTE + (slug === "/" ? "" : slug)).replace(/\/$/, "");
    }
    const dir = path.posix.dirname(rel);
    return `${DOCS_ROUTE}/${dir === "." ? "" : dir + "/"}${slug}`;
  }
  const noExt = rel.replace(/\.mdx?$/, "");
  const trimmed = noExt.replace(/(^|\/)index$/, "");
  return trimmed ? `${DOCS_ROUTE}/${trimmed}` : DOCS_ROUTE;
}

let cached = null;

function loadDocsIndex() {
  if (cached) return cached;
  const byFile = new Map();
  const byUrl = new Map();
  for (const rel of walk(DOCS_DIR)) {
    const raw = fs.readFileSync(path.join(DOCS_DIR, rel), "utf8");
    const { data } = matter(raw);
    const url = urlForDoc(rel, data.slug);
    byFile.set(rel, url);
    byUrl.set(url, rel);
  }
  cached = { byFile, byUrl };
  return cached;
}

function splitHash(href) {
  const i = href.indexOf("#");
  return i === -1 ? [href, ""] : [href.slice(0, i), href.slice(i)];
}

function findDocFile(candidate, byFile) {
  const p = candidate.replace(/\/$/, "").replace(/\.mdx?$/, "");
  for (const option of [
    `${p}.md`,
    `${p}.mdx`,
    `${p}/index.md`,
    `${p}/index.mdx`,
  ]) {
    if (byFile.has(option)) return option;
  }
  return null;
}

/**
 * Resolve `href` as written in `sourceRel` (a path relative to the site
 * dir, e.g. "docs/queries/match.md").
 *
 * Returns one of:
 *   { kind: "external", href }           leave untouched
 *   { kind: "doc", file, url, hash }     another docs page (file relative to docs/)
 *   { kind: "site", url, hash }          a site path that is not a docs page
 */
function resolveDocHref(sourceRel, href, pageUrl) {
  if (/^[a-z][a-z0-9+.-]*:/i.test(href) || href.startsWith("#")) {
    return { kind: "external", href };
  }
  const { byFile, byUrl } = loadDocsIndex();
  const [target, hash] = splitHash(href);

  if (target.startsWith("/")) {
    const clean = target.replace(/\/$/, "") || "/";
    const file = byUrl.get(clean) || byUrl.get(clean.replace(/\.mdx?$/, ""));
    if (file) return { kind: "doc", file, url: byFile.get(file), hash };
    return { kind: "site", url: target, hash };
  }

  // Relative to the source file first (how authors think about it), then
  // relative to the page URL (how a browser resolves it).
  if (sourceRel.startsWith("docs/")) {
    const fromFile = path.posix.normalize(
      path.posix.join(
        path.posix.dirname(sourceRel.slice("docs/".length)),
        target,
      ),
    );
    const file = findDocFile(fromFile, byFile);
    if (file) return { kind: "doc", file, url: byFile.get(file), hash };
  }
  const base = new URL(pageUrl || "/", "https://site.invalid");
  const resolved = new URL(target, base).pathname.replace(/\/$/, "");
  const file = byUrl.get(resolved);
  if (file) return { kind: "doc", file, url: byFile.get(file), hash };
  return { kind: "site", url: resolved, hash };
}

module.exports = { DOCS_DIR, SITE_DIR, loadDocsIndex, resolveDocHref };
