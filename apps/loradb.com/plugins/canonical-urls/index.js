// Post-build rewriter that strips trailing slashes from same-site URLs in
// the emitted HTML, so every internal reference matches the canonical form.
//
// The site runs `trailingSlash: false`: /docs/queries is the canonical URL
// and /docs/queries/ 308-redirects onto it. Most of the site already emits
// the canonical form, but index docs (docs/index.md, docs/queries/index.md)
// have a permalink that keeps its trailing slash — "/docs/", "/docs/queries/"
// — and Docusaurus' own DocBreadcrumbs feeds that raw permalink into both
// the visible <a href> and the BreadcrumbList JSON-LD `item`.
//
// The result was a page whose <link rel="canonical"> said /docs/queries
// while its own structured data said /docs/queries/, plus breadcrumb links
// that cost every visitor and crawler a redirect hop.
//
// Why a postbuild pass instead of swizzling DocBreadcrumbs: swizzling means
// ejecting a whole theme component and re-vendoring it on every Docusaurus
// upgrade, to fix two strings. Normalising the output is smaller, has no
// upgrade surface, and catches any future component that emits a
// trailing-slash self-reference.
//
// Deliberately NOT rewritten:
//   - href="/"                     the site root; "/" is its canonical form
//   - https://loradb.com/          same, as an absolute URL
//   - anything containing a dot in the final segment (asset paths)
//   - other origins — both patterns are anchored to this site

const fs = require("node:fs/promises");
const path = require("node:path");

const SITE_URL = "https://loradb.com";

// href="/a/b/" -> href="/a/b". Requires at least one path segment and no
// dot anywhere in the path, which keeps asset URLs untouched.
const ROOT_RELATIVE_RE = /href="(\/[^"?#.]*[^"/?#.])\/"/g;

// "https://loradb.com/a/b/" -> "https://loradb.com/a/b". Quoted on both
// sides so it only ever matches a complete attribute or JSON string value,
// never a substring of prose.
const ABSOLUTE_RE = new RegExp(
  `"${SITE_URL}(/[^"?#.]*[^"/?#.])/"`,
  "g",
);

module.exports = function canonicalUrlsPlugin() {
  return {
    name: "canonical-urls",

    async postBuild({ outDir }) {
      const all = await fs.readdir(outDir, {
        recursive: true,
        withFileTypes: false,
      });
      const htmlFiles = all
        .filter((f) => typeof f === "string" && f.endsWith(".html"))
        .map((f) => f.split(path.sep).join("/"));

      let editedFiles = 0;
      let rewrites = 0;

      for (const rel of htmlFiles) {
        const abs = path.join(outDir, rel);
        const html = await fs.readFile(abs, "utf8");

        let count = 0;
        const updated = html
          .replace(ROOT_RELATIVE_RE, (_m, p) => {
            count++;
            return `href="${p}"`;
          })
          .replace(ABSOLUTE_RE, (_m, p) => {
            count++;
            return `"${SITE_URL}${p}"`;
          });

        if (count > 0) {
          await fs.writeFile(abs, updated, "utf8");
          editedFiles++;
          rewrites += count;
        }
      }

      console.log(
        `[canonical-urls] stripped ${rewrites} trailing slash(es) across ${editedFiles} page(s)`,
      );
    },
  };
};
