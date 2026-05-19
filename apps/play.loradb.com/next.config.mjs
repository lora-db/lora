// LoraDB Playground — Next.js config
//
// This app is shipped as a fully static export (`output: 'export'`) and
// hosted as flat files behind a CDN (Cloudflare Pages). Everything runs in
// the user's browser tab:
//   * the Cypher editor and parser (WASM via @loradb/lora-query),
//   * the graph database engine (WASM via @loradb/lora-wasm, executed
//     inside a Web Worker so the main thread stays responsive),
//   * the WebGPU/WebGL canvas (@loradb/lora-graph-canvas),
//   * the result grid, history, and saved-query persistence (IndexedDB).
//
// There are no server actions, no `/api` routes, and no Node-runtime
// dependencies — so a static export is both sufficient and strictly
// preferable to the Node/Edge runtimes (no cold starts, no per-request
// compute cost, trivial caching).
//
// The webpack tweaks below are kept untouched because they configure
// async-WebAssembly imports for the bundle and shim out the `loader-node`
// path of @loradb/lora-wasm (which references `node:module`) plus the
// missing `react-responsive-carousel` peer of glide-data-grid. None of
// that interacts with the static-export pipeline.

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

/**
 * Read the version of the `@loradb/lora-wasm` workspace dep so we can
 * bake it into the static bundle. The runtime can then surface "wasm
 * v0.11.2" without round-tripping to the binary — handy for spotting
 * stale deploys at a glance. We try the workspace source first
 * (canonical in this monorepo) and fall back to the resolved node-
 * modules entry so the config still works if someone trims the repo.
 */
function readLoraWasmVersion() {
  const candidates = [
    path.join(__dirname, "../../crates/bindings/lora-wasm/package.json"),
    path.join(__dirname, "node_modules/@loradb/lora-wasm/package.json"),
  ];
  for (const p of candidates) {
    try {
      const raw = fs.readFileSync(p, "utf8");
      const v = JSON.parse(raw).version;
      if (typeof v === "string" && v.length > 0) return v;
    } catch {
      // try next candidate
    }
  }
  return "unknown";
}

const loraWasmVersion = readLoraWasmVersion();

/** @type {import('next').NextConfig} */
const nextConfig = {
  reactStrictMode: true,
  output: "export",
  trailingSlash: true,
  images: { unoptimized: true },
  env: {
    NEXT_PUBLIC_LORA_WASM_VERSION: loraWasmVersion,
  },
  transpilePackages: [
    "@loradb/lora-graph-canvas",
    "@loradb/lora-query",
    "@loradb/lora-wasm",
  ],
  webpack(config, { isServer, webpack }) {
    config.experiments = {
      ...config.experiments,
      asyncWebAssembly: true,
      topLevelAwait: true,
      layers: true,
    };
    // Browser bundle workarounds (TODO: fix upstream and remove):
    //   * `@loradb/lora-wasm` ships a single entry that statically
    //     pulls `loader-node.js`, which references `node:module`. For
    //     the client compile we rewrite loader-node to an empty shim
    //     so the chunk can build; the runtime path always goes through
    //     the Web Worker / `pkg-bundler` route via `createDatabase`.
    //   * `react-responsive-carousel` is a peer dep of glide-data-grid
    //     that's never installed; the image-overlay editor that needs
    //     it isn't reachable from the cell kinds we render.
    //   * `marked` is also a peer dep of glide-data-grid, pulled in only
    //     by `markdown-div.js` (the markdown-cell renderer) which we
    //     never mount. We rewrite that single module to the empty shim
    //     so the whole `marked` import chain is dead at build time and
    //     we can drop the runtime dependency.
    const empty = path.join(__dirname, "shims/empty.mjs");
    config.resolve = config.resolve || {};
    config.resolve.alias = {
      ...(config.resolve.alias || {}),
      "react-responsive-carousel": empty,
    };
    if (!isServer) {
      config.plugins = config.plugins || [];
      config.plugins.push(
        new webpack.NormalModuleReplacementPlugin(
          /lora-wasm[\\/]dist[\\/]loader-node\.js$/,
          empty,
        ),
        new webpack.NormalModuleReplacementPlugin(
          /glide-data-grid[\\/]dist[\\/](esm|cjs)[\\/]internal[\\/]markdown-div[\\/]markdown-div\.js$/,
          empty,
        ),
      );
    }
    return config;
  },
};

export default nextConfig;
