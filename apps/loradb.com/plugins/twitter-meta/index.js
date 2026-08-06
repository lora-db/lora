// Post-build rewriter that mirrors per-page og:* tags into twitter:* tags
// so Twitter/X link previews show the actual page title, description, and
// image instead of a single sitewide fallback.
//
// Docusaurus's <Layout title description> (and the equivalent doc/blog
// page metadata) only auto-writes <title>, meta description, og:title,
// and og:description per page. It does NOT auto-derive twitter:title /
// twitter:description — so any global twitter:* set in themeConfig.metadata
// pins every page to the homepage copy. Removing the global tags lets
// Twitter fall back to og:* but means twitter:* is missing entirely.
//
// This plugin closes the gap: it reads the per-page og:title /
// og:description / og:image / og:image:alt that Docusaurus already
// emits correctly, and writes matching twitter:title / twitter:description /
// twitter:image / twitter:image:alt tags before </head> on every built
// .html file.
//
// If a page already emits a twitter:* tag (e.g. PageSEO set one
// explicitly), the existing tag wins — this plugin only fills gaps.

const fs = require("node:fs/promises");
const path = require("node:path");

const HEAD_END_RE = /<\/head>/i;

function escapeAttr(value) {
  return value
    .replace(/&/g, "&amp;")
    .replace(/"/g, "&quot;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

// Decode the HTML entities react-helmet writes into meta content
// attributes so we can re-emit a clean value for twitter:* tags. The
// re-encode step (escapeAttr) then re-escapes for the new attribute.
function decodeEntities(value) {
  return value
    .replace(/&quot;/g, '"')
    .replace(/&#x27;/g, "'")
    .replace(/&#39;/g, "'")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&amp;/g, "&");
}

function readMeta(html, attrName, attrValue) {
  // Match a <meta> tag where one attribute (name|property) equals attrValue
  // and capture the content attribute. Tolerates attribute order — Docusaurus
  // emits data-rh="true" before name/property, while the static config-derived
  // headTags emit name/property first.
  const re = new RegExp(
    `<meta\\b[^>]*\\b${attrName}=["']${attrValue}["'][^>]*\\bcontent=["']([^"']*)["'][^>]*>`,
    "i",
  );
  const m = html.match(re);
  if (m) return decodeEntities(m[1]);

  // content before name/property
  const re2 = new RegExp(
    `<meta\\b[^>]*\\bcontent=["']([^"']*)["'][^>]*\\b${attrName}=["']${attrValue}["'][^>]*>`,
    "i",
  );
  const m2 = html.match(re2);
  if (m2) return decodeEntities(m2[1]);

  return null;
}

function hasMeta(html, attrName, attrValue) {
  const re = new RegExp(
    `<meta\\b[^>]*\\b${attrName}=["']${attrValue}["']`,
    "i",
  );
  return re.test(html);
}

module.exports = function twitterMetaPlugin() {
  return {
    name: "twitter-meta",

    async postBuild({ outDir }) {
      const all = await fs.readdir(outDir, {
        recursive: true,
        withFileTypes: false,
      });
      const htmlFiles = all
        .filter((f) => typeof f === "string" && f.endsWith(".html"))
        .map((f) => f.split(path.sep).join("/"));

      let edited = 0;
      for (const rel of htmlFiles) {
        const abs = path.join(outDir, rel);
        const html = await fs.readFile(abs, "utf8");

        const ogTitle = readMeta(html, "property", "og:title");
        const ogDescription = readMeta(html, "property", "og:description");
        const ogImage = readMeta(html, "property", "og:image");
        const ogImageAlt = readMeta(html, "property", "og:image:alt");

        const additions = [];
        if (ogTitle && !hasMeta(html, "name", "twitter:title")) {
          additions.push(
            `<meta name="twitter:title" content="${escapeAttr(ogTitle)}"/>`,
          );
        }
        if (ogDescription && !hasMeta(html, "name", "twitter:description")) {
          additions.push(
            `<meta name="twitter:description" content="${escapeAttr(ogDescription)}"/>`,
          );
        }
        if (ogImage && !hasMeta(html, "name", "twitter:image")) {
          additions.push(
            `<meta name="twitter:image" content="${escapeAttr(ogImage)}"/>`,
          );
        }
        if (ogImageAlt && !hasMeta(html, "name", "twitter:image:alt")) {
          additions.push(
            `<meta name="twitter:image:alt" content="${escapeAttr(ogImageAlt)}"/>`,
          );
        }

        if (additions.length === 0) continue;
        if (!HEAD_END_RE.test(html)) continue;

        const updated = html.replace(HEAD_END_RE, `${additions.join("")}</head>`);
        await fs.writeFile(abs, updated, "utf8");
        edited++;
      }

      console.log(
        `[twitter-meta] mirrored og:* into twitter:* on ${edited} page(s)`,
      );
    },
  };
};
