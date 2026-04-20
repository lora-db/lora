const path = require('path');

module.exports = {
  title: 'LoraDB',
  tagline: 'A graph database with a Cypher-like query engine.',
  url: 'https://loradb.com',
  baseUrl: '/',
  onBrokenLinks: 'warn',
  onBrokenMarkdownLinks: 'warn',
  favicon: 'img/meta/favicon-96x96.png',
  organizationName: 'loradb',
  projectName: 'lora',
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
    metadata: [
      { name: 'og:type', content: 'website' },
      { name: 'og:site_name', content: 'LoraDB Docs' },
      { name: 'twitter:card', content: 'summary_large_image' },
      { name: 'twitter:domain', content: 'loradb.com' },
    ],
    colorMode: {
      defaultMode: 'light',
    },
    navbar: {
      hideOnScroll: true,
      title: 'LoraDB',
      logo: {
        alt: 'LoraDB',
        src: 'logos/loradb-bird.svg',
        srcDark: 'logos/loradb-bird-dark.svg',
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
        { to: '/about', label: 'About', position: 'left' },
        { to: '/why', label: 'Why', position: 'left' },
        { type: 'search', position: 'right' },
        {
          href: 'https://discord.gg/loradb',
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
          href: 'https://github.com/lora-db/lora',
          position: 'right',
          className: 'icon-link icon-link-mask icon-link-github',
          'aria-label': 'GitHub repository',
          target: '_blank',
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
            { label: 'About', to: '/about' },
            { label: 'Why LoraDB', to: '/why' },
            { label: 'Blog', to: '/blog' },
            { label: 'Contact', to: '/contact' },
          ],
        },
        {
          title: 'Community',
          items: [
            { label: 'GitHub', href: 'https://github.com/lora-db/lora' },
            { label: 'Discord', href: 'https://discord.gg/loradb' },
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
  ],
  customFields: {},
  themes: [
    [
      '@easyops-cn/docusaurus-search-local',
      {
        hashed: true,
        indexDocs: true,
        indexPages: true,
        indexBlog: true,
        docsRouteBasePath: '/docs',
        blogRouteBasePath: '/blog',
        highlightSearchTermsOnTargetPage: true,
        searchResultLimits: 8,
        searchResultContextMaxLength: 50,
        explicitSearchResultPath: true,
      },
    ],
  ],
};
