# LoraDB Playground

In-browser IDE for the LoraDB graph database. Runs entirely client-side —
the WASM engine, the Cypher editor, the graph canvas, and the result grid
all live in your browser tab. No backend, no server actions, no `/api`
routes. Data persists locally via IndexedDB and `localStorage`.

## Development

```bash
yarn workspace @loradb/play dev
```

Open <http://localhost:3000>.

Other useful scripts:

```bash
yarn workspace @loradb/play typecheck   # strict tsc, --noEmit
yarn workspace @loradb/play lint        # next lint
yarn workspace @loradb/play build       # static export → apps/play.loradb.com/out
```

If you change any of the workspace dependencies
(`@loradb/lora-wasm`, `@loradb/lora-query`, `@loradb/lora-graph-canvas`),
rebuild them first so the Next bundler picks up fresh artefacts:

```bash
yarn workspace @loradb/lora-wasm build
yarn workspace @loradb/lora-query build
yarn workspace @loradb/lora-graph-canvas build
```

## Architecture summary

- `app/` — Next 15 App Router shell, Mantine providers, root layout, and
  the playground page.
- `app/_components/` — Dockview panel layout, query editor wrapper,
  graph canvas wrapper, result grid, etc.
- `lib/` — client-only utilities (history, saved-queries, settings,
  snapshot import/export, worker plumbing for `@loradb/lora-wasm`).
- `shims/empty.mjs` — webpack alias target used by `next.config.mjs` to
  drop `loader-node` from the client bundle (see config for rationale).
- `public/` — static assets served verbatim, plus `_headers` and
  `_redirects` consumed by Cloudflare Pages at deploy time.

The build is a fully static export: `yarn build` writes
`apps/play.loradb.com/out/` with self-contained HTML, JS, CSS, and WASM
assets that any object store / CDN can serve as flat files.

## Production deploy

### Recommended host: Cloudflare Pages

The Docusaurus site at `apps/loradb.com` already occupies the single
GitHub Pages site allowed per repo, so the playground deploys to
Cloudflare Pages instead. The `.github/workflows/play-loradb.yml`
workflow builds the static export on every push to `main` that touches
the app or its workspace deps and uploads the result to Cloudflare via
`cloudflare/wrangler-action@v3`.

One-time setup:

1. **Create the Pages project.** In the Cloudflare dashboard go to
   _Workers & Pages → Create → Pages → Create using Direct Upload_ and
   name it `play-loradb`. Do not connect a Git source — this repo
   deploys via GitHub Actions, not Cloudflare's built-in builder.
   (If you _do_ connect to Git, disable auto-builds; otherwise CF will
   try to run `next build` itself and fight the workflow.)
2. **Attach the custom domain.** In the project page open
   _Custom domains → Set up a custom domain_ and enter
   `play.loradb.com`. Cloudflare prints a CNAME target like
   `play-loradb.pages.dev`. Add a `CNAME` record at your DNS provider
   for `play.loradb.com` pointing at that target, then wait for the
   custom-domain status in the dashboard to flip to "Active". TLS is
   issued automatically.
3. **Provision repo secrets.** In GitHub go to _Settings → Secrets and
   variables → Actions → New repository secret_ and add:
   - `CLOUDFLARE_API_TOKEN` — create at
     <https://dash.cloudflare.com/profile/api-tokens> with the
     _Pages — Edit_ permission scope (Account → Cloudflare Pages →
     Edit). No other permissions are needed.
   - `CLOUDFLARE_ACCOUNT_ID` — visible in the right sidebar of any
     Cloudflare dashboard page.
   The deploy job runs a pre-flight check that fails with a clear
   `::error::` message if either secret is missing.
4. **Trigger the first deploy.** Either push a change under
   `apps/play.loradb.com/` to `main`, or run the workflow manually:
   ```bash
   gh workflow run play-loradb
   ```
   Once it completes the site is live at <https://play.loradb.com>.

`wrangler.toml` at the app root pins `pages_build_output_dir = "out"`
and the project name `play-loradb`, so you can also run an ad-hoc
deploy from a workstation:

```bash
yarn workspace @loradb/play build
npx wrangler pages deploy apps/play.loradb.com/out \
  --project-name=play-loradb --branch=main
```

### Alternative: Vercel

Vercel is fully supported as a static host for the same `out/`
directory. Workflow:

1. Install the Vercel CLI globally (`npm i -g vercel`).
2. From `apps/play.loradb.com`, run `vercel link --project play-loradb`
   to bind the directory to a new Vercel project.
3. In the project settings on vercel.com configure:
   - Framework preset: **Next.js**.
   - Build command: `yarn workspace @loradb/play build`.
   - Install command: `yarn install --immutable` (run from the repo
     root — the project root must therefore be the repo root, not
     `apps/play.loradb.com`).
   - Output directory: `apps/play.loradb.com/out`.
4. Add the custom domain `play.loradb.com` under _Domains_ and update
   the DNS `CNAME` to the Vercel target that the dashboard prints.
5. If you want CI-driven deploys (rather than Vercel's own Git
   integration), add a workflow that runs `vercel deploy --prebuilt`
   after building. Not included by default — the Cloudflare workflow
   in this repo is the canonical path.

There is **no** committed workflow for Vercel; this section is provided
purely as a fallback.

### Caveats

- The app is fully static. There are no API routes, no server actions,
  no edge functions, no ISR — it _must_ be hosted as flat files behind
  a CDN.
- The WASM payloads must be served with
  `Content-Type: application/wasm`. The bundled `public/_headers` file
  handles that on Cloudflare Pages; replicate the rule if you host
  elsewhere.
- IndexedDB and `localStorage` are origin-scoped. Saved queries,
  snapshots, settings, and history are local to the user's browser
  and the active origin — switching origins (e.g.
  `play.loradb.com` ↔ a staging URL) does not migrate user data.
- `@loradb/lora-wasm` ships its own Web Worker. Cross-origin isolation
  (`COOP`/`COEP`) is **not** required, but caching the worker and its
  WASM payload as `immutable` (handled by `public/_headers`) keeps
  reloads fast.

## Build verification locally

```bash
yarn workspace @loradb/play build
# Static export lands in apps/play.loradb.com/out
npx serve apps/play.loradb.com/out -l 5000
# Visit http://localhost:5000
```

The static export is genuinely static — there is no `next start`. If
something only works under `next dev` and breaks under `next build`, it
is almost certainly an SSR-vs-static-export issue (e.g. a top-level
`window` reference inside a module imported by a Server Component) and
needs a `"use client"` boundary.

## Known issues

- **Hash-route reload via direct URL.** Deep links that encode state in
  the URL hash (`#q=...`) refresh cleanly because `_redirects` falls
  back to `index.html`; deep links that rely on a path that Next did
  not statically render at build time will 404. Keep new state in the
  hash, not the pathname, until we revisit routing.
- **WASM mime on non-Cloudflare hosts.** Vercel and most CDNs serve
  `.wasm` correctly out of the box; bespoke object-store setups (e.g.
  raw S3 + CloudFront) need a manual MIME override or the browser
  refuses to instantiate via streaming.
- **Workspace dep rebuilds.** The CI workflow rebuilds `lora-wasm`,
  `lora-query`, and `lora-graph-canvas` before building `@loradb/play`.
  Locally, after pulling, run the workspace builds shown in the
  Development section or `next build` may load stale `dist/` output.
