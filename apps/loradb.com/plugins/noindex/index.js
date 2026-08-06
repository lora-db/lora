// Post-build rewriter that flips the <meta name="robots"> tag from the
// site-wide "index, follow, max-image-preview:large" default to
// "noindex,follow,max-image-preview:large" on a curated list of routes
// that are auto-generated, thin, or duplicative:
//
//   - /blog/page/N         paginated blog list (duplicates /blog)
//   - /blog/tags, /blog/tags/<tag>
//   - /blog/authors, /blog/authors/<author>
//   - /docs/tags, /docs/tags/<tag>
//
// These routes are already excluded from sitemap.xml; flipping the meta
// robots tag keeps them out of the index even when crawlers reach them
// through other paths (internal links, RSS readers, the navbar).
//
// The "follow" half is intentional: we want crawl budget spent on these
// pages to discover the linked-to posts and tags, just not on indexing
// the thin pages themselves.
//
// Runs in postBuild so it sees the final HTML after Docusaurus has
// rendered every route. Cheap: a few dozen files, a single regex
// replacement per file.

const fs = require("node:fs/promises");
const path = require("node:path");

// Each matcher is a function (relativeHtmlPath) => boolean. The path is
// posix-style, relative to outDir, and includes the .html suffix (or
// /index.html, depending on trailingSlash config).
const DEFAULT_MATCHERS = [
  (p) => p === "blog/tags.html" || p.startsWith("blog/tags/"),
  // The bare /blog/authors index is thin (just a list of names) —
  // noindex it. Per-author pages (/blog/authors/<slug>.html) carry a
  // bio + Person JSON-LD + post list, so they stay indexable.
  (p) => p === "blog/authors.html" || p === "blog/authors/index.html",
  (p) => p.startsWith("blog/page/"),
  // Per-author paginators: blog/authors/<slug>/page/2.html. Page 1 lives
  // at blog/authors/<slug>.html and stays indexable (bio + Person JSON-LD);
  // page 2+ is the same thin duplicate shape as /blog/page/N.
  (p) => /^blog\/authors\/[^/]+\/page\//.test(p),
  (p) => p === "docs/tags.html" || p.startsWith("docs/tags/"),
  (p) => p === "blog/tags/index.html",
];

// Matches any <meta ...> tag that has a name="robots" attribute, no
// matter where it sits in the attribute list (react-helmet emits
// data-rh="true" before name=, the static headTags from
// docusaurus.config.js emit name= first). The greedy negation on `[^>]*`
// is fine because <meta> tags do not allow nested `>`.
const ROBOTS_META_RE = /<meta\b[^>]*\bname=["']robots["'][^>]*>/gi;
const HEAD_OPEN_RE = /<head(\s[^>]*)?>/i;

module.exports = function noindexPlugin(_context, options = {}) {
  const matchers = options.matchers || DEFAULT_MATCHERS;
  const content = options.content || "noindex,follow,max-image-preview:large";

  return {
    name: "noindex-thin-pages",

    async postBuild({ outDir }) {
      const all = await fs.readdir(outDir, {
        recursive: true,
        withFileTypes: false,
      });
      const htmlFiles = all
        .filter((f) => typeof f === "string" && f.endsWith(".html"))
        .map((f) => f.split(path.sep).join("/"));

      const targets = htmlFiles.filter((f) => matchers.some((m) => m(f)));

      let edited = 0;
      let skipped = 0;
      for (const rel of targets) {
        const abs = path.join(outDir, rel);
        const html = await fs.readFile(abs, "utf8");

        const replacement = `<meta name="robots" content="${content}"/>`;
        let updated;
        // Replace all existing robots metas with one. Both the static
        // headTags-derived meta and the react-helmet meta carry "index"
        // by default; we want exactly one tag, with noindex.
        ROBOTS_META_RE.lastIndex = 0;
        if (ROBOTS_META_RE.test(html)) {
          ROBOTS_META_RE.lastIndex = 0;
          let first = true;
          updated = html.replace(ROBOTS_META_RE, () => {
            if (first) {
              first = false;
              return replacement;
            }
            return "";
          });
        } else if (HEAD_OPEN_RE.test(html)) {
          updated = html.replace(HEAD_OPEN_RE, (m) => `${m}${replacement}`);
        } else {
          skipped++;
          continue;
        }

        if (updated === html) {
          skipped++;
          continue;
        }
        await fs.writeFile(abs, updated, "utf8");
        edited++;
      }
      console.log(
        `[noindex] flipped robots meta on ${edited} thin page(s)${
          skipped ? ` (${skipped} skipped)` : ""
        }`,
      );
    },
  };
};
