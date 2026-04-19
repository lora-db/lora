# LoraDB Docs

Documentation site for **LoraDB** — an in-memory graph database with a
Cypher-like query engine, written in Rust.

Built with [Docusaurus](https://docusaurus.io/) v3.

## Develop

```bash
npm install
npm start
```

The dev server runs at <http://localhost:3000/docs/>.

## Build

```bash
npm run build
npm run serve   # preview the production build
```

## Project layout

```
docs/                          Markdown content
  index.md                     Introduction
  getting-started/             Installation + first query
  concepts/                    Nodes, relationships, properties
  queries.md                   Cypher-style query examples
  api.md                       (placeholder)
  architecture.md              (placeholder)

src/styles/                    SCSS theme (Infima overrides + design tokens)
static/                        Static assets (favicon, github icon, fonts)

docusaurus.config.js           Site + navbar + theme config
sidebars.js                    Sidebar layout
```

The site extends the classic Docusaurus theme and the Ionic design-system
tokens package (`@ionic-internal/ionic-ds`) for its color palette; the Ionic
_content_, plugins, and framework-specific pages have been removed.

## Deployment

The site is deployed to **GitHub Pages** at <https://loradb.com> from the
`main` branch by the [`loradb-docs`](../../.github/workflows/loradb-docs.yml)
GitHub Actions workflow. Each push to `main` that touches `apps/loradb.com/**`
(or the workflow itself) rebuilds the site and publishes the contents of
`apps/loradb.com/build` as a Pages artifact via the official
`actions/upload-pages-artifact` + `actions/deploy-pages` actions. No manual
`docusaurus deploy` / `gh-pages` branch is used.

### Repository settings (one-time)

In the GitHub repository settings:

1. **Settings → Pages → Build and deployment → Source**: set to
   **GitHub Actions**.
2. **Settings → Pages → Custom domain**: set to `loradb.com` and enable
   **Enforce HTTPS** once GitHub has issued the certificate.
3. **Settings → Environments → `github-pages`**: restrict deployments to the
   `main` branch (the workflow already targets this environment).

### Custom domain (CNAME)

The custom domain is pinned by [`static/CNAME`](./static/CNAME), which contains
a single line:

```
loradb.com
```

Docusaurus copies everything in `static/` into the build output verbatim, so
`build/CNAME` is produced on every build and included in the Pages artifact.
This prevents GitHub Pages from dropping the custom-domain setting when a new
deployment is published. Do not remove this file.

### DNS requirements for `loradb.com`

Because `loradb.com` is an **apex (root) domain**, it must be pointed at
GitHub Pages with `A` records — `CNAME` is not valid at the apex. Configure
the following records at the DNS registrar for `loradb.com`:

| Type    | Host / Name | Value                                      | Notes                          |
| ------- | ----------- | ------------------------------------------ | ------------------------------ |
| `A`     | `@`         | `185.199.108.153`                          | GitHub Pages apex              |
| `A`     | `@`         | `185.199.109.153`                          | GitHub Pages apex              |
| `A`     | `@`         | `185.199.110.153`                          | GitHub Pages apex              |
| `A`     | `@`         | `185.199.111.153`                          | GitHub Pages apex              |
| `AAAA`  | `@`         | `2606:50c0:8000::153`                      | optional (IPv6)                |
| `AAAA`  | `@`         | `2606:50c0:8001::153`                      | optional (IPv6)                |
| `AAAA`  | `@`         | `2606:50c0:8002::153`                      | optional (IPv6)                |
| `AAAA`  | `@`         | `2606:50c0:8003::153`                      | optional (IPv6)                |
| `CNAME` | `www`       | `<gh-org-or-user>.github.io.`              | optional, for `www` → apex     |

Notes:

- Use the current GitHub Pages IP list from the
  [GitHub Pages docs](https://docs.github.com/pages/configuring-a-custom-domain-for-your-github-pages-site/managing-a-custom-domain-for-your-github-pages-site#configuring-an-apex-domain)
  — the four `A` addresses above are the published apex IPs and are the ones
  to set if they are still current at configuration time.
- The `www` `CNAME` is only required if the site should be reachable at
  `www.loradb.com` as well. GitHub Pages will automatically redirect the `www`
  host to the apex (or vice versa) when both the DNS record and the Pages
  custom-domain setting are in place. The primary canonical domain remains
  `loradb.com`.
- After DNS propagates, **Settings → Pages → Custom domain** must show a
  green check and HTTPS must be enforced.

### Local development

Local development is unaffected by the Pages configuration — `npm start`
still serves at `http://localhost:3000/` independent of `url` / `baseUrl`.
