const path = require('path');

const SITE_URL = 'https://loradb.com';
const SITE_DESCRIPTION =
  'LoraDB is an embedded, Rust-native graph database with a Cypher-like query engine — built for AI agents, robotics, and context-rich systems that reason over connected data.';

module.exports = {
  title: 'LoraDB',
  tagline: 'The embedded graph database for connected systems.',
  url: SITE_URL,
  baseUrl: '/',
  onBrokenLinks: 'throw',
  onBrokenMarkdownLinks: 'throw',
  onBrokenAnchors: 'throw',
  favicon: 'img/meta/favicon.ico',
  organizationName: 'loradb',
  projectName: 'lora',
  headTags: [
    // Full favicon set — ordered so browsers pick the right one.
    {
      tagName: 'link',
      attributes: { rel: 'icon', type: 'image/x-icon', href: '/img/meta/favicon.ico' },
    },
    {
      tagName: 'link',
      attributes: { rel: 'icon', type: 'image/svg+xml', href: '/img/meta/favicon.svg' },
    },
    {
      tagName: 'link',
      attributes: { rel: 'icon', type: 'image/png', sizes: '16x16', href: '/img/meta/favicon-16x16.png' },
    },
    {
      tagName: 'link',
      attributes: { rel: 'icon', type: 'image/png', sizes: '32x32', href: '/img/meta/favicon-32x32.png' },
    },
    {
      tagName: 'link',
      attributes: { rel: 'icon', type: 'image/png', sizes: '48x48', href: '/img/meta/favicon-48x48.png' },
    },
    {
      tagName: 'link',
      attributes: { rel: 'icon', type: 'image/png', sizes: '96x96', href: '/img/meta/favicon-96x96.png' },
    },
    {
      tagName: 'link',
      attributes: { rel: 'apple-touch-icon', sizes: '180x180', href: '/img/meta/apple-touch-icon.png' },
    },
    // PWA manifest + theme colors
    {
      tagName: 'link',
      attributes: { rel: 'manifest', href: '/site.webmanifest' },
    },
    {
      tagName: 'meta',
      attributes: { name: 'theme-color', content: '#1b6dff' },
    },
    {
      tagName: 'meta',
      attributes: { name: 'apple-mobile-web-app-title', content: 'LoraDB' },
    },
    {
      tagName: 'meta',
      attributes: { name: 'application-name', content: 'LoraDB' },
    },
    {
      tagName: 'meta',
      attributes: { name: 'msapplication-TileColor', content: '#0a0f1f' },
    },
  ],
  i18n: {
    defaultLocale: 'en',
    locales: ['en'],
  },
  presets: [
    [
      '@docusaurus/preset-classic',
      /** @type {import('@docusaurus/preset-classic').Options} */
      {
        docs: {
          routeBasePath: '/docs',
          sidebarPath: require.resolve('./sidebars.js'),
          editUrl: ({ docPath }) =>
            `https://github.com/lora-db/lora/edit/main/apps/loradb.com/docs/${docPath}`,
          exclude: ['README.md'],
        },
        blog: {
          routeBasePath: '/blog',
          path: 'blog',
          blogTitle: 'LoraDB Blog',
          blogDescription:
            'Engineering notes, architecture pieces, release notes, and design writing from the LoraDB team.',
          blogSidebarTitle: 'Recent posts',
          blogSidebarCount: 'ALL',
          postsPerPage: 10,
          showReadingTime: true,
          editUrl: ({ blogPath }) =>
            `https://github.com/lora-db/lora/edit/main/apps/loradb.com/blog/${blogPath}`,
          feedOptions: {
            type: ['rss', 'atom'],
            title: 'LoraDB Blog',
            description:
              'Engineering notes, architecture pieces, release notes, and design writing from the LoraDB team.',
            copyright: `Copyright © ${new Date().getFullYear()} LoraDB.`,
            language: 'en',
          },
          onInlineAuthors: 'warn',
          onUntruncatedBlogPosts: 'warn',
        },
        sitemap: {
          changefreq: 'weekly',
          priority: 0.5,
          ignorePatterns: ['/tags/**'],
        },
        theme: {
          customCss: [
            require.resolve('./node_modules/modern-normalize/modern-normalize.css'),
            require.resolve('./node_modules/@ionic-internal/ionic-ds/dist/tokens/tokens.css'),
            require.resolve('./src/styles/custom.scss'),
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
      { name: 'description', content: SITE_DESCRIPTION },
      { name: 'keywords', content: 'graph database, cypher, embedded database, rust, wasm, node.js, python, go, ruby, knowledge graph, AI agents, LoraDB' },
      { name: 'author', content: 'LoraDB' },
      { name: 'robots', content: 'index, follow, max-image-preview:large' },
      { property: 'og:type', content: 'website' },
      { property: 'og:site_name', content: 'LoraDB' },
      { property: 'og:title', content: 'LoraDB — the embedded graph database for connected systems' },
      { property: 'og:description', content: SITE_DESCRIPTION },
      { property: 'og:url', content: SITE_URL },
      { property: 'og:image', content: `${SITE_URL}/img/meta/og-image.png` },
      { property: 'og:image:secure_url', content: `${SITE_URL}/img/meta/og-image.png` },
      { property: 'og:image:type', content: 'image/png' },
      { property: 'og:image:width', content: '1200' },
      { property: 'og:image:height', content: '630' },
      { property: 'og:image:alt', content: 'LoraDB — the embedded graph database for connected systems.' },
      { property: 'og:locale', content: 'en_US' },
      { name: 'twitter:card', content: 'summary_large_image' },
      { name: 'twitter:site', content: '@loradb' },
      { name: 'twitter:creator', content: '@loradb' },
      { name: 'twitter:domain', content: 'loradb.com' },
      { name: 'twitter:url', content: SITE_URL },
      { name: 'twitter:title', content: 'LoraDB — the embedded graph database for connected systems' },
      { name: 'twitter:description', content: SITE_DESCRIPTION },
      { name: 'twitter:image', content: `${SITE_URL}/img/meta/og-image.png` },
      { name: 'twitter:image:alt', content: 'LoraDB — the embedded graph database for connected systems.' },
    ],
    colorMode: {
      defaultMode: 'light',
    },
    navbar: {
      hideOnScroll: true,
      title: 'LoraDB',
      logo: {
        alt: 'LoraDB',
        src: 'logos/loradb-mark.svg',
        srcDark: 'logos/loradb-mark-dark.svg',
        href: '/',
        target: '_self',
        width: 24,
        height: 24,
      },
      items: [
        {
          type: 'doc',
          docId: 'index',
          label: 'Docs',
          position: 'left',
        },
        { to: '/blog', label: 'Blog', position: 'left' },
        { to: '/features', label: 'Features', position: 'left' },
        { type: 'search', position: 'right' },
        {
          href: 'https://discord.gg/vUgKb6C8Af',
          position: 'right',
          className: 'icon-link icon-link-mask icon-link-discord',
          'aria-label': 'Discord',
          target: '_blank',
        },
        {
          href: 'https://x.com/loradb',
          position: 'right',
          className: 'icon-link icon-link-mask icon-link-x',
          'aria-label': 'X',
          target: '_blank',
        },
        {
          type: 'custom-githubStars',
          position: 'right',
          href: 'https://github.com/lora-db/lora',
          label: 'GitHub repository',
          repo: 'lora-db/lora',
        },
        {
          to: '/playground',
          label: 'Playground',
          position: 'left',
          className: 'navbar-soon',
          'aria-label': 'Playground (coming soon)',
      },
      ],
    },
    footer: {
      style: 'light',
      links: [
        {
          title: 'Docs',
          items: [
            { label: 'Introduction', to: '/docs' },
            { label: 'Installation', to: '/docs/getting-started/installation' },
            { label: 'Queries', to: '/docs/queries' },
            { label: 'Functions', to: '/docs/functions/overview' },
            { label: 'Limitations', to: '/docs/limitations' },
          ],
        },
        {
          title: 'Product',
          items: [
            { label: 'What is LoraDB', to: '/docs' },
            { label: 'Why LoraDB', to: '/docs/why' },
            { label: 'Blog', to: '/blog' },
            { label: 'Contact', to: '/contact' },
          ],
        },
        {
          title: 'Community',
          items: [
            { label: 'GitHub', href: 'https://github.com/lora-db/lora' },
            { label: 'Discord', href: 'https://discord.gg/vUgKb6C8Af' },
            { label: 'X', href: 'https://x.com/loradb' },
            { label: 'LinkedIn', href: 'https://linkedin.com/company/loradb' },
            { label: 'Medium', href: 'https://medium.com/loradb' },
          ],
        },
        {
          title: 'More',
          items: [
            { label: 'loradb.com', href: 'https://loradb.com' },
            { label: 'Security', to: '/contact#security' },
          ],
        },
      ],
      copyright: `LoraDB · Copyright © ${new Date().getFullYear()}`,
    },
    prism: {
      theme: { plain: {}, styles: [] },
      additionalLanguages: ['shell-session', 'http', 'diff', 'json', 'rust', 'cypher'],
    },
  },
  plugins: [
    'docusaurus-plugin-sass',
    [
      'docusaurus-plugin-module-alias',
      {
        alias: {
          react: path.resolve(__dirname, './node_modules/react'),
          'react-dom': path.resolve(__dirname, './node_modules/react-dom'),
        },
      },
    ],
    [
      require.resolve('./plugins/github-stars'),
      { repo: 'lora-db/lora' },
    ],
  ],
  customFields: {},
  themes: [
    [
      '@easyops-cn/docusaurus-search-local',
      {
        hashed: 'filename',
        indexDocs: true,
        indexPages: true,
        indexBlog: true,
        docsRouteBasePath: '/docs',
        blogRouteBasePath: '/blog',
        highlightSearchTermsOnTargetPage: false,
        searchResultLimits: 8,
        searchResultContextMaxLength: 50,
        explicitSearchResultPath: true,

        removeDefaultStopWordFilter: ['en'],
        // Heading-focused index. Everything here is stripped before lunr sees
        // the page, so search mostly matches h1–h6 text per section.
        ignoreCssSelectors: [
          // Page chrome repeated on every route
          '.navbar',
          '.footer',
          'aside', // docs left sidebar + blog "Recent posts" sidebar
          '.theme-doc-breadcrumbs',
          '.theme-doc-toc-mobile',
          '.theme-doc-toc-desktop',
          '[class*="tableOfContents_"]', // blog post right-side TOC
          '.theme-doc-version-badge',
          '.theme-doc-footer',
          '.theme-edit-this-page',
          '.theme-last-updated',
          '.pagination-nav',
          // Blog post meta
          '[class*="authorCol_"]',
          '[class*="tags_"]',
          // Anchor link (#) injected into each heading — invisible but noisy
          '.hash-link',
          // Body prose — scoped to <main> so it covers .markdown docs/blog,
          // MDX pages (/contact), and custom React pages (/) alike.
          'main p',
          'main ul',
          'main ol',
          'main blockquote',
          'main pre',
          'main table',
          'main figure',
          'main img',
          'main details',
          'main .admonition',
          'main .tabs-container',
        ],
      },
    ],
  ],
};
