const path = require("path");
const llmsTxtManifest = require("./plugins/llms-txt/manifest");

const SITE_URL = "https://loradb.com";
const SITE_DESCRIPTION =
  "LoraDB is a local-first, in-memory property-graph engine written in Rust that speaks a pragmatic subset of Cypher — built for AI agents, robotics, and context-rich systems that reason over connected data.";

// Sitemap priority + changefreq tiers. Higher priority signals to crawlers
// that a URL is closer to the canonical surface of the site relative to
// other URLs on the same domain (it is not a global rank signal). The
// homepage and the docs/blog landings sit at the top; reference docs and
// engineering essays follow; deep API/function reference and individual
// release notes sit lower. Anything not matched falls through to the
// preset defaults (0.5 / monthly).
function tierForUrl(url) {
  const p = new URL(url).pathname.replace(/\/+$/, "") || "/";

  if (p === "/") return { priority: 1.0, changefreq: "daily" };

  if (p === "/docs" || p === "/blog") {
    return { priority: 0.9, changefreq: "daily" };
  }

  if (p === "/docs/why") {
    return { priority: 0.9, changefreq: "weekly" };
  }

  if (p.startsWith("/docs/getting-started/")) {
    return { priority: 0.8, changefreq: "weekly" };
  }

  if (p === "/docs/cookbook" || p === "/docs/performance") {
    return { priority: 0.8, changefreq: "monthly" };
  }

  if (p.startsWith("/docs/concepts/")) {
    return { priority: 0.7, changefreq: "monthly" };
  }

  if (
    p.startsWith("/docs/queries/") ||
    p.startsWith("/docs/functions/") ||
    p.startsWith("/docs/data-types/") ||
    p.startsWith("/docs/api/")
  ) {
    return { priority: 0.6, changefreq: "monthly" };
  }

  if (p.startsWith("/docs/")) {
    return { priority: 0.7, changefreq: "monthly" };
  }

  if (p.startsWith("/blog/")) {
    return { priority: 0.6, changefreq: "monthly" };
  }

  if (p === "/features" || p === "/contact") {
    return { priority: 0.6, changefreq: "monthly" };
  }

  // Benchmarks had no branch here, so all seven URLs fell through to the
  // 0.4 catch-all — the lowest tier on the site. They are the opposite of
  // low value: /benchmarks/lora-vs-<engine> are the highest-intent pages
  // we publish (someone searching "neo4j alternative" or "kuzu vs" is
  // already comparing engines), and they are backed by a measured harness
  // rather than marketing copy. The index gets landing-page weight; the
  // individual comparisons sit alongside pillar docs.
  if (p === "/benchmarks") {
    return { priority: 0.8, changefreq: "monthly" };
  }

  if (p.startsWith("/benchmarks/")) {
    return { priority: 0.7, changefreq: "monthly" };
  }

  return { priority: 0.4, changefreq: "monthly" };
}

module.exports = {
  title: "LoraDB",
  tagline: "The embedded graph database for connected systems.",
  url: SITE_URL,
  baseUrl: "/",
  // Canonical URLs have no trailing slash. Locks the form Docusaurus
  // writes into <link rel="canonical">, the sitemap, and internal links
  // so search engines see one URL per page instead of /foo and /foo/.
  trailingSlash: false,
  onBrokenLinks: "throw",
  onBrokenAnchors: "throw",
  markdown: {
    hooks: {
      onBrokenMarkdownLinks: "throw",
    },
  },
  favicon: "img/meta/favicon.ico",
  organizationName: "loradb",
  projectName: "lora",
  headTags: [
    // Network hints for first-paint performance. Umami serves the
    // analytics script; github.com is hit for the repo-stars fetch
    // fallback. dns-prefetch is the cheap one (DNS only); preconnect
    // opens the TCP + TLS handshake too — keep that for hosts we are
    // certain to talk to on every page.
    {
      tagName: "link",
      attributes: {
        rel: "preconnect",
        href: "https://cloud.umami.is",
        crossorigin: "anonymous",
      },
    },
    {
      tagName: "link",
      attributes: { rel: "dns-prefetch", href: "https://api.github.com" },
    },
    // OpenSearch description — lets browsers offer "Add LoraDB" to their
    // built-in search bar. Tiny win, costs nothing to keep current.
    {
      tagName: "link",
      attributes: {
        rel: "search",
        type: "application/opensearchdescription+xml",
        title: "LoraDB",
        href: "/opensearch.xml",
      },
    },
    // Search engine verification meta tags. Fill in the verification
    // strings issued by Google Search Console / Bing Webmaster Tools
    // when the property is added. Leaving the placeholders commented
    // means the build does not advertise a half-configured verification.
    //
    // {
    //   tagName: 'meta',
    //   attributes: { name: 'google-site-verification', content: 'REPLACE_WITH_GSC_TOKEN' },
    // },
    // {
    //   tagName: 'meta',
    //   attributes: { name: 'msvalidate.01', content: 'REPLACE_WITH_BING_TOKEN' },
    // },
    // Full favicon set — ordered so browsers pick the right one.
    {
      tagName: "link",
      attributes: {
        rel: "icon",
        type: "image/x-icon",
        href: "/img/meta/favicon.ico",
      },
    },
    {
      tagName: "link",
      attributes: {
        rel: "icon",
        type: "image/svg+xml",
        href: "/img/meta/favicon.svg",
      },
    },
    {
      tagName: "link",
      attributes: {
        rel: "icon",
        type: "image/png",
        sizes: "16x16",
        href: "/img/meta/favicon-16x16.png",
      },
    },
    {
      tagName: "link",
      attributes: {
        rel: "icon",
        type: "image/png",
        sizes: "32x32",
        href: "/img/meta/favicon-32x32.png",
      },
    },
    {
      tagName: "link",
      attributes: {
        rel: "icon",
        type: "image/png",
        sizes: "48x48",
        href: "/img/meta/favicon-48x48.png",
      },
    },
    {
      tagName: "link",
      attributes: {
        rel: "icon",
        type: "image/png",
        sizes: "96x96",
        href: "/img/meta/favicon-96x96.png",
      },
    },
    {
      tagName: "link",
      attributes: {
        rel: "apple-touch-icon",
        sizes: "180x180",
        href: "/img/meta/apple-touch-icon.png",
      },
    },
    // PWA manifest + theme colors
    {
      tagName: "link",
      attributes: { rel: "manifest", href: "/site.webmanifest" },
    },
    {
      tagName: "meta",
      attributes: { name: "theme-color", content: "#1b6dff" },
    },
    {
      tagName: "meta",
      attributes: { name: "apple-mobile-web-app-title", content: "LoraDB" },
    },
    {
      tagName: "meta",
      attributes: { name: "application-name", content: "LoraDB" },
    },
    {
      tagName: "meta",
      attributes: { name: "msapplication-TileColor", content: "#0a0f1f" },
    },
    // JSON-LD: Organization + WebSite + SoftwareApplication +
    // SoftwareSourceCode. Emitted on every route — truthful, stable
    // facts only (legal name, site URL, logo, social profiles, search
    // action for the site's built-in search UI, product positioning,
    // and a pointer to the source repository). No reviews/ratings or
    // prices — those would only be claimed if and when real data
    // exists.
    {
      tagName: "script",
      attributes: { type: "application/ld+json" },
      innerHTML: JSON.stringify({
        "@context": "https://schema.org",
        "@graph": [
          {
            "@type": "Organization",
            "@id": `${SITE_URL}/#organization`,
            name: "LoraDB",
            url: SITE_URL,
            logo: `${SITE_URL}/img/meta/icon-512.png`,
            sameAs: [
              "https://github.com/lora-db/lora",
              "https://x.com/loradb",
              "https://discord.gg/vUgKb6C8Af",
              "https://linkedin.com/company/loradb",
              "https://medium.com/loradb",
            ],
          },
          {
            "@type": "WebSite",
            "@id": `${SITE_URL}/#website`,
            url: SITE_URL,
            name: "LoraDB",
            description: SITE_DESCRIPTION,
            publisher: { "@id": `${SITE_URL}/#organization` },
            inLanguage: "en",
            potentialAction: {
              "@type": "SearchAction",
              target: {
                "@type": "EntryPoint",
                urlTemplate: `${SITE_URL}/search?q={search_term_string}`,
              },
              "query-input": "required name=search_term_string",
            },
          },
          {
            "@type": "SoftwareApplication",
            "@id": `${SITE_URL}/#software`,
            name: "LoraDB",
            url: SITE_URL,
            applicationCategory: "DeveloperApplication",
            applicationSubCategory: "DatabaseApplication",
            operatingSystem: "Linux, macOS, Windows, Web",
            description: SITE_DESCRIPTION,
            offers: {
              "@type": "Offer",
              price: "0",
              priceCurrency: "USD",
            },
            softwareRequirements:
              "Rust toolchain or one of: Node.js, Python, Go, Ruby, modern web browser (WebAssembly).",
            programmingLanguage: "Rust",
            license: "https://github.com/lora-db/lora/blob/main/LICENSE",
            downloadUrl: "https://github.com/lora-db/lora",
            publisher: { "@id": `${SITE_URL}/#organization` },
            author: { "@id": `${SITE_URL}/#organization` },
          },
          {
            "@type": "SoftwareSourceCode",
            "@id": `${SITE_URL}/#sourcecode`,
            name: "LoraDB",
            description:
              "Source-available repository for LoraDB — a local-first, in-memory property-graph engine in Rust with a Cypher-like query engine and bindings for Node.js, Python, WASM, Go, and Ruby.",
            codeRepository: "https://github.com/lora-db/lora",
            programmingLanguage: ["Rust", "TypeScript", "Python", "Go", "Ruby"],
            license: "https://github.com/lora-db/lora/blob/main/LICENSE",
            author: { "@id": `${SITE_URL}/#organization` },
            isPartOf: { "@id": `${SITE_URL}/#software` },
          },
          // Person entity for the primary blog author. Standalone (not
          // referenced by @id from BlogPosting, which Docusaurus emits
          // independently) — Google's structured-data extractor still
          // matches by name and treats the Person record as the canonical
          // author profile. Boosts E-E-A-T signals on technical content.
          {
            "@type": "Person",
            "@id": `${SITE_URL}/#person-joost`,
            name: "Joost van Berkel",
            url: "https://github.com/zolero",
            jobTitle: "Author, LoraDB",
            worksFor: { "@id": `${SITE_URL}/#organization` },
            sameAs: ["https://github.com/zolero"],
            knowsAbout: [
              "graph databases",
              "property graphs",
              "Cypher query language",
              "embedded databases",
              "Rust",
              "WebAssembly",
              "query planning",
              "write-ahead logging",
            ],
          },
        ],
      }),
    },
  ],
  i18n: {
    defaultLocale: "en",
    locales: ["en"],
  },
  presets: [
    [
      "@docusaurus/preset-classic",
      /** @type {import('@docusaurus/preset-classic').Options} */
      {
        docs: {
          routeBasePath: "/docs",
          sidebarPath: require.resolve("./sidebars.js"),
          editUrl: ({ docPath }) =>
            `https://github.com/lora-db/lora/edit/main/apps/loradb.com/docs/${docPath}`,
          exclude: ["README.md"],
        },
        blog: {
          routeBasePath: "/blog",
          path: "blog",
          blogTitle: "LoraDB Blog",
          blogDescription:
            "Engineering notes, architecture pieces, release notes, and design writing from the LoraDB team.",
          blogSidebarTitle: "Recent posts",
          blogSidebarCount: "ALL",
          postsPerPage: 10,
          showReadingTime: true,
          editUrl: ({ blogPath }) =>
            `https://github.com/lora-db/lora/edit/main/apps/loradb.com/blog/${blogPath}`,
          feedOptions: {
            type: ["rss", "atom"],
            title: "LoraDB Blog",
            description:
              "Engineering notes, architecture pieces, release notes, and design writing from the LoraDB team.",
            copyright: `Copyright © ${new Date().getFullYear()} LoraDB.`,
            language: "en",
          },
          onInlineAuthors: "warn",
          onUntruncatedBlogPosts: "warn",
        },
        sitemap: {
          // Defaults are overridden per-URL by createSitemapItems below.
          // Anything that escapes the tiering still gets a reasonable
          // fallback rather than the legacy flat 0.5/weekly.
          changefreq: "monthly",
          priority: 0.5,
          // Exclude low-value or auto-generated routes:
          //   - /blog/tags/**       blog tag listings (and their paginated forms)
          //   - /docs/tags/**       doc tag listings
          //   - /blog/archive       chronological archive (duplicates /blog)
          //   - /blog/authors       bare author index (thin — list of names)
          //   - /blog/page/**       paginator pages (duplicate /blog)
          //   - /blog/authors/*/page/**
          //                         per-author paginator pages. Page 1 of an
          //                         author (/blog/authors/<slug>) stays
          //                         indexable; page 2+ is the same thin
          //                         paginator shape as /blog/page/N and was
          //                         previously shipping in the sitemap as
          //                         indexable, which is what put
          //                         /blog/authors/loradb/page/2 into
          //                         "crawled, currently not indexed".
          //   - /search             local search UI (needs JS + query string)
          //   - /404(.html)         error page
          //
          // Per-author pages (/blog/authors/<slug>) ARE indexable —
          // each carries a bio paragraph, a Person JSON-LD block, and
          // a list of posts. The thin-page list above only excludes
          // the bare /blog/authors index.
          //
          // These same routes also get <meta name="robots" content="noindex,follow">
          // injected via plugins/noindex — sitemap exclusion alone is not
          // enough because crawlers also follow internal links.
          ignorePatterns: [
            "/blog/tags/**",
            "/docs/tags/**",
            "/blog/archive",
            "/blog/authors",
            "/blog/authors/*/page/**",
            "/blog/page/**",
            "/search",
            "/404",
            "/404.html",
          ],
          // Tier priority + changefreq so the homepage, the docs/blog
          // landings, and pillar reference pages dominate over deep
          // reference and individual blog posts. Without this, every URL
          // ships at the same 0.5 / weekly default — fine for a small
          // site, but it discards the only ranking signal sitemap.xml
          // actually carries between URLs on the same domain.
          createSitemapItems: async (params) => {
            const { defaultCreateSitemapItems, ...rest } = params;
            const items = await defaultCreateSitemapItems(rest);
            return items.map((item) => {
              const tier = tierForUrl(item.url);
              return { ...item, ...tier };
            });
          },
        },
        theme: {
          customCss: [
            require.resolve("modern-normalize/modern-normalize.css"),
            require.resolve("@ionic-internal/ionic-ds/dist/tokens/tokens.css"),
            require.resolve("./src/styles/custom.scss"),
          ],
        },
      },
    ],
  ],
  /** @type {import('@docusaurus/preset-classic').ThemeConfig} */
  themeConfig: {
    // Global fallback metadata. Per-page <Layout title=... description=...>
    // writes title / description / og:title / og:description / twitter:title /
    // twitter:description automatically on top of these.
    metadata: [
      // Global fallbacks. Per-page metadata from <Layout title description>
      // or frontmatter overrides title / description / og:title /
      // og:description / twitter:title / twitter:description automatically.
      // Intentionally omitted here: og:url / twitter:url — we do NOT set a
      // single site-root URL sitewide, because the canonical link is
      // per-page and og:url pointing at the site root on a deep page is
      // misleading to crawlers that use og:url for deduplication.
      { name: "description", content: SITE_DESCRIPTION },
      {
        name: "keywords",
        content:
          "graph database, cypher, embedded database, rust, wasm, node.js, python, go, ruby, knowledge graph, AI agents, LoraDB",
      },
      { name: "author", content: "LoraDB" },
      { name: "robots", content: "index, follow, max-image-preview:large" },
      { property: "og:type", content: "website" },
      { property: "og:site_name", content: "LoraDB" },
      {
        property: "og:title",
        content: "LoraDB — the embedded graph database for connected systems",
      },
      { property: "og:description", content: SITE_DESCRIPTION },
      { property: "og:image", content: `${SITE_URL}/img/meta/og-image.png` },
      {
        property: "og:image:secure_url",
        content: `${SITE_URL}/img/meta/og-image.png`,
      },
      { property: "og:image:type", content: "image/png" },
      { property: "og:image:width", content: "1200" },
      { property: "og:image:height", content: "630" },
      {
        property: "og:image:alt",
        content: "LoraDB — the embedded graph database for connected systems.",
      },
      { property: "og:locale", content: "en_US" },
      { name: "twitter:card", content: "summary_large_image" },
      { name: "twitter:site", content: "@loradb" },
      { name: "twitter:creator", content: "@loradb" },
      { name: "twitter:domain", content: "loradb.com" },
      { name: "twitter:image", content: `${SITE_URL}/img/meta/og-image.png` },
      {
        name: "twitter:image:alt",
        content: "LoraDB — the embedded graph database for connected systems.",
      },
    ],
    colorMode: {
      defaultMode: "light",
    },
    navbar: {
      hideOnScroll: true,
      title: "LoraDB",
      logo: {
        alt: "LoraDB",
        src: "logos/loradb-mark.svg",
        srcDark: "logos/loradb-mark-dark.svg",
        href: "/",
        target: "_self",
        width: 24,
        height: 24,
      },
      items: [
        {
          type: "doc",
          docId: "index",
          label: "Docs",
          position: "left",
        },
        { to: "/blog", label: "Blog", position: "left" },
        { to: "/features", label: "Features", position: "left" },
        { to: "/benchmarks", label: "Benchmarks", position: "left" },
        { type: "search", position: "right" },
        {
          href: "https://discord.gg/vUgKb6C8Af",
          position: "right",
          className: "icon-link icon-link-mask icon-link-discord",
          "aria-label": "Discord",
          target: "_blank",
        },
        {
          href: "https://x.com/loradb",
          position: "right",
          className: "icon-link icon-link-mask icon-link-x",
          "aria-label": "X",
          target: "_blank",
        },
        {
          type: "custom-githubStars",
          position: "right",
          href: "https://github.com/lora-db/lora",
          label: "GitHub repository",
          repo: "lora-db/lora",
        },
        {
          href: "https://play.loradb.com",
          label: "Playground",
          position: "left",
          "aria-label": "Playground",
          target: "_blank",
          rel: "noopener",
        },
      ],
    },
    footer: {
      style: "light",
      // Footer columns mirror the homepage's intent router: Get started,
      // Reference, Operate, Product. Same buckets the marketing pages
      // route into, so the footer is the third copy of the same map.
      links: [
        {
          title: "Get started",
          items: [
            { label: "Installation", to: "/docs/getting-started/installation" },
            { label: "Ten-minute tour", to: "/docs/getting-started/tutorial" },
            { label: "Cheat sheet", to: "/docs/queries/cheat-sheet" },
            { label: "Cookbook", to: "/docs/cookbook" },
          ],
        },
        {
          title: "Reference",
          items: [
            { label: "Queries", to: "/docs/queries" },
            { label: "Functions", to: "/docs/functions/overview" },
            { label: "Data types", to: "/docs/data-types/overview" },
            { label: "Concepts", to: "/docs/concepts/graph-model" },
          ],
        },
        {
          title: "Operate",
          items: [
            { label: "HTTP API", to: "/docs/api/http" },
            { label: "Snapshots", to: "/docs/snapshot" },
            { label: "Limitations", to: "/docs/limitations" },
            { label: "Troubleshooting", to: "/docs/troubleshooting" },
          ],
        },
        {
          title: "Product",
          items: [
            { label: "What is LoraDB", to: "/docs" },
            { label: "Why LoraDB", to: "/docs/why" },
            { label: "Features", to: "/features" },
            { label: "Benchmarks", to: "/benchmarks" },
            { label: "Blog", to: "/blog" },
            { label: "Contact", to: "/contact" },
          ],
        },
        {
          title: "Community",
          items: [
            { label: "GitHub", href: "https://github.com/lora-db/lora" },
            { label: "Discord", href: "https://discord.gg/vUgKb6C8Af" },
            { label: "X", href: "https://x.com/loradb" },
            { label: "LinkedIn", href: "https://linkedin.com/company/loradb" },
            { label: "Medium", href: "https://medium.com/loradb" },
            { label: "Security", to: "/contact#security" },
          ],
        },
      ],
      copyright: `LoraDB · Copyright © ${new Date().getFullYear()}`,
    },
    prism: {
      theme: { plain: {}, styles: [] },
      additionalLanguages: [
        "shell-session",
        "bash",
        "http",
        "diff",
        "json",
        "python",
        "rust",
        "ruby",
        "go",
        "cypher",
      ],
    },
  },
  plugins: [
    "docusaurus-plugin-sass",
    [
      "docusaurus-plugin-module-alias",
      {
        alias: {
          react: path.dirname(require.resolve("react/package.json")),
          "react-dom": path.dirname(require.resolve("react-dom/package.json")),
        },
      },
    ],
    [require.resolve("./plugins/github-stars"), { repo: "lora-db/lora" }],
    [
      require.resolve("./plugins/llms-txt"),
      {
        title: "LoraDB",
        description: SITE_DESCRIPTION,
        intro: llmsTxtManifest.intro,
        sections: llmsTxtManifest.sections,
      },
    ],
    require.resolve("./plugins/noindex"),
    require.resolve("./plugins/twitter-meta"),
    require.resolve("./plugins/canonical-urls"),
  ],
  scripts: [
    {
      src: "https://cloud.umami.is/script.js",
      defer: true,
      "data-website-id": "077ddb23-aded-457c-9f48-f67f39779873",
    },
    // Core Web Vitals beacon. Fires LCP / INP / CLS as Umami custom
    // events at page-hidden so the analytics dashboard surfaces
    // performance regressions per route alongside pageviews. Tiny
    // inline PerformanceObserver code — no library dependency.
    {
      src: "/js/cwv-beacon.js",
      defer: true,
    },
  ],
  customFields: {},
  themes: [
    [
      "@easyops-cn/docusaurus-search-local",
      {
        hashed: "filename",
        indexDocs: true,
        indexPages: true,
        indexBlog: true,
        docsRouteBasePath: "/docs",
        blogRouteBasePath: "/blog",
        highlightSearchTermsOnTargetPage: false,
        searchResultLimits: 8,
        searchResultContextMaxLength: 50,
        explicitSearchResultPath: true,

        removeDefaultStopWordFilter: ["en"],
        // Heading-focused index. Everything here is stripped before lunr sees
        // the page, so search mostly matches h1–h6 text per section.
        ignoreCssSelectors: [
          // Page chrome repeated on every route
          ".navbar",
          ".footer",
          "aside", // docs left sidebar + blog "Recent posts" sidebar
          ".theme-doc-breadcrumbs",
          ".theme-doc-toc-mobile",
          ".theme-doc-toc-desktop",
          '[class*="tableOfContents_"]', // blog post right-side TOC
          ".theme-doc-version-badge",
          ".theme-doc-footer",
          ".theme-edit-this-page",
          ".theme-last-updated",
          ".pagination-nav",
          // Blog post meta
          '[class*="authorCol_"]',
          '[class*="tags_"]',
          // Anchor link (#) injected into each heading — invisible but noisy
          ".hash-link",
          // Body prose — scoped to <main> so it covers .markdown docs/blog,
          // MDX pages (/contact), and custom React pages (/) alike.
          "main p",
          "main ul",
          "main ol",
          "main blockquote",
          "main pre",
          "main table",
          "main figure",
          "main img",
          "main details",
          "main .admonition",
          "main .tabs-container",
        ],
      },
    ],
  ],
};
