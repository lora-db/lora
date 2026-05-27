// Build-time emitter for /llms.txt and /llms-full.txt.
//
// llms.txt is a curated index (per the llmstxt.org convention) that points
// AI clients — ChatGPT, Claude, Perplexity, Google AI Overviews — at the
// load-bearing pages of the site: what LoraDB is, how to get started in
// each binding, the query and data-type reference, and a hand-picked set
// of engineering essays. Crawlers use it as a shortcut to canonical
// content, so we keep it tight rather than dumping the entire sitemap.
//
// llms-full.txt concatenates the raw markdown body of those same pages
// (frontmatter stripped) so a single fetch carries the full canonical
// surface. Both files live at the site root next to robots.txt.
//
// Curation is declared in plugin options (see plugins/llms-txt/manifest.js).
// Pages not listed there are still indexed by search engines via the
// regular sitemap; they are just not surfaced to LLM crawlers as
// "canonical" entry points.

const fs = require("node:fs/promises");
const path = require("node:path");
const matter = require("gray-matter");

module.exports = function llmsTxtPlugin(context, options = {}) {
  const { siteDir, siteConfig } = context;
  const siteUrl = siteConfig.url.replace(/\/$/, "");
  const siteTitle = options.title || siteConfig.title;
  const siteDescription = options.description || siteConfig.tagline || "";

  return {
    name: "llms-txt",

    async postBuild({ outDir }) {
      const sections = options.sections || [];
      if (sections.length === 0) {
        console.warn("[llms-txt] no sections configured; skipping");
        return;
      }

      const resolved = await Promise.all(
        sections.map(async (section) => ({
          title: section.title,
          intro: section.intro || null,
          entries: await Promise.all(
            (section.entries || []).map((entry) => loadEntry(siteDir, entry)),
          ),
        })),
      );

      const indexBody = renderIndex({
        title: siteTitle,
        description: siteDescription,
        intro: options.intro || null,
        sections: resolved,
        siteUrl,
      });
      await fs.writeFile(path.join(outDir, "llms.txt"), indexBody, "utf8");

      const fullBody = renderFull({
        title: siteTitle,
        description: siteDescription,
        intro: options.intro || null,
        sections: resolved,
        siteUrl,
      });
      await fs.writeFile(path.join(outDir, "llms-full.txt"), fullBody, "utf8");

      const entryCount = resolved.reduce((n, s) => n + s.entries.length, 0);
      console.log(
        `[llms-txt] wrote llms.txt (${entryCount} entries) and llms-full.txt (${fullBody.length.toLocaleString()} chars)`,
      );
    },
  };
};

async function loadEntry(siteDir, entry) {
  if (!entry.source || !entry.url) {
    throw new Error(
      `[llms-txt] manifest entry missing source or url: ${JSON.stringify(entry)}`,
    );
  }
  const absPath = path.join(siteDir, entry.source);
  let raw;
  try {
    raw = await fs.readFile(absPath, "utf8");
  } catch (err) {
    throw new Error(
      `[llms-txt] could not read ${entry.source}: ${err.message}`,
    );
  }
  const { data, content } = matter(raw);
  const title =
    entry.title || data.title || data.sidebar_label || basename(entry.source);
  const description = entry.description || data.description || null;
  return {
    title,
    description,
    url: entry.url,
    body: content.trim(),
  };
}

function renderIndex({ title, description, intro, sections, siteUrl }) {
  const lines = [];
  lines.push(`# ${title}`);
  lines.push("");
  if (description) {
    lines.push(`> ${description}`);
    lines.push("");
  }
  if (intro) {
    lines.push(intro.trim());
    lines.push("");
  }
  for (const section of sections) {
    lines.push(`## ${section.title}`);
    lines.push("");
    if (section.intro) {
      lines.push(section.intro.trim());
      lines.push("");
    }
    for (const entry of section.entries) {
      const url = `${siteUrl}${entry.url}`;
      const desc = entry.description ? `: ${entry.description}` : "";
      lines.push(`- [${entry.title}](${url})${desc}`);
    }
    lines.push("");
  }
  return lines.join("\n").replace(/\n+$/, "\n");
}

function renderFull({ title, description, intro, sections, siteUrl }) {
  const lines = [];
  lines.push(`# ${title}`);
  lines.push("");
  if (description) {
    lines.push(`> ${description}`);
    lines.push("");
  }
  lines.push(`Canonical bundle: ${siteUrl}/llms-full.txt`);
  lines.push(`Index: ${siteUrl}/llms.txt`);
  lines.push("");
  if (intro) {
    lines.push(intro.trim());
    lines.push("");
  }
  for (const section of sections) {
    for (const entry of section.entries) {
      lines.push("---");
      lines.push("");
      lines.push(`# ${entry.title}`);
      lines.push("");
      lines.push(`URL: ${siteUrl}${entry.url}`);
      if (entry.description) {
        lines.push("");
        lines.push(`> ${entry.description}`);
      }
      lines.push("");
      lines.push(entry.body);
      lines.push("");
    }
  }
  return lines.join("\n").replace(/\n+$/, "\n");
}

function basename(p) {
  return path.basename(p, path.extname(p));
}
