// Curated manifest for llms.txt + llms-full.txt.
//
// Each entry pairs a content source file (markdown on disk, relative to
// the Docusaurus site root) with the URL it will be served from. Pages
// listed here are exposed to AI crawlers as the "canonical" surface of
// the project; pages NOT listed are still indexed by search engines via
// sitemap.xml, but are not pulled into the llms-full.txt bundle.
//
// Two rules of thumb when editing:
//
//   1. Only canonical pages. Every binding's getting-started page belongs
//      here. Most release-note blog posts do not — pick a few that are
//      genuinely standalone essays.
//   2. Keep order intentional. Crawlers and humans read top-to-bottom.

const docEntry = (slug, urlOverride) => ({
  source: `docs/${slug}.md`,
  url: urlOverride || `/docs/${slug}`,
});

const blogEntry = (dir, slug) => ({
  source: `blog/${dir}/index.md`,
  url: `/blog/${slug}`,
});

module.exports = {
  intro: [
    "LoraDB is a local-first, in-memory property-graph engine written in",
    "Rust. It speaks a pragmatic subset of Cypher, embeds directly inside a",
    "host process via a Rust crate or one of five language bindings",
    "(Node.js, Python, WASM, Go, Ruby), and exposes an HTTP server for",
    "language-agnostic access. Snapshots ship on every binding; WAL-backed",
    "durability is available on every filesystem-backed surface.",
    "",
    "The pages below are the canonical reference for the engine, its query",
    "and data-type surface, every binding's installation and usage path,",
    "and the engineering rationale behind the project.",
  ].join("\n"),

  sections: [
    {
      title: "Overview",
      entries: [
        { ...docEntry("index", "/docs"), title: "What is LoraDB" },
        { ...docEntry("why"), title: "Why LoraDB" },
      ],
    },
    {
      title: "Getting started",
      intro:
        "Install paths and minimal working examples for every supported runtime.",
      entries: [
        docEntry("getting-started/installation"),
        docEntry("getting-started/playground"),
        docEntry("getting-started/tutorial"),
        docEntry("getting-started/node"),
        docEntry("getting-started/python"),
        docEntry("getting-started/wasm"),
        docEntry("getting-started/go"),
        docEntry("getting-started/ruby"),
        docEntry("getting-started/rust"),
        docEntry("getting-started/server"),
      ],
    },
    {
      title: "Concepts",
      entries: [
        docEntry("concepts/graph-model"),
        docEntry("concepts/nodes"),
        docEntry("concepts/relationships"),
        docEntry("concepts/properties"),
        docEntry("concepts/schema-free"),
        docEntry("concepts/result-formats"),
      ],
    },
    {
      title: "Queries",
      intro:
        "The supported subset of Cypher, clause by clause, plus the cheat sheet.",
      entries: [
        {
          ...docEntry("queries/index", "/docs/queries"),
          title: "Queries overview",
        },
        docEntry("queries/match"),
        docEntry("queries/create"),
        docEntry("queries/where"),
        docEntry("queries/return-with"),
        docEntry("queries/set-delete"),
        docEntry("queries/unwind-merge"),
        docEntry("queries/aggregation"),
        docEntry("queries/ordering"),
        docEntry("queries/paths"),
        docEntry("queries/parameters"),
        docEntry("queries/indexes"),
        docEntry("queries/constraints"),
        docEntry("queries/examples"),
        docEntry("queries/cheat-sheet"),
      ],
    },
    {
      title: "Functions",
      entries: [
        docEntry("functions/overview"),
        docEntry("functions/aggregation"),
        docEntry("functions/string"),
        docEntry("functions/math"),
        docEntry("functions/number"),
        docEntry("functions/list"),
        docEntry("functions/map"),
        docEntry("functions/temporal"),
        docEntry("functions/spatial"),
        docEntry("functions/vectors"),
        docEntry("functions/utility"),
      ],
    },
    {
      title: "Data types",
      entries: [
        docEntry("data-types/overview"),
        docEntry("data-types/scalars"),
        docEntry("data-types/lists-and-maps"),
        docEntry("data-types/temporal"),
        docEntry("data-types/spatial"),
        docEntry("data-types/vectors"),
      ],
    },
    {
      title: "API",
      entries: [{ ...docEntry("api/http"), title: "HTTP API" }],
    },
    {
      title: "Guides and reference",
      entries: [
        docEntry("cookbook"),
        docEntry("snapshot"),
        docEntry("wal"),
        docEntry("performance"),
        docEntry("errors"),
        docEntry("limitations"),
        docEntry("troubleshooting"),
      ],
    },
    {
      title: "Engineering essays",
      intro:
        "Hand-picked posts explaining why the engine is shaped the way it is.",
      entries: [
        blogEntry("2026-04-05-why-i-started-loradb", "why-i-started-loradb"),
        blogEntry(
          "2026-04-08-in-memory-or-it-does-not-work",
          "in-memory-or-it-does-not-work",
        ),
        blogEntry(
          "2026-04-12-efficient-storage-is-the-product",
          "efficient-storage-is-the-product",
        ),
        blogEntry(
          "2026-04-22-vectors-belong-next-to-relationships",
          "vectors-belong-next-to-relationships",
        ),
        blogEntry(
          "2026-04-25-snapshots-before-a-log",
          "snapshots-before-a-log",
        ),
        blogEntry(
          "2026-05-20-where-sql-quietly-degrades",
          "where-sql-quietly-degrades",
        ),
      ],
    },
  ],
};
